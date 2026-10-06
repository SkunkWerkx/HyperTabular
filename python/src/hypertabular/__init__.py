"""Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and
ODS — read a batch at a time into typed columns, with a
`HyperCast <https://github.com/SkunkWerkx/HyperCast>`_ verdict for every cell.

The native core owns no memory and reads no files. It is linked straight into a CPython
extension module (``hypertabular._native``, PyO3), and the extension is the binding: it
allocates a value array and a verdict array per column, once, and the core fills them in
one call per batch, with the interpreter released. A column then reaches Python whole — a
read-only ``memoryview`` of the door's own item type over the batch's copy of that array,
which anything that takes a buffer takes — with no Python call per cell::

    from hypertabular import Column, DelimitedReader, Dialect, Fault, Success

    plan = [Column.i32(0), Column.text(1), Column.f64(2)]
    with DelimitedReader.open("orders.csv", Dialect.CSV, plan) as reader:
        for batch in reader:
            ids, names, scores = batch.columns

            # A column at a time, as the core wrote it…
            total = sum(scores.values)            # memoryview of C doubles
            if scores.fault_count:
                for row, fault in scores.faults():
                    print(row, fault.reason.name, scores.raw(row))

            # …or a cell at a time, as HyperCast's union.
            for row in range(batch.rows):
                match scores[row]:
                    case Success(value):
                        print(names.values[row], value)
                    case Fault(reason, offset, length):
                        print(reason.name, "on line", batch.line(row), "in", scores.raw(row))

    # A workbook reads into the same batch.
    book = Workbook.open("orders.xlsx")
    for batch in book.sheet("Orders", SheetOptions(), plan):
        ...

- **Nothing is sniffed.** The :class:`Dialect` states the separator, the quoting and the
  header; :class:`SheetOptions` states a sheet's header and whether empty rows are skipped;
  the plan states each column's door and, for numbers, its ``NumFormat``.
- **HyperCast is the judge.** :class:`Success`, :class:`Fault`, :class:`CastFailure`,
  :class:`NumFormat`, :class:`UnixPrecision`, :class:`DateOrder` and :class:`ExcelEpoch`
  are the ``hypercast`` package's own objects, re-exported here unchanged — not copies. A
  text cell means exactly what ``hypercast.cast_*`` says of the same text, a typed workbook
  cell is converted by the door directly, and its value is the
  Python type that door gives: ``int``, ``float``, ``bool``, ``decimal.Decimal``,
  ``uuid.UUID``, ``datetime``, ``date``, ``time``, ``timedelta`` (and ``str`` for text).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a ``Fault`` in its column and the read goes on. A record of the wrong width, input
  that ends inside a quoted cell, a workbook whose container or parts cannot be read,
  raises :class:`TabularError` — after every intact row before it has been delivered.
- **A batch owns what it shows.** It stays valid after the reader has moved on.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from typing import ClassVar

from hypercast import (
    CastFailure,
    DateOrder,
    ExcelEpoch,
    Fault,
    NumFormat,
    Success,
    UnixPrecision,
    Verdict,
)

from . import _native

#: Which backend this process loaded: always ``"native"``, the PyO3 extension that links the
#: Rust core straight into CPython — the only backend, and the one every wheel ships.
BACKEND: str = "native"

__all__ = [
    "BACKEND",
    "native_version",
    "DelimitedReader",
    "Workbook",
    "WorkbookFormat",
    "Sheet",
    "SheetInfo",
    "SheetOptions",
    "Batch",
    "ColumnData",
    "Dialect",
    "Column",
    "Door",
    "TabularError",
    "TabularFailure",
    # HyperCast's own, re-exported unchanged.
    "CastFailure",
    "Success",
    "Fault",
    "Verdict",
    "NumFormat",
    "UnixPrecision",
    "DateOrder",
    "ExcelEpoch",
]


class Door(IntEnum):
    """The door a column is cast through: HyperCast's, plus :data:`TEXT` for the bytes
    themselves. The values are the core's own door codes."""

    BOOL = 1
    I8 = 2
    I16 = 3
    I32 = 4
    I64 = 5
    U8 = 6
    U16 = 7
    U32 = 8
    U64 = 9
    F32 = 10
    F64 = 11
    UUID = 12
    TIMESTAMP = 13
    UNIX = 14
    DATE = 15
    TIME = 16
    DURATION = 17
    TEXT = 18
    DECIMAL = 19
    DATE_ORDERED = 20
    DATETIME = 21
    EXCEL_SERIAL = 22


