"""A messier file: what a dialect, a sheet and a column can declare, each on and off and by each
route a read takes, and what comes back beside the values: a fault's detail, a cell's origin and a
batch's own list of faults. What every option reads is the corpus's; these say that each is the one
doing it, that a stream reads as promptly as it can, and how long what's handed out stays right.
"""

from __future__ import annotations

import asyncio
import gc
import io
import os
import sys
import threading
from collections.abc import Callable, Iterable
from pathlib import Path
from typing import Any

import pytest
from test_corpus import _AsyncDribble, _corpus_dir, _Dribble, _run

from hypertabular import (
    _native,
    CastFailure,
    Column,
    DateOptions,
    DateOrder,
    DelimitedReader,
    Detail,
    Dialect,
    Encoding,
    Fault,
    LongRows,
    MonthNames,
    Origin,
    Problem,
    Range,
    SheetOptions,
    ShortRows,
    Success,
    TabularError,
    TabularFailure,
    Workbook,
)

WORKBOOKS = _corpus_dir() / "workbook"
TEXT = [Column.text(0)]
HEADLESS: dict[str, Any] = {"has_header": False}


def _said(batches: Iterable[Any]) -> list[Any]:
    """A read's rows as their verdicts and origins, then its faults with their rows counted across
    it: a stream may deliver the same rows in more batches."""
    rows: list[Any] = []
    faults: list[Any] = []
    for batch in batches:
        faults += [
            (problem.row + len(rows), problem.reason, problem.detail) for problem in batch.faults()
        ]
        for row in range(batch.rows):
            rows.append(
                [
                    (batch.get(column, row), batch.origin(column, row))
                    for column in range(len(batch.columns))
                ]
            )
    return [*rows, faults]


def _read_each_way(
    data: bytes, dialect: Dialect, plan: list[Column], tmp_path: Path
) -> list[list[Any]]:
    """What a read says from bytes, a file, a file object and a stream, each with what it failed
    with, if it did; all of them the same or the test fails."""

    def drained(reader: DelimitedReader) -> list[Any]:
        batches = []
        try:
            for batch in reader:
                batches.append(batch)
        except TabularError as error:
            return [*_said(batches), (error.kind, error.record, error.line, error.byte)]
        return _said(batches)

    async def streamed(source: Any) -> list[Any]:
        reader = await DelimitedReader.open_async(source, dialect, plan)
        batches = []
        try:
            async for batch in reader:
                batches.append(batch)
        except TabularError as error:
            return [*_said(batches), (error.kind, error.record, error.line, error.byte)]
        return _said(batches)

    path = tmp_path / "input.csv"
    path.write_bytes(data)
    first = drained(DelimitedReader(data, dialect, plan))
    assert drained(DelimitedReader.open(path, dialect, plan)) == first
    assert drained(DelimitedReader(_Dribble(data), dialect, plan)) == first
    assert _run(streamed(_AsyncDribble(data))) == first
    assert _run(streamed(data)) == first
    return first


def _texts(said: list[Any]) -> list[Any]:
    """The values of a read's rows, a fault as its reason, then what it failed with."""
    rows = []
    for row in said:
        if isinstance(row, tuple):
            rows.append(row[0])
        elif row and len(row[0]) == 2:
            rows.append(
                [
                    verdict.value if isinstance(verdict, Success) else verdict.reason
                    for verdict, _ in row
                ]
            )
    return rows


M, E = CastFailure.MALFORMED, CastFailure.EMPTY

