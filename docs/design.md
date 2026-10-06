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
eight: readers that own their buffers, and files. It allocates, in Rust's own idiom,
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
a timestamp, a boolean and `i32`): about 575 MB/s through the core into the caller's
buffers, against 900 MB/s for the `csv` crate splitting records without casting anything. Chunk size makes no difference (560 MB/s at 64 KiB).

### Workbooks in the core

A workbook is read by the same rules (`docs/workbook.md`): the container is bytes the
caller holds, the zip directory is walked in place, a part is inflated through a window
of the caller's and tokenized as it comes out, and a batch of rows is cast through the
caller's plan into the caller's columns — the layout a delimited batch has, so that what
reads one reads the other. The state block is offsets and counters and the inflate
tables; no pointer is kept between calls.

One thing the format forces. A part is a compressed stream, and a read through one cannot
be rolled back to where a call began. So a workbook call that runs out of room does not
start over: it stops where it is, says which buffer is too small and what size would do,
and is made again once the caller has grown that buffer *with its contents kept*. The
readers are written a token at a time to make that exact — a token is looked at, acted
on, and only then stepped past.

The inflate is the core's own, `kernel::inflate`. No inflate that exists can be put under
the proof — the ones in safe Rust index their tables, and the ones behind a C ABI turn a
panic into an abort the proof cannot see — so this one is written for it: a 27 KiB state
block of plain integers, the output buffer doubling as the dictionary, and every step
either completing or leaving the state as it found it, which is what makes it resumable
at any byte of input or output. It is held to zlib's deflate at every level, fed a byte
at a time into a window barely larger than the dictionary, and to thousands of damaged
streams. On sheet XML, linux-x64: 703 MB/s, against 1,051 for zlib-rs with its run-time
SIMD and 599 for miniz_oxide. One thing it taught about the proof: the table builder took
the symbol mapping as a function pointer, and a call through a pointer is one the
compiler must assume can unwind, so the proof refused the whole inflate until the pointer
became a value to match on. Another, from the workbook's casts: converting a double to a
128-bit integer is a call into the compiler's runtime (`__fixdfti`) that the static
library would then have to bring along, so HyperCast's typed doors, which the core casts
a number through, read an integral double's bits instead.

### The Rust API is a binding

`DelimitedReader`, and `Workbook` with its `Sheet`s, are the Rust binding over those
calls, and the model for the other seven. A reader owns everything the core is handed —
the input buffer and the reads that fill it, the column arrays, the cell table, the
arena, a workbook's window and its shared strings — allocated once and reused; it makes
the call, grows the buffer the core names when one is too small, and puts what the core
did not consume back in front of it. And it lends what the core wrote instead of copying
it: `read()` returns a `Batch` that borrows the reader, so a column is the array the core
filled and a text cell is a slice of the input or of the shared strings. Reading the next
batch is what ends the last.

```rust
let plan = [Column::i64(0), Column::f64(2).format(continental), Column::text(3)];

let mut reader = DelimitedReader::open(path, Dialect::CSV, &plan)?;
while let Some(batch) = reader.read()? {
    let ids: &[i64] = batch.i64(0);               // a whole column
    let verdicts = batch.verdicts(0);             // and a verdict beside each value
    let amount: Result<f64, Fault> = batch.get(1, row);   // one cell, as HyperCast judged it
    let name = batch.text(2, row);
}

let book = Workbook::open(path)?;
let mut sheet = book.sheet("Data", SheetOptions::default(), &plan)?;
while let Some(batch) = sheet.read()? { /* the same batch */ }
```

Those are the binding's choices; none of it is in the core, and the binding calls the
core's Rust functions, not the C exports. It may panic where the core may not — asking a
column for another door's type is the caller's bug, and says so.

## The contract

### A workbook cell — what the core reads before it casts

Delimited text only ever has bytes, and the core hands them straight to HyperCast's
doors. A workbook has typed cells: the workbook core reads each into one of these
(`kernel/workbook/cell.rs`), and the cast matrix below says what every door makes of each:

