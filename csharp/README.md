# HyperTabular for .NET

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for every cell.

```sh
dotnet add package HyperTabular
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

```csharp
using System.Diagnostics;
using System.Linq;
using System.Text;
using HyperCast;
using HyperTabular;

// Open, read the header, find each column by name, then declare the plan.
using var reader = DelimitedReader.Open("people.csv", Dialect.Csv);
var header = reader.Header!;
Console.WriteLine($"header: [{string.Join(", ", header)}]");
reader.Bind([
    Column.Int32(header.Ordinal("id")),
    Column.Text(header.Ordinal("name")),
    Column.Double(header.Ordinal("score")),
]);

while (reader.Read() is { } batch)
{
    // A column at a time, as the core wrote it…
    ReadOnlySpan<int> ids = batch.Values<int>(0);
    ReadOnlySpan<CellVerdict> verdicts = batch.Verdicts(2);
    Console.WriteLine($"ids: [{string.Join(", ", ids.ToArray())}]");
    Console.WriteLine($"faults: {verdicts.ToArray().Count(verdict => !verdict.IsOk)}");

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
        Console.WriteLine(line);
    }
}

// A plan whose positions are known is given up front, and bound before the header is read.
using var byPosition = DelimitedReader.Open(
    "people.csv", Dialect.Csv, [Column.Int32(0), Column.Text(1), Column.Double(2)]);
foreach (var row in byPosition.Rows()) // every row, batch after batch
{
    Console.WriteLine($"line {row.Line}: {row.GetString(1)}");
}

// A workbook reads into the same batch, from a path, memory or a stream.
using var book = Workbook.Open("people.xlsx");
Console.WriteLine($"sheets: [{string.Join(", ", book.Sheets.Select(info => info.Name))}]");
var sheet = book.Sheet("People", SheetOptions.Default);
sheet.Bind([Column.Text(sheet.Header!.Ordinal("name")), Column.Decimal(sheet.Header.Ordinal("score"))]);
foreach (var row in sheet.Rows())
{
    var line = row.Get<decimal>(1) switch
    {
        Success<decimal> score => $"{row.GetString(0)}: {score.Value}",
        Fault fault => $"line {row.Line}: {fault.Reason}",
        null => throw new UnreachableException(),
    };
    Console.WriteLine(line);
}
```

The `switch` over a cell's verdict is C# 15: a .NET 11 project's default, and
`<LangVersion>preview</LangVersion>` in a .NET 10 one. On C# 14, the .NET 10 default,
HyperCast's `Match`, `Switch` or `TryGetValue` read the same verdict, one delegate or
`out` per case ([HyperCast's README](https://github.com/SkunkWerkx/HyperCast/tree/master/csharp#without-c-15)).

A plan whose positions are known can still be given up front —
`DelimitedReader.Open(path, Dialect.Csv, plan)`, `book.Sheet(0, SheetOptions.Default, plan)` —
and is bound before the header is read.

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```csharp
using System.Linq;
using System.Text;
using HyperCast;
using HyperTabular;

// Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
var dialect = Dialect.Csv with
{
    Skip = 1,
    ShortRows = ShortRows.Pad,
    Encoding = TextEncoding.Windows1252,
    Tokens = new Tokens("NA"),
};

byte[] input = [.. "Exported by hand\nname,amount\ncaf"u8, 0xE9, .. ",NA\ntea\nmilk,12x4\n"u8];
using var reader = new DelimitedReader(input.AsMemory(), dialect, [
    Column.Text(0),
    Column.Double(1).WithTokens("NA", "-"), // in place of the dialect's
]);
var batch = reader.Read()!;
foreach (var row in batch)
{
    Console.WriteLine($"{row.GetString(0)} {row.Verdict(1).Reason} {row.Origin(1)} {row.Fault(1)?.Detail}");
}
// What went wrong, and only that: a blank, a token and a padded cell aren't problems.
foreach (var problem in batch.Faults())
{
    Console.WriteLine($"line {problem.Line} column {problem.SourceColumn} {problem.Reason} {problem.Detail}");
}

