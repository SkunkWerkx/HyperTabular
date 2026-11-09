"""Replays ``corpus/workbook.json`` — the contract every binding replays, and the one the Rust
binding replays — through this binding: each package opened from ``bytes``, from its path,
from a file object (whole, and a few bytes a read) and from an asyncio stream, each sheet
read by index and (where the name finds it) by name, in batches of one row, of two, and of
more than any sheet has — with the plan up front, and header first, bound once the header
is read. Each batch's faults list is held to the case's ``problems``, and a case's ``suggest`` to
what its sheet says. And the workbook's own surface around that.

A sheet's buffers are the Rust reader's, grown by it, so that the corpus's one-element replay is
Rust's and C's to run: nothing reaches them from here."""

from __future__ import annotations

import io
import json
from pathlib import Path
from typing import Any

import pytest
from test_corpus import (
    _AsyncDribble,
    _assert_cell,
    _assert_column,
    _assert_failure,
    _assert_keys_are_known,
    _assert_notation,
    _assert_rows,
    _column_of,
    _corpus_dir,
    _Dribble,
    _format_of,
    _problem_json,
    _problems_without_notes,
    _reported,
    _run,
    _suggestion_json,
    _unknown_keys,
)

from hypertabular import (
    CastFailure,
    Column,
    ExcelEpoch,
    Header,
    Range,
    Sheet,
    SheetInfo,
    SheetOptions,
    SuggestOptions,
    TabularError,
    TabularFailure,
    Workbook,
    WorkbookFormat,
)

CORPUS: list[dict[str, Any]] = json.loads(
    (_corpus_dir() / "workbook.json").read_text(encoding="utf-8")
)

_BATCH_ROWS = (1, 2, 1024)


def _path(case: dict[str, Any]) -> Path:
    return _corpus_dir() / case["file"]


def _openings(case: dict[str, Any]) -> list[tuple[str, Any]]:
    path = _path(case)
    return [
        ("bytes", lambda: Workbook(path.read_bytes())),
        ("path", lambda: Workbook.open(path)),
        ("file object", lambda: Workbook(io.BytesIO(path.read_bytes()))),
        ("short reads", lambda: Workbook(_Dribble(path.read_bytes()))),
        ("asyncio", lambda: _run(Workbook.open_async(_AsyncDribble(path.read_bytes())))),
    ]


def test_the_corpus_is_the_whole_contract():
    """The corpus is as large as it claims, and reaches every cell it says it does."""
    assert len(CORPUS) == 398
    assert len({case["file"] for case in CORPUS}) == 41
    cells = sum(len(case["rows"]) * len(case["plan"]) for case in CORPUS if "sheet" in case)
    assert cells >= 12_000


def test_every_corpus_key_is_read():
    """A key this replay doesn't know fails it, wherever in a case it is. ``note`` is the one no
    replay reads: a sentence for whoever reads the file."""
    _assert_keys_are_known(CORPUS, workbook=True)


def _options_of(
    settings: dict[str, Any], batch_rows: int = 4096, notes: bool = True
) -> SheetOptions:
    """A case's ``options``."""
    range_ = settings.get("range")
    return SheetOptions(
        settings["has_header"],
        settings["skip_empty_rows"],
        batch_rows,
        skip=settings.get("skip", 0),
        header_row=settings.get("header_row", 0),
        range=None if range_ is None else Range.parse(range_),
        tokens=settings.get("tokens", ()),
        notes=notes,
    )


def _replay(
    label: str,
    case: dict[str, Any],
    plan: list[Column],
    sheet: Sheet,
    batch_rows: int,
    notes: bool = True,
) -> None:
    assert _unknown_keys(case, workbook=True) == [], label
    rows = case["rows"]
    numbers = case["numbers"]
    assert sheet.header == (None if case["header"] is None else tuple(case["header"])), label
    assert sheet.plan == tuple(plan), label

    seen = 0
    failure = None
    problems: list[dict[str, Any]] = []
    try:
        for batch in sheet:
            assert 1 <= batch.rows <= batch_rows, label
            assert seen + batch.rows <= len(rows), f"{label}: more rows than the corpus lists"
            for row in range(batch.rows):
                assert batch.line(row) == numbers[seen + row], f"{label}, row {seen + row}"
                assert len(rows[seen + row]) == len(plan), f"{label}: a row of another width"
            problems += [_problem_json(problem, seen) for problem in batch.faults()]
            for index, (column, data) in enumerate(zip(plan, batch.columns)):
                expected = [row[index] for row in rows[seen : seen + batch.rows]]
                for row, cell in enumerate(expected):
                    _assert_cell(
                        f"{label}, row {seen + row}, column {index}",
                        column,
                        data,
                        row,
                        cell,
                        judged=False,
                        notes=notes,
                    )
                    assert batch.raw(index, row) == data.raw(row), label
                _assert_column(f"{label}, column {index}", column, data, expected)
            _assert_rows(label, plan, batch)
            seen += batch.rows
    except TabularError as error:
        failure = error
        # A failed sheet stays failed: the same failure, again.
        with pytest.raises(TabularError) as again:
            sheet.read()
        assert again.value is error, label
    else:
        assert sheet.read() is None, label
    assert seen == len(rows), f"{label}: {seen} rows, the corpus lists {len(rows)}"
    assert len(numbers) == seen, label
    wanted = case["problems"] if notes else _problems_without_notes(case["problems"])
    assert problems == wanted, f"{label}: problems"
    _assert_failure(label, failure, case)


