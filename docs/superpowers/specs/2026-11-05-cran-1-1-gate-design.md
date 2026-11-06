# HyperCast and HyperTabular 1.1: the CRAN gate

Date: 2026-11-05. Status: design approved by the owner. One spec and one plan by the owner's request.
Approval covers the design. Real-core quote tests, the D2 measurements, the Numbers format
investigation and CRAN's feedback on D9 remain implementation and release gates.

## Outcome

Everything in `../HyperCast/docs/CRAN_CHECKLIST.md` is closed with evidence: the eight open
HyperTabular features, its reader contracts, the R surface and the CRAN gates, with binding parity.
HyperCast then goes to CRAN once, followed by HyperTabular, with no follow-up HyperCast update caused
by our own work. A reviewer can still send a package back, and this spec can't rule that out.

## Constraints

- HyperCast 1.1 and HyperTabular 1.1 are held together. Submission is the owner's, by hand, and
  waits on the owner's answer from CRAN about HyperUuid's feedback.
- HyperCast's surface freezes only after HyperTabular's features have shown it needs nothing more.
- Implementation starts after the Go `-unsafe` work in another thread finishes, because that thread
  is the only one editing code. This spec and its plan are the only files this work writes until then.
- The owner drives git. Nothing here commits, pushes, dispatches or moves a pin.
- The 1.0 exports and layouts stay as they are. The core stays `no_std`, allocation-free and
  panic-free, and every memory buffer stays the caller's.
- Parity rule: shared behaviour is implemented once in the core, the corpus pins it, and every binding
  exposes it in its idiom. A binding that can't follow says why in its README.
- Bindings: Rust, C#, Java, Go, Swift, Python, Ruby, PHP, TypeScript, R, C and C++.

## Decisions

| # | Decision | Example | Why |
|---|---|---|---|
| D1 | Only cell boundaries change: after a quoted cell closes, later quotes can't hide a separator or line end. Text extraction stays exactly as it is. The fix applies to the 1.0 exports too, with no legacy mode. | `"x"y"z,w"` becomes two cells, reading `x"y"z` and `w"`. Today it's one cell, reading `x"y"z,w`. `"x"y"z"` still reads `x"y"z`. | It's the checklist's rule, and a legacy mode would keep a known data-loss bug alive. No existing corpus vector changes. |
| D2 | Text that isn't valid UTF-8 is a `Malformed` verdict on that cell, on every read. A bad header is a structural failure. | `\xe9t\xe9` under the UTF-8 default is `Malformed` at offset 0, not raw bytes. | A bad cell is a verdict by contract. The validation cost is measured and printed. |
| D3 | A missing-value token matches the unquoted cell's exact bytes. A quoted cell never matches. | With `na = c("NA")`, `NA` is missing and `"NA"` is the text NA. | Quoting is how a file says "this is the value". No per-column switch unless someone asks. |
| D4 | `skip` counts physical lines and ignores quotes. `header_row` counts records after the skip, and records before it are discarded. | A title line holding a stray `"` is skipped safely. | Title lines are what `skip` is for, and they're often not valid records. |
| D5 | A named plan and each strict resolver refuse a duplicate name, with an error naming both positions. Lookups that return an optional keep their signature and their first-match behaviour, and their docs say so. Position-based plans still work. | `id,name,id` with `col_i32("id")` or Go's `Ordinal("id")` is refused. Go's `Find("id")` still returns `0, true`. | The checklist forbids a silent wrong choice in a plan. An optional return can't carry an error, and changing it would break source compatibility for no safety gain, since those callers asked for the first match. The refusal changes a 1.0 behaviour, in the CHANGELOG. |
| D6 | R keeps its `plan =` named list of `col_*()`, with no compact string like `"icd"`. | `plan = list(id = col_i32("id"))` | It's already the `col_types` shape, and a letter code would hide the declarations this package exists to make. |
| D7 | HyperCast's `cast_char()` gains a `code_point` column. U+0000 stays `ok` with `NA` as its `value`. | `U+0000` gives `ok = TRUE`, `value = NA`, `code_point = 0`. | The checklist requires exact output for every value, and today NUL has none. It's additive, and it has to land before the freeze. |
| D8 | Text R can't hold (an embedded NUL, or over `INT_MAX` bytes) is `ok` with `NA` as its `value` on both R surfaces. HyperCast's `cast_text()` moves from `out_of_range` to this, and gains `raw = TRUE` for a list of raw vectors, as does `col_text()`. HyperTabular's `problems()` adds a `host_conversion` row. | `cast_text(list(as.raw(c(0x61, 0, 0x62))))` and a table cell holding the same bytes both give `ok = TRUE`, `value = NA`. With `raw = TRUE`, the value is those bytes. | The checklist says a host failure mustn't read as the core rejecting a valid value, standalone and table conversion must agree, and exact output must exist. It matches D7, and `ok` still tells this apart from a missing input, whose `ok` is `NA`. |
| D9 | The Rust floor stays 1.89, so the two r-oldrel macOS flavors keep failing at `configure`, explained in `cran-comments.md`. | Those flavors have rustc 1.84.1. | Both crates are edition 2024, which needs Rust 1.85, so the floor can't reach 1.84 without an edition downgrade. Revisit only if CRAN's HyperUuid feedback says otherwise. |
| D10 | The suggester reads a path, bytes or a sheet, never a stream. | `suggest_plan(file)` | A forward-only stream would lose the sampled rows. |
| D11 | Suggestions use `f64` for fractional numbers rather than decimal, and the invariant notation unless the caller declares a format. | `2.5` suggests `f64`; `1,234` is ambiguous. | It's what most users expect, and the plan is printed for them to change. |

