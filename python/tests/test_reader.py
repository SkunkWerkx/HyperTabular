"""This binding's own promises, the ones the shared corpus cannot express: what a column
looks like from Python (a ``memoryview`` over the array the core wrote, or a ``list`` of
the values HyperCast's package gives that door), that a batch owns what it shows, how the
three kinds of source behave, and that a caller's bug is an exception of the documented
type while bad data never is.
"""

from __future__ import annotations

import array
import copy
import datetime as dt
import io
import pickle
import sys
import uuid
from decimal import Decimal
from pathlib import Path

import hypercast
import pytest

import hypertabular
from hypertabular import _native
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
    NumFormat,
    Success,
    TabularError,
    TabularFailure,
    UnixPrecision,
)

HEADLESS = Dialect(",", has_header=False)
TABS = Dialect("\t", quoting=False, has_header=False, skip_blank_lines=False)


def read_all(reader: DelimitedReader) -> list[Batch]:
    """Every batch the reader delivers."""
    return list(reader)


def one_batch(data: bytes, plan: list[Column], dialect: Dialect = HEADLESS) -> Batch:
    """The single batch a small input reads as."""
    (batch,) = read_all(DelimitedReader(data, dialect, plan))
    return batch


# --- the plan --------------------------------------------------------------------------------

# Every door, the factory call that declares it, and the hypercast door that judges it.
DOORS = {
    Door.BOOL: (lambda o: Column.bool(o), lambda t: hypercast.cast_bool(t)),
    Door.I8: (lambda o: Column.i8(o), lambda t: hypercast.cast_i8(t, NumFormat.INVARIANT)),
    Door.I16: (lambda o: Column.i16(o), lambda t: hypercast.cast_i16(t, NumFormat.INVARIANT)),
    Door.I32: (lambda o: Column.i32(o), lambda t: hypercast.cast_i32(t, NumFormat.INVARIANT)),
    Door.I64: (lambda o: Column.i64(o), lambda t: hypercast.cast_i64(t, NumFormat.INVARIANT)),
    Door.U8: (lambda o: Column.u8(o), lambda t: hypercast.cast_u8(t, NumFormat.INVARIANT)),
    Door.U16: (lambda o: Column.u16(o), lambda t: hypercast.cast_u16(t, NumFormat.INVARIANT)),
    Door.U32: (lambda o: Column.u32(o), lambda t: hypercast.cast_u32(t, NumFormat.INVARIANT)),
    Door.U64: (lambda o: Column.u64(o), lambda t: hypercast.cast_u64(t, NumFormat.INVARIANT)),
    Door.F32: (lambda o: Column.f32(o), lambda t: hypercast.cast_f32(t, NumFormat.INVARIANT)),
    Door.F64: (lambda o: Column.f64(o), lambda t: hypercast.cast_f64(t, NumFormat.INVARIANT)),
    Door.DECIMAL: (
        lambda o: Column.decimal(o),
        lambda t: hypercast.cast_decimal(t, NumFormat.INVARIANT),
    ),
    Door.UUID: (lambda o: Column.uuid(o), lambda t: hypercast.cast_uuid(t)),
    Door.TIMESTAMP: (lambda o: Column.timestamp(o), lambda t: hypercast.cast_timestamp(t)),
    Door.UNIX: (
        lambda o: Column.unix(o, UnixPrecision.NANOSECONDS),
        lambda t: hypercast.cast_unix(t, UnixPrecision.NANOSECONDS),
    ),
    Door.EXCEL_SERIAL: (
        lambda o: Column.excel_serial(o, ExcelEpoch.Y1900),
        lambda t: hypercast.cast_excel_serial(t, ExcelEpoch.Y1900),
    ),
    Door.DATE: (lambda o: Column.date(o), lambda t: hypercast.cast_date(t)),
    Door.DATE_ORDERED: (
        lambda o: Column.date_ordered(o, DateOrder.DAY_MONTH_YEAR),
        lambda t: hypercast.cast_date(t, DateOrder.DAY_MONTH_YEAR),
    ),
    Door.DATETIME: (
        lambda o: Column.datetime(o, DateOrder.MONTH_DAY_YEAR),
        lambda t: hypercast.cast_datetime(t, DateOrder.MONTH_DAY_YEAR),
    ),
    Door.TIME: (lambda o: Column.time(o), lambda t: hypercast.cast_time(t)),
    Door.DURATION: (lambda o: Column.duration(o), lambda t: hypercast.cast_duration(t)),
}

