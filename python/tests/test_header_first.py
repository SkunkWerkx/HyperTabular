"""The 0.8 surface the corpus replays cannot pin down on their own: the header as a lookup,
a reader or sheet opened without a plan and bound after, the row view, a workbook read from
a file object, and asyncio — a stream awaited for input, ``async for``, and a cancelled read
that leaves the reader as it was."""

from __future__ import annotations

import asyncio
import copy
import io
import pickle
import sys
from pathlib import Path

import pytest
from test_corpus import _AsyncDribble, _corpus_dir, _run

from hypertabular import (
    Column,
    DelimitedReader,
    Dialect,
    Fault,
    Header,
    Row,
    SheetOptions,
    Success,
    TabularError,
    TabularFailure,
    Workbook,
)

HEADLESS = Dialect(",", has_header=False)


# --- the header ---------------------------------------------------------------------------------


def test_the_header_is_still_the_tuple_of_names_it_was():
    """A Header is a tuple of str, equal to the plain tuple, and code that took one works."""
    header = DelimitedReader(b"id,name\n1,a\n", Dialect.CSV).header
    assert isinstance(header, Header) and isinstance(header, tuple)
    assert header == ("id", "name") and ("id", "name") == header
    assert list(header) == ["id", "name"] and header[1] == "name" and len(header) == 2
    assert "id" in header and header.index("name") == 1
    assert repr(header) == "Header(('id', 'name'))"
    assert hash(header) == hash(("id", "name"))
    for again in (copy.copy(header), copy.deepcopy(header), pickle.loads(pickle.dumps(header))):
        assert again == header and again.ordinal(b"name") == 1


def test_a_name_is_looked_up_exactly_and_the_first_wins():
    """Exact, case- and space-sensitive, untrimmed; duplicates resolve to the first."""
    header = DelimitedReader(b"id,Name, name,name,id\n", Dialect.CSV).header
    assert header is not None
    assert header.ordinal("id") == 0 and header.ordinal("name") == 3
    assert header.ordinal("Name") == 1 and header.ordinal(" name") == 2
    assert header.ordinal(b"id") == 0 and header.ordinal(b" name") == 2
    for missing in ("ID", "name ", "nom", ""):
        assert header.get(missing) is None and header.get(missing, -1) == -1
    assert header.get("name") == 3 and header.get(b"name", -1) == 3


def test_a_missing_name_is_a_key_error_naming_it():
    """ordinal raises KeyError, and the message says which column is missing."""
    header = DelimitedReader(b"id,name\n", Dialect.CSV).header
    assert header is not None
    with pytest.raises(KeyError, match="Region Code"):
        header.ordinal("Region Code")
    with pytest.raises(KeyError, match="Region Code"):
        header.ordinal(b"Region Code")
    with pytest.raises(TypeError):
        header.get(0)  # type: ignore[arg-type]


def test_a_name_that_is_not_utf8_is_found_by_its_own_bytes():
    """The str has U+FFFD; the bytes are what the file holds, and find the column."""
    header = DelimitedReader(b"id,caf\xe9\n", Dialect.CSV).header
    assert header is not None
    assert header[1] == "caf�"
    assert header.ordinal(b"caf\xe9") == 1 and header.ordinal("caf�") == 1
    assert header.get(b"caf\xc3\xa9") is None


def test_a_header_can_be_made_by_hand():
    """Header(names) encodes each name for the bytes lookup; raw must match in length."""
    assert Header(("a", "é")).ordinal("é".encode()) == 1
    assert Header() == ()
    with pytest.raises(ValueError):
        Header(("a",), (b"a", b"b"))


# --- open, then bind --------------------------------------------------------------------------


