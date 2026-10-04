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
- **Quote resolution** (`kernel/delimited/unescape.rs`): a cell whose first byte is `"`
  and whose only two quotes are its first and last bytes is its inner slice, zero-copy. A
  cell that starts with `"` and contains more is unescaped once, into the arena: `""` →
  `"`, a lone trailing `"` is the closing quote, any other lone `"` is kept literally. A
  cell that does not start with `"` is delivered raw, quotes and all. Nothing is repaired
  silently — the HyperCast door reports `Malformed` at the exact byte.
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
- **UTF-8 BOM** at byte 0 is skipped. Encoding is not otherwise inspected: bytes in, bytes
  out, and the HyperCast doors read UTF-8.
- **Header** (`has_header`) is the first record, read when the reader is built, exposed
  as `Header` with exact-bytes ordinal lookup, and fixes the column count. It is never
  trimmed, folded, or de-duplicated.

## The core's call

One call a batch: `fill(state, input, last, specs, columns, max_rows, cells, arena, out)`.

- `state` is a 64-byte block of the caller's: the dialect, the line and record counters,
  the column count, and the failure once there is one.
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

## The reader

- A slice is scanned in place; nothing is copied but unescaped cells.
- A file or a stream is read through one buffer (256 KiB to begin with,
  `DelimitedOptions::buffer_bytes`). When the core consumes nothing, the unconsumed tail
  is moved to the front, more is read behind it, and the call is made again. Because a
  row start is always a clean scanner state (quote parity zero, no pending `\r`), nothing
  is rebased — the cost is re-scanning at most one row per refill.
- A row that does not complete within the buffer doubles it, up to
  `DelimitedReader::MAX_ROW_BYTES` (1 GiB), then `FailureKind::RowTooLong`.
- The reader owns the column arrays (`DelimitedOptions::batch_rows` long, 4096 by
  default), the cell table and the arena, and lends them: `read()` returns a `Batch` that
  borrows the reader. A column of the batch is the array the core wrote; a text cell is a
  slice of the input, or of the arena if it had a doubled quote in it. Reading the next
  batch is what ends the last.
- An arena too small for a batch's unescaped text makes for short batches, not an error;
  the reader doubles it once the short batch has been given back.

## The scanner (`kernel/delimited/engine.rs`, `scan.rs`)

Per 64-byte block: four compare masks (separator, `\n`, `\r`, `"`), the inclusive prefix
XOR of the quote mask (a carry-less multiply on aarch64 PMULL / x86-64 PCLMULQDQ, a
six-round shift cascade otherwise) XORed with the carried parity, then
`structural = (sep | lf | cr) & !inside_quotes`. Bits are popped in order; every bit ends a
cell, `\n`/`\r` also end the row, and a `\r\n` pair is one ending (the `\n` is cleared from
the mask, or carried across the block edge when the `\r` is bit 63). Each cell records
its quote count from a popcount of the quote mask between its start and end.

Engines, best first: `neon+pmull`, `neon` (aarch64); `avx2+pclmulqdq`, `sse2+pclmulqdq`,
`sse2` (x86-64); `swar` (everywhere). The walker is monomorphised per engine under the
matching `#[target_feature]`; the core asks the CPU itself which it can run, without the
standard library, and caches the answer. `tests/kernel_delimited.rs` runs every engine
the machine offers.

## Allocation story

The core allocates nothing, ever: `tests/kernel_allocation_free.rs` watches a whole input
go through it with a counting allocator, and `check-core.sh` holds the shipped library's
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

## Exports

`hypertabular_delimited_state_size`, `_init`, `_header`, `_fill`, `_unescape`, and
`hypertabular_version`, over the `#[repr(C)]` shapes in `kernel/abi.rs`. The library
exports nothing of HyperCast's: the doors are compiled in, not re-exported, so it links
beside `libhypercast` without a clash.

## Numbers

Linux x64, a 66 MB, million-row file, six columns cast to `i64`, `f64`, text, a timestamp,
a boolean and `i32`: about 575 MB/s through the core into the caller's buffers, against
900 MB/s for the `csv` crate splitting records without casting anything.

`cargo bench --bench delimited_benchmarks` measures three scopes on a generated
200 000 × 10 file: `structure` (the core with an empty plan, on every engine), `cells`
(every cell through the text door, against the `csv` crate) and `batch` (ten typed
columns). It has not been re-run since the reader was rebuilt on the core; the figures
this file used to carry were of the first, row-at-a-time reader, which is gone.

## Parked

- Sep's tiered mask fast paths (pure-separator blocks, separator+newline blocks).
- Parallel chunk scanning (Polars' two-state chunk analysis on top of the same masks).
- Multi-byte separators and alternative quote characters.
- Encodings other than UTF-8.
