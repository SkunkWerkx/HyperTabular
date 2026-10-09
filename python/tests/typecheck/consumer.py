"""What a consumer writes, type-checked by ``tests/test_typing.py`` (never imported or run).

Two things are held here. ``assert_type`` fails the check when the checker sees ``Any``
where the package promises a type — which is what the whole surface would be without
``py.typed`` and a stub for the extension module. And ``assert_never`` in the fall-through
arm is the exhaustiveness HyperCast's union promises, carried through this package: it
type-checks only because the two ``case`` arms above it leave nothing of a cell over.
"""

import asyncio
import io
from collections.abc import Sequence
from pathlib import Path
from typing import Any, assert_never, assert_type

import hypercast

import hypertabular
from hypertabular import (
    Batch,
    CastFailure,
    Column,
    ColumnData,
    DateOrder,
    DelimitedReader,
    Dialect,
    Door,
    ExcelEpoch,
    Fault,
    Header,
    NumFormat,
    Row,
    Rows,
    Sheet,
    SheetInfo,
    SheetOptions,
    Success,
    TabularError,
    TabularFailure,
    UnixPrecision,
    Verdict,
    Workbook,
    WorkbookFormat,
)


def describe(verdict: Verdict[Any]) -> str:
    """Presents a verdict by matching its two cases, exhaustively."""
    match verdict:
        case Success(value):
            return f"got {value}"
        case Fault(reason, offset, length):
            assert_type(reason, CastFailure)
            assert_type(offset, int)
            assert_type(length, int)
            return f"{reason.name} at {offset}+{length}"
        case _:
            assert_never(verdict)


plan = [
    Column.bool(0),
    Column.i32(1),
    Column.f64(2, NumFormat(",", ".", NumFormat.ALL)),
    Column.decimal(3, fmt=NumFormat.INVARIANT),
    Column.unix(4, UnixPrecision.MILLISECONDS),
    Column.excel_serial(5, ExcelEpoch.Y1904),
    Column.date(6),
    Column.date(6, DateOrder.DAY_MONTH_YEAR),
    Column.date_ordered(6, DateOrder.DAY_MONTH_YEAR),
    Column.datetime(7, DateOrder.MONTH_DAY_YEAR),
    Column.text(8),
]
assert_type(plan, list[Column])
assert_type(plan[0].door, Door)
assert_type(plan[0].ordinal, int)
assert_type(plan[2].format, NumFormat)
assert_type(Dialect.CSV, Dialect)
assert_type(Dialect(";", quoting=False, has_header=False, skip_blank_lines=False).separator, str)

reader = DelimitedReader(b"a\n1\n", Dialect.CSV, plan, batch_rows=1024)
assert_type(reader, DelimitedReader)
assert_type(DelimitedReader(io.BytesIO(b""), Dialect.TSV, plan, buffer_bytes=4096), DelimitedReader)
assert_type(DelimitedReader.open("rows.csv", Dialect.CSV, plan), DelimitedReader)
assert_type(reader.header, Header | None)
# A Header is the tuple of names it always was.
names: tuple[str, ...] | None = reader.header
assert_type(reader.plan, tuple[Column, ...])
assert_type(reader.column_count, int | None)
assert_type(reader.is_bound, bool)
assert_type(reader.records, int)
assert_type(reader.read(), Batch | None)

# Header first: open without a plan, look the names up, then bind.
unbound = DelimitedReader.open("rows.csv", Dialect.CSV, batch_rows=256)
assert_type(DelimitedReader(io.BytesIO(b"a\n"), Dialect.CSV), DelimitedReader)
header = unbound.header
assert header is not None
assert_type(header, Header)
assert_type(header.ordinal("id"), int)
assert_type(header.ordinal(b"id"), int)
assert_type(header.get("id"), int | None)
assert_type(header.get(b"id", -1), int)
assert_type(header[0], str)
unbound.bind([Column.i32(header.ordinal("id")), Column.text(header.ordinal("name"))])
for each in unbound.rows():
    assert_type(each, Row)
    assert_type(each.index, int)
    assert_type(each.line, int)
    assert_type(each.batch, Batch)
    assert_type(each.get(0), Success[Any] | Fault)
    assert_type(each.text(1), str | None)
    assert_type(each.raw(1), bytes)