class TabularFailure(IntEnum):
    """Why an input could not be read as rows at all. The values are the core's codes."""

    UNCLOSED_QUOTE = 1
    """The input ended inside a quoted cell."""
    COLUMN_COUNT = 2
    """A record's cell count disagrees with the first record's."""
    ROW_TOO_LONG = 3
    """A single record is larger than :data:`DelimitedReader.MAX_ROW_BYTES`."""
    NOT_A_ZIP = 16
    """The workbook's container is not a zip file."""
    CONTAINER = 17
    """The zip's own structure is broken."""
    ENCRYPTED = 18
    """The workbook is encrypted."""
    METHOD = 19
    """A part is compressed by a method other than stored or deflate."""
    MISSING_PART = 20
    """A part the workbook cannot be read without is missing."""
    XML = 21
    """A part's XML ends inside a construct."""
    DEFLATE = 22
    """A part's bytes are not a deflate stream, or stop before the stream does."""
    NOT_A_WORKBOOK = 23
    """The zip is neither an XLSX nor an ODS workbook."""
    SHARED_STRING = 24
    """A cell names a shared string the table does not have."""
    TOO_LARGE = 25
    """More text than can be addressed: over 4 GiB in a batch or in the shared strings, or
    2 GiB in a cell."""


class WorkbookFormat(IntEnum):
    """Which kind of workbook a :class:`Workbook` is."""

    XLSX = 1
    """Office Open XML: ``.xlsx``, ``.xlsm``."""
    ODS = 2
    """OpenDocument: ``.ods``."""


class TabularError(Exception):
    """A structural failure: the input is not rows of cells — a record of the wrong width,
    input that ends inside a quoted cell, a workbook whose container or parts cannot be
    read. Never a cell's verdict: a value that does not cast is a ``Fault`` in its column,
    and the read goes on. A structural failure ends the input, after every intact row
    before it has been delivered.

    For a workbook, :attr:`record` is the part the failure is in, :attr:`line` the sheet
    row and :attr:`byte` the offset within the part's inflated bytes.
    """

    kind: TabularFailure
    """What is wrong."""
    record: int
    """Zero-based index of the offending record — the header and skipped blank lines included."""
    line: int
    """One-based line the offending record starts on."""
    byte: int
    """Absolute byte offset of the offending record's start."""
    expected: int
    """Cells in the first record, for :data:`TabularFailure.COLUMN_COUNT`."""
    found: int
    """Cells in this record, for :data:`TabularFailure.COLUMN_COUNT`."""

    def __init__(
        self,
        kind: TabularFailure,
        record: int,
        line: int,
        byte: int,
        expected: int = 0,
        found: int = 0,
    ) -> None:
        """Builds the failure the reader raises; the arguments are its attributes."""
        # The arguments themselves, not the message, so the exception copies and pickles.
        super().__init__(kind, record, line, byte, expected, found)
        self.kind = TabularFailure(kind)
        self.record = record
        self.line = line
        self.byte = byte
        self.expected = expected
        self.found = found

    def __str__(self) -> str:
        """The failure in words, with its position."""
        where = f"record {self.record} (line {self.line}, byte {self.byte})"
        part = f"part {self.record} of the workbook"
        match self.kind:
            case TabularFailure.COLUMN_COUNT:
                return f"{where} has {self.found} cells; the first record had {self.expected}"
            case TabularFailure.UNCLOSED_QUOTE:
                return f"the input ended inside a quoted cell in {where}"
            case TabularFailure.ROW_TOO_LONG:
                limit = _native.DelimitedReader.MAX_ROW_BYTES
                return f"{where} exceeds the {limit}-byte row ceiling"
            case TabularFailure.NOT_A_ZIP:
                return "the workbook is not a zip file"
            case TabularFailure.ENCRYPTED:
                return "the workbook is encrypted"
            case TabularFailure.METHOD:
                return f"{part} is compressed by method {self.found}, neither stored nor deflate"
            case TabularFailure.MISSING_PART:
                return f"part {self.record}, which the workbook cannot be read without, is missing"
            case TabularFailure.XML:
                return f"{part} ends inside an XML construct (byte {self.byte})"
            case TabularFailure.DEFLATE:
                return f"{part} is not a whole deflate stream (byte {self.byte})"
            case TabularFailure.NOT_A_WORKBOOK:
                return "the zip is neither an XLSX nor an ODS workbook"
            case TabularFailure.SHARED_STRING:
                return f"row {self.line} names shared string {self.found}; the table has {self.expected}"
            case TabularFailure.TOO_LARGE:
                return "the workbook holds more text than a batch can address"
            case _:
                return "the workbook's zip structure is broken"