## Core design

### Input order

Every delimited read runs these steps in order, and nothing later reorders them:

1. Decode a declared encoding (the chunks the core sees are UTF-8 from here on).
2. Skip the UTF-8 or UTF-16LE byte order mark at byte 0.
3. Skip `skip` physical lines.
4. Tokenize into records and cells (quotes per D1).
5. Discard records before `header_row`, then read the header if declared.
6. Apply the ragged-row policy.
7. Unescape.
8. Match missing tokens (D3).
9. Cast through the door.

A sheet runs: select the range, skip rows inside it, read the header, filter empty rows, then steps
8 and 9 (tokens match text cells only).

### Quotes (row 1)

A quote opens a quoted cell only as the cell's first byte. Inside, `""` is a literal quote and a lone
`"` closes it. After closing, quotes are literal until a separator or line end. With no open cell,
a quote is an ordinary byte.

These rules decide boundaries only. `unescape.rs` keeps its rule for a cell that starts with `"`: drop
the opening quote, read `""` as `"`, drop a lone quote that's the cell's last byte, and keep any other
lone quote. So extraction doesn't track where the quoted part closed. Scanner and extraction agree because
extraction never moves a boundary. A cell that doesn't start with `"` is delivered raw, as today.

| Input | Today | With this rule |
|---|---|---|
| `"x"y"z"` | `x"y"z` | `x"y"z` |
| `"x"y"z,w"` | one cell, `x"y"z,w` | `x"y"z` and `w"` |
| `"x"y""z"` | failure: input ends inside a quote | `x"y"z` |
| `12" pipe,1` / `3,4` / `6" pipe` | one cell holding all three lines | three records, as written |
| `12" pipe,1` alone | failure: input ends inside a quote | two cells, as written |

A throwaway model of both rules, run over `corpus/delimited.json`, split all 11 quote-bearing vectors
identically, and it gave this table. The implementation has to confirm that against the real core. If
one does change, it's explained in `corpus/README.md`'s terms rather than regenerated. New vectors pin
each row above.

The block walker keeps its current fast path: a block with no quotes is classified by masks alone.
A block with quotes walks its events in order. Inside an open cell, prefix XOR from the opening quote
finds the close, so long quoted runs stay in bit operations.

Tests cover every engine (`Swar`, `Neon`, `NeonPmull`, `Sse2`, `Sse2Clmul`, `Avx2Clmul`), block and
buffer splits at every offset around a quote, odd stray quotes, escaped quotes, multiline records, and
a differential check against a plain byte-at-a-time reference state machine. No record may disappear
or merge. `docs/delimited.md`'s parity description and "Parked: parallel chunked scanning" change to
match, since parity is no longer a global carry. The benchmark gains a stray-quote file and an
all-quoted file beside the existing one.