# Text that leans on each door's edges: the limits, the signs, the digits Python's own
# types cannot hold, the forms that do not cast. None of it contains a tab or a line break.
TEXTS = [
    "",
    " ",
    "0",
    "1",
    "-1",
    "true",
    "No",
    " yes ",
    "127",
    "128",
    "-128",
    "-129",
    "255",
    "256",
    "32767",
    "-32768",
    "65535",
    "65536",
    "2147483647",
    "-2147483648",
    "4294967295",
    "4294967296",
    "9223372036854775807",
    "-9223372036854775808",
    "18446744073709551615",
    "18446744073709551616",
    "(1,234)",
    "1,234.50",
    "0x1F",
    "50%",
    "1e3",
    "1E-3",
    "3.4028235e38",
    "3.5e38",
    "1e400",
    "nan",
    "0.1",
    "-0.0",
    "1.10",
    "79228162514264337593543950335",
    "79228162514264337593543950336",
    "0.0000000000000000000000000001",
    "12x4",
    "$5",
    "01020304-0506-4708-890a-0b0c0d0e0f10",
    "{01020304-0506-4708-890A-0B0C0D0E0F10}",
    "urn:uuid:01020304-0506-4708-890a-0b0c0d0e0f10",
    "0102030405064708890a0b0c0d0e0f10",
    "not-a-uuid",
    "2026-01-02T15:04:05Z",
    "2026-01-02T15:04:05.123456789Z",
    "2026-01-02T15:04:05.999999999+05:30",
    "1969-12-31T23:59:59.5Z",
    "0001-01-01T00:00:00Z",
    "9999-12-31T23:59:59.999999999Z",
    "2026-02-30T00:00:00Z",
    "2026-01-02",
    "1/7/2026",
    "31/12/1999",
    "12/31/1999",
    "2026-13-01",
    "1/7/2026 3:04:05.123456789 PM",
    "12/31/1999 23:59:59",
    "31.12.1999 00:00",
    "15:04:05",
    "15:04:05.123456789",
    "00:00:00",
    "23:59:59.999999999",
    "24:00:00",
    "7:5",
    "PT1H30M0.5S",
    "-P1DT6H",
    "P1D",
    "PT0.000000999S",
    "-PT0.000000999S",
    "-PT0.0000015S",
    "1:02:03",
    "-1.5s",
    "3.5s",
    "P10675199D",
    "P99999999D",
    "1700000000",
    "1700000000123456789",
    "-1",
    "-1500000000",
    "253402300800000000000",
    "45000",
    "45000.5",
    "60",
    "0.5",
    "2958465.999999999",
    "2958466",
    "1.25",
]


def test_every_door_has_a_factory_named_for_it():
    """Column has one factory per door, named for it."""
    assert set(DOORS) | {Door.TEXT} == set(Door) and len(Door) == 22
    for door, (declare, _) in DOORS.items():
        column = declare(3)
        assert (column.ordinal, column.door) == (3, door)
        assert getattr(Column, door.name.lower()) is not None
    assert Column.text(0).door is Door.TEXT
    # `date` mirrors hypercast.cast_date: strict ISO alone, the separated forms with an order.
    assert Column.date(0, DateOrder.MONTH_DAY_YEAR) == Column.date_ordered(
        0, DateOrder.MONTH_DAY_YEAR
    )
    assert repr(Column.unix(2, UnixPrecision.SECONDS)) == "Column.unix(2, UnixPrecision.SECONDS)"
    assert repr(Column.i32(0)) == "Column.i32(0)"


@pytest.mark.parametrize("door", list(DOORS), ids=lambda door: door.name.lower())
def test_a_cell_means_what_hypercast_says_of_the_same_text(door: Door):
    """HyperCast is the judge. One column of text, read through the door; every cell must be exactly
    the verdict — the same value of the same type, or the same fault and span — that hypercast's
    own door returns for that text.
    """
    declare, judge = DOORS[door]
    data = "\n".join(TEXTS).encode("utf-8") + b"\n"
    column = one_batch(data, [declare(0)], TABS).column(0)
    assert len(column) == len(TEXTS)
    values = column.values
    for row, text in enumerate(TEXTS):
        want = judge(text.encode("utf-8"))
        got = column[row]
        assert got == want, f"{door.name}: {text!r} -> {got!r}, hypercast says {want!r}"
        assert type(got) is type(want)
        assert column.raw(row) == text.encode("utf-8")
        if isinstance(want, Success):
            assert type(got.value) is type(want.value), f"{door.name}: {text!r}"
            assert values[row] == want.value
            if isinstance(want.value, dt.datetime):
                assert got.value.tzinfo is want.value.tzinfo
            if isinstance(want.value, Decimal):
                assert got.value.as_tuple() == want.value.as_tuple()
    # Something of each kind happened, or this proved less than it reads.
    kinds = {type(column[row]) for row in range(len(TEXTS))}
    assert kinds == {Success, Fault}, door


def test_the_verdict_types_are_hypercasts_own():
    """Success, Fault and CastFailure are HyperCast's own classes, not copies."""
    assert hypertabular.Success is hypercast.Success
    assert hypertabular.Fault is hypercast.Fault
    assert hypertabular.CastFailure is hypercast.CastFailure
    assert hypertabular.NumFormat is hypercast.NumFormat
    column = one_batch(
        b"7\nx\n\n", [Column.i32(0)], Dialect(",", has_header=False, skip_blank_lines=False)
    ).column(0)
    ok, malformed, empty = column
    assert type(ok) is hypercast.Success and ok == Success(7)
    assert type(malformed) is hypercast.Fault and malformed.reason is CastFailure.MALFORMED
    assert empty == Fault(CastFailure.EMPTY, 0, 0) and hypercast.optional(empty) is None
    assert hypercast.optional(ok) is ok


