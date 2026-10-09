"""Replays ``corpus/workbook.json`` — the contract every binding replays, and the one the Rust
binding replays — through this binding: each package opened from ``bytes``, from its path,
from a file object (whole, and a few bytes a read) and from an asyncio stream, each sheet
read by index and (where the name finds it) by name, in batches of one row, of two, and of
more than any sheet has — with the plan up front, and header first, bound once the header
is read. And the workbook's own surface around that."""

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
    _assert_rows,
    _column_of,
    _corpus_dir,
    _Dribble,
    _run,
)

from hypertabular import (
    Column,
    ExcelEpoch,
    Header,
    Sheet,
    SheetInfo,
    SheetOptions,
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
    assert len(CORPUS) >= 80
    cells = sum(len(case["rows"]) * len(case["plan"]) for case in CORPUS if "sheet" in case)
    assert cells >= 12_000


def _replay(
    label: str, case: dict[str, Any], plan: list[Column], sheet: Sheet, batch_rows: int
) -> None:
    rows = case["rows"]
    numbers = case["numbers"]
    assert sheet.header == (None if case["header"] is None else tuple(case["header"])), label
    assert sheet.plan == tuple(plan), label

    seen = 0
    failure = None
    try:
        for batch in sheet:
            assert 1 <= batch.rows <= batch_rows, label
            assert seen + batch.rows <= len(rows), f"{label}: more rows than the corpus lists"
            for row in range(batch.rows):
                assert batch.line(row) == numbers[seen + row], f"{label}, row {seen + row}"
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
            options = SheetOptions(settings["has_header"], settings["skip_empty_rows"], batch_rows)
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
    with pytest.raises(ValueError):
        SheetOptions(batch_rows=0)
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