@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_workbook_corpus(case: dict[str, Any]) -> None:
    """One case: opened every way, read every way, held to the corpus."""
    name = case["name"]
    # A package the core refuses: every way of opening it gives the one failure.
    if "sheet" not in case:
        for source, open_book in _openings(case):
            with pytest.raises(TabularError) as refused:
                open_book()
            _assert_failure(f"{name} ({source})", refused.value, case)
        return

    plan = [_column_of(entry) for entry in case["plan"]]
    index = case["sheet"]
    sheet_name = case["sheets"][index]["name"]
    settings = case["options"]
    for source, open_book in _openings(case):
        book = open_book()
        assert book.format is WorkbookFormat[case["format"].upper()], name
        assert book.date_system is ExcelEpoch(case["epoch"]), name
        assert [(sheet.name, sheet.hidden) for sheet in book.sheets] == [
            (sheet["name"], sheet["hidden"]) for sheet in case["sheets"]
        ], name
        by_name = [sheet.name for sheet in book.sheets].index(sheet_name) == index
        for batch_rows in _BATCH_ROWS:
            options = _options_of(settings, batch_rows)
            for which in (index, sheet_name) if by_name else (index,):
                label = f"{name}: {source}, {batch_rows} rows a batch, sheet {which!r}"
                _replay(label, case, plan, book.sheet(which, options, plan), batch_rows)
                # Header first: the sheet opened without a plan, its header read, and the
                # plan bound after — the header row's cells kept for a row that repeats it.
                label = f"{label}, header first"
                _replay(
                    label,
                    case,
                    plan,
                    _header_first(label, case, book, which, options, plan),
                    batch_rows,
                )


@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
def test_workbook_corpus_with_notes_off(case: dict[str, Any]) -> None:
    """Notes off, every cell reads the same with no detail or origin, and the faults list the
    same less a formula with no cached value."""
    if "sheet" not in case:
        return
    plan = [_column_of(entry) for entry in case["plan"]]
    book = Workbook.open(_path(case))
    for batch_rows in _BATCH_ROWS:
        options = _options_of(case["options"], batch_rows, notes=False)
        sheet = book.sheet(case["sheet"], options, plan)
        _replay(f"{case['name']}: notes off", case, plan, sheet, batch_rows, notes=False)


@pytest.mark.parametrize("case", CORPUS, ids=lambda case: case["name"])
@pytest.mark.parametrize("batch_rows", [1, 3])
def test_a_sheet_s_batch_held_past_the_next_read_still_answers_for_itself(
    case: dict[str, Any], batch_rows: int
) -> None:
    """A batch's faults, asked after every later read, are those it had when it was read: a sheet's
    source columns and row numbers and a formula's missing cache among them."""
    if "sheet" not in case:
        return
    plan = [_column_of(entry) for entry in case["plan"]]
    book = Workbook.open(_path(case))

    def batches() -> Any:
        options = _options_of(case["options"], batch_rows, notes=True)
        sheet = book.sheet(case["sheet"], options, plan)
        while True:
            try:
                batch = sheet.read()
            except TabularError:
                return
            if batch is None:
                return
            yield batch

    at_once = [batch.faults() for batch in batches()]
    held = list(batches())
    assert [batch.faults() for batch in held] == at_once
    wanted = [problem for problem in case["problems"]]
    assert len([problem for faults in at_once for problem in faults]) == len(wanted)


def _suggesting() -> list[dict[str, Any]]:
    return [case for case in CORPUS if "suggest" in case]


def test_the_corpus_says_what_to_suggest():
    """The corpus says what to suggest."""
    assert len(_suggesting()) == 15