// A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
var found = DelimitedReader.Suggest(
    "id,total,day\n1,\"1,234.50\",2026-01-07\n2,12.00,2026-01-08\n"u8.ToArray(), Dialect.Csv);
foreach (var column in found.Report)
{
    Console.WriteLine($"{column.Name} {column.Status} {column.Door} {column.Matched} of {column.Sampled}");
}

// A sheet takes the part of it that's the table; its plan's ordinals count from B here.
var options = SheetOptions.Default with { Range = SheetRange.Parse("B2:D40") };
Console.WriteLine($"{options.Range.FirstColumn} {options.Range.LastRow}");
```

A `TabularException` of `TabularFailure.Encoding` comes after every intact row before it, and its
`Byte` alone counts the original bytes: a read's spans are all the decoded text's. A sheet's `Range`
is parsed by the core, so `SheetRange.Parse("A5:F200")` means what it means in every binding, with
`new SheetRange(firstRow, firstColumn, lastRow, lastColumn)` for what that can't say, such as every
row from the fifth (`new SheetRange(5, 0, 0, 0)`). A plan's ordinals count from the range's first
column, while `Batch.Line` and a problem's `SourceColumn` stay the sheet's own. `Skip` and `HeaderRow`,
on a `Dialect` and on `SheetOptions`, count from zero; `SkipEmptyRows` and `SkipBlankLines` work on what
they leave.

`Notes = false`, on a `Dialect` or `SheetOptions`, drops the detail on a `Fault` and `Origin`, which
then say `FaultDetail.None` and `Origin.Cell`: the read costs a little less, and `Faults` can't list a
formula with no cached value. `Column.WithDateOptions` hands a date door HyperCast's `DateOptions`, and
on a sheet it and `WithTokens` reach only a text cell. A cell's detail is on `batch.Fault(column, row)`,
`row.Fault(column)`, `Get<T>`'s `Fault` and `Faults()`; `CellVerdict.ToFault()` has only what the verdict
holds. `Faults()` belongs to its batch: going on with it after the next read throws
`InvalidOperationException`.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous and read
as text until a format is declared or another cell settles them, which takes in a column of prices at
three decimals; `SuggestOptions.DateOrder` does the same for `01/02/2026`. `2,5` beside `2.5` is
unresolved, and `1,23,456`, Indian grouping, is text until the column declares Indian grouping, because
nothing says its culture. A sheet's number cell has no notation, so a column of them is a number under
any format, where text holding the same digits asks. A column of `0` and `1` is `bool`, `007` and an
integer past `long` are text, and a typed date in a sheet is text, for a plan to change by hand. With
nothing to choose between, a resolved number is planned in `,` groups of three, and
`SuggestedColumn.Format` names what the plan uses: a later `1,23,456` is refused.
`DelimitedReader.Suggest` takes a path or memory. A headerless file is opened twice, so no suggestion
is offered of a stream. `Sheet.Suggest` answers from the sheet's first row whatever has been read of it,
and leaves it unbound and where it was. A failure past a whole sample, an `IOException` or a
`TabularException`, only sets `Truncated`; one among the sampled rows is thrown, and so is a cancellation
or a `ThreadInterruptedException` wherever it comes.

**To keep every row of a sheet, set `SkipEmptyRows` to `false`.** By default a row whose only cells
are formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `Origin.Uncached`.

A column given no format reads `NumFormat.Invariant`, which is Western: `1,234,567` is 1234567 and
`1,23,456` is `Malformed`, and a format that names no policy is the same. If a column of Indian numbers
comes back refused, declare it, one column at a time, so an Indian column reads beside a Western one in
the same read:

```csharp
using HyperCast;
using HyperTabular;

var indian = NumFormat.Invariant with { Grouping = NumGrouping.Indian };
Column[] plan =
[
    Column.Int32(0, indian),                // 1,23,456 is 123456, and 1,234,567 is Malformed
    Column.Int32(1, NumFormat.Invariant),   // 1,234,567 is 1234567, and 1,23,456 is Malformed
];
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat.Detect` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column.Double` reads `1,2345` as
1.2345, `12,34` as 12.34 and `0,125` as 0.125. A format holding the retired grouping bit `256` (the words
`351` and `863`), which only a reflective decoder can make, is an `ArgumentException` when the plan is
bound, before any cell, naming the plan column, with HyperCast's refusal as its inner exception, and in
a suggest's options before it opens anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape and
the locales neither policy matches.

