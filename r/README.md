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
#>        value    ok       reason offset length                  detail
#> 1 2026-07-01  TRUE         <NA>     NA     NA                    <NA>
#> 2       <NA> FALSE out_of_range      0      2 date_field_out_of_range
#> 3 2026-07-03  TRUE         <NA>     NA     NA                    <NA>
#>   detail_code       raw origin
#> 1          NA      <NA>   cell
#> 2          28 13/1/2026   cell
#> 3          NA      <NA>   cell
hypercast::faults(t$columns$amount)
#>   row    reason offset length               detail detail_code
#> 1   2 malformed      2      1 unexpected_character           2
#> 2   3     empty      0      0                blank           1
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
#>        value   ok reason offset length detail detail_code  raw origin
#> 1 2023-03-15 TRUE   <NA>     NA     NA   <NA>          NA <NA>   cell
#> 2 1900-02-28 TRUE   <NA>     NA     NA   <NA>          NA <NA>   cell
#> 3 9999-12-31 TRUE   <NA>     NA     NA   <NA>          NA <NA>   cell
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
name or by its position from 1. A name the header holds twice is refused: name that column by
position. A plan column reading past the width the header gives every record is the caller's
error. `read_batch()` returns the next `hypertabular_batch` of up to `batch_rows` rows, and
`NULL` once there are no more. `read_delimited()` and `read_sheet()` read the whole input into
one `hypertabular_table`, which also carries the `header` and the `failure`; with no plan,
every header column is read as text. `close()` lets go of a reader's or a workbook's native
memory now, and of a connection the reader opened; it is safe twice, and a sheet reader reads
on after its workbook is closed.

Nothing about the input is sniffed. `delimited_dialect()` declares the separator, quoting
(RFC 4180), whether there is a header, whether a blank line is skipped, the lines to `skip`
and the records to discard ahead of the header (`header_row`), what a short record is
(`fill`) and a long one (`long_rows`: `"reject"`, `"truncate"` or `"report"`), the `na`
cells, and the `encoding` (`"utf-8"`, `"windows-1252"`, `"latin-1"` or `"utf-16le"`); a
reader takes each of these too, in place of the dialect's. `sheet_reader()` takes whether
there is a header, whether empty rows are skipped, `skip`, `header_row`, `na` and a `range`
such as `"B3:D200"`.

**To keep every row of a sheet, set `skip_empty_rows = FALSE`.** By default a row whose only
cells are formulas with no cached value is skipped as empty, with nothing to say so; with it off,
those rows are delivered, their cells at origin `"uncached"`.

A plan is a list of columns, one factory per door: `col_bool()`, `col_i8()` through
`col_u64()`, `col_f32()`, `col_f64()` and `col_decimal()` (each taking a
`hypercast::num_format()`, the invariant format when `NULL`), `col_uuid()`,
`col_timestamp(options)`, `col_unix(precision)`, `col_excel_serial(epoch)`,
`col_date(order, options, months)` (`NULL` for ISO 8601 only), `col_datetime(order, options,
months)`, `col_time()`, `col_duration()` and `col_text(raw)`, each also taking its own `na`.
A format is the core's to judge: a plan holding one the core refuses (a policy this core
doesn't know, a stale layout) is an R error naming the column, before the source is opened.
Indian grouping is declared per column (`hypercast::num_format(grouping = "indian")`), beside
Western ones. A workbook's typed cell is cast directly by its door (a stored `42` never passes
through text to become an integer); a text cell goes through the door as delimited text would.

## The verdict