| Variant | Produced by | Meaning |
| --- | --- | --- |
| `Empty` | all | no value (missing cell, sparse-row gap) |
| `Text(&[u8])` | string cells (shared, inline, formula-string) | UTF-8 bytes, untrimmed — the HyperCast doors trim |
| `Number(f64)` | workbook numeric cells with a non-temporal number format | the IEEE double the file stores |
| `Bool(bool)` | workbook `t="b"` / `office:boolean-value` | |
| `Wall { date, nanos }` | date-formatted Excel serials ≥ 1; ODS `office:date-value`; XLSX `t="d"` | a zoneless wall-clock instant: calendar date + nanoseconds since midnight |
| `Clock(nanos)` | time-formatted Excel serials in `[0, 1)` | a time of day with no date |
| `Span(Duration)` | Excel elapsed formats (`[h]:mm:ss`); ODS `office:time-value` | a signed duration |
| `Error(code)` | `t="e"` / `calcext:value-type="error"` | `#N/A`, `#DIV/0!`, …, by BIFF code |

### `Door` and `Column` — what the caller declares

A `Door` is one HyperCast door — all of them: `Bool`, `I8…I64`, `U8…U64`, `F32`, `F64`,
`Decimal`, `Uuid`, `Timestamp`, `Unix(precision)`, `ExcelSerial(epoch)`, `Date`,
`DateOrdered(order)`, `DateTime(order)`, `Time`, `Duration` — plus `Text`, which asks for
the bytes themselves. A door that declares something carries it, numbered as HyperCast's
own exports number it. One enum, in the core, for both layers. A `Column` is a door, the
`NumFormat` its numeric doors use, and the source ordinal it reads — one factory per door
(`Column::i64(0)`, `Column::unix(4, UnixPrecision::Millis)`, …), named as the bindings name
them. A plan is a slice of columns, in the order to produce them — a projection, so a
40-column file can be read into 5 typed columns, and one source column through two doors. Nothing is sniffed: no type
inference, no separator detection, no header heuristics. Culture stays out of the core
exactly as in HyperCast — `NumFormat` is declared per column.

### The cast matrix

Every `(Cell, Door)` pair has one defined outcome. `Empty` is `Reason::Empty` for every
door. `Text` goes through the HyperCast door verbatim (the `Text` door copies bytes; empty
text is `Empty`). The typed cells:

| Cell → Door | integers | reals | Decimal | Bool | Uuid | Timestamp | Unix | Date | Time | Duration | Text |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `Number(v)` | whole & in range → value; fractional → Malformed; else OutOfRange | in range → value (f32 overflow → OutOfRange) | shortest round-trip decimal; past 96 bits or 28 places → OutOfRange | 0/1 only, else Malformed | Malformed | serial rules ⇒ UTC | whole v at declared precision | serial rules | serial fraction | v days → seconds | shortest round-trip text |
| `Bool(b)` | Malformed | Malformed | Malformed | b | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `true`/`false` |
| `Wall{d,n}` | Malformed | Malformed | Malformed | Malformed | Malformed | d+n read as UTC | as Timestamp | d | n | Malformed | ISO 8601 `yyyy-MM-ddTHH:mm:ss[.fffffffff]` |
| `Clock(n)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | n | n as a span | `HH:mm:ss[.f]` |
| `Span(s)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `0 ≤ s < 24h` → nanos, else OutOfRange | s | ISO 8601 `PT…` |
| `Error(e)` | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | Malformed | `#N/A` etc. |

The `Number` row is HyperCast's too: every cell there goes through the typed twin of the
text door (`i32_from_f64`, `decimal_from_f64`, `bool_from_f64`, `unix_from_f64`,
`excel_time`, `excel_duration`, …). All of them read a double as the one number it names,
the shortest decimal that rounds back to it — the digits Excel and LibreOffice write into
the file, whichever of them wrote it — so the stored `2.5` is the decimal `2.5`, the sum
`0.1 + 0.2` is `0.30000000000000004`, and an integer above 2⁵³ is that decimal's digits
(`2^63` is `9223372036854776000`). HyperCast holds each twin to its text door read on that
text.

"Serial rules" are HyperCast's, not a second copy of them: a `Number` cell on a temporal
door goes through `hypercast::excel_serial`, the typed twin of `cast_excel_serial` and the
same code, replayed against the same corpus.

- 1900 system: epoch 1899-12-30. Serial `60` is the nonexistent 1900-02-29 that Lotus
  1-2-3 invented and Excel keeps → `OutOfRange`, the verdict the text `1900-02-29` gets.
  Serials `1 ≤ s < 60` are shifted one day so `1` is 1900-01-01; a serial under `1` names
  no day → `OutOfRange`.
