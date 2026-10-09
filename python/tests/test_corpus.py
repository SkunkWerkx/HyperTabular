"""Replays the shared conformance corpus (``corpus/delimited.json`` at the repository root)
— the same file the Rust and C# bindings replay — through this binding: from memory, from
file objects read through buffers too small for a record, from a file object that hands
back less than it was asked for, and from a path, in batches of one row, two, and many.
How the input is cut up is the binding's business and must not change the answer. And
again header first — opened without a plan, the plan bound once the header is read — and
asynchronously, from an asyncio stream that hands back a few bytes a read.

Every cell is held to the corpus three ways: as the verdict ``column[row]`` gives, as the
whole-column views (``values``, ``verdicts``, ``faults()``) give it, and — HyperCast being
the judge — as ``hypercast.cast_*`` gives it for the cell's own text.
"""

from __future__ import annotations

import asyncio
import datetime as dt
import io
import json
import struct
import uuid as uuidlib
from decimal import Decimal
from pathlib import Path
from typing import Any, Awaitable, Callable

import hypercast
import pytest

from hypertabular import (
    CastFailure,
    Column,
    DateOrder,
    Batch,
    DelimitedReader,
    Dialect,
    Door,
    ExcelEpoch,
    Fault,
    Header,
    NumFormat,
    Success,
    TabularError,
    TabularFailure,
    UnixPrecision,
)


def _corpus_dir() -> Path:
    # tests/corpus is a symlink to the root corpus/, so the vectors travel with this
    # directory when it is copied without the rest of the checkout. Where the link is
    # checked out as a plain file (Windows without core.symlinks), or dangles (the forge's
    # Alpine suite copies tests/ and corpus/ side by side), it is not a directory and the
    # walk carries on up to the root's own corpus/.
    for parent in Path(__file__).resolve().parents:
        candidate = parent / "corpus"
        if (candidate / "delimited.json").is_file():
            return candidate
    raise FileNotFoundError("corpus directory not found")


CORPUS: list[dict[str, Any]] = json.loads(
    (_corpus_dir() / "delimited.json").read_text(encoding="utf-8")
)

_REASON = {
    "empty": CastFailure.EMPTY,
    "malformed": CastFailure.MALFORMED,
    "out_of_range": CastFailure.OUT_OF_RANGE,
}

_EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


def _format_of(entry: dict[str, Any]) -> NumFormat:
    fmt = entry.get("format")
    if fmt is None:
        return NumFormat.INVARIANT
    return NumFormat(fmt["decimal_sep"], fmt["group_sep"], fmt["flags"], fmt.get("currency", ""))


def _column_of(entry: dict[str, Any]) -> Column:
    # A plan entry names its door as HyperCast's corpus names its types, and Column's
    # factories carry the same names.
    door, ordinal = entry["door"], entry["ordinal"]
    factory = getattr(Column, door)
    if door == "unix":
        return factory(ordinal, UnixPrecision(entry["precision"]))
    if door == "excel_serial":
        return factory(ordinal, ExcelEpoch(entry["epoch"]))
    if door in ("date_ordered", "datetime"):
        return factory(ordinal, DateOrder(entry["order"]))
    if "format" in entry:
        return factory(ordinal, _format_of(entry))
    return factory(ordinal)


def _time_of(nanos: int) -> tuple[int, int, int, int]:
    second, nano = divmod(nanos, 1_000_000_000)
    return second // 3600, second % 3600 // 60, second % 60, nano // 1000


def _expected_value(column: Column, cell: dict[str, Any]) -> Any:
    """The Python value the corpus says a cell that cast must hold."""
    match column.door:
        case Door.F32:
            return struct.unpack("f", struct.pack("f", cell["value"]))[0]
        case Door.DECIMAL:
            return Decimal(cell["value"])
        case Door.UUID:
            return uuidlib.UUID(hex=cell["value"])
        case Door.TIMESTAMP | Door.UNIX | Door.EXCEL_SERIAL:
            return _EPOCH + dt.timedelta(
                seconds=cell["seconds"], microseconds=cell["nanos"] // 1000
            )
        case Door.DATE | Door.DATE_ORDERED:
            return dt.date(cell["year"], cell["month"], cell["day"])
        case Door.DATETIME:
            return dt.datetime(
                cell["year"], cell["month"], cell["day"], *_time_of(cell["nanos_of_day"])
            )
        case Door.TIME:
            return dt.time(*_time_of(cell["nanos"]))
        case Door.DURATION:
            nanos = cell["nanos"]
            micros = nanos // 1000 if nanos >= 0 else -((-nanos) // 1000)
            return dt.timedelta(seconds=cell["seconds"], microseconds=micros)
        case Door.TEXT:
            return cell["text"]
        case _:
            return cell["value"]