# Each option, its input and plan, what that reads with it and without. The "without" is the same
# dialect with that one option off.
_OPTIONS = {
    "skip": (b"title\na\n7\n", {"skip": 1}, [Column.i32(0)], [[7]], [[M], [7]]),
    "header_row": (
        b"x\na\n7\n",
        {"header_row": 1},
        [Column.i32(0)],
        [[7]],
        [[M], [7]],
    ),
    "short_rows": (
        b"a,b\n7\n",
        {"short_rows": ShortRows.PAD},
        [Column.i32(0), Column.i32(1)],
        [[7, E]],
        [TabularFailure.COLUMN_COUNT],
    ),
    "long_rows_report": (
        b"a\n7,8\n",
        {"long_rows": LongRows.REPORT},
        [Column.i32(0)],
        [[7]],
        [TabularFailure.COLUMN_COUNT],
    ),
    "long_rows_truncate": (
        b"a\n7,8\n",
        {"long_rows": LongRows.TRUNCATE},
        [Column.i32(0)],
        [[7]],
        [TabularFailure.COLUMN_COUNT],
    ),
    "windows_1252": (
        b"a\n\x80\n",
        {"encoding": Encoding.WINDOWS_1252},
        TEXT,
        [["€"]],
        [[M]],
    ),
    "latin_1": (b"a\n\xe9\n", {"encoding": Encoding.LATIN_1}, TEXT, [["é"]], [[M]]),
    "utf_16le": (
        "a\n€\n".encode("utf-16-le"),
        {"encoding": Encoding.UTF_16LE},
        TEXT,
        [["€"]],
        [TabularFailure.ENCODING],
    ),
    "tokens": (
        b"a\nNA\n7\n",
        {"tokens": ("NA",)},
        [Column.i32(0)],
        [[E], [7]],
        [[M], [7]],
    ),
}


@pytest.mark.parametrize("option", _OPTIONS)
def test_each_option_does_its_own_work_by_every_route(option: str, tmp_path: Path) -> None:
    """Each option does its own work by every route."""
    data, on, plan, with_it, without = _OPTIONS[option]
    assert _texts(_read_each_way(data, Dialect(",", **on), plan, tmp_path)) == with_it
    off = _texts(_read_each_way(data, Dialect(","), plan, tmp_path))
    if option == "utf_16le":
        assert off[-1] != with_it[-1]
    else:
        assert off == without


def test_a_dialect_refuses_what_it_can_not_be_told() -> None:
    """A dialect refuses what it can not be told."""
    for declared in ({"skip": -1}, {"header_row": -1}):
        with pytest.raises(ValueError):
            Dialect(",", **declared)
    with pytest.raises((TypeError, ValueError)):
        Dialect(",", tokens="NA")
    with pytest.raises(ValueError):
        Dialect(",", encoding=99)  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        Dialect(",", short_rows=99)  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        Dialect(",", long_rows=99)  # type: ignore[arg-type]


def test_a_header_row_past_the_input_has_no_header() -> None:
    """A header row past the input has no header."""
    reader = DelimitedReader(b"x\n", Dialect(",", header_row=5))
    assert reader.header is None or reader.header == ()


def test_the_lines_a_batch_says_count_what_was_passed_over() -> None:
    """The lines a batch says count what was passed over."""
    (batch,) = list(
        DelimitedReader(b"title\nx\na\n7\n8\n", Dialect(",", skip=1, header_row=1), [Column.i32(0)])
    )
    assert [batch.line(row) for row in range(batch.rows)] == [4, 5]
    assert batch.column(0).values.tolist() == [7, 8]


def test_a_column_s_own_tokens_replace_the_dialect_s() -> None:
    """A column's own tokens replace the dialect s."""
    data = b"a,b,c\nNA,NA,-\n"
    dialect = Dialect(",", tokens=("NA",))
    plan = [Column.i32(0), Column.i32(1).with_tokens(["-"]), Column.i32(2).with_tokens(["-"])]
    (batch,) = list(DelimitedReader(data, dialect, plan))
    assert batch.origin(0, 0) is Origin.TOKEN and batch.column(0)[0].reason is E
    assert batch.column(1)[0].reason is M
    assert batch.origin(2, 0) is Origin.TOKEN
    # An empty list says this column has none, the dialect's included.
    plan = [Column.i32(0).with_tokens([]), Column.i32(1)]
    (batch,) = list(DelimitedReader(data, dialect, plan))
    assert batch.column(0)[0].reason is M and batch.origin(0, 0) is Origin.CELL
    assert batch.origin(1, 0) is Origin.TOKEN
    # Not said is not none.
    assert Column.i32(0).tokens is None and Column.i32(0).with_tokens([]).tokens == ()
    with pytest.raises(TypeError):
        Column.i32(0).with_tokens("NA")