- 1904 system: epoch 1904-01-01, no leap bug; serial `0` is that day.
- Fraction × 86 400 s, snapped (HyperCast): of the times that store as the same double,
  the one with the fewest fractional-second digits. The double resolves ~0.6 µs at serial
  45 000 and ~40 µs at 9999-12-31, so the nearest nanosecond is float noise; snapping
  reads Excel's `23:59:59` on the second and keeps a real sub-millisecond time the double
  can hold.
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
`#[repr(C)]` and given a zero code for success. For a text cell the span indexes the
cell's own bytes, exactly what the HyperCast door reported. For a typed workbook cell
that fails its door the span covers the whole of the cell said as text — its canonical
rendering, which the core writes into the batch's arena for the purpose. So a binding can
always show *which bytes* offended: beside the columns the core fills a cell table, a span
per cell saying where the cell's text is, and `Batch::raw` reads it — for any cell of
delimited text, and for a workbook's text cells and its typed cells that did not cast.
`Empty` has no text and no span.

### `Batch` — column-major, crossing-friendly

Per plan column, an array of the door's own values (`i32`, `f64`, `[u8; 16]`,
`Timestamp`, …) and a parallel array of `CellVerdict`s; a text column holds spans. A cell
that did not cast holds the door's zero. The arrays are the caller's — in Rust, the
reader's — reused from batch to batch, and are the thing that crosses the boundary once
per batch instead of once per cell. This is the "column buffers in, parallel verdict
arrays out" shape the HyperCast roadmap committed to.

The Rust `Batch` is a view of them, the same for delimited text and for a sheet:
`batch.i64(col)` and its ten siblings give a primitive column whole; `batch.verdicts(col)`
its verdicts; `batch.get::<T>(col, row)` one cell as `Result<T, Fault>` in HyperCast's own
types; `batch.text(col, row)` a text cell's bytes; `batch.raw(col, row)` the text a fault
points into; `batch.line(row)` the line or the sheet row the row came from.

### Errors

One type. `Error::Io` is the read; `Error::Separator`, `Error::Plan` and `Error::NoSheet`
are the caller's own declarations, refused before any read; `Error::Structure` is data
that is broken — a `Failure` with a `kind` (one enum over the delimited and the workbook
failures, numbered as the core numbers them), and `record`, `line`, `byte`, `expected`,
`found`. A structural failure is returned once every intact row before it has been
delivered, and again on every read after: forward-only means there is no recovery point.
A cell that does not cast is never an error.

## Delimited text

Design lineage: Sep's mask fast paths and packed col-end arrays; zsv/Polars/simdcsv's
carry-less-multiply quote mask; zsv's zero-copy cell delivery with a quoted flag. The
decisions specific to it are in `docs/delimited.md`.

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
  into the caller's arena. Anything else is delivered raw, and the HyperCast door
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

## Workbooks

Design lineage: Sylvan's forward-only sheet streaming, deferred `<v>` parsing, `t`-attribute
dispatch and date-kind classification; calamine's parse-the-raw-bytes trick and ODS repeat
handling. The decisions specific to it are in `docs/workbook.md`.

- **Container.** A hand-rolled central-directory zip reader over the container's bytes
  (stored + deflate only, zip64 aware, local headers trusted only for the data offset,
  every offset checked against the container's length). Inflate is the core's own, into
  a window of the caller's — so a 300 MB `sheet1.xml` streams through 64 KiB instead of
  materialising, and a deflate bomb is only as large as the window the caller will give.
- **XML.** One small pull tokenizer (start/empty/end/text/CDATA, lazy attributes, entity
  decoding on demand, namespace prefixes ignored — `<x:c>` and `<c>` are the same cell)
  over that window, with no state of its own. It serves the small parts and the big ones
  alike; no DOM anywhere. Comments, PIs, and doctypes are recognised, not assumed absent.
- **XLSX cells.** `r` gives the column (missing `r` ⇒ previous + 1, per ISO 29500), `s`
  the `cellXfs` index, `t` the type. `<v>` is parsed straight from its bytes: `s` →
  shared-string index; `b` → `0`/`1`; `e` → error; `d` → ISO wall-clock; otherwise a
  double, then classified by the style's number-format kind: number, date, time (→
  `Wall`/`Clock`), elapsed (→ `Span`). `<is>` is collected like a shared string; `<f>` is
  skipped — the cached value is the value. Shared strings are loaded once into a buffer
  of the caller's with a span for each (rich-text runs concatenated, `rPh` phonetics
  skipped), and a shared-string cell is a span into it, never a copy.
