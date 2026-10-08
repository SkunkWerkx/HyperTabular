# HyperTabular

[![CI](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml/badge.svg)](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)
[![crates.io](https://img.shields.io/crates/v/hypertabular.svg)](https://crates.io/crates/hypertabular)
[![NuGet](https://img.shields.io/nuget/v/HyperTabular.svg)](https://www.nuget.org/packages/HyperTabular)
[![Maven Central](https://img.shields.io/maven-central/v/io.github.skunkwerkx/hypertabular.svg)](https://central.sonatype.com/artifact/io.github.skunkwerkx/hypertabular)
[![PyPI](https://img.shields.io/pypi/v/hypertabular.svg)](https://pypi.org/project/hypertabular/)
[![Go Reference](https://pkg.go.dev/badge/github.com/SkunkWerkx/HyperTabular/go.svg)](https://pkg.go.dev/github.com/SkunkWerkx/HyperTabular/go)
[![Swift Package](https://img.shields.io/github/v/tag/SkunkWerkx/HyperTabular?label=swift%20package&sort=semver)](https://github.com/SkunkWerkx/HyperTabular/tags)
[![Gem](https://img.shields.io/gem/v/hypertabular.svg)](https://rubygems.org/gems/hypertabular)
[![Packagist](https://img.shields.io/packagist/v/skunkwerkx/hypertabular.svg)](https://packagist.org/packages/skunkwerkx/hypertabular)

**Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS — read a batch at a time into typed columns, with a [HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell. Written once in Rust as a core with no standard library, no allocation and no panic, called directly from C#, Java, Go, Swift, Python, Ruby and PHP, with a shared conformance corpus so every binding agrees byte for byte.**

Reading a spreadsheet or a CSV into typed values usually means one of two things: a library that guesses (the separator, the header, whether `1/7/2026` is January or July, whether `12.185` is a thousand or twelve) and throws on the first cell it cannot read, or a string per cell and a parser per cell in the host language. HyperTabular does neither. The dialect and the plan — which source column, through which HyperCast door, in which notation — are declared, and nothing is sniffed. A cell that does not cast is a verdict in its column, `Empty`, `Malformed` or `OutOfRange` with the span of the offending text, and the read goes on. And the work happens in one native call per batch: the binding allocates the column buffers once, the core fills them, and the boundary is crossed once per few thousand rows, not once per cell.

```csharp
// C# (.NET 11+) — the same batch for delimited text and for a sheet
Column[] plan = [Column.Int32(0), Column.Text(1), Column.Double(2)];
using var reader = DelimitedReader.Open("orders.csv", Dialect.Csv, plan);

while (reader.Read() is { } batch)
{
    ReadOnlySpan<int> ids = batch.Values<int>(0);          // a whole column, as the core wrote it
    for (var row = 0; row < batch.Rows; row++)
    {
        var line = batch.Get<double>(2, row) switch        // or one cell, as HyperCast's union
        {
            Success<double> score => $"{batch.GetString(1, row)}: {score.Value}",
            Fault fault => $"line {batch.Line(row)}: {fault.Reason}",
        };
    }
}

using var book = Workbook.Open("orders.xlsx");             // XLSX or ODS, told by what is in it
var sheet = book.Sheet("Orders", SheetOptions.Default, plan);
while (sheet.Read() is { } batch) { /* the same batch */ }
```

```rust
// Rust — the API over the core
let plan = [Column::i64(0), Column::f64(2), Column::text(3)];
let mut reader = DelimitedReader::open("data.csv", Dialect::CSV, &plan)?;
while let Some(batch) = reader.read()? {
    let ids: &[i64] = batch.i64(0);
    let amount: Result<f64, Fault> = batch.get(1, 0);
}
```

## Install

| Language | Package |
| --- | --- |
| [Rust](rust/) | `cargo add hypertabular` |
| [C#](csharp/) | `dotnet add package HyperTabular` |
| [Java](java/) | `implementation("io.github.skunkwerkx:hypertabular:<version>")` |
| [Go](go/) | `go get github.com/SkunkWerkx/HyperTabular/go@latest` |
| [Swift](swift/) | `.package(url: "https://github.com/SkunkWerkx/HyperTabular", from: "<version>")` |
| [Python](python/) | `pip install hypertabular` |
| [Ruby](ruby/) | `gem install hypertabular` |
| [PHP](php/) | `composer require skunkwerkx/hypertabular` |

Each brings HyperCast 0.7 with it: the verdict, fault, number-format and declared-option types a batch hands out are HyperCast's own, from its package, not copies.

## The shape

- **One batch type.** `read()` returns a batch — rows, the line each row came from, a whole column as the core wrote it (a span, a slice, a `memoryview`, a `MemorySegment`), a verdict beside each value, one cell as HyperCast's union, and the raw text a cell was cast from — or nothing when the input is done. The same type for delimited text and for a sheet.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and whether blank lines are skipped; `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size; the plan states each column's door and, for numbers, its `NumFormat` (separators, grouping, accounting parens, currency — or HyperCast's structural `DETECT`, which refuses the ambiguous rather than guess). Dates in text are read under a declared field order.
- **HyperCast is the judge.** A text cell means exactly what HyperCast's door would say of the same text. A workbook's numeric cell is converted from the stored `f64` by HyperCast's typed doors: an exact decimal (the shortest one that names the double, so a stored `0.1` is one tenth), a date or date-time under the workbook's own date system (1900 or 1904), a time of day, a span — each time snapped to the fewest fractional-second digits that store as the same double, so Excel's `9999-12-31 23:59:59` reads on the second.
- **A bad value is a verdict; a broken file is an error.** A record of the wrong width, input that ends inside a quoted cell, a workbook whose container or parts cannot be read, an encrypted package: each is a structural error, raised only after every intact row before it has been delivered.
- **Text is zero-copy.** A text cell is a span into the input, or into a workbook's shared strings. Only a cell with `""` inside, or a typed workbook cell asked for as text, is written to an arena.
- **Streaming where the format allows it.** Delimited text is read through a buffer the reader refills, so a file of any size reads in memory bounded by that buffer and the longest row. A workbook is a zip whose directory is at its end, so the package is held in memory; each sheet is inflated through a sliding window as it is read, never whole, so a deflate bomb is only as large as the window.

## The core, and what holds it

The crate is two layers. **The core** (`rust/src/kernel`) is what the native library `libhypertabular` is made of and what every binding calls: it reads bytes the caller hands it and writes into buffers the caller hands it. **The Rust API** over it, behind the default `std` feature, is one binding among eight. The core's three properties are things a build refuses to get wrong, and `rust/check-core.sh` checks every one on the code and on the library it becomes, on every pull request:

| Property | What makes it impossible to break quietly |
| --- | --- |
| No standard library | The library is the core built with `std` off, and is checked again for a target that has no standard library at all (`thumbv7em-none-eabi`). |
| No allocation | The crate never declares `alloc`, so `Vec`, `Box` and `String` cannot be named. The shared library's imports are held to an allow-list (`memcpy`, `memset`, `memcmp`/`bcmp`, `memmove`, `abort`, and `getauxval` on arm64 Linux to ask which CPU it is), and a counting allocator watches whole inputs go through the core in the test suite: the count is zero. |
| No panic | Every export is declared through one macro that puts it under dtolnay's `no-panic`: the link fails, naming the export, if any panic path survives optimization. The proof is shown a canary export that can panic and has to reject it. That is why the core inflates with its own code — neither `miniz_oxide` nor `zlib-rs` passes the proof. |
| Nothing else linked in | The dependency closure is HyperCast and nothing else, taken with its own exports off, so `libhypertabular` carries none of HyperCast's symbols and links beside `libhypercast` in one program. |

The design record — why one repository, how the scanner and the workbook reader work, what was studied and borrowed — is in [`docs/`](docs/).

## Receipts

- **The corpus is the contract.** `corpus/delimited.json` (33 cases) and `corpus/workbook.json` (198 cases) replay through the Rust suite and through every binding's own, byte for byte, fault spans included. The workbook cases read generated files and files written by real applications: Excel for Windows (data, strict OOXML, the 1904 date system, an encrypted and a protected workbook, a macro-enabled one, ZIP64), Excel for the web, LibreOffice and Google Sheets, in both formats. [`corpus/README.md`](corpus/README.md) records each writer and what it does that the others do not.
- **Delimited text:** every cell located and handed out as text at 1.05 GiB/s, against 978 MiB/s for the `csv` crate splitting the same records; ten typed columns cast into the caller's buffers at 654 MiB/s; the structural scan alone at 2.18 GiB/s on AVX2. Linux x64, a 26 MB 200 000 × 10 file ([`docs/delimited.md`](docs/delimited.md#numbers)).
- **Workbooks, through every binding.** Two real-application files, read whole by each binding's own benchmark harness through one eight-column plan, every verdict looked at and every text cell's bytes, each harness printing the same checksum. Excel's `excel-win-300k.xlsx` (111 MB of sheet XML in a 16.7 MB package) and LibreOffice's `libreoffice-300k.ods` (360 MB of `content.xml` in 13.3 MB); linux-x64, an Intel Core i9-11900H:

  | Binding (harness) | xlsx open | xlsx read | ods open | ods read |
  | --- | ---: | ---: | ---: | ---: |
  | Rust (Criterion) | 34 ms | 601 ms | 337 ms | 1.20 s |
  | C# (BenchmarkDotNet) | 32 ms | 591 ms | 334 ms | 1.21 s |
  | Java (JMH) | 41 ms | 600 ms | 346 ms | 1.23 s |
  | Go (`go test -bench`) | 30 ms | 590 ms | 328 ms | 1.20 s |
  | Swift (package-benchmark) | 37 ms | 617 ms | 336 ms | 1.20 s |
  | Python (pyperf) | 39 ms | 630 ms | 344 ms | 1.24 s |
  | Ruby (benchmark-ips), Magnus | 30 ms | 1.01 s | 330 ms | 1.61 s |
  | Ruby (benchmark-ips), Fiddle | 36 ms | 1.04 s | 343 ms | 1.65 s |
  | PHP (phpbench) | 44 ms | 776 ms | 346 ms | 1.34 s |

  2026-10-07, every binding on the 0.7.0 core. [`docs/workbook.md`](docs/workbook.md#numbers) has where the time goes, why Ruby's and PHP's rows decode every value, and how to run each harness.
- **AOT on both managed platforms.** The C# AOT smoke test publishes under `PublishAot` with the core linked in from the package's static archive, and runs, on every CI leg. The Java smoke test (`java/aot-smoke-test`) builds a GraalVM Native Image on the jar's own reachability metadata and reads through every door; it is run by hand, not in CI.

## Platform support

`.github/workflows/ci.yml` builds the core fresh on five real-hardware legs and runs every binding's own suite, corpus replay included, against that leg's library; a second job does the same for the two musl RIDs inside Alpine containers, and Intel macOS is cross-built and tested at the core under Rosetta 2. The pipeline is the shared [SkunkWerkx forge](https://github.com/SkunkWerkx/.github), the same one HyperCast and HyperUuid ship from.

| Language | linux-x64 | linux-arm64 | linux-musl-x64 | linux-musl-arm64 | osx-x64 | osx-arm64 | win-x64 | win-arm64 |
| --- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| [Rust](rust/) (core) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| [C#](csharp/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [Java](java/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [Go](go/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [Swift](swift/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [Python](python/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [Ruby](ruby/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ |
| [PHP](php/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | — |

- **osx-x64 (Intel macOS): built, and tested at the core only.** The library is cross-compiled on the Apple silicon runner and the core's own suite runs on it under Rosetta 2; no binding's suite runs on Intel macOS.
- **PHP on win-arm64.** PHP ships no native Windows ARM64 build, so it runs as an x64 process there and loads the win-x64 library.

Swift and Go link the core into the consumer's executable on every platform, so there is nothing to deploy beside it; C# does for a Native AOT publish. Everything else loads the shared library.

**iOS and Mac Catalyst** load no libraries at all, so C#, Swift and Go link the core into the app there, from static libraries cross-compiled in the same job as the rest: C# for `ios-arm64`, `iossimulator-arm64`, `maccatalyst-arm64` and `maccatalyst-x64`, where a .NET iOS, MAUI or Mac Catalyst app (`net11.0-ios`, `net11.0-maccatalyst`) needs nothing but the package reference; Swift for the three arm64 ones, as an XCFramework, from iOS 16 and Mac Catalyst 16; and Go for all four, chosen by build tag, since Go builds every one of them as `GOOS=ios`. HyperCast's archives link beside them in the same app, and the two share no symbols. CI's `test-apple-mobile` job builds them on a Mac from that run's archives: C# and Swift run in an iOS simulator and as a Mac Catalyst process, Go's suite runs in the simulator, and each links an iOS device build.

**Runtime floors** follow upstream support and match HyperCast's: .NET 11, JDK 25, Go 1.26, Swift 6.2, Python 3.11, Ruby 3.3 and PHP 8.2. The crate's floor is Rust 1.88, HyperCast's own.

## WebAssembly

The binding is compiled to WebAssembly and the core is linked into that build by the
ecosystem's own toolchain — the same archive as everywhere else, built for `wasm32`. The
core imports nothing (no clock, no entropy, no allocator: the caller's buffers are all the
memory it touches), so a page loads one module and needs no shim beyond what the language
already brings. Every browser row below runs in headless Chrome in CI: on every pull request, except Ruby's, whose ruby.wasm interpreter build per Ruby minor runs in the weekly and release builds instead. Java goes the other way: the core runs as wasm inside the JVM.

| Binding | WebAssembly |
| --- | --- |
| Rust | **Yes.** The full suite passes under [wasmtime](https://wasmtime.dev/) on `wasm32-wasip1`, and `rust/browser-test` runs the crate on `wasm32-unknown-unknown` in Chrome. `cargo wasm-staticlib` builds the archive the bindings below link. |
| C# | **Yes, on .NET 11.** `dotnet add package HyperTabular` into a Blazor WebAssembly project links the core beside HyperCast's through the package's `.targets`; `csharp/HyperTabular.WasmSmokeTest` proves it. |
| Go | **Yes, through [TinyGo](https://tinygo.org) 0.42+.** `tinygo build -target=wasm` (or `wasip1`) links `staticlib/wasm`; stock Go's wasm toolchain cannot link a C library. |
| Swift | **Yes, on Swift 6.2+.** swift.org's WebAssembly SDK links the artifact bundle's `wasm32-unknown-wasip1` archive; the result is a `wasm32-wasip1` module, run in the browser through a WASI shim. |
| Python | **Yes, on [Pyodide](https://pyodide.org/) 314.x.** `await micropip.install("hypertabular")` installs the ninth wheel (`pyemscripten_2026_0_wasm32`) and `hypercast`'s; CI runs the whole pytest suite in Pyodide under Node and in Chrome. |
| Ruby | **Yes, through `rbwasm build`.** List `hypertabular-wasm` instead of `hypertabular` in the Gemfile handed to ruby.wasm 2.10+ (Ruby 3.4 or 4.0): it carries the Magnus extension prebuilt for `wasm32-wasip1` and shares the interpreter with `hypercast-wasm`. |
| Java | **In process, through [GraalWasm](https://www.graalvm.org/webassembly/).** The jar bundles the core as a `wasm32-wasip1` module; `-Dhypertabular.backend=wasm` selects it, and it is the automatic fallback where the jar has no native build. On GraalVM's JIT it reads the 300 000-row workbook in 1.9 s against 0.72 s native; on a stock JDK it runs interpreted. CI runs the whole suite through it on every leg. Compiling the binding itself to wasm is blocked: no Java-to-wasm compiler supports the Foreign Function & Memory API. |
| PHP | Not built. There is no maintained wasm engine PHP can embed, and HyperCast's `ext-php-rs` spike for WordPress Playground is not shipped. |

Arguments are kept to twelve integers or fewer on every export, because Mono's interpreter
— which runs .NET in the browser — passes no more to a native function; the work buffers
travel in one `hypertabular_buffers` struct instead.

## Provenance

Every native library carries a GitHub build-provenance attestation from the forge's build, checked again before it is committed for Go, Swift and PHP (`stage-native-binaries.yml`) and before it is packed into a gem (`release.yml`). Packages signed inside this repository's own `release.yml` — the gem and the published NuGet package — verify with `gh attestation verify <file> --repo SkunkWerkx/HyperTabular`; everything signed by the forge's reusable workflows — the crate, the jar, the wheels, the pre-push NuGet package and every native library — needs `--signer-repo SkunkWerkx/.github` as well, or `--owner SkunkWerkx` in place of both.

## Layout

```
corpus/     the shared conformance cases and the workbooks they read — the cross-language contract
rust/       the core (no_std, allocation-free, panic-free; 14 exports, hypertabular_version among them) and the Rust API over it
csharp/     the .NET 11 binding: LibraryImport, HyperCast's Verdict<T>, corpus replay, AOT smoke test
java/       the JDK 25+ binding: FFM, HyperCast's sealed Verdict, corpus replay, Native Image smoke test
go/         the Go binding: the core linked in through cgo, typed slices and Get[T]
swift/      the SwiftPM binding: the core linked in as a static library on every platform
python/     the 3.11+ binding: a PyO3 extension, columns as memoryviews, abi3 wheels
ruby/       the 3.3+ binding: a Magnus extension with a Fiddle fallback, pattern-matched Data verdicts
php/        the 8.2+ binding: ext-ffi, Success|Fault per cell
docs/       the design record: the core, delimited text, workbooks, prior art
```

## Contributing

Pull requests and issues are welcome. `.github/workflows/ci.yml` builds and tests every binding on every platform — a PR should stay green there before merging. Locally, `rust/check-core.sh` is the core's proof and `cargo test` its suite; each binding's README says how to run its own.

## License

[MIT](LICENSE)
