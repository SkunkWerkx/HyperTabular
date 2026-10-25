# hypertabular

Forward-only tabular parsing — delimited text and spreadsheets — with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

The crate is two layers:

- **The core** (`hypertabular::kernel`) — `no_std`, allocation-free and panic-free. It
  fills buffers its caller owns, and is what the native library `libhypertabular` exports
  to C#, Java, Go, Swift, Ruby, PHP and Python.
- **The Rust API** (the default `std` feature) — the Rust binding over the core: readers
  that own their buffers, read files, and lend out what the core wrote.

```rust
use hypertabular::{Column, DelimitedReader, Dialect, Fault, SheetOptions, Workbook};

// Open, read the header, and only then say which columns to read, by name, through
// which door. Nothing is sniffed.
let mut reader = DelimitedReader::open_unbound("data.csv", Dialect::CSV)?;
let header = reader.header().unwrap();
let plan = [
    Column::i64(header.require("id")?),
    Column::f64(header.require("amount")?),
    Column::text(header.require("name")?),
];
reader.bind(&plan)?;
while let Some(batch) = reader.read()? {
    let ids: &[i64] = batch.i64(0);              // a whole column, as the core wrote it
    let verdicts = batch.verdicts(0);            // and a verdict beside each value
    for row in batch {                           // or the batch read across, a row at a time
        let amount: Result<f64, Fault> = row.get(1);
        let name = row.text_str(2);              // a str borrowed from the input
    }
}

let book = Workbook::open("data.xlsx")?;         // XLSX, ODS or Numbers, told by what is in it
let mut sheet = book.sheet_unbound("Data", SheetOptions::default())?;
let header = sheet.header().unwrap();
sheet.bind(&[
    Column::decimal(header.require("amount")?),
    Column::timestamp(header.require("created_at")?),
])?;
while let Some(batch) = sheet.read()? {
    let amounts = batch.values::<hypertabular::Decimal>(0);
    let created_at = batch.values::<hypertabular::Timestamp>(1);
    // Check batch.verdicts(column) beside each slice before using its values.
}
```

A reader that already knows its columns by position takes the plan when it opens
(`DelimitedReader::open(path, dialect, &plan)`). A workbook opens from a path, from bytes,
or from any `Read` (`Workbook::from_reader`). With the `async` feature, a reader and a
workbook open from a
[`futures_io::AsyncRead`](https://docs.rs/futures-io/0.3/futures_io/trait.AsyncRead.html)
— Tokio's through `tokio_util::compat` — and `read_async` awaits the source between
batches.

Each stream refill returns to parsing after a successful read. A complete header or
batch can be delivered while the source remains open, without filling the input buffer.
A workbook still needs its complete container before opening.

`Batch::values::<T>(column)` lends a whole column for any supported value type, including
decimals and temporal values; the primitive accessors such as `i64` remain available.
Slices borrow the reader and cannot survive its next read. A failed cell holds the door's
zero: read `batch.verdicts(column)` alongside values. Text uses `batch.text` or `text_str`.

The supported allocation-free Rust core surface is `kernel::abi`, `kernel::door`,
`kernel::exports`, `kernel::delimited::fill`, and `kernel::workbook`'s state, memory,
format constants and `book`/`rows` operations. Parser and SIMD implementation modules
are private. The C ABI is unchanged.

Build `Column`, `Dialect`, and `SheetOptions` with their constructors, constants and
builder methods. These configuration types are non-exhaustive so settings can be added;
`Door`, `Error`, `Format`, and `FailureKind` likewise require wildcard arms when matched
outside the crate.

A cell that does not cast is never an error: it is a `Fault` in the batch — `Empty`,
`Malformed` or `OutOfRange`, with the span of the offending bytes — and `batch.raw` gives
the text it points into. Structural failures (a torn row, a corrupt zip) are `Error`s,
returned once every intact row before them has been delivered.

**WebAssembly.** The crate needs nothing from its host — no clock, no entropy, no
allocator in the core — so it builds for `wasm32-unknown-unknown` as it is, and its whole
suite passes under wasmtime on `wasm32-wasip1`. CI runs `browser-test/` in headless Chrome
on every pull request.

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
