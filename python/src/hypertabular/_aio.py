"""The waiting an asynchronous read does, on the extension's behalf.

The core never waits, and the extension never calls an asynchronous stream: when a reader
over one needs more input, a step of its read ends by saying how much it has room for. The
coroutines here await that much from the stream's ``read(size)``, feed it in, and take the
next step — so everything the core does happens between awaits, with the interpreter
released, and the event loop is never blocked on the stream. ``DelimitedReader.read_async``,
``open_async``, ``async for`` and ``Workbook.open_async`` hand their work to these.

Cancellation is asyncio's own. A step that wants input has already made the room it asks
for and changes nothing more until it is fed, so a read cancelled while it awaits the stream
leaves the reader where it was, and the next read asks for the same and goes on.
"""

from __future__ import annotations

from collections.abc import Awaitable, Iterable
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from . import Column, Dialect
    from ._native import Batch, DelimitedReader, Workbook


class AsyncReadable(Protocol):
    """What an asynchronous stream has to offer: ``asyncio.StreamReader``'s ``read``."""

    def read(self, size: int, /) -> Awaitable[bytes]:
        """Up to ``size`` bytes; ``b""`` at the end of the stream."""
        ...


async def read(reader: DelimitedReader, stream: AsyncReadable | None) -> Batch | None:
    """The next batch of ``reader``, awaiting ``stream`` for whatever input it asks for."""
    while True:
        step = reader._advance()
        if not isinstance(step, int):
            return step
        assert stream is not None, "only a reader over a stream asks to be fed"
        reader._feed(await stream.read(step))


async def next_batch(pending: Awaitable[Batch | None]) -> Batch:
    """The next batch for ``async for``, which ends where ``read_async`` returns ``None``."""
    batch = await pending
    if batch is None:
        raise StopAsyncIteration
    return batch


async def open_reader(
    stream: AsyncReadable | bytes,
    dialect: Dialect,
    plan: Iterable[Column] | None,
    batch_rows: int,
    buffer_bytes: int,
) -> DelimitedReader:
    """A reader over ``stream``, its header read: ``DelimitedReader.open_async``."""
    from ._native import DelimitedReader

    reader = DelimitedReader._over_async(stream, dialect, plan, batch_rows, buffer_bytes)
    while (wanted := reader._header()) is not None:
        assert not isinstance(stream, bytes)
        reader._feed(await stream.read(wanted))
    return reader


async def workbook(stream: AsyncReadable, chunk: int) -> Workbook:
    """The workbook ``stream`` holds, read to its end: ``Workbook.open_async``."""
    from ._native import Workbook

    chunks = []
    while part := await stream.read(chunk):
        chunks.append(part)
    return Workbook._from_chunks(chunks)