async def read_asynchronously(stream: asyncio.StreamReader) -> int:
    """Reads a stream with asyncio, a batch and then the rest."""
    fed = await DelimitedReader.open_async(stream, Dialect.CSV)
    assert_type(fed, DelimitedReader)
    fed.bind([Column.i64(0)])
    first = await fed.read_async()
    assert_type(first, Batch | None)
    total = 0 if first is None else first.rows
    async for later in fed:
        assert_type(later, Batch)
        total += later.rows
    assert_type(await DelimitedReader.open_async(b"a\n1\n", Dialect.CSV, plan), DelimitedReader)
    book = await Workbook.open_async(stream)
    assert_type(book, Workbook)
    return total


with DelimitedReader.open(Path("rows.csv"), Dialect.PSV, plan) as opened:
    assert_type(opened, DelimitedReader)
    try:
        for batch in opened:
            assert_type(batch, Batch)
            assert_type(batch.rows, int)
            assert_type(batch.columns, tuple[ColumnData, ...])
            assert_type(batch.raw(0, 0), bytes)
            assert_type(batch.line(0), int)
            assert_type(batch.get(0, 0), Success[Any] | Fault)
            assert_type(batch.text(8, 0), str | None)
            assert_type(batch.row(0), Row)
            assert_type(batch.iter_rows(), Rows)
            for row in batch:
                assert_type(row, Row)
            column = batch.column(0)
            assert_type(column, ColumnData)
            assert_type(column.column, Column)
            assert_type(column.values, Sequence[Any])
            assert_type(column.verdicts, memoryview)
            assert_type(column.fault_count, int)
            assert_type(column.faults(), list[tuple[int, Fault]])
            assert_type(column.raw(0), bytes)
            assert_type(column[0], Success[Any] | Fault)
            assert_type(describe(column[0]), str)
            for cell in column:
                assert_type(cell, Success[Any] | Fault)
                assert_type(hypercast.optional(cell), Success[Any] | Fault | None)
    except TabularError as error:
        assert_type(error.kind, TabularFailure)
        assert_type(error.record, int)
        assert_type(error.line, int)
        assert_type(error.byte, int)
        assert_type(error.expected, int)
        assert_type(error.found, int)

book = Workbook.open(Path("orders.xlsx"))
assert_type(book, Workbook)
assert_type(Workbook(b""), Workbook)
assert_type(Workbook(io.BytesIO(b"")), Workbook)
assert_type(book.format, WorkbookFormat)
assert_type(book.date_system, ExcelEpoch)
assert_type(book.sheets, tuple[SheetInfo, ...])
assert_type(book.sheets[0].name, str)
assert_type(book.sheets[0].hidden, bool)
sheet = book.sheet("Orders", SheetOptions(skip_empty_rows=False, batch_rows=256), plan)
assert_type(sheet, Sheet)
assert_type(book.sheet(0, SheetOptions(), plan), Sheet)
assert_type(sheet.options, SheetOptions)
assert_type(sheet.header, Header | None)
assert_type(sheet.plan, tuple[Column, ...])
assert_type(sheet.read(), Batch | None)
for rows in sheet:
    assert_type(rows, Batch)
headed = book.sheet("Orders", SheetOptions())
assert_type(headed.is_bound, bool)
if headed.header is not None:
    headed.bind([Column.text(headed.header.ordinal("Name"))])
for sheet_row in headed.rows():
    assert_type(sheet_row, Row)

assert_type(hypertabular.native_version(), str)
assert_type(hypertabular.BACKEND, str)