def test_a_declared_notation_is_hypercasts_num_format():
    """A numeric column's notation is a HyperCast NumFormat, honored as declared."""
    continental = NumFormat(",", ".", NumFormat.ALL, "€")
    batch = one_batch(
        b"1.234,5;\xe2\x82\xac 2,50\n",
        [Column.f64(0, continental), Column.decimal(1, fmt=continental)],
        Dialect(";", has_header=False),
    )
    assert batch.column(0).values.tolist() == [1234.5]
    assert batch.column(1).values == [Decimal("2.5")]
    assert batch.column(1)[0] == hypercast.cast_decimal("€ 2,50".encode(), continental)
    assert repr(batch.column(0).column) == "Column.f64(0, NumFormat(',', '.', 95, '€'))"


# --- columns, the way Python reads them -------------------------------------------------------

PRIMITIVES = [
    (Column.bool, "?", 1, b"true\nx\nno\n", [True, False, False]),
    (Column.i8, "b", 1, b"-128\nx\n127\n", [-128, 0, 127]),
    (Column.i16, "h", 2, b"-32768\nx\n32767\n", [-32768, 0, 32767]),
    (Column.i32, "i", 4, b"-2147483648\nx\n2147483647\n", [-(2**31), 0, 2**31 - 1]),
    (
        Column.i64,
        "q",
        8,
        b"-9223372036854775808\nx\n9223372036854775807\n",
        [-(2**63), 0, 2**63 - 1],
    ),
    (Column.u8, "B", 1, b"0\nx\n255\n", [0, 0, 255]),
    (Column.u16, "H", 2, b"0\nx\n65535\n", [0, 0, 65535]),
    (Column.u32, "I", 4, b"0\nx\n4294967295\n", [0, 0, 2**32 - 1]),
    (Column.u64, "Q", 8, b"0\nx\n18446744073709551615\n", [0, 0, 2**64 - 1]),
    (Column.f32, "f", 4, b"1.5\nx\n-0.25\n", [1.5, 0.0, -0.25]),
    (Column.f64, "d", 8, b"1.5\nx\n1e300\n", [1.5, 0.0, 1e300]),
]


@pytest.mark.parametrize(
    "declare, code, size, data, expected",
    PRIMITIVES,
    ids=[entry[0].__name__ for entry in PRIMITIVES],
)
def test_a_primitive_column_is_a_memoryview_of_the_array_the_core_wrote(
    declare, code, size, data, expected
):
    """A primitive column's values are a memoryview over the array the core filled."""
    column = one_batch(data, [declare(0)]).column(0)
    values = column.values
    assert type(values) is memoryview
    assert (values.format, values.itemsize, values.ndim, values.shape) == (code, size, 1, (3,))
    assert values.readonly and values.c_contiguous and values.nbytes == 3 * size
    assert values.tolist() == expected
    # The array itself, native-endian, with zero where the cell did not cast.
    assert array.array(code if code != "?" else "B", values.tobytes()).tolist() == [
        int(v) if code == "?" else v for v in expected
    ]
    with pytest.raises(TypeError):
        values[0] = values[1]
    # The same view each time: it is made once.
    assert column.values is values
    assert column.fault_count == 1
    assert column.faults() == [(1, Fault(CastFailure.MALFORMED, 0, 1))]


def test_verdicts_are_a_view_of_what_the_core_wrote():
    """A column's verdicts are read from what the core wrote, fault spans included."""
    column = one_batch(
        b"12x4\n\n256\n7\n", [Column.u8(0)], Dialect(",", has_header=False, skip_blank_lines=False)
    ).column(0)
    verdicts = column.verdicts
    assert type(verdicts) is memoryview and verdicts.readonly
    assert (verdicts.format, verdicts.itemsize, verdicts.shape) == ("I", 4, (4, 3))
    # offset, length, reason — reason 0 for a cell that cast, else CastFailure's code.
    assert verdicts.tolist() == [[2, 1, 2], [0, 0, 1], [0, 3, 3], [0, 0, 0]]
    assert column.verdicts is verdicts
    assert column.fault_count == 3
    assert column.faults() == [
        (0, Fault(CastFailure.MALFORMED, 2, 1)),
        (1, Fault(CastFailure.EMPTY, 0, 0)),
        (2, Fault(CastFailure.OUT_OF_RANGE, 0, 3)),
    ]
    assert column.values.tolist() == [0, 0, 0, 7]


