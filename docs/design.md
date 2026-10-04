# HyperTabular — design record

The decisions behind the tabular layer, written down so nothing lives only in a
conversation. Companion to [prior-art.md](prior-art.md), which records what was studied
and where each borrowed idea came from, and to [delimited.md](delimited.md) and
[workbook.md](workbook.md), the format-specific records.

## One repository, one library

Delimited text and spreadsheets are one crate, `hypertabular`, and one native library,
`libhypertabular`. They began as three repositories — a contract crate and two providers,
each provider a library of its own that linked HyperCast statically — and that shape did
not survive what HyperCast and HyperUuid settled about shipping a Rust core to eight
languages:

- Two provider archives each carried Rust's standard library, and two such archives cannot
  be linked into one program (`duplicate symbol: rust_eh_personality`). Each also carried
  HyperCast's 22 C symbols, so neither linked beside `libhypercast.a` either
  (`multiple definition of cast_bool`). That is every binding that links the core rather
  than loads it: Go, Swift, C# Native AOT.
- The code a binding needs to read a batch — the plan, the verdicts, the column buffers —
  is the same for both formats, and had no package to live in.

HyperCast is the one dependency: every cell verdict *is* a HyperCast verdict with its
closed `Reason` set — `Empty`, `Malformed`, `OutOfRange`. HyperTabular adds no fourth
reason; structural failures (a torn CSV row, a corrupt zip) are a separate thing, never a
cell verdict, exactly as Svartalfheim's `ITabularReader` drew the line ("structural
failures throw, bad values are the caller's verdict"). It is taken with its `exports`
feature off, so the library built here carries none of HyperCast's symbols.

## The core, and what holds it

The crate is two layers, and the line between them is the point of the design.

**The core** (`rust/src/kernel`) is what the native library is made of and what every
binding calls. It reads bytes the caller hands it and writes into buffers the caller hands
it. It has no standard library, allocates nothing, and cannot panic.

**The Rust API** (everything else, behind the default `std` feature) is one binding among
eight: owned batches, a row-at-a-time reader, files. It allocates, in Rust's own idiom,
exactly as the C# binding allocates in C#'s. Nothing in it is in the library.

The three properties of the core are not a discipline. Each is something a build refuses
to get wrong, and `rust/check-core.sh` runs every one of them, on the code and on the
library it becomes:

| Property | What makes it impossible to break quietly |
| --- | --- |
| No standard library | The library is the kernel built with `std` off, where the name `std` does not compile. Checked again for a target that has no standard library to link (`thumbv7em-none-eabi`). |
| No allocation | The crate never declares the `alloc` crate, so `Vec`, `Box` and `String` do not exist to be named, and there is no allocator for anything to call. The proof is on the artifact: the shared library's import list is compared against an allow-list (`memcpy`, `memset`, `memcmp`/`bcmp`, `memmove`, `abort`) and a `malloc` on it fails the check. The static library's one object is held to the same list. And a counting allocator watches a whole input go through the core in the test suite: the count is zero, not "zero once warm". |
| No panic | Every export is declared through one macro (`export!`, `kernel/exports.rs`), which is the only place a symbol can be made and which puts the function under dtolnay's `no-panic`: the link fails, naming the export, if the optimizer leaves any panic path in it. The library's export list is compared whole against the macro's uses, so an export outside the proof is not an export. And the proof is shown a function it has to reject — a canary export that can panic, compiled only for that check — because a proof that has never refused anything has not been shown to check anything. |
| The same code everywhere | No `cfg(feature)` under `kernel` except the macro's two. What the checks say of the shipped library they say of the core in every build that links it. |
| Nothing else linked in | The core's dependency closure is `hypercast` and nothing else, asserted. HyperCast holds itself to the same three properties, float parsing included. |

What follows from "no allocation" is the shape of every call: memory is an argument. The
caller brings the input, a 64-byte state block, the column buffers, and any scratch the
core needs, sized by rules the core states; where a size cannot be known up front the core
says how much it needs and takes nothing until it has it. The binding allocates all of it
once, in whatever its language does best, and the core is crossed once per chunk.

### Delimited text in the core

`kernel::delimited::fill` — one call, a chunk in, columns out:

- **Input.** Any chunk that starts on a row boundary. The call reports the bytes it is
  finished with; streaming is the caller putting the rest at the front of the next chunk.
  An unfinished row at the end of a chunk is left alone, so a row longer than the caller's
  buffer is the caller's to make room for.
- **Two passes over the chunk.** The scanner (`docs/delimited.md`) walks whole rows and
  records where each cell the plan reads begins and ends, in a table the caller supplied.
  Then each plan column is cast in one loop — one door, straight down the table — into that
  column's value and verdict arrays. Column-major casting is what lets the numeric
  notation be resolved once per column and each door's loop stay monomorphic.
