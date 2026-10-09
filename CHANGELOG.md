# Changelog

All eight packages in this repository — the `hypertabular` crate and the C#, Java, Go,
Python, Ruby, PHP and Swift bindings — share one coordinated version, so one changelog covers
all of them. Each entry marks which packages it actually affects.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] — 2026-10-09

### Added

- **Android for C#, Go and Swift**, arm64 and x86_64, as HyperCast and HyperUuid have it.
  The forge cross-builds the core with the NDK for API level 21 (`android`), its segments
  aligned to the 16 KB pages Android 15 devices may use and Google Play requires. *(C#, Go,
  Swift)*
  - C#: the package carries `runtimes/android-{arm64,x64}/native/libhypertabular.so`, which a
    .NET for Android or MAUI app on CoreCLR loads out of its APK, and
    `staticlibs/android-{arm64,x64}/libhypertabular.a`, which a Native AOT publish links in.
  - Go: `staticlib/android_arm64` and `staticlib/android_amd64`, linked under `GOOS=android`
    with the NDK's clang; Android no longer stops at compile time, and
    `.github/scripts/check_go_archives.sh` holds it to its own archives rather than the Linux
    pair.
  - Swift: the artifact bundle carries `aarch64-unknown-linux-android` and
    `x86_64-unknown-linux-android`, linked by the Swift SDK for Android (Swift 6.3+, API
    28+).
  - CI's new `test-android` job builds `HyperTabular.AndroidSmokeTest` on CoreCLR and under
    Native AOT for both RIDs from a package packed in that run, cross-builds the Go and
    Swift suites with both corpora, runs everything x86_64 in a 16 KB-page emulator, and
    inspects or links the arm64 builds. The Native AOT smoke test's body moved into
    `SmokeTest.cs` so the Android Activity can run it.
