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

// A plan: which source column, through which door. Nothing is sniffed.
let plan = [Column::i64(0), Column::f64(2), Column::text(3)];

let mut reader = DelimitedReader::open("data.csv", Dialect::CSV, &plan)?;
while let Some(batch) = reader.read()? {
    let ids: &[i64] = batch.i64(0);              // a whole column, as the core wrote it
    let verdicts = batch.verdicts(0);            // and a verdict beside each value
    for row in 0..batch.rows() {
        let amount: Result<f64, Fault> = batch.get(1, row);
        let name = batch.text(2, row);           // a slice of the input
    }
}

let book = Workbook::open("data.xlsx")?;         // XLSX or ODS, told by what is in it
let mut sheet = book.sheet("Data", SheetOptions::default(), &plan)?;
while let Some(batch) = sheet.read()? {
    // the same batch
}
# Ok::<(), hypertabular::Error>(())
```

A cell that does not cast is never an error: it is a `Fault` in the batch — `Empty`,
`Malformed` or `OutOfRange`, with the span of the offending bytes — and `batch.raw` gives
the text it points into. Structural failures (a torn row, a corrupt zip) are `Error`s,
returned once every intact row before them has been delivered.

**WebAssembly.** The crate needs nothing from its host — no clock, no entropy, no
allocator in the core — so it builds for `wasm32-unknown-unknown` as it is, and its whole
suite passes under wasmtime on `wasm32-wasip1`. CI runs `browser-test/` in headless Chrome
on every pull request.

The design record is in [`docs/`](https://github.com/SkunkWerkx/HyperTabular/tree/master/docs).
