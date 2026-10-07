# Changelog

All eight packages in this repository — the `hypertabular` crate and the C#, Java, Go,
Python, Ruby, PHP and Swift bindings — share one coordinated version, so one changelog covers
all of them. Each entry marks which packages it actually affects.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- **Benchmarks in every ecosystem's own harness** — Criterion, BenchmarkDotNet, JMH,
  `testing.B`, package-benchmark, pyperf, benchmark-ips and phpbench — over the same two
  300 000-row workbooks and the same checksum; `docs/workbook.md` has the table.
- **Every platform the forge builds.** linux-x64, linux-arm64, linux-musl-x64,
  linux-musl-arm64, osx-arm64, win-x64 and win-arm64 tested on real hardware, osx-x64
  built and tested at the core; every native library carries a build-provenance
  attestation, verified again before it is staged or packed.

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
  archive linked in, and each runs in headless Chrome in CI on every pull request.
  *(crate, C#, Go, Swift, Python, Ruby)*
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

[Unreleased]: https://github.com/SkunkWerkx/HyperTabular/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/SkunkWerkx/HyperTabular/releases/tag/v0.7.0
