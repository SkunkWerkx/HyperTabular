# Delimited text — design notes

The shared architecture — the core and what holds it, doors and columns, verdicts, the
batch, the C ABI, and the prior art — is recorded once in [`design.md`](design.md) and
[`prior-art.md`](prior-art.md). This file holds what is specific to delimited text.

Two layers read it. The core (`rust/src/kernel/delimited`) scans and casts into memory
its caller owns, and is what `libhypertabular` exports. `DelimitedReader`
(`rust/src/delimited.rs`) is the Rust binding over it — the same thing each language's
binding is: it owns the buffers, reads the file, and lends out what the core wrote.

## What is fixed here, and why

- **Separator: one ASCII byte** — tab or `0x20..=0x7E`, never `"`. Anything else is the
  caller's mistake, said before any read (`Error::Separator`; `ERR_CONTRACT` from the
  core). Multi-byte separators are out of scope: the scanner's structural classes are
  single bytes by construction.
- **Quote character: `"` only**, RFC 4180 doubling. No escape character. `quoting: false`
  removes the quote class from the scanner entirely.
- **Quote boundaries** (`kernel/delimited/scan.rs`): a quote opens a quoted cell only as
  the cell's first byte. Inside, `""` is a literal quote and a lone `"` closes it. After
  closing, quotes are ordinary bytes until a separator or a line ending, and the same goes
  for a cell that never opened. So `"x"y"z,w"` is two cells, `12" pipe,1` is two cells, and
  a stray quote can't hide a row. Input that ends inside an open cell is
  `FailureKind::UnclosedQuote`.
- **Quote resolution** (`kernel/delimited/unescape.rs`): a cell whose first byte is `"`
  and whose only two quotes are its first and last bytes is its inner slice, zero-copy. A
  cell that starts with `"` and contains more is unescaped once, into the arena: `""` →
  `"`, a lone trailing `"` is the closing quote, any other lone `"` is kept literally. A
  cell that does not start with `"` is delivered raw, quotes and all. Nothing is repaired
  silently — the HyperCast door reports `Malformed` at the exact byte. Resolution never
  moves a boundary, and it doesn't track where the quoted part closed: `"x"y"z"` reads
  `x"y"z`.
- **Row terminators**: `\n`, `\r\n`, `\r`. A `\r` that is the last byte of input that is
  not yet the last is held back until more says whether a `\n` follows.
- **Blank lines** are skipped by default (`skip_blank_lines`), matching csv-core and
  DuckDB. Off, a blank line is a one-cell row — and will fail the column-count check
  unless the file is single-column, which is the only case where delivering it is
  meaningful.
- **Column count** is fixed by the first record (header or not). Disagreement is a
  structural failure (`FailureKind::ColumnCount`) carrying the record's index, its line,
  its absolute byte offset, and the two counts — returned once every intact row before it
  has been delivered, and again on every read after.
- **UTF-8 BOM** at byte 0 is skipped. The scanner looks at nothing else of the encoding,
  since no byte of a multi-byte character can be a structural one.
- **Another encoding is declared, never sniffed.** `Dialect::encoding` says Windows-1252, Latin-1
  or UTF-16LE, and the text is decoded ahead of everything here. See "Decoding a declared
  encoding".
- **Text that isn't valid UTF-8** is judged by HyperCast's text door, without a copy. In a
  column read through the text door it is a `Malformed` verdict on that cell, spanning the
  first invalid sequence of the unescaped text (`\xe9t\xe9` is `Malformed` at offset 0,
  length 1), and the rows go on. A numeric or date door has always refused it the same way.
  A cell no plan column reads gets no verdict. A header name has no verdict to carry a
  fault, so one that isn't valid is a structural failure, `FailureKind::Encoding`, located as
  a column count failure is. An embedded NUL is valid text and passes through.