- **Header first, plan after; workbooks from streams; rows; text without strings; async**
  — the first downstream feedback (issues #8–#12, written against C#), carried into every
  binding the language allows. A reader or a sheet opens without a plan, reads its header,
  resolves names to ordinals (exact, first match, a missing name an error that names it) and
  binds the plan once; a workbook opens from a stream; a batch reads across as rows, and a
  reader or a sheet iterates rows across batches; C# and Java decode text to UTF-16 without
  a string, and Go views it as a string without a copy; C#, Swift, Python and Rust read
  asynchronously and cancellably, a cancelled read losing nothing. No ABI change: the core
  was always two-phase. `docs/design.md` ("The bindings, side by side") maps each feature to
  each language and says why the gaps are gaps. *(every package)*
- **Rust — header first, plan after.** `DelimitedReader::open_unbound`, `from_slice_unbound`
  and `from_reader_unbound` (and the same on `DelimitedOptions`), and `Workbook::sheet_unbound`,
  open a source and read its header without a plan; `bind(&plan)` declares it once, before the
  first read, and sizes every buffer then. A read before it is `Error::Unbound`, a second bind
  `Error::AlreadyBound`. `Header::ordinal` takes a `&str` as readily as bytes, and
  `Header::require` makes a missing name an `Error::NoColumn` that names it. The plan-taking
  constructors are unchanged. *(crate)*
- **Rust — `Workbook::from_reader`.** Opens a workbook from any `Read`, read to its end.
  *(crate)*
- **Rust — rows.** `Batch::row(i)` and `for row in batch` read a batch across: a `Row` has the
  batch's own accessors for its index, and `DelimitedReader::for_each_row` /
  `Sheet::for_each_row` run across batches. `Batch::text_str` (and `Row::text_str`) give a text
  cell as a `str`, borrowed unless the input was not UTF-8. *(crate)*
- **Rust — the `async` feature.** `DelimitedReader::from_async_reader` and `read_async`, and
  `Workbook::from_async_reader`, over `futures_io::AsyncRead`. The core still never waits; only
  the binding's refills are awaited, and a read whose future is dropped part-way loses nothing.
  *(crate)*
- **C# — header-first open and a deferred plan.** `DelimitedReader(Stream, Dialect,
  bufferBytes, leaveOpen)`, `DelimitedReader(ReadOnlyMemory<byte>, Dialect)`,
  `DelimitedReader.Open(path, Dialect, bufferBytes)`, `Workbook.Sheet(int, SheetOptions)` and
  `Workbook.Sheet(string, SheetOptions)` read the header without a plan;
  `DelimitedReader.Bind(plan, batchRows)` and `Sheet.Bind(plan)` declare it once, before the
  first read. `Read()` throws `InvalidOperationException` until then (not final), as does a
  second `Bind`, and a refused plan leaves the reader unbound. `IsBound`, `Plan` (now empty
  until bound), and `DelimitedReader.ColumnCount` (`int?`: the header's width, or the first
  record's once read). The plan-taking constructors and overloads are unchanged, and bind
  before the header is read. *(C#)*
- **C# — `Header`.** `DelimitedReader.Header` and `Sheet.Header` are now a `Header`, still an
  `IReadOnlyList<string>`, with `Ordinal(string)` / `Ordinal(ReadOnlySpan<byte>)` (exact,
  case-sensitive, untrimmed, first match; `KeyNotFoundException` naming the column),
  `TryOrdinal` for both, and `Utf8(ordinal)`, the bytes a name was decoded from. *(C#)*
- **C# — workbook from a `Stream`.** `new Workbook(Stream, leaveOpen)` and
  `Workbook.Open(Stream, leaveOpen)` read to the end into pinned memory the workbook owns —
  exactly the remaining length of a seekable stream, doubling for one that is not; more than
  `int.MaxValue` bytes is `TabularFailure.TooLarge`, refused before anything is allocated
  when the stream says its length — and dispose the stream as soon as it has been read
  unless `leaveOpen`. The format is told from the bytes. *(C#)*
- **C# — a row view.** `foreach (var row in batch)` (`Batch.RowEnumerator`) and
  `Batch.Row(i)` hand out `Row`, a `ref struct` view of (batch, index) with `Index`, `Line`,
  `Get<T>`, `Verdict`, `TryGetText`, `GetString`, `GetChars`, `TryGetChars` and `Raw`,
  allocating nothing. `DelimitedReader.Rows()` and `Sheet.Rows()` (`RowSequence`) go on
  across batches, reading as each runs out. *(C#)*
- **C# — UTF-16 text without strings.** `Batch.GetChars(column, row)` decodes a text cell
  into a `ReadOnlySpan<char>` in a lazily made, batch-owned arena that is only appended to
  until the next read — a span handed out earlier is never written over — and
  `Batch.TryGetChars(column, row, Span<char>, out int)` into the caller's buffer; invalid
  UTF-8 replaced as `GetString` replaces it, an empty cell an empty span. Nothing is
  allocated once the arena has grown to a batch's text. *(C#)*
- **C# — async with cancellation.** `DelimitedReader.ReadAsync(CancellationToken)`
  (`ValueTask<Batch?>`, synchronous for a reader of memory), `DelimitedReader.OpenAsync(Stream,
  Dialect, …, CancellationToken)` with and without a plan — the stream constructors read the
  header synchronously, which a browser's stream cannot — `Workbook.OpenAsync(Stream,
  leaveOpen, CancellationToken)`, and `IAsyncDisposable` on `DelimitedReader`, disposing its
  stream asynchronously. The token is observed before anything is done (a cancelled token
  throws before any native call) and at every refill; a cancelled read loses nothing, and
  the reader goes on from where it was. `HyperTabular.WasmSmokeTest` reads a file it serves
  through an `HttpClient` streamed response this way. *(C#)*
- **Java — header-first open and a deferred plan.** `DelimitedReader.of(byte[], Dialect)`,
  `of(MemorySegment, Dialect)`, `of(InputStream, Dialect[, bufferBytes])`,
  `open(Path, Dialect[, bufferBytes])`, `Workbook.sheet(int, SheetOptions)` and
  `Workbook.sheet(String, SheetOptions)` read the header without a plan;
  `DelimitedReader.bind(plan[, batchRows])` and `Sheet.bind(plan)` declare it once, before
  the first read. `read()` throws `IllegalStateException` until then (not final), as does a
  second `bind`, and a refused plan leaves the reader unbound. `isBound()`, `plan()` (now
  empty until bound), and `DelimitedReader.columnCount()` (`OptionalInt`: the header's width, or
  the first record's once read). The plan-taking factories are unchanged, and check the plan
  before the header is read. *(Java)*
- **Java — `Header`.** `DelimitedReader.header()` and `Sheet.header()` now return a `Header`,
  still an unmodifiable `List<String>`, with `ordinal(String)` / `ordinal(byte[])` /
  `ordinal(MemorySegment)` (exact, case-sensitive, untrimmed, first match;
  `NoSuchElementException` naming the column), `findOrdinal` for each (`OptionalInt`), and
  `utf8(ordinal)`, the bytes a name was decoded from. Source-compatible for callers holding
  a `List<String>`; code compiled against 0.7 needs recompiling. *(Java)*
- **Java — workbook from an `InputStream`.** `Workbook.of(InputStream)` reads the stream to
  its end into native memory the workbook owns and closes it — on failure too — as
  `DelimitedReader.of(InputStream, …)` takes its stream over. The format is told from the
  bytes. *(Java)*
- **Java — a row view.** `Batch` is `Iterable<Row>` (`for (Row row : batch)`), and
  `Batch.row(i)` picks one: `Row` is a view of (batch, index) with `index()`, `line()`,
  `get(column, type)`, `isOk`, `verdict`, `text`, `string`, `chars`, `getChars`, `raw` and
  `rawString`. `DelimitedReader.forEachRow(Consumer<? super Row>)` and `Sheet.forEachRow`
  read every batch left and hand over each row, valid only for its own call. *(Java)*
- **Java — UTF-16 text without strings.** `Batch.chars(column, row)` (and `Row.chars`)
  decodes a text cell into a lazily made, batch-owned char array that is only appended to
  until the next read — replaced, never resized, when it runs out, so a view handed out
  earlier is never written over — and returns a read-only `CharBuffer` over the cell;
  `getChars(column, row, char[], offset)` decodes into the caller's array and returns the
  count, or `-1` when it does not fit. Bytes that are not UTF-8 are replaced exactly as
  `string` replaces them; an empty cell is `null` / `0`. *(Java)*
- **Go — header-first open and a deferred plan.** `NewDelimitedReaderUnbound`,
  `NewDelimitedReaderBytesUnbound`, `OpenDelimitedUnbound`, `Workbook.SheetUnbound` and
  `Workbook.SheetNamedUnbound` read the header without a plan; `Bind(plan)` on
  `*DelimitedReader` and `*Sheet` declares it once, before the first `Read`. `Read` returns
  `ErrUnbound` until then (not sticky), a second `Bind` `ErrAlreadyBound`, and a refused plan
  leaves the reader unbound. `IsBound()`, and `DelimitedReader.ColumnCount()` (the header's
  width, or the first record's once read; 0 while unknown). The plan-taking constructors are
  unchanged, and bind before the header is read. *(Go)*
- **Go — `Header`.** `Header()` on the reader and the sheet now returns `Header`, a named
  `[]string` — ranged, indexed and passed as a `[]string` as before — with `Ordinal(name)` /
  `OrdinalBytes(name)` (first exact match; a `*NoColumnError` naming the column, matched by
  `errors.Is(err, ErrNoColumn)`) and `Find` / `FindBytes` (`(int, bool)`). *(Go)*
- **Go — workbook from an `io.Reader`.** `NewWorkbookReader(r)` reads to the end into memory
  the workbook owns; the format is told from the bytes, and the reader is not closed.
  *(Go)*
- **Go — a row view.** `Batch.Row(i)` and `Batch.All()` (`iter.Seq[Row]`), and
  `DelimitedReader.All()` / `Sheet.All()` (`iter.Seq2[Row, error]`) across batches, ending
  with the input's error unless it ended at `io.EOF`. A `Row` has `Index`, `Line`,
  `Verdict`, `Fault`, `Text`, `TextString` and `Raw`, and `Cell[T](row, column)` is `Get`
  for a row; none of it allocates (asserted in `allocs_test.go`). *(Go)*
- **Go — text as a string without a copy.** `Batch.TextString(column, row)` and
  `Row.TextString(column)` view a text cell's bytes as a `string` (`unsafe.String`), valid
  until the next `Read`. *(Go)*
- **Swift — header first, plan after.** Every `DelimitedReader` opening (`bytes:`,
  `bytesNoCopy:`, `reading:`, `contentsOfFile:`) has a plan-less overload, and
  `Workbook.sheet(_:options:)` / `sheet(named:options:)` open a sheet without one; each reads
  its header, and `bind(_:)` declares the plan once, before the first read. A read before it
  throws `PlanError.unbound` (not sticky), a second bind `PlanError.alreadyBound`; `isBound`
  and `DelimitedReader.columnCount` report the state. `header` is now a `Header` — a
  `RandomAccessCollection` of `String`, comparable to an array literal — with
  `ordinal(of:)` throwing `NoSuchColumn` naming the missing name and `firstIndex(of:)`
  returning `nil`, both exact byte for byte (no Unicode equivalence) and taking a `String` or
  UTF-8 bytes. Code that passed `header` where a `[String]` is expected wraps it in
  `Array(_:)`. *(Swift)*
- **Swift — workbook from a stream.** `Workbook(reading:)` reads a `DelimitedReader.Source`
  closure or a Foundation `InputStream` (opened if need be, closed once read) to its end, into
  memory the workbook owns. *(Swift)*
- **Swift — rows.** `Batch` is a `RandomAccessCollection` of `Row` (`for row in batch`,
  `batch[i]`); a `Row` has `index`, `line`, `get(_:as:)`, `verdict(_:)`, `text(_:)`,
  `string(_:)` and `raw(_:)`, allocates nothing, and is valid as long as its batch.
  `DelimitedReader.forEachRow` and `Sheet.forEachRow` read every row left across batches.
  `rows` is still the row count. *(Swift)*
- **Swift — async.** `DelimitedReader(bytes:dialect:…) async` (plan-less, or with a plan)
  reads any `AsyncSequence` of `UInt8` — `URL.resourceBytes`, `FileHandle.bytes` — and
  `readAsync()` reads it, checking Task cancellation before the read and before each refill;
  a cancelled read loses nothing and the next goes on from there. `readAsync()` reads any
  reader (memory never suspends). `Workbook(bytes:) async` awaits a byte sequence to its end.
  Within the macOS 13 / iOS 16 floors. *(Swift)*
- **Python — header first, plan after.** `plan` is optional on `DelimitedReader(source,
  dialect, plan=None)`, `DelimitedReader.open(path, dialect, plan=None)` and
  `Workbook.sheet(which, options, plan=None)`: without one the header is read and the reader
  or sheet waits for `bind(plan)`, once, before the first read. A read before it, or a second
  `bind`, raises `RuntimeError` (a later `bind` puts the first right); a plan that cannot be
  honoured leaves it unbound. `is_bound` on both, and `DelimitedReader.column_count`. The
  header is now a `Header` — still the tuple of `str` it was — with `ordinal(name)` (exact,
  first match, `KeyError` naming a missing column; a `bytes` name matched against the file's
  own bytes) and `get(name, default=None)`. *(Python)*
- **Python — a workbook from a file object.** `Workbook(data)` takes a binary file object as
  well as `bytes`, read to its end. *(Python)*
- **Python — rows.** `for row in batch`, `batch.iter_rows()` and `batch.row(i)` give `Row`
  views — `index`, `line`, `get(column)` (`Success` or `Fault`), `text(column)`,
  `raw(column)` — and `reader.rows()` / `sheet.rows()` run across batches. `Batch.get(column,
  row)` and `Batch.text(column, row)` are the per-cell calls a row's are. A row, like its
  batch, stays valid after the read moves on. *(Python)*
- **Python — asyncio.** `await DelimitedReader.open_async(stream, dialect, plan=None)` reads
  an `asyncio.StreamReader` (anything with `async read(size)`), its header awaited;
  `await reader.read_async()` and `async for batch in reader` read it, and
  `await Workbook.open_async(stream)` opens a workbook from one. The extension never calls
  the stream: a read that needs input says how much, the stream is awaited for it and fed
  in, and the core's work runs between awaits with the GIL released. A read cancelled while
  it awaits leaves the reader resumable. *(Python)*
- **Ruby — header-first open and a deferred plan.** The plan is now optional:
  `DelimitedReader.new(source, dialect)`, `DelimitedReader.open(path, dialect)` and
  `Workbook#sheet(which, options)` read the header and stop, and `#bind(plan)` on the reader
  and the sheet declares the plan once, before the first read (it returns the receiver).
  A `read` before `bind` raises `RuntimeError` (not sticky), a second `bind` raises
  `RuntimeError`, and a plan that is not Columns raises `ArgumentError` and leaves the reader
  unbound. `#bound?` on both, and `DelimitedReader#column_count` (the header's width, or the
  first record's once read; `nil` while unknown). `#plan` is `[]` until bound. A plan handed to
  the constructor is bound before the header is read, as before. Both backends do it: the
  Fiddle runtime and the Magnus extension now take the plan through `bind`. *(Ruby)*
- **Ruby — `HyperTabular::Header`.** `#header` on the reader and the sheet returns a
  `Header`, a frozen `Array` subclass — equal to the `Array` of the same names, indexed,
  iterated and pattern-matched as before — with `ordinal(name)` (first exact match; a
  `KeyError` naming the column, or the block's value with a block, as `Hash#fetch` does) and
  `find_ordinal(name)` (`nil` for none). A binary String is matched as its bytes. *(Ruby)*
- **Ruby — workbook from an IO.** `Workbook.new` takes anything with `#read` as well as a
  String: read to its end into a String the workbook owns, the format told from the bytes,
  and the IO closed once read when `close_source: true` says so. *(Ruby)*
- **Ruby — a row view.** `HyperTabular::Row` — `index`, `line`, `get(column)` (also `[]` and
  `verdict`), `value(column)`, `raw(column)`, `size`, `to_a`, and `deconstruct` for
  `case row in [...]`. `Batch` is `Enumerable` over its rows (`each`, and `row(i)`), and
  `DelimitedReader#each` / `Sheet#each` walk every remaining row across batches (both are
  `Enumerable`). A batch is a copy, so a row outlives the read. `DelimitedReader#each_row`
  still yields frozen Arrays of verdicts. *(Ruby)*
- **PHP — header first, plan after.** `DelimitedReader::fromString`, `fromStream` and `open`,
  and `Workbook::sheet`, take the plan as `?array $plan = null`: left out, the source is opened
  and its header read, and `bind(array $plan)` declares the plan once, before the first read
  (`isBound()` says whether it has been). A read before it, or a second bind, is a
  `LogicException`; the first does not stick. A plan handed to a factory is still checked
  before the header is read. `headerIndex()` is a new `HyperTabular\Header` over the names
  `header()` still returns as an array — countable, iterable, indexable and JSON-serializable —
  with `ordinal($name)` (exact, case-sensitive, first match; an `OutOfBoundsException` naming
  the column) and `find($name)` (`null` instead). `DelimitedReader::columnCount()` is a
  record's width once known. A sheet opened without a plan keeps the header row's cells for an
  ODS row that repeats, and the workbook corpus replays header first, buffers stingy included.
  *(PHP)*
- **PHP — `Workbook::fromStream($stream)`.** Opens a workbook from a stream resource, read from
  where it stands to its end; the stream stays the caller's, as `DelimitedReader::fromStream`'s
  does. *(PHP)*
- **PHP — rows.** `Batch` is `IteratorAggregate`: `foreach ($batch as $row)` yields a
  `HyperTabular\Row` per row — `index()`, `line()`, `get($column)`, `value($column)`,
  `fault($column)`, `raw($column)`, each the batch's own answer for that row — and
  `DelimitedReader::rows()` / `Sheet::rows()` are generators of every row left, across batches.
  *(PHP)*

### Changed

- **HyperCast 0.8.0.** Every package now depends on HyperCast 0.8.0 rather than 0.7.0 — the
  first with Android, which this release's C#, Go and Swift packages ship for. The verdict,
  fault and option types a batch hands out are HyperCast's own, so they come from 0.8.0 too.
  *(every package)*
- **Rust — `Error` has three new variants.** `Error::NoColumn`, `Error::Unbound` and
  `Error::AlreadyBound`; `Error` is not `#[non_exhaustive]`, so a `match` that names every
  variant needs an arm for them. *(crate)*
- **C# — `Header` is a `Header?` rather than an `IReadOnlyList<string>?`.** On
  `DelimitedReader` and `Sheet`. `Header` implements `IReadOnlyList<string>`, so source that
  reads it as a list compiles unchanged; an assembly compiled against 0.7 needs recompiling.
  *(C#)*
- **Java — `header()` returns `Header` rather than `List<String>`.** On `DelimitedReader`
  and `Sheet`. `Header` is an unmodifiable `List<String>`, so source compiles unchanged;
  code compiled against 0.7 needs recompiling. *(Java)*
- **Go — `Header()` returns `Header` rather than `[]string`.** Source that ranges, indexes,
  slices, assigns or passes it compiles unchanged; a reflection-based comparison against a
  `[]string` (`reflect.DeepEqual`, testify's `Equal`) now needs `[]string(header)`. *(Go)*
- **Swift — `header` is a `Header?` rather than a `[String]?`.** On `DelimitedReader` and
  `Sheet`. It is a `RandomAccessCollection` of `String` and compares with an array literal;
  code that passes it where a `[String]` is expected wraps it in `Array(_:)`. *(Swift)*
- **Ruby — a `nil` plan means "bind later".** `DelimitedReader.new(source, dialect, nil)` used
  to read through an empty plan; it now opens the reader unbound. Pass `[]` for an empty
  plan. *(Ruby)*

### Fixed

- **Ruby — the Magnus extension survives a compacting garbage collection.** It kept the
  `Runtime::Book` module and its `StructureError` class in a Rust static, which a compacting
  collection could move without updating; the next workbook opened then called into
  whatever object had taken the module's place — a segfault, or `undefined method 'stingy'
  for an instance of Array`, which is how CI caught it (run 37718484155, macOS, after the
  reader spec's `GC.compact`). Both are pinned when the extension loads, and a new spec
  moves every movable object before opening a workbook. *(Ruby)*

## [0.7.0] — 2026-10-07

The first release. HyperTabular reads delimited text — CSV, TSV, any single-byte ASCII
separator — and workbooks — XLSX and ODS — a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell. It starts at
0.7.0 rather than 0.1.0 to share its version with HyperCast 0.7.0, the release it is built
on and the first with the interop surface and the typed doors every binding here reads
cells through. *(every package)*

### Added

- **One native core, `libhypertabular`, with no standard library, no allocation and no
  panic.** The core reads bytes the caller hands it and writes into buffers the caller
  owns: the binding allocates the input, one value array and one verdict array per
  column, and the table that locates each cell, once, and the core fills them in one call
  per batch — the native boundary is crossed once per few thousand rows, not once per
  cell. `rust/check-core.sh` proves all three properties on the library itself on every
  pull request: an allow-list of imports (`malloc` fails it), the export list compared
  whole, and dtolnay's `no-panic` on every export, shown a canary it has to reject. A
  counting allocator watches whole inputs go through the core and sees zero. The C ABI is
  14 exports, `hypertabular_version` among them; HyperCast is the one dependency, taken with its
  own exports off, so the library carries none of its symbols and links beside
  `libhypercast` in one program. *(crate, every binding)*
- **Delimited text.** A forward-only reader over any chunk that starts on a row boundary:
  a declared `Dialect` (separator, quoting, header, blank lines) and a declared plan (which
  source column, through which HyperCast door) — nothing is sniffed. A SIMD scanner
  (AVX2 and PCLMULQDQ on x86-64, NEON and PMULL on arm64, eight bytes at a time in a
  `u64` elsewhere) locates the cells, chosen at run time, and each plan column is cast in
  one monomorphic loop. Text is zero-copy: a span
  into the input, with only cells holding `""` unescaped into an arena. About 575 MB/s
  into the caller's buffers with a boolean and an `i32` cast, linux-x64. *(every package)*
- **Workbooks — XLSX (transitional and strict, `.xlsm` included) and ODS.** The container
  is read by the core's own zip reader (ZIP64 included) and its own inflate, written for
  the no-panic proof because neither `miniz_oxide` nor `zlib-rs` passes it. Shared
  strings, styles and the date system (1900 or 1904) are loaded once; a numeric cell's
  format says whether it is a date, a date-time, a time or an elapsed span, and each door
  converts the stored `f64` directly through HyperCast's typed doors — an exact shortest
  decimal, an Excel serial snapped to the fewest fractional-second digits that store as
  the same double, a time of day, a duration. Hidden sheets are listed and read; chart
  and macro sheets are not sheets. An encrypted package or an OLE file is refused by name.
  A 300 000-row Excel workbook reads in about 0.6 s through the core, linux-x64. *(every
  package)*
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is a
  HyperCast `Fault` — `Empty`, `Malformed` or `OutOfRange` with the offending span — in
  its column, and the read goes on. A record of the wrong width, input that ends inside a
  quoted cell, a workbook whose container or parts cannot be read, is a structural error,
  raised only after every intact row before it has been delivered. *(every package)*
- **A Rust API over the core**, behind the default `std` feature: `DelimitedReader`,
  `Workbook` / `Sheet`, one `Batch`, `Column` factories and `batch.get::<T>(column, row)`.
  The core itself is `hypertabular::kernel`. *(crate)*
- **Seven bindings, one shape.** Every binding mirrors the Rust API: one batch type from
  `read()`, a whole column at a time or a cell at a time, `line(row)` and the raw text of
  a cell, `Workbook` / `Sheet` / `SheetInfo` / `SheetOptions`, and HyperCast's own verdict,
  format and option types taken from its package rather than copied:
  - C# (.NET 11): `LibraryImport` only, `batch.Get<T>(column, row)` over HyperCast's
    `Verdict<T>` union, spans over pinned buffers; a Native AOT publish links the core in
    from the package's static archive (proven by `HyperTabular.AotSmokeTest`).
  - Java (JDK 25): FFM downcalls, `batch.get(column, row, Double.class)` over HyperCast's
    sealed `Verdict`, `MemorySegment` columns; GraalVM Native Image proven by
    `java/aot-smoke-test`.
  - Go: the core linked in through cgo on Linux, macOS and Windows; `hypertabular.Get[T]`
    and typed slices per column.
  - Swift: the core linked in as a static library on every platform;
    `batch.get(column, row:, as:)` over HyperCast's `Verdict` enum.
  - Python (3.11+): a PyO3 extension, columns as typed `memoryview`s, `match`/`case` over
    `Success` / `Fault`; abi3 wheels, no compiler to install.
  - Ruby (3.3+): pattern-matched `Data` verdicts, a column decoded in one `unpack`. A
    Magnus extension ships in precompiled platform gems for every RID but Intel macOS,
    each carrying Ruby 3.4 and 4.0, and replaces the Fiddle crossing in place, so values
    and verdicts are the same Ruby on both backends; a spec holds the two to the same
    answer over every cell of both corpora. `HyperTabular::BACKEND` reports `:native` or
    `:fiddle`, and `HYPERTABULAR_PURE=1` forces Fiddle, which the universal gem carries
    with all eight libraries for every other Ruby or platform.
  - PHP (8.2+): ext-ffi, `Success|Fault` per cell, a column decoded in one `unpack`.
- **A conformance corpus every implementation replays.** `corpus/delimited.json` (33
  cases) and `corpus/workbook.json` (198 cases), over generated workbooks and files
  written by real applications — Excel for Windows (data, strict, 1904, encrypted,
  protected, macro-enabled, ZIP64), Excel for the web, LibreOffice and Google Sheets, in
  both formats — replayed byte for byte by the Rust suite and by every binding's own.
  *(every package)*
- **Benchmarks in every ecosystem's own harness** — Criterion, BenchmarkDotNet, JMH,
  `testing.B`, package-benchmark, pyperf, benchmark-ips and phpbench — over the same two
  300 000-row workbooks and the same checksum; `docs/workbook.md` has the table. *(every
  package)*
- **Every platform the forge builds.** linux-x64, linux-arm64, linux-musl-x64,
  linux-musl-arm64, osx-arm64, win-x64 and win-arm64 tested on real hardware, osx-x64
  built and tested at the core; every native library carries a build-provenance
  attestation, verified again before it is staged or packed. *(every package)*
- **iOS and Mac Catalyst for C#, Swift and Go**, as HyperCast 0.7.0 and HyperUuid 0.7.0
  have them. Neither platform loads a library, so the core is linked into the app from
  static archives the forge cross-compiles beside the rest (`apple_mobile`). *(C#, Swift,
  Go)*
  - C#: the package carries archives for `ios-arm64`, `iossimulator-arm64`,
    `maccatalyst-arm64` and `maccatalyst-x64`, linked through `NativeReference` and reached
    as `__Internal`; every export is declared for the shared library, the browser and
    `__Internal`.
  - Swift: `swift/HyperTabularCoreApple.xcframework` on iOS 16 and Mac Catalyst 16,
    declared only on a Mac.
  - Go: `ios`, `iossimulator` and `maccatalyst` archives chosen by build tag; Android and
    the Intel iOS simulator stop at compile time instead of linking the wrong archive, and
    `.github/scripts/check_go_archives.sh` holds every platform to its archive.
  - HyperTabular's archive and HyperCast's link side by side in one app and share no
    symbols. CI's `test-apple-mobile` job runs C# and Swift in an iOS simulator and as a
    Mac Catalyst process, Go's suite in the simulator, and links an iOS device build of
    each.
- **WebAssembly, for every binding whose toolchain can link the core into a wasm build.**
  The core imports nothing — no clock, no entropy, no allocator — so each is the same
  archive linked in, and CI runs each on every pull request — in headless Chrome for the
  browser builds, under wasmtime, Node or the JVM for the rest. *(crate, C#, Java, Go,
  Swift, Python, Ruby)*
  - Rust: the whole suite passes under wasmtime on `wasm32-wasip1`, and
    `rust/browser-test` runs the crate on `wasm32-unknown-unknown` in the browser; a
    `cargo wasm-staticlib` alias builds the archive the bindings link.
  - C#: Blazor WebAssembly on .NET 11 — the package's `.targets` link its own archive
    beside HyperCast's and export the core's functions; `HyperTabular.WasmSmokeTest`
    proves it.
  - Go: a TinyGo backend (`-target=wasm`, `wasip1`) linking `staticlib/wasm`.
  - Swift: swift.org's WebAssembly SDK links the `wasm32-unknown-wasip1` archive in the
    artifact bundle.
  - Python: a ninth wheel for Pyodide 314 (`pyemscripten_2026_0_wasm32`); the whole pytest
    suite runs inside Pyodide under Node and in Chrome.
  - Ruby: the `hypertabular-wasm` gem, the Magnus extension prebuilt for `wasm32-wasip1`
    for `rbwasm build` to link into a ruby.wasm interpreter beside `hypercast-wasm`.
  - Java: an in-process GraalWasm backend instead, as HyperCast's — the jar carries the
    core as `native/wasm32-wasip1/hypertabular.wasm`; `-Dhypertabular.backend=wasm`
    selects it, and it is the automatic fallback on a platform with no bundled native
    build or whose library will not load. `Tabular.backend()` reports which is in use;
    GraalWasm stays a dependency the consumer adds. The whole suite runs through it on
    every leg (`testWasm`); on GraalVM's JIT the 300 000-row workbook reads in 2.6× the
    native time.
  - PHP is not built for WebAssembly, for HyperCast's reasons.
- **Fuzzing.** `rust/fuzz` puts the core under libFuzzer with four targets: delimited text
  in any dialect through every door, raw XLSX/ODS containers, workbook XML inside a valid
  package, and the inflater checked against zlib-rs. Each drives the core as the C ABI
  does, with buffers that start empty and grow only when asked, and checks that it reads
  the same as with plenty of room. About four million executions before this release found
  nothing in the core; CI holds the fuzz crate to fmt and clippy. *(crate)*
- **One release pipeline for all eight packages.** `prepare-release.yml` bumps every
  manifest and dispatches CI on the bump; `stage-native-binaries.yml` verifies the
  attestation of each library that run built and commits them for the packages that
  resolve from git — PHP, Swift and Go; a `v*` tag runs `release.yml`, which refuses a tag
  whose CI run or committed libraries report another version, then publishes to
  crates.io, NuGet, Maven Central, PyPI and RubyGems, attesting what it publishes and
  using trusted publishing (OIDC) wherever the registry offers it. Packagist, SwiftPM and
  the Go module (`go/v0.7.0`) resolve from the tags. *(every package)*

[Unreleased]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/SkunkWerkx/HyperTabular/releases/tag/v0.7.0