def test_open_header_first_then_bind(tmp_path: Path):
    """The flow the feature is for, from bytes, a file object and a path."""
    data = b"M49 Code,Country,ISO-alpha2 Code\n4,Afghanistan,AF\n8,Albania,AL\n"
    path = tmp_path / "m49.csv"
    path.write_bytes(data)
    for reader in (
        DelimitedReader(data, Dialect.CSV),
        DelimitedReader(io.BytesIO(data), Dialect.CSV, buffer_bytes=4),
        DelimitedReader.open(path, Dialect.CSV),
    ):
        with reader:
            header = reader.header
            assert header is not None and reader.column_count == 3
            assert not reader.is_bound and reader.plan == ()
            reader.bind(
                [
                    Column.text(header.ordinal("ISO-alpha2 Code")),
                    Column.i32(header.ordinal("M49 Code")),
                ]
            )
            assert reader.is_bound and len(reader.plan) == 2
            (batch,) = list(reader)
            assert batch.column(0).values == ["AF", "AL"]
            assert batch.column(1).values.tolist() == [4, 8]


def test_reading_before_bind_is_an_error_that_does_not_last():
    """read() before bind raises RuntimeError; bind puts it right."""
    reader = DelimitedReader(b"a\n1\n", Dialect.CSV)
    for attempt in (reader.read, lambda: next(reader), lambda: list(reader.rows())):
        with pytest.raises(RuntimeError, match="bind"):
            attempt()
    reader.bind([Column.i32(0)])
    batch = reader.read()
    assert batch is not None and batch.column(0).values.tolist() == [1]


def test_bind_is_once_and_a_refused_plan_leaves_the_reader_unbound():
    """A second bind raises; a plan that cannot be honoured is refused and changes nothing."""
    reader = DelimitedReader(b"a\n1\n", Dialect.CSV)
    with pytest.raises(TypeError, match="Column"):
        reader.bind(["i32"])  # type: ignore[list-item]
    assert not reader.is_bound and reader.plan == ()
    reader.bind([Column.i32(0)])
    with pytest.raises(RuntimeError, match="bound once"):
        reader.bind([Column.i32(0)])
    # The plan-taking constructor has bound already.
    planned = DelimitedReader(b"a\n1\n", Dialect.CSV, [Column.i32(0)])
    assert planned.is_bound
    with pytest.raises(RuntimeError, match="bound once"):
        planned.bind([Column.i32(0)])


def test_a_plan_up_front_is_checked_before_the_header_is_read():
    """A plan error comes first, even over a header that cannot be read."""
    with pytest.raises(TypeError, match="Column"):
        DelimitedReader(b'a,"b\n', Dialect.CSV, [0])  # type: ignore[list-item]
    with pytest.raises(TabularError):
        DelimitedReader(b'a,"b\n', Dialect.CSV)


def test_a_headerless_source_binds_by_position():
    """No header: header is None, the width is known once the first record is read."""
    reader = DelimitedReader(b"1,x\n2,y\n", HEADLESS)
    assert reader.header is None and reader.column_count is None
    reader.bind([Column.text(1), Column.i32(0)])
    batch = reader.read()
    assert batch is not None and batch.column(0).values == ["x", "y"]
    assert reader.column_count == 2


def test_a_sheet_header_first(tmp_path: Path):
    """A sheet opened without a plan, by index and by name, bound after its header."""
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    planned = book.sheet(0, SheetOptions(), [Column.text(1), Column.i64(0)])
    expected = [list(batch.column(0).values) for batch in planned]
    for which in (0, book.sheets[0].name):
        sheet = book.sheet(which, SheetOptions())
        header = sheet.header
        assert isinstance(header, Header) and header == planned.header
        assert not sheet.is_bound and sheet.plan == ()
        with pytest.raises(RuntimeError, match="bind"):
            sheet.read()
        with pytest.raises(TypeError, match="Column"):
            sheet.bind([1])  # type: ignore[list-item]
        assert not sheet.is_bound
        sheet.bind([Column.text(1), Column.i64(header.ordinal(header[0]))])
        with pytest.raises(RuntimeError, match="bound once"):
            sheet.bind([Column.text(1)])
        assert [list(batch.column(0).values) for batch in sheet] == expected


# --- rows ---------------------------------------------------------------------------------------


