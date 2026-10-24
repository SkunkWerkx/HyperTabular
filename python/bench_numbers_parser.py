"""numbers-parser over numbers-300k.numbers: the row beside this binding's in docs/workbook.md.

Run with: pip install numbers-parser; python bench_numbers_parser.py [--once]

The work bench_workbook.py's "values" scope does on the same file, done by the Python reader
every other reader of .numbers is held against. "open" is the document opened and nothing
read. "read" is opened, then the first table's rows after the header iterated and every
cell's value made, the cells that have one counted and the text column's lengths summed: the
checksum every binding's benchmark prints. numbers-parser decodes the whole document at open
and the read walks what the open built, so the open is most of the cost. An error cell has no
value in numbers-parser as it has no cast in the core, and an empty cell is None, so the
checksum over the README's file is the same 5 285 882.

The file is read from corpus/generate/out/, or HYPERTABULAR_BENCH_DIR; a file that is not
there is said so. Best of five runs of each, or one with --once.
"""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path

from numbers_parser import Document  # type: ignore[import-not-found]

FILE = "numbers-300k.numbers"
TEXT_COLUMN = 2


def read(path: Path) -> int:
    """Opens the document and iterates the first table's rows, every value made."""
    table = Document(path).sheets[0].tables[0]
    checksum = 0
    for row in table.iter_rows(min_row=1, values_only=True):
        checksum += sum(1 for value in row if value is not None)
        text = row[TEXT_COLUMN]
        if isinstance(text, str):
            checksum += len(text)
    return checksum


def best_of(runs: int, work: object) -> float:
    """The shortest of ``runs`` timings of ``work``, in seconds."""
    best = float("inf")
    for _ in range(runs):
        start = time.perf_counter()
        work()  # type: ignore[operator]
        best = min(best, time.perf_counter() - start)
    return best


def main() -> None:
    """Times the open and the read, and prints the checksum."""
    directory = Path(
        os.environ.get("HYPERTABULAR_BENCH_DIR")
        or Path(__file__).resolve().parent.parent / "corpus" / "generate" / "out"
    )
    path = directory / FILE
    if not path.exists():
        print(f"{FILE}: not in {directory}")
        return
    runs = 1 if "--once" in sys.argv else 5
    print(f"{FILE}: checksum {read(path)}")
    opened = best_of(runs, lambda: len(Document(path).sheets))
    print(f"{FILE} open: {opened:.2f} s (best of {runs})")
    print(f"{FILE} read: {best_of(runs, lambda: read(path)):.2f} s (best of {runs})")


if __name__ == "__main__":
    main()