def test_a_quoted_cell_is_never_a_token() -> None:
    """A quoted cell is never a token."""
    (batch,) = list(
        DelimitedReader(
            b'a,b\nNA,"NA"\n', Dialect(",", tokens=("NA",)), [Column.text(0), Column.text(1)]
        )
    )
    assert batch.origin(0, 0) is Origin.TOKEN and batch.column(0)[0].reason is E
    assert batch.origin(1, 0) is Origin.CELL and batch.column(1)[0] == Success("NA")


def test_a_column_s_date_options_and_month_names_are_read() -> None:
    """A column's date options and month names are read."""
    compact = DateOptions(DateOptions.COMPACT_DATE)
    data = b"d\n20260207\n"
    plain, declared = Column.date(0), Column.date(0).with_date_options(compact)
    assert list(DelimitedReader(data, Dialect.CSV, [plain]))[0].column(0)[0].reason is M
    assert isinstance(list(DelimitedReader(data, Dialect.CSV, [declared]))[0].column(0)[0], Success)
    assert declared.date_options == compact and declared.months is None

    names = [
        "janv",
        "fevr",
        "mars",
        "avr",
        "mai",
        "juin",
        "juil",
        "aout",
        "sept",
        "oct",
        "nov",
        "dec",
    ]
    months = MonthNames(list(enumerate(names, 1)))
    data = b"d\n2-fevr-2026\n"
    plain = Column.date_ordered(0, DateOrder.DAY_MONTH_YEAR)
    assert list(DelimitedReader(data, Dialect.CSV, [plain]))[0].column(0)[0].reason is M
    named = plain.with_date_options(DateOptions(), months)
    (batch,) = list(DelimitedReader(data, Dialect.CSV, [named]))
    day = batch.column(0)[0].value
    assert (day.year, day.month, day.day) == (2026, 2, 2)


def test_a_detail_says_what_failed_on_the_fault_and_the_list() -> None:
    """A detail says what failed on the fault and the list."""
    plan = [Column.i32(0), Column.u8(1)]
    (batch,) = list(DelimitedReader(b"a,b\n1x,300\n", Dialect.CSV, plan))
    malformed, out_of_range = batch.column(0)[0], batch.column(1)[0]
    assert isinstance(malformed, Fault) and isinstance(out_of_range, Fault)
    assert malformed.detail is Detail.UNEXPECTED_CHARACTER
    assert malformed.detail_code == int(Detail.UNEXPECTED_CHARACTER)
    assert out_of_range.reason is CastFailure.OUT_OF_RANGE
    (first, second) = batch.faults()
    assert (first.detail, first.detail_code) == (malformed.detail, malformed.detail_code)
    assert (second.detail, second.reason) == (out_of_range.detail, CastFailure.OUT_OF_RANGE)
    assert isinstance(first, Problem)
    # The same by each way there is to a cell.
    assert batch.column(0).faults() == [(0, malformed)]
    assert batch.get(0, 0) == malformed
    assert next(iter(batch)).get(0) == malformed


def test_with_notes_off_nothing_is_said_of_a_fault_but_its_reason() -> None:
    """With notes off nothing is said of a fault but its reason."""
    plan = [Column.i32(0)]
    data = b"a\n1x\n"

    async def streamed() -> Any:
        reader = await DelimitedReader.open_async(
            _AsyncDribble(data), Dialect.CSV, plan, notes=False
        )
        return await reader.read_async()

    for batch in (
        next(iter(DelimitedReader(data, Dialect.CSV, plan, notes=False))),
        _run(streamed()),
    ):
        fault = batch.column(0)[0]
        assert fault.reason is M and fault.detail is None and fault.detail_code is None
        (problem,) = batch.faults()
        assert problem.detail is None and problem.detail_code is None
        assert problem.origin is Origin.CELL and batch.origin(0, 0) is Origin.CELL


def test_notes_off_a_token_is_an_empty_cell_and_says_no_more() -> None:
    """Notes off a token is an empty cell and says no more."""
    (batch,) = list(
        DelimitedReader(b"a\nNA\n", Dialect(",", tokens=("NA",)), [Column.i32(0)], notes=False)
    )
    assert batch.column(0)[0].reason is E and batch.origin(0, 0) is Origin.CELL
    assert batch.faults() == []