def _judge(column: Column, raw: bytes) -> Any:
    """What HyperCast's own package says of a cell's text through the column's door."""
    match column.door:
        case Door.BOOL | Door.UUID | Door.TIMESTAMP | Door.TIME | Door.DURATION | Door.DATE:
            return getattr(hypercast, f"cast_{column.door.name.lower()}")(raw)
        case Door.UNIX:
            return hypercast.cast_unix(raw, column.declared)
        case Door.EXCEL_SERIAL:
            return hypercast.cast_excel_serial(raw, column.declared)
        case Door.DATE_ORDERED:
            return hypercast.cast_date(raw, column.declared)
        case Door.DATETIME:
            return hypercast.cast_datetime(raw, column.declared)
        case _:
            return getattr(hypercast, f"cast_{column.door.name.lower()}")(raw, column.format)


def _assert_cell(
    label: str,
    column: Column,
    data: Any,
    row: int,
    expected: dict[str, Any],
    judged: bool = True,
) -> None:
    """Holds one cell of a batch's column to what the corpus says of it — and, when
    ``judged``, to what HyperCast's own package says of its raw text (a typed workbook cell
    that cast has none: the door converted the stored value directly)."""
    verdict = data[row]
    offset, length, reason = data.verdicts[row, 0], data.verdicts[row, 1], data.verdicts[row, 2]
    raw = data.raw(row)
    expect = expected["expect"]

    # The union consumption idiom in action — match over HyperCast's two case types.
    match verdict:
        case Success(value):
            assert expect == "ok", f"{label}: unexpectedly cast to {value!r}"
            want = _expected_value(column, expected)
            assert value == want and type(value) is type(want), f"{label}: {value!r}, want {want!r}"
            assert reason == 0, label
            if column.door is Door.DECIMAL:
                # Equality alone would let "1.10" pass for "1.1": pin the canonical scale
                # the core produced, digit for digit.
                sign, digits, exponent = value.as_tuple()
                assert sign == int(expected["negative"]), label
                assert digits == tuple(int(d) for d in str(int(expected["magnitude"]))), label
                assert exponent == -expected["scale"], label
            if column.door is Door.TEXT:
                assert raw == expected["text"].encode("utf-8"), label
        case Fault(reason=why, offset=at, length=span):
            assert expect in _REASON, f"{label}: expected {expect} but faulted with {verdict!r}"
            assert why is _REASON[expect], f"{label}: {verdict!r}"
            assert (at, span, int(why)) == (offset, length, reason), label
            if "fault" in expected:
                assert [at, span] == expected["fault"], f"{label}: fault span"
                # The cell's own text is still to hand, for the diagnostic a fault deserves.
                assert raw.decode("utf-8") == expected["raw"], label
            if expect == "empty":
                assert raw == b"", label
        case other:  # pragma: no cover - the union is closed
            raise AssertionError(f"{label}: produced no case: {other!r}")

    # HyperCast is the judge: the cell is what its own package makes of the same text.
    if judged and column.door is not Door.TEXT:
        assert verdict == _judge(column, raw), f"{label}: disagrees with hypercast on {raw!r}"


def _assert_column(label: str, column: Column, data: Any, expected: list[dict[str, Any]]) -> None:
    """Holds a column's whole-batch views to the same cells."""
    rows = len(expected)
    assert len(data) == rows and data.column is column, label
    verdicts = list(data)
    assert verdicts == [data[row] for row in range(rows)], label

    values = data.values
    assert len(values) == rows, label
    for row, verdict in enumerate(verdicts):
        match verdict:
            case Success(value):
                assert values[row] == value, f"{label}, row {row}"
            case Fault():
                # A primitive column holds zero where a cell did not cast; a list holds None.
                assert values[row] == (0 if isinstance(values, memoryview) else None), (
                    f"{label}, row {row}"
                )

    faults = [(row, verdict) for row, verdict in enumerate(verdicts) if isinstance(verdict, Fault)]
    assert data.faults() == faults, label
    assert data.fault_count == len(faults), label
    assert data.verdicts.shape == (rows, 3) and data.verdicts.format == "I", label
    assert [reason for _, _, reason in data.verdicts.tolist()] == [
        int(verdict.reason) if isinstance(verdict, Fault) else 0 for verdict in verdicts
    ], label


