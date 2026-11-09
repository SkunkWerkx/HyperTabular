"""Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS
and Numbers — read a batch at a time into typed columns, with a
`HyperCast <https://github.com/SkunkWerkx/HyperCast>`_ verdict for every cell.

The native core owns no memory and reads no files. It is linked straight into a CPython
extension module (``hypertabular._native``, PyO3), and the extension is the binding, over the
Rust API: the Rust reader's buffers are filled in one call per batch, with the interpreter
released, and each batch is copied out of them. A column then reaches Python whole — a
read-only ``memoryview`` of the door's own item type over the batch's copy of that array,
which anything that takes a buffer takes — with no Python call per cell::

    from hypertabular import Column, DelimitedReader, Dialect, Fault, Success

    # The header first, then a plan that names the columns it reads.
    with DelimitedReader.open("orders.csv", Dialect.CSV) as reader:
        header = reader.header
        plan = [
            Column.i32(header.ordinal("id")),
            Column.text(header.ordinal("name")),
            Column.f64(header.ordinal("score")),
        ]
        reader.bind(plan)
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

    # A row at a time, across batches.
    with DelimitedReader.open("orders.csv", Dialect.CSV, plan) as reader:
        for row in reader.rows():
            print(row.line, row.text(1), row.get(2))

    # A workbook reads into the same batch.
    book = Workbook.open("orders.xlsx")
    for batch in book.sheet("Orders", SheetOptions(), plan):
        ...

    # And asyncio: a reader over an asyncio.StreamReader, or anything with async read(size).
    reader = await DelimitedReader.open_async(stream, Dialect.CSV, plan)
    async for batch in reader:
        ...

- **Nothing is sniffed.** The :class:`Dialect` states the separator, the quoting, the header,
  what to pass over, what a record of the wrong width is, the encoding and what stands for no
  value; :class:`SheetOptions` states a sheet's header, what to pass over, its range and whether
  empty rows are skipped; the plan states each column's door and, for numbers, its ``NumFormat``.
  :meth:`DelimitedReader.suggest` offers a plan to inspect, from a sample, and applies nothing.
- **HyperCast is the judge.** :class:`Success`, :class:`Fault`, :class:`CastFailure`,
  :class:`NumFormat`, :class:`UnixPrecision`, :class:`DateOrder`, :class:`ExcelEpoch`,
  :class:`Detail`, :class:`DateOptions` and :class:`MonthNames` are the ``hypercast`` package's own objects, re-exported here unchanged — not copies. A
  text cell means exactly what ``hypercast.cast_*`` says of the same text, a typed workbook
  cell is converted by the door directly, and its value is the
  Python type that door gives: ``int``, ``float``, ``bool``, ``decimal.Decimal``,
  ``uuid.UUID``, ``datetime``, ``date``, ``time``, ``timedelta`` (and ``str`` for text).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a ``Fault`` in its column and the read goes on. A record of the wrong width, input
  that ends inside a quoted cell, a workbook whose container or parts cannot be read,
  raises :class:`TabularError` — after every intact row before it has been delivered.
- **A batch owns what it shows.** It stays valid after the reader has moved on, and so do a
  :class:`Row` of it and its :meth:`Batch.faults` list.
- **Free-threaded CPython runs it without the GIL.** An ``abi3t`` wheel ships for 3.15 and
  newer, beside the ``abi3`` one for every CPython 3.11+. A batch and a workbook
  can be shared by any number of threads; a reader or a sheet is read by one at a time, and
  another thread calling into it meanwhile gets ``RuntimeError``, never a race.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import IntEnum
from typing import Any, ClassVar, Self, overload

from hypercast import (
    CastFailure,
    DateOptions,
    DateOrder,
    Detail,
    ExcelEpoch,
    Fault,
    MonthNames,
    NumFormat,
    Success,
    UnixPrecision,
    Verdict,
)

from . import _native

__all__ = [
    "native_version",
    "DelimitedReader",
    "Workbook",
    "WorkbookFormat",
    "Sheet",
    "SheetInfo",
    "SheetOptions",
    "Range",
    "Batch",
    "ColumnData",
    "Row",
    "Rows",
    "Header",
    "AmbiguousColumn",
    "Dialect",
    "ShortRows",
    "LongRows",
    "Encoding",
    "Column",
    "Door",
    "Origin",
    "Problem",
    "SuggestOptions",
    "SuggestStatus",
    "SuggestedColumn",
    "Suggestion",
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
    "Detail",
    "DateOptions",
    "MonthNames",
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
    """The zip is not an XLSX, ODS or Numbers workbook."""
    SHARED_STRING = 24
    """A cell names a shared string the table does not have."""
    TOO_LARGE = 25
    """More text than can be addressed: over 4 GiB in a batch or in the shared strings, or
    2 GiB in a cell."""
    IWA = 26
    """A Numbers part is not what the IWA packaging says: ``found`` is 1 for a broken chunk
    or Snappy block, 2 for a malformed message, 3 for an object a reference names that is
    not there (``expected`` is its identifier), 4 for cell storage of a version the core
    does not read (``expected`` is the version)."""
    ENCODING = 27
    """The input holds a unit its declared :class:`Encoding` has no character for. Its
    :attr:`TabularError.byte` alone counts the original bytes, not the decoded text."""


class WorkbookFormat(IntEnum):
    """Which kind of workbook a :class:`Workbook` is."""

    XLSX = 1
    """Office Open XML: ``.xlsx``, ``.xlsm``."""
    ODS = 2
    """OpenDocument: ``.ods``."""
    NUMBERS = 3
    """Apple Numbers: ``.numbers``, each table a sheet."""


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
    """Absolute byte offset of the offending record's start, in the decoded text under a declared
    encoding. For :data:`TabularFailure.ENCODING`, where the unit is in the original input."""
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
                return "the zip is not an XLSX, ODS or Numbers workbook"
            case TabularFailure.SHARED_STRING:
                return f"row {self.line} names shared string {self.found}; the table has {self.expected}"
            case TabularFailure.TOO_LARGE:
                return "the workbook holds more text than a batch can address"
            case TabularFailure.IWA:
                return (
                    f"{part} of the Numbers document is not what the IWA packaging says"
                    f" (byte {self.byte}, detail {self.found})"
                )
            case TabularFailure.ENCODING:
                return (
                    f"record {self.record} (line {self.line}) holds a unit its declared encoding"
                    f" has no character for, at byte {self.byte} of the input"
                )
            case _:
                return "the workbook's zip structure is broken"


class ShortRows(IntEnum):
    """What a record with fewer cells than the width is."""

    REJECT = 0
    """A structural failure."""
    PAD = 1
    """A row whose missing cells are ``Empty``, at ``Origin.PADDED``."""


class LongRows(IntEnum):
    """What a record with more cells than the width is."""

    REJECT = 0
    """A structural failure."""
    REPORT = 1
    """A row cut to the width, and listed by :meth:`Batch.faults` with the width it had."""
    TRUNCATE = 2
    """A row cut to the width, and nothing said."""


class Encoding(IntEnum):
    """What a delimited file's bytes are, declared and never sniffed. A workbook declares its own."""

    UTF8 = 0
    """UTF-8, read in place. A cell that isn't valid is that cell's ``MALFORMED`` verdict."""
    WINDOWS_1252 = 1
    """Windows-1252. The five bytes the code page leaves undefined are their C1 controls, as
    WHATWG has them. ``EF BB BF`` is three characters here, not a byte-order mark."""
    LATIN_1 = 2
    """Latin-1 (ISO-8859-1): every byte the code point of its value."""
    UTF_16LE = 3
    """UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code
    unit at the end of the input, is :data:`TabularFailure.ENCODING`."""