def test_a_column_of_objects_is_a_list_with_none_where_a_cell_did_not_cast():
    """An object column is a list, None where a cell did not cast."""
    data = (
        b"1.10,01020304-0506-4708-890a-0b0c0d0e0f10,2026-01-02T15:04:05.123456789Z,2026-01-02,"
        b"1/7/2026 15:04:05,15:04:05.5,-PT1.5S, padded \n"
        b"x,x,x,x,x,x,x,\n"
    )
    plan = [
        Column.decimal(0),
        Column.uuid(1),
        Column.timestamp(2),
        Column.date(3),
        Column.datetime(4, DateOrder.MONTH_DAY_YEAR),
        Column.time(5),
        Column.duration(6),
        Column.text(7),
    ]
    batch = one_batch(data, plan)
    expected = [
        Decimal("1.1"),
        uuid.UUID("01020304-0506-4708-890a-0b0c0d0e0f10"),
        dt.datetime(2026, 1, 2, 15, 4, 5, 123456, tzinfo=dt.timezone.utc),
        dt.date(2026, 1, 2),
        dt.datetime(2026, 1, 7, 15, 4, 5),
        dt.time(15, 4, 5, 500000),
        dt.timedelta(seconds=-1.5),
        " padded ",
    ]
    for column, want in zip(batch.columns, expected):
        values = column.values
        assert type(values) is list and values == [want, None]
        assert type(values[0]) is type(want)
        assert column.values is values
        assert column.fault_count == 1
    assert batch.column(0).values[0].as_tuple() == Decimal("1.1").as_tuple()
    assert batch.column(2).values[0].tzinfo is dt.timezone.utc
    assert batch.column(4).values[0].tzinfo is None
    # Text fails one way only: no bytes at all.
    assert batch.column(7)[1] == Fault(CastFailure.EMPTY, 0, 0)


def test_a_cell_is_matchable_and_a_column_is_a_sequence_of_cells():
    """A column is a sequence of cells, each a Success or Fault to match on."""
    column = one_batch(b"1\nx\n3\n", [Column.i32(0)]).column(0)
    assert type(column) is ColumnData and len(column) == 3
    described = []
    for verdict in column:
        match verdict:
            case Success(value):
                described.append(f"got {value}")
            case Fault(reason, offset, length):
                described.append(f"{reason.name} at {offset}+{length}")
    assert described == ["got 1", "MALFORMED at 0+1", "got 3"]
    assert column[-1] == Success(3) and column[-3] == column[0]
    for row in (3, -4, sys.maxsize):
        with pytest.raises(IndexError):
            column[row]
        with pytest.raises(IndexError):
            column.raw(row)
    with pytest.raises(TypeError):
        column["0"]


def test_raw_is_the_text_a_cell_was_cast_from():
    """raw(i) is the unescaped bytes a cell was cast from."""
    data = b'n,q\n" 7 ","say ""hi"""\nx,""\n12x4,"a\nb"\n'
    batch = one_batch(data, [Column.i32(0), Column.text(1), Column.i32(5)], Dialect.CSV)
    number, text, beyond = batch.columns
    # Quoting resolved, nothing trimmed — what a fault's span indexes.
    assert [number.raw(row) for row in range(3)] == [b" 7 ", b"x", b"12x4"]
    assert [text.raw(row) for row in range(3)] == [b'say "hi"', b"", b"a\nb"]
    assert text.values == ['say "hi"', None, "a\nb"]
    assert number[2] == Fault(CastFailure.MALFORMED, 2, 1)
    assert number.raw(2)[2:3] == b"x"
    # A source column past the record's end reads as empty.
    assert [beyond.raw(row) for row in range(3)] == [b"", b"", b""]
    assert beyond.fault_count == 3 and beyond.values.tolist() == [0, 0, 0]
    assert batch.raw(1, 0) == text.raw(0) and batch.raw(0, -1) == b"12x4"
    with pytest.raises(IndexError):
        batch.raw(3, 0)
    with pytest.raises(IndexError):
        batch.column(3)
    assert batch.column(-1).column is batch.columns[2].column


def test_text_that_is_not_utf8_is_replaced_in_the_str_and_kept_in_raw():
    """Invalid UTF-8 is replaced in the str value and kept byte for byte in raw."""
    column = one_batch(b"caf\xe9\nok\n", [Column.text(0)]).column(0)
    assert column.values == ["caf�", "ok"]
    assert column.raw(0) == b"caf\xe9"
    header = DelimitedReader(b"caf\xe9\n1\n", Dialect.CSV, [Column.i32(0)]).header
    assert header == ("caf�",)


def test_a_plan_is_a_projection():
    """A plan picks, reorders and repeats source columns."""
    data = b"a,b,c\n1,2,3\n4,5,6\n"
    batch = one_batch(
        data, [Column.text(2), Column.i32(0), Column.f64(0), Column.i32(0)], Dialect.CSV
    )
    assert batch.column(0).values == ["3", "6"]
    assert batch.column(1).values.tolist() == [1, 4]
    assert batch.column(2).values.tolist() == [1.0, 4.0]
    # No columns at all still counts the rows.
    empty = one_batch(data, [], Dialect.CSV)
    assert (empty.rows, empty.columns) == (2, ())
    assert repr(empty) == "<hypertabular.Batch rows=2 columns=0>"


# --- batches ------------------------------------------------------------------------------------


def _rows(count: int) -> bytes:
    lines = ["id,name,score"]
    for row in range(count):
        name = f'"name {row}, ""quoted"""' if row % 7 == 0 else f"name {row}"
        score = "oops" if row % 11 == 0 else f"{row / 4}"
        lines.append(f"{row},{name},{score}")
    return ("\n".join(lines) + "\n").encode()