def _assert_failure(label: str, actual: TabularError | None, case: dict[str, Any]) -> None:
    expected = case.get("failure")
    if expected is None:
        assert actual is None, f"{label}: {actual}"
        return
    assert actual is not None, f"{label}: no failure raised"
    # The corpus names a kind as the core does: the member's name, in lower case.
    assert actual.kind is TabularFailure[expected["kind"].upper()], label
    assert (actual.record, actual.line, actual.byte) == (
        expected["record"],
        expected["line"],
        expected["byte"],
    ), label
    if "expected" in expected:
        assert (actual.expected, actual.found) == (expected["expected"], expected["found"]), label


def _assert_rows(label: str, plan: list[Column], batch: Batch) -> None:
    """Holds the batch's row view to the batch: every row, in order, each member the
    batch's own call with the row's index."""
    rows = list(batch)
    assert len(rows) == batch.rows and [row.index for row in rows] == list(range(batch.rows))
    assert [row.index for row in batch.iter_rows()] == list(range(batch.rows)), label
    for row in rows:
        assert row.batch is batch and len(row) == len(plan), label
        assert row.line == batch.line(row.index), label
        assert batch.row(row.index).index == row.index, label
        for index, column in enumerate(plan):
            assert row.get(index) == batch.get(index, row.index), label
            assert row.get(index) == batch.column(index)[row.index], label
            assert row.raw(index) == batch.raw(index, row.index), label
            if column.door is Door.TEXT:
                assert row.text(index) == batch.text(index, row.index), label
                assert row.text(index) == batch.column(index).values[row.index], label


def _assert_batch(
    label: str,
    plan: list[Column],
    rows: list[list[dict[str, Any]]],
    seen: int,
    batch: Batch,
    batch_rows: int,
) -> int:
    """Holds one batch to the corpus rows from ``seen`` on; returns the rows seen after it."""
    assert 1 <= batch.rows <= batch_rows and len(batch) == batch.rows, label
    assert seen + batch.rows <= len(rows), f"{label}: more rows than the corpus lists"
    columns = batch.columns
    assert len(columns) == len(plan), label
    for index, (column, data) in enumerate(zip(plan, columns)):
        expected = [row[index] for row in rows[seen : seen + batch.rows]]
        for row, cell in enumerate(expected):
            _assert_cell(f"{label}, row {seen + row}, column {index}", column, data, row, cell)
            assert batch.raw(index, row) == data.raw(row), label
        _assert_column(f"{label}, column {index}", column, data, expected)
    _assert_rows(label, plan, batch)
    return seen + batch.rows


def _assert_unbound(label: str, case: dict[str, Any], reader: DelimitedReader) -> None:
    """A reader opened without a plan: its header read, nothing to read rows through yet."""
    expected = case["header"]
    assert reader.header == (None if expected is None else tuple(expected)), label
    assert expected is None or isinstance(reader.header, Header), label
    if expected:
        assert reader.column_count == len(expected), label
    assert not reader.is_bound and reader.plan == (), label
    # Reading before a plan is bound is the caller's mistake, and not a lasting one.
    with pytest.raises(RuntimeError, match="bind"):
        reader.read()


def _header_first(
    case: dict[str, Any], open_reader: Callable[[Dialect, list[Column] | None], DelimitedReader]
) -> Callable[[Dialect, list[Column]], DelimitedReader]:
    """``open_reader`` as the header-first flow uses it: no plan, the header read, then the
    plan bound."""

    def opened(dialect: Dialect, plan: list[Column]) -> DelimitedReader:
        reader = open_reader(dialect, None)
        _assert_unbound(case["name"], case, reader)
        reader.bind(plan)
        assert reader.is_bound
        with pytest.raises(RuntimeError, match="bound once"):
            reader.bind(plan)
        return reader

    return opened