class Origin(IntEnum):
    """Where a cell's verdict came from: :meth:`Batch.origin`."""

    CELL = 0
    """A door judged the cell's own text."""
    TOKEN = 1
    """The cell's exact unquoted bytes are a declared token, so it is ``Empty`` and no door ran."""
    PADDED = 2
    """The row has no such cell: padding for a short row, or an ordinal past the row's end."""
    UNCACHED = 3
    """A workbook's formula cell with no cached value. It is ``Empty`` and no door ran."""


class SuggestStatus(IntEnum):
    """What a sample says of a column."""

    RESOLVED = 1
    """One door and one declaration take every cell, and the plan reads the column through them."""
    AMBIGUOUS = 2
    """Every cell is read, but the sample can't say which declaration it's written in, as ``1,234``
    can't: :attr:`SuggestedColumn.candidates` are the choices. The plan reads it as text."""
    UNRESOLVED = 3
    """There's nothing to go on, or a cell no door took beside cells one did. The plan reads it as
    text."""


def _count(name: str, value: int) -> None:
    """Refuses what no count of lines or rows can be."""
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{name} must be an int, not {type(value).__name__}")
    if not 0 <= value < 2**32:
        raise ValueError(f"{name} must fit an unsigned 32-bit count, not {value}")


def _tokens(tokens: Iterable[str]) -> tuple[str, ...]:
    """A token list as a tuple of ``str``; a lone ``str`` is too easy a mistake to take a letter at
    a time."""
    if isinstance(tokens, (str, bytes)):
        raise TypeError("tokens must be a list of str, not one token")
    listed = tuple(tokens)
    for token in listed:
        if not isinstance(token, str):
            raise TypeError(f"a token must be a str, not {type(token).__name__}")
    return listed