A batch's `columns` is a named list with one `hypercast_verdict` per plan column: the data
frame HyperCast's own R doors return, with one row per row read, read by
`hypercast::is_ok()`, `hypercast::faults()` and `hypercast::optional()` as theirs are. Its
`door` attribute names the factory (`"col_i32"`). Values come back in the R types hypercast
gives them (a 32-bit integer as a double, a 64-bit one and a decimal as their exact text,
an instant as `POSIXct`, cut to the millisecond so it cannot round into the next second, with
its exact `seconds` and `nanos` beside it, and so on; see hypercast's README). A fault's
`detail` names why, a code the installed hypercast has no name for reading as `"unknown"`
with its number in `detail_code`. Repeated output names are made unique as `make.unique(sep = "_")`
makes them (`a`, `a`, `a_1` reads as `a`, `a_2`, `a_1`); the batch's `original_names` keeps
them as given.

Each verdict here carries two columns more. `raw` is the text of each faulted cell (`NA`
where the cell cast), the text its `offset` and `length` index. For delimited text that is the
cell as the file holds it, quotes resolved; for a workbook, a text cell's text, or a typed
cell said as text. The span counts in the units `substr()` counts `raw` in, so
`substr(raw, offset + 1, offset + length)` is the offending text: characters for UTF-8, and
bytes for a cell that is not valid UTF-8, whose `raw` is marked `"bytes"`. The core reports
spans in UTF-8 bytes and the shim re-expresses them. `origin` says where a verdict came from:
`"cell"`, `"token"` (an `na` matched), `"padded"` (a cell a short record lacked under
`fill = TRUE`) or `"uncached"` (a workbook formula with no cached value).

Text R can't hold (a NUL byte inside it, or more than `INT_MAX` bytes) is `ok` with an `NA`
value, through `col_text()` as through `hypercast::cast_text()`: `ok` says the core accepted
the cell, and the `NA` that R can't represent it. So checking `ok` alone doesn't guarantee a
usable character value; `col_text(raw = TRUE)` gives each cell's exact bytes as a raw vector,
and `problems()` lists each such cell. A faulted cell holding a NUL has `raw` `NA` and a span
in bytes.

A batch's `line` is each row's 1-based line (delimited text) or row number in the sheet, and
`row` its place among the rows the read delivered. `as.data.frame()` gives the values alone,
`NA` where a cell did not cast; a cell that broke its column's constraints keeps its value
there, as cast.

## Constraints

A numeric or text column can declare hypercast's constraints, the OpenAPI 3.2 keywords
(`minimum`, `exclusive_minimum`, `maximum`, `exclusive_maximum`, `multiple_of` with `rounding`,
`min_length`, `max_length`, `enum`, with `trim` and `case_sensitive` for text), through the
column factory's `constraints`, a `hypercast::hypercast_constraints()` declaration for the
column's door. A declaration for `"i32"` on a `col_u8()` is an R error when the column is made,
and an R column being a list anyone can edit, the declaration is judged again, from its fields,
when the plan is bound. A door that takes no constraints has no argument for them. Each cell is
cast first, so text that isn't the type is a fault as ever, and then judged:

```r
plan <- list(
  qty = col_i32("qty", constraints = hypercast::hypercast_constraints(
    "i32", minimum = 1L, maximum = 10L)),
  sex = col_text("sex", constraints = hypercast::hypercast_constraints(
    "text", enum = c("Female", "Male")))
)
t <- read_delimited(charToRaw("qty,sex\n12,MALE\n7,x\n"), plan = plan)
t$columns$qty[, c("value", "adjusted", "ok", "reason", "keyword", "raw")]
#> <hypercast_checked_verdict> 1 ok, 0 faulted, 1 violated, 0 missing
#>   value adjusted    ok reason keyword  raw
#> 1    12       NA FALSE   <NA> maximum   12
#> 2     7    FALSE  TRUE   <NA>    <NA> <NA>
t$columns$sex$value
#> [1] "Male" "x"
problems(t)[, c("row", "column", "kind", "reason", "keyword", "raw")]
#>   row column kind reason keyword raw
#> 1   1    qty cell   <NA> maximum  12
#> 2   2    sex cell   <NA>    enum   x
```

A constrained column's verdict is hypercast's `hypercast_checked_verdict`, the data frame
`hypercast::cast_checked()` returns, read by `hypercast::violations()` as theirs is, with
`raw` and `origin` beside it as above. A violation isn't a fault: the cell cast, so its value
is kept exactly as cast (never rounded), `ok` is `FALSE`, `reason` is `NA` and `keyword` names
the keyword it broke, and its span is the judged text's. A cell `multiple_of` rounded is `ok`
with `adjusted` `TRUE`, and a text `enum` match reads as the entry as declared, with
`enum_index` naming it (`"MALE"` reads as `"Male"`). An unconstrained column keeps its plain
`hypercast_verdict`.

## Problems

`problems()` lists what went wrong in a batch or a table, computed when it is called, so a
read pays nothing for it: one row per cell `malformed` or `out_of_range`, per workbook
formula with no cached value, per record `long_rows = "report"` cut to the width (`kind`
`"row"`, with its `expected` and `found` widths), per cell `ok` with an `NA` value
(`"host_conversion"`), and per cell that broke its column's constraints (`reason` `NA`, its
`keyword` named). Blanks, `na` matches and `fill` padding are declared, not wrong, and
are left out.

```r
t <- read_delimited(charToRaw("a,b\n1,x\n2,3,4\n"), long_rows = "report",
                    plan = list(a = col_i32("a"), b = col_i32("b")))
problems(t)[, c("row", "line", "column", "kind", "reason", "raw", "expected", "found")]
#>   row line column kind    reason  raw expected found
#> 1   1    2      b cell malformed    x       NA    NA
#> 2   2    3   <NA>  row      <NA> <NA>        2     3
```

## Suggesting a plan

Declaring the locale, a number format and a date order, is the recommended route, and then
nothing needs inferring. A suggestion is a starting point for a plan to inspect.

