# HyperWorkbook — design notes

The shared architecture — the `Cell`/`Plan`/`Batch` contract, the cast matrix, the serial
date rules, the FFI shape, and the prior art — is recorded once in HyperTabular's
[`docs/design.md`](https://github.com/SkunkWerkx/HyperTabular/blob/master/docs/design.md)
and [`docs/prior-art.md`](https://github.com/SkunkWerkx/HyperTabular/blob/master/docs/prior-art.md).
This file holds only what is specific to this provider.

## What is fixed here, and why

- **Format detection is by container, not extension.** A zip with `[Content_Types].xml`
  or `xl/workbook.xml` is XLSX; one whose `mimetype` entry says
  `application/vnd.oasis.opendocument.spreadsheet` is ODS; anything else is
  `Error::NotAWorkbook`. Nothing about the *data* is sniffed.
- **Cells reach the engine as what the file stores.** A numeric `<v>` becomes
  `Cell::Number(f64)`; under a temporal number format it becomes `Wall`/`Clock`/`Span`
  by the serial rules; `t="b"` is `Bool`; `t="e"` is `Error`; `t="d"` is an ISO wall
  clock; shared, inline and formula strings are `Text`. A number the serial rules reject
  (negative, serial 60, past 9999) stays a `Number` so the door reports the fault, not
  this layer.
- **Formulas are never evaluated.** `<f>` is skipped; the cached value is the value. A
  formula with no cached value is an empty cell.
- **Date-kind classification** (`xlsx/styles.rs`): built-in ids 14–22, 27–36, 45, 47,
  50–58, 71–81 → date/time; 46 → elapsed; 49 → text. Custom codes: first `;` section,
  `"…"` and `\`/`_`/`*` escapes skipped, `[h]`/`[m]`/`[s]` ⇒ elapsed, any other `[…]`
  ignored, `AM/PM` skipped, then any of `y m d h s` ⇒ date/time.
- **Rows.** `<row r>` gaps and empty `<row>`s are empty rows: skipped by default
  (`skip_empty_rows`), delivered with their sheet row numbers otherwise. A row keeps
  its cells up to the last populated column (`Row::len`); reading past it is `Empty`.
  Missing `r` on a row or cell means "the next one", per ISO 29500. `dimension` is not
  consulted (it is advisory and often stale).
- **Header** (`has_header`) is the first delivered row, rendered through the text door
  (typed cells become their canonical text; empty cells become empty names).
- **Sheets are independent.** A `Sheet` owns its own reader over the container
  (`Source::reopen`: a cursor clones, a file reopens by path) and an `Arc` of the shared
  tables, so it outlives the `Workbook` and several can be open at once. Hidden and
  very-hidden sheets are listed with their state and opened only on request; chart,
  dialog, and macro sheets are not listed (no cells).
- **ODS typing** comes from `office:value-type` and its paired value attribute —
  `office:value`, `office:date-value`, `office:time-value`, `office:boolean-value`,
  `office:string-value` — never from styles. `calcext:value-type="error"` wins over
  `office:value-type="string"` (that is how LibreOffice writes error cells). String cells
  join `text:p` paragraphs with `\n` and expand `text:s`/`text:tab`/`text:line-break`;
  annotations and drawings are skipped. `number-columns-repeated` on an empty cell just
  advances the column; on a value it fills the columns (capped at ODF's 16 384);
  `number-rows-repeated` delivers the row that many times — a repeated *empty* row is one
  parse and (by default) zero deliveries, so LibreOffice's million-row padding costs
  nothing. ODS sheet hidden state lives in styles this reader does not read; it is always
  reported as not hidden.

## Container and streaming

- `zip.rs`: a hand-rolled central-directory reader — EOCD found from the tail, zip64
  locator honoured, entries stored or deflated only, encryption and other methods
  refused. Local headers are read only for the data offset. Names are matched
  ASCII-case-insensitively with any leading `/` ignored (OPC's rules; third-party writers
  root parts inconsistently).
- Inflate is `flate2` on the `zlib-rs` backend, streaming: a sheet part of any size runs
  through a fixed window into the tokenizer's buffer.
- `xml.rs`: one pull tokenizer for every part. Start/empty/end tags with lazy
  attributes, text, CDATA; comments, PIs, doctypes skipped. Tag scanning is quote-aware;
  text scanning is an eight-bytes-at-a-time `find_byte`. Entities are decoded only when
  a `&` is present. The buffer grows to hold the largest single token and no more.
- Open-time work per workbook: `_rels/.rels` → workbook part → its `.rels` → sheets
  (with `state`), `workbookPr@date1904`, `styles.xml` (`numFmts` + `cellXfs`), and the
  shared-string table streamed into one byte arena with an offset table. The
  shared-string preload is the documented allocating boundary: it is proportional to
  the table, not the sheet. For ODS, `content.xml` is streamed once at open to list the
  tables (names only).

## Allocation story

Per row, the parser reuses one cell vector and one text arena; shared strings are
referenced by index into the workbook's arena, never copied. Inline strings, formula
strings, and ODS text are copied into the row arena (they must be — the tokenizer's
buffer moves on). The inflate window and tokenizer buffer are allocated once per sheet.

## Exports (`src/ffi.rs`)

`hyperworkbook_open_bytes` / `open_path` / `close` / `format` / `date_system` /
`sheet_count` / `sheet_name` / `open_sheet` / `header_count` / `header_name` /
`read_batch` / `last_error`. As with HyperDelimited, the statically linked
`hypercast` doors are exported alongside.

## First-wave numbers (2026-08-28)

`cargo bench --bench workbook_benchmarks` on the machine this was written on — linux-arm64
under WSL2, 12 cores — criterion cut to 10 samples × 4 s, so directional receipts. The
files are built in memory by the bench: 100 000 × 8 (integer, double, shared string,
inline string, date serial, time serial, boolean, elapsed serial), ~30 MB of sheet XML
for XLSX and ~83 MB of `content.xml` for ODS. Throughput is over the *uncompressed*
part.

| Scope | XLSX | ODS |
| --- | ---: | ---: |
| inflate + tokenize only (the floor) | 300 MiB/s, 99 ms | 441 MiB/s, 188 ms |
| rows delivered, no cell touched | 126 MiB/s, 236 ms — 424 k rows/s, 3.4 M cells/s | 134 MiB/s, 617 ms — 162 k rows/s |
| every cell cast through a HyperCast door (8 doors) | 122 MiB/s, 245 ms | 129 MiB/s, 643 ms |

What the numbers say: the cast pass is nearly free (a 4 % delta over row delivery — the
typed batch is the right shape); the cell parser costs about as much again as inflate +
tokenize on XLSX, which is the next place to look (attribute scanning per `<c>` is done
three times — `r`, `s`, `t` — and could be one pass). For scale only, not comparison
(different file, different machine): calamine's README claims 1.12 M cells/s on its
186 MB test file.

## Parked

- **XLS (BIFF8).** The read-only record set and the CONTINUE-aware string cursor are
  documented in the prior-art record; it waits on a decision that has not been taken.
- **XLSB.** Not requested; the record ids are in the prior-art record if it ever is.
- **Lazy shared strings** (Sylvan's forward cursor) if a workbook with a huge table and a
  small read ever makes the preload the wrong trade.
- **Cell-level styles beyond the number-format kind**, hidden rows/columns, merged
  ranges: not consulted; this reader delivers values.