@dataclass(frozen=True, slots=True)
class Dialect:
    """How the text is delimited, declared by the caller. Nothing is sniffed: the separator
    is stated, quoting is stated, the header is stated, the encoding is stated — the same stance
    HyperCast's ``NumFormat`` takes for numeric notation.
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
    skip: int = 0
    """Physical lines to pass over after a byte-order mark, before anything is read of them: a
    quote in one means nothing."""
    header_row: int = 0
    """Records to discard after those, ahead of the header if there is one. A blank line this
    dialect skips isn't a record."""
    short_rows: ShortRows = ShortRows.REJECT
    """What a record with fewer cells than the width is."""
    long_rows: LongRows = LongRows.REJECT
    """What a record with more cells than the width is."""
    encoding: Encoding = Encoding.UTF8
    """What the input's bytes are. Never sniffed, and anything but UTF-8 is decoded ahead of the
    read, so that a batch's text and every span are the decoded text's."""
    tokens: Sequence[str] = ()
    """What stands for no value in every column without tokens of its own: a cell whose exact,
    unquoted bytes are one of these is ``Empty``, whatever its door, at ``Origin.TOKEN``.

    Each distinct list is kept for the life of the process once a reader is made with it: a program
    building lists from request data grows without bound. Declare a fixed list."""

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
        _count("skip", self.skip)
        _count("header_row", self.header_row)
        object.__setattr__(self, "short_rows", ShortRows(self.short_rows))
        object.__setattr__(self, "long_rows", LongRows(self.long_rows))
        object.__setattr__(self, "encoding", Encoding(self.encoding))
        object.__setattr__(self, "tokens", _tokens(self.tokens))


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
    tokens: tuple[str, ...] | None = None
    """The tokens this column answers to, in place of its dialect's or its sheet's: see
    :meth:`with_tokens`."""
    date_options: DateOptions | None = None
    """What the date doors accept beyond their own shapes: see :meth:`with_date_options`."""
    months: MonthNames | None = None
    """The month vocabulary the date doors read: see :meth:`with_date_options`."""

    def __post_init__(self) -> None:
        """Refuses an ordinal no source column can have."""
        if not isinstance(self.ordinal, int) or isinstance(self.ordinal, bool):
            raise TypeError(f"ordinal must be an int, not {type(self.ordinal).__name__}")
        # No workbook format has 2^20 columns (XLSX and ODS stop at 16,384, Numbers at
        # 1,000), and at the cap one row of cell table is at most 8 MiB.
        if not 0 <= self.ordinal < 2**20:
            raise ValueError(
                f"ordinal must be at least 0, and an ordinal must be below 2^20 (1,048,576);"
                f" got {self.ordinal}"
            )

    def __repr__(self) -> str:
        """The column as the factory call that makes it."""
        said = [str(self.ordinal)]
        if self.declared is not None:
            said.append(f"{type(self.declared).__name__}.{self.declared.name}")
        if self.format is not NumFormat.INVARIANT:
            said.append(repr(self.format))
        made = f"Column.{self.door.name.lower()}({', '.join(said)})"
        if self.tokens is not None:
            made += f".with_tokens({list(self.tokens)!r})"
        if self.date_options is not None or self.months is not None:
            made += f".with_date_options({self.date_options!r}, {self.months!r})"
        return made

    def with_tokens(self, tokens: Iterable[str]) -> Column:
        """The same column answering to these tokens rather than its dialect's or its sheet's: a
        cell whose exact, unquoted bytes are one of them is ``Empty``, whatever its door, at
        ``Origin.TOKEN``. An empty list means none, so a column can opt out. On a sheet, only a text
        cell can be one.

        Each distinct list is kept for the life of the process once a plan holding it is bound: a
        program building lists from request data grows without bound."""
        return replace(self, tokens=_tokens(tokens))

    def with_date_options(
        self, options: DateOptions | None = None, months: MonthNames | None = None
    ) -> Column:
        """The same column reading dates under these options and this month vocabulary: the
        ``timestamp``, ``date``, ``date_ordered`` and ``datetime`` doors read them, as HyperCast's
        own do. A declaration HyperCast refuses raises ``ValueError`` when the plan is bound. On a
        sheet they reach only a text cell, since a typed date cell has no text for them to read.

        Each distinct vocabulary is kept for the life of the process once a plan holding it is
        bound: a program building them from request data grows without bound."""
        return replace(self, date_options=options, months=months)

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