### Extending the ABI

No 1.0 layout changes. New size-prefixed structs and new exports follow HyperCast's `RawDateOptions`
precedent: `size` first, sizes the core doesn't read are refused, and validation runs before any
write. Each struct's layout is pinned by `offset_of!` and carried by the header.

| New export | Replaces, for callers that want the options | Takes |
|---|---|---|
| `hypertabular_delimited_init_with` | `_delimited_init` | `RawDelimitedOptions` |
| `hypertabular_delimited_fill_with` | `_delimited_fill` | `ColumnSpecWith`, `ColumnBufferWith`, `BuffersWith` |
| `hypertabular_workbook_sheet_with` | `_workbook_sheet` | `RawSheetOptions` |
| `hypertabular_workbook_fill_with` | `_workbook_fill` | as for the delimited fill |
| `hypertabular_range_parse` | new | `A5:F200` text to four numbers, so bindings don't each parse it |
| `hypertabular_transcode` | new | row 6 |
| `hypertabular_faults` | new | row 11 |
| `hypertabular_suggest` | new | row 12 |

`_delimited_header` and `_workbook_header` need no new export: the options reach them through the state
block, and `_state_size` already reports its size, so a binding that hard-codes 64 is a bug to fix. No
export exceeds twelve integer arguments. Every "Lists that move together" site moves with these
and `check_exports.py` holds them.

**`RawDelimitedOptions`:** `size`, `separator`, `quoting`, `skip_blank_lines`, `engine`, `skip`
(`u32`), `header_row` (`u32`), `short_rows` (`0` reject, `1` pad), `long_rows` (`0` reject, `1` report,
`2` truncate).

**`RawSheetOptions`:** `size`, `has_header`, `skip_empty_rows`, `skip`, `header_row`, and the range as
`first_row`, `first_column`, `last_row`, `last_column`, each 1-based like the `A5:F200` it comes from,
with `0` meaning no limit on that side.

**`ColumnSpecWith`:** `size`, `ordinal`, `door`, `param`, `format` (`RawNumFormat`), `date`
(`RawDateOptions`), then pointer and length pairs for the month vocabulary and for the column's
tokens. Tokens use HyperCast's vocabulary encoding: a little-endian `u32` length, then the bytes. A
column's token list replaces the dialect's for that column. A null pointer inherits it; an empty
list means no tokens.

**`ColumnBufferWith`:** `values`, `verdicts` (the same 12-byte `CellVerdict`, so existing views don't
change), and `notes`, a nullable array of a new 4-byte `CellNote { detail: u16, origin: u16 }`.
`detail` is HyperCast's number and `origin` says where a missing or padded cell came from:

| `origin` | Meaning |
|---|---|
| `0` | The cell as written, blanks included |
| `1` | A declared missing token |
| `2` | Padding for a short row, or an ordinal past the row's end |
| `3` | An uncached formula |

**`BuffersWith`:** the 1.0 `Buffers` fields, then the dialect's tokens as a pointer and length, then
`row_widths`, a nullable array of each row's actual cell count.

A caller who passes null `notes` pays nothing extra for them. Filling them routes the doors through
`hypercast::detailed`, which HyperCast guarantees is the same parse with the same values, and the cost
is measured and printed. A typed workbook cell uses the typed twin of that door.

### Ragged rows (row 2)

- **Short rows:** rejected as today, or padded with `Empty` verdicts at `origin` `2`.
- **Long rows:** rejected as today; reported, meaning the extra cells are dropped and `hypertabular_faults`
  gives a row-level entry with the expected and found widths; or truncated silently because the caller
  said so.

The first record still fixes the width. Failure codes and fields don't change.

### Skip, header row and range (row 4)

As in Input order and D4. Sheet coordinates stay absolute: ordinals in a plan are relative to the range,
and `source_column` in a problem is the sheet's own column. A range past the sheet's end reads as what
exists. An inverted range is the caller's mistake, refused before any read.

### Missing tokens (row 5)