def _replay(
    label: str,
    case: dict[str, Any],
    batch_rows: int,
    open_reader: Callable[[Dialect, list[Column]], DelimitedReader],
) -> None:
    dialect = Dialect(**case["dialect"])
    plan = [_column_of(entry) for entry in case["plan"]]
    rows = case["rows"]

    reader = open_reader(dialect, plan)
    assert reader.header == (None if case["header"] is None else tuple(case["header"])), label
    assert reader.plan == tuple(plan), label

    seen = 0
    failure = None
    try:
        for batch in reader:
            seen = _assert_batch(label, plan, rows, seen, batch, batch_rows)
    except TabularError as error:
        failure = error
        # A structural failure is final: the same one, again.
        with pytest.raises(TabularError) as again:
            reader.read()
        assert again.value is error, label
    else:
        assert reader.read() is None, label
    finally:
        # Lets go of the file a path was opened as, pass or fail.
        reader.close()
    assert seen == len(rows), f"{label}: {seen} rows, the corpus lists {len(rows)}"
    _assert_failure(label, failure, case)


class _Dribble:
    """A binary file object that hands back less than it was asked for: one to three
    bytes a read, whatever the size. A reader has to take a short read for what it is."""

    def __init__(self, data: bytes) -> None:
        self._data = data
        self._at = 0
        self._reads = 0

    def read(self, size: int = -1) -> bytes:
        self._reads += 1
        take = min(size, self._reads % 3 + 1) if size > 0 else 0
        chunk = self._data[self._at : self._at + take]
        self._at += len(chunk)
        return chunk


_BATCH_ROWS = (1, 2, 1024)


def test_the_corpus_is_the_whole_contract():
    """The delimited corpus is present and exercises every door."""
    assert len(CORPUS) >= 30
    doors = {entry["door"] for case in CORPUS for entry in case["plan"]}
    assert doors == {door.name.lower() for door in Door}, "a door the corpus never opens"


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_from_memory(case: dict[str, Any], batch_rows: int) -> None:
    """Every corpus case reads the same from bytes in memory, at every batch size."""
    data = case["input"].encode("utf-8")
    _replay(
        f"{case['name']} (memory, {batch_rows} rows a batch)",
        case,
        batch_rows,
        lambda dialect, plan: DelimitedReader(data, dialect, plan, batch_rows=batch_rows),
    )


@pytest.mark.parametrize("buffer_bytes", (1, 5, 64, DelimitedReader.DEFAULT_BUFFER_BYTES))
@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_from_a_file_object(
    case: dict[str, Any], batch_rows: int, buffer_bytes: int
) -> None:
    """Every corpus case reads the same streamed from a file object through a small buffer."""
    data = case["input"].encode("utf-8")
    _replay(
        f"{case['name']} (stream through {buffer_bytes} bytes, {batch_rows} rows a batch)",
        case,
        batch_rows,
        lambda dialect, plan: DelimitedReader(
            io.BytesIO(data), dialect, plan, batch_rows=batch_rows, buffer_bytes=buffer_bytes
        ),
    )


@pytest.mark.parametrize("buffer_bytes", (2, DelimitedReader.DEFAULT_BUFFER_BYTES))
@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_from_short_reads(case: dict[str, Any], batch_rows: int, buffer_bytes: int) -> None:
    """Every corpus case reads the same from a stream that returns a few bytes per read."""
    data = case["input"].encode("utf-8")
    _replay(
        f"{case['name']} (short reads into {buffer_bytes} bytes, {batch_rows} rows a batch)",
        case,
        batch_rows,
        lambda dialect, plan: DelimitedReader(
            _Dribble(data), dialect, plan, batch_rows=batch_rows, buffer_bytes=buffer_bytes
        ),
    )