class AmbiguousColumn(ValueError):
    """A name :meth:`Header.ordinal` was asked for that the header gives to more than one column.
    A plan has to say which by position; :meth:`Header.get` keeps the first."""

    name: str
    """The name."""
    first: int
    """The first column holding it, from zero."""
    second: int
    """The second column holding it, from zero."""

    def __init__(self, name: str, first: int, second: int) -> None:
        """Builds the refusal :meth:`Header.ordinal` raises; the arguments are its attributes."""
        super().__init__(name, first, second)
        self.name = name
        self.first = first
        self.second = second

    def __str__(self) -> str:
        """The refusal in words, with both positions."""
        return (
            f'the header names two columns "{self.name}", at positions {self.first} and'
            f" {self.second}: say which by position"
        )


class Header(tuple[str, ...]):
    """The names a source's header declares, in column order: a tuple of ``str`` — which is
    what the header always was, so code that treats it as one goes on working — that also
    looks a name up, for a plan whose ordinals come from the names rather than from where
    the publisher happened to put each column::

        reader = DelimitedReader.open("orders.csv", Dialect.CSV)
        header = reader.header
        reader.bind([Column.i32(header.ordinal("id")), Column.text(header.ordinal("name"))])

    A lookup is exact — case, spaces and all, nothing trimmed. :meth:`ordinal` refuses a name two
    columns share, and :meth:`get` keeps the first. A ``bytes`` name is matched against the name's
    UTF-8: a read's names all are, since one that isn't fails the open. A ``Header`` made by hand
    can name other bytes with ``raw``.
    """

    _raw: tuple[bytes, ...]

    def __new__(cls, names: Iterable[str] = (), raw: Iterable[bytes] | None = None) -> Self:
        """The header of ``names``; ``raw`` is the bytes each was read from (by default,
        each name encoded as UTF-8)."""
        header = super().__new__(cls, names)
        header._raw = (
            tuple(raw) if raw is not None else tuple(name.encode("utf-8") for name in header)
        )
        if len(header._raw) != len(header):
            raise ValueError("raw must have one entry for each name")
        return header

    def ordinal(self, name: str | bytes) -> int:
        """The ordinal of the one column named ``name`` — what a :class:`Column` factory takes. A
        name the header does not have raises ``KeyError``, naming it, and one that two columns share
        raises :class:`AmbiguousColumn`, naming both: a plan bound to the first would read the wrong
        column without a word. Say which by position, or take :meth:`get`'s first match."""
        first = self.get(name)
        if first is None:
            raise KeyError(f"the header has no column named {name!r}")
        names: Sequence[str | bytes] = self if isinstance(name, str) else self._raw
        for second in range(first + 1, len(names)):
            if names[second] == name:
                said = name if isinstance(name, str) else name.decode("utf-8", "replace")
                raise AmbiguousColumn(said, first, second)
        return first

    @overload
    def get(self, name: str | bytes) -> int | None: ...
    @overload
    def get(self, name: str | bytes, default: int) -> int: ...
    def get(self, name: str | bytes, default: int | None = None) -> int | None:
        """The ordinal of the first column named ``name``, or ``default`` (``None``) when the
        header has none. It keeps the first match of a name two columns share, where
        :meth:`ordinal` refuses."""
        names: tuple[str, ...] | tuple[bytes, ...]
        if isinstance(name, str):
            names = self
        elif isinstance(name, bytes):
            names = self._raw
        else:
            raise TypeError(f"a column name is a str or bytes, not {type(name).__name__}")
        try:
            return names.index(name)
        except ValueError:
            return default

    def __repr__(self) -> str:
        """The header as the call that makes it."""
        return f"Header({tuple(self)!r})"