def _collect(reader: DelimitedReader) -> tuple[list[int], list[str | None], list[float], list[int]]:
    ids: list[int] = []
    names: list[str | None] = []
    scores: list[float] = []
    faults: list[int] = []
    for batch in reader:
        first = len(ids)
        ids.extend(batch.column(0).values)
        names.extend(batch.column(1).values)
        scores.extend(batch.column(2).values)
        faults.extend(first + row for row, _ in batch.column(2).faults())
    return ids, names, scores, faults


def test_how_the_input_is_cut_up_does_not_change_the_answer(tmp_path: Path):
    """Buffer and batch sizes change how the input is read, never what it reads as."""
    count = 5000
    data = _rows(count)
    plan = [Column.i32(0), Column.text(1), Column.f64(2)]
    expected = (
        list(range(count)),
        [f'name {row}, "quoted"' if row % 7 == 0 else f"name {row}" for row in range(count)],
        [0.0 if row % 11 == 0 else row / 4 for row in range(count)],
        [row for row in range(count) if row % 11 == 0],
    )
    path = tmp_path / "rows.csv"
    path.write_bytes(data)
    for batch_rows in (1, 7, 4096, 100_000):
        assert _collect(DelimitedReader(data, Dialect.CSV, plan, batch_rows=batch_rows)) == expected
    for buffer_bytes in (1, 7, 4096, DelimitedReader.DEFAULT_BUFFER_BYTES):
        reader = DelimitedReader(io.BytesIO(data), Dialect.CSV, plan, buffer_bytes=buffer_bytes)
        assert _collect(reader) == expected
        assert reader.records == count + 1
        with DelimitedReader.open(
            path, Dialect.CSV, plan, batch_rows=999, buffer_bytes=buffer_bytes
        ) as reader:
            assert _collect(reader) == expected
    with open(path, "rb", buffering=0) as raw:
        assert _collect(DelimitedReader(raw, Dialect.CSV, plan, buffer_bytes=64)) == expected


@pytest.mark.parametrize("source", ["memory", "stream", "path"])
def test_a_batch_owns_what_it_shows(source: str, tmp_path: Path):
    """A batch stays valid after the reader moves on."""
    data = _rows(300)
    plan = [Column.i32(0), Column.text(1), Column.f64(2)]
    if source == "memory":
        reader = DelimitedReader(data, Dialect.CSV, plan, batch_rows=100)
    elif source == "stream":
        reader = DelimitedReader(
            io.BytesIO(data), Dialect.CSV, plan, batch_rows=100, buffer_bytes=512
        )
    else:
        path = tmp_path / "rows.csv"
        path.write_bytes(data)
        reader = DelimitedReader.open(path, Dialect.CSV, plan, batch_rows=100, buffer_bytes=512)
    first = reader.read()
    assert first is not None
    ids = first.column(0).values
    before = ids.tolist()
    # Read on, to the end, and let the reader go: the first batch is untouched — its
    # arrays, its text and the raw text of every cell.
    rest = read_all(reader)
    assert sum(batch.rows for batch in rest) == 300 - first.rows
    reader.close()
    del reader, rest, data
    assert ids.tolist() == before == list(range(first.rows))
    assert first.column(1).values[7] == 'name 7, "quoted"'
    assert first.column(1).raw(7) == b'name 7, "quoted"'
    assert first.column(2).raw(0) == b"oops" and first.column(2)[0] == Fault(
        CastFailure.MALFORMED, 0, 1
    )
    # And a view outlives the batch it came from.
    del first
    assert ids.tolist() == before


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide cannot start a thread")
def test_readers_on_several_threads_do_not_disturb_each_other():
    """The core runs with the interpreter released, so readers on other threads run beside it; each
    one's arrays are its own.
    """
    from concurrent.futures import ThreadPoolExecutor

    data = _rows(3000)
    plan = [Column.i32(0), Column.text(1), Column.f64(2)]
    expected = _collect(DelimitedReader(data, Dialect.CSV, plan))

    def read(index: int) -> bool:
        if index % 2:
            reader = DelimitedReader(
                io.BytesIO(data), Dialect.CSV, plan, batch_rows=64, buffer_bytes=257
            )
        else:
            reader = DelimitedReader(data, Dialect.CSV, plan, batch_rows=17 + index)
        return _collect(reader) == expected

    with ThreadPoolExecutor(max_workers=8) as pool:
        assert all(pool.map(read, range(32)))


def test_reading_past_the_end_is_none_and_iteration_stops():
    """After the last batch, next_batch is None and iteration stops."""
    reader = DelimitedReader(b"a\n1\n2\n3\n", Dialect.CSV, [Column.i32(0)], batch_rows=2)
    assert reader.batch_rows == 2 and reader.dialect is Dialect.CSV
    assert iter(reader) is reader
    assert [batch.rows for batch in reader] == [2, 1]
    assert reader.read() is None and reader.read() is None
    with pytest.raises(StopIteration):
        next(reader)
    assert reader.records == 4


