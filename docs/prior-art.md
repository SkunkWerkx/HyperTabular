# Prior art — what was studied, and what was taken

Recorded 2026-08-28 from direct reads of each project's source (not summaries), so every
borrowed idea in [design.md](design.md) has a traceable origin. Read paths only; writing
was out of scope. Where a claim could not be verified from source it says so.

The full raw survey behind the non-Sep/Sylvan sections — every URL, pseudocode, and
"not verified" flag — is kept verbatim in [prior-art-tabular-parsers.md](prior-art-tabular-parsers.md).

## Delimited text

### nietras/Sep (C#) — `src/Sep/`

The model for a span-first, allocation-free reader surface. What was read:
`SepReader.cs`, `SepReader.IO.Sync.cs`, `SepReaderState.cs`, `Internals/SepParser*.cs`,
`Internals/SepParseMask.cs`, `Internals/SepColInfo.cs`, `Internals/SepUnescape.cs`.

- **Parser loop.** Per 32/64 UTF-16 chars: narrow to bytes (saturating, so non-ASCII can
  never collide with a special), four compares (`\n`, `\r`, `"`, separator), OR into a
  special-char mask. Three tiers: mask empty → skip; `separators == special + quoteCount`
  → pure separator walk; `separators|lineEndings == special + quoteCount` → one-row walk;
  else the general per-bit state machine. The `+ quoteCount` addition is the trick that
  spoils both equalities whenever any quote is in flight.
- **Quote state is one integer's parity.** `ParseAnyChar`: inside quotes (`quoteCount &
  1`), separators and newlines are ignored (newlines still bump the line number); a `"`
  increments the count; the count is stored per column in `SepColInfo(ColEnd, QuoteCount)`
  and reset at every col end. `""` is just two toggles. Unescaping is lazy, in place, and
  branch-free (`SepUnescape.UnescapeInPlace`), applied only when `Unescape = true` and the
  cell's count says so; a count of exactly 2 with quotes at both ends is a slice.
- **Buffers.** `char[]` rented from `ArrayPool`, 24 K chars initial (sized to L1d),
  padding zeroed after data so vector loads over-read safely; col ends packed as
  `[rowStart, end₀ … endₙ]` per row in one `int[]`; up to 512 rows per parse call.
  Partial row moved to offset 0 in both arrays before the next fill; the filler reads one
  char fewer than free space and peeks one more after a trailing `\r` so `\r\n` is never
  split; a `\r\r` is parked in `_trailingCarriageReturn`.
- **Deliberately absent.** No comments, no escape char but `""`, no header sniffing (only
  separator auto-detect, which is off when `Sep` is declared), trims only ASCII space,
  no object mapping/reflection. Column count fixed by the first row; disagreement throws.
- **Parallelism.** `ParallelEnumerate` swaps whole parsed batches (chars + col infos)
  into worker states; the SIMD scan itself stays serial.
- **Numbers (README, "Comparison Benchmarks", 0.13.0/.NET 9).** Rows-only 21.3 GB/s on a
  9950X (MB/s counts UTF-16 chars as 2 bytes); EPYC 7763: Row 8.1 GB/s, Cols 5.9 GB/s,
  always-quoted Row 2.9 GB/s; floats 25 000 × 40: 33.8 ms single-thread.
- **Taken:** the parity-only quote model, per-cell quote count as the unescape decision,
  the packed col-end layout, the partial-row slide, the tiered mask fast paths.
- **Not taken:** UTF-16 and the narrowing step (our input is bytes); ArrayPool rentals.

### liquidaty/zsv (C) — `src/zsv_scan_delim_fast.c`, `src/zsv_scan_delim.c`, `src/zsv_internal.c`