@dataclass(frozen=True, slots=True)
class SheetOptions:
    """How a sheet of a :class:`Workbook` is read."""

    has_header: bool = True
    """Whether the sheet's first row is a header, exposed through :attr:`Sheet.header` and
    never delivered as a row."""
    skip_empty_rows: bool = True
    """Whether a row with no cells is skipped rather than delivered as a row of empty
    cells. **Set this to ``False`` to keep every row.** A row whose only cells are formulas with no
    cached value counts as empty, and nothing says it was left out; off, it's delivered, its cells
    at ``Origin.UNCACHED``."""
    batch_rows: int = 4096
    """Rows per batch, at most."""
    skip: int = 0
    """Rows to pass over at the start of the range, empty ones included. Their cells aren't
    judged."""
    header_row: int = 0
    """Rows to read and discard after those, as ``skip_empty_rows`` delivers them."""
    range: _native.Range | None = None
    """The part of the sheet to read, or all of it."""
    tokens: Sequence[str] = ()
    """What stands for no value in a text cell, for every column without tokens of its own. Each
    distinct list is kept for the life of the process, as :attr:`Dialect.tokens` says."""
    notes: bool = True
    """Whether a batch keeps each cell's detail and origin. Off, a read costs a little less, a
    ``Fault`` has no ``detail``, ``origin`` says ``Origin.CELL``, and :meth:`Batch.faults` can't list
    a formula with no cached value."""

    def __post_init__(self) -> None:
        """Refuses what no read can be told."""
        if not isinstance(self.batch_rows, int) or isinstance(self.batch_rows, bool):
            raise TypeError(f"batch_rows must be an int, not {type(self.batch_rows).__name__}")
        if not 0 <= self.batch_rows < 2**64:
            raise ValueError(f"batch_rows must fit an unsigned 64-bit count, not {self.batch_rows}")
        _count("skip", self.skip)
        _count("header_row", self.header_row)
        if self.range is not None and not isinstance(self.range, _native.Range):
            raise TypeError(f"range must be a Range, not {type(self.range).__name__}")
        object.__setattr__(self, "tokens", _tokens(self.tokens))


@dataclass(frozen=True, slots=True)
class Problem:
    """One thing that went wrong in a :class:`Batch`: :meth:`Batch.faults`."""

    row: int
    """The row in the batch, from zero."""
    line: int
    """Where the row came from: :meth:`Batch.line`."""
    plan_column: int | None
    """The column of the plan, or ``None`` for an entry about the row as a whole."""
    source_column: int | None
    """The column in the source, from zero, or ``None`` for an entry about the row as a whole. For
    delimited text it's the plan column's ordinal, and for a sheet its own column, ``A`` being
    ``0``, wherever the range starts."""
    reason: CastFailure | None
    """The verdict's reason: ``MALFORMED``, ``OUT_OF_RANGE``, or ``EMPTY`` for a formula with no
    cached value. ``None`` for an entry about the row as a whole."""
    detail: Detail | None
    """What failed in the cell, as a ``Fault``'s ``detail`` says."""
    detail_code: int | None
    """The core's number for it, which outlasts a :class:`Detail` that has no name for it."""
    origin: Origin
    """Where the verdict came from. An entry about the row as a whole says ``Origin.CELL``."""
    offset: int
    """The fault's span within the cell's text, in bytes; ``0`` where there's no span."""
    length: int
    """The fault's span's length in bytes; ``0`` where there's no span."""
    expected: int
    """For a long row, the width the first record fixed; otherwise ``0``."""
    found: int
    """For a long row, the cells it had; otherwise ``0``."""