`suggest_plan()` reads a sample of a source's rows as text and asks the core which door reads
each column. It is never applied: printed, it is the plan as code to paste, each column's
status beside it, and `$plan` is that plan. A column the sample leaves open (`01/02/2026`
under two date orders, `1,234` or `1.234` under two notations) is `"ambiguous"` and planned as
text until a `format` or `date_order` is declared, or another cell settles it. `$report` holds each column's door, `param`, status and
counts, the `candidates` an ambiguous column leaves and the `format` a resolved number is planned
in. `param` is the door's parameter, as every binding gives it: a date's order (1
`year_month_day`, 2 `month_day_year`, 3 `day_month_year`, the first candidate's where
ambiguous), a Unix precision or an Excel epoch, and otherwise 0. `truncated` says whether the
source has rows past the sample.

```r
s <- suggest_plan(charToRaw("id,when,amount\n1,2026-01-07,1.5\n2,2026-01-08,12\n"))
s
#> # 3 columns from a sample of 2 rows
#> list(
#>   id = col_i32("id"),         # resolved, 2 of 2
#>   when = col_date("when"),    # resolved, 2 of 2
#>   amount = col_f64("amount")  # resolved, 2 of 2
#> )
t <- read_delimited(charToRaw("id,when,amount\n1,2026-01-07,1.5\n"), s$plan)
```

## Failures

A structurally broken file (a record with the wrong number of cells, an input ending inside
a quoted cell, a record past the 1 GiB ceiling, a unit its declared encoding has no character
for, a workbook that is not a zip, is encrypted, or has a part missing or cut short) is a
`hypertabular_failure`, handed back as a value in the idiom hypercast uses for a cell: a list
with its `kind`, `record`, `line`, `byte` and, for a column count, `expected` and `found`, and
a `format()` that says it in a sentence. It comes after every intact row before the break has
been delivered and is final: `failure()` holds it from then on, and every later
`read_batch()` returns `NULL`. `read_delimited()` and `read_sheet()` return the rows before it
with it as their `failure`; `workbook()` returns a workbook that could not be opened with its
`failure` set and no sheets. A connection's own read error is an R error of class
`hypertabular_io_error`.

R errors are reserved for the caller's own declarations, what the C ABI calls a contract
violation: a dialect, plan, sheet or size that cannot be (the core says which sizes fit, and a
`batch_rows` or `buffer_bytes` past them is refused with what it needs), a format the core
refuses, a source column the header does not have, a plan bound twice, a read before one is
bound or after `close()`, or a reader asked of a workbook that failed to open or is closed.
A reader is called into once at a time: a call into the same reader from a handler, an
`on.exit`, a finalizer or a connection's own R code while it reads is an R error, and the
read goes on as it was. An interrupt mid-read loses nothing inside the read: either the reader
reads on, the batch being handed over handed again by the next `read_batch()`, numbered the
same, or it refuses every further read, saying the read was interrupted. A reader of a file, a
compressed file or a raw connection always reads on; one of another connection (a pipe, a
socket, a URL) whose own read the interrupt cut part-way refuses, because bytes may be missing.
`close()` always works. One gap no R function can close is the caller's own: an interrupt that
lands after `read_batch()`'s last interrupt check, in the caller's code before the batch is
assigned, loses that one batch (about one in 6,000 interrupts, under a storm of them). A lost
batch shows as a gap in `row`: a caller who reads on after catching interrupts can check that
each batch's first `row` follows the last one's.
A connection must be opened in binary mode (`"rb"`) or left closed: standard input reads as
`file("stdin")`, and text in memory as a raw vector (`charToRaw()`). A connection the caller
closes mid-read is an R error saying so.

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

Before a hypercast submission, `.github/scripts/r_revdep_check.sh <hypercast tarball>
<hypertabular tarball>`, from the repository root, installs that hypercast into a temporary
library and runs `R CMD check --as-cran --no-manual` on this package's tarball against it; it
fails on anything beyond the incoming NOTE of a new submission (`.github/RELEASING.md`, step 6).

The suite replays both corpus files: every delimited case from a raw vector, from
connections read through buffers of 1, 5 and 64 bytes, and from a file, in batches of one
row, two and many, with the plan by position and by header name, and with every buffer the
core works in starting at one element; and every workbook case whole and seven rows at a
time on the same starved buffers, the sheet by position and by name. It holds every read to
signalling nothing over every truncation of every delimited corpus input and over truncated
and corrupted workbooks, and replays each case's problems and suggestion. `test-values.R` holds
every door's table cell to the standalone hypercast cast of the same text, and the declarations
around them; `test-lifecycle.R` closes early and twice, calls into a reader from inside its own
read, reads under `gctorture`, loads both packages in either order in fresh processes, and,
in a process of its own, interrupts reads at each step a read moves on at and from outside at
random, holding every value and `row` to the file.

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
