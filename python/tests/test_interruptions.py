"""A read cut short: by a stream that breaks its contract, by Ctrl-C while a file the reader
opened waits for data, or by a close from inside the stream it awaits. Every row reaches the
caller once, or every later read raises, saying why.
"""

from __future__ import annotations

import contextlib
import os
import signal
import sys
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from hypertabular import Column, DelimitedReader, Dialect, Workbook

from test_corpus import _run

ROWS = 50
DATA = b"a,b\n" + b"".join(b"%d,r%d\n" % (row, row) for row in range(ROWS))
PLAN = [Column.i32(0), Column.text(1)]


def _ints(reader: DelimitedReader) -> list[int]:
    return [value for batch in reader for value in batch.column(0).values]


class _Breaking:
    """Hands out DATA seven bytes a read, and on read ``at`` hands over what ``bad`` makes instead."""

    def __init__(self, at: int, bad: Callable[[int], Any]) -> None:
        self.at = at
        self.bad = bad
        self.reads = 0
        self.position = 0

    def take(self, size: int) -> Any:
        self.reads += 1
        if self.reads == self.at:
            return self.bad(size)
        chunk = DATA[self.position : self.position + min(size, 7)]
        self.position += len(chunk)
        return chunk


class _SyncBreaking(_Breaking):
    def read(self, size: int) -> Any:
        return self.take(size)


class _AsyncBreaking(_Breaking):
    async def read(self, size: int) -> Any:
        return self.take(size)


BROKEN = [
    pytest.param(
        lambda size: b"9" * (size + 1), ValueError, "more than size", id="more-than-asked"
    ),
    pytest.param(lambda size: "x", TypeError, "must return bytes", id="str"),
]


@pytest.mark.parametrize(("bad", "kind", "says"), BROKEN)
def test_a_chunk_a_file_object_hands_over_that_cannot_be_read_is_final(
    bad: Callable[[int], Any], kind: type[Exception], says: str
) -> None:
    """What the stream handed over is gone from it, so the read can't go on as if it weren't."""
    reader = DelimitedReader(_SyncBreaking(6, bad), Dialect.CSV, PLAN, batch_rows=2)
    seen: list[int] = []
    with pytest.raises(kind, match=says) as first:
        while (batch := reader.read()) is not None:
            seen.extend(batch.column(0).values)
    assert seen == list(range(len(seen)))
    for _ in range(2):
        with pytest.raises(kind) as again:
            reader.read()
        assert again.value is first.value
    with pytest.raises(kind):
        next(iter(reader))


@pytest.mark.parametrize(("bad", "kind", "says"), BROKEN)
def test_a_chunk_an_async_stream_hands_over_that_cannot_be_read_is_final(
    bad: Callable[[int], Any], kind: type[Exception], says: str
) -> None:
    """The same of an asynchronous stream: nothing is fed, and nothing reads on past it."""

    async def main() -> None:
        reader = await DelimitedReader.open_async(
            _AsyncBreaking(6, bad), Dialect.CSV, PLAN, batch_rows=2
        )
        seen: list[int] = []
        with pytest.raises(kind, match=says) as first:
            while (batch := await reader.read_async()) is not None:
                seen.extend(batch.column(0).values)
        assert seen == list(range(len(seen)))
        for _ in range(2):
            with pytest.raises(kind) as again:
                await reader.read_async()
            assert again.value is first.value

    _run(main())


def test_a_close_from_inside_an_async_stream_s_read_says_so() -> None:
    """The read that was awaiting the stream says the reader was closed under it."""

    class Closing:
        def __init__(self) -> None:
            self.reader: DelimitedReader | None = None
            self.position = 0

        async def read(self, size: int) -> bytes:
            if self.reader is not None and self.position > 20:
                self.reader.close()
            chunk = DATA[self.position : self.position + min(size, 7)]
            self.position += len(chunk)
            return chunk

    async def main() -> None:
        stream = Closing()
        reader = await DelimitedReader.open_async(stream, Dialect.CSV, PLAN, batch_rows=2)
        stream.reader = reader
        with pytest.raises(ValueError, match="closed while the read awaited its stream"):
            while await reader.read_async() is not None:
                pass
        with pytest.raises(ValueError, match="read of a closed DelimitedReader"):
            await reader.read_async()

    _run(main())


BUSY = "the reader is in the middle of a call; nothing that call runs can call back into it"


def test_a_stream_calling_back_into_its_reader_is_refused_and_the_read_goes_on() -> None:
    """A call into the reader from inside its own stream's read raises ``RuntimeError`` saying
    so, the words every binding uses, and the read that called the stream carries on whole."""

    class Calling:
        def __init__(self) -> None:
            self.reader: DelimitedReader | None = None
            self.position = 0
            self.refused: list[str] = []

        def read(self, size: int) -> bytes:
            if self.reader is not None:
                for call in (self.reader.read, lambda: next(self.reader), self.reader.close):
                    try:
                        call()
                    except RuntimeError as error:
                        self.refused.append(str(error))
            chunk = DATA[self.position : self.position + min(size, 7)]
            self.position += len(chunk)
            return chunk

    stream = Calling()
    reader = DelimitedReader(stream, Dialect.CSV, PLAN, batch_rows=4)
    stream.reader = reader
    assert _ints(reader) == list(range(ROWS))
    assert stream.refused, "the stream never got to call back"
    assert set(stream.refused) == {BUSY}