def test_the_header():
    """The header is the first record's fields when the dialect has one, else empty."""
    plan = [Column.i32(0)]
    assert DelimitedReader(b"a,b\n1,2\n", Dialect.CSV, plan).header == ("a", "b")
    assert DelimitedReader(b"1,2\n", HEADLESS, plan).header is None
    # No record at all: an empty header, and no rows.
    empty = DelimitedReader(b"", Dialect.CSV, plan)
    assert empty.header == () and read_all(empty) == []
    assert DelimitedReader(io.BytesIO(b""), Dialect.CSV, plan).header == ()
    # A byte-order mark is not data, and a quoted name is its text.
    marked = DelimitedReader(b'\xef\xbb\xbf"first, ""id""",b\n1,2\n', Dialect.CSV, plan)
    assert marked.header == ('first, "id"', "b")
    assert read_all(marked)[0].column(0).values.tolist() == [1]
    # More names than fit the first table the binding offers the core.
    wide = ",".join(f"c{index}" for index in range(200))
    assert DelimitedReader(wide.encode() + b"\n", Dialect.CSV, plan).header == tuple(
        wide.split(",")
    )


# --- structure ----------------------------------------------------------------------------------


def test_a_structural_failure_is_an_exception_after_the_intact_rows():
    """A structural failure raises only after the rows before it are delivered."""
    reader = DelimitedReader(
        b"a,b\n1,2\n3,4\n5\n6,7\n", Dialect.CSV, [Column.i32(0)], batch_rows=1024
    )
    batch = reader.read()
    assert batch is not None and batch.column(0).values.tolist() == [1, 3]
    with pytest.raises(TabularError) as raised:
        reader.read()
    error = raised.value
    assert error.kind is TabularFailure.COLUMN_COUNT
    assert (error.record, error.line, error.byte, error.expected, error.found) == (3, 4, 12, 2, 1)
    assert str(error) == "record 3 (line 4, byte 12) has 1 cells; the first record had 2"
    # Final: the same failure, again, however it is asked for.
    with pytest.raises(TabularError) as again:
        next(reader)
    assert again.value is error
    # The rows before it are still there.
    assert batch.column(0).values.tolist() == [1, 3]


def test_a_quote_never_closed():
    """A quote left open at the end of input is a structural failure."""
    reader = DelimitedReader(
        io.BytesIO(b'a\n1\n"2\n3\n'), Dialect.CSV, [Column.i32(0)], buffer_bytes=2
    )
    assert [batch.column(0).values.tolist() for batch in _until_failure(reader)] == [[1]]
    with pytest.raises(TabularError) as raised:
        reader.read()
    error = raised.value
    assert error.kind is TabularFailure.UNCLOSED_QUOTE
    assert (error.record, error.line, error.byte, error.expected, error.found) == (2, 3, 4, 0, 0)
    assert str(error) == "the input ended inside a quoted cell in record 2 (line 3, byte 4)"


def _until_failure(reader: DelimitedReader) -> list[Batch]:
    batches = []
    try:
        for batch in reader:
            batches.append(batch)
    except TabularError:
        pass
    return batches


@pytest.mark.parametrize("source", ["memory", "stream", "path"])
def test_a_record_past_the_row_ceiling_is_a_structural_failure(source: str, tmp_path: Path):
    """The real limits are a gibibyte for a stream's buffer and two for what the core is handed of
    an in-memory input at once; the suite lowers both to reach them.
    """
    data = b"1\n2\n" + b"x" * 31 + b"\n" + b"y" * 32 + b"\n3\n"
    plan = [Column.text(0)]
    if source == "memory":
        reader = DelimitedReader(data, HEADLESS, plan)
    elif source == "stream":
        reader = DelimitedReader(io.BytesIO(data), HEADLESS, plan, buffer_bytes=4)
    else:
        path = tmp_path / "rows.csv"
        path.write_bytes(data)
        reader = DelimitedReader.open(path, HEADLESS, plan, buffer_bytes=4)
    _native._limits(reader, 32, 32)
    # A record of exactly the limit, line ending included, is read; one byte more is not.
    seen = [value for batch in _until_failure(reader) for value in batch.column(0).values]
    assert seen == ["1", "2", "x" * 31]
    with pytest.raises(TabularError) as raised:
        reader.read()
    error = raised.value
    assert error.kind is TabularFailure.ROW_TOO_LONG
    assert (error.record, error.line, error.byte, error.expected, error.found) == (3, 4, 36, 0, 0)
    with pytest.raises(TabularError) as again:
        reader.read()
    assert again.value is error
    reader.close()


def test_an_in_memory_input_larger_than_one_call_takes_is_read_in_windows():
    """In-memory input larger than one core call is read in windows, losing nothing."""
    count = 500
    data = _rows(count)
    plan = [Column.i32(0), Column.text(1), Column.f64(2)]
    expected = _collect(DelimitedReader(data, Dialect.CSV, plan))
    assert len(expected[0]) == count
    for window in (64, 100, 4096, len(data) - 14, len(data)):
        reader = DelimitedReader(data, Dialect.CSV, plan, batch_rows=64)
        _native._limits(reader, 1 << 30, window)
        assert _collect(reader) == expected, window


def test_a_broken_header_fails_the_constructor():
    """A header that cannot be read fails the constructor."""
    with pytest.raises(TabularError) as raised:
        DelimitedReader(b'a,"b\n1,2\n', Dialect.CSV, [Column.i32(0)])
    assert raised.value.kind is TabularFailure.UNCLOSED_QUOTE
    assert (raised.value.record, raised.value.line, raised.value.byte) == (0, 1, 0)


