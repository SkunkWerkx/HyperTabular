## Submission

This is a new submission. It imports hypercast, the package of the same authors whose
verdicts every cell is returned as; hypercast is submitted first.

## Test environments

* macOS 27.2 on Apple M6 (aarch64-apple-darwin27.0.0), R 4.6.2 (Homebrew build), rustc and
  cargo 1.99.1.

## R CMD check results

`R CMD check --as-cran hypertabular_<version>.tar.gz`, with `_R_CHECK_CRAN_INCOMING_=false`
until hypercast is on CRAN.

macOS, R 4.6.2:

    Status: OK

The testthat expectations pass, including the replay of the two corpus files (and the 27
workbooks one of them reads) every binding of the project replays. `NEWS.md`'s heading
carries the version number, which R's NEWS parser requires.

## Compiled code

The package builds a Rust static library from the vendored crates in
`src/rust/vendor.tar.xz` (two crates, the `hypertabular` core and the `hypercast` crate it
casts cells with), offline, with `cargo build -j 2`, `CARGO_HOME` under `src/`, and removes
the Rust build directory after linking. `configure` prints the `cargo` and `rustc` versions
and stops with a pointer to https://rustup.rs when either is missing or `rustc` is older than
1.89.

The wrapper crate in `src/rust/` depends on the `hypertabular` crate from crates.io at
exactly this package's version, as any Rust consumer of it would, and on nothing else; the
core depends only on `hypercast`, by the same authors, so `inst/AUTHORS` names no third-party
crate. The vendored crates are those releases' sources as `cargo package` writes them.

The core is `#![no_std]`, allocation-free and panic-free, and leaves what those would need
to the binding and to R:

* No std. The crates are built with `default-features = false`: no I/O, threads,
  formatting or heap from Rust's standard library. They take nothing from the operating
  system; the shim reads files and connections through R.
* No allocation. No function in the core allocates; every buffer it works in is the
  shim's, allocated with `R_Calloc` and grown on the core's request, and freed by a
  finalizer.
* No panics. No input makes the core panic; every outcome is a return code over the C ABI,
  which the shim turns into verdict rows or a failure value. No read signals an error for
  its data; R errors are reserved for malformed arguments. The archive is built with
  `panic = "abort"` behind a panic handler nothing reaches.

## Windows

Rtools ships no Rust, so the Windows build uses rustup's `x86_64-pc-windows-gnu` target
(`aarch64-pc-windows-gnullvm` on ARM64), chosen by `tools/rustarch.R` in
`src/Makevars.win`. Not tested locally; to be confirmed on win-builder.