def test_a_read_with_notes_off_costs_a_batch_nothing_it_does_not_ask_for() -> None:
    """Values are what they were with notes on, in a column with no faults as well."""
    data = b"a,b\n1,x\n2,y\n"
    plan = [Column.i32(0), Column.text(1)]
    on = list(DelimitedReader(data, Dialect.CSV, plan))
    off = list(DelimitedReader(data, Dialect.CSV, plan, notes=False))
    assert on[0].column(0).values.tolist() == off[0].column(0).values.tolist()
    assert on[0].column(1).values == off[0].column(1).values
    assert on[0].faults() == off[0].faults() == []


def test_a_padded_cell_is_a_blank_and_a_long_row_is_on_the_list() -> None:
    """A padded cell is a blank and a long row is on the list."""
    data = b"a,b\n1\n2,3,4\n5,6\n"
    dialect = Dialect(",", short_rows=ShortRows.PAD, long_rows=LongRows.REPORT)
    (batch,) = list(DelimitedReader(data, dialect, [Column.i32(0), Column.i32(1)]))
    assert batch.origin(1, 0) is Origin.PADDED and batch.column(1)[0].reason is E
    (long_row,) = batch.faults()
    assert (long_row.row, long_row.plan_column, long_row.source_column) == (1, None, None)
    assert (long_row.expected, long_row.found, long_row.line) == (2, 3, 3)
    assert long_row.reason is None and long_row.origin is Origin.CELL


def test_a_problem_names_its_source_column_as_well_as_its_plan_s() -> None:
    """A problem names its source column as well as its plan s."""
    (batch,) = list(DelimitedReader(b"a,b,c\n1,2,x\n", Dialect.CSV, [Column.i32(2)]))
    (problem,) = batch.faults()
    assert (problem.plan_column, problem.source_column, problem.offset, problem.length) == (
        0,
        2,
        0,
        1,
    )


def test_a_range_is_a_value() -> None:
    """A range is a value."""
    made = Range(4, 2, 12, 3)
    assert (made.first_row, made.first_column, made.last_row, made.last_column) == (4, 2, 12, 3)
    assert made == Range.parse("B4:C12") and hash(made) == hash(Range.parse("B4:C12"))
    assert made != Range.parse("B4:C13")
    assert repr(made) == "Range(4, 2, 12, 3)"
    assert Range() == Range(0, 0, 0, 0)
    for text in ("", "A", "1", "A1", "B4:", "C12:B4", "A0:B1"):
        with pytest.raises(ValueError):
            Range.parse(text)
    with pytest.raises(OverflowError):
        Range(-1)
    # 0 is no limit, so a first with no last is every row from there, and a last before it is refused.
    assert Range(5).last_row == 0
    with pytest.raises(ValueError):
        Range(5, 0, 4, 0)


def test_a_sheet_takes_each_of_its_options() -> None:
    """A sheet takes each of its options."""
    book = Workbook.open(WORKBOOKS / "ranges.xlsx")
    whole = book.sheet(0, SheetOptions()).suggest()
    ranged = book.sheet(0, SheetOptions(range=Range.parse("B4:C12")))
    found = ranged.suggest()
    assert len(found.plan) == 2 and len(whole.plan) != 2
    # A range's plan counts from its first column, and its faults say where in the sheet they are.
    (name,) = [case for case in ("id",) if case == found.report[0].name]
    assert name == "id"
    assert ranged.header == ("id", found.report[1].name)
    plan = [Column.i32(0), Column.text(1)]
    for batch in book.sheet(0, SheetOptions(range=Range.parse("B4:C12")), plan):
        for problem in batch.faults():
            assert problem.source_column == 1 and problem.plan_column == 0
            assert batch.line(problem.row) > 4