def _pairs(
    name: str, entries: Mapping[int, Any] | Iterable[tuple[int, Any]]
) -> tuple[tuple[int, Any], ...]:
    """Per-column declarations in the order given, the last one for a column being the one that
    counts."""
    listed = tuple(entries.items() if isinstance(entries, Mapping) else entries)
    for entry in listed:
        ordinal = entry[0]
        if not isinstance(ordinal, int) or isinstance(ordinal, bool):
            raise TypeError(f"{name}: an ordinal must be an int, not {type(ordinal).__name__}")
        if ordinal < 0:
            raise ValueError(f"{name}: an ordinal must be at least 0, not {ordinal}")
    return listed


@dataclass(frozen=True, slots=True)
class SuggestOptions:
    """How a sample is taken and read: :meth:`DelimitedReader.suggest`."""

    sample_rows: int = 1000
    """The most rows the sample holds. ``0`` is taken as ``1``. It stops at 4096 rows of delimited text,
    and at a sheet's own batch size, read in as many batches as the core says fit: a sample is never
    refused for the size of its batch."""
    format: NumFormat | None = None
    """Reads every column's numbers in this format, which settles what ``,`` and ``.`` mean in them.
    Indian grouping declared makes ``1,23,456`` a number and ``1,234,567`` text."""
    column_formats: Mapping[int, NumFormat] | Iterable[tuple[int, NumFormat]] = ()
    """Per source column, ahead of :attr:`format`, the last given winning: ``{0: indian}``, or pairs.
    Every format given is judged first of anything, whether it's replaced or the sample ever reaches
    its ordinal."""
    date_order: DateOrder | None = None
    """Reads every column's dates in this order where the sample leaves the order open, as
    ``01/02/2026`` does. A column whose cells rule that order out keeps the order they leave."""
    column_date_orders: Mapping[int, DateOrder] | Iterable[tuple[int, DateOrder]] = ()
    """Per source column, ahead of :attr:`date_order`, the last given winning."""

    def __post_init__(self) -> None:
        """Refuses what no sample can be told. A negative count of rows is refused; a format that
        can't be honoured is the suggest's to refuse."""
        if not isinstance(self.sample_rows, int) or isinstance(self.sample_rows, bool):
            raise TypeError(f"sample_rows must be an int, not {type(self.sample_rows).__name__}")
        if self.sample_rows < 0:
            raise ValueError(f"sample_rows must be at least 0, not {self.sample_rows}")
        object.__setattr__(self, "column_formats", _pairs("column_formats", self.column_formats))
        object.__setattr__(
            self,
            "column_date_orders",
            tuple(
                (ordinal, DateOrder(order))
                for ordinal, order in _pairs("column_date_orders", self.column_date_orders)
            ),
        )
        if self.date_order is not None:
            object.__setattr__(self, "date_order", DateOrder(self.date_order))

    def _declared(self, ordinal: int) -> NumFormat | None:
        """The format declared for source column ``ordinal``."""
        declared: tuple[tuple[int, NumFormat], ...] = self.column_formats  # type: ignore[assignment]
        for entry in reversed(declared):
            if entry[0] == ordinal:
                return entry[1]
        return self.format


@dataclass(frozen=True, slots=True)
class SuggestedColumn:
    """What a sample says of one source column."""

    ordinal: int
    """The zero-based source column, as a :class:`Column` names it."""
    name: str | None
    """The header's name for it, if the source has a header."""
    door: Door
    """The door found. Text when unresolved, and under the first candidate when ambiguous."""
    declared: UnixPrecision | DateOrder | ExcelEpoch | None
    """What that door declares beside itself."""
    format: NumFormat | None
    """The notation the plan reads a resolved numeric column in: the one declared for it, or else
    the one candidate the sample left, or otherwise ``NumFormat.INVARIANT``. Both of those are
    Western, so a later ``1,23,456`` is refused. ``None`` for any other column."""
    candidates: tuple[DateOrder | NumFormat, ...]
    """What to choose between: empty unless ambiguous. Declare one with
    :attr:`SuggestOptions.column_date_orders` or :attr:`SuggestOptions.column_formats`, and ask again,
    since the door can differ under each."""
    status: SuggestStatus
    """How far the sample settles it."""
    matched: int
    """The cells :attr:`door` took. Resolved or ambiguous, that's all of :attr:`sampled`. Unresolved,
    it's the most that any one door but the text door took."""
    sampled: int
    """The cells that aren't blank."""