def test_a_row_is_the_batch_read_across():
    """Each Row member is the batch's own call at the row's index."""
    plan = [Column.i32(0), Column.text(1)]
    (batch,) = list(DelimitedReader(b"1,a\nx,\n3,c\n", HEADLESS, plan))
    rows = list(batch)
    assert [row.index for row in rows] == [0, 1, 2]
    assert all(isinstance(row, Row) for row in rows)
    assert [row.line for row in rows] == [1, 2, 3]
    assert rows[0].get(0) == Success(1) and isinstance(rows[1].get(0), Fault)
    assert [row.text(1) for row in rows] == ["a", None, "c"]
    assert [row.raw(0) for row in rows] == [b"1", b"x", b"3"]
    assert batch.row(-1).index == 2 and repr(batch.row(0)) == "<hypertabular.Row index=0 line=1>"
    with pytest.raises(IndexError):
        batch.row(3)
    with pytest.raises(IndexError):
        rows[0].get(2)
    with pytest.raises(TypeError, match="text door"):
        rows[0].text(0)


def test_rows_span_batches_and_outlive_the_read():
    """reader.rows() reads across batches and stops at the end; a row stays valid after."""
    data = b"n\n" + b"".join(b"%d\n" % index for index in range(10))
    reader = DelimitedReader(data, Dialect.CSV, [Column.i64(0)], batch_rows=3)
    rows = list(reader.rows())
    assert [row.get(0) for row in rows] == [Success(index) for index in range(10)]
    assert len({id(row.batch) for row in rows}) == 4
    assert reader.read() is None and list(reader.rows()) == []
    # A sheet's rows, likewise.
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    sheet = book.sheet(0, SheetOptions(batch_rows=1), [Column.text(1)])
    lines = [row.line for row in sheet.rows()]
    assert lines == [2, 3, 4, 6]


# --- a workbook from a stream -----------------------------------------------------------------


def test_a_workbook_from_a_file_object_reads_it_to_the_end(tmp_path: Path):
    """A binary file object is read to its end; it is the caller's to close."""
    path = _corpus_dir() / "workbook" / "basic.xlsx"
    with path.open("rb") as stream:
        book = Workbook(stream)
        assert not stream.closed and stream.read() == b""
    assert book.sheets == Workbook.open(path).sheets
    with pytest.raises(TypeError, match="Workbook.open"):
        Workbook(path)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="binary mode"):
        Workbook(io.StringIO("PK"))  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="bytes or a binary file object"):
        Workbook(42)  # type: ignore[arg-type]
    with pytest.raises(TabularError) as refused:
        Workbook(io.BytesIO(b"not a zip at all"))
    assert refused.value.kind is TabularFailure.NOT_A_ZIP


# --- asyncio ------------------------------------------------------------------------------------


def _stream(data: bytes) -> asyncio.StreamReader:
    stream = asyncio.StreamReader()
    stream.feed_data(data)
    stream.feed_eof()
    return stream


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide's loop cannot be run from a test")
def test_an_asyncio_stream_reader_is_read():
    """asyncio.StreamReader is the stream the async surface is shaped for."""
    data = b"id,name\n" + b"".join(b"%d,n%d\n" % (index, index) for index in range(50))

    async def main() -> list[int]:
        reader = await DelimitedReader.open_async(
            _stream(data), Dialect.CSV, batch_rows=8, buffer_bytes=16
        )
        assert reader.header == ("id", "name")
        reader.bind([Column.i32(reader.header.ordinal("id"))])
        ids: list[int] = []
        async for batch in reader:
            ids.extend(batch.column(0).values.tolist())
        assert await reader.read_async() is None
        return ids

    assert _run(main()) == list(range(50))


def test_a_reader_over_an_async_stream_is_not_read_synchronously():
    """read() on a reader that has to await its stream says what to call instead."""

    async def main() -> None:
        reader = await DelimitedReader.open_async(
            _AsyncDribble(b"a\n1\n"), Dialect.CSV, [Column.i32(0)]
        )
        with pytest.raises(RuntimeError, match="read_async"):
            reader.read()
        # Nothing was lost by asking.
        batch = await reader.read_async()
        assert batch is not None and batch.column(0).values.tolist() == [1]

    _run(main())