def test_a_sheet_s_skip_header_row_and_tokens_change_what_it_reads() -> None:
    """A sheet's skip header row and tokens change what it reads."""
    path = WORKBOOKS / "basic.xlsx"
    book = Workbook.open(path)
    plan = [Column.text(1)]
    plain = book.sheet(0, SheetOptions(), plan)
    assert plain.header is not None
    rows = [cell for batch in plain for cell in batch.column(0).values]
    skipped = book.sheet(0, SheetOptions(skip=1), plan)
    assert skipped.header is not None and skipped.header[1] == rows[0]
    assert [cell for batch in skipped for cell in batch.column(0).values] == rows[1:]
    headed = book.sheet(0, SheetOptions(header_row=1), plan)
    assert headed.header == skipped.header
    assert [cell for batch in headed for cell in batch.column(0).values] == rows[1:]
    token = rows[0]
    assert token is not None
    tokened = book.sheet(0, SheetOptions(tokens=(token,)), plan)
    (batch, *_) = tokened
    assert batch.column(0)[0].reason is E and batch.origin(0, 0) is Origin.TOKEN
    # Notes off, a sheet has no origin to say.
    quiet = book.sheet(0, SheetOptions(tokens=(token,), notes=False), plan)
    (batch, *_) = quiet
    assert batch.column(0)[0].reason is E and batch.origin(0, 0) is Origin.CELL
    for refused in ({"skip": -1}, {"header_row": -1}):
        with pytest.raises(ValueError):
            SheetOptions(**refused)


def test_skip_empty_rows_off_keeps_every_row() -> None:
    """A row with nothing but formulas with no cached value is taken for empty unless asked."""
    book = Workbook.open(WORKBOOKS / "uncached.xlsx")
    plan = [Column.text(0), Column.i32(1)]
    kept = list(book.sheet(0, SheetOptions(True, False), plan))
    dropped = list(book.sheet(0, SheetOptions(True, True), plan))
    assert sum(batch.rows for batch in kept) == sum(batch.rows for batch in dropped) + 2
    origins = [batch.origin(1, row) for batch in kept for row in range(batch.rows)]
    assert Origin.UNCACHED in origins
    # With notes off, the list can't say a formula has no cached value.
    (batch, *_) = book.sheet(0, SheetOptions(True, False, notes=False), plan)
    assert all(problem.origin is not Origin.UNCACHED for problem in batch.faults())
    assert batch.origin(1, 0) is Origin.CELL
    (noted, *_) = kept
    assert any(problem.origin is Origin.UNCACHED for problem in noted.faults())


@pytest.mark.parametrize(
    ("encoding", "codec"),
    [(Encoding.WINDOWS_1252, "cp1252"), (Encoding.UTF_16LE, "utf-16-le")],
)
def test_a_record_too_long_under_an_encoding_is_a_failure(encoding: Encoding, codec: str) -> None:
    """The ceiling is of the decoded text, and what came before it is delivered first."""
    dialect = Dialect(",", encoding=encoding)
    data = ("a\n1\n" + "x" * 500 + "\n").encode(codec)
    for source in (data, io.BytesIO(data)):
        reader = DelimitedReader(source, dialect, [Column.text(0)], batch_rows=1, buffer_bytes=8)
        _native._limits(reader, 100, 100)
        first = reader.read()
        assert first is not None and first.text(0, 0) == "1"
        with pytest.raises(TabularError) as failed:
            reader.read()
        assert failed.value.kind is TabularFailure.ROW_TOO_LONG


def test_an_encoding_failure_comes_after_every_intact_row_and_says_the_file_s_byte() -> None:
    """An encoding failure comes after every intact row and says the file's byte."""
    dialect = Dialect(",", encoding=Encoding.UTF_16LE)
    data = "a\n1\n2\n".encode("utf-16-le") + b"\x00\xdc\n\x00"
    rows: list[int] = []
    with pytest.raises(TabularError) as failed:
        for batch in DelimitedReader(data, dialect, [Column.i32(0)]):
            rows += batch.column(0).values.tolist()
    assert rows == [1, 2]
    assert failed.value.kind is TabularFailure.ENCODING
    assert (failed.value.record, failed.value.line, failed.value.byte) == (3, 4, 12)

    async def main() -> None:
        seen: list[int] = []
        reader = await DelimitedReader.open_async(_AsyncDribble(data), dialect, [Column.i32(0)])
        with pytest.raises(TabularError) as failed:
            async for batch in reader:
                seen += batch.column(0).values.tolist()
        assert seen == [1, 2]
        assert failed.value.byte == 12

    _run(main())