@dataclass(frozen=True, slots=True)
class Suggestion:
    """What :meth:`DelimitedReader.suggest`, :meth:`DelimitedReader.suggest_bytes` and
    :meth:`Sheet.suggest` found: a plan to change or to pass to a read, and why."""

    plan: tuple[Column, ...]
    """A column for each source column, in order. Only a resolved column reads through its door: the
    rest read as text, which refuses nothing and loses nothing."""
    report: tuple[SuggestedColumn, ...]
    """A report for each source column, in the plan's order."""
    truncated: bool
    """Whether the source has rows past the sample. A source with exactly the sample's rows
    hasn't."""


_COMMA_DECIMAL = NumFormat(",", ".", NumFormat.ALL)
"""The one notation a suggestion can name besides Western and what's declared."""


def _declaring(door: Door, param: int) -> UnixPrecision | DateOrder | ExcelEpoch | None:
    """What the core's ``param`` says beside a door, every door covered."""
    match door:
        case Door.UNIX:
            return UnixPrecision(param)
        case Door.DATE_ORDERED | Door.DATETIME:
            return DateOrder(param)
        case Door.EXCEL_SERIAL:
            return ExcelEpoch(param)
        case _:
            return None


def _suggestion(
    columns: list[tuple[tuple[object, ...], tuple[int, int, int]]],
    truncated: bool,
    options: SuggestOptions | None,
) -> Suggestion:
    """The extension's answer as the package's own objects: a format's kind is ``1`` Western, ``2``
    the comma decimal, and ``0`` what the caller declared. A format the caller declared comes back
    as the caller's own object, whichever kind it is."""

    def named(kind: int, ordinal: int) -> NumFormat:
        made = NumFormat.INVARIANT if kind == 1 else _COMMA_DECIMAL if kind == 2 else None
        declared = None if options is None else options._declared(ordinal)
        if declared is not None and (made is None or declared.packed == made.packed):
            return declared
        assert made is not None, "the core names no other format"
        return made

    plan = []
    report = []
    for said, planned in columns:
        ordinal, name, door, param, kind, candidates, status, matched, sampled = said
        assert isinstance(ordinal, int) and isinstance(door, int) and isinstance(param, int)
        assert isinstance(candidates, list)
        found = Door(door)
        report.append(
            SuggestedColumn(
                ordinal,
                name,  # type: ignore[arg-type]
                found,
                _declaring(found, param),
                None if kind is None else named(kind, ordinal),  # type: ignore[arg-type]
                tuple(
                    DateOrder(code) if which == 0 else named(code, ordinal)
                    for which, code in candidates
                ),
                SuggestStatus(status),  # type: ignore[arg-type]
                matched,  # type: ignore[arg-type]
                sampled,  # type: ignore[arg-type]
            )
        )
        into = Door(planned[0])
        plan.append(Column(ordinal, into, named(planned[2], ordinal), _declaring(into, planned[1])))
    return Suggestion(tuple(plan), tuple(report), truncated)


# The extension's own classes are the package surface. _bind hands it this package's plan
# column, dialect, structural failure, workbook format, header, faults entry, origin and suggestion;
# it imports HyperCast's verdict types itself.
_native._bind(
    Column,
    Dialect,
    TabularError,
    TabularFailure,
    WorkbookFormat,
    Header,
    Problem,
    Origin,
    SuggestOptions,
    _suggestion,
)

DelimitedReader = _native.DelimitedReader
Workbook = _native.Workbook
Sheet = _native.Sheet
SheetInfo = _native.SheetInfo
Range = _native.Range
Batch = _native.Batch
ColumnData = _native.ColumnData
Row = _native.Row
Rows = _native.Rows
native_version = _native.native_version
