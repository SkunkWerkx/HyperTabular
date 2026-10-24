# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```
pip install hypertabular
```

```python
from hypertabular import Column, DelimitedReader, Dialect, Fault, SheetOptions, Success, Workbook

# The header first; then a plan that reads the columns by name, wherever the file put them.
with DelimitedReader.open("orders.csv", Dialect.CSV) as reader:
    header = reader.header                    # Header(('id', 'name', 'score'))
    plan = [
        Column.i32(header.ordinal("id")),     # KeyError naming the column if it is missing
        Column.text(header.ordinal("name")),
        Column.f64(header.ordinal("score")),
    ]
    reader.bind(plan)
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

# A workbook reads into the same batch, its sheet header first or with the plan up front.
book = Workbook.open("orders.xlsx")
print(book.sheets)                            # (SheetInfo(name='Orders', hidden=False),)
sheet = book.sheet("Orders", SheetOptions())
sheet.bind([Column.i32(sheet.header.ordinal("id")), Column.text(sheet.header.ordinal("name"))])
for batch in sheet:
    ...
```

A plan known up front goes straight to the constructor — `DelimitedReader.open(path, dialect,
plan)`, `book.sheet(which, options, plan)` — which binds it before the header is read.

## Header first

A reader made without a plan — `DelimitedReader(source, dialect)`, `DelimitedReader.open(path,
dialect)`, `book.sheet(which, options)` — reads the header and waits: `bind(plan)` declares the
plan once, before the first read. A read before it raises `RuntimeError` (and a later `bind`
puts that right); so does a second `bind`. `is_bound` says which it is, and a delimited
reader's `column_count` is the header's width, or the first record's once it has been read.

The header is a `Header`: the tuple of `str` it always was, plus `ordinal(name)` — the first
column with exactly that name, case and spaces included, or a `KeyError` naming it — and
`get(name, default=None)`, which returns `default` instead. A `bytes` name is matched against
the bytes the file holds.

## Rows

A batch is column-major; a `Row` reads it across, for code that builds an object a row at a
time. `for row in batch` (or `batch.iter_rows()`, `batch.row(i)`) gives views — the batch and
an index, nothing copied — with `index`, `line`, `get(column)` (the cell as `Success` or
`Fault`), `text(column)` (a text cell's `str`, or `None`) and `raw(column)`. `reader.rows()`
and `sheet.rows()` run across batches, reading the next as one runs out. A batch owns what it
shows, so a row stays valid after the reader has moved on.

```python
with DelimitedReader.open("orders.csv", Dialect.CSV, plan) as reader:
    orders = [
        Order(id.value, row.text(1))
        for row in reader.rows()
        if isinstance(id := row.get(0), Success)
    ]
```

## Streams and asyncio

`DelimitedReader(source, …)` takes `bytes` or a binary file object (anything with
`read(size)`), and so does `Workbook(data)`: a workbook is read to the end into memory it owns,
since a zip is read from its directory at the end back. Either way the format is read from the
bytes. A file object is the caller's to close.

An asynchronous stream — an `asyncio.StreamReader`, or anything with `async read(size)` — is
read with asyncio:

```python
reader = await DelimitedReader.open_async(stream, Dialect.CSV)   # the header is awaited too
reader.bind([Column.i64(reader.header.ordinal("id"))])
async for batch in reader:                                       # or: await reader.read_async()
    ...

book = await Workbook.open_async(stream)
```

The core never waits: when a read needs input it says how much, the stream is awaited for that
much and the read goes on, all the core's work done between awaits with the interpreter
released. Cancellation is asyncio's, and a read cancelled while it awaits the stream leaves the
reader where it was — the next read carries on. `read_async` and `async for` work on any
reader; over `bytes` or a blocking file object they complete without awaiting anything.

## The shape

The native core owns no memory and reads no files. It is linked straight into a CPython
extension module (`hypertabular._native`, PyO3), and the extension is the binding: it
allocates a value array and a verdict array per column, once, and the core fills them in one
call per batch with the interpreter released. A column reaches Python whole — a read-only
`memoryview` of the door's own item type, which anything that takes a buffer takes — with no
Python call per cell.

- **One batch type.** Iterating a reader or a sheet (or calling `read()`) gives a `Batch`:
  `rows`, `columns`, `column(i)`, `line(row)`, `raw(column, row)`, `get(column, row)` and
  `text(column, row)`, and its rows when iterated. Each column is a
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

Each platform gets three wheels, which between them cover every CPython from the 3.11 floor
up, with the GIL or without it, so nothing is compiled on install:

| Wheel | Interpreters |
| --- | --- |
| `cp311-abi3` | CPython 3.11 and newer, with the GIL |
| `cp314-cp314t` | free-threaded CPython 3.14 |
| `cp315-abi3.abi3t` | CPython 3.15 and newer, free-threaded or not ([PEP 803](https://peps.python.org/pep-0803/)) |

The platforms are Linux (glibc and musl) on x86-64 and arm64, macOS on Apple silicon and
Intel, and Windows on x86-64 and arm64: 24 wheels, and a twenty-fifth for Pyodide
([below](#in-the-browser-pyodide)). There is no source distribution, so an interpreter none
of them matches (free-threaded 3.13, PyPy) gets `No matching distribution found` from pip.

**Free-threaded CPython runs it without the GIL.** The extension declares it needs no GIL, and
so does HyperCast's, so a free-threaded interpreter keeps the GIL off after
`import hypertabular` (no `RuntimeWarning`), and readers on different threads run truly in
parallel. A `Batch`, its columns and rows, and a `Workbook` are immutable once made and can be
shared by any number of threads; a column's `values` is built once, whichever thread asks
first. A `DelimitedReader` or a `Sheet` is read by one thread at a time, as everywhere: a
second thread calling into one while another is inside a read gets `RuntimeError` (PyO3's
"Already borrowed") and nothing else, never a race on its buffers. The bindings are written
once, for free-threading; a GIL build runs the same code. CI runs the whole suite on
free-threaded 3.15 on every platform, and installs each wheel on its own interpreters before
it is published.

## In the browser (Pyodide)

The same PyO3 extension, compiled for Pyodide's Emscripten target, is published to PyPI as a
twenty-fifth wheel, `hypertabular-<version>-cp311-abi3-pyemscripten_2026_0_wasm32.whl`, so micropip
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

Every wheel PyPI serves, the Pyodide one included, verifies from the release tag alone: the
release attestation, signed by this repository's `release.yml` at the tag, names every one of
them, with the release inventory as its predicate.

```sh
pip download hypertabular==<version> --no-deps -d .
gh attestation verify hypertabular-<version>-*.whl --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Each wheel also carries build provenance from where it was built, installed and attested:
`hyper-build-wheels.yml` in `SkunkWerkx/.github`, at the preparation commit on master the
release was prepared from (`release.yml` verifies each wheel against it before publishing it
unchanged). That check names the signer, or uses `--owner` for both:

```sh
gh attestation verify hypertabular-<version>-*.whl \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
# or: gh attestation verify hypertabular-<version>-*.whl --owner SkunkWerkx
```

Both are the GitHub/Sigstore transparency-log route, checked with `gh attestation verify`, the
same route every other artifact in this project uses. Separately, PyPI itself holds a
[PEP 740](https://peps.python.org/pep-0740/) publish attestation for every wheel, filed by
`gh-action-pypi-publish` at publication and shown on each file's PyPI page, which PyPI-side
tooling checks on its own. The root README's [Provenance](../README.md#provenance) section has
the whole picture.

## License

[MIT](LICENSE)