- Fast engine: 64-byte blocks, four masks, `fast_prefix_xor` = `_mm_clmulepi64_si128(x,
  ~0)` (six shift-XOR rounds without PCLMUL); `state_mask = inside_quote ? ~B : B`;
  `inside_quote = state_mask >> 63`; `valid_delims = all_delims & ~state_mask`; pop bits
  with `ctz`/`blsr`. Bails to the compat engine on a quote that opens mid-cell.
- Cells delivered zero-copy as `{str, len, quoted}`; common quoted case is `s++; n -= 2`;
  embedded `""` collapsed by `memmove`.
- Single buffer (256 KiB default, ≥ 2× max row), the memmove of the unfinished row is
  **deferred to the next `zsv_parse_more`** so delivered rows stay valid.
- Numbers (`app/benchmark/results/…2026-03-26`, Xeon 8488C, 500 k × 20): fast 5.44 GB/s
  unquoted / 4.56 GB/s quoted; compat 2.04 / 1.03 GB/s, single thread.
- **Taken:** the clmul prefix-XOR with a carried parity bit, zero-copy cell + flag, the
  deferred slide.

### pola-rs/polars — `crates/polars-io/src/csv/read/{parser,splitfields,read_impl}.rs`, `polars-utils/src/clmul.rs`

- `prefix_xorsum_inclusive`: PCLMULQDQ on x86-64, `arm_clmul64` (PMULL) on aarch64 with
  `neon+aes`, shift cascade otherwise — a clean template for the three-way dispatch.
- `CountLines::analyze_chunk` computes newline counts **for both starting states**
  (outside/inside quotes) so chunks can be analysed independently and stitched — the
  cheapest known route to parallel scanning; `accept_line` validates a split point by
  field-counting three consecutive lines.
- Fields are delivered with quotes included plus a `needs_escaping` flag; unescaping is
  deferred to the typed builder; floats via `fast_float2`, ints via `atoi_simd`.
- **Taken:** the dispatch template; the two-state chunk idea is parked for a later round.

### DuckDB — `src/execution/operator/csv_scanner/`

- `CSVState state_machine[256][19]` byte-major table, two-state carry
  (`states[0]=prev, states[1]=cur`), SWAR skip of runs with no structural byte;
  `QUOTED_NEW_LINE` is its own state so quoted newlines are counted, not rows.
- Values are zero-copy `string_t` into the pinned buffer unless escaped.
- Parallel row-start search: assume a state, parse one row, accept if the column count
  matches; prefer the quoted candidate when the previous chunk ended inside quotes.
- Sniffer (dialect × type × header) is a large feature we explicitly do not want — the
  caller declares.
- **Taken:** nothing structural; confirms the "structural bytes only" scalar walk.

### Apache Arrow (C++ `cpp/src/arrow/csv/`, Rust `arrow-csv`)

- Block chunker + parser split; `ParsedValueDesc{offset:31, quoted:1}` descriptor arrays
  and `VisitColumn` bulk conversion per column — the column-major conversion pass.
- Default chunker ignores quotes (`newlines_in_values=false`), a correctness trade we
  don't make. arrow-csv copies through `csv_core` (not zero-copy).
- **Taken:** column-major typed conversion as a separate pass over descriptors.

### BurntSushi/csv-core

- 10-state NFA compiled to a DFA over ≤ 7 byte classes; one lookup + one conditional
  copy per byte; "prefers a parse over no parse", never errors; blank lines skipped at
  record start; `Terminator::CRLF` accepts `\r`, `\n`, `\r\n`; BOM stripped only if the
  first 3 bytes arrive together. Its author notes the class indirection "doesn't make
  much of a difference".
- ~490 MB/s (`byte_records` with a reused record, docs.rs tutorial).
- **Taken:** blank-line skipping as the default; the lenient "text after closing quote is
  literal" stance.

### simdjson lineage — Langdale/Lemire, `geofflangdale/simdcsv`, minio/simdcsv