A match produces `Empty` at `origin` `1`, on every door including `Text`. An ordinary declared missing
value is not a parse problem, so `hypertabular_faults` leaves it out.

### Encoding (row 6)

`hypertabular_transcode(encoding, input, len, last, out, cap, progress)` is stateless, and the binding
runs it before the fill, on caller-owned buffers. A code unit cut off by a chunk's end stays
unconsumed until `last`. `encoding` is `1` Windows-1252, `2` Latin-1 or `3` UTF-16LE. Undefined
Windows-1252 bytes map to their C1 controls, as WHATWG does. A lone surrogate or a truncated final
unit is a new structural failure, `Failure::ENCODING = 27`, delivered after every intact row before
it, with `byte` being the original input's offset. Nothing is ever silently replaced.

With no encoding declared the text is UTF-8, validated per D2, without copying. Workbooks declare
their own encoding, so this option is delimited only. UTF-16BE and sniffing are out of scope.

Embedded NUL passes through the core untouched. A host that can't hold it says so (D8).

### Uncached formulas (row 9)

A new `Cell::Uncached` carries a formula with no cached value in XLSX (`<f>` with no `<v>`) and ODS
(`table:formula` with no value). It reads as `Empty` at `origin` `3` under `fill_with`, and as `Empty`
through the 1.0 exports, which is today's behaviour. Blank cells and cached errors don't change.

Whether a Numbers file can hold a formula with no result isn't known yet, so the work checks the IWA cell
storage. If it can, Numbers joins with a vector. If it can't, `docs/workbook.md` says so, and that's why
there's no Numbers vector.

### Problems (row 11)

`hypertabular_faults` is resumable and writes into a caller-owned array of a new `RawProblem`: row,
plan column (`u32::MAX` for a row-level entry), reason, detail, origin, span offset and length,
expected and found widths. It lists `Malformed`, `OutOfRange`, uncached formulas and reported long
rows. It leaves out blanks, declared missing tokens and declared padding. Each binding offers a
batch-level faults view beside the per-cell verdicts, and adds line and source column from what the
batch already holds.

### Plan suggester (row 12)

The binding reads up to `sample_rows` (default 1,000, capped at one batch) through `Text` doors, then
`hypertabular_suggest` tries each column against the doors. Each column gets a door, a param and a
status: `resolved`, `ambiguous` or `unresolved`, with the matched and sampled counts.

Order of preference: bool, `i32`, `i64`, `f64`, ISO date, timestamp, UUID, text. Blanks don't count.
One non-blank failure makes a column `unresolved`, read as text. A cell that fits several declarations,
such as `01/02/2026` under two date orders, is `ambiguous`. A suggestion is never applied: each binding
returns a plan to inspect and pass back.

### Headers