- **How that's checked.** When the plan has a text column, a fill first asks the door about
  the whole region its rows came from, once. If that's valid, every cell in it is: a cell is
  cut out at separators, line ends and quotes, all ASCII, and no byte of a multi-byte
  character is ASCII, so valid UTF-8 split at those bytes stays valid; unescaping only takes
  ASCII quotes out of what's left. A refused region says where its first bad byte is, and no
  more: the row holding that byte is set aside and the rest of the region asked again from the
  row after, for up to sixteen rows a fill. The door is then asked about each text cell of the
  rows set aside, and of every row past the sixteenth's, unescaped, and only that gives a
  verdict or a span. So a bad cell costs its batch a second reading of its bytes and faults
  only itself, and a bad byte in a column the plan doesn't read costs the same and faults
  nothing.
  `the_region_check_gives_what_each_cell_would` holds this to the door, cell for cell, on
  seeded input, at every chunk split.
- **Header** (`has_header`) is the first record, read when the reader is built, exposed
  as `Header` with exact-bytes ordinal lookup, and fixes the column count. It is never
  trimmed, folded, or de-duplicated.

## The core's call

One call a batch: `fill(state, input, last, specs, columns, max_rows, cells, arena, out)`.

- `state` is a block of the caller's, as large as `hypertabular_delimited_state_size` says: the
  dialect, the line and record counters, the column count, and the failure once there is one, in
  its first 64 bytes, and what `_init_with` was asked for after them. That is 80 bytes; a block
  only `_init` writes may be the 64 that 1.0 reported, and the core reads no more of it.
- `input` starts where the last call's `consumed` ended; `last` says nothing follows it.
- `specs` is the plan — an ordinal, a door, what the door declares, a numeric format —
  and `columns` the two arrays each plan column is written into: values of the door's own
  type, and a verdict beside each.