def test_the_failure_copies_and_pickles_with_everything_it_carries():
    """TabularError survives copy and pickle with every field it carries."""
    error = TabularError(TabularFailure.COLUMN_COUNT, 3, 4, 12, 2, 1)
    assert isinstance(error, Exception) and not isinstance(error, ValueError)
    for twin in (copy.copy(error), pickle.loads(pickle.dumps(error))):
        assert type(twin) is TabularError and str(twin) == str(error)
        assert (twin.kind, twin.record, twin.line, twin.byte, twin.expected, twin.found) == (
            TabularFailure.COLUMN_COUNT,
            3,
            4,
            12,
            2,
            1,
        )
    long = TabularError(TabularFailure.ROW_TOO_LONG, 9, 10, 1234)
    assert (
        str(long)
        == f"record 9 (line 10, byte 1234) exceeds the {DelimitedReader.MAX_ROW_BYTES}-byte row ceiling"
    )
    assert DelimitedReader.MAX_ROW_BYTES == 1 << 30


# --- sources ------------------------------------------------------------------------------------


def test_a_path_is_opened_and_closed_by_the_reader(tmp_path: Path):
    """A path is opened by the reader and closed when it is."""
    path = tmp_path / "rows.csv"
    path.write_bytes(b"a\n1\n2\n")
    plan = [Column.i32(0)]
    for spelled in (path, str(path)):
        with DelimitedReader.open(spelled, Dialect.CSV, plan) as reader:
            assert reader.header == ("a",)
            batch = reader.read()
        assert batch is not None and batch.column(0).values.tolist() == [1, 2]
        with pytest.raises(ValueError, match="closed"):
            reader.read()
        assert reader.header == ("a",)
        reader.close()  # twice is fine
    with pytest.raises(FileNotFoundError):
        DelimitedReader.open(tmp_path / "missing.csv", Dialect.CSV, plan)
    with pytest.raises(TypeError):
        DelimitedReader.open(b"a\n1\n", Dialect.CSV, plan)  # type: ignore[arg-type]


def test_a_file_object_is_the_callers_to_close():
    """A file object passed in stays open: closing it is the caller's job."""
    stream = io.BytesIO(b"a\n1\n2\n")
    with DelimitedReader(stream, Dialect.CSV, [Column.i32(0)]) as reader:
        assert read_all(reader)[0].column(0).values.tolist() == [1, 2]
    assert not stream.closed


def test_a_read_that_fails_is_the_callers_exception_and_not_the_end():
    """A read that raises propagates the caller's exception rather than ending the input."""

    class Flaky:
        def __init__(self) -> None:
            self.calls = 0
            self.inner = io.BytesIO(b"a\n1\n2\n")

        def read(self, size: int) -> bytes:
            self.calls += 1
            if self.calls == 2:
                raise OSError("the network went away")
            return self.inner.read(size)

    # Two bytes a read: the first is the header, the second fails.
    reader = DelimitedReader(Flaky(), Dialect.CSV, [Column.i32(0)], buffer_bytes=2)
    with pytest.raises(OSError, match="network"):
        reader.read()
    # Nothing was lost: the read is asked for again and the rows arrive.
    assert [value for batch in reader for value in batch.column(0).values] == [1, 2]


# --- a caller's bug is an exception, of the documented type -------------------------------------


def test_the_source_must_be_bytes_or_a_binary_file_object(tmp_path: Path):
    """The constructor takes bytes or a binary file object; a path goes through open."""
    plan = [Column.i32(0)]
    for source in ("a\n1\n", tmp_path / "rows.csv"):
        with pytest.raises(TypeError, match="DelimitedReader.open"):
            DelimitedReader(source, Dialect.CSV, plan)  # type: ignore[arg-type]
    for source in (42, None, bytearray(b"a\n1\n"), memoryview(b"a\n1\n")):
        with pytest.raises(TypeError, match="bytes or a binary file object"):
            DelimitedReader(source, Dialect.CSV, plan)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="binary mode"):
        DelimitedReader(io.StringIO("a\n1\n"), Dialect.CSV, plan)  # type: ignore[arg-type]

    class Greedy:
        def read(self, size: int) -> bytes:
            return b"x" * (size + 1)

    with pytest.raises(ValueError, match="more than size"):
        DelimitedReader(Greedy(), Dialect.CSV, plan)


def test_the_dialect_is_declared_and_checked():
    """A dialect is declared, and an invalid one raises."""
    assert Dialect.CSV == Dialect(",", True, True, True)
    assert (Dialect.TSV.separator, Dialect.PSV.separator) == ("\t", "|")
    for separator in ("", ",,", '"', "\n", "\r", "\x00", "\x7f", "é", "€"):
        with pytest.raises(ValueError, match="separator"):
            Dialect(separator)
    with pytest.raises(TypeError):
        Dialect(44)  # type: ignore[arg-type]
    with pytest.raises(AttributeError):
        Dialect.CSV.separator = ";"  # type: ignore[misc]
    for dialect in (",", None, (",", True, True, True)):
        with pytest.raises(TypeError, match="Dialect"):
            DelimitedReader(b"", dialect, [])  # type: ignore[arg-type]
    for separator in (";", "|", "\t", " ", "~", "a"):
        reader = DelimitedReader(
            f"1{separator}2\n".encode(), Dialect(separator, has_header=False), [Column.i32(1)]
        )
        assert read_all(reader)[0].column(0).values.tolist() == [2]


