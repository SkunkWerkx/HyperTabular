"""A column's grouping: Western unless the caller declares Indian on that column, in the same read
as a column that doesn't, and a stale policy word refused where it's handed over, before any cell.
What the shapes read is HyperCast's and the corpus's.

HyperCast's own constructor refuses a stale word, so what an old program's stored format would hand
over is stood in for here by an object that passes for a ``NumFormat`` and packs that word. The
door these tests prove is HyperTabular's: ``num_format_of`` in ``rust/src/python_ext.rs``, through
``ColumnSpec::num_format``, which every bind, open and suggest takes.
"""

from __future__ import annotations

import asyncio
import dataclasses
import io
import re
from pathlib import Path
from typing import Any

import pytest
from test_corpus import _AsyncDribble, _corpus_dir, _run

from hypertabular import (
    CastFailure,
    Column,
    DelimitedReader,
    Dialect,
    Door,
    Fault,
    NumFormat,
    SheetOptions,
    Success,
    SuggestOptions,
    SuggestStatus,
    TabularError,
    Workbook,
)

INDIAN = NumFormat.INVARIANT.with_grouping(NumFormat.GROUPING_INDIAN)
BOTH = b'indian,western\n"1,23,456","1,23,456"\n"1,234,567","1,234,567"\n'
WORKBOOKS = _corpus_dir() / "workbook"
GROUPED = [WORKBOOKS / f"grouping.{kind}" for kind in ("xlsx", "ods", "numbers")]

# Where a format's packed bytes hold its flags and policy word: where the two formats differ.
_WORD = (
    min(at for at, (a, b) in enumerate(zip(NumFormat.INVARIANT.packed, INDIAN.packed)) if a != b)
    & ~3
)


class _Stale:
    """A format stored by an older HyperCast, or an old program's literal: the word as it arrives."""

    __class__ = NumFormat  # type: ignore[assignment]

    def __init__(self, word: int) -> None:
        packed = bytearray(NumFormat.INVARIANT.packed)
        packed[_WORD : _WORD + 4] = word.to_bytes(4, "little")
        self.packed = bytes(packed)


def _policy(fmt: NumFormat) -> int:
    """What a format's packed word holds: its flags and its grouping policy."""
    return int.from_bytes(fmt.packed[_WORD : _WORD + 4], "little")


def _word(word: int) -> Any:
    """The word: a real format where HyperCast makes one, a stand-in for what it refuses."""
    return {95: NumFormat.INVARIANT, 607: INDIAN}.get(word) or _Stale(word)


WORDS = [(351, True), (863, True), (95, False), (607, False)]


def _planned(format: Any) -> list[Column]:
    return [Column.i32(0), dataclasses.replace(Column.text(1), format=format)]


def _rejects(refused: bool, call: Any, what: str) -> None:
    """``call`` is refused, naming what it was handed, or it's taken."""
    if refused:
        with pytest.raises(ValueError, match=rf"{re.escape(what)}.*cannot be honoured"):
            call()
    else:
        call()


def test_the_stand_in_packs_what_the_word_says() -> None:
    """The controls that make the rest mean something: 95 and 607 as stand-ins pack exactly what the
    real formats do, and the word is where the policy is."""
    assert _policy(NumFormat.INVARIANT) == 95 and _policy(INDIAN) == 607
    assert _Stale(95).packed == NumFormat.INVARIANT.packed
    assert _Stale(607).packed == INDIAN.packed
    assert isinstance(_Stale(351), NumFormat)
    reader = DelimitedReader(
        BOTH, Dialect.CSV, [dataclasses.replace(Column.i32(0), format=_Stale(607))]
    )
    (batch,) = list(reader)
    assert batch.column(0)[0] == Success(123456)


@pytest.mark.parametrize("word", [351, 863])
def test_hypercast_itself_will_not_make_a_stale_word(word: int) -> None:
    """Why it takes a stand-in: the route that would make one is shut upstream."""
    with pytest.raises(ValueError):
        NumFormat(".", ",", word & 0xFF, grouping=word & ~0xFF)


def _assert_both(batch: Any, indian: int, western: int) -> None:
    assert batch.column(indian)[0] == Success(123456)
    assert isinstance(batch.column(western)[0], Fault)
    assert batch.column(western)[0].reason is CastFailure.MALFORMED
    assert isinstance(batch.column(indian)[1], Fault)
    assert batch.column(indian)[1].reason is CastFailure.MALFORMED
    assert batch.column(western)[1] == Success(1234567)


def test_an_indian_column_reads_beside_a_western_one() -> None:
    """An indian column reads beside a western one."""
    plan = [Column.i32(0, INDIAN), Column.i32(1)]
    (batch,) = list(DelimitedReader(BOTH, Dialect.CSV, plan))
    _assert_both(batch, 0, 1)
    assert _policy(Column.i32(1).format) == 95

    async def main() -> None:
        # All of it at once: what comes in dribbles is delivered as it comes, a row at a time.
        reader = await DelimitedReader.open_async(BOTH, Dialect.CSV, plan)
        batch = await reader.read_async()
        assert batch is not None
        _assert_both(batch, 0, 1)
        reader = await DelimitedReader.open_async(_AsyncDribble(BOTH), Dialect.CSV, plan)
        seen = [batch async for batch in reader]
        assert [batch.column(0)[0] for batch in seen][0] == Success(123456)
        assert seen[-1].column(1)[seen[-1].rows - 1] == Success(1234567)

    _run(main())


