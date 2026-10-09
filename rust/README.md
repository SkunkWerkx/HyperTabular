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

let book = Workbook::open("data.xlsx")?;         // XLSX or ODS, told by what is in it
let mut sheet = book.sheet_unbound("Data", SheetOptions::default())?;
sheet.bind(&plan)?;                              // or book.sheet("Data", options, &plan)
while let Some(batch) = sheet.read()? {
    // the same batch
}
```

A reader that already knows its columns by position takes the plan when it opens
(`DelimitedReader::open(path, dialect, &plan)`). A workbook opens from a path, from bytes,
or from any `Read` (`Workbook::from_reader`). With the `async` feature, a reader and a
workbook open from a
[`futures_io::AsyncRead`](https://docs.rs/futures-io/0.3/futures_io/trait.AsyncRead.html)
— Tokio's through `tokio_util::compat` — and `read_async` awaits the source between
batches.

A cell that does not cast is never an error: it is a `Fault` in the batch — `Empty`,
`Malformed` or `OutOfRange`, with the span of the offending bytes — and `batch.raw` gives
the text it points into. Structural failures (a torn row, a corrupt zip) are `Error`s,
returned once every intact row before them has been delivered.

**WebAssembly.** The crate needs nothing from its host — no clock, no entropy, no
allocator in the core — so it builds for `wasm32-unknown-unknown` as it is, and its whole
suite passes under wasmtime on `wasm32-wasip1`. CI runs `browser-test/` in headless Chrome
on every pull request.

The design record is in [`docs/`](https://github.com/SkunkWerkx/HyperTabular/tree/master/docs).
