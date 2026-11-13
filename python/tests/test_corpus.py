"""Replays the shared conformance corpus (``corpus/delimited.json`` at the repository root)
— the same file the Rust and C# bindings replay — through this binding: from memory, from
file objects read through buffers too small for a record, from a file object that hands
back less than it was asked for, and from a path, in batches of one row, two, and many.
How the input is cut up is the binding's business and must not change the answer. And
again header first — opened without a plan, the plan bound once the header is read — and
asynchronously, from an asyncio stream that hands back a few bytes a read.

Every cell is held to the corpus three ways: as the verdict ``column[row]`` gives, as the
whole-column views (``values``, ``verdicts``, ``faults()``) give it, and — HyperCast being
the judge — as ``hypercast.cast_*`` gives it for the cell's own text. Each batch's faults list is
held to the case's ``problems``, and a case's ``suggest`` to what a suggestion says. A key this
replay doesn't know fails it: ``test_every_corpus_key_is_read``.
"""

from __future__ import annotations

import asyncio
import dataclasses
import datetime as dt
import io
import json
import struct
import sys
import uuid as uuidlib
from decimal import Decimal
from pathlib import Path
from collections.abc import Coroutine
from typing import Any, Awaitable, Callable, TypeVar

import hypercast
import pytest

from hypertabular import (
    CastFailure,
    Checked,
    Column,
    Constraints,
    DateOptions,
    DateOrder,
    Batch,
    DelimitedReader,
    Dialect,
    Door,
    Encoding,
    ExcelEpoch,
    Fault,
    Header,
    LongRows,
    MonthNames,
    NumFormat,
    Origin,
    Problem,
    Rounding,
    ShortRows,
    Success,
    SuggestOptions,
    SuggestStatus,
    Suggestion,
    TabularError,
    TabularFailure,
    UnixPrecision,
    Violation,
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


_DATE_FLAGS = {
    "compact_date": DateOptions.COMPACT_DATE,
    "space_separator": DateOptions.SPACE_SEPARATOR,
    "hour_offset": DateOptions.HOUR_OFFSET,
}

_ENCODINGS = {
    "windows-1252": Encoding.WINDOWS_1252,
    "latin-1": Encoding.LATIN_1,
    "utf-16le": Encoding.UTF_16LE,
}

# Every key the replays read, by where it is: what isn't here fails `_unknown_keys`, so a key
# added to the corpus can't be passed over. A cell, a problem and a failure compare whole.
_CELL_KEYS = {
    "expect",
    "value",
    "text",
    "seconds",
    "nanos",
    "year",
    "month",
    "day",
    "nanos_of_day",
    "magnitude",
    "scale",
    "negative",
    "fault",
    "raw",
    "raw_hex",
    "detail",
    "origin",
    "keyword",
    "adjusted",
    "enum_index",
    "bits",
}
_PROBLEM_KEYS = {
    "row",
    "line",
    "plan_column",
    "source_column",
    "reason",
    "fault",
    "detail",
    "origin",
    "expected",
    "found",
    "keyword",
}
_FAILURE_KEYS = {"kind", "record", "line", "byte", "expected", "found"}
_PLAN_KEYS = {
    "door",
    "ordinal",
    "precision",
    "order",
    "epoch",
    "format",
    "tokens",
    "options",
    "constraints",
}
# A plan entry's constraints, by the OpenAPI names HyperCast's corpus gives them.
_CONSTRAINT_KEYS = {
    "minimum": "minimum",
    "exclusiveMinimum": "exclusive_minimum",
    "maximum": "maximum",
    "exclusiveMaximum": "exclusive_maximum",
    "multipleOf": "multiple_of",
    "minLength": "min_length",
    "maxLength": "max_length",
    "enum": "enum",
    "trim": "trim",
    "caseSensitive": "case_sensitive",
    "rounding": "rounding",
}
_FORMAT_KEYS = {"decimal_sep", "group_sep", "flags", "currency"}
_DATE_OPTION_KEYS = {"flags", "pivot", "months"}
_DELIMITED_KEYS = {
    "name",
    "input",
    "input_hex",
    "dialect",
    "plan",
    "header",
    "rows",
    "problems",
    "failure",
    "suggest",
}
_DIALECT_KEYS = {
    "separator",
    "quoting",
    "has_header",
    "skip_blank_lines",
    "skip",
    "header_row",
    "short_rows",
    "long_rows",
    "tokens",
    "encoding",
}
_SUGGEST_KEYS = {"columns", "truncated", "sample_rows", "format"}
_SUGGESTED_KEYS = {"door", "param", "status", "matched", "sampled"}
_WORKBOOK_KEYS = {
    "name",
    "file",
    "format",
    "epoch",
    "sheets",
    "sheet",
    "options",
    "plan",
    "header",
    "numbers",
    "rows",
    "problems",
    "failure",
    "note",
    "suggest",
}
_SHEET_OPTION_KEYS = {"has_header", "skip_empty_rows", "range", "skip", "header_row", "tokens"}
_SHEET_KEYS = {"name", "hidden"}


def _unknown_keys(case: dict[str, Any], workbook: bool = False) -> list[str]:
    """Every key of a case no replay here reads, as a path."""
    name = case.get("name", "a case with no name")
    unknown: list[str] = []

    def check(value: Any, path: str, known: set[str]) -> None:
        if isinstance(value, dict):
            unknown.extend(f"{path}.{key}" for key in value if key not in known)

    check(case, name, _WORKBOOK_KEYS if workbook else _DELIMITED_KEYS)
    if workbook:
        check(case.get("options"), f"{name}.options", _SHEET_OPTION_KEYS)
        for sheet in case.get("sheets", []):
            check(sheet, f"{name}.sheets[]", _SHEET_KEYS)
    else:
        check(case.get("dialect"), f"{name}.dialect", _DIALECT_KEYS)
    for column in case.get("plan", []):
        check(column, f"{name}.plan[]", _PLAN_KEYS)
        check(column.get("format"), f"{name}.plan[].format", _FORMAT_KEYS)
        check(column.get("options"), f"{name}.plan[].options", _DATE_OPTION_KEYS)
        check(column.get("constraints"), f"{name}.plan[].constraints", set(_CONSTRAINT_KEYS))
    for row in case.get("rows", []):
        for cell in row:
            check(cell, f"{name}.rows[][]", _CELL_KEYS)
    for problem in case.get("problems", []):
        check(problem, f"{name}.problems[]", _PROBLEM_KEYS)
    check(case.get("failure"), f"{name}.failure", _FAILURE_KEYS)
    suggest = case.get("suggest")
    if suggest is not None:
        check(suggest, f"{name}.suggest", _SUGGEST_KEYS)
        check(suggest.get("format"), f"{name}.suggest.format", _FORMAT_KEYS)
        for column in suggest["columns"]:
            check(column, f"{name}.suggest.columns[]", _SUGGESTED_KEYS)
    return unknown


def _key_places(value: Any, path: tuple[Any, ...] = ()) -> list[tuple[Any, ...]]:
    """Every object in ``value``, first of each list only."""
    if isinstance(value, dict):
        places = [path]
        for key, inner in value.items():
            places += _key_places(inner, (*path, key))
        return places
    if isinstance(value, list) and value:
        return _key_places(value[0], (*path, 0))
    return []


def _assert_keys_are_known(cases: list[dict[str, Any]], workbook: bool) -> None:
    """Nothing in the corpus is passed over, and the check notices a key added anywhere."""
    assert [path for case in cases for path in _unknown_keys(case, workbook)] == []
    for case in cases:
        for place in _key_places(case):
            edited = json.loads(json.dumps(case))
            at = edited
            for step in place:
                at = at[step]
            at["junk"] = True
            unknown = _unknown_keys(edited, workbook)
            assert len(unknown) == 1 and unknown[0].endswith(".junk"), (place, unknown)


def _format_of(entry: dict[str, Any]) -> NumFormat:
    """The format an entry names, its flags split as HyperCast's constructor takes them: the
    lenience flags, and the grouping policy above them."""
    fmt = entry.get("format")
    if fmt is None:
        return NumFormat.INVARIANT
    return NumFormat(
        fmt["decimal_sep"],
        fmt["group_sep"],
        fmt["flags"] & 0xFF,
        fmt.get("currency", ""),
        grouping=fmt["flags"] & ~0xFF,
    )


def _declared_options(column: Column, entry: dict[str, Any]) -> Column:
    """A plan entry's ``tokens`` and date ``options``."""
    if "tokens" in entry:
        column = column.with_tokens(entry["tokens"])
    options = entry.get("options")
    if options is not None:
        flags = 0
        for name in options["flags"]:
            flags |= _DATE_FLAGS[name]
        months = None
        if "months" in options:
            months = MonthNames([tuple(month) for month in options["months"]])
        column = column.with_date_options(DateOptions(flags, options.get("pivot", 0)), months)
    return column


def _column_of(entry: dict[str, Any]) -> Column:
    # A plan entry names its door as HyperCast's corpus names its types, and Column's
    # factories carry the same names. An entry with no format builds a column given none, so that
    # the vector says what a column with no format reads.
    door, ordinal = entry["door"], entry["ordinal"]
    factory = getattr(Column, door)
    if "constraints" in entry:
        column = factory(ordinal, constraints=_constraints_of(door, entry["constraints"]))
    elif door == "unix":
        column = factory(ordinal, UnixPrecision(entry["precision"]))
    elif door == "excel_serial":
        column = factory(ordinal, ExcelEpoch(entry["epoch"]))
    elif door in ("date_ordered", "datetime"):
        column = factory(ordinal, DateOrder(entry["order"]))
    else:
        column = factory(ordinal)
    # Any door takes a format, as a suggested plan's do: only a number's reads it.
    if "format" in entry:
        column = dataclasses.replace(column, format=_format_of(entry))
    return _declared_options(column, entry)


def _constraints_of(door: str, entry: dict[str, Any]) -> Constraints[Any]:
    """A plan entry's constraints as HyperCast's own declaration for the door it names."""
    declared = {_CONSTRAINT_KEYS[key]: value for key, value in entry.items()}
    if "rounding" in declared:
        declared["rounding"] = Rounding[declared["rounding"].upper()]
    return Constraints(hypercast.Door[door.upper()], **declared)


def _input_of(case: dict[str, Any]) -> bytes:
    if "input_hex" in case:
        return bytes.fromhex(case["input_hex"])
    return case["input"].encode("utf-8")


def _dialect_of(entry: dict[str, Any]) -> Dialect:
    declared = dict(entry)
    if "short_rows" in declared:
        declared["short_rows"] = ShortRows[declared["short_rows"].upper()]
    if "long_rows" in declared:
        declared["long_rows"] = LongRows[declared["long_rows"].upper()]
    if "encoding" in declared:
        declared["encoding"] = _ENCODINGS[declared["encoding"]]
    return Dialect(**declared)


def _plain_verdict(column: Column) -> bool:
    """Whether HyperCast's plain door says what this column's says, with nothing declared beside
    it that ``_judge`` doesn't pass."""
    return column.date_options is None and column.months is None


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


def _detail_name(fault: Fault) -> str | None:
    """A fault's detail by the corpus's name for it: the member's, in lower case."""
    return None if fault.detail is None else fault.detail.name.lower()


def _assert_cell(
    label: str,
    column: Column,
    data: Any,
    row: int,
    expected: dict[str, Any],
    judged: bool = True,
    notes: bool = True,
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
                if "raw_hex" in expected:
                    assert raw == bytes.fromhex(expected["raw_hex"]), label
                else:
                    assert raw.decode("utf-8") == expected["raw"], label
            # What failed, and where the verdict came from: on the fault, and the batch's own.
            said = expected.get("detail") if notes else None
            assert _detail_name(verdict) == said, f"{label}: detail {verdict.detail}"
            if expect == "empty" and "origin" not in expected:
                assert raw == b"", label
        case other:  # pragma: no cover - the union is closed
            raise AssertionError(f"{label}: produced no case: {other!r}")

    # Where the verdict came from, whatever it is.
    origin = expected.get("origin", "cell") if notes else "cell"
    assert data.origin(row) is Origin[origin.upper()], f"{label}: {data.origin(row)}"

    # HyperCast is the judge: the cell is what its own package makes of the same text. A cell no
    # door was asked about is the read's word, not HyperCast's.
    if judged and column.door is not Door.TEXT and "origin" not in expected:
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


def _assert_checked_value(label: str, column: Column, value: Any, expected: dict[str, Any]) -> None:
    """A checked cell's value, passed or as cast: a float to its exact bits, a decimal to its
    digits and scale."""
    match column.door:
        case Door.F32 | Door.F64:
            packed = struct.pack(">f" if column.door is Door.F32 else ">d", value)
            assert type(value) is float and f"0x{packed.hex()}" == expected["bits"], (
                f"{label}: {value!r}, want bits {expected['bits']}"
            )
        case Door.DECIMAL:
            sign, digits, exponent = value.as_tuple()
            assert (sign, digits, exponent) == (
                int(expected["negative"]),
                tuple(int(d) for d in str(int(expected["magnitude"]))),
                -expected["scale"],
            ), f"{label}: {value!r}"
        case _:
            want = _expected_value(column, expected)
            assert value == want and type(value) is type(want), f"{label}: {value!r}, want {want!r}"


def _assert_checked(
    label: str,
    column: Column,
    batch: Batch,
    index: int,
    row: int,
    expected: dict[str, Any],
    notes: bool,
) -> None:
    """Holds one cell of a constrained column to what the corpus says of it, through the checked
    getter: what passed, a violation of a constraint the column declares, or the cell's fault."""
    verdict = batch.get_checked(index, row)
    verdicts = batch.column(index).verdicts
    offset, length, reason = verdicts[row, 0], verdicts[row, 1], verdicts[row, 2]
    raw = batch.raw(index, row)
    expect = expected["expect"]
    assert column.constraints is not None
    match verdict:
        case Checked(value, adjusted, enum_index):
            assert expect == "ok", f"{label}: unexpectedly passed with {value!r}"
            assert (adjusted, enum_index) == (expected["adjusted"], expected.get("enum_index")), (
                f"{label}: {verdict!r}"
            )
            assert reason == 0, label
            if enum_index is not None and column.door is Door.TEXT:
                # The entry as declared: the caller's own string.
                entries = column.constraints.enum
                assert entries is not None and value is entries[enum_index], label
            _assert_checked_value(label, column, value, expected)
        case Violation(keyword, value, at, span):
            assert expect == "violation", f"{label}: unexpectedly broke {verdict!r}"
            assert keyword.name == expected["keyword"], f"{label}: {verdict!r}"
            assert [at, span] == expected["fault"] and (at, span, reason) == (offset, length, 4), (
                f"{label}: violation span"
            )
            assert raw.decode("utf-8") == expected["raw"], label
            _assert_checked_value(label, column, value, expected)
        case Fault(reason=why, offset=at, length=span):
            assert expect in _REASON, f"{label}: expected {expect} but faulted with {verdict!r}"
            assert why is _REASON[expect] and (at, span, int(why)) == (offset, length, reason), (
                f"{label}: {verdict!r}"
            )
            if "fault" in expected:
                assert [at, span] == expected["fault"], f"{label}: fault span"
                if "raw_hex" in expected:
                    assert raw == bytes.fromhex(expected["raw_hex"]), label
                else:
                    assert raw.decode("utf-8") == expected["raw"], label
            said = expected.get("detail") if notes else None
            assert _detail_name(verdict) == said, f"{label}: detail {verdict.detail}"
        case other:  # pragma: no cover - the union is closed
            raise AssertionError(f"{label}: produced no case: {other!r}")
    origin = expected.get("origin", "cell") if notes else "cell"
    assert batch.origin(index, row) is Origin[origin.upper()], label


def _assert_checked_column(
    label: str,
    column: Column,
    batch: Batch,
    index: int,
    seen: int,
    expected: list[dict[str, Any]],
    notes: bool,
) -> None:
    """A constrained column: each cell through the checked getter, the plain getters refusing
    it, and its whole-batch views as the core wrote them, a violation's value kept as cast."""
    data = batch.column(index)
    for row, cell in enumerate(expected):
        _assert_checked(
            f"{label}, row {seen + row}, column {index}", column, batch, index, row, cell, notes
        )
        assert batch.raw(index, row) == data.raw(row), label
    refusals = [lambda: data[0], lambda: list(data), lambda: batch.get(index, 0)]
    if column.door is Door.TEXT:
        refusals.append(lambda: batch.text(index, 0))
    for refused in refusals:
        with pytest.raises(TypeError, match="get_checked"):
            refused()
    verdicts = [batch.get_checked(index, row) for row in range(len(expected))]
    faults = [(row, verdict) for row, verdict in enumerate(verdicts) if isinstance(verdict, Fault)]
    assert data.faults() == faults and data.fault_count == len(faults), label
    assert [reason for _, _, reason in data.verdicts.tolist()] == [
        4
        if isinstance(verdict, Violation)
        else int(verdict.reason)
        if isinstance(verdict, Fault)
        else 0
        for verdict in verdicts
    ], label
    values = data.values
    for row, verdict in enumerate(verdicts):
        match verdict:
            case Violation(value=value):
                assert values[row] == value, f"{label}, row {row}: the value as cast"
            case Checked(value=value, enum_index=None):
                assert values[row] == value, f"{label}, row {row}"
            case Fault():
                assert values[row] == (0 if isinstance(values, memoryview) else None), label


def _assert_failure(label: str, actual: TabularError | None, case: dict[str, Any]) -> None:
    expected = case.get("failure")
    if expected is None:
        assert actual is None, f"{label}: {actual}"
        return
    assert actual is not None, f"{label}: no failure raised"
    # The corpus names a kind as the core does: the member's name, in lower case. The whole
    # failure is held to it, an absent `expected` and `found` being 0.
    assert actual.kind is TabularFailure[expected["kind"].upper()], label
    assert (actual.record, actual.line, actual.byte, actual.expected, actual.found) == (
        expected["record"],
        expected["line"],
        expected["byte"],
        expected.get("expected", 0),
        expected.get("found", 0),
    ), label


def _problem_json(problem: Problem, base: int) -> dict[str, Any]:
    """An entry of a batch's faults list in the corpus's shape, its row counted across the read."""
    entry: dict[str, Any] = {"row": base + problem.row, "line": problem.line}
    if problem.plan_column is None:
        assert problem.source_column is None and problem.reason is None
        assert (problem.offset, problem.length) == (0, 0)
        entry["expected"], entry["found"] = problem.expected, problem.found
    elif problem.keyword is not None:
        # A violation: the cell cast, so there's no reason, and the span is the judged text's.
        assert problem.source_column is not None and problem.reason is None
        assert (problem.expected, problem.found) == (0, 0)
        entry["plan_column"] = problem.plan_column
        entry["source_column"] = problem.source_column
        entry["reason"] = "violation"
        entry["keyword"] = problem.keyword.name
        entry["fault"] = [problem.offset, problem.length]
    else:
        assert problem.source_column is not None and problem.reason is not None
        assert (problem.expected, problem.found) == (0, 0)
        entry["plan_column"] = problem.plan_column
        entry["source_column"] = problem.source_column
        entry["reason"] = problem.reason.name.lower()
        if problem.reason is not CastFailure.EMPTY:
            entry["fault"] = [problem.offset, problem.length]
        else:
            assert (problem.offset, problem.length) == (0, 0)
    if problem.detail is not None:
        assert problem.detail_code == int(problem.detail)
        entry["detail"] = problem.detail.name.lower()
    if problem.origin is not Origin.CELL:
        entry["origin"] = problem.origin.name.lower()
    return entry


def _problems_without_notes(problems: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """What a read with notes off lists of ``problems``: no formula with no cached value, and no
    detail or origin on the rest."""
    return [
        {key: value for key, value in problem.items() if key not in ("detail", "origin")}
        for problem in problems
        if problem.get("origin") != "uncached"
    ]


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
            assert row.get_checked(index) == batch.get_checked(index, row.index), label
            if column.constraints is not None:
                # The plain getters refuse a constrained column: they'd read a violation as passed.
                for refused in (row.get, row.text):
                    with pytest.raises(TypeError, match="get_checked"):
                        refused(index)
                assert row.raw(index) == batch.raw(index, row.index), label
                continue
            assert row.get(index) == batch.get(index, row.index), label
            assert row.get(index) == batch.column(index)[row.index], label
            assert row.raw(index) == batch.raw(index, row.index), label
            assert row.origin(index) is batch.origin(index, row.index), label
            assert row.origin(index) is batch.column(index).origin(row.index), label
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
    problems: list[dict[str, Any]],
    notes: bool = True,
) -> int:
    """Holds one batch to the corpus rows from ``seen`` on, and adds its faults list to
    ``problems``; returns the rows seen after it."""
    assert 1 <= batch.rows <= batch_rows and len(batch) == batch.rows, label
    assert seen + batch.rows <= len(rows), f"{label}: more rows than the corpus lists"
    columns = batch.columns
    assert len(columns) == len(plan), label
    problems += [_problem_json(problem, seen) for problem in batch.faults()]
    for row in rows[seen : seen + batch.rows]:
        assert len(row) == len(plan), f"{label}: a corpus row of another width"
    for index, (column, data) in enumerate(zip(plan, columns)):
        expected = [row[index] for row in rows[seen : seen + batch.rows]]
        if column.constraints is not None:
            _assert_checked_column(label, column, batch, index, seen, expected, notes)
            continue
        judged = notes and _plain_verdict(column)
        for row, cell in enumerate(expected):
            _assert_cell(
                f"{label}, row {seen + row}, column {index}",
                column,
                data,
                row,
                cell,
                judged=judged,
                notes=notes,
            )
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
    notes: bool = True,
) -> None:
    assert _unknown_keys(case) == [], label
    dialect = _dialect_of(case["dialect"])
    plan = [_column_of(entry) for entry in case["plan"]]
    rows = case["rows"]

    try:
        reader = open_reader(dialect, plan)
    except TabularError as error:
        # A failure in the header record itself comes from opening, with no rows.
        assert rows == [] and case["problems"] == [], label
        _assert_failure(label, error, case)
        return
    assert reader.header == (None if case["header"] is None else tuple(case["header"])), label
    assert reader.plan == tuple(plan), label

    seen = 0
    failure = None
    problems: list[dict[str, Any]] = []
    try:
        for batch in reader:
            seen = _assert_batch(label, plan, rows, seen, batch, batch_rows, problems, notes)
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
    wanted = case["problems"] if notes else _problems_without_notes(case["problems"])
    assert problems == wanted, f"{label}: problems"
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


def test_every_corpus_key_is_read():
    """A key this replay doesn't know fails it, wherever in a case it is."""
    assert len(CORPUS) == 137
    _assert_keys_are_known(CORPUS, workbook=False)


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_with_notes_off(case: dict[str, Any], batch_rows: int) -> None:
    """Notes off, every cell reads the same with no detail or origin, and the faults list the
    same less a formula with no cached value."""
    data = _input_of(case)
    _replay(
        f"{case['name']} (notes off, {batch_rows} rows a batch)",
        case,
        batch_rows,
        lambda dialect, plan: DelimitedReader(
            data, dialect, plan, batch_rows=batch_rows, notes=False
        ),
        notes=False,
    )


@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_from_memory(case: dict[str, Any], batch_rows: int) -> None:
    """Every corpus case reads the same from bytes in memory, at every batch size."""
    data = _input_of(case)
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
    data = _input_of(case)
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
    data = _input_of(case)
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
    path.write_bytes(_input_of(case))
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
    data = _input_of(case)
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
    data = _input_of(case)
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
    path.write_bytes(_input_of(case))
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


T = TypeVar("T")


def _run(coroutine: Coroutine[Any, Any, T]) -> T:
    """``asyncio.run``, except under Pyodide, whose event loop is the browser's: a synchronous
    test cannot block on it (``asyncio.run`` there raises "Cannot stack switch"), so the
    coroutine is driven by hand. That is enough for every stream it is used with here, which
    only ever yields to the loop through ``asyncio.sleep(0)``; a test that needs a real loop —
    an ``asyncio.StreamReader``, a task to cancel — is skipped under Pyodide instead."""
    if sys.platform != "emscripten":
        return asyncio.run(coroutine)
    try:
        while True:
            if coroutine.send(None) is not None:
                coroutine.close()
                raise RuntimeError("this coroutine needs an event loop; skip it under Pyodide")
    except StopIteration as done:
        return done.value  # type: ignore[no-any-return]


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
    dialect = _dialect_of(case["dialect"])
    plan = [_column_of(entry) for entry in case["plan"]]
    rows = case["rows"]

    try:
        if header_first:
            reader = await open_reader(dialect, None)
            _assert_unbound(label, case, reader)
            reader.bind(plan)
        else:
            reader = await open_reader(dialect, plan)
    except TabularError as error:
        # A failure in the header record itself comes from opening, with no rows.
        assert rows == [] and case["problems"] == [], label
        _assert_failure(label, error, case)
        return
    assert reader.header == (None if case["header"] is None else tuple(case["header"])), label

    seen = 0
    failure = None
    problems: list[dict[str, Any]] = []
    try:
        first = await reader.read_async()
        if first is not None:
            seen = _assert_batch(label, plan, rows, seen, first, batch_rows, problems)
            async for batch in reader:
                seen = _assert_batch(label, plan, rows, seen, batch, batch_rows, problems)
    except TabularError as error:
        failure = error
        with pytest.raises(TabularError) as again:
            await reader.read_async()
        assert again.value is error, label
    else:
        assert await reader.read_async() is None, label
    assert seen == len(rows), f"{label}: {seen} rows, the corpus lists {len(rows)}"
    assert problems == case["problems"], f"{label}: problems"
    _assert_failure(label, failure, case)


@pytest.mark.parametrize("header_first", (False, True), ids=("planned", "header-first"))
@pytest.mark.parametrize("buffer_bytes", (2, DelimitedReader.DEFAULT_BUFFER_BYTES))
@pytest.mark.parametrize("batch_rows", _BATCH_ROWS)
@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_corpus_asynchronously(
    case: dict[str, Any], batch_rows: int, buffer_bytes: int, header_first: bool
) -> None:
    """Every corpus case reads the same from an asyncio stream of short reads."""
    data = _input_of(case)

    async def open_reader(dialect: Dialect, plan: list[Column] | None) -> DelimitedReader:
        return await DelimitedReader.open_async(
            _AsyncDribble(data), dialect, plan, batch_rows=batch_rows, buffer_bytes=buffer_bytes
        )

    _run(
        _replay_async(
            f"{case['name']} (async, {buffer_bytes} bytes, {batch_rows} rows a batch)",
            case,
            batch_rows,
            open_reader,
            header_first,
        )
    )


def _suggestion_json(found: Suggestion) -> list[dict[str, Any]]:
    """A report in the corpus's shape. An ambiguous column's ``param`` is the set of its
    candidates, which this surface gives as a list, and a resolved number's is left at 0, since a
    report names the format the plan uses and can't say what ``param`` was."""
    columns = []
    for column in found.report:
        param = 0
        if column.candidates:
            for candidate in column.candidates:
                if isinstance(candidate, DateOrder):
                    param |= 1 << int(candidate)
                else:
                    param |= 2 if candidate.decimal_sep == "." else 4
        elif column.door in (Door.DATE_ORDERED, Door.DATETIME):
            assert isinstance(column.declared, DateOrder)
            param = int(column.declared)
        columns.append(
            {
                "door": column.door.name.lower(),
                "param": param,
                "status": column.status.name.lower(),
                "matched": column.matched,
                "sampled": column.sampled,
            }
        )
    return columns


def _reported(columns: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """What a report should say of the corpus's ``columns``: see ``_suggestion_json``."""
    made = []
    for column in columns:
        column = dict(column)
        dated = column["door"] in ("date_ordered", "datetime")
        if column["status"] == "resolved" and not dated:
            column["param"] = 0
        made.append(column)
    return made


def _assert_notation(
    label: str, found: Suggestion, columns: list[dict[str, Any]], declared: NumFormat | None
) -> None:
    """Each resolved number is planned and reported in the format the corpus's ``param`` says, or
    the one declared; anything else reports none."""
    for reported, planned, want in zip(found.report, found.plan, columns):
        numeric = reported.door in _NUMERIC_DOORS
        if reported.status is SuggestStatus.RESOLVED and numeric:
            notation = NumFormat(",", ".", NumFormat.ALL) if want["param"] == 4 else None
            wanted = declared or notation or NumFormat.INVARIANT
            assert reported.format is not None, label
            assert reported.format.packed == wanted.packed, label
            assert planned.format.packed == wanted.packed, label
        else:
            assert reported.format is None, label


_NUMERIC_DOORS = {
    Door.I8,
    Door.I16,
    Door.I32,
    Door.I64,
    Door.U8,
    Door.U16,
    Door.U32,
    Door.U64,
    Door.F32,
    Door.F64,
    Door.DECIMAL,
}


def _suggesting() -> list[dict[str, Any]]:
    return [case for case in CORPUS if "suggest" in case]


def test_the_corpus_says_what_to_suggest():
    """The corpus says what to suggest."""
    assert len(_suggesting()) >= 12


@pytest.mark.parametrize("case", _suggesting(), ids=lambda case: case["name"])
def test_every_suggestion_is_what_the_corpus_says(case: dict[str, Any], tmp_path: Path) -> None:
    """A suggestion from bytes and from a file says what the corpus does, and its plan reads back
    refusing nothing it resolved, from memory and asynchronously."""
    label = case["name"]
    expected = case["suggest"]
    data = _input_of(case)
    dialect = _dialect_of(case["dialect"])
    declared = _format_of(expected) if "format" in expected else None
    asked = SuggestOptions(sample_rows=expected.get("sample_rows", 1000), format=declared)
    found = DelimitedReader.suggest_bytes(data, dialect, asked)
    columns = expected["columns"]
    assert _suggestion_json(found) == _reported(columns), label
    assert found.truncated is expected["truncated"], label
    _assert_notation(label, found, columns, declared)
    path = tmp_path / "case.csv"
    path.write_bytes(data)
    assert DelimitedReader.suggest(path, dialect, asked) == found, label

    sample = expected.get("sample_rows", len(case["rows"]))

    def refused(batch: Batch, base: int) -> list[Problem]:
        return [
            problem
            for problem in batch.faults()
            if problem.plan_column is not None
            and found.report[problem.plan_column].status is SuggestStatus.RESOLVED
            and found.report[problem.plan_column].door is not Door.TEXT
            and problem.row + base < sample
        ]

    read = 0
    with DelimitedReader(data, dialect, found.plan) as reader:
        try:
            for batch in reader:
                assert refused(batch, read) == [], label
                read += batch.rows
        except TabularError:
            assert read >= sample, label

    async def main() -> None:
        reader = await DelimitedReader.open_async(_AsyncDribble(data), dialect, found.plan)
        read = 0
        try:
            async for batch in reader:
                assert refused(batch, read) == [], label
                read += batch.rows
        except TabularError:
            assert read >= sample, label

    _run(main())


def test_suggest_fails_only_where_a_read_of_the_case_fails() -> None:
    """Every case the corpus has, whether it says what to suggest or not."""
    planned = failed = 0
    for case in CORPUS:
        try:
            DelimitedReader.suggest_bytes(_input_of(case), _dialect_of(case["dialect"]))
            planned += 1
        except TabularError:
            failed += 1
            assert "failure" in case, case["name"]
    assert planned > 80 and failed > 0, (planned, failed)
