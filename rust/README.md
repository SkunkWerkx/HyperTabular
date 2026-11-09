# hypertabular

Forward-only tabular parsing — delimited text and spreadsheets — with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

The crate is two layers:

- **The core** (`hypertabular::kernel`) — `no_std`, allocation-free and panic-free. It
  fills buffers its caller owns, and is what the native library `libhypertabular` exports
  over its C ABI to C, C++ and the bindings that drive it.
- **The Rust API** (the default `std` feature) — the Rust binding over the core: readers
  that own their buffers, read files, and lend out what the core wrote.

```sh
cargo add hypertabular
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

```rust
use hypertabular::{Column, Decimal, DelimitedReader, Dialect, Fault, SheetOptions, Workbook};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open, read the header, and only then say which columns to read, by name, through
    // which door. Nothing is sniffed.
    let mut reader = DelimitedReader::open_unbound("people.csv", Dialect::CSV)?;
    let header = reader.header().unwrap();
    let names: Vec<_> = header.names().map(String::from_utf8_lossy).collect();
    println!("header: {names:?}");
    let plan = [
        Column::i32(header.require("id")?),
        Column::text(header.require("name")?),
        Column::f64(header.require("score")?),
    ];
    reader.bind(&plan)?;
    while let Some(batch) = reader.read()? {
        let ids: &[i32] = batch.i32(0); // a whole column, as the core wrote it
        let verdicts = batch.verdicts(2); // and a verdict beside each value
        println!("ids: {ids:?}");
        println!("no score: {}", verdicts.iter().filter(|v| !v.is_ok()).count()); // blanks too
        println!("problems: {}", batch.faults().count()); // what went wrong, in the whole batch
        for row in batch {
            // or the batch read across, a row at a time
            let name = row.text_str(1)?; // a str borrowed from the input
            let score: Result<f64, Fault> = row.get(2);
            match score {
                Ok(score) => println!("{name}: {score}"),
                Err(fault) => println!(
                    "line {}: {:?} in {:?}",
                    row.line(),
                    fault.reason,
                    String::from_utf8_lossy(&row.raw(2))
                ),
            }
        }
    }

    // A reader that knows its columns by position takes the plan when it opens.
    let plan = [Column::i32(0), Column::text(1), Column::f64(2)];
    let mut by_position = DelimitedReader::open("people.csv", Dialect::CSV, &plan)?;
    by_position.for_each_row(|row| -> Result<(), Box<dyn std::error::Error>> {
        println!("line {}: {}", row.line(), row.text_str(1)?); // every row, batch after batch
        Ok(())
    })?;

    let book = Workbook::open("people.xlsx")?; // XLSX, ODS or Numbers, told by what is in it
    let sheets: Vec<&str> = book.sheets().iter().map(|s| s.name.as_str()).collect();
    println!("sheets: {sheets:?}");
    let mut sheet = book.sheet_unbound("People", SheetOptions::default())?;
    let header = sheet.header().unwrap();
    sheet.bind(&[
        Column::text(header.require("name")?),
        Column::decimal(header.require("score")?),
    ])?;
    while let Some(batch) = sheet.read()? {
        let scores = batch.values::<Decimal>(1);
        // Check batch.verdicts(column) beside each slice before using its values.
        for (row, verdict) in batch.verdicts(1).iter().enumerate() {
            match verdict.fault() {
                None => println!("{}: {}", batch.text_str(0, row)?, scores[row]),
                Some(fault) => println!("line {}: {:?}", batch.line(row), fault.reason),
            }
        }
    }
    Ok(())
}
```

`header.require` matches a name exactly, and won't choose: a name no column has is
`Error::NoColumn`, and a name two columns share is `Error::AmbiguousColumn`, which says both
positions. `header.ordinal` is the optional lookup, and answers with the first of them. Names are
kept as the file wrote them, repeats included.

A reader that already knows its columns by position takes the plan when it opens
(`DelimitedReader::open(path, dialect, &plan)`), so a repeated name is no obstacle to it. Once a
header has fixed the width every record has, a plan column at or past it could only ever be empty,
so it is `Error::Plan` naming the column — from `bind`, or from the constructor as soon as it has
read the header. Without a header such a column reads as empty, and a plan reaching past 256 source
columns reads shorter batches, as many rows as about 8 MiB of cell table holds. A workbook opens
from a path, from bytes, or from any `Read` (`Workbook::from_reader`). With the `async` feature, a
reader and a workbook open from a
[`futures_io::AsyncRead`](https://docs.rs/futures-io/0.3/futures_io/trait.AsyncRead.html)
— Tokio's through `tokio_util::compat` — and `read_async` awaits the source between
batches.

A delimited stream goes back to parsing after every read that returns bytes, so a live
source — a pipe, a socket — hands over each complete header and batch as soon as it has
arrived, without waiting for the buffer to fill or the source to close. A workbook is a zip
whose directory is at its end, so it is read whole before it opens.

`batch.values::<T>(column)` lends a typed column whole, for every door but text —
decimals, UUIDs and temporal values included — and `batch.i64(column)` and its siblings
are the same for the primitives. A slice borrows the reader and cannot outlive its next
read. A cell that did not cast holds the door's zero, so read `batch.verdicts(column)`
beside the values. A text cell is `batch.text` or `batch.text_str`.

A binding that hands a batch out past the next read keeps copies of it, and `KeptBatch` works
the batch's faults list out from them when it's asked for, the list `batch.faults()` gave:
each column's `batch.verdicts`, `batch.notes` and `batch.source_column`, each row's
`batch.line`, and `batch.reported_widths()`.

`Column`, `Dialect` and `SheetOptions` are built with their constructors, constants and
builder methods (`Column::format`, `Dialect::with_quoting`, …), and are
`#[non_exhaustive]` so a minor release can add a setting; `Door`, `Error`, `Format` and
`FailureKind` are too, so a `match` on one outside the crate needs a wildcard arm. Below the Rust
API, the core is reached through `kernel::abi`, `kernel::door`, `kernel::exports`,
`kernel::delimited::fill`, `kernel::faults`, `kernel::suggest`, `kernel::transcode`, and
`kernel::workbook`'s state, memory, format constants and `book` and `rows` operations — what the C ABI exports;
the parsing, compression and SIMD modules behind them are private. That ABI's C header,
`include/hypertabular.h`, is generated from them by cbindgen (`tests/header.rs` fails when it
is stale), and the release's C and C++ archives are described in
[`c/README.md`](https://github.com/SkunkWerkx/HyperTabular/tree/master/c).

Nothing is sniffed in a read. Declaring the locale, a number format and a date order, is the
recommended route, and then nothing needs inferring. A caller can also ask, for a starting point
to inspect: `DelimitedReader::suggest` (a path), `DelimitedReader::suggest_slice` (bytes) and
`Sheet::suggest` sample up to 1,000 rows through the text door and return a `Suggestion`: a `plan` to
look at, change and pass to a read, a `report` of each column's door, status and counts, and whether
rows lie past the sample (`truncated`). There's no form of it for a stream, which would lose what it
sampled.

With no format declared, each of two notations has to read the whole column. `1,234,567` and
`1,234.5` read only with `,` grouping, and `1.234.567`, `1.234,5` and `2,5` only with `,` as the
decimal. `1,234` and `1.234` read both ways, so they come back ambiguous and read as text until the
caller declares a format (`SuggestOptions::format`) or another cell settles them, which takes in a
column of prices at three decimals. `0,125` is 0.125, since no thousands group starts with a zero.
`01/02/2026` is ambiguous until a date order is declared (`SuggestOptions::date_order`). A resolved numeric column is planned in the
notation that read it, or the one declared; with neither to choose between, in `NumFormat::INVARIANT`, so
a later `1,23,456` is `Malformed`. `SuggestedColumn::format` names the format the plan uses.

Read the report, because some answers can surprise. `0` and `1` are bool, and `F` and `No` read
`false`. `1,23,456`, Indian grouping, is text with no format declared, because nothing says its culture,
and declaring Indian grouping for the column makes it a number, and `1,234,567` text. A workbook's number cell has no notation, so a column of them is a number under any format,
where text holding the same digits asks. A workbook's date column suggests text, since a date cell reads
`2023-03-15T00:00:00`. `007` and an integer past `i64` are text, with no note saying why.

**To keep every row of a sheet, set `skip_empty_rows` to `false`** (`SheetOptions::with_empty_rows_skipped(false)`).
By default a row whose only cells are formulas with no cached value is filtered out as empty, with
nothing to say so; off, those rows are delivered, their cells at `Origin::Uncached`.

A column given no format reads `NumFormat::INVARIANT`, whose grouping is Western: `1,234,567` is
1234567 and `1,23,456` is `Malformed`. If a column of Indian numbers comes back refused, declare it, one
column at a time, so an Indian column reads beside a Western one in the same read:

```rust
use hypertabular::{Column, DelimitedReader, Dialect, Grouping, NumFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = b"indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";
    let plan = [
        Column::i32(0).format(NumFormat::INVARIANT.with_grouping(Grouping::Indian)),
        Column::i32(1),
    ];
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &plan)?;
    let batch = reader.read()?.expect("a batch");
    assert_eq!(batch.get::<i32>(0, 0), Ok(123456));
    assert!(batch.get::<i32>(1, 0).is_err()); // `1,23,456` under Western
    assert!(batch.get::<i32>(0, 1).is_err()); // `1,234,567` under Indian
    assert_eq!(batch.get::<i32>(1, 1), Ok(1234567));
    Ok(())
}
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat::DETECT` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `F64` reads `1,2345` as 1.2345, `12,34`
as 12.34 and `0,125` as 0.125. A format holding the retired
grouping bit `256` (the words `351` and `863`) is `Error::Plan` when the plan is bound, before any cell,
and in a `suggest`'s options before it opens anything;
`NumFormat::new` itself takes the word without a word. No locale name is read: Rust has no locale bridge.
HyperCast's [grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape and the locales neither policy matches.

`Dialect::with_tokens`, `SheetOptions::with_tokens` and `Column::tokens` declare what stands for no
value, and `Column::date_options` what a date door accepts. So that those types can stay `Copy`,
each distinct token list or month vocabulary is kept for the life of the process, shared process-wide:
that's bounded by the lists a program declares, but a server building them from request data grows
without bound.

The core decides how much a batch may ask for, before anything is allocated: a `batch_rows` or a
`buffer_bytes` of 0 is `Error::ZeroBatchRows` or `Error::ZeroBufferBytes`, and a batch past 4 GiB (214
million rows of one `i32` column, 1.8 million of 100 text columns) is `Error::BatchTooLarge`, at open and
when a plan is bound. A size it allows and the machine can't give is `Error::NoMemory`. A suggestion is
never refused for the size of its own batches, since the core says what fits and the sampler reads as
many as it takes; a row that alone is past the limit is `Error::RowTooWide`, naming its width.

A cell that does not cast is never an error: it is a `Fault` in the batch — `Empty`,
`Malformed` or `OutOfRange`, with the span of the offending bytes — and `batch.raw` gives
the text it points into. Structural failures (a torn row, a corrupt zip) are `Error`s,
returned once every intact row before them has been delivered.

**WebAssembly.** The crate needs nothing from its host — no clock, no entropy, no
allocator in the core — so it builds for `wasm32-unknown-unknown` as it is, and its whole
suite passes under wasmtime on `wasm32-wasip1` (all but its three `should_panic` tests,
which libtest skips where a panic aborts). CI runs that suite in `core-checks` and
`browser-test/` in headless Chrome, on every pull request that touches the core or `.github/`.

The design record is in [`docs/`](https://github.com/SkunkWerkx/HyperTabular/tree/master/docs).

## Verifying provenance

The published `.crate` verifies from the release tag alone: the release attestation, signed by
this repository's `release.yml` at the tag, names it among every file the release ships, with
the release inventory as its predicate.

```sh
curl -LO https://static.crates.io/crates/hypertabular/hypertabular-<version>.crate
gh attestation verify hypertabular-<version>.crate --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Its build provenance is the second check. The crate is packed by `hyper-pack-crate.yml`, a
reusable workflow that lives in `SkunkWerkx/.github`, and that is the identity Fulcio records
as the signer, so `--repo` alone is not enough there: add `--signer-repo`, or use `--owner` in
place of both.

```sh
gh attestation verify hypertabular-<version>.crate \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
# or: gh attestation verify hypertabular-<version>.crate --owner SkunkWerkx
```

Get the signer wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which
reads like a bad signature but is only an identity mismatch. The root README's
[Provenance](../README.md#provenance) section has the whole picture: the inventory, which
artifacts are signed from where, and the gates every byte passes before publication.