# --- Ctrl-C while a file the reader opened waits ------------------------------------------------

needs_fifo = pytest.mark.skipif(
    not hasattr(os, "mkfifo") or sys.platform == "emscripten",
    reason="no named pipes, or no thread to write one",
)
# Under PyPy a signal doesn't cut a blocked open or read short, so Ctrl-C lands only when the writer
# writes (8.00 s in CI): the binding has no interrupted call to run the handler from.
cuts_a_blocked_call_short = pytest.mark.skipif(
    sys.implementation.name == "pypy",
    reason="PyPy doesn't cut a blocked open or read short on a signal",
)


def _interrupted_within(seconds: float, call: Callable[[], Any]) -> float:
    """How long ``call`` took to raise KeyboardInterrupt, from a real SIGINT sent after 0.3 s."""
    timer = threading.Timer(0.3, os.kill, (os.getpid(), signal.SIGINT))
    started = time.monotonic()
    timer.start()
    try:
        with pytest.raises(KeyboardInterrupt):
            call()
    finally:
        timer.cancel()
    took = time.monotonic() - started
    assert took < seconds, f"Ctrl-C took {took:.2f} s to reach the read"
    return took


class _Writer:
    """The FIFO's writer, on a thread: opens it (after ``open_after`` seconds, or once let go),
    writes ``first``, waits until let go (or eight seconds), and writes ``rest``.
    """

    def __init__(self, fifo: Path, first: bytes, rest: bytes, open_after: float = 0.0) -> None:
        self.go = threading.Event()
        self.thread = threading.Thread(
            target=self.run, args=(fifo, first, rest, open_after), daemon=True
        )
        self.thread.start()

    def run(self, fifo: Path, first: bytes, rest: bytes, open_after: float) -> None:
        if open_after:
            self.go.wait(open_after)
        # A reader the interrupt ended has closed its end, and what is left has nowhere to go.
        with contextlib.suppress(BrokenPipeError), open(fifo, "wb", 0) as sink:
            sink.write(first)
            self.go.wait(8)
            sink.write(rest)

    def finish(self) -> None:
        self.go.set()
        self.thread.join(20)


@needs_fifo
@cuts_a_blocked_call_short
def test_ctrl_c_reaches_a_read_of_a_path_waiting_on_its_writer_and_the_read_goes_on(
    tmp_path: Path,
) -> None:
    """A read of a file the reader opened is cut short as CPython's own is, losing nothing."""
    fifo = tmp_path / "stalled.csv"
    os.mkfifo(fifo)
    writer = _Writer(fifo, b"a\n1\n2\n", b"3\n4\n")
    try:
        reader = DelimitedReader.open(fifo, Dialect.CSV, [Column.i32(0)], batch_rows=2)
        first = reader.read()
        assert first is not None
        assert list(first.column(0).values) == [1, 2]
        _interrupted_within(3.0, reader.read)
        writer.finish()
        assert _ints(reader) == [3, 4]
    finally:
        writer.finish()


@needs_fifo
@cuts_a_blocked_call_short
def test_ctrl_c_reaches_an_open_waiting_on_a_fifo_s_writer(tmp_path: Path) -> None:
    """Opening a FIFO waits for its writer, and Ctrl-C ends the wait; the next open goes on."""
    fifo = tmp_path / "unwritten.csv"
    os.mkfifo(fifo)
    writer = _Writer(fifo, b"a\n1\n", b"", open_after=8)
    try:
        _interrupted_within(3.0, lambda: DelimitedReader.open(fifo, Dialect.CSV))
        writer.go.set()
        reader = DelimitedReader.open(fifo, Dialect.CSV, [Column.i32(0)])
        assert _ints(reader) == [1]
    finally:
        writer.finish()


@needs_fifo
@cuts_a_blocked_call_short
@pytest.mark.parametrize(
    "call",
    [
        pytest.param(lambda fifo: DelimitedReader.suggest(fifo, Dialect.CSV), id="suggest"),
        pytest.param(lambda fifo: Workbook.open(fifo), id="workbook"),
    ],
)
def test_ctrl_c_reaches_a_whole_read_of_a_path_waiting_on_its_writer(
    tmp_path: Path, call: Callable[[Path], Any]
) -> None:
    """A suggestion's sample and a workbook read through a file, and Ctrl-C ends either."""
    fifo = tmp_path / "stalled"
    os.mkfifo(fifo)
    writer = _Writer(fifo, b"a,b\n1,x\n", b"2,y\n")
    try:
        _interrupted_within(3.0, lambda: call(fifo))
    finally:
        writer.finish()
