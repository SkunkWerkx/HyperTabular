# HyperTabular — design record

The decisions behind the three tabular repositories, written down so nothing lives only in
a conversation. Companion to [prior-art.md](prior-art.md), which records what was studied
and where each borrowed idea came from.

## The three repositories

| Repository | Crate | Role |
| --- | --- | --- |
| **HyperTabular** | `hypertabular` (rlib) | The contract every format provider speaks: the format-neutral `Cell`, the caller-declared `Plan` of doors, the cast engine that turns cells into HyperCast verdicts, the column-major `Batch`, and the `#[repr(C)]` shapes the bindings share. No I/O, no format knowledge. |
| **HyperDelimited** | `hyperdelimited` (cdylib + rlib) | CSV/TSV/PSV — any single-byte ASCII separator — as a `hypertabular` provider. The SIMD structural scanner lives here. No spreadsheet dependencies. |
| **HyperWorkbook** | `hyperworkbook` (cdylib + rlib) | XLSX and ODS as `hypertabular` providers. Zip container, inflate, a sheet-XML tokenizer, styles/shared-strings, Excel serial dates. XLS is deliberately parked (see "Parked"). |

HyperUuid is not in this graph; it is the pattern precedent (one Rust core, seven bindings)
and nothing more. HyperCast is the one real upstream dependency: every cell verdict *is* a
HyperCast `Result<T, Fault>` with HyperCast's closed `Reason` set — `Empty`, `Malformed`,
`OutOfRange`. HyperTabular adds no fourth reason; structural failures (a torn CSV row, a
corrupt zip) are a separate error type, never a cell verdict, exactly as Svartalfheim's
`ITabularReader` drew the line ("structural failures throw, bad values are the caller's
verdict").

### Dependency mechanics

- `hypercast` is consumed as a **git dependency** on `SkunkWerkx/HyperCast` (`branch =
  "master"`); Cargo finds the package inside the repo's `rust/` directory on its own.
  `hypercast` is on crates.io (0.2.0 as of 2026-09-02); `hypertabular` is not. The git
  source is kept so the contract tracks the core's master rather than its last tag.
- `hypertabular` is consumed by the two providers as a **path dependency**
  (`../../HyperTabular/rust`). The remote has carried the crate since 2026-08-28, so a git
  source resolves now (it could not while the remote was empty: Cargo resolves the
  original git source even when it is `[patch]`ed to a local path). The path source is
  kept so the providers build against the sibling checkout while all three crates move
  together; the flip is one line in each provider's `Cargo.toml`, noted there. A
  standalone clone of a provider needs its sibling checked out beside it.

## The contract

### `Cell` — what a provider hands the engine

Delimited text only ever has bytes; a workbook has typed cells. One enum covers both, so
the cast engine is written once:

| Variant | Produced by | Meaning |
| --- | --- | --- |
| `Empty` | all | no value (missing cell, blank field, sparse-row gap) |
| `Text(&[u8])` | delimited fields; workbook string cells (shared, inline, formula-string) | UTF-8 bytes, untrimmed — the HyperCast doors trim |
| `Number(f64)` | workbook numeric cells with a non-temporal number format | the IEEE double the file stores |
| `Bool(bool)` | workbook `t="b"` / `office:boolean-value` | |
| `Wall { date, nanos }` | date-formatted Excel serials ≥ 1; ODS `office:date-value`; XLSX `t="d"` | a zoneless wall-clock instant: calendar date + nanoseconds since midnight |
| `Clock(nanos)` | time-formatted Excel serials in `[0, 1)` | a time of day with no date |
| `Span(Duration)` | Excel elapsed formats (`[h]:mm:ss`); ODS `office:time-value` | a signed duration |
| `Error(CellError)` | `t="e"` / BIFF error codes / `calcext:value-type="error"` | `#N/A`, `#DIV/0!`, … |

### `Door` and `Plan` — what the caller declares

A `Door` is one HyperCast door (`Bool`, `I8…I64`, `U8…U64`, `F32`, `F64`, `Uuid`,
`Timestamp`, `Unix(precision)`, `Date`, `Time`, `Duration`) plus `Text`, which asks for the
bytes themselves. HyperCast has since added three doors with no `Door` here yet:
`cast_date_ordered` and `cast_datetime` (a caller-declared `DateOrder`, 2026-08-30) and
`cast_excel_serial` (2026-08-31; see "Parked"). A `Column` is a door, the `NumFormat` its numeric doors use, and the
source ordinal it reads. A `Plan` is the ordered list of columns to produce — a projection,
so a 40-column file can be read into 5 typed columns. Nothing is sniffed: no type
inference, no separator detection, no header heuristics. Culture stays out of the core
exactly as in HyperCast — `NumFormat` is declared per column.

### The cast matrix

Every `(Cell, Door)` pair has one defined outcome. `Empty` is `Reason::Empty` for every
door. `Text` goes through the HyperCast door verbatim (the `Text` door copies bytes; empty
text is `Empty`). The typed cells:

| Cell → Door | integers | reals | Bool | Uuid | Timestamp | Unix | Date | Time | Duration | Text |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `Number(v)` | integral & in range → value; non-integral → Malformed; else OutOfRange | in range → value (f32 overflow → OutOfRange) | 0/1 only, else Malformed | Malformed | serial rules ⇒ UTC | integral v at declared precision | serial rules | serial fraction | v days → seconds | shortest round-trip text |
| `Bool(b)` | Malformed | Malformed | b | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `true`/`false` |
| `Wall{d,n}` | Malformed | Malformed | Malformed | Malformed | d+n read as UTC | as Timestamp | d | n | Malformed | ISO 8601 `yyyy-MM-ddTHH:mm:ss[.fffffffff]` |
| `Clock(n)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | n | n as a span | `HH:mm:ss[.f]` |
| `Span(s)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `0 ≤ s < 24h` → nanos, else OutOfRange | s | ISO 8601 `PT…` |
| `Error(e)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `#N/A` etc. |

"Serial rules" (Excel's two date systems, `hypertabular::serial`):

- 1900 system: epoch 1899-12-30. Serial `60` is the nonexistent 1900-02-29 that Lotus
  1-2-3 invented and Excel keeps → `Malformed`. Serials `1 ≤ s < 60` are shifted one day
  (epoch effectively 1899-12-31) so `1` is 1900-01-01. Serials ≥ 61 use the epoch as-is.
- 1904 system: epoch 1904-01-01, no leap bug.
- Fraction × 86 400 s, kept at nanosecond resolution (not rounded to milliseconds — the
  double carries ~0.1 µs at serial 45 000; rounding is a presentation choice for bindings).
- `s < 0` → `Malformed` (Excel renders `####`). `s ≥ 2 958 466` (past 9999-12-31) →
  `OutOfRange`. A serial in `[0, 1)` is a `Clock`, not a `Wall`: there is no date in it,
  so `Date`/`Timestamp` doors on it are `Malformed`, not a silent 1899-12-30.

Reading `Wall` as UTC is the one interpretive act in the matrix, and it is unavoidable:
neither Excel serials nor ODS date-values carry a zone. It is stated here, and bindings
present the resulting `Timestamp` as their zone-aware instant with that caveat.

### Verdicts and fault spans

A `CellVerdict` is `{ offset: u32, len: u32, reason: u32 }` — HyperCast's `Fault` made
`#[repr(C)]` and given a zero code for success. For a `Text` cell the span indexes the
cell's own bytes, exactly what the HyperCast door reported. For a typed cell the span
covers the whole rendering once the batch has rendered it (a door called directly on a
typed cell returns a zero span — there is no text yet). So a binding can always show
*which bytes* offended: on the failure path (and only there — `Empty` is not recorded)
the batch copies the offending cell's raw text, or the typed cell's canonical rendering,
into its text arena and lists it in a fault table `{ row, column, raw span }`, so the fault
is data even after the provider's buffer has moved on. Success paths copy nothing except
`Text`-door values.

### `Batch` — column-major, crossing-friendly

`Batch` holds, per plan column, a typed value vector (`Vec<i32>`, `Vec<f64>`, `Vec<[u8;16]>`,
`Vec<Timestamp>`, …) and a parallel `Vec<CellVerdict>`; `Text` columns hold `(offset, len)`
spans into one shared byte arena. The batch is reused across fills — vectors are cleared,
never reallocated once warm — and it is the thing that crosses the FFI boundary once per
`max_rows` rows: pointers into it stay valid until the next fill. This is the
"column buffers in, parallel verdict arrays out" shape the HyperCast roadmap committed to.

The first-class Rust surface is both levels: `fill_batch(&mut source, &plan, &mut batch,
max_rows)` for bulk, and `source.next_row()?` → `row.cell(i)` → `column.cast(&cell)` for
row-at-a-time code that wants an ordinary `Result<T, Fault>` per cell.

### `TabularSource` — the provider trait

```rust
pub trait TabularSource {
    type Row<'a>: Row where Self: 'a;
    type Error;
    fn header(&self) -> Option<&Header>;             // declared, never sniffed
    fn date_system(&self) -> DateSystem;             // Excel1900 unless a workbook says 1904
    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Self::Error>;   // forward-only
}
```

`Row` exposes `len()` and `cell(i) -> Cell<'_>`; an ordinal past the row's end is `Empty`
(sparse workbook rows). Providers own their buffers; a `Row` borrows them and dies at the
next `next_row`. Header lookup (`ordinal(name)`) is on `Header`, resolved once by the
caller and reused in the hot loop — Svartalfheim's `Ordinal` idiom.

## HyperDelimited

Design lineage: Sep's mask fast paths and packed col-end arrays; zsv/Polars/simdcsv's
carry-less-multiply quote mask; zsv's zero-copy cell delivery with a quoted flag. The
provider-specific decisions and first-wave numbers are in HyperDelimited's own
`docs/design.md`.

- **Bytes, not chars.** The input is UTF-8 bytes; the separator is one ASCII byte (tab or
  `0x20..=0x7E`, never `"`). Non-ASCII bytes can never collide with a structural byte, so
  no narrowing or saturation step is needed — the one place a byte core is strictly
  simpler than Sep's UTF-16 core.
- **One pass, 64 bytes at a time.** Four compare masks per block — separator, `\n`, `\r`,
  `"` — from NEON, AVX2, SSE2, or a SWAR fallback. Quote parity is a prefix-XOR of the
  quote mask (PMULL on aarch64, PCLMULQDQ on x86-64, six shift-XOR rounds otherwise)
  carried across blocks in one bit. Structural bits are `(sep | lf | cr) & !inside_quotes`;
  a CR immediately followed by LF is one row ending, carried across the block edge in a
  second bit. The walker pops bits with `trailing_zeros` and `x & (x - 1)` and writes cell
  ends into a packed `u32` array with Sep's `[row start, end₀, end₁, …]` layout, plus a
  per-cell quote count from a popcount of the quote mask between ends — that count is the
  whole "does this cell need unescaping" decision.
- **Quoting is parity, and only parity.** A `"` toggles; `""` toggles twice. A cell whose
  first and last bytes are quotes with exactly two quotes is delivered as the inner slice,
  zero-copy. A cell that starts with a quote and holds more is unescaped once, at row end,
  into the reader's scratch arena. Anything else is delivered raw, and the HyperCast door
  will say `Malformed` at the exact byte. `quoting: false` turns the quote class off.
- **Buffer model.** A slice source scans in place with no copy. A `Read` source owns one
  growable buffer; the unfinished row is moved to the front and re-scanned from its start
  (a row start is always a clean scanner state), so nothing is rebased. Rows longer than
  the buffer double it, up to a documented ceiling.
- **Rows.** `\n`, `\r\n`, and `\r` all end a row. A completely empty line is skipped by
  default. The column count is fixed by the first row; a later row that disagrees is a
  structural error carrying the row index, the line number, and the byte offset. EOF
  inside quotes is a structural error. A UTF-8 BOM at byte 0 is skipped.
- **Header.** Declared by the caller (`has_header`), read at open, exposed as `Header`
  with ordinal lookup; never trimmed or unescaped beyond the cell rules above.

## HyperWorkbook

Design lineage: Sylvan's forward-only sheet streaming, deferred `<v>` parsing, `t`-attribute
dispatch and date-kind classification; calamine's parse-the-raw-bytes trick and ODS repeat
handling. The provider-specific decisions and first-wave numbers are in HyperWorkbook's
own `docs/design.md`.

- **Container.** A hand-rolled central-directory zip reader (stored + deflate only, zip64
  aware, local headers trusted only for the data offset). Inflate is `flate2` on the
  `zlib-rs` backend — the fastest *streaming* pure-Rust inflate measured in the survey —
  so a 300 MB `sheet1.xml` streams through a fixed window instead of materialising.
  Sources are `Read + Seek` that can reopen themselves (file by path, cursor by clone), so
  every `Sheet` owns an independent reader and outlives its `Workbook`.
- **XML.** One small pull tokenizer (start/empty/end/text/CDATA, lazy attributes, entity
  decoding on demand, namespace prefixes ignored — `<x:c>` and `<c>` are the same cell)
  over a refillable buffer. It serves the small parts and the big ones alike; no DOM
  anywhere. Comments, PIs, and doctypes are recognised, not assumed absent.
- **XLSX cells.** `r` gives the column (missing `r` ⇒ previous + 1, per ISO 29500), `s`
  the `cellXfs` index, `t` the type. `<v>` is parsed straight from its bytes: `s` →
  shared-string index; `b` → `0`/`1`; `e` → error; `d` → ISO wall-clock; otherwise a
  double, then classified by the style's number-format kind: number, date/time (→
  `Wall`/`Clock`), elapsed (→ `Span`). `<is>` is collected like a shared string; `<f>` is
  skipped — the cached value is the value. Shared strings are preloaded into one byte
  arena with an offset table (rich-text runs concatenated, `rPh` phonetics skipped); that
  preload is the documented allocating boundary.
- **Date-kind classification.** Built-in ids 14–22, 27–36, 45–47, 50–58, 71–81 are
  temporal (46 elapsed); custom codes: first `;` section only, skip `"…"` literals and
  `\`/`_`/`*` escapes, `[h]`/`[m]`/`[s]` ⇒ elapsed, any other `[…]` ignored, then any of
  `y m d h s` ⇒ date/time, else number. This is the intersection of what Sylvan,
  calamine, xlrd, and POI agree on.
- **Rows.** Row `r` gaps and empty `<row>`s become empty rows only if `skip_empty_rows`
  is off (default on: a row with no cells carries nothing to cast, and LibreOffice pads
  sheets with them). Sheets are opened by index or name; hidden sheets are listed with
  their state and opened only on request.
- **ODS.** `mimetype` checked, `content.xml` streamed. Typing comes from
  `office:value-type` and its paired value attribute (`office:value`,
  `office:date-value`, `office:time-value`, `office:boolean-value`) — no style lookup.
  String cells join `text:p` with `\n`, expand `text:s`/`text:tab`/`text:line-break`,
  skip `office:annotation`/`draw:frame`. `number-columns-repeated` on empty cells only
  advances the column; `number-rows-repeated` on empty rows is never materialised.
  `calcext:value-type="error"` is honoured.

## FFI shape (shared, implemented per provider)

The core is crossed once per batch. Each provider exports, over a plain C ABI:

- `open_bytes(ptr, len, …)`, `open_path(ptr, len, …)` → opaque handle via an out-param;
  `close(handle)`.
- `header_count` / `header_name` — the declared header row.
- `read_batch(handle, plan*, plan_len, max_rows, out_batch*, out_columns*) → i64` — rows
  filled, or a negative code (`-1` contract violation, `-2` structural error, `-3` I/O)
  with details retrievable by `last_error(handle, …)`. `out_columns[i]` receives
  `{ values, verdicts }` pointers into the handle's batch, and `out_batch` the row count,
  the text arena, and the fault table — all valid until the next `read_batch`/`close`.

`hypertabular::ffi` owns the `#[repr(C)]` types (`CellVerdict`, `Span`, `FaultRaw`,
`RawColumnSpec`, `RawColumnView`, `RawBatchView`) so both providers — and all seven
bindings — share one layout. Because `hypercast` is linked statically, each provider's
library also carries HyperCast's 20 `cast_*` exports.

## Parked, deliberately

- **XLS (BIFF8).** The read-only record set is small and well documented (see prior-art
  §12); the CONTINUE-aware string cursor is the one tricky piece. It waits on a decision
  that has not been taken.
- **Parallel chunked scanning.** The scanner is sequential by construction (quote parity
  is a global carry). Polars' two-state chunk analysis is the cheapest known way to
  parallelise on top of the same mask code; it is a later round, not a design constraint.
- **Writing.** Out of scope for this version, as stated up front.
- **Collapsing `hypertabular::serial` onto HyperCast's Excel-serial door.** HyperCast
  shipped `cast_excel_serial` on 2026-08-31: serial *text* in, a caller-declared
  `ExcelEpoch`, a `Timestamp` out — the door for a CSV column of serials. The conversion
  here stays separate because it starts from the `f64` the workbook stores and produces a
  `Wall`/`Clock`/`Span`, which are tabular concepts. The two carry the same rules (epoch,
  the phantom serial 60, the fraction as time of day) independently, and nothing yet pins
  them to agree.