@pytest.mark.parametrize("case", _suggesting(), ids=lambda case: case["name"])
def test_every_suggestion_is_what_the_corpus_says(case: dict[str, Any]) -> None:
    """Through the sheet, and a plan that then reads back refusing nothing, opened from a path and
    from an asyncio stream."""
    label = case["name"]
    expected = case["suggest"]
    declared = _format_of(expected) if "format" in expected else None
    asked = SuggestOptions(format=declared)
    options = _options_of(case["options"])
    for book in (
        Workbook.open(_path(case)),
        _run(Workbook.open_async(_AsyncDribble(_path(case).read_bytes()))),
    ):
        sheet = book.sheet(case["sheet"], options)
        found = sheet.suggest(asked)
        columns = expected["columns"]
        assert _suggestion_json(found) == _reported(columns), label
        assert found.truncated is expected["truncated"], label
        _assert_notation(label, found, columns, declared)
        for batch in book.sheet(case["sheet"], options, found.plan):
            refused = [
                problem
                for problem in batch.faults()
                if problem.reason in (CastFailure.MALFORMED, CastFailure.OUT_OF_RANGE)
            ]
            assert refused == [], label


def _header_first(
    label: str,
    case: dict[str, Any],
    book: Workbook,
    which: int | str,
    options: SheetOptions,
    plan: list[Column],
) -> Sheet:
    sheet = book.sheet(which, options)
    expected = case["header"]
    assert sheet.header == (None if expected is None else tuple(expected)), label
    assert expected is None or isinstance(sheet.header, Header), label
    assert not sheet.is_bound and sheet.plan == (), label
    with pytest.raises(RuntimeError, match="bind"):
        sheet.read()
    sheet.bind(plan)
    assert sheet.is_bound
    with pytest.raises(RuntimeError, match="bound once"):
        sheet.bind(plan)
    return sheet


def test_what_is_not_there_is_an_error_of_its_own(tmp_path: Path) -> None:
    """A sheet, a file or a container that is not there each says so in its own way."""
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    plan = [Column.text(0)]
    with pytest.raises(KeyError):
        book.sheet("No such sheet", SheetOptions(), plan)
    with pytest.raises(IndexError):
        book.sheet(99, SheetOptions(), plan)
    with pytest.raises(TypeError):
        book.sheet(1.5, SheetOptions(), plan)
    with pytest.raises(FileNotFoundError):
        Workbook.open(tmp_path / "missing.xlsx")
    with pytest.raises(TabularError) as refused:
        Workbook(b"not a zip at all")
    assert refused.value.kind is TabularFailure.NOT_A_ZIP
    assert str(refused.value) == "the workbook is not a zip file"

    # Two sheets read at once, each with its own buffers, over one workbook — which they
    # keep alive between them.
    first = book.sheet(0, SheetOptions(), plan)
    second = book.sheet(0, SheetOptions(has_header=False), plan)
    del book
    headed = first.read()
    assert headed is not None and second.header is None and first.header is not None
    batch = second.read()
    assert batch is not None and batch.rows == headed.rows + 1
    assert second.read() is None


def test_a_batch_outlives_its_sheet() -> None:
    """A sheet's batch owns what it shows, as a delimited one does."""
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    assert isinstance(book.sheets[0], SheetInfo)
    assert repr(book.sheets[0]) == f"SheetInfo(name={book.sheets[0].name!r}, hidden=False)"
    sheet = book.sheet(0, SheetOptions(), [Column.text(1), Column.i64(0)])
    batch = sheet.read()
    assert batch is not None
    names = list(batch.column(0).values)
    del sheet, book
    assert list(batch.column(0).values) == names
    # Row 5 of the sheet is empty, and skipped: a row number is where the row was.
    assert [batch.line(row) for row in range(batch.rows)] == [2, 3, 4, 6]
    assert batch.line(-1) == 6


def test_the_core_judges_a_sheets_size():
    """A sheet's batch is judged at open and at a late plan; zero is refused; omitted is the default."""
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    plan = [Column.i32(0)]
    just_over = "^batch_rows asks for 4831838224 bytes, past the limit of 4294967296$"
    with pytest.raises(ValueError, match=just_over):
        book.sheet(0, SheetOptions(batch_rows=1 << 27), plan)
    late = book.sheet(0, SheetOptions(batch_rows=1 << 27))
    with pytest.raises(ValueError, match=just_over):
        late.bind(plan)
    assert not late.is_bound
    for none in (plan, None):
        with pytest.raises(ValueError, match="^batch_rows must be at least 1$"):
            book.sheet(0, SheetOptions(batch_rows=0), none)
    for bad in (-1, 2**64):
        with pytest.raises(ValueError, match="batch_rows must fit an unsigned 64-bit count"):
            SheetOptions(batch_rows=bad)
    assert SheetOptions().batch_rows == 4096