- **Date-kind classification.** Built-in ids per ISO 29500 §18.8.30: 14–17, 22, 27–31,
  36, 50, 51, 54, 57, 58, 71–74, 77, 81 are dates; 18–21, 32, 33, 45, 47, 75, 76, 78, 80
  times; 46 and 79 (Thai `[h]:mm:ss`) elapsed. 34, 35, 52, 53, 55, 56 are times in
  Chinese and dates in Japanese or Korean — the file does not say which UI language
  wrote it — and are read as times, which invents no date. Custom codes: first `;`
  section only, skip `"…"` literals and `\`/`_`/`*` escapes, `[h]`/`[m]`/`[s]` ⇒
  elapsed, any other `[…]` ignored, then `y`, `d` or a month `m` ⇒ date, `h`, `s` or a
  minutes `m` alone ⇒ time, else number. `m` is minutes after an `h` or before an `s`
  (§18.8.31, "month versus minutes"), the month otherwise. The scan is the intersection
  of what Sylvan, calamine, xlrd, and POI agree on; the date/time split is the spec's.
- **What a date serial is.** A date format's serial is a wall clock; under one day it is
  a time of day in the 1900 system (serial `0` is the `1900-01-00` that never was), and in
  the 1904 system still a date — serial `0` is 1904-01-01. A time format's serial is a
  time of day under one day and a wall clock past it.
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

Fourteen exports, every one declared through the one macro in `kernel/exports.rs`, over
the `#[repr(C)]` shapes in `kernel/abi.rs`:

- `hypertabular_version`
- `hypertabular_delimited_state_size`, `_init`, `_header`, `_fill`, `_unescape`
- `hypertabular_workbook_state_size`, `_open`, `_sheets`, `_strings`, `_styles`, `_sheet`,
  `_header`, `_fill`

Return codes: `0` done; `-1` a contract violation (a caller bug); `-2` a structural
failure, described in the result; `-3`, `-4` and `-5` an arena, a cell table or a window
too small, with the size that would do. For delimited text nothing was consumed and the
call is made again from the same input; for a workbook the read stopped where it was and
the call resumes once the buffer has been grown with its contents kept.

No build of the crate exports anything else, and `check-core.sh` holds the whole crate to
that: the layers above the core are Rust API, not symbols.

### The bindings, side by side

Every binding mirrors the Rust API — one `Batch` from `read()`, a per-cell getter, `line`,
`raw`, `Workbook`/`Sheet` — in its own language's grain. Two things differ on purpose.

**How long a batch lives.** C#, Java, Go and Swift hand out a batch as a view of the
reader's buffers, valid until the next `read()`: no copy per batch, and a language that can
say "valid until" says it. Python, Ruby and PHP copy what the core wrote into the batch
as it is made, so a batch outlives the reader's next read: a garbage-collected dynamic
language has no way to hold a caller to "until the next read", and `list(reader)` is the
first thing anyone writes.

**What a fault's span counts in.** HyperCast's rule, kept here: a span is in the units of
the text you were handed. Where `raw` is bytes — C#, Java, Go, Swift, Python, PHP — the
span is bytes, as the core wrote it. Ruby's `raw` is a String, so its span is in the
characters `String#[]` slices by, as HyperCast's own gem reports it. A helper that turns
`raw` into a string for display (Java's `rawString`) does not move the span.

Each binding also replays `corpus/workbook.json` with every buffer a workbook call works in
starting at one element (C#, Java, Go, Swift, Ruby and PHP behind a test-only switch;
Python reads through the Rust `Sheet`, whose buffers are `Vec`s grown by `resize`), so the
grow-and-keep path above runs mid-part thousands of times against the corpus.

## Parked, deliberately

- **XLS (BIFF8).** The read-only record set is small and well documented (see prior-art
  §12); the CONTINUE-aware string cursor is the one tricky piece. It waits on a decision
  that has not been taken.
- **Parallel chunked scanning.** The scanner is sequential by construction (quote parity
  is a global carry). Polars' two-state chunk analysis is the cheapest known way to
  parallelise on top of the same mask code; it is a later round, not a design constraint.
- **Writing.** Out of scope for this version, as stated up front.