@dataclass(frozen=True, slots=True)
class Dialect:
    """How the text is delimited, declared by the caller. Nothing is sniffed: the separator
    is stated, quoting is stated, the header is stated — the same stance HyperCast's
    ``NumFormat`` takes for numeric notation.
    """

    separator: str
    """The single-byte separator: tab, or any printable ASCII character except ``"``."""
    quoting: bool = True
    """Whether ``"`` quotes cells (RFC 4180, ``""`` for a literal quote). Off, a quote is
    an ordinary byte."""
    has_header: bool = True
    """Whether the first record is a header, exposed through
    :attr:`DelimitedReader.header` and never delivered as a row."""
    skip_blank_lines: bool = True
    """Whether a completely empty line is skipped rather than read as a one-cell row."""

    CSV: ClassVar[Dialect]
    """Comma-separated, quoted, with a header, blank lines skipped."""
    TSV: ClassVar[Dialect]
    """Tab-separated, otherwise as :data:`CSV`."""
    PSV: ClassVar[Dialect]
    """Pipe-separated, otherwise as :data:`CSV`."""

    def __post_init__(self) -> None:
        """Refuses a separator the scanner cannot honour — a caller bug, not a data verdict."""
        if not isinstance(self.separator, str):
            raise TypeError(f"separator must be a str, not {type(self.separator).__name__}")
        valid = len(self.separator) == 1 and (
            self.separator == "\t" or (" " <= self.separator <= "~" and self.separator != '"')
        )
        if not valid:
            raise ValueError(
                f"separator {self.separator!r} is not tab or a printable ASCII character other than '\"'"
            )


Dialect.CSV = Dialect(",")
Dialect.TSV = Dialect("\t")
Dialect.PSV = Dialect("|")


