# hypertabular

Forward-only tabular parsing — delimited text and spreadsheets — with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

The crate is two layers:

- **The core** (`hypertabular::kernel`) — `no_std`, allocation-free and panic-free. It
  fills buffers its caller owns, and is what the native library `libhypertabular` exports
  to C#, Java, Go, Swift, Ruby, PHP and Python.
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
        println!("faults: {}", verdicts.iter().filter(|v| !v.is_ok()).count());
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

A reader that already knows its columns by position takes the plan when it opens
(`DelimitedReader::open(path, dialect, &plan)`). Once a header has fixed the width every
record has, a plan column at or past it could only ever be empty, so it is `Error::Plan`
naming the column — from `bind`, or from the constructor as soon as it has read the header.
Without a header such a column reads as empty, and a plan reaching past 256 source columns
reads shorter batches, as many rows as about 8 MiB of cell table holds. A workbook opens from a path, from bytes,
or from any `Read` (`Workbook::from_reader`). With the `async` feature, a reader and a
workbook open from a
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

`Column`, `Dialect` and `SheetOptions` are built with their constructors, constants and
builder methods (`Column::format`, `Dialect::with_quoting`, …), and are
`#[non_exhaustive]` so a minor release can add a setting; `Door`, `Error`, `Format` and
`FailureKind` are too, so a `match` on one outside the crate needs a wildcard arm. Below the Rust API, the core is reached through `kernel::abi`,
`kernel::door`, `kernel::exports`, `kernel::delimited::fill`, and `kernel::workbook`'s
state, memory, format constants and `book` and `rows` operations — what the C ABI exports;
the parsing, compression and SIMD modules behind them are private.

A cell that does not cast is never an error: it is a `Fault` in the batch — `Empty`,
`Malformed` or `OutOfRange`, with the span of the offending bytes — and `batch.raw` gives
the text it points into. Structural failures (a torn row, a corrupt zip) are `Error`s,
returned once every intact row before them has been delivered.

**WebAssembly.** The crate needs nothing from its host — no clock, no entropy, no
allocator in the core — so it builds for `wasm32-unknown-unknown` as it is, and its whole
suite passes under wasmtime on `wasm32-wasip1` (all but its three `should_panic` tests,
which libtest skips where a panic aborts). CI runs that suite in its `test-wasi` job and
`browser-test/` in headless Chrome, on every pull request.

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
