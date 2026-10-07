# HyperTabular for .NET

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for every cell.

```csharp
using System.Text;
using HyperCast;
using HyperTabular;

Column[] plan = [Column.Int32(0), Column.Text(1), Column.Double(2)];
using var reader = DelimitedReader.Open("orders.csv", Dialect.Csv, plan);

while (reader.Read() is { } batch)
{
    // A column at a time, as the core wrote it…
    ReadOnlySpan<int> ids = batch.Values<int>(0);
    ReadOnlySpan<CellVerdict> verdicts = batch.Verdicts(0);

    // …or a cell at a time, as HyperCast's union.
    for (var row = 0; row < batch.Rows; row++)
    {
        var line = batch.Get<double>(2, row) switch
        {
            Success<double> score => $"{batch.GetString(1, row)}: {score.Value}",
            Fault fault => $"line {batch.Line(row)}: {fault.Reason} in \"{Encoding.UTF8.GetString(batch.Raw(2, row))}\"",
        };
    }
}

// A workbook reads into the same batch.
using var book = Workbook.Open("orders.xlsx");
var sheet = book.Sheet("Orders", SheetOptions.Default, plan);
while (sheet.Read() is { } batch) { /* … */ }
```

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — the input, one value array and one verdict array per
column, the table that locates each cell — once, pinned, and reuse them for every batch. The
core fills them in one native call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **One batch type.** `Read()` returns a `Batch` — `Rows`, `Columns`, `Line(row)`,
  `Verdicts(column)`, `Values<T>(column)` for a whole primitive column, `Get<T>(column, row)`
  for any one cell, `TryGetText`/`GetString` for text, and `Raw` for the text a cell was
  cast from — or `null` when there are no more rows. It is a view of the reader's buffers,
  valid until the next `Read()`, and the same type for delimited text and for a sheet.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header and whether empty rows are skipped; the plan states
  each column's door and, for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Verdict<T>`, `Fault`, `NumFormat`, `UnixPrecision`,
  `DateOrder` and `ExcelEpoch` are HyperCast's own types, from its package. A text cell
  means exactly what `Cast` would say of the same text; a typed workbook cell is converted
  by the door directly (a stored `2.5` through the decimal door is `2.5`).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on. A record of the wrong width, input that
  ends inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `TabularException`, raised after every intact row before it.
- **Text is zero-copy.** `TryGetText` hands back a span into the reader's own buffer — the
  input, or a workbook's shared strings; only a cell with `""` inside, or a typed workbook
  cell said as text, is written to an arena.
- **Native AOT.** Source-generated `LibraryImport` only. An AOT publish links the core in
  from the package's static archive: one executable, nothing loaded at start.
- **iOS and Mac Catalyst.** A `net11.0-ios` or `net11.0-maccatalyst` app (MAUI included)
  needs nothing but the package reference: its `buildTransitive` targets link the core in
  from static archives for `ios-arm64`, `iossimulator-arm64`, `maccatalyst-arm64` and
  `maccatalyst-x64`, since neither platform loads a library.

`Tabular.IsAvailable` and `Tabular.NativeVersion` answer whether the native library
resolved, without the first read being what finds out.

## WebAssembly (Blazor)

One compiled assembly covers browser-wasm too: every native entry point is declared three
times — `"hypertabular"` for platforms that load a library, `"*"` for the statically linked
wasm module and `"__Internal"` for iOS and Mac Catalyst — picked at the call site by
`OperatingSystem.IsBrowser()` and `OperatingSystem.IsIOS()`. CI builds the
`wasm32-unknown-emscripten` archive on every pull request, the release pack stages it under
`runtimes/browser-wasm/nativeassets/`, and the package's `build/net11.0/HyperTabular.targets`
wires it into a Blazor WebAssembly project with no configuration: a `NativeFileReference`
hands the archive to the linker beside HyperCast's (the two share no symbols), an
`EmccExportedFunction` per export makes each resolvable through `"*"`, and the
exception-handling translation .NET 11 needs is applied.

No export takes more than twelve integer arguments, because Mono's interpreter, which runs
.NET in the browser, passes no more to a native function; the work buffers travel in one
struct instead. `HyperTabular.WasmSmokeTest` proves the chain in headless Chrome on every
pull request: a Blazor app that reads delimited text and a workbook through the public API
and renders `PASS` or `FAIL` into the page.
