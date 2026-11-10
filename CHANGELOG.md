# Changelog

Every package in this repository — the `hypertabular` crate, the C#, Java, Go, Python, Ruby,
PHP, Swift, TypeScript and R bindings, and the C archives with their C++ header — shares one
coordinated version, so one changelog covers all of them. Each entry marks which packages it
actually affects.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Every binding now needs HyperCast 1.1.0, whose own release comes first: its NuGet package,
jar, Go module, PyPI package, gem, PHP extension, npm package, R package and C archive.

### Added

- **Reading a messier delimited file.** A dialect can now pass over physical lines (`skip`),
  discard records ahead of the header (`header_row`), pad short records and cut long ones
  (`short_rows`, `long_rows`), and declare tokens that stand for no value, for every column or
  column by column; a column can declare date options and month names to its door. A batch says
  what failed in a cell (`detail`), and where its verdict came from (`origin`), beside the verdict
  it always gave. Through the new exports `hypertabular_delimited_init_with` and
  `hypertabular_delimited_fill_with` and their size-tagged structs; the 1.0 exports keep their
  layouts and their masking of a format's flags, and a block from `_init` is still its 64 bytes, though
  `hypertabular_delimited_state_size` now reports 80. A Rust reader takes those notes unless
  `DelimitedOptions::notes(false)` says otherwise, a sheet unless `SheetOptions::with_notes(false)`
  does, and that costs, against notes off (Apple M6, 2026-11-06, `docs/delimited.md`): +6.7% on
  `cells`, +1.8% on multi-byte cells, nothing measurable on typed batches and +11.2% where four cells
  in five fail. Notes off still costs the options' checks, +0.9% to +2.6%. Rust keeps each distinct
  token list or month vocabulary for the life of the process, so that `Column` and `Dialect` stay
  `Copy`: a server building them from request data grows without bound. *(Rust and Go; each other
  binding's entry is below)*
- **Reading a file in Windows-1252, Latin-1 or UTF-16LE.** A dialect declares the encoding
  (`Dialect::with_encoding`, never sniffed), and the new export `hypertabular_transcode` decodes it
  to UTF-8 ahead of the fill, holding nothing between calls. A lone surrogate or a final half unit
  is a structural failure, `Failure::ENCODING`, raised after every intact row, whose `byte` is
  the original input's offset; nothing is ever replaced. Everything else about a read, spans
  included, speaks of the decoded text. *(Rust and Go; each other binding's entry is below)*
- **Reading part of a sheet.** A sheet can now be read through a range like `A5:F200`
  (`Range::parse`, or `hypertabular_range_parse`), pass over rows (`skip`) and discard rows ahead of
  the header (`header_row`). A plan's ordinals count from the range's first column, while a row's
  number stays the sheet's own. A range's edge never walks a repeat, and a read ends at the range's
  last row, so a failure in a row past it isn't met, though what opening reads is met regardless:
  all of an ODS's `content.xml` and every chunk of a Numbers document. A workbook's text cells also
  take the tokens and date options a delimited column does, and its cells report `detail` and
  `origin` as delimited ones do, typed cells included. Through the new exports
  `hypertabular_workbook_sheet_with` and `hypertabular_workbook_fill_with`; the 1.0 exports keep
  their layouts and their masking of a format's flags. *(Rust and Go; each other binding's entry is below)*
- **A workbook says when a formula has no cached result.** `hypertabular_workbook_fill_with` reads
  one, XLSX's `<f>` with no `<v>` or an empty one and ODS's `table:formula` with no value, as `Empty`
  at `origin` `uncached`; a cached empty string, a blank and a cached error don't change, nor do the
  rows a sheet delivers, and `hypertabular_workbook_fill` says plain `Empty` as it did. So a row whose
  only cells are formulas with nothing spelled is still an empty row: `skip_empty_rows` leaves it out
  and nothing says so, a table of nothing but such formulas can read as no rows, and turning the
  filter off delivers them marked. No Numbers file here holds a formula with no result, so none is
  read as one. *(Rust and Go; each other binding's entry is below)*
- **A list of what went wrong in a batch.** The new export `hypertabular_faults` writes a batch's
  `Malformed` and `OutOfRange` cells, formulas with no cached value and long rows under
  `long_rows = report` into a caller's array of a new 32-byte `RawProblem`, a few at a time from a
  cursor; blanks, declared tokens and padding aren't listed. Rust's `Batch::faults` yields each as a
  `Problem`, with `line` and `source_column`. It can't name a row the empty-row filter left out.
  `corpus/delimited.json` and `corpus/workbook.json` carry `problems` on every case they read, and
  old cases differ by that key alone. *(Rust and Go; each other binding's entry is below)*
- **A kept batch's faults list.** A binding whose batch outlives the read keeps copies of it, and Rust's
  new `KeptBatch` works the batch's faults list out from them when it's asked for, the same list
  `Batch::faults` gave: each column's `Batch::verdicts`, `Batch::notes` (new), `Batch::source_column`
  and each row's `Batch::line`, with `Batch::reported_widths` for a read that reports long rows, the last
  two now public, go into `KeptColumn`s, and `KeptBatch::faults` yields `Problem`s through `KeptFaults`.
  `KeptFaults` is `Send` and `Sync`, and `CellNote` is exported beside `CellVerdict`. *(Rust; TypeScript
  takes its list through it)*
- **Asking what a source's columns look like.** The new export `hypertabular_suggest` tries each
  column of a sample against the doors, in the order bool, `i32`, `i64`, `f64`, ISO date, date under
  a declared order, timestamp, UUID, and writes a new 20-byte `RawSuggestion`: door, param, status
  (`resolved`, `ambiguous` or `unresolved`) and matched and sampled counts. It's its own call and is
  never applied: nothing is sniffed in a read, and declaring the locale, a number format and a date
  order, stays the recommended route, with this a starting point for a plan to inspect. With no format
  declared, each of two notations has to read the whole column, `,` grouping digits or `,` as the
  decimal. `1,234,567`, `1,234.5` and `2.5` read only under the first, and `1.234.567`, `1.234,5`, `2,5`
  and `0,125` only under the second, and those are resolved, `param` `4`, in that notation. `1,234` and
  `1.234` read under both, and come back ambiguous, as `01/02/2026` does, until a format or a date order
  is declared or a value settles them: that takes in a column of prices at three decimals, which reads
  as text until then. `1,234` beside `2.5` is a resolved `f64`, and `2,5` beside `2.5` is unresolved. A
  binding's plan reads any other resolved number with no format declared in `NumFormat::INVARIANT`, flags
  `95`, so a later `1,23,456` is `Malformed`. Some answers can surprise, and are printed: `0` and `1` are bool, `F`
  and `No` read `false`; `1,23,456`, Indian grouping, is text with no format declared, and declaring
  Indian grouping for the column makes it a number; a workbook's date, time and
  duration cells suggest text through the text door; `007` (or `٠٠٧` in a format's native digits) and an integer past `i64` stay text, with
  no note, a declared symbol's `E` (`EUR`) being no exponent. A workbook's number cell has no notation, so the export takes a byte a cell saying which were
  typed, and a column of them is a number under any format, whose plan reads every one, while text
  holding `1.234` keeps its question, beside them too. A binding's `SuggestOptions` takes a date order,
  for every column or for one, that answers a date the sample left open. Rust's `DelimitedReader::suggest`, `DelimitedReader::suggest_slice` and `Sheet::suggest`
  sample up to 1,000 rows of a path, bytes or a sheet, never a stream, and return a plan, a report and
  whether rows lie past the sample. `corpus/delimited.json` gains cases with a `suggest` key. *(Rust and Go; each
  other binding's entry is below)*
- **Go has all of that.** `Dialect` adds `Skip`, `HeaderRow`, `ShortRows`, `LongRows`, `Encoding` and
  `Tokens`, and `SheetOptions` adds `Skip`, `HeaderRow`, `Range` and `Tokens`, with `ParseRange` and
  `NewRange`. A column has `WithTokens` and `WithDateOptions`. A batch has `Detail`, `Origin` and
  `Faults`, and a row `Detail` and `Origin`; `Notes(false)` turns the notes off for a reader or a
  sheet, whose constructors now take options. `SuggestDelimited`, `SuggestDelimitedBytes` and
  `Sheet.Suggest` return a `*Suggestion`; a `SampleRows` of 0 is not set, so 1,000, where Rust's
  `sample_rows(0)` is 1, because a struct can't tell a field left out from 0. A `Faults` list ranged over past the next `Read()` panics. Every instance of the translated core is now 64 KiB larger,
  four pages to start where it was three.
  *(Go)*
- **C and C++ have all of that.** The nine new exports are in `hypertabular.h`, and `c/README.md`
  lists them. In `hypertabular.hpp`, `Dialect` adds `with_skip`, `with_header_row`, `with_short_rows`,
  `with_long_rows`, `with_encoding` and `with_tokens`, and `SheetOptions` adds `with_skip`,
  `with_header_row`, `with_range`, `with_tokens` and `with_notes`, with `Range::parse` and `Range::of`.
  A column has `tokens` and `date_options`. A batch has `detail`, `origin` and `faults`, a row `detail`
  and `origin`; `DelimitedOptions` has `with_notes`. `DelimitedReader::suggest`,
  `DelimitedReader::suggest_bytes` and `Sheet::suggest` return a `Suggestion`. `Header::require`
  refuses a name two columns share, with `ErrorKind::ambiguous_column`, and `ordinal` still takes the
  first. A `faults()` walk past the next `read()` ends the program with `std::abort()`. A column given no format is `NumFormat::invariant()`, Western grouping, as every binding's is.
  *(C and C++)*
- **C# has all of that.** Every read now takes the size-tagged fills, and the 23 exports are declared
  three times each, none past Mono's twelve integer arguments. `Dialect` adds `Skip`, `HeaderRow`,
  `ShortRows`, `LongRows`, `Encoding`, `Tokens` and `Notes`, and `SheetOptions` adds `Skip`, `HeaderRow`,
  `Range`, `Tokens` and `Notes`, with `SheetRange.Parse`, `TryParse` and its constructor. A column has
  `WithTokens` and `WithDateOptions`. A batch has `Fault`, `Origin` and `Faults()`, a row `Fault` and
  `Origin`, and `Get<T>`'s `Fault` carries HyperCast's detail. `DelimitedReader.Suggest`, from a path or
  memory, and `Sheet.Suggest` return a `Suggestion`. `Header.Ordinal` refuses a name two columns share
  with the new `DuplicateColumnException`, a `KeyNotFoundException`, where it took the first; `TryOrdinal`
  takes the first. `TabularFailure.Encoding` is new. A column given no format is `NumFormat.Invariant`,
  Western grouping. A walk of a batch's rows with `GetEnumerator` now throws `InvalidOperationException`
  past the next `Read`, where it followed the batch on. A batch kept past every other reference to its reader or workbook now stays readable, where the collector could free the text under it. *(C#)*
- **Java has all of that, on both paths.** Every read now takes the size-tagged fills, through FFM on
  JDK 25 and JNI below it, whose 23 natives each carry `no_panic`. `Dialect` adds `withSkip`,
  `withHeaderRow`, `withShortRows`, `withLongRows`, `withEncoding`, `withTokens` and `withNotes`, and
  `SheetOptions` adds `withSkip`, `withHeaderRow`, `withRange`, `withTokens` and `withNotes`, with
  `Range.parse`, `tryParse` and its constructor. A column has `withTokens` and `withDateOptions`. A batch
  has `detail`, `origin` and `faults()`, a row `detail` and `origin`, and a walk of a batch's rows now throws
  `ConcurrentModificationException` past the next `read()`, where it followed the batch on. `DelimitedReader.suggest`, from a
  path or memory, and `Sheet.suggest` return a `Suggestion`. `Header.ordinal` refuses a name two columns
  share with the new `AmbiguousColumnException`, a `NoSuchElementException`, where it took the first;
  `findOrdinal` takes the first. `TabularFailure.ENCODING` is new, and a text cell that isn't UTF-8 is
  `null` from `text`, `string` and `chars` beside its `MALFORMED`, where 1.0 replaced its bytes. A column
  given no format is `NumFormat.INVARIANT`, Western grouping. A stream is read once a call, so finished rows come
  back however long it then blocks. When a read fills what it asked for, the next asks for more, up to 64 MiB,
  so one long record from a stream that fills every read no longer costs the square of its length: 1 GiB took
  175 s and takes under a second. A file never paid that (1 GiB, 0.4 s). A stream whose reads come back short
  still does, in Java and in Rust alike, since a fill looks again from the record's start: 256 MiB at 64 KiB per
  read takes about 22 s in either, and 1 GiB at just under 1 MiB per read about the same.
  *(Java)*
- **Swift has all of that.** Every read now takes the size-tagged fills, and the four 1.0 exports they
  replace stay reachable, internally, for the test that still crosses them. `Dialect` adds `skip`,
  `headerRow`, `shortRows`, `longRows`, `encoding`, `tokens` and `notes`, and `SheetOptions` adds `skip`,
  `headerRow`, `range`, `tokens` and `notes`, with `SheetRange(parsing:)` and its initializer, both throwing
  `RangeError`. A column has `withTokens` and `withDateOptions`. A batch has `fault(_:row:)`, `origin(_:row:)`
  and `faults()`, a row `fault` and `origin`, and `get`'s `.fault` carries HyperCast's detail.
  `DelimitedReader.suggest`, from a path or bytes, and `Sheet.suggest` return a `Suggestion`.
  `Header.ordinal(of:)` refuses a name two columns share with the new `AmbiguousColumn`, where it took the
  first; `firstIndex(of:)` takes the first. A plan or a suggest's options holding a retired grouping word is
  `PlanError.refused(column:)`. `TabularFailure.encoding` is new. A column given no format is
  `NumFormat.invariant`, Western grouping. A `Row`, a slice or `reversed()` of a batch's rows, now the new
  `Batch.Rows`, or a walk of them kept past the next `read()` now stops the program with a precondition failure,
  where it read the next batch's cells as its own. A batch or a row that outlives its reader or sheet now stays
  readable, where it read freed memory. *(Swift)*
- **Python has all of that.** `Dialect` adds `skip`, `header_row`, `short_rows`, `long_rows`, `encoding`
  and `tokens`; `SheetOptions` adds `skip`, `header_row`, `range`, `tokens` and `notes`; `Column` adds
  `with_tokens` and `with_date_options`; readers take `notes=`, and `open_async` and `read_async` all of
  it. `Fault` carries HyperCast's `detail`; `batch.origin`, `column.origin` and `row.origin` say where a
  verdict came from, and `batch.faults()` lists a batch's `Problem`s. `DelimitedReader.suggest`,
  `suggest_bytes` and `sheet.suggest` answer with a plan and a report. `Header.ordinal` now raises
  `AmbiguousColumn`, a `ValueError`, for a name two columns share, where it took the first; `get` keeps
  that. The extension's delimited read now goes through the Rust API, so that a text cell that isn't
  UTF-8 is `MALFORMED` where a `str` used to replace what it couldn't decode, a header name that isn't
  UTF-8 raises `TabularError` where it was replaced, and a `Header`'s bytes are its decoded UTF-8. A
  column in Indian grouping reads beside a Western one, delimited and sheet, and a format holding the
  retired bit raises `ValueError` before any cell. A second read of a reader over an asynchronous stream,
  from another coroutine or thread while one awaits the stream, raises `RuntimeError`, where it could
  deliver rows out of order or twice. The move is slower, and measured so: on an Apple M6 on 2026-11-07, against 1.0's extension in the same session (a plain timing loop, each the best of 5 in the lowest of three rounds), an xlsx sheet of 500k rows and 3 columns reads in 107.0 ms against 101.2 ms, a 97 MiB delimited file of 2M rows and 6 columns in 83.9 ms against 78.6 ms and from `bytes` in 75.2 ms against 73.9 ms, and a file of 2M rows whose 6M cells are all bad in 98.7 ms against 87.4 ms. A faults list costs
  a read nothing until it's asked for, and asked of those 6M bad cells, batch by batch, the same read takes
  5.05 s against 103.1 ms without it (Apple M6, 2026-11-08, in one session, the lowest of three rounds of each). *(Python)*
- **Ruby has all of that, on both paths.** The Fiddle table takes the size-tagged fills and the rest of the 23
  exports, and the Magnus extension answers the same. `Dialect` adds `skip:`, `header_row:`, `short_rows:`,
  `long_rows:`, `encoding:` and `tokens:`; `SheetOptions` adds `skip:`, `header_row:`, `range:` (a
  `HyperTabular::Range`), `tokens:` and `notes:`; `Column` adds `with_tokens` and `with_date_options`; readers
  take `notes:`. `Fault` carries HyperCast's `detail`, with `Batch::BLANK` beside `Batch::EMPTY`; `origin` on a
  batch, a column and a row says where a verdict came from, and `batch.faults` lists a batch's `Problem`s, their
  spans in characters. `DelimitedReader.suggest`, `suggest_string` and `sheet.suggest` answer with a plan and a
  report. `Header#ordinal` now raises `AmbiguousColumn`, an `ArgumentError`, for a name two columns share, where
  it took the first; `get` keeps that. A column in Indian grouping reads beside a Western one, delimited and
  sheet, a plan with no format is Western, and a format holding the retired bit raises before any cell. A stream
  that stalls hands over the rows it finished, and a stream that raises raises to the caller without ending the
  read. A `batch_rows` no memory holds is a `NoMemoryError` naming it, and on both paths the arrays a batch sizes
  are asked for zeroed and left untouched, where a large one had its every page written, enough at `1 << 40` to
  take a machine down. It's slower, and measured so: on an Apple M6 on 2026-11-07, against 1.0 in the same session (a plain timing
  loop, each the best of 3 in the lowest of three rounds, Magnus then Fiddle), an xlsx sheet of 500k rows and 3
  columns reads in 146.7 and 146.4 ms against 139.4 and 140.4 ms, a 92 MiB delimited file of 2M rows and 6 columns
  in 54.8 and 57.1 ms against 51.3 and 53.1 ms and from a string in 58.4 and 59.8 ms against 56.4 and 54.9 ms,
  its values in 114.1 and 119.2 ms against 111.9 and 113.3 ms, and every verdict in 787 and 784 ms against 782 and
  789 ms. A file of 2M rows whose 6M cells are all bad reads in 104.0 and 105.7 ms against 89.3 and 90.6 ms, and
  with every verdict in 4.57 and 4.74 s against 4.08 and 4.09 s. `notes: false` takes most of that back (93.3 and
  94.2 ms, 4.08 and 4.05 s). A faults list costs a read nothing until it's asked for, and asked of those 6M bad
  cells, batch by batch, the same read takes 6.34 and 6.33 s. *(Ruby)*
- **PHP has all of that, through the Rust API.** The extension now reads through the Rust reader and
  sheet, every PHP-callable body under the same panic guard. `Dialect` adds `skip`, `headerRow`,
  `shortRows`, `longRows`, `encoding` and `tokens`; `SheetOptions` adds `skip`, `headerRow`, `range` (a new
  `Range`, with `Range::parse`), `tokens` and `notes`; `Column` adds `withTokens` and `withDateOptions`;
  readers take `notes:`. `Batch::detail()` and `Row::detail()` are HyperCast's `DetailedFault`, `origin()` on a
  batch and a row says where a verdict came from, and `Batch::faults()` with no column lists the batch's
  `Problem`s, their spans in bytes; with a column it is still that column's faults keyed by row.
  `DelimitedReader::suggest`, `suggestString` and `Sheet::suggest` answer with a `Suggestion`.
  `Header::ordinal()` now throws `AmbiguousColumn`, an `InvalidArgumentException`, for a name two columns
  share, where it returned the first; `find()` keeps that. `TabularFailure::Encoding` is new, and a header
  name that isn't UTF-8 is that failure. A column in Indian grouping reads beside a Western one, delimited
  and sheet, a column with no format is Western, and a format holding the retired word is an
  `InvalidArgumentException` before any cell, HyperCast's own refusal its previous exception. A stream is
  read one `fread()` a fill, so a stream that stalls hands over the rows it finished, and a stream's
  failure goes to the caller without ending the read. A stream wrapper's PHP calling back into the
  reader it feeds, or the `rows()` iterating it, mid-read, is a `LogicException`, and the read goes on.
  A suggestion past a whole sample says `truncated` for an `fread()` that returns `false`, and rethrows
  what a stream wrapper's PHP throws. It's slower than the extension was when it drove
  the core directly, and measured so: on an Apple M6 on 2026-11-07, against that build in the same session (a plain timing loop, each the best of 5 in the lowest of three rounds),
  an xlsx sheet of 500k rows and 3 columns reads in 152.3 ms against 143.3 ms, an 87 MiB delimited file of
  2M rows and 6 columns in 67.4 ms against 63.7 ms, its values in 102.2 ms against 99.0 ms, every verdict
  in 169.6 ms against 167.7 ms, and from a string in 63.2 ms against 64.4 ms. A file of 2M rows whose 6M
  cells are all bad reads in 118.1 ms against 106.0 ms, and with every verdict in 524.7 ms against
  515.8 ms; `notes: false` takes most of that back (109.5 ms and 517.5 ms). A faults list costs a read
  nothing until it's asked for, and asked of those 6M bad cells, batch by batch, the same read takes
  1.69 s. *(PHP)*
- **TypeScript has all of that, through the Rust API.** Every `#[napi]` body stays under `catch_unwind`.
  `Dialect` takes `skip`, `headerRow`, `shortRows`, `longRows`, `encoding` and `tokens`; `SheetOptions`
  takes `skip`, `headerRow`, `range` (a `Range`, from `Range.parse` or `Range.of`, or its text),
  `tokens` and `notes`, and `DelimitedOptions` takes `notes`; `Column` has `withTokens`,
  `withDateOptions` and a `format` getter. `detail()` and `origin()` on a batch and a row say what
  failed in a cell and where its verdict came from, and `Batch.faults()` is the batch's `Problem`s,
  worked out through Rust's `KeptBatch` when it's asked for, a new array each call.
  `DelimitedReader.suggest`, `suggestBytes` and `Sheet.suggest` answer an outcome of a `Suggestion`.
  `Header.require` answers `ambiguousColumn`, with `first` and `second`, for a name two columns share;
  `ordinal()` takes the first. A size the core refuses is the `argument` error in the core's words, a
  zero included, and a size is any whole number, 2^53 and past it included, the core judging it;
  `sampleRows: 0` means 1, and left out it is the thousand. `close()` answers `{ ok: true, value: null
  }`. A call back into a reader or sheet from JavaScript it runs mid-call, a getter in a plan or a
  setter on `Object.prototype`, is the `busy` error and the outer call goes on; a getter (`isBound`,
  `header`, `columnCount`, `records`), which answered `false` or `null` there, throws it, an `Error`
  with `code` `ERR_HYPERTABULAR_BUSY` and `kind` `busy`, each its own property, defined with no setter run. A column in Indian grouping reads beside a
  Western one, a column with no format is Western, and a format holding the retired word is a `plan`
  error before any cell. A `Batch` and a `Row` are now plain JavaScript, and a batch's copy an
  `ArrayBuffer` V8 owns, so a synchronous loop no longer holds every batch it drops until the event loop
  turns. Three passes over a 91 MB file of 2M rows and 6 columns in one synchronous loop, in Docker capped
  at 4 GiB on an Apple M6 on 2026-11-07, peaked at 1991 MB (`for (const read of reader)` and `while` over
  `read()`) and past 3.3 GB, where the run was stopped (`rows()`, holding a `Row` or not), before, and
  now, in five runs each, at 125 to 126 MB for both loops, 71 to 74 MB with `rows()` and 69 to 73 MB
  holding a `Row` on Node 26.13 (84 to 86, 66 to 69 and 66 to 71 MB on the 22.14 floor), and at 69 MB
  with `batchRows: 1` (62 MB on 22.14). The WebAssembly fallback's memory is the module's own, never
  shrinks and can't pass 4 GiB, and a batch's copy there still goes back only once the loop yields: the
  same loops peaked at 1640 MB and past 3.3 GB before, and at 980 MB now, 215 MB yielding after every
  read and 281 MB after every 64. All of it costs, measured in the same Docker on 2026-11-07 against
  HEAD's binding over HEAD's core, in the same session (a plain timing loop, each the best of 5 in the
  lowest of three rounds, each side run twice, yielding after each pass): the 91 MB file reads in 187.8
  ms against 172.5 ms, an xlsx sheet of 300k rows and 8 columns as text in 410.2 ms against 408.7 ms,
  and a file of 2M rows whose 6M cells are all bad in 143.9 ms against 128.3 ms, 130.9 ms against 129.6
  ms with `notes: false`; that loop's peak RSS is 155 MB against 702 MB. *(TypeScript)*
- **R has all of that, through the `_with` exports.** `delimited_dialect()` and the delimited readers take
  `skip`, `header_row`, `fill`, `long_rows` (`"reject"`, `"truncate"` or `"report"`), `na` and `encoding`;
  `sheet_reader()` and `read_sheet()` take `skip`, `header_row`, `na` and `range`; every `col_*()` takes
  `na`, `col_timestamp()`, `col_date()` and `col_datetime()` take `options` (and the last two `months`),
  and `col_text(raw = TRUE)` gives each cell's bytes. Each verdict gains `detail` (a factor, `"unknown"`
  for a code the installed hypercast doesn't name, its number in `detail_code`) and `origin`, a batch
  gains `row` and `original_names`, and repeated output names are made unique with `_1`, `_2`, ....
  `problems()` lists a batch's or a table's problems, from 1, when it's asked; text the core accepted that
  R can't hold is `ok` with an `NA` value and a `host_conversion` row there. `suggest_plan()` prints a
  plan to paste and `$plan` reads back; its report's `param` is the door's parameter, as every binding's
  is (an ambiguous date's is the first candidate's order, a number's 0); a failure among the sampled rows is its `failure`, with no plan,
  and past a whole sample a structural failure or a connection's read error only says `truncated`.
  `close()` on a reader, a sheet reader and a workbook lets go now and is safe twice. A plan holding a
  format the core refuses is an R error naming the column before the source is opened, and a stale
  packed layout (351 or 863) one naming both packages' versions; a name the header holds twice is
  refused (`which the header holds at columns 1 and 2`); sizes are the core's to judge, a `batch_rows`
  or `buffer_bytes` of 0 is refused, `sample_rows = 0` means 1, and a row past the limit is an R error
  naming its width. A column in Indian grouping reads beside a Western one. `"encoding"` is a new
  failure kind. A call back into a reader from a handler, an `on.exit`, a finalizer or a connection's R
  code mid-read is an R error and the read goes on. An interrupt mid-read loses nothing inside the read:
  the reader reads on, every row once and numbered right (a file's reader always does), or refuses
  every further read, naming the interrupt (a pipe or socket cut part-way); `close()` always works.
  An interrupt landing in the caller's own code after `read_batch()` returns can lose that one batch,
  which no R function can prevent; it shows as a gap in `row`. A connection's own read error is an R
  error of class `hypertabular_io_error`, a connection closed mid-read is an R error saying so, and a
  text-mode connection is refused, pointing to `file("stdin")` or a raw vector. `suggest_plan()` on a
  sheet refuses `batch_rows = 0` as the core does. *(R)*
- **`corpus/delimited.json` gains the two grouping policies**: `Western grouping refuses an Indian group`,
  `no format is Western grouping` and `Indian grouping reads an Indian group` tell flags `95`, no format and
  flags `607` apart on `1,23,456`, `an Indian column beside a Western one` reads both in one read, and
  `corpus/README.md` says a plan entry with no `format` means flags `95`. Further cases pin `12,34`,
  `1,2,345` and the other shapes neither policy reads. The suggester's cases cover each shape of the delimiter rule, alone and beside cells
  that settle or conflict.
- **`corpus/workbook.json` gains `suggest.xlsx`, `suggest.ods` and `suggest.numbers`**, with a `suggest`
  key on the cases that declare nothing, a comma decimal and Western grouping, each read by every binding's
  sheet: number cells and text cells holding the same digits, apart and in one column.
- **`corpus/workbook.json` holds a range's last column to its word**: `a range's last column ends the table` reads
  `B4:C12` of `ranges.xlsx`, `ranges.ods` and `ranges.numbers`, three cases, where every earlier range ran to the
  sheet's own edge and a read that dropped the last column passed every replay.
- **`corpus/workbook.json` carries `detail` and `origin` on its cells**, and `ranges.xlsx`,
  `ranges.ods`, `ranges.numbers`, `uncached.xlsx`, `uncached.ods` and `dates.xlsx`, `dates.ods` and
  `dates.numbers` join its packages, the last three the first whose text a declared field order or
  month names decide. Every case it
  had reads the same otherwise, including the four cells of `numbers-data.xlsx` and one each of
  `basic.xlsx` and `basic-1904.xlsx` that are now `uncached`.
- **A TypeScript binding for Node and Bun, on npm as `@skunkwerkx/hypertabular`.** A napi-rs
  addon over the core, ten packages: the root, eight platform addons and the WebAssembly
  fallback `@skunkwerkx/hypertabular-wasm32-wasip1`, for Node `^22.14.0 || >=23.6.0` and Bun. npm or `bun add` installs it, and Node runs either.
  Every cell is a `@skunkwerkx/hypercast` verdict, `{ ok: true, value }` or `{ ok: false,
  fault }`; opening, binding and reading return an `Outcome<T>` whose structural and argument
  errors are values, never thrown. Batches are copied out of the reader, and readers and sheets
  have `close()`. `@skunkwerkx/hypercast` `^1.1.0` is a peer dependency. Under the fallback
  on Windows, opening by path is a recorded loss (an `io` error; `fromBytes` reads the same
  file). *(TypeScript)*
- **An R binding, as the source package `hypertabular`.** A C shim over the core's C ABI,
  built from a vendored copy of the core and HyperCast with Rust 1.89 or newer; R 4.2 and
  later. Every cell is a hypercast `hypercast_verdict` with the faulted text beside its span,
  and a structural failure is a `hypertabular_failure` value; declaration misuse is an error,
  as in Python and PHP. Positions count from 1. Requires hypercast 1.1.0 on CRAN, which is
  submitted first. *(R)*
- **C: one archive per platform on the GitHub Release.** `hypertabular-<version>-<rid>.tar.gz`
  (`.zip` on Windows) for the eight desktop RIDs, android-arm64, android-x64, iOS, the iOS
  simulator, Mac Catalyst on both architectures, and wasi-wasm32 (the Apple mobile and
  WebAssembly ones static only): the generated header, the shared and static libraries,
  `hypertabularConfig.cmake` and `hypertabular.pc`. The C header stands alone: it carries
  HyperCast's value types and numeric format under the HyperTabular prefix. *(C)*
- **C++: `hypertabular.hpp` in every C archive.** A header-only C++17 binding over the C header
  and HyperCast's `hypercast.hpp` (1.1.0 or later): every cell is a `hypercast::Verdict<T>`,
  and opening, binding and reading return a `hypertabular::Result<T>`, everything `noexcept`.
  A C++ program finds both archives; it links HyperCast's library only if it calls HyperCast's
  own doors. *(C++)*
- **Python: a source distribution.** `hypertabular-<version>.tar.gz` now ships with the
  wheels, so where no wheel fits, pip falls back to building from source (Rust 1.89 or newer
  and a C linker), with HyperCast's own sdist beside it where HyperCast has no wheel; CI
  proves that build on x86-64 glibc 2.17 and on musl. Attested like the wheels. *(Python)*
- **Release plumbing for npm, CRAN, PIE and the C archives.** The preparation run packs,
  attests and preserves the ten npm tarballs, the R source tarball (both corpora and every
  workbook fixture they read), the 36 PHP extension zips (NTS and ZTS) and the C archives; the tag publishes
  npm through Trusted Publishing and attaches the zips and archives to the GitHub Release, and
  the R tarball goes to CRAN by hand from the preserved bytes. *(repository only)*
- **C# — .NET 10 is supported, on desktop, server and in the browser.** The package builds
  `net10.0` and `net11.0` from one source, and package validation holds their public APIs
  identical. On .NET 10 it covers Windows, Linux (glibc and musl) and macOS, framework-dependent
  and Native AOT, and Blazor WebAssembly; iOS, Mac Catalyst and Android stay .NET 11 and later,
  and a .NET 10 project for any of them stops at build with the new error `HYPERTABULAR003`,
  which names the platform. The package depends on HyperCast 1.1.0, the first with
  `lib/net10.0/`; a consumer on C# 14 reads a cell's verdict with HyperCast's `Match`, `Switch`
  or `TryGetValue`, and the union `switch` takes C# 15. `HyperTabular.targets` moves to
  `build/net10.0/` (and `buildTransitive/net10.0/`), one copy NuGet selects for both
  frameworks. CI runs the suite and the Native AOT smoke test on both frameworks on every
  desktop and musl leg, and a new `test-nuget-package` job proves from the packed package that
  a .NET 10 Native AOT publish links the core in. *(C#)*

### Changed

- **UTF-8 is checked with SIMD, through HyperCast.** HyperCast 1.1's text door validates 64 bytes
  and more with simdutf8 in native builds (NEON on aarch64, AVX2 or SSE 4.2 on x86_64, detected
  at run time) and in a wasm32 build with simd128, which reaches every delimited batch's region
  check and every workbook cell of 64 bytes or more. Go's translated core and the other
  WebAssembly builds have no simd128 and keep core's check, unchanged. Verdicts and fault spans
  are unchanged everywhere. simdutf8 is now in the core's dependency closure, below HyperCast,
  `no_std` and allocation-free: `rust/check-core.sh` requires HyperCast as the one direct
  dependency and allows nothing below it but simdutf8, so it holds with a HyperCast either side
  of the change.
  Text that fails costs more than before, since the SIMD check reads it whole before core finds
  the span (HyperCast's `docs/benchmarks.md`: a bad first byte in 4,100 bytes went from 1.2 ns
  to 244 ns on an Apple M6). Against 1.0 in one session (Apple M6, 2026-11-08,
  `docs/delimited.md`), a delimited file whose every cell is multi-byte now reads 32.1% slower,
  where validating it with core's check cost 146.2%; a workbook's cells under 64 bytes, as most are,
  keep core's check. R's `inst/AUTHORS` names simdutf8 and the work
  it builds on, and its author, the simd-json.rs developers and the simdjson authors are
  copyright holders in `Authors@R`.
  *(all packages)*
- **The core decides how much memory a batch may ask for.** One function, `kernel/size.rs`'s
  `check_batch`, reached through the new export `hypertabular_batch_check`, counts a batch's values,
  verdicts, notes, row widths and cell table, and refuses anything past 4 GiB, a fixed limit, before
  anything is allocated, at open and again when a plan is bound. C#, Java, Go, Swift, C++, Python,
  Ruby, PHP, TypeScript and R lose their own size checks and raise their usual argument error with the
  core's words. A path's sizes are judged before its file is opened, in every binding.
- **A suggestion is never refused for the size of its own batch.** Nobody set that number. The core says
  how many rows fit its 64 MiB budget (`sample_rows` in `HyperTabularRawBatchCheck`, which is now 48 bytes,
  with `width`), and the sampler reads as many batches as the sample takes, with the same answer. A row
  that alone is past the limit is `Error::RowTooWide` and each binding's own spelling, naming its width.
- **An explicit `batch_rows` or `buffer_bytes` of 0 is refused.** Rust and C++ read it as 1, and Rust's
  `Error` gains `ZeroBatchRows`, `ZeroBufferBytes`, `BatchTooLarge`, `RowTooWide` and `NoMemory`. An option left out
  keeps its default, and `SampleRows = 0` still means one row. Go's `SheetOptions` is a struct, so its
  `BatchRows: 0` is now the default where it was an error; the `BatchRows(0)` option is what's refused.
  Java's, Ruby's, Python's and PHP's `SheetOptions` no longer judge a 0 when they're made: the sheet
  does when it's opened. A negative count is still refused when they're made. Swift's 0 throws `BatchSizeError` where it trapped.
- **Ruby's over-large batch is an `ArgumentError`**, as in every other binding, where it was a
  `NoMemoryError` naming `batch_rows`; arrays are still asked for zeroed and left untouched. On a
  Ruby whose allocator is calloc, a refused allocation now raises rather than handing out a null
  pointer, and an allocation refused during a refill leaves a read that can be asked again.
- **A stray quote can't hide a separator or a line end.** A quote opens a quoted cell only as the
  cell's first byte, and once that cell closes, later quotes are literal up to the next separator or
  line end. This changes what a 1.0 read returns when a closed quoted cell is followed by more quotes,
  or a quote sits inside an unquoted cell: `"x"y"z,w"` is now two cells, `x"y"z` and `w"`, where 1.0
  read one, `x"y"z,w`; `12" pipe,1` is two cells where 1.0 failed with `unclosed_quote`; and three lines
  1.0 merged into one cell are three records, which can now be a `column_count` failure if their widths
  differ. Quoted text itself reads as it did (`"x"y"z"` is still `x"y"z`), a file with well-formed
  quoting reads the same, and no 1.0 case in `corpus/delimited.json` moved. The 1.0 exports change too;
  there's no legacy mode. It costs, on one Apple M6 on 2026-11-05 (`docs/delimited.md`): +1.0% to +3.8%
  on `structure` by engine, +3.5% to +6.9% on an all-quoted file, +0.9% on `cells` and +0.1% on typed
  batches. *(all packages)*
- **Text that isn't valid UTF-8 is `Malformed`, on every read.** With no encoding declared, a cell read
  through the text door whose bytes aren't UTF-8 is now that cell's `Malformed` verdict, at the
  offending byte: `\xe9t\xe9` is `Malformed` at offset 0. 1.0 handed such bytes out as text, and Rust's
  `text_str` replaced them with U+FFFD; a caller reading a Latin-1 file that way now gets faults, and
  should declare the file's `encoding`, or read a cell's bytes back with `raw`. A header name that isn't
  valid UTF-8 can't carry a verdict, so it's a structural failure, the new `Failure::ENCODING` (27)
  and `FailureKind::Encoding`, where 1.0 returned the bytes. The 1.0 exports change too. It costs
  (Apple M6, 2026-11-06, `docs/delimited.md`, `docs/workbook.md`): +6.8% on `cells`, +146.2% on a file
  whose every cell is multi-byte, +2.9% on typed batches with a text column, and +19.3% reading
  `text_heavy` from a workbook, whose real files moved +0.5% to +1.3%. A batch with an invalid cell
  reads its bytes a second time and asks the door about that row's text cells; past sixteen such
  rows in a batch, the rest are asked cell by cell. Those figures predate HyperCast's SIMD check (above). The
  release with it, against tag `v1.0.0` over HyperCast 1.0.0 in one session (Apple M6, 2026-11-10,
  `docs/delimited.md`, `docs/workbook.md`), with notes and the options' checks in it too: +15.8% on
  `cells`, +34.1% on the multi-byte file, +19.7% on a file with one bad byte in every batch
  (9.38 ms against 9.03 ms for the same file valid), +131.0% on one with a bad byte in every row,
  +8.5% on typed batches, +9.9% where four cells in five fail (HyperCast 1.1's doors are level
  with 1.0's there, under 1.0's core), +1.4% on `text_heavy` and +2.9% on the xlsx read, the other
  real files within 2.0%. An XLSX shared string is asked about once, when the strings are loaded, and
  a plain read's rows go through a sink without the header, width and policy steps on the SIMD
  engines. *(all packages)*
- **`hypertabular_delimited_state_size` reports 80 bytes, where 1.0 reported 64**, and
  `HyperTabularDelimitedState_SIZE` in the header is 80: the block `_init_with` writes holds what it
  was asked for after the first 64. A block only ever given to `hypertabular_delimited_init` may still
  be 64 bytes, and the core reads and writes no more of it. A 1.0 caller that allocates what the
  function reports works as before. One that requires the reported size to equal its own 64-byte block
  now fails that check against 1.1, as 1.0's cgo Go binding did. Such a caller keeps its 64 bytes
  for `_init` and drops the equality, or allocates what the function reports, as `_init_with` needs.
  *(C, and any caller of the C ABI)*
- **A name two columns share is refused where a plan is built from names.** Rust's `Header::require`
  now returns the new `Error::AmbiguousColumn { name, first, second }` for `id,name,id` and `"id"`,
  naming positions `0` and `2`, where 1.0 returned `0` and left the second `id` unread with nothing said.
  Code that relied on that gets the error at `?`, and says which it wants by position (`Column::i32(2)`)
  or asks for the first with `Header::ordinal`, which keeps its signature and its first match. Plans
  by position, and the names a header reports, don't change. `Error` is `#[non_exhaustive]`, so a match
  already needs its wildcard arm. Every binding's strict lookup refuses the same way, each entry above
  naming its error, and its optional lookup keeps the first match. *(all packages)*
- **A Rust delimited reader refuses a `NumFormat` with flag bits HyperCast doesn't define**, or
  with grouping policy `11`: `Error::Plan` when the plan is bound, where 1.0 ignored them.
  `hypertabular_delimited_fill` and `hypertabular_workbook_fill` still ignore every flag from bit 8
  up, as 1.0 did, so they read every format as Western (and what that answers has moved, in the entry
  below); `hypertabular_delimited_fill_with`
  and `hypertabular_workbook_fill_with` read them all and refuse what they can't honour. A Rust
  sheet refuses them as a delimited reader does. *(Rust)*
- **Unicode is credited.** HyperCast's core carries Unicode's case folding data, so every package
  names `MIT AND Unicode-3.0` and carries `LICENSE-UNICODE` where its format allows; PHP's release zips
  and Swift's hold only the library, and the README is their notice. R names Unicode, Inc. in
  `Authors@R` and `inst/AUTHORS`.
- **The JavaScript toolchain is Bun.** Every dependency tree here (`typescript/`, its browser
  smoke and consumer-test harness, `ruby/wasm-smoke/`) is locked by `bun.lock` and installed with
  `bun install --frozen-lockfile`, and CI, the local loop and Dependabot follow. npm keeps what
  publishing needs: `npm pack` and `npm publish`, and the consumer scenarios that install as an
  npm user does. Node still runs the suite, and the packages install from npm with either tool.
- **No legacy grouping, so a grouped number 1.0 read can now be refused.** HyperCast 1.1 has two grouping
  policies. Western is the default and flags `95`: a plan with no format, an all-zero format and every
  binding's `Invariant` are all Western, and a 1.0 export, which reads 8 bits of flags, reads every format
  Western. Indian is flags `607`, and only ever declared. These shapes read in 1.0 and are `Malformed`,
  detail `group_size`, by default now: `1,23,456`, `12,34,567`, `12,34`, `1,2`, `1234,567`, `1,2,345`,
  `1,2345,678`, `1234,567,890`, `1,234,56,789`, `0,125` and `0,000`, and under a space-group format `1 2 345`
  and `1234 567`, which were already `Malformed` under the default comma. They also gave
  wrong values: `0,125` read 125, `12,34` 1234 and `1234,567` 1234567. So a file that held them reads faults
  there. `1,000`, `1,234,567` and `0125` still read. **To read an Indian column, declare it on that column:**
  `NumFormat::INVARIANT.with_grouping(Grouping::Indian)` in Rust,
  `hypercast.Invariant.WithGrouping(hypercast.GroupingIndian)` in Go,
  `NumFormat::invariant().with_grouping(hypercast::Grouping::indian)` in C++,
  `NumFormat.Invariant with { Grouping = NumGrouping.Indian }` in C#,
  `NumFormat.INVARIANT.withGrouping(Grouping.INDIAN)` in Java, and flags `607` in C. One read
  can hold an Indian column beside a Western one. An Indian format refuses `1,234,567`, and neither policy
  reads `0,125`, `12,34` or `1,2,345`. Under a comma decimal `0,125` is 0.125, and `0.125` is refused. What
  it costs is measured in HyperCast's `docs/benchmarks.md`, and [its notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have the shapes and
  the locales neither policy matches. *(all packages)*
- **A format with a retired grouping bit is refused before any cell.** Value `256` is retired and never
  reused, so a stale `351` or `863` can't change meaning: `Error::Plan` in Rust, an error from Go's
  constructors, `Bind` and `Suggest`, `ErrorKind::plan` in C++, an `ArgumentException` from C#'s, in Java `NumFormat`'s own constructor, so
  that no plan can hold one, and `HYPERTABULAR_ERR_CONTRACT` from
  `hypertabular_delimited_fill_with`, `hypertabular_workbook_fill_with` and `hypertabular_suggest`, whose
  format isn't let off by a `size` of `0`. `hypertabular_delimited_fill` and `hypertabular_workbook_fill`
  can't see the bit and read those formats Western. A suggest judges every format its options declare
  before it opens anything: one replaced by a later one, one past the sample, an empty
  source and a broken one are all the format's refusal, never the file's. `RawSuggestion::NOTATION_FLAGS` is `95`.
  `hypertabular` now re-exports HyperCast's `Grouping`.
  Every other binding refuses it as its entry above says. *(all packages)*
- **Numbers read as HyperCast 1.1 reads them.** White space after a leading sign is now part
  of a number in a delimited cell and in an XLSX or ODS numeric cell, so `- 12` is -12 where it
  was `Malformed`. *(all packages)*
- **PHP — breaking: the package is a native extension, installed with PIE.** `pie install
  skunkwerkx/hypercast:<version>` and then `pie install skunkwerkx/hypertabular:<version>`
  install `ext-hypertabular`, an ext-php-rs extension that requires `ext-hypercast` 1.1 and
  hands out its classes for every cell; 36 pre-packaged zips (PHP 8.3, 8.4 and 8.5, NTS and ZTS,
  on Linux x64 and arm64, glibc and musl, macOS arm64 and Windows x64; no Intel macOS),
  attached to the GitHub Release. On a ZTS PHP it loads from php.ini only: a `dl()` stops the
  script with a warning naming the `extension=` line. `ext-ffi`, `php/src` and its committed libraries are gone, and the floor is
  PHP 8.3. The API keeps its names and behaviour, with these differences: `Batch`, `Row`,
  `Sheet` and `Header` cannot be constructed; rows come through a forward-only `RowIterator`;
  `Workbook::sheet()` raises a `TypeError` for anything but an int or a string; and
  `Tabular::isAvailable()` is true only when ext-hypercast is loaded; 1.0's `Scratch`, the suite's hook
  into a workbook's buffers, is gone, its buffers the Rust reader's own. Stubs ship in
  `php/stubs/`. *(PHP)*
- **Rust — minimum Rust is now 1.89** (was 1.88), the floor the Hyper* crates share. Builds
  from this repository and the Python sdist need `rustc` 1.89. *(Rust, Python)*
- **Go — one backend: the core translated to Go source, no cgo, no binary in git.** wasm2go
  translates the core's `wasm32-wasip1` build into `go/internal/core/core.go`, so
  `CGO_ENABLED=0` builds every GOOS and GOARCH, TinyGo and stock Go's `js` and `wasip1`
  included. `staticlib/`, the static and TinyGo backends, the `unsupported` files, the core
  freshness test and the `hypertabular_local` tag are gone. Each reader runs its own instance
  of the core; text in memory is copied in by chunks, and a buffer past the core's 4 GiB is an
  error. It is slower than the cgo binding it replaces, measured on an Apple M6 on 2026-11-05:
  1.7–2.1x per read (delimited 33.8 ms against 15.9 ms, an xlsx read 482 ms against 254 ms),
  with several times the memory (126 MiB against 21.7 MiB for the xlsx read); `go/README.md`
  has every row. The corpus tests are now `TestCorpus*`. Requires HyperCast's Go module
  v1.1.0. *(Go)*
- **Go — the core is translated with wasm2go's `-unsafe`.** Against the same wasm translated
  without it, delimited reads went from 61.3 to 34.4 ms and an xlsx read from 700 to 485 ms, with
  memory and allocations unchanged; an ods open is level. Every memory access is still
  bounds-checked, which `go/internal/core/bounds_test.go` now pins, and the whole suite runs on
  s390x under QEMU to prove the big-endian path. It remains slower than the cgo binding: 1.7–2.3x
  per read on 2026-11-08, against that binding over 1.0's core and over this release's alike. *(Go)*
- **Java runs on JDK 21.** The jar is multi-release: its base reaches the core through JNI,
  from a library (`libhypertabular_jni`) built for every platform, and JDK 25 and later keep
  FFM. The API, verdicts, exceptions and messages are the same on both, except that the load
  failure names the path's own library. On JDK 21, code that names `MemorySegment` needs
  `--enable-preview`; on the module path, `--enable-native-access` names
  `io.github.skunkwerkx.hypercast` as well. Requires HyperCast 1.1.0. *(Java)*
- **Swift — the core comes from the GitHub Release, not from git.** The root `Package.swift`
  takes `HyperTabularCore.artifactbundle.zip` and `HyperTabularCoreApple.xcframework.zip` by
  URL and checksum, written at release by the preparation run; `HYPERTABULAR_LOCAL_CORE` and
  the core freshness test are gone (`.github/scripts/local-core.sh` builds the checkout's own).
  Requires HyperCast 1.1.0. *(Swift)*
- **Python — requires `hypercast>=1.1.0`.** *(Python)*
- **The C header is generated.** `include/hypertabular.h` comes from the core's exports and
  the types they take, by cbindgen, with every ABI layout pinned and carried as a `*_SIZE`
  constant; Swift's bundle carries the same file, and Swift now uses its names. `Span::FLAG` is
  written as the literal it is. *(core, Swift)*
- **One dispatch prepares and attests, one tag publishes.** `prepare-release.yml` bumps, dates
  the CHANGELOG heading, runs the full board, packs every registry input, attests each package
  and the inventory, preserves them for 90 days and commits the staging commit; pushing
  `vX.Y.Z` at it verifies the preserved bytes, signs the release attestation from the tag and
  publishes. `stage-native-binaries.yml` is gone, with every committed binary; CodeQL runs on
  the preparation commit and must leave no open alert before the staging commit exists.
  *(repository only)*
- **The pull-request board runs only what the pull request could have broken.** The Rust core
  on all seven hosts first, then one job per host and binding, each started only when its
  tree, the core or the workflows changed, after its lint passed once on a Linux arm64 runner;
  `pr-board` is the one status to read. The release-mode board keeps its coverage and adds the
  core's suite, both corpora included, on big-endian s390x under QEMU. *(repository only)*
- **The Blazor archive is built on stable Rust**, with `-Zbuild-std` under `RUSTC_BOOTSTRAP=1`
  for that one build, no longer on a pinned nightly. *(repository only)*
- **Python — the `cp314-cp314t` wheel is gone.** Each platform ships two wheels where it
  shipped three, `cp311-abi3` for CPython 3.11 and newer with the GIL and `cp315-abi3.abi3t`
  for 3.15 and newer with or without it: 17 wheels a release where 1.0.0 had 25. The
  free-threaded 3.14 wheel proved the GIL-free path, and it was bound to one interpreter
  version, too narrow an ABI to carry. Free-threaded 3.13 and 3.14 now get `No matching
  distribution found`, as PyPy does; free-threading starts at 3.15. *(Python)*
- **C# — the iOS and Mac Catalyst smoke test runs on the SDK's defaults.** Its Mac Catalyst
  build set `Registrar` to `managed-static`, after a Release app on .NET 11's default,
  `trimmable-static`, aborted before `Main`, a fault then put down to the SDK. It was the
  test's own: its code touches no Apple API, so the trimmer dropped the platform assembly the
  runtime calls into at startup, and the iOS device build, linked in CI but never run, dropped
  it as well. One UIKit symbol used in a module initializer, as every real app uses UIKit,
  keeps that assembly and puts UIKit back in the native link, so the hand-added UIKit
  framework is gone too. The package is unchanged. *(repository only)*
- **C# — the benchmarks float to BenchmarkDotNet's prereleases.** 0.15.x, the newest
  stable, knows no runtime past .NET 10 and stops in its SDK validation on net11.0; the 0.16
  previews know .NET 11, and the harness takes them (`*-*`) until 0.16.0 ships stable.
  The in-process toolchain that sidestepped it is gone, so each benchmark runs in its own
  process, BenchmarkDotNet's default, as the README's C# row now is. *(repository only)*

### Fixed

- **The static library links without dead-stripping.** A plain `cc app.c libhypertabular.a`
  failed with an undefined `rust_eh_personality`; the static library now carries a weak hidden
  definition, so two Hyper* archives, or a Rust program with std's own, link as one. *(core)*
- **The shared library names itself.** Its macOS install name is
  `@rpath/libhypertabular.dylib` and its Linux and Android soname `libhypertabular.so`, so a
  program linked against it finds it through its rpath or the loader's search path. *(core)*
- **C# — a Debug Blazor WebAssembly build loads** (the same failure as
  [HyperUuid#50](https://github.com/SkunkWerkx/HyperUuid/issues/50)). Chromium refused an
  unstripped Debug build's `dotnet.native.wasm` with "module uses a mix of legacy and new
  exception handling instructions": the package translated the archive's legacy exception
  handling with a `wasm-opt` pass that rides the SDK's post-link step, and the SDK skips that
  step for an unstripped module. The archive no longer needs translating: it is built with
  `-Zbuild-std`, on stable Rust with `RUSTC_BOOTSTRAP=1` for that one build, for plain wasm,
  so it holds no exception-handling instructions and none of the features .NET 10's toolchain
  rejects (dotnet/runtime#132858), and the package's `--translate-to-exnref` item is gone. The
  same build is what lets .NET 10 link it, and the archive in the package shrinks from 3.6 MB
  to 663 KB. *(C#)*
- **Ruby — `require "hypertabular"` is silent under `ruby -w`.** 1.0.0 printed "method
  redefined" and "previous definition" lines for `Runtime::Delimited.start`, its `.version`
  and `Runtime.unescape` whenever the Magnus extension loaded: the Fiddle definitions sat on
  the singleton classes, where the extension then defined its own. They now live in
  `FiddleBackend` modules those owners extend, so the extension's definitions go in front of
  them through the ancestor chain and nothing is redefined, as in HyperUuid and HyperCast.
  Method lookup and both backends' behaviour are unchanged; `spec/native_backend_spec.rb`
  requires the gem under warnings and requires an empty stderr. *(ruby)*
- **C# — a stream that calls back into its reader is refused.** A `Read`, `ReadAsync`, `Bind` or
  `Rows()` loop from inside the reader's stream, or a second read while a `ReadAsync` waited on it,
  was answered from buffers the outer read had offsets into, and that read could hand out bytes that
  were never in the input. Each now throws `InvalidOperationException`, and the outer read goes on
  whole. A `Dispose` from there ends that read with `ObjectDisposedException` where it read on, and a
  `Read` inside `foreach (var row in reader.Rows())` makes the loop's next step throw where it went on
  in the next batch. *(C#)*
- **Java — a stream that calls back into its reader is refused**, on both paths. A `read()`, `bind`
  or `forEachRow` from inside the reader's stream corrupted the outer read, silently or with a
  structural failure that wasn't in the input; each now throws `IllegalStateException`, and the outer
  read goes on whole. A `close()` from there ends that read with a message saying so. On JDK 21 to 24
  a shared arena's segment handed to `DelimitedReader.of(MemorySegment, …)` or
  `Workbook.of(MemorySegment)` is copied when it's opened: closed by another thread in the middle of
  a native call, it was read after it was freed, where JDK 25 refuses that close until the call
  returns. A file read a thread interrupt ended now says so on every later read, where it said only
  that the channel was closed, and an interrupt past a whole `suggest` sample is thrown, not reported
  as `truncated()`. *(Java)*
- **Go — a source that calls back into its reader is refused.** A `Read` from inside the reader's
  `io.Reader` moved the buffer under the outer read, which then delivered rows lost, garbled or from
  the wrong line with no error, or a structural failure that wasn't in the input; a `Close` from
  there was accepted, though the read in hand went on to return a batch. A `Read`, `Bind` or `Close`
  from there now returns the new `ErrBusy` (`errors.Is`), closes nothing, and the outer read goes on
  whole. A `batch.All()` ranged over past the next `Read` or a `Close`, and a `reader.All()` or
  `sheet.All()` with either in its body, panic as a stale `Faults()` does, where they yielded the next
  batch's rows, or lost some and yielded others twice. Under TinyGo a `batch.All()` loop allocates five times,
  where it allocated three. *(Go)*
- **Swift — a source that calls back into its reader is refused.** A `read()` from inside the
  reader's `Source` freed the buffer the source was writing into, and a read could come back with a
  row from the wrong line. A `read()`, `bind(_:)` or `forEachRow` from there now
  throws the new `PlanError.busy`, and the outer read goes on whole. *(Swift)*
- **C++ — a source or a `for_each_row` callback that calls back in is refused.** A `read()` from
  inside a `DelimitedReader`'s source wrote into the buffer it had freed, and one from a
  `for_each_row` callback freed the cells under the walk, or the walk went on through the next
  batch. A `read()` or `bind()` from either, on a reader or a sheet, is now the new
  `ErrorKind::busy`, and the outer read goes on whole. A reader or sheet destroyed or moved from
  there, and a batch's row walk past the next `read()`, end the program with `std::abort()`, as a
  stale faults walk does. *(C++)*
- **Ruby — an IO or a plan that calls back into its reader is refused**, on both backends. A
  `read`, `each`, `bind` or `close` from inside the reader's IO moved the buffer under the outer
  read, which then handed out rows from the wrong line or lost them, or raised a structural failure
  that wasn't in the input; a second `bind` from the plan's own `each` was accepted. Each now raises
  `RuntimeError` ("the reader is in the middle of a call; nothing that call runs can call back into
  it", TypeScript's sentence), and the outer call goes on whole. *(ruby)*
- **Ruby — an exception that lands mid-read loses nothing**, on both backends. `Timeout.timeout`,
  `Thread#raise` or Ctrl-C's `Interrupt` landing after the core had read a batch, before the reader
  had moved on with it, made every later row's line wrong (`DelimitedReader`) or dropped the batch
  (`Sheet`); one that landed as the IO answered dropped what it had handed over, and one that landed
  in a read blocked on a pipe or socket stitched the bytes around the ones Ruby dropped into wrong
  rows. Such an exception now waits until the reader and the core agree, or until the IO's answer is
  held, and the batch it lands on is handed over by the next `read`, or by the next walk if it
  landed as `read` or `each` handed the batch out. The IO's own read still isn't held up while it
  blocks, so a `Timeout` still breaks it; that read raises the exception and every later one raises
  `RuntimeError` naming it. Since Ruby can't tell such an exception from the IO's own error
  (`Timeout.timeout(t, Errno::ETIMEDOUT)` raises one), anything the IO's read raises is now final,
  as in Rust and Go: a transient `IOError` or `SystemCallError` the reader went on after, it now
  refuses after. A walk left part-way through a batch
  (`break`, `first`, an exception from the block) leaves the rest of that batch behind, as Rust and
  Python do. On Fiddle, one that landed as a grown buffer was attached left the core reading memory
  the reader no longer held; the core is now handed a String that is never changed after, and the
  one before is let go of only once the next is held. Anything raised inside a read once the core
  has read past a batch makes every later read raise `RuntimeError` naming it. *(ruby)*
- **Python — Ctrl-C reaches a read of a path waiting on a FIFO.** `DelimitedReader.open`,
  `DelimitedReader.suggest` and `Workbook.open` read a file as CPython does: a signal that cuts an
  open or a read short runs its handler, and its `KeyboardInterrupt` isn't final, where the read
  waited for more data and then lost the batch it had read. A stream that hands over more than it
  was asked for, or something other than `bytes`, is final, where the next read went on without what
  it had handed over, sync and async; and a reader closed while a read awaits its stream ends that
  read with `ValueError` saying so. On Linux and macOS a missing path now raises CPython's own
  `FileNotFoundError` (`[Errno 2] No such file or directory: '<path>'`, with `filename` set), where
  it was Rust's `No such file or directory (os error 2)` with none; Windows keeps Rust's. *(python)*
- **Python — a reader or sheet in the middle of a call says so.** A call into a `DelimitedReader`
  or `Sheet` from another thread while one is inside a read, or from a stream's own `read`, raised
  PyO3's `RuntimeError` "Already borrowed"; it now raises `RuntimeError` ("the reader is in the
  middle of a call; nothing that call runs can call back into it", or "the sheet …"), the words
  every binding uses, and the outer read goes on whole. *(python)*

## [1.0.0] — 2026-10-25

The first stable release. Every package's public API and the platforms in the README's
[Platform support](README.md#platform-support) table are now a commitment under Semantic
Versioning: breaking or dropping either takes a new major version. Every binding reads Apple
Numbers beside XLSX and ODS; a live stream hands over each header and batch as it arrives,
in Rust, Python and Swift; a plan can no longer ask for unbounded memory, with one ordinal
cap, 2^20, in every binding; the Rust API is fixed at what a 1.0 can promise
(`#[non_exhaustive]`, a kernel surface of only what a binding calls); what was never used
goes (Java's GraalWasm backend, Python's `BACKEND`); and every shipped file verifies from
the release tag alone.

### Added

- **Rust — `Batch::values::<T>` is public.** It lends any typed column whole — decimals,
  UUIDs, timestamps, dates, date-times and durations as well as the primitives — where only
  the primitives had accessors (`batch.i64(col)` and its siblings, which it now underlies).
  The slice borrows the reader as theirs do, and a cell that did not cast holds the door's
  zero beside its verdict, as theirs does. *(Rust)*
- **C#, Java and Swift lend more columns whole.** C#'s `Batch.Values<T>` lends a decimal
  column as HyperCast's `RawDecimal`, a timestamp, Unix or Excel-serial column as
  `RawTimestamp`, a date column as `RawDate`, a date-time column as `RawCivil` and a
  duration column as `RawDuration`, zero-copy, and a time column as `ulong` nanoseconds; a
  UUID column is still read with `Get<Guid>`, since the core's RFC 9562 bytes are not a
  `Guid`'s layout. Java's `values(column)` returns the column segment for every door but
  text, read with HyperCast's `NativeValues` readers at their `*_BYTES` stride. Swift's
  `values(_:as: UInt64.self)` lends a time column's nanoseconds; HyperCast's Swift package
  has no value types with the core's layout, so the other doors stay a cell at a time.
  *(C#, Java, Swift)*
- **Apple Numbers.** Every binding reads a `.numbers` document — Apple's IWA packaging:
  Snappy-compressed chunks of Protocol Buffers objects — the way it reads XLSX and ODS,
  told by what the container holds, never by its name. Each table is a sheet, listed
  under its sheet's name (`<sheet> - <table>` when a sheet holds several, as Numbers'
  own Excel export names them) and hidden when its sheet is; cells are read from the
  v5 storage Numbers has written since 2018 — decimals and currencies as numbers, dates
  as wall clocks or, under a format that shows no date (a custom format's pattern looked
  up in the document's list), times of day, durations as spans, booleans, text and rich
  text with the runs joined, error cells as `#VALUE!`
  (the file does not record which error), the cells a merge covers as empty — and a
  row a tile leaves out is an empty row. The core's new `kernel/workbook/{iwa,snappy,
  proto,numbers}.rs` stay inside the no-panic, no-allocation proofs; the window holds
  only the chunks of the object in hand, and a deflated part is inflated into its far
  end. `Format` gains `Numbers` (`3`), `Opened.strings_bytes` is what the
  data-list parts decode to, and a new failure kind **`iwa`** (`26`, part `10`) names
  a Numbers part that is not what the packaging says — `found` 1 a broken chunk, 2 a
  malformed message, 3 an object a reference names that is not there (`expected` its
  identifier), 4 cell storage of a version the core does not read (`expected` the
  version). The `not_a_workbook` message now says "not an XLSX, ODS or Numbers
  workbook". The corpus gains `numbers-data.numbers`, written by Numbers 26.3.1 on
  macOS, its Excel export `numbers-data.xlsx`, and two generated Numbers packages in the
  layouts Numbers never writes small (chunks of a few bytes, seven-row tiles, wide
  offsets, a segmented string list, rows left out of their tile); `workbook.json` grows
  from 198 to 246 cases, every one of them new. *(Rust, C#, Java, Go, Python, Ruby, PHP, Swift)*
- **A core freshness check for the suites that link committed binaries.** Go's, Swift's
  and PHP's suites refuse to run against `go/staticlib`, the Swift artifact bundle or
  `php/src/native` when `rust/src`, `rust/Cargo.toml` or `rust/Cargo.lock` differ from the
  commit those binaries were staged from, and name the local-core command to run instead
  (Go and PHP stop before any test; Swift fails `CoreFreshnessTests`). CI, the local core,
  shallow clones and a checkout with fresh binaries written into the tree skip it.
  `.github/scripts/local-core.sh` finds a Forge checkout at `../HyperForge` as well as
  `../.github`. *(repository only)*

### Changed

- **The release flow, before its first run here.** The wheels are uploaded by a step of
  `release.yml`'s own `pypi-publish` job, so their PEP 740 publish attestations name
  `release.yml`, as 0.9.0's do (the Forge's composite action nested the upload, where it
  cannot find its own container image; HyperUuid's and HyperCast's 1.0.0 wheels went up
  from another workflow for it); staging starts by itself once the release-mode CI on the
  preparation commit is green, and a second request for a commit already staged is skipped
  with a notice; the Forge commit's step output is `revision`, which keeps CodeQL's
  `actions/cache-poisoning/poisonable-step` quiet. *(repository only)*
- **Built on HyperCast 1.0.0.** Every binding depends on HyperCast's first stable release,
  within its major: the crate `1.0.0`, NuGet `1.0.0`, Maven `1.0.0`, Go `v1.0.0`, Python
  `>=1.0.0,<2`, Ruby `~> 1.0`, PHP `^1.0.0` and Swift `from: "1.0.0"`, where it was `0.9.0`
  (Swift: a commit pin, which no tagged package may carry). *(all packages)*
- **Swift — on HyperCast 1.0.0, whose doors do not throw.** HyperCast's Swift doors and
  `Cast.nativeVersion()` dropped `throws` (bad data was always the verdict's `Fault`), so a
  call needs no `try`; one left in place compiles with a warning. *(Swift)*
- **Breaking — Rust: the kernel's public surface is what a binding calls, and no more.**
  `kernel::abi`, `kernel::door`, `kernel::exports`, `kernel::delimited::fill`, and
  `kernel::workbook`'s state, memory, format constants and `book` and `rows` modules stay
  public; the delimited engine, scanner and unescape, the inflate, and the workbook's zip,
  XML, styles, number, part, sort, cell, Snappy, protobuf, IWA and Numbers modules are
  private to the crate, so 1.0.0 does not freeze them. The tests that drove
  them directly are compiled into the crate from `tests/` and every integration target is
  listed in the manifest. *(Rust)*
- **Breaking — Rust: `Column`, `Dialect`, `SheetOptions`, `Door`, `Error`, `Format` and
  `FailureKind` are `#[non_exhaustive]`.** A setting, a door, a format the core learns to
  read or a failure it can report is an addition without a new major version once 1.0.0 is
  out. The three configuration structs are built with their constructors, constants and
  builder methods rather than struct literals; a match on an enum outside the crate names a
  wildcard arm, and a failure code this binding does not know reads as `Container`, as it
  already did. *(Rust)*
- **C# — `HyperTabular.targets` no longer lists an `EmccExportedFunction` per export.** The
  `NativeFileReference` is the whole Blazor WebAssembly wiring: the SDK's generated pinvoke
  table calls each native function by symbol, so the link resolves them against the archive
  and `LibraryImport("*")` reaches them through that table. The smoke test reaches all
  fourteen exports with the list gone (on SDK 11.0.100-rc.2) and fails at the link with the
  archive unlinked, so the list was dead weight. *(C#)*
- **The release is two acts at one tag, and every shipped file verifies from the tag.**
  Pushing `vX.Y.Z` packs every registry input, attests each package and the release as a
  whole, preserves the bytes and plans every upload without publishing; a dispatch of
  `release.yml` with `prepared_run_id` publishes from the preserved bytes through what was
  the recovery path. The release attestation is one statement signed at the tag whose
  subjects are every registry package and every native library committed for Go, Swift and
  PHP, and whose predicate is the release inventory, so
  `gh attestation verify <file> --repo SkunkWerkx/HyperTabular --source-ref refs/tags/vX.Y.Z
  --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1` answers for
  any of them, including the wheels and the committed natives that build provenance alone
  tied to the preparation commit on master (SkunkWerkx/HyperUuid#35). The GitHub Release
  notes come from this file's section for the version plus the verification commands, and a
  tag whose CHANGELOG lacks the section is refused before anything publishes. The job bodies
  of `release.yml`, `prepare-release.yml`, `stage-native-binaries.yml` and every static check
  in `ci.yml` moved to the Forge (`SkunkWerkx/.github`, called at master, composite actions
  where the repository's own identity signs or publishes), with this repository described to
  it by `.github/hyper.json` and `.github/scripts/abi_probe.c`; CI gained `check-semver`
  against the last crates.io release, the no_std consumer build and the shared library's
  export check on every RID. Dependabot bumps every ecosystem in one monthly pull request,
  every third-party action is pinned to a commit, and CodeQL runs weekly. CI also runs the
  Rust suite under wasmtime on `wasm32-wasip1` (`test-wasi`) and SwiftLint's `missing_docs`
  over the Swift package's public API (`swift/.swiftlint.yml`), on every pull request.
  *(repository only)*

### Removed

- **Java — the GraalWasm backend.** The jar no longer carries the core a second time as a
  `wasm32-wasip1` module, and `Tabular.backend()`, the `hypertabular.backend` property and the
  fall-back to the module on a platform without a native build are gone: the binding is FFM
  downcalls into the library it bundles. The module served a platform nobody had asked for,
  and replaying the suite through it in interpreter mode on five CI legs cost more than every
  other step of a leg together once the Numbers reader was in it (23 minutes against 15 on
  linux-x64). `cargo wasm-module` and the `wasm32-wasip1` export flags leave
  `rust/.cargo/config.toml` with it. *(Java)*
- **Python — `hypertabular.BACKEND`.** It was always `"native"` once the PyO3 extension
  became the only backend, so it named no choice. `hypertabular.native_version()` still
  reports the core the extension was built from. *(PyPI)*

### Fixed

- **A worksheet cut off between two rows is a broken part, not a shorter sheet.** An XLSX
  worksheet whose XML ended after a whole `</row>` without closing `sheetData` — cut off by
  the file, or by a zip directory that declared the part that long — read as a shorter
  sheet: the rows past the cut were gone with no failure. The part reader now counts the
  elements a part has opened and not closed, and the end of a part inside one is
  `FailureKind::Xml` (`found` 6, an unterminated element), raised after every intact row
  before it, final for the sheet, in every part the core reads — a string table cut
  between two strings is refused at load. *(all packages)*
- **A string table that claims a gigabyte is not given one.** The size a workbook's zip
  directory declared for `sharedStrings.xml` was what every binding sized its string arena
  by, before the part was read, capped only at 256 MiB; a kilobyte of XLSX could ask for
  all of it. The core now holds that figure to what the part's compressed bytes can inflate
  to (1032 to one, deflate's own ceiling), so the arena is never larger than the part
  could fill, and an honest file is sized as before. *(all packages)*
- **Rust — the delimited row ceiling no longer depends on the buffer the reader began
  with.** A stream's buffer started at `buffer_bytes` and doubled unclamped, so a record
  longer than `DelimitedReader::MAX_ROW_BYTES` was read when the buffer happened to hold it
  and refused when it did not. The buffer now starts at most at the ceiling and grows to it
  and no further, through `Read` and the `async` feature's `AsyncRead` alike, so a record
  that fills the ceiling without ending is `FailureKind::RowTooLong` whatever the buffer
  began at — as the other seven bindings already read. *(Rust)*
- **Rust — a live delimited stream no longer holds back a header or batch that has arrived.**
  A refill, through `Read` or the `async` feature's `AsyncRead`, used to read until the
  buffer was full or the source ended, so a pipe or a socket that paused after a few
  records held them back; it now goes back to parsing after any read that returns bytes.
  Interrupted reads, records split across reads, cancellation and terminal errors behave
  as before. *(Rust)*
- **Python — the same, for a file and a file object.** A file `DelimitedReader.open` opened
  (a FIFO, say) was read until the buffer was full, and a buffered file object's
  `read(size)` over a pipe or a socket waits for all of `size`. The reader now parses after
  any read that returns bytes, calls a file object's `read1(size)` where it has one, and
  opens the path with the interpreter released, so a FIFO whose writer is another Python
  thread no longer deadlocks the open. *(PyPI)*
- **Rust — a slice of 2 GiB or more is read.** `from_slice` handed the core the whole slice
  in one call, and the core takes under 2 GiB a call (its offsets are 31 bits), so a larger
  slice failed at once as `Error::Separator`. It is handed over in windows under 2 GiB now,
  as Python already hands over `bytes`, and only a single record that long is
  `FailureKind::RowTooLong`. *(Rust)*
- **Swift — a live asynchronous sequence is read as it arrives, in whole batches.** A reader
  opened from an `AsyncSequence` awaited its bytes until the read buffer filled or the
  sequence ended, so a header or row that had arrived from a live source (`URLSession.bytes`)
  was held back. A task owned by the reader now awaits the sequence, up to `bufferBytes`
  ahead, and a read takes everything that has arrived, waiting only when nothing has: a
  batch is `batchRows` rows when that many are in, and a row from a paused source is read
  within 200 µs. A cancelled read still loses nothing, a sequence's error still follows
  every byte before it, and the task stops when the reader is released. The sequence is
  passed `sending`. *(Swift)*
- **Swift — a FIFO or pipe opened by path is read as it is written.**
  `DelimitedReader(contentsOfFile:)` read through `FileHandle.read(upToCount:)`, which on a
  FIFO waits for the whole count; off Windows it makes one `read(2)` per refill. *(Swift)*
- **Every binding refuses a plan ordinal of 2^20 (1,048,576) or more, as its own plan
  error.** No workbook format has that many columns (XLSX and ODS stop at 16,384, Numbers
  at 1,000), and at the cap one row of cell table is at most 8 MiB and a sheet's row slots
  at most 16 MiB, so no plan can ask for more. Before, the bounds differed and some broke:
  Rust took any ordinal that fit a `u32`, so on `wasm32` `Column::text(usize::MAX)`
  overflowed and panicked and on a 64-bit host an ordinal near `u32::MAX` sized a cell table
  of tens of terabytes and aborted on the allocation; Swift crashed on a precondition for an
  ordinal of 2³¹ − 1 or more; C# overflowed `Ordinal + 1` at `int.MaxValue` and bound a
  corrupt width; Java took every `int`; Python, Ruby, Go and PHP stopped at 2³¹ − 1. Now an
  ordinal of 2^20 or more is refused for a delimited plan and a sheet's alike, before
  anything is allocated: Rust `Error::Plan`, C# `ArgumentException`, Java
  `IllegalArgumentException`, Swift the new `PlanError.outOfReach(column:)`, Python
  `ValueError`, Ruby `ArgumentError`, PHP `InvalidArgumentException`, Go's plan error.
  *(Rust, C#, Java, Go, Python, Ruby, PHP, Swift)*
- **A plan column's ordinal no longer sizes a batch of cell table.** A delimited reader's
  cell table took the plan's widest ordinal plus two entries for every row of a batch, and
  blanked them all on every row, so on a two-row file `Column::i64(100_000)` took 3.2 GB,
  2 000 000 took 8 GB, and 1 000 000 000 was killed past 10 GB with nothing read. Now,
  once a header has fixed the width every record has, `bind` — or a constructor given the
  plan, as soon as it has read the header — refuses a column at or past that width as the
  binding's own plan error naming it (Rust `Error::Plan`, C# `ArgumentException`, Java
  `IllegalArgumentException`, Swift `PlanError.outOfReach(column:)`, Python `ValueError`,
  Ruby `ArgumentError`, PHP `InvalidArgumentException`, Go's plan error): such a column
  could only ever be empty. Without a header, where the width is not known at bind, the
  table holds as many rows as 4096 × 257 entries (8 MiB, a default batch of a plan
  reaching 256 source columns) allow, and never less than one: a plan reaching further
  reads shorter batches, and its far columns read as empty in the memory one row's reach
  takes, at most 8 MiB under the 2^20 cap. An ordinary plan's batches are full as before, and a sheet is unchanged — its cell
  table is a batch of plan columns, not of reach, and its header does not fix the width of
  the rows below. The corpus case "a plan is a projection, in its own order", which read
  ordinal 9 behind a four-name header as empty, reads the same record without a header.
  *(Rust, C#, Java, Go, Python, Ruby, PHP, Swift)*

## [0.9.0] — 2026-10-21

Built on HyperCast 0.9.0: its culture-fidelity sweep reaches every plan column's format, and
its free-threaded Python extension lets this one go free-threaded beside it. Also the
consumer feedback from the Blazor WebAssembly integration, in the same shape as HyperCast's.

### Added

- **A plan column's format carries everything HyperCast 0.9.0's `NumFormat` declares.** The
  numeric doors of a plan column read the currency amount's own separators, further spellings
  of the minus and plus signs and the percent symbol, native digits beside ASCII, separators
  of one or two characters, and the `TRAILING_SIGN` lenience, as HyperCast's doors do, because
  the core casts every cell through them. Nothing a binding's caller writes changes: each
  binding's `Column` takes HyperCast's own `NumFormat`, which now takes the whole
  declaration (`NumFormat.From(CultureInfo)` in C#, `from(Locale)` in Java, and the rest).
  *(core, every binding)*
- **C# — error `HYPERTABULAR002` when the `wasm-tools` workload is missing.** Without the
  workload a Blazor WebAssembly publish skips linking the native core with nothing louder than
  the SDK's own warning, and ships a runtime with no `hypertabular_*` symbols; `NETSDK1147`
  never fires for this. The package's targets now stop the build instead. An app that keeps a
  managed fallback can set `HyperTabularRequireWasmWorkload` to `false`. HyperCast's and
  HyperUuid's packages carry the same check. *(C#)*
- **Python — free-threaded CPython, with the GIL off.** Each platform now gets three wheels
  where it had one: `cp311-abi3` for CPython 3.11+ with the GIL as before, `cp314-cp314t` for
  free-threaded 3.14, and `cp315-abi3.abi3t`, PEP 803's stable ABI for 3.15+, which serves 3.15
  with or without the GIL. PyO3 0.30's modules declare they need no GIL, and HyperCast 0.9.0's
  do too, so a free-threaded interpreter keeps it off after `import hypertabular`, and readers
  on different threads run in parallel. A `Batch` and a `Workbook` can be shared by any
  number of threads, and a column's `values` is built once whichever thread asks first. A
  reader or a sheet is still read by one thread at a time: another thread calling into one
  meanwhile gets `RuntimeError`, never a race on its buffers. Nothing needed changing for
  that, and new tests hold it on 3.14t and 3.15t. PyPI gets 25 wheels per release where it
  got 9. CI installs each wheel on its own interpreters and checks the GIL stays off, and runs
  the whole suite on free-threaded 3.15 on every platform. A build from a checkout needs
  maturin 1.15 or newer. *(PyPI)*

### Changed

- **Breaking — HyperCast 0.9.0 is the floor in every package**: `hypercast = "0.9.0"` for the
  crate, `hypercast>=0.9.0,<0.10` on PyPI, `HyperCast` 0.9.0 on NuGet, `hypercast` 0.9.0 on
  Maven Central, `~> 0.9.0` for the gem, `^0.9.0` on Packagist, `from: "0.9.0"` for the Swift
  package and `v0.9.0` for the Go module. The format layout below is 0.9.0's, and the crate's
  `python-values` is built on PyO3 0.30, which the Python binding needs for free-threading
  (Cargo links one PyO3 per build). The types this crate re-exports from HyperCast
  (`Decimal`, `NumFormat`, `Fault`, the temporal types and the rest) are therefore HyperCast
  0.9's, so a crate that names them through its own `hypercast` dependency moves to 0.9 with
  this one. *(every package)*
- **Breaking — `ColumnSpec` carries HyperCast 0.9.0's `RawNumFormat`, so it is 104 bytes where
  it was 44.** The spec's ordinal, door and parameter are followed by the format as 0.9.0 lays
  it out: a size word, then the `NumFormat` in place, 92 bytes, 4-aligned (HyperCast's
  `rust/src/abi.rs` has the offset table). All zeros, a size of `0`, is still the invariant
  notation, and a size the core does not read is a contract violation, as a format HyperCast's
  doors would refuse always was. Every binding restates the layout where it builds a spec: the
  C header, the C# `RawColumnSpec`, Go's cgo preamble, Java's `SPEC_BYTES`, PHP's FFI
  declarations, Ruby's packed spec and the Magnus and Python extensions; each writes the format
  through HyperCast's own `ToRaw`, `Raw`, `writeFormat`, `packed` or `rawFormat`, so no bridge
  lays a byte of it out itself. In Rust, `NumFormat`'s separators are `Symbol<8>` where they
  were `char`, so a format is built with `NumFormat::new` and its `with_*` methods rather than
  a struct literal; `Symbol` and `NativeDigits` are re-exported beside `CurrencySymbol`. The
  corpus replays are byte-identical in all eight languages. *(core, every binding)*
- **C# — the browser-wasm archive is at `runtimes/browser-wasm/nativeassets/net11.0/`**, the
  path the forge stages, where the package's targets named `net10.0/`: a Blazor WebAssembly
  link against a package packed by the current forge would not have found the archive. The
  targets now live in `build/net11.0/HyperTabular.targets`, with `buildTransitive/` importing
  them, as HyperCast's do, so a project that reaches the package through a class library gets
  the same wiring. *(C#)*

### Fixed

- **C# — the browser-wasm archive carries build provenance, and the package checks it.**
  0.8.0's release notes said every artifact was attested; the archive a Blazor WebAssembly app
  links, `runtimes/browser-wasm/nativeassets/net10.0/libhypertabular.a`, was built in CI but
  never attested, the same gap HyperCast's 0.8.0 package had. The forge now attests it where
  it is built and `hyper-pack-nuget.yml` verifies every native it packs, failing the release
  on one without provenance. The v0.8.0 release notes are corrected. *(C#)*
- **C# README — the Blazor WebAssembly section says what the link costs and needs.** The
  archive is linked even when the trimmer drops `HyperTabular` from the app, since the targets
  run before the trimmer and export all 14 entry points regardless, and the link needs the
  `wasm-tools` workload (`HYPERTABULAR002` above). *(C#)*
- **README examples.** Every README example was compiled and run against the local build, the
  root README's C# and Rust ones included. *(all)*

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

### Notes

- **The workbook read before this release, measured.** The std workbook reader the core
  replaced, inflating with zlib-rs, read the 300 000-row workbook in 505 ms against the
  core's 604 ms (`tests/kernel_workbook.rs`, `throughput`, measured before that reader was
  deleted). The difference was long guessed to be the inflate, since the core's runs at
  about two thirds of zlib-rs's speed; measured (Rust, best of three, before the tokenizer
  pass below), it was not: the core's inflate alone took 117 ms over the xlsx sheet's
  111 MB and 93 ms over the ODS part's 360 MB, and casting was not it either (an empty plan
  read the xlsx in 689 ms against the full plan's 705 ms, ODS 1.66 s and 1.72 s). The rest
  was the XML — tokenizing the parts and assembling rows — about 570 ms for the xlsx sheet
  (195 MB/s) and, for ODS, about 380 ms of the open's scan and 1.1 s of the read
  (330 MB/s). The text door was the one plan that cost more: all eight columns through it
  read in 1.03 s and 2.01 s. *(crate, every binding)*
- **The tokenizer pass, 2026-10-05.** Profiled (samply, the ODS read), the XML was most of
  it in two places. `tag()` found a tag's `>` a byte at a time, minding quotes, over ODS's
  long cell tags; it now jumps from one `>` or quote to the next eight bytes at a time, as
  `find_byte` finds one byte. Every attribute lookup walked the tag's attributes from the
  start — four or five a cell for ODS, three (`r`, `s`, `t`) for XLSX; each cell's are now
  gathered in one pass, each the first of its name as the lookups found it. And the
  nine-byte `<![CDATA[` comparison, a `memcmp` call, ran for every ordinary tag until `<!`
  was asked first. Best of several runs, Rust, before and after the three: XLSX read
  710 → 580 ms, ODS read 1.88 → 1.19 s, ODS open 530 → 320 ms. *(crate, every binding)*
- **What the first cross-binding table changed.** Ruby and PHP each ran a scan over every
  span of a batch, in the host language, to find how much of the arena to copy, where the
  core's own `arena_used` already says (Ruby's read went from 1.05 s to 0.68 s before any
  values were made); and PHP decoded its record doors with an `unpack()` a cell, now one a
  column (0.28 s to 0.14 s of decoding for the xlsx file). *(Ruby, PHP)*

[1.0.0]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.9.0...v1.0.0
[0.9.0]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/SkunkWerkx/HyperTabular/releases/tag/v0.7.0