- Quote mask = carry-less multiply of the quote bitmask by all-ones (prefix XOR), carry =
  sign bit of the previous mask; `field_sep = (end | sep) & ~quote_mask`; CRLF handled by
  `end = lf & ((cr << 1) | prev_cr_end)`. `""` needs no special masking (two toggles
  cancel on a non-delimiter byte). Cheap needs-unescape test: `quotes & (quotes << 1)`.
- **Taken:** all of it — this is the scanner's inner loop.

## Spreadsheets

### MarkPflug/Sylvan.Data.Excel (C#) — `source/Sylvan.Data.Excel/`

- `ZipArchive` container; `_rels/.rels` → workbook part; `workbook.xml.rels` → sheet
  targets (rooted paths stripped) and styles/sharedStrings overrides; `workbook.xml`
  sheets (`name`, `state`, `r:id`) and `workbookPr@date1904`; `styles.xml` `numFmts` +
  `cellXfs → numFmtId` (`xfMap`). Shared strings **lazy**: a persistent forward
  `XmlReader` materialises `si` up to the requested index (`uniqueCount` capped at 128
  for the initial allocation).
- Sheet: `XmlReader` over a 64 KiB `StreamReader`, custom `NameTable` for reference-equal
  element names; pre-`sheetData` scan takes `dimension` (advisory row count) and hidden
  `cols`. Per cell: `r` (column only, ≤ 4 chars), `t` first char dispatch (`n s str b
  inlineStr d e`), `s`; `<v>` text copied into a 64-char-per-column slot and **parsed on
  access**; `<f>` skipped; missing `r` ⇒ previous + 1; `Array.Clear(values)` per row.
- Rows: gaps in `row@r` surfaced as empty rows; empty `<row>` elements skipped under
  `IgnoreEmptyTrailingRows` (any empty row, not just trailing); hidden rows optional.
- Dates: `ExcelFormat.DetermineKind` scans custom codes (`y`/`d` ⇒ Date, `m` before `:`
  ⇒ time, `[h]`/`[m]`/`[s]` ⇒ elapsed, `"…"` and `\x` skipped); built-ins 14–17, 22 Date;
  18–21, 35, 45–47 Time. `TryGetDate`: epoch 1899-12-30 / 1904-01-01, `[60,61)` rejected,
  `[1,60)` +1 day, `< 1` only as time, rounded to milliseconds.
- Accessors: `GetString` renders numbers with the invariant culture and dates as ISO;
  `GetInt32` requires exactness; error cells throw/null/string per option.
- XLSB: varint record framing, `CellRK/Real/Bool/Error/Isst/St` and the `Fmla*` twins;
  RK decode shared with XLS. XLS: own CFB reader, `Workbook`/`Book` stream, BIFF5/8, SST
  eager, CONTINUE-aware string reader that re-reads the compression flag per continuation,
  cell-record-driven row assembly with `PeekRow`.
- Numbers (`MarkPflug/Benchmarks/docs/ExcelReaderBenchmarks.md`, 65 k rows): xlsx 162 ms /
  666 KB vs baseline `XmlReader` walk 112 ms; xlsb 24.9 ms; xls 16.3 ms.
- **Taken:** the part-resolution order, `t` dispatch, deferred `<v>` parsing (we go one
  step further and parse from the raw bytes at row time, like calamine, because our
  consumer is a typed batch), the date-kind scan, the serial rules, sparse/gap handling.
- **Not taken:** DOM for the small parts; millisecond rounding; treating every empty row
  as "trailing".

### tafia/calamine (Rust) — `src/xlsx/`, `src/ods.rs`, `src/xls.rs`, `src/formats.rs`

- `zip 8` + `quick-xml 0.41`; every `worksheet_range` **materialises a dense grid** (the
  only streaming API is `worksheet_cells_reader`); XLS parses every sheet at open.
- XLSX cell loop parses `<v>` from raw bytes (`atoi_simd`, `fast_float2`) and looks the
  style up in a `Vec<CellFormat>` built at open; namespace prefixes ignored.