def test_the_plan_is_columns_and_checked():
    """A plan must be Column objects; anything else raises."""
    for plan in ([0], [Column.i32(0), "i32"], [None]):
        with pytest.raises(TypeError, match="Column"):
            DelimitedReader(b"", Dialect.CSV, plan)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        DelimitedReader(b"", Dialect.CSV, 7)  # type: ignore[arg-type]
    # Any iterable will do, and the reader keeps its own tuple of it.
    reader = DelimitedReader(b"", Dialect.CSV, (Column.i32(index) for index in range(3)))
    assert reader.plan == (Column.i32(0), Column.i32(1), Column.i32(2))

    for ordinal in (-1, 2**31):
        with pytest.raises(ValueError, match="ordinal"):
            Column.i32(ordinal)
    for ordinal in ("0", 0.0, None, True):
        with pytest.raises(TypeError, match="ordinal"):
            Column.text(ordinal)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="NumFormat"):
        Column.i32(0, (".", ",", 0))  # type: ignore[arg-type]
    for declare, members in (
        (Column.unix, 4),
        (Column.excel_serial, 2),
        (Column.date_ordered, 3),
        (Column.datetime, 3),
    ):
        for not_a_member in (0, members + 1, "1", None):
            with pytest.raises(ValueError):
                declare(0, not_a_member)  # type: ignore[arg-type]
        assert declare(0, members).declared == members  # type: ignore[arg-type]
    # A column built around the factories is still checked where it is used.
    with pytest.raises(ValueError, match=r"plan\[1\] names no door"):
        DelimitedReader(b"", Dialect.CSV, [Column.i32(0), Column(0, Door.UNIX)])
    with pytest.raises(ValueError, match=r"plan\[0\] names no door"):
        DelimitedReader(b"", Dialect.CSV, [Column(0, 99)])  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=r"plan\[0\].format"):
        DelimitedReader(b"", Dialect.CSV, [Column(0, Door.I32, format=None)])  # type: ignore[arg-type]
    with pytest.raises(AttributeError):
        Column.i32(0).ordinal = 1  # type: ignore[misc]


def test_sizes_are_checked():
    """batch_rows and buffer_bytes must be positive."""
    plan = [Column.i32(0)]
    for name in ("batch_rows", "buffer_bytes"):
        with pytest.raises(ValueError, match=name):
            DelimitedReader(io.BytesIO(b""), Dialect.CSV, plan, **{name: 0})
        with pytest.raises(OverflowError):
            DelimitedReader(io.BytesIO(b""), Dialect.CSV, plan, **{name: -1})
        with pytest.raises(TypeError):
            DelimitedReader(io.BytesIO(b""), Dialect.CSV, plan, **{name: "1"})
    with pytest.raises(TypeError):
        DelimitedReader(b"", Dialect.CSV, plan, 1024)  # type: ignore[misc]
    # The reader allocates its arrays once, up front: a batch nobody could allocate is
    # refused there, and is not a crash.
    for plan in ([Column.i32(0)], [Column.text(0)], []):
        with pytest.raises(MemoryError, match="batch_rows"):
            DelimitedReader(b"a\n1\n", Dialect.CSV, plan, batch_rows=sys.maxsize)


def test_numpy_takes_a_column_without_copying():
    """numpy.asarray takes a primitive column without copying it."""
    numpy = pytest.importorskip("numpy")
    batch = one_batch(b"1,2.5\nx,x\n3,-1\n", [Column.i64(0), Column.f64(1)])
    ids = numpy.asarray(batch.column(0).values)
    assert ids.dtype == numpy.int64 and ids.tolist() == [1, 0, 3] and not ids.flags.writeable
    assert numpy.shares_memory(ids, numpy.asarray(batch.column(0).values))
    cast = numpy.asarray(batch.column(1).verdicts)[:, 2] == 0
    assert cast.tolist() == [True, False, True]
    assert numpy.asarray(batch.column(1).values)[cast].tolist() == [2.5, -1.0]


def test_an_arena_that_cramps_a_batch_is_grown():
    """The core ends a batch early when its arena fills; the batch after one that did starts
    with the arena doubled, so twenty thousand escaped rows are a handful of batches."""
    row = '"' + 'say ""hi"" ' * 8 + '"\n'
    expected = 'say "hi" ' * 8
    reader = DelimitedReader(
        (row * 20_000).encode(), Dialect(",", has_header=False), [Column.text(0)]
    )
    batches = list(reader)
    assert sum(batch.rows for batch in batches) == 20_000
    assert len(batches) < 15
    assert all(batch.column(0).values[-1] == expected for batch in batches)
    # And every row of a batch is where it was in the text.
    assert batches[0].line(0) == 1 and batches[-1].line(-1) == 20_000
