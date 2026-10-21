"""Many threads at once: on a free-threaded CPython they really run in parallel, so these are
where a GIL assumption in the extension would show. A GIL build runs them too, serialized."""

from __future__ import annotations

import io
import os
import sys
import sysconfig
import threading
import time

import pytest
from test_corpus import _corpus_dir

from hypertabular import Column, DelimitedReader, Dialect, SheetOptions, Workbook

pytestmark = pytest.mark.skipif(sys.platform == "emscripten", reason="Pyodide has no threads")

THREADS = 8


def _run_together(work) -> list:
    """Runs ``work(i)`` on THREADS threads released at once, and returns what each returned."""
    results: list = [None] * THREADS
    errors: list = []
    barrier = threading.Barrier(THREADS)

    def run(i: int) -> None:
        barrier.wait()
        try:
            results[i] = work(i)
        except BaseException as error:  # noqa: BLE001 - re-raised on the main thread below
            errors.append(error)

    workers = [threading.Thread(target=run, args=(i,)) for i in range(THREADS)]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    if errors:
        raise errors[0]
    return results


def _rows(count: int) -> bytes:
    lines = ["id,name,score,when"]
    for row in range(count):
        name = f'"name {row}, ""quoted"""' if row % 7 == 0 else f"name {row}"
        score = "oops" if row % 11 == 0 else f"{row / 4}"
        lines.append(f"{row},{name},{score},2026-10-{row % 28 + 1:02}")
    return ("\n".join(lines) + "\n").encode()


PLAN = [Column.i32(0), Column.text(1), Column.f64(2), Column.date(3), Column.decimal(2)]


def _cells(batch) -> list[tuple]:
    """Everything a batch shows, cell by cell, in a form that compares."""
    return [
        tuple((repr(batch.get(column, row)), batch.raw(column, row)) for column in range(len(PLAN)))
        for row in range(batch.rows)
    ]


@pytest.mark.skipif(
    not sysconfig.get_config_var("Py_GIL_DISABLED"), reason="needs a free-threaded CPython"
)
@pytest.mark.skipif("PYTHON_GIL" in os.environ, reason="PYTHON_GIL overrides the module's choice")
def test_import_leaves_the_gil_off():
    """The extension declares it needs no GIL, so importing it doesn't switch the GIL back on
    — nor does the hypercast extension it imports."""
    assert not sys._is_gil_enabled()


def test_threads_sharing_one_batch_all_see_the_same_cells_and_the_same_views():
    """A batch is handed out to be shared, so many threads read one at once: each sees every
    cell exactly as one thread does, and a column's lazily built ``values`` and ``verdicts``
    are built once — every thread gets the very same object."""
    (batch,) = list(DelimitedReader(_rows(500), Dialect.CSV, PLAN, batch_rows=1000))
    expected = _cells(batch)

    def work(i: int) -> tuple:
        views = [(id(c.values), id(c.verdicts)) for c in batch.columns]
        rows = [tuple(row.get(c) for c in range(len(PLAN))) for row in batch]
        return _cells(batch), views, len(rows)

    results = _run_together(work)
    for cells, views, rows in results:
        assert cells == expected
        assert views == results[0][1]
        assert rows == batch.rows


def test_one_reader_shared_by_threads_hands_out_every_row_exactly_once():
    """A reader is read by one thread at a time. Threads that share one anyway are refused
    with ``RuntimeError`` while another is inside a read, never let in to race on its
    buffers: between them they read every row exactly once."""
    count = 20_000
    for source in ("memory", "stream"):
        data = _rows(count)
        reader = (
            DelimitedReader(data, Dialect.CSV, PLAN, batch_rows=37)
            if source == "memory"
            else DelimitedReader(
                io.BytesIO(data), Dialect.CSV, PLAN, batch_rows=37, buffer_bytes=999
            )
        )

        def work(i: int) -> tuple[list[int], int]:
            ids: list[int] = []
            refused = 0
            while True:
                try:
                    batch = reader.read()
                except RuntimeError as error:
                    assert "borrow" in str(error), error
                    refused += 1
                    # Lets a GIL build's reading thread have the GIL back rather than wait
                    # out a switch interval behind this loop; free-threaded, it is a yield.
                    time.sleep(0)
                    continue
                if batch is None:
                    return ids, refused
                ids.extend(batch.column(0).values)

        results = _run_together(work)
        everything = sorted(row for ids, _ in results for row in ids)
        assert everything == list(range(count)), source


def test_many_sheets_of_one_shared_workbook_read_at_once_agree():
    """A workbook is immutable once open and every sheet of it has buffers of its own, so
    threads each opening and reading their own sheet of one shared workbook all get what one
    thread gets."""
    book = Workbook.open(_corpus_dir() / "workbook" / "basic.xlsx")
    options = SheetOptions()

    def read() -> list:
        sheet = book.sheet(0, options)
        width = len(sheet.header)
        sheet.bind([Column.text(c) for c in range(width)] + [Column.f64(c) for c in range(width)])
        return [[repr(cell) for column in batch.columns for cell in column] for batch in sheet]

    expected = read()
    assert expected
    for got in _run_together(lambda i: [read() for _ in range(50)]):
        assert all(one == expected for one in got)