def test_a_failure_in_the_header_is_opening_s_and_stays_said() -> None:
    """A failure in the header is opening's and stays said."""
    dialect = Dialect(",", encoding=Encoding.UTF_16LE)
    data = b"\xff\xfea\x00,\x00\x00\xd8"
    with pytest.raises(TabularError) as failed:
        DelimitedReader(data, dialect)
    assert failed.value.byte == 6

    async def main() -> None:
        with pytest.raises(TabularError) as failed:
            await DelimitedReader.open_async(_AsyncDribble(data), dialect)
        assert failed.value.byte == 6

    _run(main())


# --- a stream that stalls ------------------------------------------------------------------------


class _Stalling:
    """A stream that hands over what it has once, and has nothing else until it's let go: what a
    pipe or a socket does. A second read blocks; the test fails if the call under test waits for it."""

    def __init__(self, *parts: bytes) -> None:
        self.parts = list(parts)
        self.reads = 0
        self.go = threading.Event()

    def read(self, size: int) -> bytes:
        self.reads += 1
        if self.parts:
            return self.parts.pop(0)
        self.go.wait(30)
        return b""


def _timed(call: Callable[[], Any], release: Callable[[], None]) -> Any:
    """``call``'s answer, on a thread, or fails if it hasn't one within five seconds, letting the
    stream go so that the thread ends."""
    answer: list[Any] = []
    worker = threading.Thread(target=lambda: answer.append(call()))
    worker.start()
    worker.join(5)
    stalled = worker.is_alive()
    if stalled:
        release()
    worker.join(30)
    assert not stalled, "it waited on the stream with finished rows in hand"
    return answer[0]


_ENCODED = [
    pytest.param(Encoding.UTF8, "utf-8", id="utf-8"),
    pytest.param(Encoding.WINDOWS_1252, "cp1252", id="windows-1252"),
    pytest.param(Encoding.UTF_16LE, "utf-16-le", id="utf-16le"),
]


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide has no threads")
@pytest.mark.parametrize(("encoding", "codec"), _ENCODED)
def test_a_stalled_stream_s_finished_rows_reach_the_caller(encoding: Encoding, codec: str) -> None:
    """A stalled stream's finished rows reach the caller."""
    stream = _Stalling("a\n1\n2\n".encode(codec), "3\n".encode(codec))
    dialect = Dialect(",", encoding=encoding)
    plan = [Column.i32(0)]
    reader = _timed(lambda: DelimitedReader(stream, dialect, plan), stream.go.set)
    assert reader.header == ("a",)
    assert stream.reads == 1
    # What the first transfer finished goes without a second, or the stream's next transfer is the
    # one more a read takes, and no third, which would wait.
    batch = _timed(reader.read, stream.go.set)
    assert batch.column(0).values.tolist()[:2] == [1, 2]
    assert stream.reads <= 2
    seen = batch.rows
    while seen < 3:
        batch = _timed(reader.read, stream.go.set)
        assert batch is not None
        seen += batch.rows
    stream.go.set()
    assert _timed(reader.read, stream.go.set) is None


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide has no threads")
def test_a_pipe_s_finished_rows_reach_the_caller() -> None:
    """A buffered file object's ``read(n)`` waits for all of ``n``: a pipe is why it's read1 here."""
    made, kept = os.pipe()
    with os.fdopen(made, "rb") as readable:
        try:
            os.write(kept, b"a\n1\n2\n")
            release = lambda: os.close(os.dup(kept)) or os.write(kept, b"x" * 300_000)  # noqa: E731
            reader = _timed(
                lambda: DelimitedReader(readable, Dialect.CSV, [Column.i32(0)]), release
            )
            batch = _timed(reader.read, release)
            assert batch.column(0).values.tolist() == [1, 2]
        finally:
            os.close(kept)


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide has no threads")
@pytest.mark.parametrize(("encoding", "codec"), _ENCODED)
def test_a_stalled_async_stream_s_finished_rows_reach_the_caller(
    encoding: Encoding, codec: str
) -> None:
    """A stalled async stream's finished rows reach the caller."""

    class Stalling:
        def __init__(self) -> None:
            self.parts = ["a\n".encode(codec), "1\n2\n".encode(codec)]
            self.later = ["3\n".encode(codec), b""]
            self.more = asyncio.Event()

        async def read(self, size: int) -> bytes:
            if self.parts:
                return self.parts.pop(0)
            await self.more.wait()
            self.more.clear()
            return self.later.pop(0)

    async def main() -> None:
        stream = Stalling()
        dialect = Dialect(",", encoding=encoding)
        reader = await asyncio.wait_for(
            DelimitedReader.open_async(stream, dialect, [Column.i32(0)]), 5
        )
        batch = await asyncio.wait_for(reader.read_async(), 5)
        assert batch is not None and batch.column(0).values.tolist() == [1, 2]
        # The next read waits on the stream, and a cancelled wait loses nothing.
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(reader.read_async(), 0.05)
        stream.more.set()
        batch = await asyncio.wait_for(reader.read_async(), 5)
        assert batch is not None and batch.column(0).values.tolist() == [3]
        stream.more.set()
        assert await asyncio.wait_for(reader.read_async(), 5) is None

    _run(main())