- Formats: built-in 14–22, 45, 47 date-time; 46 duration; custom-code scan with `[h/m/s]`
  ⇒ duration.
- ODS: `MAX_ROWS/COLUMNS/CELLS` caps, `number-columns-repeated` on empties deferred until a
  later non-empty cell, `text:s@text:c` expanded, annotations skipped, date → ISO string,
  time → ISO duration string.
- README: 186 MB file, 25.3 s vs openpyxl 238.6 s.
- **Taken:** raw-byte `<v>` parsing, prefix-agnostic names, the ODS repeat discipline.
- **Not taken:** dense ranges, string-typed dates.

### Format references

- **XLSX / ISO 29500** (via the Open XML SDK reference pages and MS-OI29500): `c@r`
  optional (⇒ previous column + 1), `t ∈ {b, n, s, str, inlineStr, e, d}`, `d` values are
  ISO 8601, `is` shares the `si` content model (`t | r/t | rPh | phoneticPr`),
  `dimension` "optional and not required", built-in `numFmtId` table (14–22 dates,
  45–47 times, 27–36 / 50–58 / 71–81 locale dates), custom ids ≥ 164, `xml:space=
  "preserve"`. Attribute order and the absence of comments/CDATA inside `sheetData` are
  **not** guaranteed by the spec — hence a real tokenizer, not a regex.
- **ODF 1.3** (OASIS Part 2 packages, Part 3 schema RNG): `mimetype` first and stored;
  `office:value-type ∈ {float, percentage, currency, date, time, boolean, string}` with
  the paired value attribute (`office:value`, `office:date-value` as `xsd:date|dateTime`,
  `office:time-value` as `xsd:duration`, `office:boolean-value`, `office:string-value`);
  no `void` type — an untyped cell is empty; `table:number-columns-repeated` /
  `number-rows-repeated` default 1; LibreOffice pads with huge repeats;
  `calcext:value-type="error"` is a LibreOffice extension.
- **XLS / MS-XLS + MS-CFB** (parked): CFB v3/v4 sectors, `Workbook` stream, BIFF8 record
  set for a value reader — `BOF, FilePass (refuse), Date1904, Format, XF, BoundSheet8,
  SST+Continue, Dimensions, Number, RK, MulRk, LabelSst, Label, BoolErr, Formula+String
  (skip ShrFmla/Array), Blank/MulBlank, Row, EOF`; `XLUnicodeRichExtendedString` may
  change compression at every CONTINUE boundary; RK = `fX100 | fInt | 30-bit payload`.

### Inflate and zip (Rust)

- `zlib-rs` (via `flate2` feature `zlib-rs`): streaming, no_std-capable; 6.9 % faster than
  zlib-ng at 64 KiB chunks and 13 % at 1 KiB (trifectatech.org, 2025-02); large input
  chunks matter.
- `libdeflate`/`libdeflater`, `zune-inflate`: faster but **whole-buffer only** — usable
  for small parts where the central directory gives the size; not for a 300 MB sheet.
- `zip2` reads the central directory and ignores local headers; streaming local-header
  walks are discouraged by both `zip2` and `rc-zip`. We do the same by hand: stored +
  deflate only, zip64 aware.

### Fast XML tokenizing

- `quick-xml`: `memchr2('<','&')` for text, a 3-state quote-aware `memchr3('>','\'','"')`
  scan for tags, lazy attributes, borrowed `Cow` unescape; ~1.27 GB/s tokenizing
  (simdxml's measurement). `cigrainger/simdxml` (2026): simdjson-style structural index
  over `< > / = " '` at 1.43 GB/s — viable if the tokenizer ever becomes the bottleneck.
- pugixml/RapidXML tricks worth keeping in mind: sentinel-terminated buffers, char-class
  tables, in-place entity rewriting.
- **Taken:** a hand-rolled pull tokenizer with the same `memchr`-shaped scans, borrowed
  slices, on-demand entity decoding.