## Constraints

A numeric or text column can declare HyperCast's `Constraints<T>`, the OpenAPI 3.2 keywords
(`minimum`, `exclusiveMinimum`, `maximum`, `exclusiveMaximum`, `multipleOf` with a `Rounding`,
`minLength`, `maxLength`, `enum`, with `trim` and `caseSensitive` for text), through the factory of
its door. Each cell is cast first, so text that isn't the type is a `Fault` as ever, and then judged:

```csharp
using HyperCast;
using HyperTabular;

using var reader = new DelimitedReader("qty,sex\n12,MALE\n7,x\n"u8.ToArray().AsMemory(), Dialect.Csv,
[
    Column.Int32(0, new Constraints<int>(minimum: 1, maximum: 10)),
    Column.Text(1, new Constraints<string>(@enum: ["Female", "Male"])),
]);
var batch = reader.Read()!;
foreach (var row in batch)
{
    var qty = row.GetChecked<int>(0) switch
    {
        Checked<int> passed => $"{passed.Value}",
        Rejection<int> { Value: Violation<int> broken } => $"{broken.Value} breaks {broken.Keyword}",
        Rejection<int> { Value: Fault fault } => $"{fault.Reason}",
    };
    var sex = row.GetChecked<string>(1) switch
    {
        Checked<string> passed => passed.Value, // "MALE" matches the entry, and reads as "Male"
        Rejection<string> rejected => $"{rejected}",
    };
    Console.WriteLine($"{qty} {sex}");
}
foreach (var problem in batch.Faults())
{
    Console.WriteLine($"line {problem.Line} column {problem.SourceColumn} {problem.Keyword}");
}
```

