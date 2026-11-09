# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```
pip install hypertabular
```

The examples read this file, saved as `people.csv`; the second row's score does not cast,
so the fault path has something to print:

```csv
id,name,score
1,alice,2.5
2,bob,oops
3,carol,4.75
```

and `people.xlsx`, a workbook with one sheet named `People` holding the same table, header
row first (open `people.csv` in a spreadsheet app, rename the sheet and save it as `.xlsx`).

```python
from hypertabular import Column, DelimitedReader, Dialect, Fault, SheetOptions, Success, Workbook

# The header first; then a plan that reads the columns by name, wherever the file put them.
with DelimitedReader.open("people.csv", Dialect.CSV) as reader:
    header = reader.header  # Header(('id', 'name', 'score'))
    print("header:", list(header))
    plan = [
        Column.i32(header.ordinal("id")),  # KeyError naming the column if it is missing
        Column.text(header.ordinal("name")),
        Column.f64(header.ordinal("score")),
    ]
    reader.bind(plan)
    for batch in reader:
        ids, names, scores = batch.columns

        # A column at a time, as the core wrote it…
        print("ids:", list(ids.values))  # a memoryview of C ints
        print("faults:", scores.fault_count)

        # …or a cell at a time, as HyperCast's union.
        for row in range(batch.rows):
            match scores[row]:
                case Success(value):
                    print(f"{names.values[row]}: {value}")
                case Fault(reason):
                    print(f'line {batch.line(row)}: {reason.name} in "{scores.raw(row).decode()}"')

# A plan known up front goes straight to the constructor, bound before the header is read.
plan = [Column.i32(0), Column.text(1), Column.f64(2)]
with DelimitedReader.open("people.csv", Dialect.CSV, plan) as reader:
    for row in reader.rows():  # every row, batch after batch
        print(f"line {row.line}: {row.text(1)}")

# A workbook reads into the same batch, its sheet header first or with the plan up front.
book = Workbook.open("people.xlsx")
print("sheets:", [info.name for info in book.sheets])  # SheetInfo(name='People', hidden=False)
sheet = book.sheet("People", SheetOptions())
header = sheet.header
sheet.bind([Column.text(header.ordinal("name")), Column.decimal(header.ordinal("score"))])
for row in sheet.rows():
    match row.get(1):
        case Success(value):
            print(f"{row.text(0)}: {value}")
        case Fault(reason):
            print(f"line {row.line}: {reason.name}")
```

A plan known up front goes straight to the constructor — `DelimitedReader.open(path, dialect,
plan)`, `book.sheet(which, options, plan)` — which binds it before the header is read.

## Header first

A reader made without a plan — `DelimitedReader(source, dialect)`, `DelimitedReader.open(path,
dialect)`, `book.sheet(which, options)` — reads the header and waits: `bind(plan)` declares the
plan once, before the first read. A read before it raises `RuntimeError` (and a later `bind`
puts that right); so does a second `bind`. `is_bound` says which it is, and a delimited
reader's `column_count` is the header's width, or the first record's once it has been read.
Once a delimited header has fixed that width, a plan column at or past it could only ever be
empty, so it is a `ValueError` naming the column — from `bind`, or from the constructor (and
`open_async`) as soon as the header is read. Without a header such a column reads as empty,
and a plan reaching past 256 source columns reads shorter batches, as many rows as about
8 MiB of cell table holds.

The header is a `Header`: a tuple of `str`, with `ordinal(name)` — the one column with exactly
that name, case and spaces included, a `KeyError` naming it if there's none, and an
`AmbiguousColumn` (a `ValueError`) naming both positions if two columns share it, since a plan
bound to the first would read the wrong column without a word — and `get(name, default=None)`,
which takes the first of those and returns `default` if there's none. A `bytes` name is matched
against a name's UTF-8.

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```python
from hypertabular import (
    Column,
    DelimitedReader,
    Dialect,
    Encoding,
    NumFormat,
    Range,
    SheetOptions,
    ShortRows,
    SuggestOptions,
)

# Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
dialect = Dialect(
    ",", skip=1, short_rows=ShortRows.PAD, encoding=Encoding.WINDOWS_1252, tokens=("NA",)
)
data = b"Exported by hand\nname,amount\ncaf\xe9,NA\ntea\nmilk,12x4\n"
plan = [
    Column.text(0),
    Column.f64(1, NumFormat.INVARIANT).with_tokens(["NA", "-"]),  # in place of the dialect's
]
with DelimitedReader(data, dialect, plan) as reader:
    for batch in reader:
        for row in batch:
            cell = row.get(1)
            print(row.text(0), cell.reason.name, row.origin(1).name, cell.detail and cell.detail.name)
        # What went wrong, and only that: a blank, a token and a padded cell aren't problems.
        for problem in batch.faults():
            print("line", problem.line, "column", problem.source_column, problem.reason.name, problem.detail.name)

# A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
found = DelimitedReader.suggest_bytes(
    b'id,total,day\n1,"1,234.50",2026-01-07\n2,12.00,2026-01-08\n', Dialect.CSV
)
for column in found.report:
    print(column.name, column.status.name, column.door.name, column.matched, "of", column.sampled)

# A sheet takes the part of it that's the table; its plan's ordinals count from B here.
cut = Range.parse("B2:D40")
options = SheetOptions(range=cut)
print(options.range.first_column, options.range.last_row)
```

A `TabularError` of kind `ENCODING` comes after every intact row before it, and its `byte` alone
counts the original bytes: a read's spans are all the decoded text's. A sheet's `Range` is parsed by
the core, so `Range.parse("A5:F200")` means what it means in every binding, with `Range(first_row,
first_column, last_row, last_column)`, 1-based and `0` for no limit, for what that can't say, such
as every row from the fifth. A plan's ordinals count from the range's first column, while
`Batch.line` and a problem's `source_column` stay the sheet's own.

A `Fault` carries HyperCast's `detail`, and `batch.origin(column, row)`, `column.origin(row)` and
`row.origin(column)` say where a cell's verdict came from: `Origin.CELL`, `TOKEN`, `PADDED` or
`UNCACHED`. `batch.faults()` is a new `list` of `Problem`s each time it's asked, the row, the line,
the plan and source columns, the reason, detail, origin and span, a long row reported with its
`expected` and `found`. A list costs nothing until it's asked for, and then a `Problem` a fault: a
file of 2M rows whose 6M cells all fail reads in 103.1 ms, and in 5.05 s asking every batch for its
list (Apple M6, 2026-11-08, the lowest of three rounds of each). A batch is a copy, so a list and a batch both stay right after the next read
and after the reader has gone; nothing here goes stale.

`notes=False`, on a reader or in `SheetOptions`, drops `detail` and `origin`, which then say `None`
and `Origin.CELL`: the read costs a little less, and `faults()` can't list a formula with no cached
value. `Column.with_date_options` hands a date door HyperCast's `DateOptions` and `MonthNames`, and on
a sheet it and `with_tokens` reach only a text cell; `with_tokens([])` says a column has none, the
dialect's included. Every distinct list of tokens, and every distinct list of month names, is kept
for the life of the process, as the Rust API keeps them: declare them, don't build a new list per
file from what the files say.

All of this reads the same through `open_async` and `read_async`, and `notes=` is theirs too; a stream
stalled mid-record still hands over every row it has finished.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous and read
as text until a format is declared or another cell settles them, which takes in a column of prices at
three decimals; `SuggestOptions(date_order=...)` does the same for `01/02/2026`. `2,5` beside `2.5` is
unresolved, and `1,23,456`, Indian grouping, is text until the column declares Indian grouping, because
nothing says its culture.

`DelimitedReader.suggest(path, dialect, options)` and `suggest_bytes(data, dialect, options)` sample a
file or bytes, and `sheet.suggest(options)` a sheet, whatever it has been up to, leaving it where it
was. `SuggestOptions(sample_rows=0)` means 1 and a negative count is refused; options that declare only
a format still sample a thousand rows, at most a batch. A headerless file is opened twice, so no
suggestion is offered of a stream; in asyncio code `await asyncio.to_thread(DelimitedReader.suggest,
...)` keeps the loop free, since a suggest lets the interpreter go while it reads. A report's `format`
is the plan's own, equal to any `NumFormat` that declares the same thing, as HyperCast compares them.

A sheet's number cell has no notation, so a column of them is a number under any format, where text
holding the same digits asks. A column of `0` and `1` is bool, `007` and an integer past int64 are text,
and a typed date in a sheet is text, for a plan to change by hand. With nothing to choose between, a
resolved number is planned in `,` groups of three, and `SuggestedColumn.format` names what the plan
uses: a later `1,23,456` is refused.

**To keep every row of a sheet, set `skip_empty_rows=False`.** By default a row whose only cells are
formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `Origin.UNCACHED`.

A column given no format is Western, the same as `NumFormat.INVARIANT`: `1,234,567` is 1234567 and
`1,23,456` is `MALFORMED`. If a column of Indian numbers comes back refused, declare it, one column at
a time, so an Indian column reads beside a Western one in the same read:

```python
indian = NumFormat.INVARIANT.with_grouping(NumFormat.GROUPING_INDIAN)
either = [
    Column.i32(0, indian),  # 1,23,456 is 123456, and 1,234,567 is MALFORMED
    Column.i32(1),  # 1,234,567 is 1234567, and 1,23,456 is MALFORMED
]
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat.DETECT` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column.f64` reads `1,2345` as
1.2345, `12,34` as 12.34 and `0,125` as 0.125. A format holding the retired grouping bit `256` (the
words `351` and `863`), which only a stored or hand-packed format can hold, raises `ValueError` when
the plan is bound, before any cell, and in a suggest's options before it opens anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape
and the locales neither policy matches.

