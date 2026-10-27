# Changelog

All eight packages in this repository — the `hypertabular` crate and the C#, Java, Go,
Python, Ruby, PHP and Swift bindings — share one coordinated version, so one changelog covers
all of them. Each entry marks which packages it actually affects.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Python — the `cp314-cp314t` wheel is gone.** Each platform ships two wheels where it
  shipped three, `cp311-abi3` for CPython 3.11 and newer with the GIL and `cp315-abi3.abi3t`
  for 3.15 and newer with or without it: 17 wheels a release where 1.0.0 had 25. The
  free-threaded 3.14 wheel proved the GIL-free path, and it was bound to one interpreter
  version, too narrow an ABI to carry. Free-threaded 3.13 and 3.14 now get `No matching
  distribution found`, as PyPy does; free-threading starts at 3.15. *(Python)*

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
