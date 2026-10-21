# HyperTabular — notes for agents

A Rust core (`rust/`) that reads delimited text and XLSX/ODS workbooks into typed column
batches, judging each cell with HyperCast, behind a C ABI called from seven other
languages. `docs/design.md`, `docs/delimited.md` and `docs/workbook.md` have the why; all
CI and release logic lives in the Forge (`SkunkWerkx/.github`). This file holds what the
code alone won't tell you.

## The contract

- Nothing is sniffed: dialect, plan, number format and date order are declared by the
  caller. A bad cell is a verdict (`Empty` / `Malformed` / `OutOfRange` plus a span), not
  an error. A broken file is a structural error, raised only after every intact row before
  it has been delivered. See `docs/design.md`, "Errors".
- `corpus/delimited.json` and `corpus/workbook.json` are replayed by every binding.
  `workbook.json` was first written by an independent reader that has since been deleted,
  so a diff to it is a behaviour change to explain, not output to regenerate away
  (`corpus/README.md`).

## Two layers

- `rust/src/kernel` is the core: `no_std`, never names `alloc`, panic-free, and all memory
  is passed in by the caller. Everything else is "the Rust binding", one of eight, and
  exports nothing. `rust/check-core.sh` proves this on the built library.
- Every export goes through the `export!` macro in `kernel/exports.rs`. No export takes
  more than twelve integer arguments, because Mono's interpreter (.NET in the browser and
  iOS debug builds) passes no more; extra buffers travel in a struct.

## Buffers

A delimited fill that finds a buffer too small consumes nothing and is simply retried. A
workbook call cannot roll back a deflate stream: it stops in place and reports the size it
needs, and the caller must grow that buffer keeping its contents, then repeat the call.
Bindings prove this by replaying the corpus with every buffer starting at one element
(`docs/workbook.md`, "Growing a buffer").

## Bindings differ on purpose

C#, Java, Go and Swift hand out batch views valid until the next `read()`; Python, Ruby and
PHP copy. Ruby fault spans count characters, every other binding counts bytes
(`docs/design.md`, "The bindings, side by side").

## Upstream: HyperCast

HyperCast is the judge, taken with its `exports` feature off so the two libraries link
into one program. A HyperCast bump touches every manifest: `rust/Cargo.toml`, the csproj,
`build.gradle.kts`, `pyproject.toml`, the gemspec, `composer.json`, both `Package.swift`
files and `go.mod`.

## Lists that move together

- The export list appears in the README, `docs/design.md`, `docs/delimited.md`,
  `docs/workbook.md`, the binding READMEs, every binding's declarations and C#'s
  `EmccExportedFunction` list in `csharp/HyperTabular/build/net11.0/HyperTabular.targets`.
- Ruby ABIs: `ABIS` in `ruby/Rakefile`, both `ABIS=(…)` in `release.yml`,
  `ruby_compat_version` in `ci.yml`, and the Forge's `ruby_wasm_abis` default. Fiddle
  covers every Ruby and platform outside them and must never be restricted. Ruby 3.4
  reaches EOL on 2028-03-31: then drop it from those lists and clear
  `ruby_compat_version`.

## Never hand-edit

- `go/staticlib/`, the Swift artifact bundle and `php/src/native/` are CI-built binaries
  committed by `stage-native-binaries.yml`.
- Versions: bump through `prepare-release.yml`. The libraries embed their version.

## Claims are measured

Benchmark tables are dated receipts. Don't adjust them without re-running every harness.

## Toolchains and floors

Each language's primary toolchain is its current release; check endoflife.date rather than
assuming. Exceptions must be commented where pinned: a real platform constraint, or an
older floor shipped beside the primary and dropped at its EOL. Floors match HyperUuid and
HyperCast and move together.

## Forge coupling and releases

- The Forge is pinned to one exact commit in `ci.yml`, `release.yml` and `RELEASING.md`.
  Repin all of them, and HyperUuid and HyperCast with them, as one reviewed change.
- Registry versions can never be replaced. Release and recovery: `.github/RELEASING.md`
  and the Forge's `docs/release-runbook.md`.

## Local loop

`cd rust && ./check-core.sh && cargo test`. Bindings need `cargo cdylib` first (or
`cargo wasm-module` for Java's wasm tests); each binding README has its own steps. Large
benchmark inputs come from the ignored `corpus/generate/out/`.

## Working with the owner

Don't open pull requests or push unless asked.