The original names are kept exactly, as today. Bindings that need unique names (R's data frames) repair
them on presentation and keep the original beside them.

D5's split, per binding. The strict resolver gains a new error for a duplicate name, and the optional
lookup doesn't change:

| Binding | Strict, refuses a duplicate | Optional, keeps the first match |
|---|---|---|
| Rust | `require` | `ordinal` |
| C# | `Ordinal` | `TryOrdinal` |
| Java | `ordinal` | `findOrdinal` |
| Go | `Ordinal`, `OrdinalBytes` | `Find`, `FindBytes` |
| Swift | `ordinal(of:)` | `firstIndex(of:)` |
| Python | `ordinal` | `get` |
| Ruby | `ordinal` | `find_ordinal` |
| PHP | `ordinal` | `find` |
| TypeScript | `require` | `ordinal` |
| C++ | `require` | `ordinal` |
| R | a named plan column | none |

C has no name lookups. Rust's new `Error` variant is safe because the enum is `#[non_exhaustive]`.

## The reader's contracts

- **Coordinates:** `line` is the record's first physical line, or a sheet's absolute row, 1-based.
  Output row counts within the batch, 0-based, except R, which is 1-based and counts across the read.
  `plan_column` and `source_column` are both reported, and both survive skip, header selection, ranges
  and blank-row filtering. A sheet is named by its name.
- **Spans:** a cell fault's span indexes the cell's decoded, unescaped text, never the original file.
  A `Failure`'s `byte` indexes the decoded stream, except `ENCODING`'s. A typed workbook cell's span
  covers its text form, as today. A host conversion fault has no span. Ruby's characters and R's
  `substr()` units stay.
- **Missingness:** R's input `NA`, declared tokens, blanks, padding, malformed values and uncached
  formulas are all told apart through `ok`, `reason` and `origin`.
- **Limits:** every size, allocation and growth request is checked before it's used, with no unchecked
  narrowing. An oversized record stays `row_too_long`, and a workbook keeps `TOO_LARGE`.

## HyperCast

- D7's `code_point` column and D8's `cast_text()` change with its `raw = TRUE`, with corpus and R
  tests. `test-constraints.R:165` and the NUL note at `constraints.R:246` change to match, and
  `r/README.md` says what `ok` with `NA` means for both doors: `ok` describes the core's acceptance,
  `value = NA` means R can't represent it, and `raw = TRUE` gives the exact output. So checking `ok`
  alone doesn't guarantee a usable character value, and the two packages' docs say that in those terms.
  Other bindings' strings can hold NUL, so they don't change.
- Anything HyperTabular's work proves is missing. That's the freeze's purpose.
- An R-level compatibility guard, below.
- Its checklist boxes and `cran-comments.md` updated from the exact tarball.

## R surface

- **Reading:** `read_delimited()` and `read_sheet()` take `skip`, `header_row`, `fill`, `long_rows`, `na`,
  `encoding` and, for sheets, `range`. `col_*()` take `na` and date `options`, and verdict columns gain
  `detail`, `detail_code` and `origin`.
- **`problems(x)`:** computed from the retained verdict columns when called, so reading pays nothing
  for it. Columns: `row`, `line`, `sheet`, `column`, `source_column`, `kind`, `reason`, `detail`,
  `origin`, `offset`, `length`, `raw`. Each row's `kind` is `cell`, `row` or `host_conversion`.
- **`suggest_plan()`:** prints a pasteable `list(...)` plan with each column's status and returns it.
- **Value types:** HyperCast's R table in `r/README.md` applies unchanged, so standalone casts and table
  columns agree, and a test holds them to it.
- **Ownership:** every vector R returns is R's own, and stays valid after the next batch, close and
  garbage collection.
- **Cleanup:** a `close()` method on readers, sheets and workbooks. It's safe to call repeatedly, a read
  after close is an R error, and finalizers remain as the safety net.
- **Interrupts:** `R_CheckUserInterrupt` at each batch and every 2^20 elements of a conversion, in C
  between core calls, so no Rust frame is live when R long-jumps. Native memory is owned by external
  pointers, so an interrupt can't leak it.

**Dependency architecture, recorded:** `Imports: hypercast` for its verdict class and format
constructors, plus an independently vendored crate. Crate attribution stays as it is.

**Compatibility guard (new):** `hypercast::num_format()$packed` and `date_options()$packed` go from the
installed hypercast into HyperTabular's vendored core, and the two can update separately. The core
refuses a size it doesn't read, and R turns that into an error naming both versions. A new detail code
reads as `"unknown"` with its number kept. Tests use doctored sizes and an unknown code.

## Binding surface

| Concept | Rust | Others |
|---|---|---|
| Ragged, skip, header row, encoding, tokens | new fields on the `#[non_exhaustive]` `Dialect`, and new builder methods | The binding's existing dialect shape. |
| Sheet options, range | new `SheetOptions` fields | The binding's `SheetOptions`. |
| Column tokens, date options | `Column` methods | The binding's column constructors. |
| Detail and origin | on the batch's fault | In HyperCast's fault type where it has `detail`; origin as a batch accessor. |
| Faults view | `batch.faults()` | `Faults()`, `faults`, as the binding's naming goes. |
| Suggest | `suggest()` | Same name, in the binding's casing. |

Defaults equal today's reads, except D1, D2 and D5. Views stay views in C#, Java, Go, Swift and C++, and
copies stay copies in Python, Ruby, PHP, TypeScript and R. Go's core is regenerated after every core change.
Docs follow the same pass: READMEs, `docs/design.md`'s table, `docs/delimited.md`, `docs/workbook.md`,
the CHANGELOGs and `r/NEWS.md`.

## CRAN gates

- **Checklist reset, first:** split the boxes by repo, tick what's already true with its evidence, fix
  the stale status run and "cascade" text, and add the gaps below.
- **HyperUuid's feedback:** a gate. Carry across every reviewer comment, or record that CRAN said there
  are none. The owner's answer decides how this closes.
- **New native checks:** `rchk`, `gctorture`, valgrind, UBSAN and ASAN, an LTO build, the noSuggests
  check, C and ISO-8859-15 locales. Evidence goes in `cran-comments.md`, for both packages.
- **Compiled code:** rerun the no-panic proof with the new exports, inspect the built R library, and
  record the scope and toolchain.
- **Reverse dependency:** a scripted step that checks hypertabular against a candidate hypercast, before
  any hypercast submission.
- **Tarball:** a measured size against 5 MB and 10 MB, with `r_corpus` extended by the new fixtures and
  a missing fixture still failing. The compressed corpus is about 270 KB today. The checklist's rule
  beats HyperUuid's older "ship a subset" note.
- **`cran-comments.md` for HyperTabular:** brought up to HyperCast's level: R-devel, the R 4.2 floor,
  Windows, the r-oldrel section (D9) and the Unicode sentence.
- **Tools:** win-builder, mac-builder, rhub and checktor on the exact tarballs. Those that upload or email
  the maintainer are the owner's to run.
- **Check time:** measured for both packages against CRAN's ten-minute norm.
- **Release cadence and order:** R ships a batch of releases, not every tag. HyperCast is accepted first.

## Evidence

- **Corpus:** a vector for every behaviour and boundary above, in `corpus/delimited.json` and
  `corpus/workbook.json`, each explained, replayed by every binding. New workbook fixtures for uncached
  formulas, ranges and header rows.
- **Workbook fixtures:** the uncached fixtures come from openpyxl and LibreOffice's writer, or are
  hand-assembled and said to be, and their names go in `r_corpus`.
- **Core:** the no_std, no-allocator, no-panic and fault-span checks all stay green, with allocation counts
  for every new path.
- **Standalone and table parity:** the same bytes through `cast_text()` and `col_text()`, and through
  `cast_char()`, give the same `ok`, `value` and raw output, NUL and invalid UTF-8 included.
- **R values:** integer extrema and sentinels, values around 2^53, decimal precision, temporal extremes,
  civil versus instant, U+0000, invalid encodings and missingness.
- **R lifecycle:** interrupted and erroring reads, repeated close, garbage collection under `gctorture`,
  retained batches, header edge cases, coordinates after filtering and ranges, bounded large inputs.
- **ABI and coexistence:** old exports replayed unchanged apart from D1 and D2, new options tested, both
  R packages loaded in either order.
- **Negative controls:** each new check is shown failing once.
- **Benchmarks:** every core and binding harness the changes reach, with losses printed, on one machine.
  This is also where the parked Go decision gets its data: re-measure against native after the quote fix,
  and only then decide on a cgo tag or an assembly scanner. That decision stays the owner's.
- **Submission:** `R CMD check --as-cran` on the exact tarballs, recorded in `cran-comments.md`.

## Out of scope

Formula evaluation, writing, XLS, UTF-16BE, encoding sniffing, multi-byte separators, a second quote or
escape character, ALTREP (a later optimization, if measurements ever call for it), and the Go cgo or
assembly work itself.

## Checklist coverage

| Checklist item | Section here |
|---|---|
| Rows 1, 2, 4, 5, 6, 9, 11, 12 | Core design |
| Rows 3, 7, 8 reaching HyperTabular | Extending the ABI, Binding surface |
| Quotes, Text, Input ordering, Formula state, ABI | Core design |
| Representation, Missingness, Conversion faults | D7, D8, The reader's contracts, R surface |
| Coordinates, Spans, Limits | The reader's contracts |
| Ownership, Cleanup, Interrupts | R surface |
| Headers | D5, Core design |
| R dependency, Lockstep, Panic boundary, Tarball, Cadence, HyperUuid's lessons | R surface, CRAN gates |
| Closing evidence | Evidence |
