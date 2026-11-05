# hypertabular (R)

[![CI](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml/badge.svg)](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml)
[![CRAN](https://img.shields.io/cran/v/hypertabular.svg)](https://cran.r-project.org/package=hypertabular)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)

Delimited text and XLSX, ODS and Numbers workbooks for R, read forward a batch at a time
through a declared plan, with every cell judged by a HyperCast door. A hand-written C shim
calls the Rust core through `.Call`; the core is the same one every other HyperTabular
binding uses, built here from source as a static library and linked into the package. Every
cell comes back as a HyperCast verdict (the value, or a closed reason with the span of the
cell's text that caused it), and a structurally broken file comes back as a failure value
after the rows before it. No read signals an R error for anything in its data.

## Install

From CRAN:

```r
install.packages("hypertabular")
```

It imports [hypercast](https://cran.r-project.org/package=hypercast) (1.1.0 or newer), whose
verdicts it returns. CRAN ships the source package and compiles it per platform, so its
binaries for Windows and macOS need no Rust; a source install (Linux, or `type = "source"`)
does. R 4.2 or newer, on every platform.

From a source tarball, which needs Rust (`cargo` and `rustc` 1.89 or newer, from
[rustup](https://rustup.rs)):

```sh
R CMD INSTALL hypertabular_<version>.tar.gz
```

The tarball is what `R CMD build r` makes from a checkout after `tools/vendor.sh` (see
[Local loop](#local-loop)). It carries the `hypertabular` core crate and the `hypercast`
crate it casts with in `src/rust/vendor.tar.xz` and builds offline. `configure` prints the
`cargo` and `rustc` versions it found, and stops with a pointer to rustup when either is
missing or `rustc` is older than 1.89.

## Usage

```r
library(hypertabular)

csv <- charToRaw("id,when,amount\n1,7/1/2026,\"1,234.50\"\n2,13/1/2026,12x\n3,7/3/2026,\n")
t <- read_delimited(csv, list(id = col_i32("id"),
                              when = col_date("when", "month_day_year"),
                              amount = col_decimal("amount", hypercast::num_format())))
t
#> <hypertabular_table> 3 rows, 3 columns
#>   id (col_i32): 3 ok, 0 faulted
#>   when (col_date): 2 ok, 1 faulted
#>   amount (col_decimal): 1 ok, 2 faulted
t$columns$when
#> <hypercast_verdict: col_date> 2 ok, 1 faulted, 0 missing
#>        value    ok       reason offset length       raw
#> 1 2026-07-01  TRUE         <NA>     NA     NA      <NA>
#> 2       <NA> FALSE out_of_range      0      2 13/1/2026
#> 3 2026-07-03  TRUE         <NA>     NA     NA      <NA>
hypercast::faults(t$columns$amount)
#>   row    reason offset length
#> 1   2 malformed      2      1
#> 2   3     empty      0      0
as.data.frame(t)
#>   id       when amount
#> 1  1 2026-07-01 1234.5
#> 2  2       <NA>   <NA>
#> 3  3 2026-07-03   <NA>

reader <- delimited_reader(charToRaw("a,b\n1,x\n2\n3,z\n"), batch_rows = 1)
header(reader)
#> [1] "a" "b"
bind(reader, list(a = col_i32("a"), b = col_text("b")))
while (!is.null(batch <- read_batch(reader))) print(batch$columns$a$value)
#> [1] 1
failure(reader)
#> <hypertabular_failure: column_count> Record 2 (line 3, byte 8) has 1 cells; the first record had 2.

path <- system.file("extdata", "basic.xlsx", package = "hypertabular")
book <- workbook(path)
book
#> <hypertabular_workbook: xlsx, y1900> 1 sheet
#>   name hidden
#> 1 Data  FALSE
head(read_sheet(book, "Data", list(id = col_i32("id"), day = col_date("day")))$columns$day, 3)
#> <hypercast_verdict: col_date> 3 ok, 0 faulted, 0 missing
#>        value   ok reason offset length  raw
#> 1 2023-03-15 TRUE   <NA>     NA     NA <NA>
#> 2 1900-02-28 TRUE   <NA>     NA     NA <NA>
#> 3 9999-12-31 TRUE   <NA>     NA     NA <NA>
workbook(charToRaw("not a workbook"))$failure
#> <hypertabular_failure: not_a_zip> The workbook is not a zip file.
```

## Reading

Two readers share one surface. `delimited_reader()` takes a raw vector, a file path or a
connection (opened in binary if it is not open; one the reader opens it also closes when the
read ends) and reads it through `buffer_bytes` at a time, growing for a longer record up to a
1 GiB ceiling. `workbook()` reads a whole XLSX, ODS or Numbers file (a path or a raw vector),
tells which it is from its bytes, lists its sheets with `name` and `hidden`, and reports the
`date_system` its serials count in; `sheet_reader()` reads one sheet of it, by name or by
position from 1, and several can read at once.

With a header declared, a reader reads it as it opens: `header()` returns it, and the plan,
given at open or later through `bind()` (once), may name each source column by its header
name (the first of equal names) or by its position from 1. A plan column reading past the
width the header gives every record is the caller's error. `read_batch()` returns the next
`hypertabular_batch` of up to `batch_rows` rows, and `NULL` once there are no more.
`read_delimited()` and `read_sheet()` read the whole input into one `hypertabular_table`,
which also carries the `header` and the `failure`; with no plan, every header column is read
as text.

Nothing about the input is sniffed. `delimited_dialect()` declares the separator, quoting
(RFC 4180), whether there is a header and whether a blank line is skipped; `sheet_reader()`
takes whether there is a header and whether empty rows are skipped. A plan is a list of
columns, one factory per door: `col_bool()`, `col_i8()` through `col_u64()`, `col_f32()`,
`col_f64()` and `col_decimal()` (each taking a `hypercast::num_format()`, the invariant
format when `NULL`), `col_uuid()`, `col_timestamp()`, `col_unix(precision)`,
`col_excel_serial(epoch)`, `col_date(order)` (`NULL` for ISO 8601 only),
`col_datetime(order)`, `col_time()`, `col_duration()` and `col_text()`. A workbook's typed
cell is cast directly by its door (a stored `42` never passes through text to become an
integer); a text cell goes through the door as delimited text would.

## The verdict

A batch's `columns` is a named list with one `hypercast_verdict` per plan column: the data
frame HyperCast's own R doors return, with one row per row read, read by
`hypercast::is_ok()`, `hypercast::faults()` and `hypercast::optional()` as theirs are. Its
`door` attribute names the factory (`"col_i32"`). Values come back in the R types hypercast
gives them (a 32-bit integer as a double, a 64-bit one and a decimal as their exact text,
an instant as `POSIXct`, cut to the millisecond so it cannot round into the next second, with its exact `seconds` and `nanos` beside it, and so on; see
hypercast's README).

Each verdict here carries one column more, `raw`: the text of each faulted cell (`NA` where
the cell cast), the text its `offset` and `length` index. For delimited text that is the cell
as the file holds it, quotes resolved; for a workbook, a text cell's text, or a typed cell
said as text. The span counts in the units `substr()` counts `raw` in, so
`substr(raw, offset + 1, offset + length)` is the offending text: characters for UTF-8, and
bytes for a cell that is not valid UTF-8, whose `raw` is marked `"bytes"`. A cell holding a
NUL byte, which no R string can, has `raw` `NA` and a span in bytes; through `col_text()` it
is `out_of_range` at the NUL. The core reports spans in UTF-8 bytes and the shim
re-expresses them.

A batch's `line` is each row's 1-based line (delimited text) or row number in the sheet.
`as.data.frame()` gives the values alone, `NA` where a cell did not cast.

## Failures

A structurally broken file (a record with the wrong number of cells, an input ending inside
a quoted cell, a record past the 1 GiB ceiling, a workbook that is not a zip, is encrypted,
or has a part missing or cut short) is a `hypertabular_failure`, handed back as a value in
the idiom hypercast uses for a cell: a list with its `kind`, `record`, `line`, `byte` and,
for a column count, `expected` and `found`, and a `format()` that says it in a sentence. It
comes after every intact row before the break has been delivered and is final: `failure()`
holds it from then on, and every later `read_batch()` returns `NULL`. `read_delimited()` and
`read_sheet()` return the rows before it with it as their `failure`; `workbook()` returns a
workbook that could not be opened with its `failure` set and no sheets.

R errors are reserved for the caller's own declarations, what the C ABI calls a contract
violation: a dialect, plan, sheet or size that cannot be, a source column the header does
not have, a plan bound twice, a read before one is bound, or a reader asked of a workbook
that failed to open.

`hypertabular_native_version()` is the version of the loaded core, which equals the package
version.

## Local loop

From `r/`, with Rust, GNU tar (`brew install gnu-tar` on macOS) and the R packages
hypercast, testthat, jsonlite and devtools:

```sh
tools/vendor.sh            # derive the lock, vendor the crates, stage the header, the corpus and the example workbook, write inst/AUTHORS
tools/vendor.sh --verify   # the vendor tarball, the lock, the header, the corpus and the example are present and current
Rscript -e 'devtools::test()'
cd .. && R CMD build r
_R_CHECK_CRAN_INCOMING_=false R CMD check --as-cran hypertabular_<version>.tar.gz
```

`tools/vendor.sh --stage` does everything but `inst/AUTHORS`, the one step that needs
jsonlite. The wrapper crate in `src/rust/` depends on the `hypertabular` crate by exact
version, as any crates.io consumer does, so the graph CRAN builds is the published one;
`vendor.sh` resolves that version from the checkout's `rust/` (a command-line `[patch]`,
never a committed file, and it refuses a pin that differs from `rust/Cargo.toml`), vendors
the `hypercast` crate from crates.io, adds the core as `cargo package` writes it, and
rewrites the lock offline so it names the crates.io source. `HYPERCAST_PATH=<HyperCast's
rust/>` takes a `hypercast` crate not yet on crates.io from a checkout instead, the same way;
it is a local check only. The wrapper's `src/rust/Cargo.lock`, the vendor tarball, the C
header under `src/include/` (the checkout's `include/hypertabular.h`, which the shim
includes), the corpus copy under `tests/testthat/corpus/` (the two corpus files and every workbook
`workbook.json` reads) and `inst/extdata/basic.xlsx`, which the examples read, are generated
and gitignored; the tarball `R CMD build` makes carries them all. Rerun `tools/vendor.sh`
after any change in `rust/`, `include/` or `corpus/`. A missing corpus file is a test failure, never a
skip; the replay skips only when jsonlite (a Suggests) is absent, as on CRAN's no-Suggests
check.

The suite replays both corpus files: every delimited case from a raw vector, from
connections read through buffers of 1, 5 and 64 bytes, and from a file, in batches of one
row, two and many, with the plan by position and by header name, and with every buffer the
core works in starting at one element; and every workbook case whole and seven rows at a
time on the same starved buffers, the sheet by position and by name. It holds every read to
signalling nothing over every truncation of every delimited corpus input and over truncated
and corrupted workbooks, and covers the API around them.

Lint against an installed copy: lintr's object-usage linter loads the package it lints, and
without one every exported name is a false positive.

```sh
R CMD INSTALL --library=/tmp/rlib .
R_LIBS=/tmp/rlib Rscript -e 'print(c(lintr::lint_package(), lintr::lint_dir("tools")))'
cargo fmt --check --manifest-path src/rust/Cargo.toml
clang-format --dry-run -Werror src/*.c src/*.h
```

`lint_package()` reads `.lintr` and covers `R/` and `tests/`; `lint_dir("tools")` covers
the directory outside the package. An empty list is a pass. The C shim is formatted by
`clang-format` with `.clang-format`; the staged header under `src/include/` is cbindgen's
and is not.

## Windows

Rtools ships no Rust, so a Windows source build needs rustup's `x86_64-pc-windows-gnu`
target (`rustup target add x86_64-pc-windows-gnu`), which `src/Makevars.win` picks through
`tools/rustarch.R`. On ARM64 it picks `aarch64-pc-windows-gnullvm`; R on Windows ARM64 is
experimental and that build is untested. CRAN's own Windows binaries are built by CRAN, so
installing them needs no Rust.

## License

MIT; see [LICENSE.md](LICENSE.md).
