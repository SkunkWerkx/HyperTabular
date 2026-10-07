# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```
pip install hypertabular
```

```python
from hypertabular import Column, DelimitedReader, Dialect, Fault, SheetOptions, Success, Workbook

plan = [Column.i32(0), Column.text(1), Column.f64(2)]
with DelimitedReader.open("orders.csv", Dialect.CSV, plan) as reader:
    print(reader.header)                      # ('id', 'name', 'score')
    for batch in reader:
        ids, names, scores = batch.columns

        # A column at a time, as the core wrote it…
        total = sum(scores.values)            # a memoryview of C doubles
        for row, fault in scores.faults():
            print(row, fault.reason.name, scores.raw(row))

        # …or a cell at a time, as HyperCast's union.
        for row in range(batch.rows):
            match scores[row]:
                case Success(value):
                    print(names.values[row], value)
                case Fault(reason, offset, length):
                    print(reason.name, "on line", batch.line(row), "in", scores.raw(row))

# A workbook reads into the same batch.
book = Workbook.open("orders.xlsx")
print(book.sheets)                            # (SheetInfo(name='Orders', hidden=False),)
for batch in book.sheet("Orders", SheetOptions(), plan):
    ...
```

## The shape

The native core owns no memory and reads no files. It is linked straight into a CPython
extension module (`hypertabular._native`, PyO3), and the extension is the binding: it
allocates a value array and a verdict array per column, once, and the core fills them in one
call per batch with the interpreter released. A column reaches Python whole — a read-only
`memoryview` of the door's own item type, which anything that takes a buffer takes — with no
Python call per cell.

- **One batch type.** Iterating a reader or a sheet (or calling `read()`) gives a `Batch`:
  `rows`, `columns`, `column(i)`, `line(row)` and `raw(column, row)`. Each column is a
  `ColumnData` with `values`, `verdicts`, `fault_count`, `faults()`, `raw(row)` and
  `column[row]` for one cell as `Success` or `Fault`. A batch owns what it shows: it stays
  valid after the reader has moved on.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size;
  the plan states each column's door and, for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Success`, `Fault`, `CastFailure`, `NumFormat`,
  `UnixPrecision`, `DateOrder` and `ExcelEpoch` are the `hypercast` package's own objects,
  re-exported here unchanged. A text cell means exactly what `hypercast.cast_*` says of the
  same text, a typed workbook cell is converted by the door directly, and its value is the
  Python type that door gives: `int`, `float`, `bool`, `decimal.Decimal`, `uuid.UUID`,
  `datetime`, `date`, `time`, `timedelta`, and `str` for text.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast is
  a `Fault` in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, raises
  `TabularError` — after every intact row before it has been delivered.

`hypertabular.native_version()` reports the core the extension was built from.

## Platforms

One abi3 wheel per platform serves every CPython from 3.11 up, so nothing is compiled on
install: Linux (glibc and musl) on x86-64 and arm64, macOS on Apple silicon and Intel, and
Windows on x86-64 and arm64. The free-threaded build (`python3.14t`) is not one of them: an
abi3 wheel serves only the GIL build, and there is no source distribution to compile.

## In the browser (Pyodide)

The same PyO3 extension, compiled for Pyodide's Emscripten target, is published to PyPI as a
ninth wheel, `hypertabular-X.Y.Z-cp311-abi3-pyemscripten_2026_0_wasm32.whl`, so micropip
finds it the way pip finds the native wheels, and HyperCast's beside it:

```python
import micropip
await micropip.install("hypertabular")

import hypertabular
hypertabular.BACKEND            # "native"
hypertabular.native_version()   # the linked core's version
```

It is the same native extension, with the same API and no JavaScript bridge. CI installs the
wheel into Pyodide and runs this package's whole pytest suite in it twice, under Node and in
headless Chrome, corpus replay included; the one test that starts threads is skipped, since
Pyodide cannot. The wheel is for **Pyodide 314.x** (Python 3.14, platform
`pyemscripten_2026_0`): each Pyodide ABI year needs its own wheel.

## Verifying provenance

Every wheel is built and attested by the SkunkWerkx forge's reusable workflow:

```
gh attestation verify hypertabular-<version>-*.whl --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
```

## License

[MIT](LICENSE)