## Rows

A batch is column-major; a `Row` reads it across, for code that builds an object a row at a
time. `for row in batch` (or `batch.iter_rows()`, `batch.row(i)`) gives views — the batch and
an index, nothing copied — with `index`, `line`, `get(column)` (the cell as `Success` or
`Fault`), `text(column)` (a text cell's `str`, or `None`), `origin(column)` and `raw(column)`. `reader.rows()`
and `sheet.rows()` run across batches, reading the next as one runs out. A batch owns what it
shows, so a row stays valid after the reader has moved on.

```python
from dataclasses import dataclass


@dataclass
class Person:
    id: int
    name: str


plan = [Column.i32(0), Column.text(1)]
with DelimitedReader.open("people.csv", Dialect.CSV, plan) as reader:
    people = [
        Person(id.value, row.text(1))
        for row in reader.rows()
        if isinstance(id := row.get(0), Success)
    ]
print(people)
```

## Streams and asyncio

`DelimitedReader(source, …)` takes `bytes` or a binary file object (anything with
`read(size)`), and so does `Workbook(data)`: a workbook is read to the end into memory it owns,
since a zip is read from its directory at the end back. Either way the format is read from the
bytes. A file object is the caller's to close. A delimited reader calls the object's
`read1(size)` where it has one and parses whatever each read returns, so over a pipe, a socket
or a FIFO opened with `DelimitedReader.open` the header and every complete record arrive
without waiting for the buffer to fill.

An asynchronous stream — an `asyncio.StreamReader`, or anything with `async read(size)` — is
read with asyncio:

```python
reader = await DelimitedReader.open_async(stream, Dialect.CSV)  # the header is awaited too
reader.bind([Column.i64(reader.header.ordinal("id"))])
async for batch in reader:  # or: await reader.read_async()
    print(batch.rows)

book = await Workbook.open_async(stream)
```

The core never waits: when a read needs input it says how much, the stream is awaited for that
much and the read goes on, all the core's work done between awaits with the interpreter
released. Cancellation is asyncio's, and a read cancelled while it awaits the stream leaves the
reader where it was — the next read carries on. One read at a time: a second, from another
coroutine or thread while one awaits the stream, raises `RuntimeError` and leaves the first as it
was. `read_async` and `async for` work on any reader; over `bytes` or a blocking file object they
complete without awaiting anything.

A stream that raises, a file object's `read` or an `async read`, hands its exception to that read
and isn't final: `KeyboardInterrupt` included, the reader stays where it was, and the next read asks
the stream again. A stream that hands over more than it was asked for, or something other than
`bytes`, has lost what it handed over, so that is final: every later read raises the same error. A
file the binding opens itself (`DelimitedReader.open`, `DelimitedReader.suggest` and
`Workbook.open`) is read as CPython reads one: Ctrl-C reaches an open or a read waiting on a FIFO,
its `KeyboardInterrupt` isn't final, and the next read goes on from where it was. Under PyPy a signal
doesn't cut a blocked open or read short, so there Ctrl-C lands when the writer next writes. An I/O error from
such a file is final, and every later read raises the same one. A broken record is final
everywhere. A reader closed while one of its reads awaits the stream, from inside the stream's own
`read` or from another task, ends that read with `ValueError` saying so.

One window is CPython's own, and no binding can close it: CPython runs signal handlers as a call
returns, so a `KeyboardInterrupt` that arrives just as `read()` or `next(reader)` hands back a batch
is raised before the caller holds it, and that batch is lost. Its rows are missing, never wrong. A
`for batch in reader` loop doesn't check there, so a loop that must not lose rows to Ctrl-C iterates
with `for`.

## The shape

The native core owns no memory and reads no files. It is linked straight into a CPython
extension module (`hypertabular._native`, PyO3), and the extension is the binding, over the
Rust API: the Rust reader's buffers are filled in one call per batch with the interpreter
released, and each batch is copied out of them. A column reaches Python whole — a read-only
`memoryview` of the door's own item type, which anything that takes a buffer takes — with no
Python call per cell.

- **One batch type.** Iterating a reader or a sheet (or calling `read()`) gives a `Batch`:
  `rows`, `columns`, `column(i)`, `line(row)`, `raw(column, row)`, `get(column, row)`,
  `text(column, row)`, `origin(column, row)` and `faults()`, and its rows when iterated. Each column
  is a `ColumnData` with `values`, `verdicts`, `fault_count`, `faults()`, `origin(row)`, `raw(row)` and
  `column[row]` for one cell as `Success` or `Fault`. A batch owns what it shows: it stays
  valid after the reader has moved on.
- **The core decides how much a batch may ask for.** A `batch_rows` or `buffer_bytes` of 0, and a batch
  past 4 GiB (214 million rows of one `i32` column, 1.8 million of 100 text columns), are a `ValueError`
  from the core, when a reader or a sheet opens or a plan is bound, `open_async` included, before
  anything is allocated. A `buffer_bytes` past 1 GiB starts there. A size it allows and the machine
  can't give is a `MemoryError`. With no plan and no row widths a delimited read has no arrays, and
  reads at any `batch_rows`. `suggest` is never refused for the size of its own batches: the core says
  what fits and the sampler reads as many as it takes. A row that alone is past the limit is a `ValueError`
  naming its width. A negative `SheetOptions.batch_rows` is a `ValueError` when it's made.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and the rest
  of [a messier file](#a-messier-file); `SheetOptions` states a sheet's header, whether empty rows
  are skipped, the batch size and its range; the plan states each column's door and, for numbers,
  its `NumFormat`. A suggestion is a plan to look at, applied to nothing.
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

Each platform gets two wheels, both stable-ABI, which between them cover every CPython from
the 3.11 floor up, with the GIL or without it, so on every platform below nothing is compiled
on install:

| Wheel | Interpreters |
| --- | --- |
| `cp311-abi3` | CPython 3.11 and newer, with the GIL |
| `cp315-abi3.abi3t` | CPython 3.15 and newer, free-threaded or not ([PEP 803](https://peps.python.org/pep-0803/)) |

The platforms are Linux (glibc and musl) on x86-64 and arm64, macOS on Apple silicon and
Intel, and Windows on x86-64 and arm64: 16 wheels, and a seventeenth for Pyodide
([below](#in-the-browser-pyodide)). Elsewhere (a glibc older than 2.28, PyPy, free-threaded
3.14) pip falls back to building the same extension from the source distribution,
`hypertabular-<version>.tar.gz`, which needs Rust 1.89 or newer and a C linker, and builds
HyperCast's from its own sdist the same way where no HyperCast wheel fits. CI proves that
build on x86-64 glibc 2.17 and on musl, with CPython 3.11 at exactly Rust 1.89. pip's build
isolation fetches `maturin`. Free-threaded CPython older than 3.15 has no stable ABI, so its
wheel would serve one interpreter version, too narrow an ABI to ship; free-threading starts at
3.15, where `abi3t` covers it. Free-threaded 3.14 builds the sdist (a report-only CI job
builds and tests it there); free-threaded 3.13 cannot, since PyO3 0.30 refuses free-threaded
CPython below 3.14.

**Free-threaded CPython runs it without the GIL.** The extension declares it needs no GIL, and
so does HyperCast's, so a free-threaded interpreter keeps the GIL off after
`import hypertabular` (no `RuntimeWarning`), and readers on different threads run truly in
parallel. A `Batch`, its columns and rows, and a `Workbook` are immutable once made and can be
shared by any number of threads; a column's `values` is built once, whichever thread asks
first. A `DelimitedReader` or a `Sheet` is read by one thread at a time, as everywhere: a
second thread calling into one while another is inside a read gets `RuntimeError`, "the
reader is in the middle of a call; nothing that call runs can call back into it" ("the sheet"
for a `Sheet`, the words every binding uses), and nothing else, never a race on its buffers.
So does a stream's `read` that calls back into the reader reading it. The bindings are written
once, for free-threading; a GIL build runs the same code. CI runs the whole suite on
free-threaded 3.15 on every platform, and installs each wheel on its own interpreters before
it is published.

## In the browser (Pyodide)

The same PyO3 extension, compiled for Pyodide's Emscripten target, is published to PyPI as a
seventeenth wheel, `hypertabular-<version>-cp311-abi3-pyemscripten_2026_0_wasm32.whl`, so micropip
finds it the way pip finds the native wheels, and HyperCast's beside it:

```python
import micropip
await micropip.install("hypertabular")

import hypertabular
hypertabular.native_version()  # the linked core's version
```

It is the same native extension, with the same API and no JavaScript bridge. CI installs the
wheel into Pyodide and runs this package's whole pytest suite in it twice, under Node and in
headless Chrome, corpus replay included; the one test that starts threads is skipped, since
Pyodide cannot. The wheel is for **Pyodide 314.x** (Python 3.14, platform
`pyemscripten_2026_0`): each Pyodide ABI year needs its own wheel.

## Verifying provenance

Every wheel PyPI serves, the Pyodide one included, and the sdist verify from the release tag
alone: the
release attestation, signed by this repository's `release.yml` at the tag, names every one of
them, with the release inventory as its predicate.

```sh
pip download hypertabular==<version> --no-deps -d .
curl -sLO "$(curl -s https://pypi.org/pypi/hypertabular/<version>/json |
  jq -r '.urls[] | select(.packagetype=="sdist") | .url')"
for f in hypertabular-<version>*; do
  gh attestation verify "$f" --repo SkunkWerkx/HyperTabular \
    --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
done
```

Each wheel and the sdist also carry build provenance where each was built, installed and
attested: `hyper-build-wheels.yml` in `SkunkWerkx/.github`, at the preparation commit on
master the release was prepared from (`release.yml` verifies each wheel and the sdist against
it before publishing it unchanged). That check names the signer, or uses `--owner` for both:

```sh
for f in hypertabular-<version>*; do
  gh attestation verify "$f" \
    --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
  # or: gh attestation verify "$f" --owner SkunkWerkx
done
```

Both are the GitHub/Sigstore transparency-log route, checked with `gh attestation verify`, the
same route every other artifact in this project uses. Separately, PyPI itself holds a
[PEP 740](https://peps.python.org/pep-0740/) publish attestation for every wheel and the sdist, filed by
`gh-action-pypi-publish` at publication and shown on each file's PyPI page, which PyPI-side
tooling checks on its own. The root README's [Provenance](../README.md#provenance) section has
the whole picture.

## License

[MIT](LICENSE)