A violation isn't a fault: the cell cast, so `Fault(column, row)` is `null`, `CellVerdict.IsViolation`
says so, the value is kept exactly as cast (never rounded), and the span is the judged text's. A cell
`multipleOf` rounded passes with `Adjusted` set, and a text `enum` match reads as the entry as declared,
the caller's own string, with `EnumIndex` naming it. `Faults()` lists a violation with its `Keyword` and
no `Reason`. The constraints' type is the door's, so a column can't pair one door with another's, and
`Get<T>`, `TryGetText` and `GetString` refuse a constrained column with `InvalidOperationException`:
they would read a violation as passed. `GetChecked<T>` reads an unconstrained column too, where every
cell that cast passes, unadjusted. HyperCast judges the declaration when it is built, and refuses only
one its core cannot read.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — the input, one value array and one verdict array per
column, the table that locates each cell — once, pinned, and reuse them for every batch. The
core fills them in one native call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **Header first.** A `DelimitedReader` opened without a plan — from a `Stream`,
  `ReadOnlyMemory<byte>` or a path — and a sheet started without one
  (`book.Sheet(index or name, options)`) read the header straight away. `Header` is the list
  of names, with `Ordinal(name)` (exact, case-sensitive, untrimmed; `KeyNotFoundException`
  naming the column when it is missing, and its subclass `DuplicateColumnException`, naming both
  positions, when two columns share the name), `TryOrdinal`, which takes the first of those, and
  `Ordinal(ReadOnlySpan<byte>)` to match the raw UTF-8. `Bind(plan[, batchRows])` then
  declares the plan, once (`InvalidOperationException` on a second call, and on `Read()`
  before it — not final: bind and read). `IsBound` says which; `ColumnCount` is the header's
  width, or the first record's once read. A delimited plan column past the header's width
  could only ever be empty, so it is the plan's `ArgumentException`, naming the column,
  whether the plan is bound or came with the reader. Without a header, a column past a
  record's last cell reads as `Empty`, and a plan reaching past 256 source columns reads
  shorter batches, as many rows as about 8 MiB of cell table holds. An ordinal must be below
  2²⁰ (1,048,576), in a sheet's plan as in a reader's: no workbook format has that many
  columns (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell
  table is at most 8 MiB. Past it is the plan's `ArgumentException` too.
- **One batch type.** `Read()` returns a `Batch` — `Rows`, `Columns`, `Line(row)`,
  `Verdicts(column)`, `Values<T>(column)` for a whole column, `Get<T>(column, row)` for any
  one cell, `GetChecked<T>(column, row)` for one a constrained column judged, `Fault` and
  `Origin` for what a cell's verdict says beside itself, `Faults()` for what went wrong in the
  batch, `TryGetText`/`GetString` for text, and `Raw` for the text a cell was
  cast from —
  or `null` when there are no more rows. It is a view of the reader's buffers, valid until
  the next `Read()`, and the same type for delimited text and for a sheet.
- **Whole columns as the core wrote them.** `Values<T>` lends the primitive doors as their
  own type, the time door as `ulong` nanoseconds since midnight, and the decimal and other
  temporal doors as HyperCast's own `HyperCast.Interop` layouts — `RawDecimal`,
  `RawTimestamp` (timestamp, Unix, Excel serial), `RawDate`, `RawCivil` and `RawDuration` —
  each of which converts itself to what `Get<T>` makes. A UUID column has no span: the core
  writes RFC 9562 byte order, which is not `Guid`'s layout, so it is read with `Get<Guid>`.
- **Rows, when that is the shape.** `foreach (var row in batch)` visits each `Row` — a
  `ref struct` view of (batch, index) with `Index`, `Line`, `Get<T>(column)`, `GetChecked<T>(column)`,
  `Verdict(column)`, `Fault(column)`, `Origin(column)`, `TryGetText`, `GetString`, `GetChars`,
  `TryGetChars` and `Raw` —
  allocating nothing; `batch.Row(i)` picks one. Going on with a loop after the next `Read()` throws
  `InvalidOperationException`, rather than answer for the next batch; the check costs nothing
  measurable (BenchmarkDotNet, .NET 11 RC 3, Apple M6, 2026-11-08, two interleaved runs with and
  without it: 930 against 941 ns to walk 4096 rows, and 5.99 against 5.96 ms to read and walk a
  200 000-row file, both within the run's error). `reader.Rows()` and `sheet.Rows()` go on
  across batches, reading as each runs out; a row is valid for its own iteration only.
- **The core decides how much a batch may ask for.** A `batchRows` or `bufferBytes` of 0, and a batch
  past 4 GiB (214 million rows of one `Int32` column, 1.8 million of 100 text columns), are an
  `ArgumentOutOfRangeException` from the core, when a reader or a sheet opens or a plan is bound, before
  anything is allocated. A `bufferBytes` past 1 GiB starts there. One array past what .NET holds in one
  is refused the same way, naming that. `Suggest` is never refused for the size of its own batches: the core
  says what fits and the sampler reads as many as it takes. A row that alone is past the limit is an
  `ArgumentException` naming its width.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and the
  encoding; `SheetOptions` states a sheet's header, its range and whether empty rows are skipped;
  the plan states each column's door and, for numbers, its `NumFormat`. `Suggest` reads a sample
  to propose a plan, only when asked, and applies nothing.
- **HyperCast is the judge.** `Verdict<T>`, `Fault`, `FaultDetail`, `NumFormat`, `DateOptions`,
  `UnixPrecision`, `DateOrder` and `ExcelEpoch` are HyperCast's own types, from its package. A text cell
  means exactly what `Cast` would say of the same text; a typed workbook cell is converted
  by the door directly (a stored `2.5` through the decimal door is `2.5`).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on, and `Batch.Faults` lists a batch's. A record of the wrong width, input that
  ends inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `TabularException`, raised after every intact row before it.
- **Text is zero-copy.** `TryGetText` hands back a span into the reader's own buffer — the
  input, or a workbook's shared strings; only a cell with `""` inside, or a typed workbook
  cell said as text, is written to an arena. Text in a declared encoding is decoded into a buffer
  of the reader's, so its spans are into that, even from memory.
- **UTF-16 without strings.** `GetChars(column, row)` decodes a text cell into a
  `ReadOnlySpan<char>` in an arena the batch keeps — made on first use, only ever appended
  to while the batch is current, started over at the next `Read()` — for the BCL's
  `ReadOnlySpan<char>` parsers (`Enum.Parse<T>`, `ISpanParsable<T>`); an empty cell is an
  empty span. `TryGetChars(column, row, destination, out written)` decodes into the
  caller's buffer instead. A cell whose bytes aren't UTF-8 is `Malformed`, with no text and no chars,
  and `Raw` still has its bytes.
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
- **A stream's code runs in the middle of a read.** A call from it back into its reader —
  `Read`, `ReadAsync`, `Bind`, a loop over `Rows()` — throws `InvalidOperationException` ("The
  reader is in the middle of a read; …"), and the read it came from goes on whole. A second read
  or a bind while a `ReadAsync` is still waiting on its stream is refused the same way. `Dispose`
  is never refused: from the stream, or while a `ReadAsync` is still waiting on it, it disposes the
  reader at once, and that read ends with `ObjectDisposedException` ("The reader was disposed in
  the middle of a read.") when the stream returns. It hands out no batch, and every later call
  throws `ObjectDisposedException`. A `Read` inside `foreach (var row in reader.Rows())`
  makes the loop's next step throw, as a batch's own loop does. What the stream throws reaches the
  caller and isn't final: the next read goes on from where the reader was.
- **.NET 10 and 11.** The package builds `net10.0` and `net11.0` from one source, and package
  validation holds their public APIs identical. On .NET 10 it serves Windows, Linux and
  macOS, framework-dependent or Native AOT, and Blazor WebAssembly. iOS, Mac Catalyst and
  Android need .NET 11: a `net10.0-ios`, `net10.0-maccatalyst` or `net10.0-android` project
  stops at build with `HYPERTABULAR003`, which names the platform and the framework to
  target. A project older than .NET 10 is refused at restore (NU1202).
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
resolved, without the first read being what finds out. `NativeVersion` is a `System.Version`
(the core's own version, `null` when the library did not resolve), not a string.

## Android

A .NET for Android or MAUI app (`net11.0-android`, API 24 and later; Android takes .NET 11,
and a `net10.0-android` build stops with `HYPERTABULAR003`) references the package and
writes nothing else. On CoreCLR, .NET 11's Android runtime, the
SDK takes `runtimes/android-arm64/native/libhypertabular.so` and its x64 twin out of the
package and stores each in the APK under `lib/arm64-v8a/` and `lib/x86_64/`, and the ordinary
`"hypertabular"` import opens it, exactly as on Linux. The libraries are cross-built with the
NDK for API level 21, below any app that can reference them, and their segments are aligned
to 16 KB: Android 15 devices may use 16 KB pages, a library aligned for 4 KB does not load on
one, and Google Play requires the alignment of every new app. A Native AOT publish
(`PublishAot`, `-r android-arm64`) links `staticlibs/android-{rid}/libhypertabular.a` into
the app's own native library instead, as it does on every other RID, and the shared one is
left out of the APK. HyperCast's package does the same for its own core, and the two sit side
by side. `android-arm64` covers effectively every Android device in use and `android-x64` the
emulator; these are the two RIDs .NET for Android builds by default. The 32-bit `android-arm`
and `android-x86` are not in the package, so an app that adds them gets `Tabular.IsAvailable
== false` on those ABIs.

`HyperTabular.AndroidSmokeTest` is the Native AOT smoke test's `SmokeTest.Run()` again,
started from an Activity, and unlike the other smoke tests it takes HyperTabular as a
package, from a local folder CI packs it into, because the package's layout is what Android
needs proven. CI's `test-android` job builds it four ways from that run's libraries: CoreCLR
and Native AOT, for each RID. The x64 pair runs in an x86_64 emulator whose image uses 16 KB
pages, so a library aligned for 4 KB would fail to load there. Nothing hosted runs arm64
Android, so the arm64 pair is inspected instead: each CoreCLR APK must carry
`libhypertabular.so` for its ABI, each Native AOT APK must not carry it at all, and all four
must pass `zipalign -P 16`.

## WebAssembly (Blazor)

One compiled assembly covers browser-wasm too: every native entry point is declared three
times — `"hypertabular"` for platforms that load a library, `"*"` for the statically linked
wasm module and `"__Internal"` for iOS and Mac Catalyst — picked at the call site by
`OperatingSystem.IsBrowser()` and `OperatingSystem.IsIOS()`. CI builds the
`wasm32-unknown-emscripten` archive on every pull request, the release pack stages it under
`runtimes/browser-wasm/nativeassets/`, and the package's `build/net10.0/HyperTabular.targets`
wires it into a Blazor WebAssembly project with no configuration: a `NativeFileReference`
hands the archive to the linker beside HyperCast's (the two share no symbols), the SDK's
generated pinvoke table calls each export by symbol so `"*"` resolves every one (with the
archive missing, `wasm-ld` reports an undefined symbol per export). No
`EmccExportedFunction` list is needed: the smoke test calls every export a read takes through
that table alone, and fails at the link with the archive unlinked. The archive reaches the
linker by that path and nowhere else: the same targets file takes it back out of restore's
copy-local list, so a consumer's output carries only the linked `dotnet.native.wasm`.

That link happens whether or not the app ends up reading anything. The targets file is
evaluated before the trimmer runs and cannot see what it removes, so a client bundle whose
trimmer drops `HyperTabular` entirely still links the archive and all its entry points into
`dotnet.native.wasm`, as HyperCast's package does for its own.

**WebAssembly on .NET 10 and later, as linked.** The static library is built with
`-Zbuild-std` (on stable Rust, with `RUSTC_BOOTSTRAP=1` for that one build), for plain wasm
(MVP plus `sign-ext` and `mutable-globals`), instead of on Rust's prebuilt sysroot, as
HyperCast's is. So it holds no exception-handling instructions, and a browser never sees
.NET 11's encoding mixed with the sysroot's legacy one (`module uses a mix of legacy and new
exception handling instructions`), whether or not the SDK runs `wasm-opt` after the link.
And it uses no feature but those two, Binaryen's defaults, which every `wasm-opt` run in a
.NET 10 publish accepts with no flag: Emscripten's does not know `bulk-memory-opt` or
`call-indirect-overlong` (dotnet/runtime#132858), and the SDK's own, after the link, rejects a
saturating float-to-int conversion. The archive is checked for both before it is packed.

**Needs the `wasm-tools` workload.** Only the workload's toolchain relinks the runtime.
Without it the SDK skips the link with nothing louder than its own warning, and the publish
succeeds with a `dotnet.native.wasm` that has no `hypertabular_*` symbols in it; `NETSDK1147`
does not fire for this. So the targets file stops the build with error `HYPERTABULAR002`
instead (`dotnet workload install wasm-tools`; a `net10.0` app built with the .NET 11 SDK
takes `wasm-tools-net10`). An app that gates on `Tabular.IsAvailable` and keeps a managed
fallback can set
`<HyperTabularRequireWasmWorkload>false</HyperTabularRequireWasmWorkload>`.

No export takes more than twelve integer arguments, because Mono's interpreter, which runs
.NET in the browser, passes no more to a native function; the work buffers travel in one
struct instead. `HyperTabular.WasmSmokeTest` proves the chain in headless Chrome on every
pull request: a Blazor app that reads delimited text and a workbook through the public API
and renders `PASS` or `FAIL` into the page. One of the files it reads is one it serves,
fetched with `HttpClient` as a streamed response — a stream the browser only reads
asynchronously — and read through `OpenAsync` and `ReadAsync`. `./check.sh` in that
directory stages the wasm static library, publishes the app four ways (each framework, in
Release and in an unstripped Debug build, where the SDK runs no `wasm-opt` after the link),
loads each in headless Chromium and requires `PASS`; name legs (`net11-release net11-debug
net10-release net10-debug`) to run fewer. Each publish links HyperCast's archive from its
package beside this one.

## Native binary provenance

The `.nupkg` carries compiled native code, which is a real thing to ask questions about before adopting it inside a trust boundary. Nothing under `csharp/HyperTabular/runtimes/` is committed: the native libraries are built from `rust/` by CI and staged into the package at pack time, so what ships is produced by the same workflow run that built and tested the source. The core is a normal Rust crate with no build-time codegen (`cd rust && cargo cdylib`), so you never have to take the shipped binary at all, but a rebuild is not bit-reproducible across machines, and the mechanism that does work is a cryptographic attestation binding each artifact to the workflow run and commit that produced it. Two kinds cover the package.

The *release attestation* is one statement, signed by this repository's `release.yml` at the release tag, whose subjects are every file the release ships — the `.nupkg` as packed and as nuget.org serves it, every other registry package, and the files on the GitHub Release (Swift's two zips, the C archives and the PHP extension's zips) — and whose predicate is the release inventory (`inventory.json`: every file's size, SHA-256 and the commit it was built from). One command verifies any of them from the tag alone:

```shell
gh attestation verify HyperTabular.<version>.nupkg --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

*Build provenance* ([SLSA](https://github.com/actions/attest-build-provenance)) is the second layer, emitted where the bytes are made, which for this package is four places, because it is not the same bytes at every stage of its life:

| Attested artifact | Where | Signer |
| --- | --- | --- |
| Each native library, as built | `hyper-build-native.yml` | `SkunkWerkx/.github` |
| The browser-wasm archive, as built | `hyper-build-wasm.yml` | `SkunkWerkx/.github` |
| The `.nupkg` as packed, pre-push | `hyper-pack-nuget.yml` | `SkunkWerkx/.github` |
| The `.nupkg` as published | `release.yml`, after the push | `SkunkWerkx/HyperTabular` |

The reason for the last two rows: **nuget.org adds its repository signature as a `.signature.p7s` entry inside the `.nupkg` zip during validation**, which changes the file's SHA-256. So the package you download is not the package that was built, and one attestation cannot cover both. The pipeline takes both, and the release attestation is made for both: once in the publication gate over the packed bytes, once after the push over the served bytes, with the same inventory as predicate. The signer column is the flag that changes: an artifact signed by a reusable workflow in `SkunkWerkx/.github` needs `--signer-repo SkunkWerkx/.github` beside `--repo` (or `--owner SkunkWerkx` in place of both); one signed by this repository's `release.yml` verifies with `--repo` alone. Get it wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which reads like a bad signature but is only an identity mismatch.

```shell
# the published bytes, signed by release.yml
gh attestation verify HyperTabular.<version>.nupkg --repo SkunkWerkx/HyperTabular

# the as-built package, recovered by removing nuget.org's one added entry; signed by the Forge
zip -d HyperTabular.<version>.nupkg .signature.p7s
gh attestation verify HyperTabular.<version>.nupkg \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
```

The release run downloads the package nuget.org serves, runs `dotnet nuget verify` on it, and requires every entry but `.signature.p7s` to match the packed package before attesting it. That claim is checked on every release rather than asserted here, so if nuget.org ever changes how it finalizes packages, the run says so instead of the README quietly going stale.

Attestations are produced by every release-mode build — the release's own run and the weekly one. Pull requests build in `pr` mode, which ships nothing and so signs nothing. The release itself is two acts: the preparation run packs, attests and preserves everything, and the tag push verifies those preserved bytes and publishes, waiting up to 20 minutes for nuget.org to serve the package before attesting the served bytes; the Go tag and GitHub Release wait on it. A slower validation is recovered by dispatching `release.yml` again at the tag with `publication_run_id`, which pushes nothing twice (see [RELEASING.md](../.github/RELEASING.md)). The root README's [Provenance](../README.md#provenance) section has the whole picture across every binding.

**Not currently done: NuGet author signing.** The package carries nuget.org's repository signature but no author signature of our own, which would need an X.509 code-signing certificate registered to the account. An author signature is verified automatically by every consumer's SDK at restore time, whereas an attestation is only checked by someone who deliberately runs `gh attestation verify`; if you want the automatic restore-time check, this is the gap.