- **The cell table is the caller's map.** It locates every cell the plan reads in the
  caller's own input, so the raw text of a cell that failed to cast is a slice the caller
  already holds. There is no fault table and no copy.
- **Text is zero-copy.** A `Text` value is a span into the input. Only a cell with `""`
  inside has to be unescaped, into an arena the caller supplied, and its span is flagged
  as pointing there. A file without escaped quotes never touches the arena.
- **Failures.** A record of the wrong width, or input that ends inside a quoted cell, is
  reported after every intact row before it has been delivered, and is final for that
  state block. A table or an arena too small for one row is not a failure: the call says
  how much one row takes.

Measured on linux-x64 (a 66 MB, million-row file; six columns cast to `i64`, `f64`, text,
a timestamp, a boolean and `i32`): 548 MB/s through the core, against 441 MB/s for the
row-at-a-time reader filling an owned batch, and 974 MB/s for the `csv` crate splitting
records without casting anything. Chunk size makes no difference (538 MB/s at 64 KiB).

### What is not in the core yet

The workbook reader (`docs/workbook.md`) is still the first design: generic over
`std::io::Read`, inflating through `flate2`, with handle-based exports that own their
batches. It lives in the Rust layer and is built only with `std`. Bringing it into the
core means the same treatment — the container read from the caller's bytes, shared strings
and styles in caller buffers sized by a first call, and an inflate of the core's own, since
no third-party one passes the no-panic proof. Until then the cast matrix below describes
that layer: the format-neutral `Cell` exists for the workbook's typed cells, and the
core's delimited path, where every cell is text, calls HyperCast's doors directly.

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

A `Door` is one HyperCast door — all of them: `Bool`, `I8…I64`, `U8…U64`, `F32`, `F64`,
`Decimal`, `Uuid`, `Timestamp`, `Unix(precision)`, `ExcelSerial(epoch)`, `Date`,
`DateOrdered(order)`, `DateTime(order)`, `Time`, `Duration` — plus `Text`, which asks for
the bytes themselves. A door that declares something carries it, numbered as HyperCast's
own exports number it. One enum, in the core, for both layers. A `Column` is a door, the `NumFormat` its numeric doors use, and the
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

"Serial rules" are HyperCast's, not a second copy of them: a `Number` cell on a temporal
door goes through `hypercast::excel_serial`, the typed twin of `cast_excel_serial` and the
same code, replayed against the same corpus.

- 1900 system: epoch 1899-12-30. Serial `60` is the nonexistent 1900-02-29 that Lotus
  1-2-3 invented and Excel keeps → `OutOfRange`, the verdict the text `1900-02-29` gets.
  Serials `1 ≤ s < 60` are shifted one day so `1` is 1900-01-01; a serial under `1` names
  no day → `OutOfRange`.
- 1904 system: epoch 1904-01-01, no leap bug; serial `0` is that day.
- Fraction × 86 400 s, rounded to the nearest nanosecond (not to milliseconds — the double
  resolves ~0.6 µs at serial 45 000; coarser rounding is a presentation choice for
  bindings).
- `s < 0` → `Malformed` (Excel renders `####`). `s ≥ 2 958 466` (past 9999-12-31) →
  `OutOfRange`.
- What stays tabular (`hypertabular::serial`) is what a number *format* declares: a
  date/time-formatted serial under `1` is a `Clock`, not a `Wall` — a time of day with no
  date in it — and an elapsed-formatted serial is a `Span` of days.

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

## FFI shape

The delimited exports are the core's (`kernel/exports.rs`): `hypertabular_version`,
`hypertabular_delimited_state_size`, `_init`, `_header`, `_fill` and `_unescape`, over the
`#[repr(C)]` shapes in `kernel/abi.rs`. Return codes: `0` done; `-1` a contract violation
(a caller bug); `-2` a structural failure, described in the result; `-3` and `-4` an arena
or a cell table too small for one row, with the size one row takes.

The workbook's exports (`workbook/ffi.rs`, with the shapes in `ffi.rs`) are the earlier,
handle-based design — open a handle, `read_batch` into a batch the handle owns — and go
when the workbook moves into the core.

## Parked, deliberately

- **XLS (BIFF8).** The read-only record set is small and well documented (see prior-art
  §12); the CONTINUE-aware string cursor is the one tricky piece. It waits on a decision
  that has not been taken.
- **Parallel chunked scanning.** The scanner is sequential by construction (quote parity
  is a global carry). Polars' two-state chunk analysis is the cheapest known way to
  parallelise on top of the same mask code; it is a later round, not a design constraint.
- **Writing.** Out of scope for this version, as stated up front.
