# HyperTabular for .NET

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for every cell.

```csharp
using System.Diagnostics;
using System.Text;
using HyperCast;
using HyperTabular;

// Open, read the header, find each column by name, then declare the plan.
using var reader = DelimitedReader.Open("orders.csv", Dialect.Csv);
var header = reader.Header!;
reader.Bind([
    Column.Int32(header.Ordinal("id")),
    Column.Text(header.Ordinal("customer")),
    Column.Double(header.Ordinal("score")),
]);

while (reader.Read() is { } batch)
{
    // A column at a time, as the core wrote it…
    ReadOnlySpan<int> ids = batch.Values<int>(0);
    ReadOnlySpan<CellVerdict> verdicts = batch.Verdicts(0);

    // …or a row at a time, each cell HyperCast's union.
    foreach (var row in batch)
    {
        var line = row.Get<double>(2) switch
        {
            Success<double> score => $"{row.GetString(1)}: {score.Value}",
            Fault fault => $"line {row.Line}: {fault.Reason} in \"{Encoding.UTF8.GetString(row.Raw(2))}\"",
            // Only default(Verdict<T>) is null, and no read returns one; the arm is for the
            // compiler's null analysis (CS8655), and the two above stay exhaustive (CS8509).
            null => throw new UnreachableException(),
        };
    }
}

// A workbook reads into the same batch, from a path, memory or a stream.
using var book = Workbook.Open("orders.xlsx");
var sheet = book.Sheet("Orders", SheetOptions.Default);
sheet.Bind([Column.Int32(sheet.Header!.Ordinal("id")), Column.Text(sheet.Header.Ordinal("customer"))]);
foreach (var row in sheet.Rows()) { /* … */ }
```

A plan whose positions are known can still be given up front —
`DelimitedReader.Open(path, Dialect.Csv, plan)`, `book.Sheet(0, SheetOptions.Default, plan)` —
and is bound before the header is read.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — the input, one value array and one verdict array per
column, the table that locates each cell — once, pinned, and reuse them for every batch. The
core fills them in one native call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **Header first.** A `DelimitedReader` opened without a plan — from a `Stream`,
  `ReadOnlyMemory<byte>` or a path — and a sheet started without one
  (`book.Sheet(index or name, options)`) read the header straight away. `Header` is the list
  of names it always was, plus `Ordinal(name)` (exact, case-sensitive, untrimmed, first match;
  `KeyNotFoundException` naming the column when it is missing), `TryOrdinal`, and
  `Ordinal(ReadOnlySpan<byte>)` to match the raw UTF-8. `Bind(plan[, batchRows])` then
  declares the plan, once (`InvalidOperationException` on a second call, and on `Read()`
  before it — not final: bind and read). `IsBound` says which; `ColumnCount` is the header's
  width, or the first record's once read.
- **One batch type.** `Read()` returns a `Batch` — `Rows`, `Columns`, `Line(row)`,
  `Verdicts(column)`, `Values<T>(column)` for a whole primitive column, `Get<T>(column, row)`
  for any one cell, `TryGetText`/`GetString` for text, and `Raw` for the text a cell was
  cast from — or `null` when there are no more rows. It is a view of the reader's buffers,
  valid until the next `Read()`, and the same type for delimited text and for a sheet.
- **Rows, when that is the shape.** `foreach (var row in batch)` visits each `Row` — a
  `ref struct` view of (batch, index) with `Index`, `Line`, `Get<T>(column)`,
  `Verdict(column)`, `TryGetText`, `GetString`, `GetChars`, `TryGetChars` and `Raw` —
  allocating nothing; `batch.Row(i)` picks one. `reader.Rows()` and `sheet.Rows()` go on
  across batches, reading as each runs out; a row is valid for its own iteration only.
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
- **UTF-16 without strings.** `GetChars(column, row)` decodes a text cell into a
  `ReadOnlySpan<char>` in an arena the batch keeps — made on first use, only ever appended
  to while the batch is current, started over at the next `Read()` — for the BCL's
  `ReadOnlySpan<char>` parsers (`Enum.Parse<T>`, `ISpanParsable<T>`); an empty cell is an
  empty span. `TryGetChars(column, row, destination, out written)` decodes into the
  caller's buffer instead. Invalid UTF-8 is replaced exactly as `GetString` replaces it.
- **Workbooks from streams.** `new Workbook(stream, leaveOpen)` / `Workbook.Open(stream)`
  read an embedded resource or a response to its end into pinned memory the workbook owns —
  exactly the remaining length of a seekable stream, more than `int.MaxValue` refused as
  `TabularFailure.TooLarge` before anything is allocated — and dispose the stream as soon
  as it has been read, unless asked to leave it open.