def test_an_in_memory_reader_reads_asynchronously_without_waiting():
    """read_async and async for work over bytes and file objects, completing at once."""

    async def main() -> None:
        for source in (b"a\n1\n2\n", io.BytesIO(b"a\n1\n2\n")):
            reader = DelimitedReader(source, Dialect.CSV, [Column.i32(0)], batch_rows=1)
            assert [batch.column(0).values.tolist() async for batch in reader] == [[1], [2]]
        opened = await DelimitedReader.open_async(b"a\n1\n", Dialect.CSV, [Column.i32(0)])
        assert opened.header == ("a",)

    _run(main())


@pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide's loop cannot be run from a test")
def test_a_cancelled_read_leaves_the_reader_resumable():
    """A read cancelled while it awaits the stream fed the reader nothing; the next goes on."""
    data = b"n\n" + b"".join(b"%d\n" % index for index in range(20))

    async def main() -> list[int]:
        stream = asyncio.StreamReader()
        # The header and the start of the first record: the read has to wait for the rest.
        stream.feed_data(data[:3])
        reader = await DelimitedReader.open_async(
            stream, Dialect.CSV, [Column.i32(0)], batch_rows=100, buffer_bytes=4
        )
        pending = asyncio.ensure_future(reader.read_async())
        for _ in range(20):
            await asyncio.sleep(0)
        assert not pending.done(), "the read should be waiting on the stream"
        pending.cancel()
        with pytest.raises(asyncio.CancelledError):
            await pending
        stream.feed_data(data[3:])
        stream.feed_eof()
        values: list[int] = []
        while (batch := await reader.read_async()) is not None:
            values.extend(batch.column(0).values.tolist())
        return values

    assert _run(main()) == list(range(20))


def test_a_structural_failure_is_raised_asynchronously_and_again():
    """A broken record raises TabularError from read_async, after the intact rows, and again."""

    async def main() -> None:
        reader = await DelimitedReader.open_async(
            _AsyncDribble(b"a,b\n1,2\n3\n"), Dialect.CSV, [Column.i32(0)]
        )
        batch = await reader.read_async()
        assert batch is not None and batch.rows == 1
        with pytest.raises(TabularError) as first:
            await reader.read_async()
        with pytest.raises(TabularError) as again:
            await reader.read_async()
        assert first.value is again.value
        assert first.value.kind is TabularFailure.COLUMN_COUNT

    _run(main())


def test_what_an_async_stream_returns_is_checked():
    """A stream that returns str, or more than it was asked for, is a caller's bug."""

    class Text:
        async def read(self, size: int) -> str:
            return "a\n1\n"

    class Greedy:
        async def read(self, size: int) -> bytes:
            return b"x" * (size + 1)

    async def main() -> None:
        with pytest.raises(TypeError, match="bytes"):
            await DelimitedReader.open_async(Text(), Dialect.CSV)  # type: ignore[arg-type]
        with pytest.raises(ValueError, match="more than size"):
            await DelimitedReader.open_async(Greedy(), Dialect.CSV)
        with pytest.raises(TypeError, match="async read"):
            await DelimitedReader.open_async(42, Dialect.CSV)  # type: ignore[arg-type]
        reader = DelimitedReader(b"a\n", Dialect.CSV)
        with pytest.raises(RuntimeError, match="asynchronous stream"):
            reader._feed(b"")

    _run(main())


def test_a_workbook_from_an_async_stream():
    """Workbook.open_async reads the stream to its end, a few bytes a read."""
    path = _corpus_dir() / "workbook" / "basic.xlsx"
    book = _run(Workbook.open_async(_AsyncDribble(path.read_bytes())))
    assert book.sheets == Workbook.open(path).sheets

    if sys.platform == "emscripten":
        return  # an asyncio.StreamReader needs a running loop, which Pyodide's test cannot start

    async def from_a_stream_reader() -> Workbook:
        return await Workbook.open_async(_stream(path.read_bytes()))

    assert _run(from_a_stream_reader()).format is book.format
