# HyperDelimited — design notes

The shared architecture — the `Cell`/`Plan`/`Batch` contract, the cast matrix, the FFI
shape, and the prior art — is recorded once in HyperTabular's
[`docs/design.md`](https://github.com/SkunkWerkx/HyperTabular/blob/master/docs/design.md)
and [`docs/prior-art.md`](https://github.com/SkunkWerkx/HyperTabular/blob/master/docs/prior-art.md).
This file holds only what is specific to this provider.

## What is fixed here, and why

- **Separator: one ASCII byte** — tab or `0x20..=0x7E`, never `"`. Anything else is a
  contract violation at open (`Error::Separator`). Multi-byte separators are out of scope:
  the scanner's structural classes are single bytes by construction.
- **Quote character: `"` only**, RFC 4180 doubling. No escape character. `quoting: false`
  removes the quote class from the scanner entirely.
- **Quote resolution** (see `src/unescape.rs`): a cell whose first byte is `"` and whose
  only two quotes are its first and last bytes is sliced, zero-copy. A cell that starts
  with `"` and contains more is unescaped once into scratch: `""` → `"`, a lone trailing
  `"` is the closing quote, any other lone `"` is kept literally. A cell that does not
  start with `"` is delivered raw, quotes and all. Nothing is repaired silently — the
  HyperCast door reports `Malformed` at the exact byte.
- **Row terminators**: `\n`, `\r\n`, `\r`. A `\r` that is the last byte of a not-yet-final
  buffer is held back until the next read says whether a `\n` follows.
- **Blank lines** are skipped by default (`skip_blank_lines`), matching csv-core and
  DuckDB. Off, a blank line is a one-cell row — and will fail the column-count check
  unless the file is single-column, which is the only case where delivering it is
  meaningful.
- **Column count** is fixed by the first record (header or not). Disagreement is
  `Error::ColumnCount` with record index, line, and absolute byte offset; it is sticky.
- **UTF-8 BOM** at byte 0 is skipped. Encoding is not otherwise inspected: bytes in, bytes
  out, and the HyperCast doors read UTF-8.
- **Header** (`has_header`) is read at open, exposed as `Header` with exact-bytes ordinal
  lookup, and fixes the column count. It is never trimmed, folded, or de-duplicated.

## Buffer model

- Slice sources are scanned in place; nothing is copied except unescaped cells.
- `Read` sources own one buffer (256 KiB initially). The walker stops at a row boundary
  when the data runs out; the unfinished row is moved to offset 0, more bytes are read
  behind it, and it is scanned again from its start. Because a row start is always a
  clean scanner state (quote parity zero, no pending `\r`), nothing is rebased — the
  cost is re-scanning at most one row per refill.
- A row that does not complete within the buffer doubles it, up to `Reader::MAX_ROW_BYTES`
  (1 GiB), then `Error::RowTooLong`.
- Rows are scanned 1024 at a time into packed `cells`/`rows` arrays that are reused; a
  delivered `Row` borrows the reader and is valid until the next `next_row`.

## The scanner (`src/engine.rs`, `src/scan.rs`)

Per 64-byte block: four compare masks (separator, `\n`, `\r`, `"`), the inclusive prefix
XOR of the quote mask (a carry-less multiply on aarch64 PMULL / x86-64 PCLMULQDQ, a
six-round shift cascade otherwise) XORed with the carried parity, then
`structural = (sep | lf | cr) & !inside_quotes`. Bits are popped in order; every bit ends a
cell, `\n`/`\r` also end the row, and a `\r\n` pair is one ending (the `\n` is cleared from
the mask, or carried across the block edge when the `\r` is bit 63). Each cell records
its quote count from a popcount of the quote mask between its start and end.

Engines, best first: `neon+pmull`, `neon` (aarch64); `avx2+pclmulqdq`, `sse2+pclmulqdq`,
`sse2` (x86-64); `swar` (everywhere). The walker is monomorphised per engine under the
matching `#[target_feature]`; `engine::detect()` picks at run time. The conformance suite
runs every engine the build machine offers. As of 2026-08-28 only the aarch64 engines
have been executed (the machine this was written on); the x86-64 engines are compiled and
covered by the same suite wherever it runs.

## Allocation story

Asserted by `tests/allocation_free.rs` with a counting allocator: once the first scan has
sized the cell/row arrays, delivering rows over a slice allocates nothing — plain cells
and outer-quoted cells alike; cells with `""` inside use the scratch arena, which grows
once and then allocates nothing; a warm `fill_batch` allocates nothing.

## Exports (`src/ffi.rs`)

`hyperdelimited_open_bytes` / `open_path` / `close` / `header_count` / `header_name` /
`column_count` / `read_batch` / `last_error`. Because `hypertabular` and `hypercast` are
linked statically, `libhyperdelimited` also carries HyperCast's 20 `cast_*` exports — a
binding that already has this library loaded can call the scalar doors from it.

## First-wave numbers (2026-08-28)

`cargo bench --bench delimited_benchmarks` on the machine this was written on — linux-arm64
under WSL2, 12 cores, NEON + PMULL — with criterion cut to 10 samples × 4 s, so these are
directional receipts, not final ones. The file is the generated 200 000 × 10 CSV
(~24 MB) described in the bench source.

| Scope | HyperDelimited | `csv` crate | Verdict |
| --- | ---: | ---: | --- |
| scanner only, `neon+pmull` | 2.05 GiB/s | — | |
| scanner only, `neon` (shift cascade) | 2.04 GiB/s | — | PMULL is not the bottleneck |
| scanner only, `swar` | 1.28 GiB/s | — | the portable floor |
| rows delivered, no cell touched | 1.38 GiB/s | 782 MiB/s | **1.8× faster** |
| every cell touched | 1.04 GiB/s | 762 MiB/s | **1.4× faster** |
| every cell cast through a HyperCast door (10 doors: i64, f64, f32, timestamp, uuid, bool, date, 3× text) | 419 MiB/s | — | the payoff scope; no `csv`-side equivalent |

What the numbers say: the two NEON engines tie, so the per-bit walk (pop, push a cell end,
popcount the quotes) is the ceiling, not classification. Sep's tiered fast paths — a
separators-only block walked without the per-bit terminator check — are the obvious next
lever and are deliberately not in this first cut. The typed batch spends its time in the
doors themselves (timestamp and UUID parsing dominate), which is where it should.

## Parked

- Sep's tiered mask fast paths (pure-separator blocks, separator+newline blocks).
- Parallel chunk scanning (Polars' two-state chunk analysis on top of the same masks).
- Multi-byte separators and alternative quote characters.
- A comparison against Sep itself, which is C#-side and waits for the C# binding.