- `cells` is the cell table: per row, a span for every source column the plan reaches
  (the cell's text in the input, flagged if it is quoted text still to be unescaped) and
  one more entry with the row's start and its line.
- `arena` takes the unescaped text of the cells that needed unescaping.

The core scans as many whole rows as fit — `max_rows`, the input, the arena — into the
cell table, then casts column-major: one loop per plan column, HyperCast's door called on
each cell's bytes. It returns how many rows it wrote and how many bytes it finished with.
A row that would not fit the arena ends the batch before it; a table or an arena too
small for even one row is `ERR_CELLS` or `ERR_ARENA` with the size one row takes, and
nothing consumed. Nothing is ever half-read: a call either delivers whole rows or leaves
the state as it found it, which is what lets a caller feed the unconsumed tail again with
more behind it.

## What a read does, in order

`_init_with` and `_fill_with` add to the same call. Each step only ever sees what the one before
it left:

1. Decode: a declared encoding goes through `hypertabular_transcode`, ahead of these calls. Past
   here the input is UTF-8, declared or decoded, and nothing is judged yet.
2. A byte-order mark goes. It is not a line. A UTF-16LE mark was decoded to the same three bytes.
3. `skip` passes over physical lines. Nothing in them is read, so a quote in one means nothing,
   and a bad byte in one is nothing either. A declared encoding has already decoded the line, so
   a bad unit in one has already failed. They count toward a position's `line` and `byte`, not its
   `record`.
4. What's left is cut into records and cells, quotes deciding the boundaries.
5. `header_row` discards records (a blank line the dialect skips isn't one), and then the header
   comes if there is one. Discarded records fix no width. A failure's `record` counts them.
6. The first record kept fixes the width, and `short_rows` and `long_rows` decide what a record
   of another width is: a structural failure unless they say otherwise; or padded with cells it
   doesn't have; or cut to the width, the dropped cells never read. `row_widths` holds what each
   row had first, and under `long_rows = report`, `hypertabular_faults` lists a row-level entry for
   each row it shows was cut, with `expected` and `found` (`docs/design.md`, "Problems").
7. A delivered cell is unescaped. A span in a verdict indexes that text, not the file's.
8. Tokens: a cell the row doesn't have is `Empty` at origin `padded`, and an unquoted cell whose
   exact bytes are a token is `Empty` at origin `token`, on every door including text. A quoted
   cell is never a token. A column's list replaces the dialect's; an empty list means none.
9. The door judges what's left, under the column's numeric notation and date options, and with
   notes on says why through `hypercast::detailed`: `detail` and origin `cell`.

A padded cell is told from an empty one by the cell table: a cell that exists past a row's first
starts after a separator, so only a blanked entry says offset 0 there. A quoted cell is told from
an unquoted one by the byte before its span.

A column that asks for no notes, tokens or date options runs the loop `_fill` runs, with no door
asked why. The Rust reader takes notes unless `DelimitedOptions::notes(false)` says otherwise.
Apple M6, Rust 1.99, 2026-11-06, interleaved, the median of six runs of Criterion's median, against
the same reader with notes off: `cells/hypertabular` 8.87 ms against 8.32 ms (+6.7%),
`cells/hypertabular-utf8` 21.04 ms against 20.66 ms (+1.8%), `batch/typed/hypertabular` 13.67 ms
against 13.67 ms (no change), and `faulty/hypertabular`, where four cells in five fail their door
and have a note to write, 8.03 ms against 7.22 ms (+11.2%). Notes off still costs what the options'
checks do, against the tree before them, in a separate interleaved run that day:
`cells/hypertabular` 8.40 ms against 8.23 ms (+2.0%), `cells/hypertabular-utf8` 20.50 ms against
20.32 ms (+0.9%), `batch/typed/hypertabular` 13.77 ms against 13.41 ms (+2.6%). The earlier tables
predate both.

The region check is the same for both fills, and a region is from the first delivered row to the
end of the last, so lines passed over and records discarded are no part of it; a bad byte in a cell
a cut row dropped can only set its row aside to be asked cell by cell, which gives the same answers.

## Decoding a declared encoding

`hypertabular_transcode(encoding, input, input_len, last, out, cap, progress)` decodes `1`
Windows-1252, `2` Latin-1 or `3` UTF-16LE to UTF-8. It keeps nothing between calls. A Windows-1252
byte the code page leaves undefined is its C1 control, as WHATWG has it. UTF-16BE and sniffing are
out of scope: a declared encoding is the caller's word.

It says how far it got in a `RawTranscoded`, of this call's input and output alone:

- It writes whole characters, up to 4 bytes each, and stops at one that doesn't fit, so `consumed`
  is always where to start next and `out[..written]` is always valid UTF-8.
- A code unit the input cuts off (half a unit, or a high surrogate without its low) stays
  unconsumed until `last`. The caller hands it over again in front of the next chunk.
- A lone surrogate, or at `last` a unit cut off, returns `OK` with `failed` `1` and `failure_at` its
  offset in this call's input, which is `consumed`. That needs no room in `out`. Nothing is ever
  replaced.
- `FF FE` is U+FEFF wherever it falls. The fill's skip at byte 0 is what takes the stream's first
  one away, so a second, or one at the start of a later chunk, stays in the text. In Windows-1252
  and Latin-1, `EF BB BF` is three characters and no mark.

A reader runs it ahead of the fill, on two buffers of its own: `raw`, the original bytes (at least
4), and `text`, what the fill reads. `offset` starts at 0. When the fill says it needs more
input:

```text
if failed_at is set:
    if text's unconsumed part ends in \r: append \n (grow text if it's full), and fill again
    raise Failure::ENCODING: byte = failed_at, line and record from the state
move text's unconsumed part to its front, and grow text if that fills it
loop:
    transcode(encoding, raw[at..end], eof, text[filled..])
    if failed: failed_at = offset + failure_at
    at += consumed; offset += consumed
    if written > 0 or failed: fill again, not last
    if at < end and text has under 4 bytes of room: grow text
    else if eof: fill again, with last
    else: move raw[at..end] to raw's front, read more behind it; a read of 0 sets eof
```

The fill is only told `last` once the source has ended and all of it is decoded. A failure is never
`last`: everything ahead of the bad unit reaches the fill first, whole rows are delivered, and what's
left, the record the failure cut, is not a row, so it can't be a wrong width or an open quote. The
one thing the fill would still be holding is a `\r` that ended its record, waiting to see if a `\n`
is part of it, and a failure means none is: the reader appends that `\n` so the row is delivered. In
a quote it changes nothing, since a cut record is never delivered. It goes in the decoded buffer,
never the file, and the failure's `byte` isn't taken from it.

Everything else about the read is the same, and speaks of the decoded text: a batch's text, a span,
the `byte` of any other failure, and the buffer `MAX_ROW_BYTES` holds a record to. `Failure::ENCODING`'s
`byte` alone is the original input's, since the decoded text has no bad unit to point at, and its
`line` and `record` are the record the failure cut. A slice has to go through the same buffers, so it
is copied, and read like a stream.

A declared encoding can refuse a legal record within 3 bytes of `MAX_ROW_BYTES`' 1 GiB. With under 4
bytes of room and a unit still undecoded, the reader doesn't tell a character that won't fit from a
unit the chunk cut off, and asks for more room in both cases; at the ceiling that's
`FailureKind::RowTooLong`, though what the cut unit turned out to be might have fit.

## The reader

- A slice is scanned in place; nothing is copied but unescaped cells (under a declared encoding,
  it's read as a stream). It is handed to the
  core under 2 GiB at a time, the most one call takes, so only a single record that long
  is `FailureKind::RowTooLong`.
- A file or a stream is read through one buffer (256 KiB to begin with,
  `DelimitedOptions::buffer_bytes`; `0` is refused, and a size past 1 GiB starts there). When the core consumes nothing, the unconsumed tail
  is moved to the front, more is read behind it, and the call is made again. Each successful
  read returns to parsing, so a complete header or batch need not wait for the buffer to
  fill or the source to close. Because a
  row start is always a clean scanner state (no open cell, no pending `\r`), nothing
  is rebased — the cost is re-scanning at most one row per refill.
- A row that does not complete within the buffer doubles it, up to
  `DelimitedReader::MAX_ROW_BYTES` (1 GiB), then `FailureKind::RowTooLong`.
- The reader owns the column arrays (`DelimitedOptions::batch_rows` long, 4096 by
  default; `0`, and a batch past the core's 4 GiB limit, are refused before anything is allocated:
  `docs/design.md`, "Errors"; a suggestion sizes its own batches, and they're never refused), the cell table and the arena, and lends them: `read()` returns a `Batch` that
  borrows the reader. A column of the batch is the array the core wrote; a text cell is a
  slice of the input, or of the arena if it had a doubled quote in it. Reading the next
  batch is what ends the last.
- The cell table holds a batch of rows, or as many as 4096 × 257 entries (8 MiB: a default
  batch of a plan reaching 256 source columns) hold, and never less than one row. A plan
  reaching further reads shorter batches, rather than a table its reach multiplied by the
  batch size; every binding sizes it by the same rule.
- An arena too small for a batch's unescaped text makes for short batches, not an error;
  the reader doubles it once the short batch has been given back.
- The plan can wait for the header. `DelimitedReader::open_unbound` (and `from_slice_unbound`,
  `from_reader_unbound`) read the header and stop; `bind(&plan)` then sizes the column
  arrays and the cell table, once, before the first read. Nothing in the core's calls
  changes: `_header` never touched the plan. Reading before binding is `Error::Unbound`,
  binding twice `Error::AlreadyBound`, and the plan-taking constructors bind before the
  header is read, so a plan error still comes first. `Error::Plan` is a notation the core
  refuses, an ordinal of 2^20 (1,048,576) or more, an ordinal at or past the width a header
  has given every record (a plan bound before the header is held to it as soon as the
  header is read), or a cell table too large to allocate. The cap is every binding's: no
  workbook format has that many columns (XLSX and ODS stop at 16,384, Numbers at 1,000),
  and at the cap one row of cell table is at most 8 MiB, so a headerless plan cannot ask
  for the gigabytes a row reaching 2³¹ would take. Without a header the width is not known at bind, and a column
  past a record's last cell reads as empty.
- Behind the `async` feature, a reader opened with `from_async_reader` is refilled by
  awaiting a `futures_io::AsyncRead`; the fill between awaits is the same synchronous
  call. Each chunk is counted as it lands, so a `read_async` dropped part-way loses
  nothing and the next goes on from there.

## The scanner (`kernel/delimited/engine.rs`, `scan.rs`)

Per 64-byte block: four compare masks (separator, `\n`, `\r`, `"`), then the bytes hidden
inside quoted cells, then `structural = (sep | lf | cr) & !hidden`. Bits are popped in order;
every bit ends a cell, `\n`/`\r` also end the row, and a `\r\n` pair is one ending (the `\n`
is cleared from the mask, or carried across the block edge when the `\r` is bit 63). Each cell
records its quote count from a popcount of the quote mask between its start and end.

`hidden` takes one of three routes:

- A block with no quote and no open cell hides nothing, and needs no prefix XOR.
- Otherwise the inclusive prefix XOR of the quote mask (a carry-less multiply on aarch64
  PMULL / x86-64 PCLMULQDQ, a six-round shift cascade otherwise), XORed with the parity carried
  in, gives `inside`. When every quote that turns `inside` on is a cell's first byte or the
  second of a `""`, `inside` is the answer. Checking that takes a few bit operations, so
  well-formed files stay in this route however many quotes they have.
- Otherwise a stray quote is in the way, and the walker goes through that block's cells in
  order, reading the same prefix XOR from each opening quote to find its close. Only that
  block pays for it.

An open cell carries across blocks, with its parity. A quote at bit 63 reads the next byte to
see whether it's half of a `""`. Parity is a carry only inside an open cell: with no open
cell, the next block's state depends on where the cell boundaries fell, not on a count of
quotes.

Engines, best first: `neon+pmull`, `neon` (aarch64); `avx2+pclmulqdq`, `sse2+pclmulqdq`,
`sse2` (x86-64); `swar` (everywhere). The walker is monomorphised per engine under the
matching `#[target_feature]`; the core asks the CPU itself which it can run, without the
standard library, and caches the answer. `tests/kernel_delimited.rs` runs every engine
the machine offers.

## Allocation story

The core allocates nothing, ever: `tests/kernel_allocation_free.rs` watches a whole input
go through it with a counting allocator, the transcoder too, and `check-core.sh` holds the shipped library's
import list to one with no allocator on it. The reader allocates its buffers once and
grows them as the data demands; `tests/reader_allocation_free.rs` asserts that once the
first batches have sized them, a read allocates nothing — over a slice and over a stream.

## What holds it to the truth

- `corpus/delimited.json` — cases whose cell grid is written by hand and whose verdicts
  are HyperCast's own, replayed through the reader from a slice, a stream through
  buffers of 1, 5 and 64 bytes, and a file, in batches of 1, 2 and 1024 rows
  (`tests/corpus_delimited.rs`). Every binding replays the same file.
- The `csv` crate, as a reference that owes this crate nothing: random and hand-picked
  RFC 4180 input must come out as the same cells through the reader
  (`tests/delimited_reader.rs`), and through the core on every engine and every door,
  with HyperCast asked directly for each cell's verdict (`tests/kernel_delimited.rs`).
  The corpus has the invalid UTF-8: its `csv` reference doesn't validate.
- A byte-at-a-time state machine in `tests/kernel_delimited.rs`, for what the `csv` crate
  doesn't agree on: random strings full of stray quotes must come out as the same cells,
  rows, lines and failures on every engine, whole, cut in two, and one row a scan.
- Rust's own `char::decode_utf16`, as the reference for the transcoder, over random units full of
  surrogates (`tests/kernel_delimited.rs`), with every Windows-1252 byte against WHATWG's table
  written out by hand, every split of a chunk and every size of `out`.
- HyperCast's door asked about each cell on the spot, in `tests/kernel_delimited.rs`, for what
  the region check could get wrong: random files with broken bytes, read through both fills, with
  plans reading some ordinals or no text at all, byte-order marks, blank lines, rows of the wrong
  width, and an arena and cell table that start small and grow when refused; and files of many
  rows with a bad byte in a few of them, in most or in all, so that rows are set aside up to the
  count and past it.

## Exports

`hypertabular_delimited_state_size`, `_init`, `_init_with`, `_header`, `_fill`, `_fill_with`,
`_unescape`, `hypertabular_transcode`, `hypertabular_faults`, `hypertabular_suggest`,
`hypertabular_batch_check` and `hypertabular_version`, over the `#[repr(C)]` shapes in `kernel/abi.rs`. The library exports nothing of HyperCast's: the doors are compiled in, not re-exported, so it links
beside `libhypercast` without a clash.

## Numbers

Linux x64, a 66 MB, million-row file, six columns cast to `i64`, `f64`, text, a timestamp,
a boolean and `i32`: about 575 MB/s through the core into the caller's buffers, against
900 MB/s for the `csv` crate splitting records without casting anything. That receipt is from
before 1.0 and predates the quote rule, UTF-8 validation and notes; it hasn't been run again.

`cargo bench --bench delimited_benchmarks` measures three scopes on a generated
200 000 × 10 file (26 MB): `structure` (the core with an empty plan, on every engine),
`cells` (every cell through the text door, against the `csv` crate) and `batch` (ten typed
columns), and `structure` again on two more files, `structure_all_quoted` (every cell quoted)
and `structure_stray_quote` (stray quotes in every row). `cells` also runs on two more:
`cells/hypertabular-utf8`, the same shape with every cell multi-byte, and
`cells/hypertabular-bad-per-batch`, the first file with one byte that isn't UTF-8 in every 1024
rows, so that every batch's region is refused and a few of its rows set aside, and
`cells/hypertabular-bad-every-row`, the first file with such a byte in every row, so that all
but sixteen rows of a batch are judged cell by cell, the most validation can cost. `faulty` is a 100 000-row file where four cells in five fail their door.

This release against 1.0, Apple M6 (Mac18,5), macOS 27.2, Rust 1.99.1, 2026-11-10, one session.
**1.0** is tag `v1.0.0`'s `rust/` built over HyperCast 1.0.0 from crates.io (byte for byte its
`v1.0.0` tag); **now** is this release's tree over HyperCast 1.1 (`439b8cf`), simdutf8 in its text door. A
third build splits each loss between the two libraries: 1.0's `rust/` over HyperCast 1.1, so
**HyperCast** is that build against 1.0 and **HyperTabular** is now against that build. All three
run this release's benchmark file; 1.0's copy leaves out the stray-quote file, which 1.0 can't read,
and the setup check that every batch of the bad-per-batch file has a refused cell, which fails on
1.0 because 1.0 refuses nothing (it runs before the timing and changes none of it). Six runs of each
build, interleaved with the order rotated, the median of each build's six Criterion medians; load
average 2.2 to 3.6, so the builds' six medians spread by up to 7%.

| Scope | 1.0 | Now | Change | HyperCast | HyperTabular |
| --- | ---: | ---: | ---: | ---: | ---: |
| `structure`, NEON + PMULL | 3.28 ms | 3.57 ms (6.77 GiB/s) | +9.0% | +0.3% | +8.7% |
| `structure`, NEON | 3.35 ms | 3.58 ms | +6.8% | +0.4% | +6.4% |
| `structure`, SWAR | 6.77 ms | 6.73 ms | -0.6% | +0.1% | -0.8% |
| `structure_all_quoted`, NEON + PMULL | 3.44 ms | 3.92 ms | +13.9% | +0.2% | +13.7% |
| `structure_all_quoted`, NEON | 3.51 ms | 3.93 ms | +11.8% | +0.9% | +10.9% |
| `structure_all_quoted`, SWAR | 7.38 ms | 7.39 ms | +0.1% | +0.3% | -0.2% |
| `structure_stray_quote`, NEON + PMULL | no such read | 4.17 ms | | | |
| `structure_stray_quote`, NEON | no such read | 4.20 ms | | | |
| `structure_stray_quote`, SWAR | no such read | 7.50 ms | | | |
| `cells`, HyperTabular | 7.80 ms | 9.03 ms (2.68 GiB/s) | +15.8% | +0.8% | +14.9% |
| `cells/hypertabular-utf8` | 8.47 ms | 11.37 ms | +34.1% | +0.3% | +33.8% |
| `cells/hypertabular-bad-per-batch` | 7.84 ms | 9.38 ms | +19.7% | +0.6% | +19.0% |
| `cells/hypertabular-bad-every-row` | 7.86 ms | 18.15 ms | +131.0% | +0.2% | +130.5% |
| `cells`, the `csv` crate (control, unchanged code) | 13.22 ms | 13.08 ms (1.85 GiB/s) | -1.1% | -1.0% | -0.1% |
| `faulty`, HyperTabular | 6.13 ms | 6.74 ms | +9.9% | +0.1% | +9.8% |
| `batch`, ten typed columns | 13.06 ms | 14.17 ms (1.71 GiB/s) | +8.5% | +2.3% | +6.0% |

Every scope the core reaches is a loss but `structure` on the portable engine, and the `csv`
crate's row is the control that the session was level. Where each comes from:

- **HyperCast's share is nothing measurable.** `faulty`, where four cells in five fail their
  door (`123x`, `y123` and an empty cell at `i32`, `123z` at `f64`), is level under 1.0's core
  (+0.1%). It was +15.4% on 2026-11-09, when HyperCast 1.1's doors took that much longer to
  refuse them; HyperCast has since taken that back (its `docs/benchmarks.md`, "Refused cells").
  The typed doors in `batch` came out +2.3%, inside this session's spread, and every other scope
  within 1.0%: 1.0 never asks the text door.
- **The rest is HyperTabular's.** `cells` is validation plus notes and the options' checks.
  Validation is asked of HyperCast's text door, now SIMD, so the multi-byte file costs +34% where it
  cost +150% against core's check. One bad byte in every batch costs 4% over the valid file (9.38
  against 9.03 ms): the region is refused, the row holding the byte is found from where the door
  stopped and set aside, the rest is read as valid, and that row's text cells are asked about
  alone. A bad byte in every row is the worst case, twice the valid file (18.15 ms): past sixteen
  rows set aside, every cell left in the batch, most of them shorter than the 64 bytes the SIMD
  check starts at, is checked alone. Setting rows aside is not free on a clean read: `cells` and
  `batch` were each 1 to 2% faster in the same tree before it (8.80 against 8.96 ms and 14.14
  against 14.28 ms, five interleaved runs, on 2026-11-09). `faulty` and `batch` are mostly
  notes, with the options' checks, and in `batch` the validation of its two text columns.
- **`structure`'s +7 to +14% on NEON is the quote rule and the fill's per-row bookkeeping.**
  Swapping the scanners between the trees on 2026-11-08 (1.0's `rust/` with this release's
  `scan.rs`, and this release with 1.0's, five or six interleaved runs each) put the quote rule at
  +1.5 to +4.0% on `structure` and +4.5 to +7.0% on the all-quoted file, and the bookkeeping at +5
  to +9%. A read that asks for no header row to pass over, no widths and no policy for a row of
  the wrong width now runs through a row sink without those four steps, which took 2 to 3 points
  off both files on NEON (master against this tree, five interleaved runs: 3.65 to 3.53 ms and
  3.95 to 3.87 ms with PMULL). The portable engine keeps the full sink, because it measured 2.7%
  and 4.4% slower through the lean one, and is level with 1.0 as it was. It isn't code alignment:
  building every side with every function and branch target aligned leaves the same gaps. The
  x86-64 engines take the lean sink and haven't been measured with it.

Linux x64 (WSL 2), i9-11900H, Rust 1.99, 2026-10-07, Criterion's median, before a quote could only
open at a cell's first byte, and before validation and notes; not run again on that machine:

| Scope | Time | Throughput |
| --- | ---: | ---: |
| `structure`, AVX2 + PCLMULQDQ | 11.1 ms | 2.18 GiB/s |
| `structure`, SSE2 + PCLMULQDQ | 12.1 ms | 1.99 GiB/s |
| `structure`, SSE2 | 13.0 ms | 1.85 GiB/s |
| `structure`, SWAR (portable) | 20.7 ms | 1.16 GiB/s |
| `cells`, HyperTabular (text door) | 23.0 ms | 1.05 GiB/s |
| `cells`, the `csv` crate (no cast) | 25.3 ms | 978 MiB/s |
| `batch`, ten typed columns | 37.8 ms | 654 MiB/s |

What that rule cost, Apple M6, Rust 1.99, 2026-11-05, before and after in one session:

| Scope | Before | After | Change |
| --- | ---: | ---: | ---: |
| `structure`, NEON + PMULL | 3.18 ms | 3.30 ms | +3.8% |
| `structure`, NEON | 3.28 ms | 3.35 ms | +2.1% |
| `structure`, SWAR | 6.66 ms | 6.73 ms | +1.0% |
| `structure_all_quoted`, NEON + PMULL | 3.35 ms | 3.58 ms | +6.9% |
| `structure_all_quoted`, NEON | 3.44 ms | 3.67 ms | +6.9% |
| `structure_all_quoted`, SWAR | 7.32 ms | 7.57 ms | +3.5% |
| `structure_stray_quote`, NEON + PMULL | no such read | 3.90 ms | |
| `structure_stray_quote`, NEON | no such read | 3.95 ms | |
| `structure_stray_quote`, SWAR | no such read | 7.36 ms | |
| `cells`, HyperTabular | 7.68 ms | 7.75 ms | +0.9% |
| `batch`, ten typed columns | 13.08 ms | 13.09 ms | +0.1% |

Before the rule, the stray-quote file could not be read at all: its quotes hid separators, and
the scan stopped on a structural failure.

What validating text as UTF-8 costs, Apple M6, Rust 1.99, 2026-11-06: the same tree with and
without validation, interleaved, the median of six runs of Criterion's median.
`cells/hypertabular-utf8` is a file of the same shape with every cell multi-byte, where the loss
is much larger: the validator's fast path (`core`'s, before HyperCast took simdutf8) only covers
ASCII; the release table above is the same validation with HyperCast's SIMD check, +32% on that
file. The earlier tables predate validation. A batch with an invalid cell pays for the region and then for the rows holding one: the release
table above measures that, at 5% over the valid file for one such row in a batch and twice its
time when every row is one.

| Scope | Without | With | Change |
| --- | ---: | ---: | ---: |
| `cells`, HyperTabular | 7.74 ms | 8.27 ms | +6.8% |
| `cells/hypertabular-utf8` | 8.27 ms | 20.36 ms | +146.2% |
| `batch`, ten typed columns, one of them text | 13.04 ms | 13.42 ms | +2.9% |
| `cells`, the `csv` crate (control, unchanged code) | 13.03 ms | 13.04 ms | +0.1% |

## Parked

- **A long record from a stream that hands out a little at a time is quadratic.** A stream source
  reads once per call, so finished rows reach the caller when a stream stalls, and the times say
  that a record that hasn't ended yet is scanned again from its start on every call. Through the Rust reader, one
  record of `x` and one text column, a stream handing out 64 KiB a read, Apple M6, Rust 1.99.1,
  2026-11-08, one run each: 16 MiB in 0.095 s, 32 MiB in 0.36 s, 64 MiB in 1.48 s, 128 MiB in
  5.89 s, 256 MiB in 23.8 s, four times the time for twice the record; the same 256 MiB from a
  stream that fills the room it's given takes 0.064 s. Peak resident is the same either way, 542 MB
  for 256 MiB. Swift's README has its own row, and Java's is the same shape (21.8 s for 256 MiB on
  2026-11-07). Records of a few megabytes don't feel it. The cure is to remember how far the scan
  of an unfinished record got and resume there, which is a change to the core's state across calls,
  not made in this release.
- **A third notation for the suggester: Indian grouping.** With no format declared, `1,23,456` reads
  under neither candidate, each being Western, so it's text, resolved, and beside `5` unresolved: nothing
  is lost, and declaring Indian grouping for the column reads 123456 and suggests a number. A third
  candidate would read `1,23,456` and `1,234` alike and resolve the column numeric, at the cost of a
  `param` bit and a mapping in every binding, and of inferring a culture from digits, which the owner's
  rule is that no read does. A suggestion moving from text to a number later is additive, so it's left
  until it's asked for.
- **Locales neither grouping policy matches.** `tok`, glibc's `[4, 0]` locales and `unm_US`: HyperCast's
  [grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) hold what was measured. Nothing is added here until a policy is. A declared format
  with a policy refuses their grouped numerals (one declaring separator detection infers `,` and `.`
  instead, and reads `1,2345` as 1.2345), but the suggester follows the delimiter rule: a `[4, 0]` writer's
  `1,2345` has more than three digits after its one `,`, so it suggests a comma decimal, 1.2345. The
  report names that notation, which is why a suggestion is something to inspect.

- Sep's tiered mask fast paths (pure-separator blocks, separator+newline blocks).
- Parallel chunk scanning. Polars' two-state chunk analysis assumes quote parity alone says
  whether a chunk starts inside quotes. Here it also takes knowing where the cell started, so a
  chunk scanner would have to start each chunk at a known row boundary, or validate its guess
  against the walker's rules.
- Multi-byte separators and alternative quote characters.
- Encodings past UTF-8, Windows-1252, Latin-1 and UTF-16LE.