@dataclass(frozen=True, slots=True, repr=False)
class Column:
    """One output column of a plan: which source column it reads, the door it casts
    through, and — for the numeric doors — the notation. Build one with the factory named
    for its door (the names are HyperCast's: ``Column.i32`` is what ``hypercast.cast_i32``
    would make of each cell).

    A plan is a projection: a forty-column file can be read into five typed columns, in
    any order, and a source column can be read through more than one door.
    """

    ordinal: int
    """Zero-based ordinal of the source column. Past a record's last cell reads as empty."""
    door: Door
    """The door."""
    format: NumFormat = NumFormat.INVARIANT
    """The numeric notation, read by the numeric doors."""
    declared: UnixPrecision | DateOrder | ExcelEpoch | None = None
    """What the door declares beside itself: the Unix precision, the date order, or the
    Excel date system — never guessed."""

    def __post_init__(self) -> None:
        """Refuses an ordinal no source column can have."""
        if not isinstance(self.ordinal, int) or isinstance(self.ordinal, bool):
            raise TypeError(f"ordinal must be an int, not {type(self.ordinal).__name__}")
        if not 0 <= self.ordinal < 2**31:
            raise ValueError(f"ordinal must be between 0 and 2**31 - 1, not {self.ordinal}")

    def __repr__(self) -> str:
        """The column as the factory call that makes it."""
        said = [str(self.ordinal)]
        if self.declared is not None:
            said.append(f"{type(self.declared).__name__}.{self.declared.name}")
        if self.format is not NumFormat.INVARIANT:
            fmt = self.format
            said.append(
                f"NumFormat({fmt.decimal_sep!r}, {fmt.group_sep!r}, {fmt.flags}, {fmt.currency!r})"
            )
        return f"Column.{self.door.name.lower()}({', '.join(said)})"

    @classmethod
    def _numeric(cls, ordinal: int, door: Door, fmt: NumFormat) -> Column:
        if not isinstance(fmt, NumFormat):
            raise TypeError(f"fmt must be a hypercast.NumFormat, not {type(fmt).__name__}")
        return cls(ordinal, door, fmt)

    @classmethod
    def bool(cls, ordinal: int) -> Column:
        """A ``bool`` column: HyperCast's boolean lexicon."""
        return cls(ordinal, Door.BOOL)

    @classmethod
    def i8(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """A signed 8-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.I8, fmt)

    @classmethod
    def i16(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """A signed 16-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.I16, fmt)

    @classmethod
    def i32(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """A signed 32-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.I32, fmt)

    @classmethod
    def i64(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """A signed 64-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.I64, fmt)

    @classmethod
    def u8(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An unsigned 8-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.U8, fmt)

    @classmethod
    def u16(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An unsigned 16-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.U16, fmt)

    @classmethod
    def u32(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An unsigned 32-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.U32, fmt)

    @classmethod
    def u64(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An unsigned 64-bit integer column under the declared notation."""
        return cls._numeric(ordinal, Door.U64, fmt)

    @classmethod
    def f32(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An IEEE single column under the declared notation."""
        return cls._numeric(ordinal, Door.F32, fmt)

    @classmethod
    def f64(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An IEEE double column under the declared notation."""
        return cls._numeric(ordinal, Door.F64, fmt)

    @classmethod
    def decimal(cls, ordinal: int, fmt: NumFormat = NumFormat.INVARIANT) -> Column:
        """An exact ``decimal.Decimal`` column under the declared notation; no float is
        ever formed."""
        return cls._numeric(ordinal, Door.DECIMAL, fmt)

    @classmethod
    def uuid(cls, ordinal: int) -> Column:
        """A ``uuid.UUID`` column."""
        return cls(ordinal, Door.UUID)

    @classmethod
    def timestamp(cls, ordinal: int) -> Column:
        """An RFC 3339 instant column, as an aware UTC ``datetime``."""
        return cls(ordinal, Door.TIMESTAMP)

    @classmethod
    def unix(cls, ordinal: int, precision: UnixPrecision) -> Column:
        """A Unix-epoch column at the declared precision — never guessed from magnitude —
        as an aware UTC ``datetime``."""
        return cls(ordinal, Door.UNIX, declared=UnixPrecision(precision))

    @classmethod
    def excel_serial(cls, ordinal: int, epoch: ExcelEpoch) -> Column:
        """An Excel date-serial column under the declared date system, as an aware UTC
        ``datetime``."""
        return cls(ordinal, Door.EXCEL_SERIAL, declared=ExcelEpoch(epoch))

    @classmethod
    def date(cls, ordinal: int, order: DateOrder | None = None) -> Column:
        """A ``date`` column: strict ISO ``yyyy-MM-dd`` with no order, the separated forms
        under a declared one (which is :meth:`date_ordered`) — as ``hypercast.cast_date``."""
        if order is not None:
            return cls.date_ordered(ordinal, order)
        return cls(ordinal, Door.DATE)

    @classmethod
    def date_ordered(cls, ordinal: int, order: DateOrder) -> Column:
        """A separated-date column under the declared field order, as a ``date``."""
        return cls(ordinal, Door.DATE_ORDERED, declared=DateOrder(order))

    @classmethod
    def datetime(cls, ordinal: int, order: DateOrder) -> Column:
        """A zone-less civil date-time column under the declared field order, as a naive
        ``datetime`` — the text named no zone and none is invented."""
        return cls(ordinal, Door.DATETIME, declared=DateOrder(order))

    @classmethod
    def time(cls, ordinal: int) -> Column:
        """A 24-hour time-of-day column, as a ``time``."""
        return cls(ordinal, Door.TIME)

    @classmethod
    def duration(cls, ordinal: int) -> Column:
        """A duration column, as a ``timedelta``."""
        return cls(ordinal, Door.DURATION)

    @classmethod
    def text(cls, ordinal: int) -> Column:
        """A text column: the cell's bytes themselves, untrimmed, as a ``str``. A cell
        with no bytes at all is the one way text fails (``CastFailure.EMPTY``)."""
        return cls(ordinal, Door.TEXT)


@dataclass(frozen=True, slots=True)
class SheetOptions:
    """How a sheet of a :class:`Workbook` is read."""

    has_header: bool = True
    """Whether the sheet's first row is a header, exposed through :attr:`Sheet.header` and
    never delivered as a row."""
    skip_empty_rows: bool = True
    """Whether a row with no cells is skipped rather than delivered as a row of empty
    cells."""
    batch_rows: int = 4096
    """Rows per batch, at most."""

    def __post_init__(self) -> None:
        """Refuses a batch size no batch can have."""
        if not isinstance(self.batch_rows, int) or isinstance(self.batch_rows, bool):
            raise TypeError(f"batch_rows must be an int, not {type(self.batch_rows).__name__}")
        if self.batch_rows < 1:
            raise ValueError(f"batch_rows must be at least 1, not {self.batch_rows}")


# The extension's own classes are the package surface. _bind hands it this package's plan
# column, dialect, structural failure and workbook format; it imports HyperCast's verdict
# types itself.
_native._bind(Column, Dialect, TabularError, TabularFailure, WorkbookFormat)

DelimitedReader = _native.DelimitedReader
Workbook = _native.Workbook
Sheet = _native.Sheet
SheetInfo = _native.SheetInfo
Batch = _native.Batch
ColumnData = _native.ColumnData
native_version = _native.native_version
