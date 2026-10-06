"""pyperf benchmarks: the workbook reader over the two 300 000-row files of corpus/README.md.

Run with: python bench_workbook.py --fast -q

Excel's excel-win-300k.xlsx and LibreOffice's libreoffice-300k.ods, 300 000 rows x 8 columns
each, read whole through the plan rust/benches/workbook_benchmarks.rs reads them through, so
that this binding's number sits beside the core's. "open" is the package opened and nothing
read. "read" is opened, then the first sheet read in batches of 4096, every column's faults
counted and the text column's values made into strs: the checksum every binding's benchmark
arrives at, which says they all did the same work. "values" is "read" with every column's
values made too, the dates, times and timedeltas included: what a caller that wants Python
objects for the whole sheet pays.

The files stay out of the tree (corpus/generate/out/, sha256 in the README's table);
HYPERTABULAR_BENCH_DIR names another directory holding them. A file that is not there is
skipped.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pyperf

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))

from hypertabular import Column, SheetOptions, Workbook  # noqa: E402

PLAN = [
    Column.i64(0),
    Column.f64(1),
    Column.text(2),
    Column.date(3),
    Column.time(4),
    Column.bool(5),
    Column.duration(6),
    Column.i64(7),
]
OPTIONS = SheetOptions()
FILES = ["excel-win-300k.xlsx", "libreoffice-300k.ods"]


def read(container: bytes, every: bool) -> int:
    """Reads the first sheet whole; ``every`` makes every column's values, not only the text's."""
    checksum = 0
    for batch in Workbook(container).sheet(0, OPTIONS, PLAN):
        for column in batch.columns:
            checksum += len(column) - column.fault_count
            if every:
                column.values  # noqa: B018 - made for its cost
        for text in batch.column(2).values:
            if text is not None:
                checksum += len(text)
    return checksum


def main() -> None:
    """Registers a benchmark per file and scope."""
    runner = pyperf.Runner()
    directory = Path(
        os.environ.get("HYPERTABULAR_BENCH_DIR")
        or Path(__file__).resolve().parent.parent / "corpus" / "generate" / "out"
    )
    for name in FILES:
        path = directory / name
        if not path.exists():
            print(f"{name}: not in {directory}, skipped")
            continue
        container = path.read_bytes()
        # Once, in the parent: pyperf's workers run this script again with --worker.
        if "--worker" not in sys.argv:
            print(f"{name}: checksum {read(container, False)}")
        runner.bench_func(f"{name} open", lambda c=container: len(Workbook(c).sheets))
        runner.bench_func(f"{name} read", read, container, False)
        runner.bench_func(f"{name} values", read, container, True)


if __name__ == "__main__":
    main()