@pytest.mark.parametrize("path", GROUPED, ids=lambda path: path.name)
def test_an_indian_sheet_column_reads_beside_a_western_one(path: Path) -> None:
    """An indian sheet column reads beside a western one."""
    plan = [Column.i32(2, INDIAN), Column.i32(2)]
    (batch,) = list(Workbook.open(path).sheet(0, SheetOptions(), plan))
    _assert_both(batch, 0, 1)
    # A typed number has no notation: it's the same under either.
    assert batch.column(0)[2] == Success(1234) and batch.column(1)[2] == Success(1234)


@pytest.mark.parametrize(("word", "refused"), WORDS)
def test_a_stale_policy_is_refused_at_every_delimited_entry_point(
    word: int, refused: bool, tmp_path: Path
) -> None:
    """A stale policy is refused at every delimited entry point."""
    path = tmp_path / "grouping.csv"
    path.write_bytes(BOTH)
    plan = _planned(_word(word))
    _rejects(refused, lambda: DelimitedReader(BOTH, Dialect.CSV, plan), "plan[1]")
    _rejects(refused, lambda: DelimitedReader(io.BytesIO(BOTH), Dialect.CSV, plan), "plan[1]")
    _rejects(refused, lambda: DelimitedReader.open(path, Dialect.CSV, plan), "plan[1]")

    async def opened(source: Any) -> None:
        await DelimitedReader.open_async(source, Dialect.CSV, plan)

    _rejects(refused, lambda: _run(opened(BOTH)), "plan[1]")
    _rejects(refused, lambda: _run(opened(_AsyncDribble(BOTH))), "plan[1]")

    unbound = DelimitedReader(BOTH, Dialect.CSV)
    records = unbound.records
    _rejects(refused, lambda: unbound.bind(plan), "plan[1]")
    assert unbound.is_bound is not refused
    if refused:
        # Nothing bound, and no cell read.
        assert unbound.records == records
        with pytest.raises(RuntimeError):
            unbound.read()

        async def bound_async() -> None:
            reader = await DelimitedReader.open_async(_AsyncDribble(BOTH), Dialect.CSV)
            with pytest.raises(ValueError, match="cannot be honoured"):
                reader.bind(plan)
            assert not reader.is_bound

        _run(bound_async())


@pytest.mark.parametrize(("word", "refused"), WORDS)
def test_a_stale_policy_is_refused_at_every_sheet_entry_point(word: int, refused: bool) -> None:
    """A stale policy is refused at every sheet entry point."""
    book = Workbook.open(GROUPED[0])
    plan = _planned(_word(word))
    _rejects(refused, lambda: book.sheet(0, SheetOptions(), plan), "plan[1]")
    unbound = book.sheet(0, SheetOptions())
    _rejects(refused, lambda: unbound.bind(plan), "plan[1]")
    assert unbound.is_bound is not refused
    if refused:
        with pytest.raises(RuntimeError):
            unbound.read()


@pytest.mark.parametrize(("word", "refused"), WORDS)
def test_a_stale_policy_is_refused_when_asking_what_to_declare(word: int, refused: bool) -> None:
    """A stale policy is refused when asking what to declare."""
    sheet = Workbook.open(GROUPED[0]).sheet(0, SheetOptions())
    for options in (
        SuggestOptions(format=_word(word)),
        SuggestOptions(column_formats={0: _word(word)}),
    ):
        _rejects(
            refused,
            lambda: DelimitedReader.suggest_bytes(BOTH, Dialect.CSV, options),
            "SuggestOptions",
        )
        _rejects(refused, lambda: sheet.suggest(options), "SuggestOptions")