- **Async, with cancellation.** `DelimitedReader.OpenAsync(stream, dialect, …)` awaits the
  header and `ReadAsync(cancellationToken)` awaits each refill (`Stream.ReadAsync`); the
  native fill between them is synchronous, and a reader of memory completes synchronously.
  The token is observed before anything is done and at every refill; a cancelled read
  throws `OperationCanceledException` and loses nothing — what had been read stays buffered,
  and the next read goes on from there. `Workbook.OpenAsync(stream, leaveOpen, token)`
  awaits a container; its sheets then read synchronously, from memory. `DelimitedReader` is
  `IAsyncDisposable`, disposing its stream asynchronously unless it was left open. This is
  what a browser's streams (an `HttpClient` response, a picked file), which cannot be read
  synchronously, need.
- **Native AOT.** Source-generated `LibraryImport` only. An AOT publish links the core in
  from the package's static archive: one executable, nothing loaded at start.
- **iOS and Mac Catalyst.** A `net11.0-ios` or `net11.0-maccatalyst` app (MAUI included)
  needs nothing but the package reference: its `buildTransitive` targets link the core in
  from static archives for `ios-arm64`, `iossimulator-arm64`, `maccatalyst-arm64` and
  `maccatalyst-x64`, since neither platform loads a library.
- **Android.** A `net11.0-android` app (MAUI included) needs nothing but the package
  reference either: the package carries the core for `android-arm64` and `android-x64`
  (API 21+), loaded out of the APK on CoreCLR and linked in under Native AOT. See
  [Android](#android).

`Tabular.IsAvailable` and `Tabular.NativeVersion` answer whether the native library
resolved, without the first read being what finds out.

## Android

A .NET for Android or MAUI app (`net11.0-android`, API 24 and later, .NET 11's floor)
references the package and writes nothing else. On CoreCLR, .NET 11's Android runtime (Mono
is no longer supported there), the SDK takes `runtimes/android-arm64/native/libhypertabular.so`
and its x64 twin out of the package and stores each in the APK under `lib/arm64-v8a/` and
`lib/x86_64/`, and the ordinary `"hypertabular"` import opens it, exactly as on Linux. The
libraries are cross-built with the NDK for API level 21, below any app that can reference
them, and their segments are aligned to 16 KB: Android 15 devices may use 16 KB pages, a
library aligned for 4 KB does not load on one, and Google Play requires the alignment of
every new app. A Native AOT publish (`PublishAot`, `-r android-arm64`) links
`staticlibs/android-{rid}/libhypertabular.a` into the app's own native library instead, as it
does on every other RID, and the shared one is left out of the APK. HyperCast's package does
the same for its own core, and the two sit side by side. `android-arm64` covers effectively
every Android device in use and `android-x64` the emulator; these are the two RIDs .NET for
Android builds by default. The 32-bit `android-arm` and `android-x86` are not in the package,
so an app that adds them gets `Tabular.IsAvailable == false` on those ABIs.

`HyperTabular.AndroidSmokeTest` is the Native AOT smoke test's `SmokeTest.Run()` again,
started from an Activity, and unlike the other smoke tests it takes HyperTabular as a
package, from a local folder CI packs it into, because the package's layout is what Android
needs proven. CI's `test-android` job builds it four ways from that run's libraries: CoreCLR
and Native AOT, for each RID. The x64 pair runs in an x86_64 emulator whose image uses 16 KB
pages, so a library aligned for 4 KB would fail to load there. Nothing hosted runs arm64
Android, so the arm64 pair is inspected instead: each CoreCLR APK must carry
`libhypertabular.so` for its ABI, each Native AOT APK must not carry it at all, and all four
must pass `zipalign -P 16`.

Two .NET 11 RC1 workload behaviors shape that job, and an app on RC1 may meet them too. The
android workload's build tasks require JDK 21 (`XA0030` on newer). And the workload RC1
resolves was built against a runtime newer than RC1 on nuget.org, so a Native AOT publish
fails to restore `Microsoft.NETCore.App.Runtime.NativeAOT.android-*` by exact version until
the .NET 11 daily feed (`https://pkgs.dev.azure.com/dnceng/public/_packaging/dotnet11/nuget/v3/index.json`)
is added as a source; the job adds it.

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
and renders `PASS` or `FAIL` into the page. One of the files it reads is one it serves,
fetched with `HttpClient` as a streamed response — a stream the browser only reads
asynchronously — and read through `OpenAsync` and `ReadAsync`.
