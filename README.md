# HyperTabular

[![CI](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml/badge.svg)](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/hypertabular.svg)](https://crates.io/crates/hypertabular)
[![NuGet](https://img.shields.io/nuget/v/HyperTabular.svg)](https://www.nuget.org/packages/HyperTabular)
[![Maven Central](https://img.shields.io/maven-central/v/io.github.skunkwerkx/hypertabular.svg)](https://central.sonatype.com/artifact/io.github.skunkwerkx/hypertabular)
[![PyPI](https://img.shields.io/pypi/v/hypertabular.svg)](https://pypi.org/project/hypertabular/)
[![Go Reference](https://pkg.go.dev/badge/github.com/SkunkWerkx/HyperTabular/go.svg)](https://pkg.go.dev/github.com/SkunkWerkx/HyperTabular/go)
[![Swift Package](https://img.shields.io/github/v/tag/SkunkWerkx/HyperTabular?label=swift%20package&sort=semver)](https://github.com/SkunkWerkx/HyperTabular/tags)
[![Gem](https://img.shields.io/gem/v/hypertabular.svg)](https://rubygems.org/gems/hypertabular)
[![npm](https://img.shields.io/npm/v/@skunkwerkx/hypertabular.svg)](https://www.npmjs.com/package/@skunkwerkx/hypertabular)
[![CRAN](https://img.shields.io/cran/v/hypertabular.svg)](https://cran.r-project.org/package=hypertabular)
[![Packagist](https://img.shields.io/packagist/v/skunkwerkx/hypertabular.svg)](https://packagist.org/packages/skunkwerkx/hypertabular)
[![C/C++](https://img.shields.io/github/v/release/SkunkWerkx/HyperTabular?label=c%2Fc%2B%2B&sort=semver)](https://github.com/SkunkWerkx/HyperTabular/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)

**Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers — read a batch at a time into typed columns, with a [HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell. Written once in Rust as a core with no standard library, no allocation and no panic, called in-process from C#, Java, Go, Swift, Python, Ruby, PHP, TypeScript, R, C and C++, with a shared conformance corpus so every binding agrees byte for byte.**

Reading a spreadsheet or a CSV into typed values usually means one of two things: a library that guesses (the separator, the header, whether `1/7/2026` is January or July, whether `12.185` is a thousand or twelve) and throws on the first cell it cannot read, or a string per cell and a parser per cell in the host language. HyperTabular does neither. The dialect and the plan — which source column, through which HyperCast door, in which notation — are declared, and nothing is sniffed. A cell that does not cast is a verdict in its column, `Empty`, `Malformed` or `OutOfRange` with the span of the offending text, and the read goes on. And the work happens in one native call per batch: the binding allocates the column buffers once, the core fills them, and the boundary is crossed once per few thousand rows, not once per cell.

Both examples read `people.csv` (`id,name,score`, one row whose score is `oops`) and
`people.xlsx`, the same table on a sheet named `People`, as shown in full in each binding's
README.

```csharp
// C# (.NET 10+; the switch is C# 15) — open, read the header, bind the plan by name; the same batch for
// delimited text and for a sheet
using System.Diagnostics;
using HyperCast;
using HyperTabular;

using var reader = DelimitedReader.Open("people.csv", Dialect.Csv);
var header = reader.Header!;
Column[] plan = [Column.Int32(header.Ordinal("id")), Column.Text(header.Ordinal("name")),
    Column.Double(header.Ordinal("score"))];
reader.Bind(plan);

while (reader.Read() is { } batch)
{
    ReadOnlySpan<int> ids = batch.Values<int>(0); // a whole column, as the core wrote it
    Console.WriteLine($"ids: [{string.Join(", ", ids.ToArray())}]");
    foreach (var row in batch) // or a row at a time, allocating nothing
    {
        Console.WriteLine(row.Get<double>(2) switch // one cell, as HyperCast's union
        {
            Success<double> score => $"{row.GetString(1)}: {score.Value}",
            Fault fault => $"line {row.Line}: {fault.Reason}",
            null => throw new UnreachableException(), // default(Verdict<T>) only; no read returns it
        });
    }
}

using var book = Workbook.Open("people.xlsx"); // XLSX, ODS or Numbers, told by what is in it
var sheet = book.Sheet("People", SheetOptions.Default, plan);
while (sheet.Read() is { } batch)
{
    Console.WriteLine($"sheet rows: {batch.Rows}"); // the same batch
}
```

```rust
// Rust — the API over the core
use hypertabular::{Column, Decimal, DelimitedReader, Dialect, Fault, SheetOptions, Workbook};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = DelimitedReader::open_unbound("people.csv", Dialect::CSV)?;
    let header = reader.header().unwrap();
    let plan = [
        Column::i32(header.require("id")?),
        Column::f64(header.require("score")?),
    ];
    reader.bind(&plan)?;
    while let Some(batch) = reader.read()? {
        let ids: &[i32] = batch.i32(0);
        println!("ids: {ids:?}");
        for row in batch {
            let score: Result<f64, Fault> = row.get(1);
            println!("line {}: {score:?}", row.line());
        }
    }

    let book = Workbook::open("people.xlsx")?;
    let mut sheet = book.sheet_unbound("People", SheetOptions::default())?;
    let header = sheet.header().unwrap();
    sheet.bind(&[Column::decimal(header.require("score")?)])?;
    while let Some(batch) = sheet.read()? {
        let scores: &[Decimal] = batch.values(0);
        let verdicts = batch.verdicts(0); // check each verdict before using its value
        for (score, verdict) in scores.iter().zip(verdicts) {
            println!("{score} ok: {}", verdict.is_ok());
        }
    }
    Ok(())
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
| [PHP](php/) | `pie install skunkwerkx/hypercast:<version>`, then `pie install skunkwerkx/hypertabular:<version>` |
| [TypeScript](typescript/) | `npm install @skunkwerkx/hypertabular@<version>` |
| [R](r/) | `install.packages("hypertabular")` |
| [C, C++](c/) | `hypertabular-<version>-<rid>.tar.gz` (`.zip` on Windows) from the GitHub Release |

Every binding needs HyperCast 1.1.0 or later: the verdict, fault, number-format and declared-option types a batch hands out are HyperCast's own, from its package, not copies. Each package manager brings it along (npm as a peer dependency). PHP's extension requires `ext-hypercast`, which PIE installs first, and C++ needs HyperCast's archive for the same platform beside this one, for `hypercast.hpp`; C needs nothing else, since `hypertabular.h` carries the HyperCast layouts it uses.

## The shape

- **One batch type.** `read()` returns a batch — rows, the line each row came from, a whole column (a span, a slice, a `memoryview`, a `MemorySegment`, an array of verdicts, a verdict data frame), a verdict beside each value, one cell as HyperCast's union, and the raw text a cell was cast from — or nothing when the input is done. The same type for delimited text and for a sheet.
- **Header first, plan after.** A reader or a sheet opens without a plan, reads its header, and is told its plan once it knows where each column is: `Header.Ordinal("M49 Code")` in C#, `header.require("M49 Code")` in Rust and TypeScript, `col_i32("M49 Code")` in R, the same in every binding — exact and first match, a missing name an error that names it. A plan known by position can still be given up front.
- **Rows, when that is the shape you want.** A batch read across: `foreach (var row in batch)` and its equivalent in every binding but R, whose batch is columns of verdicts and turns into a data frame, hand out a row view with the batch's own accessors, and every reader and sheet iterates its rows across batches.
- **Streams and async.** A workbook opens from a path, bytes or a stream. Where the language has an async idiom — C# (`ReadAsync`, `OpenAsync`), Swift (`readAsync`), Python (asyncio), Rust (the `async` feature) — a delimited source is read asynchronously and cancellably, and a cancelled read loses nothing.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and whether blank lines are skipped; `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size; the plan states each column's door and, for numbers, its `NumFormat` (separators, grouping, accounting parens, currency — or HyperCast's structural `DETECT`, which refuses the ambiguous rather than guess). Dates in text are read under a declared field order.
- **HyperCast is the judge.** A text cell means exactly what HyperCast's door would say of the same text. A workbook's numeric cell is converted from the stored `f64` by HyperCast's typed doors: an exact decimal (the shortest one that names the double, so a stored `0.1` is one tenth), a date or date-time under the workbook's own date system (1900 or 1904), a time of day, a span — each time snapped to the fewest fractional-second digits that store as the same double, so Excel's `9999-12-31 23:59:59` reads on the second.
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is HyperCast's verdict in its column, never an exception, in every binding. A record of the wrong width, input that ends inside a quoted cell, a workbook whose container or parts cannot be read, an encrypted package: each is a structural error, delivered only after every intact row before it — an exception where the language raises one (an `error` in Go), a value in TypeScript (an `Outcome`), R (a `hypertabular_failure`), C (a return code) and C++ (a `hypertabular::Result`).
- **Text is zero-copy.** In the core, a text cell is a span into the input, or into a workbook's shared strings. Only a cell with `""` inside, or a typed workbook cell asked for as text, is written to an arena. Rust, C#, Java, Go, Swift and C++ lend those spans out as views until the next read; Python, Ruby, PHP, TypeScript and R copy a batch out, so it outlives the read ([why](docs/design.md#the-bindings-side-by-side)).
- **Streaming where the format allows it.** Delimited text is read through a buffer the reader refills, so a file of any size reads in memory bounded by that buffer and the longest row. A workbook is a zip whose directory is at its end, so the package is held in memory; each sheet is inflated through a sliding window as it is read, never whole, so a deflate bomb is only as large as the window.

## The core, and what holds it

The crate is two layers. **The core** (`rust/src/kernel`) is what the native library `libhypertabular` is made of and what every binding calls: it reads bytes the caller hands it and writes into buffers the caller hands it. **The Rust API** over it, behind the default `std` feature, is one binding among eleven. The core's three properties are things a build refuses to get wrong, and `rust/check-core.sh` checks every one on the code and on the library it becomes, on every pull request that touches the core:

| Property | What makes it impossible to break quietly |
| --- | --- |
| No standard library | The library is the core built with `std` off, and is checked again for a target that has no standard library at all (`thumbv7em-none-eabi`). |
| No allocation | The crate never declares `alloc`, so `Vec`, `Box` and `String` cannot be named. The shared library's imports are held to an allow-list (`memcpy`, `memset`, `memcmp`/`bcmp`, `memmove`, `abort`, and `getauxval` on arm64 Linux to ask which CPU it is), and a counting allocator watches whole inputs go through the core in the test suite: the count is zero. |
| No panic | Every export is declared through one macro that puts it under dtolnay's `no-panic`: the link fails, naming the export, if any panic path survives optimization. The proof is shown a canary export that can panic and has to reject it. That is why the core inflates with its own code — neither `miniz_oxide` nor `zlib-rs` passes the proof. |
| Nothing else linked in | The dependency closure is HyperCast and nothing else, taken with its own exports off, so `libhypertabular` carries none of HyperCast's symbols and links beside `libhypercast` in one program. |

The design record — why one repository, how the scanner and the workbook reader work, what was studied and borrowed — is in [`docs/`](docs/).

## Receipts

- **The corpus is the contract.** `corpus/delimited.json` (33 cases) and `corpus/workbook.json` (246 cases) replay through the Rust suite and through every binding's own, byte for byte, fault spans included: C#'s P/Invoke, Java's JNI on JDK 21 and FFM on 25, Go's translated core, Swift, Ruby's Magnus *and* Fiddle backends, PHP's extension, Python's, TypeScript's addon and its WebAssembly fallback, R's `.Call` shim, and the C++ header over the C ABI. The workbook cases read generated files and files written by real applications: Excel for Windows (data, strict OOXML, the 1904 date system, an encrypted and a protected workbook, a macro-enabled one, ZIP64), Excel for the web, LibreOffice and Google Sheets, in both formats, and Apple Numbers in its own format and in its Excel export. [`corpus/README.md`](corpus/README.md) records each writer and what it does that the others do not.
- **Delimited text:** every cell located and handed out as text at 1.05 GiB/s, against 978 MiB/s for the `csv` crate splitting the same records; ten typed columns cast into the caller's buffers at 654 MiB/s; the structural scan alone at 2.18 GiB/s on AVX2. Linux x64, a 26 MB 200 000 × 10 file ([`docs/delimited.md`](docs/delimited.md#numbers)).
- **Workbooks, through every binding.** Three real-application files, read whole by each binding's own benchmark harness through one eight-column plan, every verdict looked at and every text cell's bytes, each harness printing the same checksum. Excel's `excel-win-300k.xlsx` (111 MB of sheet XML in a 16.7 MB package), LibreOffice's `libreoffice-300k.ods` (360 MB of `content.xml` in 13.3 MB) and Numbers' `numbers-300k.numbers` (29.4 MB of Snappy-compressed IWA parts in a 29.6 MB package); linux-x64, an Intel Core i9-11900H:

  | Binding (harness) | xlsx open | xlsx read | ods open | ods read | numbers open | numbers read |
  | --- | ---: | ---: | ---: | ---: | ---: | ---: |
  | Rust (Criterion) | 32 ms | 565 ms | 322 ms | 1.19 s | 820 ms | 1.16 s |
  | C# (BenchmarkDotNet) † | 31 ms | 586 ms | 332 ms | 1.23 s | 873 ms | 1.22 s |
  | Java (JMH), FFM on JDK 25 | 39 ms | 606 ms | 343 ms | 1.25 s | 884 ms | 1.25 s |
  | Go (`go test -bench`), the cgo binding ‡ | 30 ms | 593 ms | 329 ms | 1.20 s | 842 ms | 1.21 s |
  | Swift (package-benchmark) | 31 ms | 617 ms | 341 ms | 1.24 s | 942 ms | 1.32 s |
  | Python (pyperf) | 39 ms | 625 ms | 337 ms | 1.24 s | 845 ms | 1.22 s |
  | Ruby (benchmark-ips), Magnus | 29 ms | 1.00 s | 341 ms | 1.64 s | 827 ms | 1.62 s |
  | Ruby (benchmark-ips), Fiddle | 34 ms | 995 ms | 343 ms | 1.68 s | 885 ms | 1.69 s |
  | PHP (phpbench), 1.0's FFI package § | 41 ms | 775 ms | 345 ms | 1.42 s | 890 ms | 1.44 s |

  2026-10-24, every binding on one build of the core at `00a511d`. numbers-parser, the Python reader every other `.numbers` reader is held against, opens the same file in 66.8 s there and reads it in 68.1 s. Three rows predate what ships now, and stand as the receipts they are until their harness is run again on this machine. † Measured on .NET 11 RC 2 with BenchmarkDotNet's in-process toolchain, before the harness moved to running each benchmark in its own process. ‡ **Predates the translated core:** the cgo binding Go no longer has. Its replacement is slower, and measured so: on an Apple M6 on 2026-11-01, against that cgo binding in the same session, every read is a loss of 2–3.7x (the xlsx read 736 ms against 274 ms, the ods read 1.40 s against 627 ms, the numbers read 1.43 s against 722 ms, a delimited read 64.3 ms against 17.5 ms), with several times the memory; not this table's machine, so not comparable with its rows, and [`go/README.md`](go/README.md#benchmarks) has every one. § **Predates the extension:** it measured 1.0's Composer package over `ext-ffi`, not the PIE extension that replaced it. TypeScript, R and C++ have no benchmark harness yet. [`docs/workbook.md`](docs/workbook.md#figures) has where the time goes, why Ruby's and PHP's rows decode every value, and how to run each harness.
- **AOT on both managed platforms.** The C# AOT smoke test publishes under `PublishAot` with the core linked in from the package's static archive, and runs, on every desktop and musl CI leg, on .NET 10 and 11. The Java smoke test (`java/aot-smoke-test`) builds a GraalVM Native Image on the jar's own reachability metadata and reads through every door; it is run by hand, not in CI.

## Platform support

`.github/workflows/ci.yml`'s `core` job builds the Rust core fresh on five real-hardware legs and the two musl RIDs (in Alpine containers), and each language's `test-<binding>` job then runs that language's own suite, corpus replay included, on the same hosts against that leg's freshly built library (Go's against the committed translation of the same core; the musl suites in the language's own `*-alpine` image). A pull request runs the bindings it could have broken (its own tree, the core or the workflows changed), each after its lint; a dispatch, the weekly run and a release run them all. Intel macOS has no leg of its own; see the osx-x64 note under the table. The pipeline is the shared [SkunkWerkx forge](https://github.com/SkunkWerkx/.github), the same one HyperCast and HyperUuid ship from.

| Language | linux-x64 | linux-arm64 | linux-musl-x64 | linux-musl-arm64 | osx-x64 | osx-arm64 | win-x64 | win-arm64 | Status |
| --- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | --- |
| [Rust](rust/) (core) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | [crates.io](https://crates.io/crates/hypertabular) |
| [C#](csharp/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ | [NuGet](https://www.nuget.org/packages/HyperTabular) |
| [Java](java/) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | [Maven Central](https://central.sonatype.com/artifact/io.github.skunkwerkx/hypertabular) |
| [Go](go/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ | `go get` (git tag) |
| [Swift](swift/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ | `.package(url:)` (git tag; the core zips on the GitHub Release) |
| [Python](python/) | ✅ | ✅ | ✅ | ✅ | built | ✅ | ✅ | ✅ | [PyPI](https://pypi.org/project/hypertabular/) |
| [Ruby](ruby/) | ✅ | ✅ | ✅ | ✅ | Fiddle, built | ✅ | ✅ | ✅ | [RubyGems](https://rubygems.org/gems/hypertabular) |
| [PHP](php/) | ✅ | ✅ | ✅ | ✅ | — | ✅ | ✅ | — | `pie install` ([Packagist](https://packagist.org/packages/skunkwerkx/hypertabular); zips on the GitHub Release) |
| [TypeScript](typescript/) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | [npm](https://www.npmjs.com/package/@skunkwerkx/hypertabular) |
| [R](r/) | source | source | source | source | source | source | source | experimental | [CRAN](https://cran.r-project.org/package=hypertabular) |
| [C, C++](c/) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | GitHub Release (one archive per RID) |

The cells that are not a plain ✅, and why each is deliberate:

- **osx-x64 (Intel macOS): built, and tested at the core, in TypeScript, in Java and in C only.** There is no Intel macOS CI leg. The library is cross-compiled on the Apple silicon runner, attested, and shipped in every package that carries one, and the Rust core's own suite runs on it there under Rosetta 2, which is why that row keeps its ✅. TypeScript's and Java's are the two binding suites that run on Intel macOS, TypeScript's on an x64 Node under Rosetta 2 against an addon cross-built there, Java's on x64 Temurin 21 and 25 under Rosetta 2 (the JNI path and the FFM path), and the C archive's smoke consumers run there too; every other binding runs on Apple silicon against the same source. Ruby there installs the universal gem and runs on Fiddle, since no precompiled gem is built for it. These legs run in release mode only, not on a pull request.
- **PHP on osx-x64 and win-arm64: no zip.** Intel macOS: Homebrew bottles no PHP for it any more, and no leg compiles a PHP to build against. Windows on ARM: PHP has never shipped a native Windows ARM64 build, so it runs as an x64 process there regardless of host CPU and takes the win-x64 zip.
- **TypeScript on Bun.** The suite runs on Node on every leg and in both musl containers, and on Bun everywhere but win-arm64, where Bun is not yet verified, and osx-x64, whose cross-build leg runs Node alone.
- **C and C++: one archive per RID, plus Android, iOS, Mac Catalyst and WebAssembly.** Every core leg builds the C and C++ smoke consumers against that leg's library, shared and static, and replays the corpus through the C++ header; CI packs every archive from the run's own libraries with the release's packer. The iOS, iOS simulator and Mac Catalyst archives are static only and built against on a Mac: linked for a device, run in the simulator, and run as Mac Catalyst on both architectures. The android-arm64, android-x64 and wasi-wasm32 archives are packed but not run: no runner executes them, and their libraries are the ones C# and Swift already test there.
- **R: source.** R ships a source package: CRAN compiles it for each platform it serves and builds its own Windows and macOS binaries, and a source install needs Rust 1.89 or newer. CI runs `R CMD check --as-cran` on linux-x64, linux-arm64, osx-arm64 and win-x64, on the current R and, on linux-x64, the previous minor too; the musl and osx-x64 cells are source builds no job here checks. On win-arm64 the build picks `aarch64-pc-windows-gnullvm`; R on Windows ARM64 is experimental, and that build is untested.

The musl libraries depend on musl's libc and nothing else, which is what lets them load on a bare `alpine` image. The core's suite, both corpora included, also runs on big-endian s390x under QEMU.

**Runtime floors** follow upstream support: a version that has reached end of life is not a floor. Today that is .NET 10, JDK 21, Go 1.26, Python 3.11, Ruby 3.3, PHP 8.3 and Node 22.13 (`^22.13.0 || >=23.5.0`, the WebAssembly runtime's floor), the same floors as HyperCast. C# covers Windows, Linux, macOS and Blazor WebAssembly on .NET 10; iOS, Mac Catalyst and Android need .NET 11 (the union `switch` itself is C# 15, .NET 11's default and `LangVersion` preview on .NET 10). JDK 21 sits above JDK 17, which is still supported upstream (Temurin to 2027-10-31), because Temurin builds no JDK 17 for Windows arm64 or musl arm64, two of the jar's eight platforms; JDK 21 to 24 take the JNI path and 25 and later FFM. PHP's extension is built per minor, so a release run builds and tests 8.3, 8.4 and 8.5 on every platform. R follows CRAN's window instead: CI checks the current release and the previous minor, and the package declares R 4.2. Swift's floor is 6.2 for a different reason: it is the first release whose package manager can link a static library, which is how the binding reaches musl and WebAssembly at all. The crate's own floor is Rust 1.89 (`rust-version` in its manifest, the floor the Hyper* crates share), and CI checks the library on exactly that toolchain; the R package and the Python sdist build the core from source and need the same.

How each binding reaches the core (linked into the executable for Swift and C#'s Native AOT, compiled as Go source for Go, inside the extension for Python, Ruby, PHP, TypeScript and R), iOS, Mac Catalyst and Android, and how each one is published: [docs/platforms.md](docs/platforms.md).

## WebAssembly

Eight of these languages compile into a WebAssembly app with the core inside it: Rust on
`wasm32-unknown-unknown` and `wasm32-wasip1`, C# in Blazor WebAssembly (.NET 10+), Python under
Pyodide, Ruby through `rbwasm build`, Go under stock Go (`GOOS=js`, `GOOS=wasip1`) and TinyGo
alike, since its core is Go source, Swift with swift.org's WebAssembly SDK, TypeScript through
its fallback package's browser entry, and C from the wasi-wasm32 archive. TypeScript is also the
one binding that runs the core as wasm inside a native process: its fallback package, where no
native addon fits. Java's is blocked on its compilers, and PHP's and R's are not built. The
core imports nothing (no clock, no entropy, no allocator: the caller's buffers are all the
memory it touches), so a page loads one module. [docs/platforms.md](docs/platforms.md#webassembly)
has the table, binding by binding, and why each gap is where it is.

## Provenance

Every file a release ships verifies from the tag alone. One release attestation, signed by
this repository's own `release.yml` at the tag, names every registry package (the `.nupkg` as
packed and as nuget.org serves it, the gems, the wheels and the sdist, the crate, the Maven
files, the ten npm tarballs and the R source tarball), Swift's two zips, the fifteen C archives
and the 18 PHP extension zips on the GitHub Release, and its predicate is the release
inventory: every file's path, size, SHA-256, the commit it was built from and the repository
that signed its build provenance. One command, any file:

```sh
gh attestation verify <file> --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Which identity signs each artifact's build provenance, and the gates on the way in: [docs/provenance.md](docs/provenance.md).

## Layout

```
corpus/     the shared conformance cases and the workbooks they read — the cross-language contract
rust/       the core (no_std, allocation-free, panic-free; 14 exports, hypertabular_version among them) and the
            Rust API over it; the Python, Ruby and PHP extensions are its features
csharp/     the .NET 10 and 11 binding: LibraryImport, HyperCast's Verdict<T>, corpus replay, AOT smoke test, Blazor wasm
java/       the JDK 21+ binding: JNI on 21–24 and FFM on 25+ from one multi-release jar, HyperCast's sealed Verdict,
            corpus replay, Native Image smoke test
java/jni/   the JNI library, a no_std Rust cdylib linking the core in
go/         the Go binding: typed slices and Get[T]; the core as Go source, translated by wasm2go from its
            wasm32-wasip1 build; stock Go and TinyGo alike, browser and WASI included
swift/      the SwiftPM binding: the core linked in as a static library on every platform
python/     the 3.11+ binding: a PyO3 extension, columns as memoryviews, abi3 and abi3t wheels, the sdist, Pyodide
ruby/       the 3.3+ binding: a Magnus extension with a Fiddle fallback, pattern-matched Data verdicts
php/        the 8.3+ binding's suite, benchmarks and stubs: the extension itself is rust/'s php feature
            (ext-php-rs), installed by PIE, Success|Fault per cell
typescript/ the Node 22.13+ and Bun binding: a napi-rs addon with the core linked in, one npm platform package per
            RID, and the WebAssembly fallback package loaded where none fits
r/          the R 4.2+ binding: .Call into a C shim over the core's static library, built from vendored crates
include/    hypertabular.h, the C header, generated by cbindgen from the core's exports, and hypertabular.hpp, the
            hand-written C++17 header over it and HyperCast's
c/          the C archives' CMake and pkg-config files, and the C and C++ smoke consumers
docs/       the design record (the core, delimited text, workbooks, prior art) and platforms and provenance in depth
```

## Contributing

Pull requests and issues are welcome. `.github/workflows/ci.yml` builds the core on every host and runs each binding's suite when the pull request could have broken it (its own tree, the core or the workflows changed); `pr-board` is the one status to keep green before merging. Locally, `rust/check-core.sh` is the core's proof and `cargo test` its suite; each binding's README says how to run its own.

## License

[MIT](LICENSE)