@pytest.mark.parametrize(("word", "refused"), WORDS)
def test_a_stale_format_is_refused_before_a_suggest_opens_anything(
    word: int, refused: bool, tmp_path: Path
) -> None:
    """A suggest judges every format it's given before it opens anything: one never met, one
    replaced, one past the sample, an empty source and a broken one. The sound words are the
    control, each getting as far as the source does."""
    fmt = _word(word)
    headerless = Dialect(",", has_header=False)
    every = SuggestOptions(format=fmt)
    past = SuggestOptions(column_formats={99: fmt})
    replaced = SuggestOptions(format=fmt, column_formats={0: INDIAN})
    twice = SuggestOptions(column_formats=[(0, fmt), (0, INDIAN)])
    for data, dialect, options in (
        (b"a\n1\n", Dialect.CSV, every),
        (b"", Dialect.CSV, every),
        (b"", headerless, every),
        (b"a\n1\n", Dialect.CSV, past),
        (b"a\n1\n", Dialect.CSV, replaced),
        (b"a\n1\n", Dialect.CSV, twice),
    ):
        _rejects(
            refused, lambda: DelimitedReader.suggest_bytes(data, dialect, options), "SuggestOptions"
        )

    # A broken source says so only of a format that's sound.
    def fails_as(error: type[Exception], call: Any) -> None:
        with pytest.raises(ValueError if refused else error) as raised:
            call()
        if refused:
            assert "cannot be honoured" in str(raised.value)

    for data, dialect in ((b'a\n"1\n', Dialect.CSV), (b'"1\n', headerless)):
        fails_as(TabularError, lambda: DelimitedReader.suggest_bytes(data, dialect, every))
    missing = tmp_path / "no-such-file.csv"
    fails_as(OSError, lambda: DelimitedReader.suggest(missing, Dialect.CSV, every))

    # A sheet: the same, and one whose sample would meet its broken part.
    sheet = Workbook.open(GROUPED[0]).sheet(0, SheetOptions())
    for options in (every, past, replaced, twice):
        _rejects(refused, lambda: sheet.suggest(options), "SuggestOptions")
    broken = Workbook.open(WORKBOOKS / "broken.xlsx").sheet(0, SheetOptions())
    fails_as(TabularError, lambda: broken.suggest(every))


def test_an_indian_shape_has_no_culture_until_one_is_declared() -> None:
    """What the delimiter rule says stays as it was: nothing says ``1,23,456`` is Indian."""

    def ask(data: bytes, options: SuggestOptions | None = None) -> Any:
        return DelimitedReader.suggest_bytes(data, Dialect.CSV, options)

    found = ask(b'a\n"1,23,456"\n')
    assert (found.report[0].door, found.report[0].status) == (Door.TEXT, SuggestStatus.RESOLVED)
    found = ask(b'a\n"1,23,456"\n5\n')
    assert (found.report[0].door, found.report[0].status) == (Door.TEXT, SuggestStatus.UNRESOLVED)
    # A suggested plan is Western: flags 95, and a later Indian shape is refused.
    found = ask(b'a\n"1,234,567"\n')
    assert found.report[0].format is NumFormat.INVARIANT
    assert _policy(found.plan[0].format) == 95
    (batch,) = list(DelimitedReader(b'a\n"1,23,456"\n', Dialect.CSV, found.plan))
    assert batch.column(0)[0].reason is CastFailure.MALFORMED

    # Declared, the column is a number, its plan reads it, and a Western shape is what's refused.
    declared = SuggestOptions(column_formats={0: INDIAN})
    both = b'a,b\n"1,23,456","1,23,456"\n'
    found = ask(both, declared)
    assert (found.report[0].door, found.report[0].status) == (Door.I32, SuggestStatus.RESOLVED)
    assert found.report[0].format is INDIAN
    assert _policy(found.plan[0].format) == 607
    (batch,) = list(DelimitedReader(both, Dialect.CSV, found.plan))
    assert batch.column(0)[0] == Success(123456)
    # The other column asked nothing declared, and stays text.
    assert found.report[1].door is Door.TEXT
    found = ask(b'a\n"1,234,567"\n', declared)
    assert (found.report[0].door, found.report[0].status) == (Door.TEXT, SuggestStatus.RESOLVED)
    assert found.report[0].format is None


def test_a_default_is_western_however_it_is_reached() -> None:
    """No format, the zero of a format and Invariant all read the same."""
    data = b'a\n"1,234,567"\n"1,23,456"\n'
    for column in (
        Column.i32(0),
        Column.i32(0, NumFormat.INVARIANT),
        dataclasses.replace(Column.i32(0), format=_word(95)),
    ):
        (batch,) = list(DelimitedReader(data, Dialect.CSV, [column]))
        assert batch.column(0)[0] == Success(1234567)
        assert batch.column(0)[1].reason is CastFailure.MALFORMED


def test_neither_policy_reads_what_neither_writes() -> None:
    """Neither policy reads what neither writes."""
    data = b'a\n"12,34"\n"1,2,345"\n'
    for fmt in (NumFormat.INVARIANT, INDIAN):
        (batch,) = list(DelimitedReader(data, Dialect.CSV, [Column.i32(0, fmt)]))
        for row in range(2):
            assert batch.column(0)[row].reason is CastFailure.MALFORMED


def test_a_plan_in_either_policy_runs_on_a_thread() -> None:
    """A suggest has no asynchronous form: it never reads a stream, and goes to a thread."""

    async def main() -> None:
        found = await asyncio.to_thread(
            DelimitedReader.suggest_bytes,
            b'a\n"1,23,456"\n',
            Dialect.CSV,
            SuggestOptions(format=INDIAN),
        )
        assert _policy(found.plan[0].format) == 607
        book = await Workbook.open_async(_AsyncDribble(GROUPED[0].read_bytes()))
        sheet = book.sheet(0, SheetOptions())
        answer = await asyncio.to_thread(sheet.suggest, SuggestOptions(format=INDIAN))
        assert answer == sheet.suggest(SuggestOptions(format=INDIAN))

    _run(main())
