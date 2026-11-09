"""What a sample of a source would have a caller declare: a plan and a report, nothing applied. A
port of ``rust/tests/suggest.rs``, which says what each of these has to be.

A suggest takes a path or bytes here, as Rust's does, never a stream. So the I/O failure past a
whole sample that Rust's and Go's stub sources prove truncated can't be staged from this surface:
it's Rust's own path, and the corpus's cases hold what it shares.
"""

from __future__ import annotations

import io
import zipfile
from enum import IntEnum
from pathlib import Path
from typing import Any

import pytest
from test_corpus import _AsyncDribble, _corpus_dir, _run

import hypertabular
from hypertabular import (
    CastFailure,
    Column,
    DateOrder,
    DelimitedReader,
    Dialect,
    Door,
    Encoding,
    ExcelEpoch,
    LongRows,
    NumFormat,
    SheetOptions,
    ShortRows,
    Success,
    SuggestOptions,
    SuggestStatus,
    Suggestion,
    TabularError,
    TabularFailure,
    UnixPrecision,
    Workbook,
)

WORKBOOKS = _corpus_dir() / "workbook"
COMMA = NumFormat(",", ".", NumFormat.ALL)
INDIAN = NumFormat.INVARIANT.with_grouping(NumFormat.GROUPING_INDIAN)
NUMERIC = {
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
Resolved, Ambiguous, Unresolved = (
    SuggestStatus.RESOLVED,
    SuggestStatus.AMBIGUOUS,
    SuggestStatus.UNRESOLVED,
)


def same(found: Any, wanted: NumFormat) -> bool:
    """Whether a suggestion found the wanted format, by HyperCast's value equality."""
    return found is not None and found == wanted


def suggest(
    data: bytes, options: SuggestOptions | None = None, dialect: Dialect = Dialect.CSV
) -> Suggestion:
    """What a sample of ``data`` suggests."""
    return DelimitedReader.suggest_bytes(data, dialect, options)


def doors(found: Suggestion) -> list[Door]:
    """The doors a report names."""
    return [column.door for column in found.report]


def rows_of(count: int) -> bytes:
    """A column ``a`` of ``count`` rows of numbers."""
    return b"a\n" + b"".join(b"%d\n" % (row + 1) for row in range(1, count + 1))


def one(data: bytes, plan: Any, dialect: Dialect = Dialect.CSV) -> Any:
    """The one batch ``data`` reads as."""
    (batch,) = list(DelimitedReader(data, dialect, plan))
    return batch


def test_a_report_says_what_the_plan_does_and_why() -> None:
    """A report says what the plan does and why."""
    found = suggest(
        b"flag,id,big,score,day,at,key,name,mixed,sep,order,none\n"
        b'true,1,5000000000,2.5,2026-01-07,2026-01-07T00:00:00Z,6ba7b810-9dad-11d1-80b4-00c04fd430c8,abc,1,"1,234",01/02/2026,\n'
        b'false,2,6000000000,3,2026-01-08,2026-01-08T00:00:00Z,6ba7b811-9dad-11d1-80b4-00c04fd430c8,def,x,"2,345",03/04/2026,\n'
    )
    assert doors(found) == [
        Door.BOOL,
        Door.I32,
        Door.I64,
        Door.F64,
        Door.DATE,
        Door.TIMESTAMP,
        Door.UUID,
        Door.TEXT,
        Door.TEXT,
        Door.I32,
        Door.DATE_ORDERED,
        Door.TEXT,
    ]
    assert found.report[10].declared is DateOrder.MONTH_DAY_YEAR
    # Only what's resolved is read through a door: the last four read as text.
    assert [column.door for column in found.plan] == [
        Door.BOOL,
        Door.I32,
        Door.I64,
        Door.F64,
        Door.DATE,
        Door.TIMESTAMP,
        Door.UUID,
        Door.TEXT,
        Door.TEXT,
        Door.TEXT,
        Door.TEXT,
        Door.TEXT,
    ]
    assert [column.status for column in found.report] == [
        *[Resolved] * 8,
        Unresolved,
        Ambiguous,
        Ambiguous,
        Unresolved,
    ]
    for ordinal, column in enumerate(found.report):
        assert column.ordinal == ordinal and found.plan[ordinal].ordinal == ordinal
    assert found.report[0].name == "flag" and found.report[11].name == "none"
    assert (found.report[8].matched, found.report[8].sampled) == (1, 2)
    assert (found.report[11].matched, found.report[11].sampled) == (0, 0)
    assert (found.report[9].matched, found.report[9].sampled) == (2, 2)
    assert found.report[0].candidates == ()
    assert found.report[10].candidates == (DateOrder.MONTH_DAY_YEAR, DateOrder.DAY_MONTH_YEAR)
    first, second = found.report[9].candidates
    assert isinstance(first, NumFormat) and isinstance(second, NumFormat)
    assert same(first, NumFormat.INVARIANT)
    assert (second.decimal_sep, second.group_sep) == (",", ".")
    assert found.truncated is False


def test_a_plan_reads_back_what_was_sampled() -> None:
    """A plan reads back what was sampled."""
    data = b"flag,id,score,day,order,name\n1,2,2.5,2026-01-07,13/02/2026,abc\n0,3,3,2026-01-08,01/02/2026,def\n"
    found = suggest(data)
    batch = one(data, found.plan)
    assert batch.rows == 2 and batch.faults() == []
    assert list(batch.column(0).values) == [True, False]
    assert batch.column(1).values.tolist() == [2, 3]
    assert batch.column(2).values.tolist() == [2.5, 3.0]
    assert batch.text(5, 1) == "def"


@pytest.mark.parametrize("door", list(Door), ids=lambda door: door.name)
def test_every_door_the_extension_can_name_is_a_column_that_binds(door: Door) -> None:
    """The core names nine doors today and the door table covers all of them, since a door it comes
    to name has to work on the day it does. So this drives what it answers from here, every door
    and each one's declared parameter, and binds the plan."""
    declared: dict[Door, tuple[type[IntEnum], ...]] = {
        Door.UNIX: (UnixPrecision,),
        Door.DATE_ORDERED: (DateOrder,),
        Door.DATETIME: (DateOrder,),
        Door.EXCEL_SERIAL: (ExcelEpoch,),
    }
    kinds = declared.get(door)
    for param in [int(member) for member in kinds[0]] if kinds else [0]:
        found = hypertabular._suggestion(
            [((0, None, int(door), param, None, [], 1, 1, 1), (int(door), param, 1))], False, None
        )
        column, said = found.plan[0], found.report[0]
        assert column.door == door and said.door == door
        if kinds:
            assert type(column.declared) is kinds[0] and column.declared == param
            assert type(said.declared) is kinds[0] and said.declared == param
        else:
            assert column.declared is None and said.declared is None
        assert DelimitedReader(b"a\n1\n", Dialect.CSV, [column]).read() is not None


def test_a_declared_format_reaches_the_plan() -> None:
    """A declared format reaches the plan."""
    data = b'a,b\n"1,234","1,234"\n'
    found = suggest(data, SuggestOptions(format=NumFormat.INVARIANT, column_formats={1: COMMA}))
    assert doors(found) == [Door.I32, Door.F64]
    assert found.plan[1].format.decimal_sep == ","
    batch = one(data, found.plan)
    assert batch.column(0).values.tolist() == [1234]
    assert batch.column(1).values.tolist() == [1.234]


def test_the_last_format_declared_for_a_column_is_its_format() -> None:
    """The last format declared for a column is its format."""
    options = SuggestOptions(column_formats=[(0, NumFormat.INVARIANT), (0, COMMA)])
    found = suggest(b'a\n"1,234"\n', options)
    assert doors(found) == [Door.F64]
    # What the caller declared comes back as the caller's own object, whichever notation it is.
    assert found.report[0].format is COMMA and found.plan[0].format is COMMA
    # What neither of the two notations is comes back as the caller's own object.
    options = SuggestOptions(column_formats=[(0, NumFormat.INVARIANT), (0, INDIAN)])
    found = suggest(b'a\n"1,23,456"\n', options)
    assert doors(found) == [Door.I32]
    assert found.report[0].format is INDIAN and found.plan[0].format is INDIAN
    # A column's own is ahead of everything's, and everything's is the rest's.
    options = SuggestOptions(format=INDIAN, column_formats={1: NumFormat.INVARIANT})
    found = suggest(b'a,b\n"1,23,456","1,234,567"\n', options)
    assert doors(found) == [Door.I32, Door.I32]
    assert found.report[0].format is INDIAN and same(found.report[1].format, NumFormat.INVARIANT)


def test_a_format_the_core_refuses_is_a_plan_error() -> None:
    """HyperCast's constructor won't make either of these, so each is stood in for: an old program's
    format, its separators in the bytes that hold them."""

    class Packed:
        __class__ = NumFormat  # type: ignore[assignment]

        def __init__(self, group: bytes) -> None:
            packed = bytearray(NumFormat.INVARIANT.packed)
            packed[20:29] = group
            self.packed = bytes(packed)

    good = NumFormat.INVARIANT.packed[20:29]
    assert Packed(good).packed == NumFormat.INVARIANT.packed
    suggest(b"a,b\n1,2\n", SuggestOptions(column_formats={1: Packed(good)}))  # type: ignore[dict-item]
    same = b"." + bytes(7) + b"\x01"
    with pytest.raises(ValueError, match="cannot be honoured"):
        suggest(b"a,b\n1,2\n", SuggestOptions(column_formats={1: Packed(same)}))  # type: ignore[dict-item]
    none = bytes(9)
    with pytest.raises(ValueError, match="cannot be honoured"):
        suggest(b"a\n0.5\n", SuggestOptions(format=Packed(none)))  # type: ignore[arg-type]


def test_options_are_checked_where_they_are_made() -> None:
    """Options are checked where they are made."""
    with pytest.raises(ValueError):
        SuggestOptions(sample_rows=-1)
    with pytest.raises(TypeError):
        SuggestOptions(sample_rows=True)
    with pytest.raises(TypeError):
        SuggestOptions(sample_rows="1")  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        DelimitedReader.suggest_bytes(b"a\n", Dialect.CSV, object())  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        DelimitedReader.suggest_bytes("a\n", Dialect.CSV)  # type: ignore[arg-type]


def test_suggest_applies_nothing(tmp_path: Path) -> None:
    """Suggest applies nothing."""
    data = b"id,score\n1,2.5\n2,3.5\n"
    path = tmp_path / "a.csv"
    path.write_bytes(data)
    assert doors(DelimitedReader.suggest(path, Dialect.CSV)) == [Door.I32, Door.F64]
    # A reader opened from the same file with a plan of its own has that plan.
    declared = [Column.text(0), Column.text(1)]
    with DelimitedReader.open(path, Dialect.CSV, declared) as reader:
        assert reader.plan == tuple(declared)
        (batch,) = list(reader)
        assert (batch.text(0, 0), batch.text(1, 0)) == ("1", "2.5")
    # One opened without a plan still has none, and says so when read.
    unbound = DelimitedReader(data, Dialect.CSV)
    suggest(data)
    assert not unbound.is_bound and unbound.plan == ()
    with pytest.raises(RuntimeError):
        unbound.read()
    # And the same of a sheet.
    sheet = Workbook.open(WORKBOOKS / "basic.xlsx").sheet(0, SheetOptions())
    sheet.suggest()
    assert not sheet.is_bound and sheet.plan == ()
    with pytest.raises(RuntimeError):
        sheet.read()


def test_a_bound_or_part_read_sheet_answers_the_same_and_stays_where_it_is() -> None:
    """A sheet answers from its first row whatever it has been up to, and carries on where it was."""
    book = Workbook.open(WORKBOOKS / "libreoffice-data.xlsx")
    options = SheetOptions(batch_rows=1)
    asked = SuggestOptions(sample_rows=1)
    fresh = book.sheet(0, options).suggest(asked)
    plan = [Column.text(column) for column in range(len(fresh.plan))]
    rows = [batch.text(0, 0) for batch in book.sheet(0, options, plan)]
    assert len(rows) > 1

    sheet = book.sheet(0, options, plan)
    assert sheet.suggest(asked) == fresh
    first = sheet.read()
    assert first is not None and first.text(0, 0) == rows[0]
    assert sheet.suggest(asked) == fresh
    assert sheet.plan == tuple(plan)
    second = sheet.read()
    assert second is not None and second.text(0, 0) == rows[1]


def test_a_named_sheet_is_the_sheet_sampled() -> None:
    """A named sheet is the sheet sampled."""
    book = Workbook.open(WORKBOOKS / "multisheet.xlsx")
    assert len(book.sheets) > 1
    answered = 0
    for at, info in enumerate(book.sheets):
        try:
            by_index = book.sheet(at, SheetOptions())
        except TabularError:
            with pytest.raises(TabularError):
                book.sheet(info.name, SheetOptions())
            continue
        assert by_index.suggest() == book.sheet(info.name, SheetOptions()).suggest()
        answered += 1
    assert answered > 1


@pytest.mark.parametrize(
    ("rows", "truncated"), [(2, False), (3, False), (4, True), (5, True), (9000, True)]
)
def test_a_sample_of_exactly_the_source_is_not_truncated(
    rows: int, truncated: bool, tmp_path: Path
) -> None:
    """A sample of exactly the source is not truncated."""
    exact = SuggestOptions(sample_rows=3)
    data = rows_of(rows)
    found = suggest(data, exact)
    assert found.truncated is truncated
    assert found.report[0].sampled == min(rows, 3)
    path = tmp_path / "exact.csv"
    path.write_bytes(data)
    assert DelimitedReader.suggest(path, Dialect.CSV, exact).truncated is truncated


def test_a_sample_is_a_thousand_rows_and_at_most_a_batch() -> None:
    """A sample is a thousand rows and at most a batch."""
    data = rows_of(9000)
    default = suggest(data)
    assert default.report[0].sampled == 1000 and default.truncated
    most = SuggestOptions(sample_rows=1_000_000)
    found = suggest(data, most)
    assert found.report[0].sampled == 4096 and found.truncated
    found = suggest(rows_of(4096), most)
    assert found.report[0].sampled == 4096 and not found.truncated


def test_a_sample_of_none_is_one() -> None:
    """Zero means one, as a batch size does. Under one row, ``1`` is a Bool; and a declared format
    alone leaves the thousand rows."""
    data = b"a\n1\n2.5\n"
    found = suggest(data, SuggestOptions(sample_rows=0))
    assert doors(found) == [Door.BOOL]
    assert found.report[0].sampled == 1 and found.truncated
    found = suggest(data)
    assert doors(found) == [Door.F64]
    assert found.report[0].sampled == 2 and not found.truncated
    found = suggest(data, SuggestOptions(format=NumFormat.INVARIANT))
    assert found.report[0].sampled == 2 and not found.truncated
    found = suggest(data, SuggestOptions(date_order=DateOrder.DAY_MONTH_YEAR))
    assert found.report[0].sampled == 2 and not found.truncated
    sheet = Workbook.open(WORKBOOKS / "basic.xlsx").sheet(0, SheetOptions())
    assert sheet.suggest(SuggestOptions(sample_rows=0)).report[0].sampled == 1
    assert sheet.suggest(SuggestOptions(format=NumFormat.INVARIANT)).report[0].sampled > 1


def test_what_the_sample_did_not_reach_has_no_say() -> None:
    """What the sample did not reach has no say."""
    data = rows_of(3) + b"oops\n"
    found = suggest(data, SuggestOptions(sample_rows=2))
    assert doors(found) == [Door.I32]
    assert found.report[0].status is Resolved and found.truncated
    assert suggest(data).report[0].status is Unresolved


def test_a_sheet_s_sample_is_at_most_its_batch() -> None:
    """A sheet's sample is at most its batch."""
    book = Workbook.open(WORKBOOKS / "libreoffice-data.xlsx")
    found = book.sheet(0, SheetOptions(batch_rows=1)).suggest()
    assert found.truncated
    assert all(column.sampled <= 1 for column in found.report)


def test_a_sheet_of_eight_rows_at_batches_of_five_is_said_to_be_cut() -> None:
    """A sheet of eight rows at batches of five is said to be cut."""
    rows = "".join(f'<row r="{row}"><c><v>{row}</v></c></row>' for row in range(1, 9))
    book = Workbook(_xlsx(rows))
    plain = SheetOptions(False, batch_rows=5)
    found = book.sheet(0, plain).suggest()
    assert found.truncated and found.report[0].sampled == 5
    whole = book.sheet(0, SheetOptions(False)).suggest()
    assert not whole.truncated and whole.report[0].sampled == 8


def test_a_failure_among_the_sampled_rows_is_said_and_one_past_them_is_not() -> None:
    """A failure among the sampled rows is said and one past them is not."""
    data = b"a,b\n1,2\n3,4\n5\n7,8\n"
    with pytest.raises(TabularError) as failed:
        suggest(data)
    assert failed.value.kind is TabularFailure.COLUMN_COUNT
    # The row right after the sample says there's more, and no more than that.
    found = suggest(data, SuggestOptions(sample_rows=2))
    assert found.truncated and found.report[0].sampled == 2
    # Declared, a ragged row is a read like any other, and what a row hasn't got is blank.
    found = suggest(data, dialect=Dialect(",", short_rows=ShortRows.PAD))
    assert found.report[1].sampled == 3
    found = suggest(b"a,b\n1,2,3\n", dialect=Dialect(",", long_rows=LongRows.TRUNCATE))
    assert len(found.plan) == 2


def test_a_column_of_tokens_is_a_column_of_blanks() -> None:
    """A column of tokens is a column of blanks."""
    dialect = Dialect(",", tokens=("NA", "-"))
    found = suggest(b'a,b,c\nNA,1,"NA"\n-,NA,"NA"\nNA,3,"NA"\n', dialect=dialect)
    # Nothing but tokens: nothing to go on, as a blank column has.
    assert found.report[0].status is Unresolved
    assert (found.report[0].matched, found.report[0].sampled) == (0, 0)
    assert found.plan[0].door is Door.TEXT
    # A token among numbers doesn't stop them being numbers.
    assert (found.report[1].door, found.report[1].status) == (Door.I32, Resolved)
    assert (found.report[1].matched, found.report[1].sampled) == (2, 2)
    # A quoted cell is never a token, so it's the text NA.
    assert (found.report[2].door, found.report[2].status) == (Door.TEXT, Resolved)
    assert found.report[2].sampled == 3


def test_invalid_text_is_a_failed_cell_and_not_a_blank() -> None:
    """Invalid text is a failed cell and not a blank."""
    found = suggest(b"a,b\n\xe9t\xe9,1\nabc,\xff\n")
    assert found.report[0].status is Unresolved
    assert (found.report[0].matched, found.report[0].sampled) == (0, 2)
    assert (found.report[1].matched, found.report[1].sampled) == (1, 2)


def test_a_quote_s_doubled_text_is_what_is_judged() -> None:
    """A quote's doubled text is what is judged."""
    assert doors(suggest(b'a\n"1"""\n')) == [Door.TEXT]
    assert doors(suggest(b'a\n"1"\n"2"\n')) == [Door.I32]


def test_no_header_has_no_names_and_the_first_record_s_width() -> None:
    """No header has no names and the first record's width."""
    found = suggest(b"1,x\n2,y\n", dialect=Dialect(",", has_header=False))
    assert doors(found) == [Door.I32, Door.TEXT]
    assert found.report[0].name is None and found.report[0].sampled == 2


def test_nothing_in_the_source_is_nothing_to_plan() -> None:
    """Nothing in the source is nothing to plan."""
    for dialect in (Dialect.CSV, Dialect(",", has_header=False)):
        found = suggest(b"", dialect=dialect)
        assert found.plan == () and found.report == () and not found.truncated
    # A header and no rows: columns with nothing to go on.
    found = suggest(b"a,b\n")
    assert doors(found) == [Door.TEXT, Door.TEXT]
    assert found.report[0].status is Unresolved and found.report[0].sampled == 0


def test_a_dialect_s_declarations_are_the_sample_s(tmp_path: Path) -> None:
    """A dialect's declarations are the sample s."""
    dialect = Dialect("\t", skip=1, header_row=1)
    found = suggest(
        b"a title, with a comma\nignored\tcolumns\nid\tscore\n7\t2.5\n", dialect=dialect
    )
    assert doors(found) == [Door.I32, Door.F64]
    assert found.report[1].name == "score"
    # Latin-1, decoded before anything's judged, and bytes and a file say the same.
    latin = Dialect(",", encoding=Encoding.LATIN_1)
    data = b"caf\xe9\n7\n8\n"
    found = suggest(data, dialect=latin)
    assert found.report[0].name == "café" and found.report[0].door is Door.I32
    path = tmp_path / "latin1.csv"
    path.write_bytes(data)
    assert DelimitedReader.suggest(path, latin) == found
    # An encoding failure among the sample is the sample's.
    with pytest.raises(TabularError) as failed:
        suggest(
            b"a\x00\n\x00\x001\x00\n\x00\x00\xd8", dialect=Dialect(",", encoding=Encoding.UTF_16LE)
        )
    assert failed.value.kind is TabularFailure.ENCODING


def test_a_path_that_is_not_there_is_an_io_error(tmp_path: Path) -> None:
    """A path that is not there is an io error."""
    with pytest.raises(OSError):
        DelimitedReader.suggest(tmp_path / "missing.csv", Dialect.CSV)


def test_a_sheet_with_no_header_has_no_names_and_its_first_row_s_width() -> None:
    """A sheet with no header has no names and its first row's width."""
    sheet = Workbook.open(WORKBOOKS / "basic.xlsx").sheet(0, SheetOptions(False))
    found = sheet.suggest()
    assert len(found.plan) == 9 and found.report[0].name is None
    # The header row is a row here: `id` and its four numbers, one door taking four of five.
    assert (found.report[0].door, found.report[0].matched) == (Door.TEXT, 4)
    assert found.report[0].sampled == 5


def _column(file: str, name: str) -> tuple[Door, SuggestStatus]:
    sheet = Workbook.open(WORKBOOKS / file).sheet(0, SheetOptions())
    (column,) = [column for column in sheet.suggest().report if column.name == name]
    return column.door, column.status


def test_a_typed_cell_is_judged_by_what_the_text_door_says_of_it() -> None:
    """A typed cell is judged by what the text door says of it."""
    for file in (
        "basic.xlsx",
        "libreoffice-data.xlsx",
        "libreoffice-data.ods",
        "numbers-data.numbers",
    ):
        assert _column(file, "id") == (Door.I32, Resolved), file
        assert _column(file, "flag") == (Door.BOOL, Resolved), file
    # A date, a date-time and a time of day, each as its writer's canonical text.
    for file in ("basic.xlsx", "libreoffice-data.ods", "numbers-data.numbers"):
        for name in ("day", "when", "at"):
            assert _column(file, name) == (Door.TEXT, Resolved), f"{file}: {name}"
    assert _column("libreoffice-data.ods", "elapsed") == (Door.TEXT, Resolved)
    assert _column("basic.xlsx", "amount") == (Door.F64, Resolved)
    assert _column("numbers-data.numbers", "pct") == (Door.F64, Resolved)
    assert _column("libreoffice-data.ods", "pct") == (Door.F64, Resolved)
    assert _column("basic.ods", "money") == (Door.I32, Resolved)
    # A column that mixes kinds says so, with a formula's cell an empty one.
    assert _column("numbers-data.numbers", "calc") == (Door.TEXT, Unresolved)
    assert _column("basic.ods", "stamp") == (Door.TEXT, Unresolved)


def _read_fails(book: Workbook, at: int) -> bool:
    try:
        for _ in book.sheet(at, SheetOptions(), [Column.text(0)]):
            pass
    except TabularError:
        return True
    return False


def test_no_sheet_s_plan_refuses_a_cell_it_sampled() -> None:
    """What was resolved reads back through its door over every sampled row of every sheet of every
    package, under each thing a caller can declare and under nothing."""
    planned = typed = refused_by_suggest = 0
    for path in sorted(WORKBOOKS.iterdir()):
        if path.suffix == ".py" or not path.is_file():
            continue
        try:
            book = Workbook.open(path)
        except (TabularError, ValueError):
            continue
        for at in range(len(book.sheets)):
            for declared in (None, NumFormat.INVARIANT, COMMA, INDIAN):
                try:
                    sheet = book.sheet(at, SheetOptions())
                except TabularError:
                    continue
                try:
                    found = sheet.suggest(SuggestOptions(format=declared))
                except TabularError as error:
                    assert _read_fails(book, at), f"{path.name} {at}: {error}"
                    refused_by_suggest += 1
                    continue
                if not found.plan:
                    continue
                planned += 1
                typed += sum(column.door is not Door.TEXT for column in found.plan)
                rows = 0
                for batch in book.sheet(at, SheetOptions(), found.plan):
                    refused = [
                        problem
                        for problem in batch.faults()
                        if problem.row + rows < 1000
                        and problem.reason in (CastFailure.MALFORMED, CastFailure.OUT_OF_RANGE)
                        and found.plan[problem.plan_column or 0].door is not Door.TEXT
                    ]
                    assert refused == [], f"{path.name} {at}, {declared}"
                    rows += batch.rows
                    if rows >= 1000:
                        break
    assert planned >= 80 and typed >= 350 and refused_by_suggest >= 1, (
        planned,
        typed,
        refused_by_suggest,
    )


@pytest.mark.parametrize(
    ("file", "names"),
    [
        ("basic.xlsx", ["amount", "ratio"]),
        ("basic.ods", ["amount", "pct"]),
        ("excel-web-data.xlsx", ["pct", "money"]),
    ],
)
def test_a_workbook_s_number_column_is_numeric_under_any_format(
    file: str, names: list[str]
) -> None:
    """A workbook's number column is numeric under any format."""
    book = Workbook.open(WORKBOOKS / file)
    for asked in (None, NumFormat.INVARIANT, COMMA):
        found = book.sheet(0, SheetOptions()).suggest(SuggestOptions(format=asked))
        for name in names:
            (column,) = [column for column in found.report if column.name == name]
            assert column.status is Resolved and column.door in NUMERIC, f"{file} {name}"


def _xlsx(rows: str) -> bytes:
    """The smallest package a reader takes, with ``rows`` as its one sheet's."""
    main = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
    made = io.BytesIO()
    with zipfile.ZipFile(made, "w", zipfile.ZIP_DEFLATED) as package:
        package.writestr("[Content_Types].xml", "<Types/>")
        package.writestr(
            "_rels/.rels",
            '<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/'
            'officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            "</Relationships>",
        )
        package.writestr(
            "xl/workbook.xml",
            f'<workbook {main} xmlns:r="r"><sheets><sheet name="Data" sheetId="1" r:id="rId0"/>'
            "</sheets></workbook>",
        )
        package.writestr(
            "xl/_rels/workbook.xml.rels",
            '<Relationships><Relationship Id="rId0" Type="http://x/worksheet" '
            'Target="worksheets/sheet0.xml"/></Relationships>',
        )
        package.writestr(
            "xl/worksheets/sheet0.xml",
            f"<worksheet {main}><sheetData>{rows}</sheetData></worksheet>",
        )
    return made.getvalue()


@pytest.mark.parametrize(
    "value",
    [
        "2147483647",
        "2147483648",
        "-2147483648",
        "-2147483649",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "18446744073709551616",
        "1e21",
        "1.5E300",
        "1.7976931348623157E+308",
        "5e-324",
        "-0",
        "0.1",
        "1234.5",
        "NaN",
        "Infinity",
        "1e999",
    ],
)
def test_a_typed_number_at_a_door_s_limit_reads_through_the_door_it_suggests(value: str) -> None:
    """A typed number at a door's limit reads through the door it suggests."""
    book = Workbook(_xlsx(f'<row r="1"><c><v>{value}</v></c></row>'))
    options = SheetOptions(False)
    for asked in (None, NumFormat.INVARIANT, COMMA):
        found = book.sheet(0, options).suggest(SuggestOptions(format=asked))
        (batch,) = list(book.sheet(0, options, found.plan))
        refused = [
            problem
            for problem in batch.faults()
            if problem.reason in (CastFailure.MALFORMED, CastFailure.OUT_OF_RANGE)
        ]
        assert refused == [], f"{value}: {found.report[0]}"
        assert found.report[0].status is not Ambiguous, value


def test_text_holding_a_number_keeps_the_owner_s_rule_in_a_sheet() -> None:
    """A sheet's sample keeps its notes, whatever a read of the sheet was to have."""
    book = Workbook.open(WORKBOOKS / "suggest.xlsx")
    found = book.sheet(0, SheetOptions()).suggest()
    assert [column.status for column in found.report[:3]] == [Resolved, Ambiguous, Ambiguous]
    assert book.sheet(0, SheetOptions(notes=False)).suggest() == found


def test_a_declared_date_order_settles_a_date_the_sample_left_open() -> None:
    """A declared date order settles a date the sample left open."""
    day_first, month_first = DateOrder.DAY_MONTH_YEAR, DateOrder.MONTH_DAY_YEAR
    opened = b"day\n01/02/2026\n03/04/2026\n"
    found = suggest(opened)
    assert found.report[0].status is Ambiguous and found.plan[0].door is Door.TEXT
    found = suggest(opened, SuggestOptions(date_order=day_first))
    assert found.report[0].status is Resolved and found.report[0].candidates == ()
    assert found.plan[0].door is Door.DATE_ORDERED and found.plan[0].declared is day_first
    assert found.report[0].declared is day_first
    # A column's own wins, the last one given.
    found = suggest(
        opened,
        SuggestOptions(date_order=day_first, column_date_orders=[(0, day_first), (0, month_first)]),
    )
    assert found.plan[0].declared is month_first
    # An order the column's cells rule out doesn't overrule them.
    found = suggest(b"day\n13/02/2026\n", SuggestOptions(date_order=month_first))
    assert found.plan[0].declared is day_first
    # An order out of the question's set, as year-first is, leaves the question open.
    found = suggest(opened, SuggestOptions(date_order=DateOrder.YEAR_MONTH_DAY))
    assert found.report[0].status is Ambiguous and found.plan[0].door is Door.TEXT


def test_a_comma_that_proved_to_group_stays_western_in_the_plan() -> None:
    """A comma that proved to group stays western in the plan."""
    data = b'a\n"1,234"\n2.5\n"1,23,456"\n'
    found = suggest(data, SuggestOptions(sample_rows=2))
    assert (found.report[0].door, found.report[0].status) == (Door.F64, Resolved)
    assert same(found.report[0].format, NumFormat.INVARIANT)
    assert same(found.plan[0].format, NumFormat.INVARIANT)
    batch = one(data, found.plan)
    assert batch.column(0).values.tolist()[:2] == [1234.0, 2.5]
    assert batch.column(0)[2].reason is CastFailure.MALFORMED
    # A month-day-year date's parameter is the same 2, and says nothing of a notation.
    found = suggest(b"a\n02/13/2026\n")
    assert found.report[0].declared is DateOrder.MONTH_DAY_YEAR
    assert found.report[0].format is None and same(found.plan[0].format, NumFormat.INVARIANT)
    # Declared, it's the declared format, and the report names that.
    found = suggest(b'a\n"1,23,456"\n', SuggestOptions(format=INDIAN))
    assert same(found.report[0].format, INDIAN) and same(found.plan[0].format, INDIAN)
    assert one(b'a\n"1,23,456"\n', found.plan).column(0)[0] == Success(123456)
    # A column that isn't a resolved number names no format.
    assert suggest(b"a\nx\n").report[0].format is None
    found = suggest(b'a\n"1,234"\n')
    assert found.report[0].status is Ambiguous and found.report[0].format is None


def test_a_number_with_no_format_declared_is_planned_western() -> None:
    """A number with no format declared is planned western."""
    data = b'a\n7\n8\n"1,23,456"\n'
    found = suggest(data, SuggestOptions(sample_rows=2))
    assert (found.report[0].door, found.report[0].status) == (Door.I32, Resolved)
    assert same(found.report[0].format, NumFormat.INVARIANT)
    assert same(found.plan[0].format, NumFormat.INVARIANT)
    batch = one(data, found.plan)
    assert batch.column(0).values.tolist()[:2] == [7, 8]
    assert batch.column(0)[2].reason is CastFailure.MALFORMED


def test_a_headerless_first_record_is_not_judged_as_a_header(tmp_path: Path) -> None:
    """A headerless first record is not judged as a header."""
    headerless = Dialect(",", has_header=False)
    data = b"\xe9t\xe9,1\nx,2\n"
    found = suggest(data, dialect=headerless)
    assert len(found.plan) == 2 and found.report[0].status is Unresolved
    assert (found.report[0].matched, found.report[0].sampled) == (0, 2)
    assert found.report[1].door is Door.I32
    path = tmp_path / "bad.csv"
    path.write_bytes(data)
    assert DelimitedReader.suggest(path, headerless) == found
    # With a header it is structural, and a read says the same.
    with pytest.raises(TabularError):
        suggest(data)
    with pytest.raises(TabularError):
        DelimitedReader(data, Dialect.CSV)


def test_a_suggestion_and_a_read_agree_on_a_lone_zero() -> None:
    """A suggestion and a read agree on a lone zero."""
    for data, fmt in ((b'a\n"0,125"\n', NumFormat.INVARIANT), (b"a\n0.125\n", COMMA)):
        found = suggest(data, SuggestOptions(format=fmt))
        assert (found.report[0].door, found.report[0].status) == (Door.TEXT, Resolved)
        assert one(data, [Column.f64(0, fmt)]).column(0)[0].reason is CastFailure.MALFORMED
    found = suggest(b'a\n"0,125"\n')
    assert same(found.report[0].format, COMMA)
    assert one(b'a\n"0,125"\n', found.plan).column(0).values.tolist() == [0.125]
    # Beside `1,234`, it's what rules out the comma decimal.
    found = suggest(b'a\n"1,234"\n0.125\n')
    assert (found.report[0].door, found.report[0].status) == (Door.F64, Resolved)
    assert same(found.report[0].format, NumFormat.INVARIANT)
    found = suggest(b"a\n0.125\n")
    assert same(found.report[0].format, NumFormat.INVARIANT)
    assert one(b"a\n0.125\n", found.plan).column(0).values.tolist() == [0.125]


def test_a_column_only_the_comma_decimal_reads_is_planned_in_it() -> None:
    """A column only the comma decimal reads is planned in it."""
    data = b'f,i\n"2,5",1.234.567\n"1.234,5",1.000\n"1,234","1,234,567"\n'
    found = suggest(data, SuggestOptions(sample_rows=2))
    assert doors(found) == [Door.F64, Door.I32]
    for column in found.report:
        assert column.status is Resolved and same(column.format, COMMA) and column.candidates == ()
    assert same(found.plan[0].format, COMMA) and same(found.plan[1].format, COMMA)
    batch = one(data, found.plan)
    assert batch.column(0).values.tolist() == [2.5, 1234.5, 1.234]
    assert batch.column(1).values.tolist()[:2] == [1234567, 1000]
    assert batch.column(1)[2].reason is CastFailure.MALFORMED


def test_a_question_reads_as_text_and_each_answer_reads_its_own_value() -> None:
    """A question reads as text and each answer reads its own value."""
    data = b"a\n1.234\n"
    found = suggest(data)
    assert found.report[0].status is Ambiguous and found.report[0].format is None
    assert found.plan[0].door is Door.TEXT
    answers = found.report[0].candidates
    assert len(answers) == 2
    read = [one(data, [Column.f64(0, fmt)]).column(0).values.tolist()[0] for fmt in answers]  # type: ignore[arg-type]
    assert read == [1.234, 1234.0]


def test_a_suggested_plan_is_a_plan_a_reader_takes_and_a_stream_reads() -> None:
    """A suggested plan is a plan a reader takes and a stream reads."""
    data = b"id,score\n1,2.5\n2,3.5\n"
    found = suggest(data)

    async def main() -> None:
        reader = await DelimitedReader.open_async(_AsyncDribble(data), Dialect.CSV, found.plan)
        assert reader.plan == found.plan
        said = [value async for batch in reader for value in batch.column(1).values.tolist()]
        assert said == [2.5, 3.5]

    _run(main())


def test_a_suggestion_is_not_refused_for_the_size_of_its_own_batch():
    """Nobody set the size of a sampler's batch, so it's never why a suggestion is refused."""
    width = 268_000
    names = ",".join(f"c{column}" for column in range(width))
    row = ",".join(["1"] * width)
    wide = DelimitedReader.suggest_bytes(f"{names}\n{row}\n{row}\n".encode(), Dialect.CSV)
    assert len(wide.report) == width
    assert wide.report[width - 1].sampled == 2
    assert not wide.truncated
    # At exactly the limit with no plan, and a sample past every batch.
    book = Workbook.open(WORKBOOKS / "basic.xlsx")
    at_the_limit = book.sheet(0, SheetOptions(batch_rows=1 << 29))
    assert at_the_limit.suggest(SuggestOptions(sample_rows=1 << 40)) == book.sheet(
        0, SheetOptions()
    ).suggest(SuggestOptions(sample_rows=4096))


def test_a_ragged_headerless_sample_across_each_batch_boundary():
    """No header, a short and a long row, and row widths kept: Rust's sampler, through this door."""
    headerless = Dialect(",", has_header=False, short_rows=ShortRows.PAD, long_rows=LongRows.REPORT)
    for limit in range(1, 7):
        found = DelimitedReader.suggest_bytes(
            b"1,x\n2,y,long\n3\n4,z\n5,w\n", headerless, SuggestOptions(sample_rows=limit)
        )
        assert len(found.report) == 2
        assert found.report[0].sampled == min(limit, 5)
        assert found.truncated == (limit < 5)