# --- how long what's handed out stays right --------------------------------------------------------


def test_a_batch_outlives_its_reader_and_the_next_read() -> None:
    """A batch here is a copy: its verdicts, raw text, notes and faults list are its own."""
    data = b"a,b\n1x,y\n" + b"".join(b"%d,z\n" % row for row in range(3)) + b"2x,w\n"
    with DelimitedReader(
        data, Dialect.CSV, [Column.i32(0), Column.text(1)], batch_rows=1
    ) as reader:
        first = reader.read()
        assert first is not None
        said = first.faults()
        (problem,) = said
        for _ in range(3):
            reader.read()
    # The reader's moved on, and gone.
    del reader
    gc.collect()
    assert first.faults() == said
    assert first.get(0, 0).detail is Detail.UNEXPECTED_CHARACTER
    assert first.raw(0, 0) == b"1x" and first.text(1, 0) == "y"
    assert first.origin(0, 0) is Origin.CELL
    assert problem.row == 0 and problem.line == 2


def test_a_faults_list_is_the_caller_s_own() -> None:
    """A faults list is the caller's own."""
    (batch,) = list(DelimitedReader(b"a\nx\ny\n", Dialect.CSV, [Column.i32(0)]))
    said = batch.faults()
    assert len(said) == 2
    said.clear()
    assert len(batch.faults()) == 2 and batch.faults() is not batch.faults()


def test_a_sheet_s_batch_outlives_its_sheet_with_its_notes() -> None:
    """A sheet's batch outlives its sheet with its notes."""
    plan = [Column.i32(1)]
    book = Workbook.open(WORKBOOKS / "ranges.xlsx")
    sheet = book.sheet(0, SheetOptions(), plan)
    batch = next(iter(sheet))
    said = batch.faults()
    del sheet, book
    gc.collect()
    assert batch.faults() == said


def test_a_reader_after_close_says_so_and_a_batch_still_reads() -> None:
    """A reader after close says so and a batch still reads."""
    reader = DelimitedReader(b"a\nx\n", Dialect.CSV, [Column.i32(0)])
    batch = reader.read()
    reader.close()
    with pytest.raises(ValueError, match="closed"):
        reader.read()
    assert batch is not None and batch.faults()[0].reason is M


def test_a_reader_refuses_a_bad_source_and_notes_is_its_own_keyword(tmp_path: Path) -> None:
    """A reader refuses a bad source and notes is its own keyword."""
    with pytest.raises(TypeError):
        DelimitedReader(b"a\n", Dialect.CSV, None, 5)  # type: ignore[misc]
    with pytest.raises(OSError):
        DelimitedReader.open(tmp_path / "nothing.csv", Dialect.CSV, notes=False)
