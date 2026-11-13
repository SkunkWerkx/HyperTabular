"""A column that declares HyperCast's constraints: what the corpus does not reach of the binding's
surface. A Python column can pair any door with any declaration, so the pairing is checked, when the
column is made and again when the plan is bound.
"""

from __future__ import annotations

import dataclasses
from decimal import Decimal

import hypercast
import pytest

from hypertabular import (
    CastFailure,
    Checked,
    Column,
    Constraints,
    DelimitedReader,
    Dialect,
    Door,
    Fault,
    Keyword,
    Rounding,
    Violation,
)

HEADERLESS = Dialect(",", has_header=False)


def _read(csv: str, *plan: Column):
    reader = DelimitedReader(csv.encode("utf-8"), HEADERLESS, list(plan))
    batch = reader.read()
    assert batch is not None
    return batch


def _at_most(maximum: int) -> Constraints[int]:
    return Constraints(hypercast.Door.I32, maximum=maximum)


def test_a_checked_column_reads_a_violation():
    """A value past ``maximum`` is a violation carrying the value as cast; a malformed cell stays a fault."""
    batch = _read("11\n7\nx\n", Column.i32(0, constraints=_at_most(10)))
    assert batch.columns[0].column.constraints == _at_most(10)

    assert batch.get_checked(0, 0) == Violation(Keyword.MAXIMUM, 11, 0, 2)
    assert batch.columns[0].verdicts[0, 2] == 4
    assert batch.columns[0].values[0] == 11, "the value as cast, kept"
    assert batch.columns[0].fault_count == 1, "a violation cast, so it isn't a fault"

    assert batch.get_checked(0, 1) == Checked(7, False, None)
    fault = batch.get_checked(0, 2)
    assert isinstance(fault, Fault) and fault.reason is CastFailure.MALFORMED
    assert batch.row(1).get_checked(0) == batch.get_checked(0, 1)


def test_a_rounded_value_is_flagged_adjusted():
    """``multipleOf`` with a rounding delivers the rounded value, flagged adjusted."""
    declared = Constraints(hypercast.Door.DECIMAL, multiple_of="0.5", rounding=Rounding.FLOOR)
    batch = _read("1.3\n1.5\n", Column.decimal(0, constraints=declared))
    assert batch.get_checked(0, 0) == Checked(Decimal("1"), True, None)
    assert batch.get_checked(0, 1) == Checked(Decimal("1.5"), False, None)


def test_a_text_enum_match_delivers_the_declared_entry():
    """A text ``enum`` match delivers the caller's own entry and its index."""
    declared = Constraints(hypercast.Door.TEXT, enum=["Female", "Male"])
    batch = _read("MALE\nother\n", Column.text(0, constraints=declared))
    passed = batch.get_checked(0, 0)
    assert passed == Checked("Male", False, 1)
    assert declared.enum is not None and passed.value is declared.enum[1], "the caller's own string"
    assert batch.get_checked(0, 1) == Violation(Keyword.ENUM, "other", 0, 5)


def test_an_unconstrained_column_keeps_its_faults_under_the_checked_getter():
    """What cast passes, unadjusted, and a fault is the same fault. No stale metadata can reach it:
    an unconstrained column is given no checks."""
    batch = _read('5\n""\nx\n', Column.i32(0))
    assert batch.columns[0].column.constraints is None
    assert batch.get_checked(0, 0) == Checked(5, False, None)
    assert batch.get_checked(0, 1) == batch.get(0, 1)
    assert batch.get_checked(0, 1).reason is CastFailure.EMPTY
    assert batch.get_checked(0, 2) == batch.get(0, 2)
    assert batch.get_checked(0, 2).reason is CastFailure.MALFORMED


def test_the_plain_getters_refuse_a_constrained_column():
    """Every unchecked getter refuses a constrained column, so no violation reads as a value."""
    batch = _read(
        "7,a\n",
        Column.i32(0, constraints=_at_most(10)),
        Column.text(1, constraints=Constraints(hypercast.Door.TEXT, max_length=3)),
    )
    refusals = {
        "get": lambda: batch.get(0, 0),
        "text": lambda: batch.text(1, 0),
        "column[row]": lambda: batch.columns[0][0],
        "iter(column)": lambda: list(batch.columns[1]),
        "row.get": lambda: batch.row(0).get(0),
        "row.text": lambda: batch.row(0).text(1),
    }
    for name, read in refusals.items():
        with pytest.raises(TypeError, match="declares constraints: read it with get_checked"):
            read()
    # The control: the checked getter reads them, and the whole-column views are there.
    assert batch.get_checked(0, 0) == Checked(7, False, None)
    assert batch.get_checked(1, 0) == Checked("a", False, None)
    assert list(batch.columns[0].values) == [7]


def test_a_mismatched_door_is_refused():
    """``Constraints`` for i32 serve no other door: refused when the column is made, and again when
    the plan is bound for a column that got past that."""
    with pytest.raises(ValueError, match="Constraints for Door.I32 were given to a Door.U8 column"):
        Column.u8(0, constraints=_at_most(10))
    with pytest.raises(ValueError, match="Door.I32 were given to a Door.I64 column"):
        dataclasses.replace(Column.i32(0, constraints=_at_most(10)), door=Door.I64)
    with pytest.raises(TypeError, match="must be a hypercast.Constraints"):
        Column.i32(0, constraints={"maximum": 10})  # type: ignore[arg-type]

    smuggled = Column.i64(0)
    object.__setattr__(smuggled, "constraints", _at_most(10))
    with pytest.raises(
        ValueError, match=r"plan\[0\]: Constraints for Door.I32 were given to a Door.I64"
    ):
        DelimitedReader(b"1\n", HEADERLESS, [smuggled])
    # A door that takes no constraints has none to take.
    uuid = Column.uuid(0)
    object.__setattr__(uuid, "constraints", Constraints(hypercast.Door.TEXT))
    with pytest.raises(ValueError, match="Door.TEXT were given to a Door.UUID"):
        DelimitedReader(b"1\n", HEADERLESS, [uuid])


def test_faults_list_a_violation_with_its_keyword():
    """``faults()`` lists a violation with its keyword and no reason, beside a fault with its reason."""
    listed = _read("11\nx\n", Column.i32(0, constraints=_at_most(10))).faults()
    assert len(listed) == 2
    assert (listed[0].keyword, listed[0].reason) == (Keyword.MAXIMUM, None)
    assert (listed[0].offset, listed[0].length) == (0, 2)
    assert (listed[1].keyword, listed[1].reason) == (None, CastFailure.MALFORMED)


def test_checks_are_sized_like_verdicts():
    """Each constrained column's checks are copied for the batch's rows: the last row's violation
    names its keyword like the first's."""
    batch = _read("11\n" * 37, Column.i32(0, constraints=_at_most(10)))
    assert batch.rows == 37
    assert batch.get_checked(0, 36) == Violation(Keyword.MAXIMUM, 11, 0, 2)
    assert [problem.keyword for problem in batch.faults()] == [Keyword.MAXIMUM] * 37


def test_a_column_is_its_factory_call():
    """A constrained column's repr is its factory call, and its equality counts the constraints."""
    column = Column.i32(0, constraints=_at_most(10))
    assert repr(column) == f"Column.i32(0, constraints={_at_most(10)!r})"
    assert column == Column.i32(0, constraints=_at_most(10))
    assert column != Column.i32(0) and column != Column.i32(0, constraints=_at_most(11))