@pytest.mark.parametrize("buffer_bytes", (3, DelimitedReader.DEFAULT_BUFFER_BYTES))
@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_from_a_path(
    case: dict[str, Any], batch_rows: int, buffer_bytes: int, tmp_path: Path
) -> None:
    """Every corpus case reads the same from a path on disk."""
    path = tmp_path / "case.csv"
    path.write_bytes(case["input"].encode("utf-8"))
    _replay(
        f"{case['name']} (file through {buffer_bytes} bytes, {batch_rows} rows a batch)",
        case,
        batch_rows,
        lambda dialect, plan: DelimitedReader.open(
            path, dialect, plan, batch_rows=batch_rows, buffer_bytes=buffer_bytes
        ),
    )


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_header_first_from_memory(case: dict[str, Any], batch_rows: int) -> None:
    """Every corpus case reads the same opened without a plan and bound after the header."""
    data = case["input"].encode("utf-8")
    _replay(
        f"{case['name']} (header first, memory, {batch_rows} rows a batch)",
        case,
        batch_rows,
        _header_first(
            case,
            lambda dialect, plan: DelimitedReader(data, dialect, plan, batch_rows=batch_rows),
        ),
    )


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_header_first_from_short_reads(case: dict[str, Any], batch_rows: int) -> None:
    """Header first, from a stream that returns a few bytes per read into a tiny buffer."""
    data = case["input"].encode("utf-8")
    _replay(
        f"{case['name']} (header first, short reads, {batch_rows} rows a batch)",
        case,
        batch_rows,
        _header_first(
            case,
            lambda dialect, plan: DelimitedReader(
                _Dribble(data), dialect, plan, batch_rows=batch_rows, buffer_bytes=2
            ),
        ),
    )


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_header_first_from_a_path(
    case: dict[str, Any], batch_rows: int, tmp_path: Path
) -> None:
    """Header first, from a path on disk."""
    path = tmp_path / "case.csv"
    path.write_bytes(case["input"].encode("utf-8"))
    _replay(
        f"{case['name']} (header first, file, {batch_rows} rows a batch)",
        case,
        batch_rows,
        _header_first(
            case,
            lambda dialect, plan: DelimitedReader.open(
                path, dialect, plan, batch_rows=batch_rows, buffer_bytes=3
            ),
        ),
    )


class _AsyncDribble:
    """An asynchronous stream, shaped as ``asyncio.StreamReader``'s ``read``, that hands back
    one to five bytes a read, and lets the event loop run before each."""

    def __init__(self, data: bytes) -> None:
        self._data = data
        self._at = 0
        self._reads = 0

    async def read(self, size: int = -1) -> bytes:
        await asyncio.sleep(0)
        self._reads += 1
        take = min(size, self._reads % 5 + 1) if size > 0 else len(self._data)
        chunk = self._data[self._at : self._at + take]
        self._at += len(chunk)
        return chunk


async def _replay_async(
    label: str,
    case: dict[str, Any],
    batch_rows: int,
    open_reader: Callable[[Dialect, list[Column] | None], Awaitable[DelimitedReader]],
    header_first: bool,
) -> None:
    dialect = Dialect(**case["dialect"])
    plan = [_column_of(entry) for entry in case["plan"]]
    rows = case["rows"]

    if header_first:
        reader = await open_reader(dialect, None)
        _assert_unbound(label, case, reader)
        reader.bind(plan)
    else:
        reader = await open_reader(dialect, plan)
    assert reader.header == (None if case["header"] is None else tuple(case["header"])), label

    seen = 0
    failure = None
    try:
        first = await reader.read_async()
        if first is not None:
            seen = _assert_batch(label, plan, rows, seen, first, batch_rows)
            async for batch in reader:
                seen = _assert_batch(label, plan, rows, seen, batch, batch_rows)
    except TabularError as error:
        failure = error
        with pytest.raises(TabularError) as again:
            await reader.read_async()
        assert again.value is error, label
    else:
        assert await reader.read_async() is None, label
    assert seen == len(rows), f"{label}: {seen} rows, the corpus lists {len(rows)}"
    _assert_failure(label, failure, case)


@pytest.mark.parametrize("header_first", (False, True), ids=("planned", "header-first"))
@pytest.mark.parametrize("buffer_bytes", (2, DelimitedReader.DEFAULT_BUFFER_BYTES))
@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_asynchronously(
    case: dict[str, Any], batch_rows: int, buffer_bytes: int, header_first: bool
) -> None:
    """Every corpus case reads the same from an asyncio stream of short reads."""
    data = case["input"].encode("utf-8")

    async def open_reader(dialect: Dialect, plan: list[Column] | None) -> DelimitedReader:
        return await DelimitedReader.open_async(
            _AsyncDribble(data), dialect, plan, batch_rows=batch_rows, buffer_bytes=buffer_bytes
        )

    asyncio.run(
        _replay_async(
            f"{case['name']} (async, {buffer_bytes} bytes, {batch_rows} rows a batch)",
            case,
            batch_rows,
            open_reader,
            header_first,
        )
    )
