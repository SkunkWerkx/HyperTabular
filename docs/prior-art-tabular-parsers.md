# Prior-art survey: high-performance read-only tabular parsers

Compiled 2026-08-28 for the HyperCast Rust core (forward-only CSV/TSV/PSV + XLSX/ODS/XLS via C FFI).
Every claim carries the URL it was read from; "not verified" marks what could not be confirmed.
nietras/Sep and MarkPflug/Sylvan.Data.Excel are deliberately excluded (studied separately).

---

## DELIMITED TEXT

### 1. BurntSushi/rust-csv and csv-core

Source: `https://raw.githubusercontent.com/BurntSushi/rust-csv/master/csv-core/src/reader.rs`, `.../csv-core/src/lib.rs`, `.../src/reader.rs`, `.../src/byte_record.rs`.

**NFA states** (reader.rs L417-439): `StartRecord=0, StartField=1, InField=2, InQuotedField=3, InEscapedQuote=4, InDoubleEscapedQuote=5, InComment=6, EndFieldDelim=7, EndRecord=8, CRLF=9`, plus three epsilon-only states not materialized in the DFA (`EndFieldTerm=200, InRecordTerm=201, End=202`). Ordering is load-bearing: `state >= final_field` (=EndFieldDelim) means "field ended", `state > final_field` means "record ended" (L672-677).

**NFA transition** (L980-1063), each returning `(next, action ∈ {Epsilon, CopyToOutput, Discard})`:
```
StartRecord: term(c)→StartRecord/Discard (blank lines skipped) | comment==c→InComment | else StartField/Epsilon
StartField : quoting&&c==quote→InQuotedField/Discard | c==delim→EndFieldDelim/Discard | term(c)→EndFieldTerm/Eps | else InField/Copy
InField    : c==delim→EndFieldDelim/Discard | term(c)→EndFieldTerm/Eps | else InField/Copy
InQuotedField: c==quote→InDoubleEscapedQuote/Discard | escape==c→InEscapedQuote/Discard | else Copy
InEscapedQuote: →InQuotedField/Copy
InDoubleEscapedQuote: double_quote&&c==quote→InQuotedField/Copy ("" -> ")
                    | c==delim→EndFieldDelim | term(c)→EndFieldTerm | else InField/Copy   // lenient: text after closing quote
InComment  : c=='\n'→StartRecord/Discard | else InComment/Discard
InRecordTerm: term.is_crlf()&&c=='\r'→CRLF/Discard | else EndRecord/Discard
CRLF       : c=='\n'→StartRecord/Discard | else StartRecord/Epsilon
```
The parser "prefers a parse over no parse" and "can never return an error" (L533-534). Comments only recognized at record start.

**DFA build with byte equivalence classes** (L762-812): a naive table would be 10×256; instead at most 7 classes: delimiter, quote, escape, comment, `\r`, `\n` (or `Any(b)`), and class 0 = everything else. `TRANS_CLASSES=7, DFA_STATES=10, TRANS_SIZE=70`. `DfaState(u8)` stores `nfa_state * num_classes` so lookup is one add:
```rust
fn get_output(&self, state: DfaState, c: u8) -> (DfaState, bool) {
    let idx = state.0 as usize + self.classes.classes[c as usize] as usize;
    (self.trans[idx], self.has_output[idx])
}
```
The build comment admits the class indirection "doesn't make much of a difference. Perhaps because everything fits into the L1 cache." A fast inner loop skips the table while in `InField`/`InQuotedField` (`scan_and_copy`: copy while class==0).

**Core loop** `read_record_dfa` (L663-684):
```rust
while nin < input.len() && nout < output.len() && nend < ends.len() {
    let (s, has_out) = self.dfa.get_output(state, input[nin]);
    self.line += (input[nin] == b'\n') as u64; state = s;
    if has_out { output[nout] = input[nin]; nout += 1; }
    nin += 1;
    if state >= self.dfa.final_field { ends[nend] = self.output_pos + nout; nend += 1;
        if state > self.dfa.final_field { break; } }
    if state == in_field || state == in_quoted { scan_and_copy(...) }
}
```
One table lookup + one conditional byte copy per byte; no SIMD.

**API** (L263-352): `read_field(input, output) -> (ReadFieldResult, nin, nout)` and `read_record(input, output, ends) -> (ReadRecordResult, nin, nout, nend)`; results `{InputEmpty, OutputFull, [OutputEndsFull], Field{record_end}/Record, End}`. `ends` are cumulative offsets "as if there was a single contiguous buffer" (`output_pos + nout`) so callers can grow output across calls. `End` is only returned when the caller passes an empty input (EOF signal). BOM stripped only if the first 3 bytes are buffered together (L610-619). `Terminator::{CRLF (default: \r, \n, or \r\n), Any(u8)}` (core lib.rs). `line` increments only on `\n`.

**csv crate layer**: `Position { byte: u64, line: u64, record: u64 }` (byte_record.rs L590-595); growth loop in `read_byte_record_impl` (src/reader.rs L1619-1674) doubles `fields`/`ends` on `OutputFull`/`OutputEndsFull`. `ByteRecordInner { pos, fields: Vec<u8>, bounds: Bounds{ends: Vec<usize>, len} }`, boxed; `get(i) = ends[i-1]..ends[i]`. `Trim::{None, Headers, Fields, All}`; `ByteRecord::trim` allocates a new record ("TODO: We could likely do this in place"). `StringRecord` validation: fast path `is_ascii()` on the whole field buffer, else per-field `from_utf8` (L545-558).

**Performance claims**: README has no numbers. docs.rs tutorial (`https://docs.rs/csv/latest/csv/tutorial/index.html`), worldcitiespop.csv 3.17M records / 151 MB: `records()` 0.645 s, `byte_records()` 0.429 s, reused `ByteRecord` 0.308 s (~490 MB/s), raw csv-core 0.572 s field-at-a-time, Serde 0.873-1.381 s.

### 2. liquidaty/zsv

Sources: `https://raw.githubusercontent.com/liquidaty/zsv/main/{README.md, src/zsv_internal.c, src/zsv_scan_delim.c, src/zsv_scan_delim_fast.c, src/vector_delim.c, src/zsv_scan_simd_avx2.h, include/zsv/common.h, include/zsv/api.h, app/benchmark/README.md}`.

Two engines: default "compat" (`zsv_scan_delim.c`, Excel-tolerant, handles `aaa"aaa`) and, since 1.4.0, "fast" (`zsv_scan_delim_fast.c`, RFC-4180-quoting only, returns `zsv_status_nonstandard_csv` and aborts on non-standard input). Dispatch on `scanner->mode` (zsv_internal.c L490-501).

**Compat SIMD**: GCC vector extensions `unsigned char __attribute__((vector_size(VECTOR_BYTES)))` with 16 (SSE2/NEON/wasm), 32 (AVX2), or 64 (AVX-512BW) bytes; `movemask_pseudo` via `pmovmskb`, NEON `vpaddlq` trick, or `wasm_i8x16_bitmask`. `vec_delims` (vector_delim.c L31-59): one load, four compares (delimiter, `\n`, `\r`, `"`), summed, movemask; returns the offset of the first vector with any hit. The main loop (zsv_scan_delim.c L113-263) is a *scalar state machine that only visits special bytes*: pop bits with `NEXT_BIT`/`__builtin_ffs`, clear with `_blsr` or `n&(n-1)`, if-chain on the byte. Quote state is tracked scalar-ly in a 7-bit `scanner->quoted` (`UNCLOSED=1, CLOSED=2, NEEDED=4, EMBEDDED=8, PENDING=16, PENDING_LF=32`) plus `quote_close_position`; `""` sets `skip_next_delim` so the second quote's bit is skipped; a chunk ending on a quote sets `PENDING` resolved at next chunk (L90-105).

**Cell delivery** (`cell_dl`, zsv_internal.c L287-332): quotes removed *in place in the input buffer*. Common case (closing quote at end, no embedded): `s++; n -= 2` — zero-copy. Embedded `""`: O(n) `memmove` per pair. `struct zsv_cell { unsigned char *str; size_t len; unsigned char quoted; }` (common.h L47-72); the `quoted` flags let a CSV writer skip rescanning ("if quoted == 0, the caller need not scan the cell contents").

**Fast engine** (64-byte blocks; avx2.h): `fast_scan_block` → 4 masks (commas, LF, CR, quotes); `fast_prefix_xor` = `_mm_clmulepi64_si128(x, ~0)` under `__PCLMUL__` else 6 shift-XOR rounds (L62-77). Per block (L460-600):
```c
all_delims = commas | newlines | crs;
if (quotes == 0 && !inside_quote) { if (!all_delims) { i += 64; continue; }
    while (all_delims) { bit = ctzll; idx = base+bit; all_delims = blsr(all_delims); comma ? store cell : row end } }
else { B = fast_prefix_xor(quotes); state_mask = inside_quote ? ~B : B;
       inside_quote = (state_mask >> 63) & 1; valid_delims = all_delims & ~state_mask;
       // non-standard check: an opening quote whose previous byte is not delim/CR/LF/quote and not at cell_start -> bail
       ... same ctz pop loop; quoted cells through cell_dl }
```
Cells in the fast path are stored raw *including quotes* ("zero-copy passthrough") unless a cell handler/column filter forces unescaping. A `skip_cells` row-count mode uses `row_ends = valid_cr | (valid_nl & ~((valid_cr<<1) & valid_nl))` to collapse CRLF and popcounts.

**Buffer model** (zsv.c L41-145; zsv_internal.c L679-730): single buffer, default 256 KB, min 4096, raised to ≥ `2*max_row_size`. `zsv_parse_more()` memmoves the unfinished row to the front (deferred to the *next* call so all delivered rows stay valid), rebases every `row.cells[i].str` pointer, reads, scans. Row exceeding buffer → warning, partial row delivered, throwaway handler until next row end.

**API shape**: push (`zsv_new(opts)` with `row_handler/cell_handler/ctx/read/stream`, loop `zsv_parse_more`, `zsv_finish`) or pull (`zsv_next_row`, implemented by saving scan-loop locals in `scanner->pull.regs` and `goto`-ing back into the loop, zsv_scan_delim.c L1-45). `zsv_parse_bytes` push-bytes entry; `zsv_get_cell(p, ix)`, `zsv_cell_count`, `zsv_set_fixed_offsets`, `zsv_set_column_filter` (fast only), `zsv_cum_scanned_length` (for `--parallel` boundary checks).

**Performance claims** (README: "world's fastest (simd) CSV parser"; `app/benchmark/results/benchmark-fast-parser-quoting-linux-x86_64-2026-03-26-1713.md`, Xeon 8488C, 500k rows × 20 cols ~120 MB, best of 5): count, single-thread: compat 2.04 GB/s unquoted / 1.03 GB/s standard-quoted; fast 5.44 / 4.56 GB/s; xan 4.39 / 1.31; xsv 0.126 s vs zsv-fast 0.021 s. `zsv fast` and `xan` marked *incorrect* on nonstandard_quoted. Memory: 1.5 MB RSS vs polars 475 MB (app/benchmark/README.md, M3 Pro, 433 MB file).

### 3. Polars CSV reader

Sources: `https://raw.githubusercontent.com/pola-rs/polars/main/crates/polars-io/src/csv/read/{read_impl.rs, parser.rs, splitfields.rs, builder.rs, utils.rs, schema_inference.rs, streaming.rs, options.rs}`, `crates/polars-utils/src/clmul.rs`. No `buffer.rs` on main; typed buffers live in `builder.rs`.

**Chunking** (read_impl.rs L325-500): `n_threads = RAYON.current_num_threads()`; chunk size:
```rust
const ALLOCATION_BUDGET: usize = 500_000;                 // bounds n_chunks * n_cols
let n_parts_hint = min(n_threads * 16, (ALLOCATION_BUDGET / n_cols).max(n_threads));
let chunk_size = min(bytes.len() / n_parts_hint, 16 * 1024 * 1024);   // "16 MB to still fit L3"
```
Boundaries via `CountLines::find_next(b, &mut chunk_size)` (parser.rs L852-869): counts quote-aware newlines in the first `chunk_size` bytes, returns `(count, last_newline_offset)`; no newline → double the size. Each chunk is a rayon task: optional whole-chunk `simdutf8` validation, `read_chunk`, then `df.height()` is checked against the pre-counted `count` ("CSV malformed: expected {} rows, actual {} rows"). Results sorted by start pointer and concatenated.

The `expected_fields` trick is in `next_line_position` (L270-330), used when a starting offset is arbitrary (e.g. decompressed streams, skip): memchr next eol, then require `accept_line` on that line *and the next two*:
```rust
fn accept_line(line, expected_fields, sep, eol, quote) -> bool {
    let mut count = 0;
    for (field, _) in SplitFields::new(line, sep, quote, eol) {
        if memchr2_iter(sep, eol, field).count() >= expected_fields { return false; }
        count += 1; }
    expected_fields.wrapping_sub(count) <= 1 }
```
After 255 failures `expected_fields -= 1` ("headers might have an extra value").

**CountLines** (L667-984): `analyze_chunk` returns `[LineStats; 2]` — stats assuming chunk starts *outside* vs *inside* a quoted string — so chunks can be analyzed independently and stitched. SIMD (`std::simd u8x64`):
```rust
let eol_mask = bytes.simd_eq(eol).to_bitmask(); let quote_mask = bytes.simd_eq(quote).to_bitmask();
let quote_parity = prefix_xorsum_inclusive(quote_mask) ^ global_quote_parity_mask;
global_quote_parity_mask = ((quote_parity as i64) >> 63) as u64;
states[0].newline_count += (eol_mask & !quote_parity).count_ones();   // states[1] uses & quote_parity
```
`prefix_xorsum_inclusive` (clmul.rs): `intel_clmul64(x, u64::MAX)` under `pclmulqdq`, `arm_clmul64` under `neon+aes`, else 6-round shift cascade. With `comment_prefix` set, SIMD is disabled.

**SplitLines / SplitFields**: `SplitLines::next` caches the remaining valid-eol bits of the last 64-byte lane in `previous_valid_eols` so consecutive lines cost one `trailing_zeros` (L414-665). `SplitFields` yields `(&[u8], needs_escaping: bool)` where the slice *includes* surrounding quotes and unescaping is deferred to the typed builder; quoted-field path: `end_mask = (has_sep|has_eol).to_bitmask() & prefix_xorsum_inclusive(quote_mask)`, remainder cached in `previous_valid_ends`; unquoted path: `(has_sep|has_eol).to_bitmask().trailing_zeros()`, no quote handling at all (a `"` mid-field is literal). Trailing `\r` is stripped per field (`eol_char` defaults to `\n`), so CRLF is handled at field level.

**Typed builders** (builder.rs): `Builder::{Boolean, Int8..Int128, UInt8..128, Float16/32/64, Decimal, Utf8, Datetime, Date, Categorical}`; floats via `fast_float2::parse`, ints via `atoi_simd::parse::<_, true, true>`; strings into `MutableBinaryViewArray` with `escape_field` (utils.rs L125-146, `""`→`"` via a `prev_quote` flag) into a scratch `Vec<u8>`; UTF-8 validated with `simdutf8` (once per chunk, or per field for lossy/ignore_errors). `read_chunk` allocates `capacity + 1` (acknowledged off-by-one). `low_memory` only affects rechunking (reader.rs L146-160) — nothing in read_impl.rs.

**Schema inference**: `infer_schema_length: Some(100)` default; `infer_field_schema` order: quoted → date/time only if `try_parse_dates` else String; `BOOLEAN_RE`; `FLOAT_RE`; `INTEGER_RE` (Int64 else Int128); date patterns; else String. Merge: `{Int64,Float64}`→Float64, `{Int64,Int128}`→Int128, else String (schema_inference.rs L298-403). No published Polars CSV benchmark found in the source or docs — not verified.

### 4. DuckDB CSV reader

Sources: `https://raw.githubusercontent.com/duckdb/duckdb/main/src/include/duckdb/execution/operator/csv_scanner/{csv_state.hpp, csv_state_machine.hpp, csv_state_machine_cache.hpp, base_scanner.hpp, scanner_boundary.hpp, csv_buffer.hpp, csv_reader_options.hpp}`, `.../src/execution/operator/csv_scanner/{state_machine/csv_state_machine_cache.cpp, scanner/string_value_scanner.cpp, scanner/scanner_boundary.cpp, buffer_manager/csv_buffer.cpp, sniffer/*.cpp}`.

**States** (csv_state.hpp), 19: `STANDARD, DELIMITER, DELIMITER_FIRST/SECOND/THIRD_BYTE, RECORD_SEPARATOR, CARRIAGE_RETURN, QUOTED, UNQUOTED, ESCAPE, INVALID, NOT_SET, QUOTED_NEW_LINE, EMPTY_SPACE, COMMENT, STANDARD_NEWLINE, UNQUOTED_ESCAPE, ESCAPED_RETURN, MAYBE_QUOTED`. Multi-byte delimiters (≤4 bytes) get intermediate states; `QUOTED_NEW_LINE` is distinct so quoted newlines are counted, not treated as records.

**Table** (csv_state_machine_cache.hpp): `CSVState state_machine[256][19]` — byte-major, 4,864 bytes of `uint8_t`; plus `bool skip_standard[256], skip_quoted[256], skip_comment[256]`. Cached per `CSVStateMachineOptions{delimiter, quote, escape, comment, new_line, strict_mode}` in a mutex-protected `unordered_map`, pre-populated for the cross product of default dialects. Key overrides in `Insert()`: `T[quote][QUOTED]=UNQUOTED; T[escape][QUOTED]=ESCAPE; T['\n'][QUOTED]=QUOTED_NEW_LINE; T[quote][UNQUOTED]=QUOTED` (so `""` = QUOTED→UNQUOTED→QUOTED, flagged escaped later); `T['\n'][CARRIAGE_RETURN]=RECORD_SEPARATOR`.

**Transition with two-state carry** (csv_state_machine.hpp):
```cpp
struct CSVStates { CSVState states[2]; /* [0]=prev, [1]=cur */ ... };
inline void Transition(CSVStates &s, char c) const { s.states[0] = s.states[1];
    s.states[1] = transition_array[(uint8_t)c][(uint8_t)s.states[1]]; }
```
**Scan loop** `BaseScanner::Process<T>` (base_scanner.hpp): per byte `Transition` then `switch(cur)` dispatching to the result type (`StringValueResult` for parsing, `ColumnCountResult` for sniffing). Fast-skip: an 8-bytes-at-a-time SWAR loop testing `ContainsZeroByte((v ^ delim) & (v ^ newline) ...)` then a byte loop over `skip_standard[]`; table lookup only near structural bytes. Buffer refill outside `Process` in `FinalizeChunkProcess()`.

**Values** (string_value_scanner.cpp): unquoted, unescaped values are **zero-copy** `string_t(ptr, size)` into the pinned buffer. Quoted+escaped → `RemoveEscape` two-pass copy. Parse chunk is all-VARCHAR vectors; `TrySimpleIntegerCast`/`TryDoubleCast` fast paths cast in place, else `VectorOperations::TryCast` per column in `Flush()`. Values spanning two buffers → `ProcessOverBufferValue` copies the tail; only two buffers are ever pinned at once.

**Parallel scanning**: `CSVBoundary{buffer_idx, buffer_pos, boundary_idx, end_pos}`; bytes per thread = `buffer_size / ROWS_PER_BUFFER(16) * ROWS_PER_THREAD(4)`, floored at `maximum_line_size`; buffer = 16 × `max_line_size` (2 MB) = 32 MB, so 8 MB per task (scanner_boundary.hpp, csv_buffer.hpp, csv_reader_options.hpp). Row-start search `SetStart` (string_value_scanner.cpp):
```
best = TryRow(STANDARD_NEWLINE); if quoting: q = TryRow(QUOTED); if none: TryRow(ESCAPE)
TryRow(state): SkipUntilState to next newline reachable from the assumed state, then IsRowValid:
    spawn a LINE_FINDER scanner with result_size=1, parse exactly one row,
    valid iff number_of_rows==1, no errors (except max line size), borked_rows.empty()
pick: prefer quoted candidate if best_row.last_state_quote; else earliest valid; retry otherwise
```
1.2.0 notes claim "around 15%" speedup from the new algorithm (`https://duckdb.org/2025/02/05/announcing-duckdb-120`). Original parallel PR: TPC-H SF1 2.93 s → 0.87 s with 4 threads (`https://github.com/duckdb/duckdb/pull/5194`).

**Buffer manager**: `buffer_size_option = 16 * max_line_size`; docs: "Must be large enough to hold four lines and can significantly impact performance" (`https://duckdb.org/docs/current/data/csv/overview.html`). Buffers pinned/evicted via DuckDB's BufferManager; `Pin()` reloads via `LoadRandomAccess` (seekable) or `Reload` (stream).

**Sniffer** (`https://duckdb.org/2023/10/27/csv-sniffer.html`; dialect_detection.cpp, type_detection.cpp): phases DetectDialect → DetectTypes → RefineTypes → DetectHeader → ReplaceTypes. Candidates: delimiters `{",", "|", ";", "\t"}` × 9 quote/escape pairs × comments `{'\0','#'}` × newline variants; each candidate = one state machine + a `ColumnCountScanner`. Scoring: consistent column count, most columns, least padding; ties prefer candidates that saw real quoted values. Sample 20,480 rows default. Types tried most-specific-first per column, popping on cast failure. NYC taxi 1.72 GB: sniffing 0.11 s of 2.43 s total (~4%). A peer-reviewed "DuckDB CSV Reader" paper: **not verified** (only blog posts found).

**Errors**: `ignore_errors` removes the borked line; `store_rejects` → `reject_scans`/`reject_errors` temp tables; `strict_mode` default true since 1.2.0 (`https://duckdb.org/2025/04/16/duckdb-csv-pollock-benchmark`; Pollock 9.599/10 weighted).

### 5. Apache Arrow CSV readers

**C++** (`https://raw.githubusercontent.com/apache/arrow/main/cpp/src/arrow/csv/{options.h, chunker.cc, lexing_internal.h, parser.h, parser.cc, converter.cc, reader.cc, column_decoder.cc}`, `cpp/src/arrow/util/delimiting.{h,cc}`, `cpp/src/arrow/util/value_parsing.{h,cc}`).

- `ReadOptions{use_threads=true; block_size = 1<<20}` ("determine multi-threading granularity as well as the size of individual record batches"); `ParseOptions{delimiter, quoting, quote_char, double_quote, escaping, escape_char, newlines_in_values=false, ignore_empty_lines, invalid_row_handler}`.
- **Chunker contract** (delimiting.h): `BoundaryFinder::{FindFirst(partial, block), FindLast(block), FindNth(...)}`; `Chunker::Process(block, &whole, &partial)` (FindLast), `ProcessWithPartial(partial, block, &completion, &rest)` (FindFirst; no delimiter → error "straddling object straddles two block boundaries" — an object larger than a block is unsupported), `ProcessFinal`, `ProcessSkip`. **With `newlines_in_values=false` (default), the chunker is a plain `find_last_of("\r\n")` and ignores quotes** — a quoted `\n` would split a block (chunker.cc `MakeChunker`). With it on, a `LexingBoundaryFinder<SpecializedOptions<Quoting,Escaping>>` runs a 6-state `Lexer` (`FIELD_START, IN_FIELD, AT_ESCAPE, IN_QUOTED_FIELD, AT_QUOTED_QUOTE, AT_QUOTED_ESCAPE`) with `ReadLine()` resuming via preserved state.
- **Bulk filter** (lexing_internal.h): `BaseBloomFilter` — each special char sets one bit of a 64-bit word by its low bits; `BloomFilter1B/2B/4B` test 1/2/4 bytes per word; `SSE4.2Filter` uses `_mm_cmpistrc` (disabled if block has NUL); `NeonFilter` `vceq_u8`. `ShouldUseBulkFilter()` samples the first 32 words: `n_skips*4+1 >= n_words`.
- **BlockParser / DataBatch** (parser.h): `struct ParsedValueDesc { uint32_t offset:31; bool quoted:1; }`; `DataBatch{num_cols_, num_rows_, parsed_buffer_ (one contiguous unescaped-values buffer), values_buffers_ (arrays of ParsedValueDesc per parsed chunk), skipped_rows_}`; value i spans `[desc[i].offset, desc[i+1].offset)`; `VisitColumn(col, visitor(data, size, quoted))`. Offset is 31-bit → a block's parsed data ≤ 2 GiB. `ParseLine` is a goto state machine (`FieldStart, InField, InQuotedField, FieldEnd, LineEnd, AbortLine, EmptyLine`); quoting only recognized at field start; `AbortLine` rewinds to line start unless `is_final`. `ParseChunk`: `kTargetChunkSize = 32768` descriptors; rows per chunk `max(32768/num_cols, 512)`. Four `<Quoting,Escaping>` template specializations remove runtime branches.
- **Conversion** (converter.cc): per-column `Converter::Make(type)` → `Convert(parser, col)` calling `VisitColumn` and appending to a builder; null check via a trie over null strings (`IsNull`: quoted values not null unless `quoted_strings_can_be_null`); floats via vendored **fast_float** (`from_chars_advanced`, configurable decimal point); ints hand-rolled `ParseUnsigned` with overflow check; hex `0x` supported. Dictionary converter with `max_cardinality_`.
- **Threading** (reader.cc): async generator pipeline `Buffer → CSVBufferIterator (strips BOM, tracks trailing_cr_) → Serial/ThreadedBlockReader (CSVBlock{partial, completion, buffer, block_index, is_final}) → BlockParsingOperator (one BlockParser per block) → column decoders`. `ThreadedBlockReader` additionally calls `chunker_->Process` so every emitted block is fully delimited; readahead = executor capacity. `TableReader` (parallel, per-column `Insert(block_index, parser)` then `Finish()` → ChunkedArray) vs `StreamingReader` (single-threaded decode, type inferred from first block only; docs: "TableReader will tend to be faster ... makes better use of available cores", `https://arrow.apache.org/docs/cpp/csv.html`). Python docs claim "at least 100 MB/s per core" (`https://arrow.apache.org/docs/python/csv.html`).

**Rust arrow-csv** (`https://raw.githubusercontent.com/apache/arrow-rs/main/arrow-csv/src/reader/{records.rs, mod.rs}`, `arrow-cast/src/parse.rs`): `RecordDecoder` wraps `csv_core::Reader`; `decode(input, to_read)` presizes `offsets` to `to_read*num_columns` and `data` by `AVERAGE_FIELD_SIZE=8` per field, loops `read_record`, handles `OutputFull` by growing, `OutputEndsFull` → "incorrect number of fields" error, `Record` → column-count check (pads if `truncated_rows`). `flush()` validates UTF-8 once for the whole batch and yields `StringRecords` with `get(i)` = unchecked slice at precomputed offsets. So the data is copied (unescaped) by csv_core, not zero-copy. `Decoder{batch_size (default 1024), projection, null_regex}`; `parse()` matches `DataType` → `build_primitive_array::<T: Parser>` (floats `lexical_core::parse`, ints `atoi::FromRadix10SignedChecked`), timestamps via chrono, decimals `parse_decimal`. Single-threaded; no published performance numbers — not verified.

### 6. simdjson-derived SIMD delimiting for CSV

Sources: `https://arxiv.org/abs/1902.08318`, `https://branchfree.org/2019/03/06/code-fragment-finding-quote-pairs-with-carry-less-multiply-pclmulqdq/`, `https://raw.githubusercontent.com/simdjson/simdjson/master/src/generic/stage1/{json_string_scanner.h, json_escape_scanner.h}`, `https://github.com/geofflangdale/simdcsv` + `raw.../master/src/main.cpp`.

**Quote mask via carry-less multiply** (branchfree.org, verbatim):
```c
quote_bits = cmp_mask_against_input(in, '"');            // 64-bit movemask
quote_mask = _mm_cvtsi128_si64(_mm_clmulepi64_si128(_mm_set_epi64x(0, quote_bits), _mm_set1_epi8(0xFF), 0));
quote_mask ^= prev_iter_inside_quote;                    // carry-in
prev_iter_inside_quote = (uint64_t)((int64_t)quote_mask >> 63);   // sign-extend bit 63
```
clmul by all-ones = prefix-XOR: bit i set iff an odd number of quotes precede it. PCLMULQDQ latency 6 cycles / throughput 1 on Skylake, hence buffering to hide latency. simdjson `finish()` reports `UNCLOSED_STRING` if the carry is set at EOF.

**Backslash odd/even run detection** (paper; json_escape_scanner.h single-pass form): `maybe_escaped = potential_escape << 1; even_series_codes_and_odd_bits = (maybe_escaped | 0xAAAA...) - potential_escape; escaped = that ^ 0xAAAA...`, with `next_is_escaped` carry. Only needed for backslash-escape CSV dialects; RFC 4180 has none.

**simdcsv** (main.cpp, verbatim structure):
```c
for (; idx < lenminus64; idx += 64) {
  prefetch(buf+idx+128); in = fill_input(buf+idx);
  quote_mask = find_quote_mask(in, prev_iter_inside_quote);
  sep = cmp(in, ',');
#ifdef CRLF
  cr = cmp(in, '\r'); cr_adjusted = (cr << 1) | prev_iter_cr_end; lf = cmp(in, '\n');
  end = lf & cr_adjusted; prev_iter_cr_end = cr >> 63;
#else
  end = cmp(in, '\n');
#endif
  field_sep = (end | sep) & ~quote_mask;
  flatten_bits(base_ptr, base, idx, field_sep);   // idx + tzcnt, bits &= bits-1; unrolled 8/16 guarded by popcount
}
```
Output is only `ParsedCSV{n_indexes, uint32_t *indexes}` — no state machine, no unescaping, no column validation; a second stage walks the index array. README gives **no throughput number** (harness prints GB/s) — the "~GB/s" claim is not verified for this repo.

**Why `""` works**: two adjacent toggles cancel; only the byte between them is marked "outside", and that byte is the second `"`, not a delimiter, so `(end|sep) & ~quote_mask` is unaffected (`http://chunkofcoal.com/posts/simd-csv/`). Post-processing still needed: strip outer quotes and collapse `""`; minio/simdcsv (Go, `https://github.com/minio/simdcsv`) clears double-quote bits in stage 1 and flags fields needing `ReplaceAll` in stage 2. Cheap needs-unescape test: `quote_bits & (quote_bits << 1) != 0` (inference, not cited).

**Locality**: quote parity is global — the SIMD pass is sequential over 64-byte blocks with three carries (`prev_iter_inside_quote`, `prev_iter_cr_end`, `next_is_escaped`). Cross-chunk parallelism needs DuckDB's guess-and-verify (§4) or Polars' two-state analysis (§3), or speculation as in Ge et al., "Speculative Distributed CSV Data Parsing", SIGMOD 2019 (`https://dl.acm.org/doi/abs/10.1145/3299869.3319898`; details not verified).

**Other verified writeups**: Chris Wellons "Fast CSV processing with SIMD" (`https://nullprogram.com/blog/2021/12/04/`): table state machine ~1 GiB/s, SIMD ~4 GiB/s. minio/simdcsv: stage 1 1498 MB/s, stage 2 1308 MB/s, end-to-end 832 MB/s vs 196 MB/s encoding/csv. Rust `simd-csv` (`https://docs.rs/simd-csv`): *not* clmul — "traditional state machine, but search for structural characters ... using SIMD"; 4-6x vs `csv` crate on unquoted data, ~1.0-1.1x on always-quoted. Lemire has no SIMD-CSV blog post (only 2008 "CSV parsing is CPU bound") — not verified beyond that. No 2023+ academic SIMD-CSV paper found — not verified.

### 7. Other notable CSV readers

- **xsv / qsv** (`https://raw.githubusercontent.com/dathere/qsv/master/{README.md, Cargo.toml, docs/PERFORMANCE.md}`): both are rust-csv consumers; the key trick is the `csv-index` (constant-time row access; qsv: 15 GB NYC 311 indexes in 14 s, "520MB file indexes in 466ms"). qsv patches `csv`, `csv-core`, `csv-index` to a "tuned fork", uses `simdutf8`, jemalloc, 128 KB read / 512 KB write buffers, `-C target-cpu=native`; the `polars` feature enables `sqlp`/`joinp` via polars 0.55.1. No SIMD in the parser itself.
- **Go encoding/csv** (`https://go.dev/src/encoding/csv/reader.go`; issue `https://github.com/golang/go/issues/16791`: 50 MB file Go 1.489 s vs Java 0.493 s): line-oriented `bufio.ReadSlice('\n')`, then per-field `bytes.IndexRune(line, Comma)` / `bytes.IndexByte(line, '"')`; unescaped fields accumulated in `recordBuffer` with `fieldIndexes` (commit `bd06d48` cut allocations from one-string-per-field to one-string-per-record: -40%/-64% allocs). Still one `string` + one `[]string` per record unless `ReuseRecord`; rune-based delimiter; no SIMD.
- **pandas C engine** (`https://raw.githubusercontent.com/pandas-dev/pandas/main/pandas/_libs/src/parser/tokenizer.cpp`, header `.../include/pandas/parser/tokenizer.h` — now `.cpp`): per-byte `switch(self->state)` over `START_RECORD, START_FIELD, ESCAPED_CHAR, IN_FIELD, IN_QUOTED_FIELD, ESCAPE_IN_QUOTED_FIELD, QUOTE_IN_QUOTED_FIELD, EAT_CRNL, EAT_CRNL_NOP, EAT_WHITESPACE, EAT_COMMENT, EAT_LINE_COMMENT, WHITESPACE_LINE, *_IN_SKIP_LINE, FINISHED`; output `stream` bytes + `word_ends`; `PUSH_CHAR`/`END_FIELD`/`END_LINE_STATE` macros. New in 2026: a 16-byte SIMD bulk scan inside `IN_FIELD`/`IN_QUOTED_FIELD` (`fast_scan_simd`, SSE2/NEON) copying normal bytes without state overhead. `low_memory`: `buffer_lines` = largest power of two below `2^20 / table_width`, chunks concatenated (parsers.pyx, c_parser_wrapper.py). pyarrow docs: `read_csv` multithreaded by default, `open_csv` single-threaded with types frozen after the first block (`https://arrow.apache.org/docs/python/csv.html`).
- **Java**: uniVocity is a char-by-char pull parser through `CharInputReader` + `CharAppender` (`AbstractParser.java`), with an optional producer-thread `ConcurrentCharInputReader`. FastCSV (`https://raw.githubusercontent.com/osiegmar/FastCSV/main/lib/src/main/java/de/siegmar/fastcsv/reader/StrictCsvParser.java`): bit-flag status (`DATA_FIELD=1, QUOTED_FIELD=2, QUOTED_MODE=4, NEW_FIELD=8, COMMENTED_RECORD=16, LAST_CHAR_WAS_CR=32, ESCAPED_QUOTE=64`), `char[]` buffer compacted/doubled, locals hoisted in `consume()`, callback `addField(buf, offset, len, isQuoted)` with in-place `cleanDelimiters()` only if `ESCAPED_QUOTE`. Benchmark (`https://github.com/osiegmar/JavaCsvBenchmarkSuite`, M4 Pro, JDK 21): picocsv 17.1M rec/s, FastCSV 13.2M, Univocity 7.4M, Commons CSV 4.1M.
- **ClickHouse** (`https://raw.githubusercontent.com/ClickHouse/ClickHouse/master/src/IO/ReadHelpers.cpp`, `base/base/find_symbols.h`, `src/Processors/Formats/Impl/CSVRowInputFormat.cpp`, `.../ParallelParsingInputFormat.h`, `src/IO/readFloatText.cpp`): `readCSVStringInto` — quoted path is plain `memchr` for the quote char (`""` → `"` by lookahead); unquoted path is a hand-inlined SSE2/NEON scan for `{'\r','\n',delimiter}` with `movemask + countr_zero`, scalar tail; `find_first_symbols<...>` is the generic ≤16-symbol version. **Parallel segmentation** `fileSegmentationEngineCSVImpl` toggles a `quotes` flag (not parity counting): inside quotes `find_first_symbols<'"'>` and treat `""` as staying inside; outside `find_first_symbols<'"','\r','\n'>`; cut at ≥`min_rows` rows and ≥`min_chunk_bytes` (10 MiB literal in Settings.cpp; docstring says 1 MiB — stale). `ParallelParsingInputFormat`: one segmentator thread, `max_threads` parsers, order-preserving circular array of `max_threads+2` units. Floats: `readFloatTextPreciseImpl` uses fast_float for ≤19-char tokens, `__int128` for long pure integers; `readFloatTextFastImpl` is a hand-rolled ≤19-significant-digit path with `shift10` table.

---

## SPREADSHEETS

### 8. tafia/calamine

Sources: `https://raw.githubusercontent.com/tafia/calamine/master/{Cargo.toml, README.md, src/lib.rs, src/datatype.rs, src/formats.rs, src/xlsx/mod.rs, src/xlsx/cells_reader.rs, src/ods.rs, src/xls.rs, src/cfb.rs, src/xlsb/mod.rs, src/xlsb/cells_reader.rs, src/utils.rs}`.

**Deps** (v0.36.1, MSRV 1.88): `zip 8.6` (deflate only), `quick-xml 0.41` (encoding), `encoding_rs`, `codepage`, `byteorder`, `atoi_simd 0.18`, `fast-float2 0.2`, optional `chrono`.

**Architecture**: `trait Reader<RS: Read+Seek> { new, worksheet_range(name) -> Range<Data>, worksheets, worksheet_formula, sheet_names, sheets_metadata, defined_names, vba_project, metadata }`; `ReaderRef::worksheet_range_ref -> Range<DataRef<'a>>`. `Range<T>{start, end, inner: Vec<T>}` is a **dense row-major grid**; `Range::from_sparse(Vec<Cell<T>>)` fills it. **`worksheet_range` is fully materialized** for every format (xlsx/mod.rs L2593-2722: loops `XlsxCellReader::next_cell()` into a Vec then `from_sparse`). The only streaming API is `worksheet_cells_reader(name) -> XlsxCellReader` (L2522) / `XlsbCellsReader`, with `next_cell() -> Cell<DataRef>` and `next_formula()`. XLS and ODS have no lazy reader: XLS parses every sheet at open (`parse_workbook` → `BTreeMap<String, SheetData>`); ODS `read_table` returns a full `Range`. Maintainer on OOM issue #433: "use worksheet_range_ref and stream the data" (`https://github.com/tafia/calamine/issues/433`) — but `worksheet_range_ref` still builds the dense grid.

**Data model** (datatype.rs): `Data{Int(i64), Float(f64), String, Bool, DateTime(ExcelDateTime{value: f64, datetime_type: DateTime|TimeDelta, is_1904}), DateTimeIso(String), DurationIso(String), Error(CellErrorType), Empty}`; `DataRef<'a>` adds `SharedString(&'a str)`. Date conversion: epoch 1899-12-30, `+1462` days if 1904, `if f < 60 { f + 1 }` for the Lotus bug, milliseconds rounded.

**XLSX cell loop** (cells_reader.rs): quick-xml `read_event_into`; `<row r>` sets `row_index`; `</row>` → `row_index += 1; col_index = 0`; `<c r s t>` via `get_attrs!` on `raw_attr()`, position from `r` or fallback `(row_index, col_index)`; `<is>` → `read_string_with_bufs`; `<v>` with `t ∈ {n,s,b,e,None}` parsed straight from raw bytes ("skipping xml10_content() + String"): `s` → `atoi_simd` index into `strings: Vec<String>`; `b` → `v != b"0"`; `e` → error enum; `d` → `DateTimeIso`; numeric → `fast_float2` then `format_excel_f64_ref(n, formats[s], is_1904)`. `<f>` ignored by `next_cell`. Workbook: shared strings pushed as owned `String`s; styles: `numFmts` into a HashMap, then per `cellXfs/xf` push `detect_custom_number_format(code)` or `builtin_format_by_id(id)` into `Vec<CellFormat>`; `date1904` from `workbookPr`. Namespace prefixes ignored (`local_name()`), so `<x:c>` works.

**Format detection** (formats.rs, verbatim): `builtin_format_by_id`: 14-22, 45, 47 → DateTime; 46 → TimeDelta. `detect_custom_number_format`: scans chars; `_ \ *` escape next; `"` toggles literal; `;` → Other (first section only); `[`…`]` containing h/m/s → TimeDelta; `a`/`A` sets AM/PM flag; any `d m h y s` (outside brackets/quotes) → DateTime.

**ODS** (ods.rs): opens `mimetype`, `META-INF/manifest.xml` (encryption check), streams `content.xml`; `MAX_ROWS=1_048_576, MAX_COLUMNS=16_384, MAX_CELLS=100_000_000`; `number-rows-repeated` capped by remaining rows; `number-columns-repeated` on empty cells is *deferred* (`empty_col_repeats`) and only expanded if a later non-empty cell appears; `office:value-type`/`office:value` → Float via fast_float2, boolean, date → `DateTimeIso`, time → `DurationIso`, string from `text:p` joined by `\n`, `text:s text:c=N` expanded, `office:annotation` skipped; `get_range` trims empty margins.

**XLS** (cfb.rs, xls.rs): CFB parse (signature, sector shift, DIFAT→FAT chains, mini-FAT for streams < 4096, 128-byte UTF-16LE dir entries). `RecordIter` eagerly gathers following `CONTINUE (0x003C)` records into `Record{typ, data, cont: Vec<&[u8]>}`. Globals handled: `0x002F FilePass → Err(Password)`, `0x0042 CodePage`, `0x0022 DateMode`, `0x041E Format`, `0x00E0 XF`, `0x0085 BoundSheet8`, `0x0809 BOF`, `0x0018 Lbl`, `0x0017 ExternSheet`, `0x00FC SST`, `0x00EB MsoDrawingGroup`, `0x000A EOF`. Sheet: `0x0200 Dimensions, 0x0203 Number, 0x0204 Label, 0x00D6 RString, 0x0205 BoolErr, 0x0207 String, 0x027E Rk, 0x00FD LabelSst, 0x00BD MulRk, 0x00E5 MergeCells, 0x0006 Formula, 0x000A EOF`. **Not handled**: `0x0201 Blank` commented out, `0x00BE MulBlank` absent. RK decode (`rk_num`): `d100 = rk[2]&1; is_int = rk[2]&2; v[4..]=rk[2..]; v[4] &= 0xFC; is_int ? (i32>>2) : f64::from_bits(...)`, `/100` if d100. SST: `read_rich_extended_string` reads `cch, flags(fHighByte=1, fExtSt=4, fRichSt=8), cRun, cbExtRst`, then `read_dbcs` which at each CONTINUE boundary re-reads byte 0 as the new `high_byte` flag, then skips `cRun*4` and `cbExtRst`.

**XLSB** (xlsb/cells_reader.rs): varint record type/length; lazy `next_cell` handles `0x0000 BrtRowHdr, 0x0002 BrtCellRk, 0x0003 BrtCellError, 0x0004|0x000A Bool, 0x0005|0x0009 Real, 0x0006|0x0008 St, 0x0007 BrtCellIsst → SharedString, 0x0092 BrtEndSheetData`; `0x0001 BrtCellBlank` skipped.

**Performance claims** (README): 186 MB file, calamine 25.3 s (1.12M cells/s) vs openpyxl 238.6 s, "9.43x faster". Third-party: python-calamine 3.58 s vs pandas 32.98 s on a 25 MB / 500k-row xlsx (`https://hakibenita.com/fast-excel-python`).

### 9. ToucanToco/fastexcel

Sources: `https://raw.githubusercontent.com/ToucanToco/fastexcel/main/{README.md, Cargo.toml, src/lib.rs, src/types/excelreader/mod.rs, src/types/excelreader/python.rs, src/data/python.rs, src/types/dtype/mod.rs, python/fastexcel/__init__.py}`, `https://fastexcel.toucantoco.dev/v0.21.0/fastexcel.html`.

PyO3 (`abi3-py310`, `gil_used=false`) over `calamine ^0.36.1` + `arrow-array ^59`, optional `polars-core`; Arrow PyCapsule interface for zero-copy to Polars. `load_sheet` calls calamine `worksheet_range` (mod.rs L312) — fully materialized; `eager=True` on XLSX uses `worksheet_range_ref` under `py.detach` (GIL released) and is documented as "faster and more memory-efficient" (docs). **"Lazy" means deferred Arrow conversion, not row streaming**; no chunked reader exists in the tree. `Range → RecordBatch` (data/python.rs): column-wise builders iterating rows via `RowSelector::Range(offset..limit)`: `Int64Array::from_iter((offset..limit).map(|row| extract_int(data.get((row,col)))))`, likewise Float64/Boolean/String/Date32/TimestampMillisecond/DurationMillisecond/Null. Dtype inference (dtype/mod.rs): `HashSet` of cell dtypes over `schema_sample_rows` (default all rows); `{Int,Bool}`→Int, `{Int,Float,Bool}`→Float, mixed with String/dates→String; `dtype_coercion="strict"` errors on mixes. Options: `use_columns`, `n_rows`, `skip_rows`, `header_row`, `column_names`, `dtypes`. No benchmark numbers in README/docs — not verified beyond qualitative claims.

### 10. ODS format specifics

Sources: ODF 1.3 Part 2 `https://docs.oasis-open.org/office/OpenDocument/v1.3/os/part2-packages/OpenDocument-v1.3-os-part2-packages.html`; Part 3 `.../part3-schema/OpenDocument-v1.3-os-part3-schema.html`; normative RNG `https://docs.oasis-open.org/office/OpenDocument/v1.3/os/schemas/OpenDocument-v1.3-schema.rng`; datypic schema renderings (`http://www.datypic.com/sc/odf/e-table_table-cell.html` etc.).

**Container** (Part 2): entries STORED or DEFLATE only (§2.2.1); "The 'mimetype' file shall be the first file of the zip file. It shall not be compressed, and it shall not use an 'extra field'" (§3.3) → MIME string sits at byte 38 (IANA magic `0:PK,30:mimetype,38:vnd.oasis.opendocument.spreadsheet`); `META-INF/manifest.xml` required with `manifest:file-entry full-path media-type` (§3.2); encrypted entries carry `manifest:encryption-data` and are deflated-then-encrypted; manifest never encrypted (§3.4.1). A reader needs only `content.xml` (typed values are attributes; `styles.xml` only for format names).

**content.xml**: `office:document-content > office:body > office:spreadsheet > table:table[@table:name, @table:style-name, @table:print]`; children in order: `table-source?, dde-source?, scenario?, forms?, shapes?`, column defs (`table:table-column[@number-columns-repeated]`, `table-column-group`, `table-header-columns`, `table-columns`), row defs (`table:table-header-rows`, `table:table-rows`, `table:table-row-group` (recursive), `table:table-row`); `text:soft-page-break` may appear among rows. `table:table-row[@table:number-rows-repeated (default 1), @style-name, @visibility] > (table:table-cell | table:covered-table-cell)+`. `table:table-cell` attrs: `table:number-columns-repeated` (default 1), `number-columns-spanned`, `number-rows-spanned`, `table:formula`, `table:style-name`, `office:value-type`, `office:value`, `office:currency`, `office:date-value`, `office:time-value`, `office:boolean-value`, `office:string-value`; children `office:annotation?`, `draw:frame*`, `text:p*`, `table:cell-range-source`, `table:detective`. `covered-table-cell` has the same value attrs (merge placeholder).

**Normative value/type pairing** (RNG `common-value-and-type-attlist`): `float` → `office:value` (double, required); `percentage` → `office:value` (raw fraction, 0.25 not 25); `currency` → `office:value` + optional `office:currency`; `date` → `office:date-value` (`xsd:date | xsd:dateTime`, e.g. `2024-01-31` or `2024-01-31T10:00:00`); `time` → `office:time-value` (`xsd:duration`, e.g. `PT13H45M00S`); `boolean` → `office:boolean-value` (`true|false`); `string` → optional `office:string-value` else text from `text:p`. **`void` is not a cell value-type** in the 1.2 or 1.3 RNG (zero occurrences in the attlist) — a cell without `office:value-type` is empty. Part 3 section numbers: 19.385 value-type, 19.384 value, 19.371 date-value, 19.382 time-value, 19.369 boolean-value, 19.370 currency, 19.383 string-value; prose bodies could not be fetched (4 MB HTML truncated) — RNG is the normative equivalent.

**Cell text**: each `text:p` is a line (join with `\n`); inside: `text:span`, `text:a`, `text:s[@text:c=N]` (N spaces, default 1), `text:tab`, `text:line-break`. `office:annotation` and `draw:frame` must be skipped when collecting text.

**Repeated padding**: LibreOffice pads the trailing row/column with huge repeat counts (secondary evidence: `https://github.com/PHPOffice/PhpSpreadsheet/pull/1289` "saves files with metadata for 1025 columns"; primary content.xml sample showing `number-columns-repeated="16384"` not fetched — not verified). Rule: never materialize repeated *empty* rows/cells; advance counters; drop trailing padding (calamine's approach).

**LibreOffice extension**: `calcext:value-type` (ns `urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0`) duplicates `office:value-type` and adds `error` (SheetJS issue `https://git.sheetjs.com/sheetjs/sheetjs/issues/548`); prefer `office:value-type`, fall back to `calcext` for error cells.

### 11. XLSX format specifics

Sources: Open XML SDK pages reproducing ISO 29500 text (`https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.spreadsheet.{cell, cellvalue, cellvalues, cellformula, inlinestring, row, sheetdimension, sharedstringtable, sharedstringitem, phoneticrun, text, cellformats, cellstyleformats, cellformat, numberingformat, numberingformats, workbookproperties, sheet, sheetstatevalues}`), MS-OI29500 (`https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/...`), calamine, xlrd `formatting.py`, POI `DateUtil.java`/`BuiltinFormats.java`.

**Package**: `[Content_Types].xml`; `xl/workbook.xml` → `sheets/sheet[@name, @sheetId, @r:id, @state=visible|hidden|veryHidden]`; `xl/_rels/workbook.xml.rels` maps `r:id` → target (relative to `xl/` unless leading `/`). `workbookPr@date1904` ("1"/"true" = 1904 system; default false). Namespaces: transitional `http://schemas.openxmlformats.org/spreadsheetml/2006/main`, strict `http://purl.oclc.org/ooxml/spreadsheetml/main`; the SDK loads strict by mapping purl → schemas.openxmlformats; matching on local names sidesteps both (calamine does).

**Sheet part**: `worksheet > sheetPr? dimension? sheetViews? sheetFormatPr? cols? sheetData > row* > c*`. `dimension@ref` "is optional and is not required" (§18.3.1.35) — streaming writers omit it; may be stale. `row[@r, @spans, @s, @customFormat, @ht, @hidden, @outlineLevel, ...]`; ascending `r` ordering: convention, ECMA text not fetched — not verified normatively; missing `r` → next sequential row (SC34 WG4 DR 16-0007). `c[@r, @s, @t, @cm, @vm, @ph] > f?, v?, is?`. **`r` is optional**: MS-OI29500 §2.1.590 c: "If this attribute is not specified, the cell shall be located in the column with the index that is 1 greater than that of the previous cell ... first `c` ... in the first column." `s` is a 0-based index into `cellXfs`. `f[@t=normal|array|dataTable|shared, @ref, @si]` (non-master shared cells have empty `<f t="shared" si="0"/>`); a value reader ignores `f` and uses `v`. `is` has the CT_Rst content model (`t | r(rPr?, t) | rPh | phoneticPr`). `t@xml:space="preserve"` must preserve whitespace.

**Cell types** (ST_CellType §18.18.11 via `CellValues`): `b` (v = 0/1), `n` default (v = xsd:double lexical incl. `1E+15`; Excel stores IEEE doubles, `https://learn.microsoft.com/office/client-developer/excel/excel-worksheet-and-expression-evaluation`; "17 sig digits" claim not verified), `s` (0-based SST index), `str` (formula cached string), `inlineStr` (value in `is`), `e` (v ∈ `#NULL!, #DIV/0!, #VALUE!, #REF!, #NAME?, #NUM!, #N/A, #GETTING_DATA`), `d` (ISO 8601; example `<c r="C4" t="d"><v>1976-11-22T08:30Z</v></c>` in §18.3.1.96; "Office 2010 and later"; Excel reads but does not write — not verified).

**Shared strings**: `sst[@count, @uniqueCount] > si > (t | r+(rPr?, t) | rPh | phoneticPr)`; value = concat(direct `t`, each `r/t`), **skip `rPh`** (phonetic furigana) and `phoneticPr`.

**Styles**: `styleSheet > numFmts > numFmt[@numFmtId, @formatCode]`; `cellXfs > xf[@numFmtId, @fontId, @fillId, @borderId, @xfId, @applyNumberFormat]` (cell `s` indexes this, 0-based); `cellStyleXfs` separate. Custom ids ≥ 164. Built-in table (§18.8.30): 0 `General`, 1 `0`, 2 `0.00`, 3 `#,##0`, 4 `#,##0.00`, 9 `0%`, 10 `0.00%`, 11 `0.00E+00`, 12 `# ?/?`, 13 `# ??/??`, **14 `mm-dd-yy`, 15 `d-mmm-yy`, 16 `d-mmm`, 17 `mmm-yy`, 18 `h:mm AM/PM`, 19 `h:mm:ss AM/PM`, 20 `h:mm`, 21 `h:mm:ss`, 22 `m/d/yy h:mm`**, 37-40 accounting `#,##0 ;(#,##0)` variants, **45 `mm:ss`, 46 `[h]:mm:ss`, 47 `mmss.0`**, 48 `##0.0E+0`, 49 `@`. Ids 5-8, 23-26, 41-44 are currency/locale and "shall follow the formatCode attribute" (writer must spell them out; POI's actual strings in BuiltinFormats.java). Locale ids 27-36 and 50-58 are zh/ja/ko date formats; 71-81 Thai dates. Date classification by readers: xlrd `(14,22),(27,36),(45,47),(50,58),(71,81)`; POI `0x0e-0x16, 0x2d-0x2f`; calamine 14-22,45,47 + 46 = duration.

**Custom format date-ness** (ISO 29500 §18.8.31 text on `numberingformats` page; xlrd `is_date_format_string`; POI `isADateFormat`; calamine): up to four `;` sections (positive;negative;zero;text) — take the first; strip `"literal"`, `\x`, `_x`, `*x`; `[...]` blocks are colors/conditions/locale (`[$-409]`) *except* `[h]`, `[m]`, `[s]` = elapsed durations; `m` is minutes only adjacent to `h`/`ss` (only affects rendering, not date-ness); then any of `y m d h s` ⇒ date/time; `General`, `@`, `0.00E+00` never. xlrd additionally weights date chars ×5 against `0#?` digit chars.

**Serials**: 1900 system serial 1 = 1900-01-01; 1904 system serial 0 = 1904-01-01 (MS-XLS Date1904). Lotus bug: Excel treats 1900 as leap (`https://learn.microsoft.com/troubleshoot/microsoft-365-apps/excel/wrongly-assumes-1900-is-leap-year`), so "days since 1899-12-30" is exact only for serial ≥ 61; for 1 ≤ serial < 60 the epoch is 1899-12-31 (xlrd `xldate.py`). Fraction × 86400 = seconds. Strict conformance implies 1900 system without compatibility (MS-OI29500 §2.1.585).

### 12. XLS BIFF8 read-only

Sources: MS-XLS (`https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-xls/...`, individual record pages), MS-CFB (`.../ms-cfb/05060311-...`, `.../60fe8611-...`), ExcelDataReader (`https://raw.githubusercontent.com/ExcelDataReader/ExcelDataReader/develop/src/ExcelDataReader/Core/BinaryFormat/{XlsBiffStream.cs, XlsSSTReader.cs, XlsBiffRKCell.cs, XlsWorksheet.cs}`), xlrd (`.../xlrd/{book.py, sheet.py, formatting.py, xldate.py}`), POI (`BOFRecord.java, LabelRecord.java`).

**CFB**: 512-byte header, signature `D0 CF 11 E0 A1 B1 1A E1`, major version 3 (512 B sectors, shift 9) or 4 (4096 B, shift 12; header zero-padded to 4096), mini sector shift 6 (64 B), Mini Stream Cutoff 0x1000 (streams < 4096 B live in the mini stream), FAT/DIFAT (109 entries in header), first directory sector. Directory entry 128 B: UTF-16 name (64 B, ≤32 code points), type (1 storage, 2 stream, 5 root), left/right/child (red-black tree, NOSTREAM=0xFFFFFFFF), start sector, 8-byte size (v3 readers ignore upper 32 bits); root's start/size = the mini stream. Stream name MUST be `Workbook` for BIFF8; readers fall back to `Book` (BIFF5/7) and accept a raw BIFF stream without CFB (xlrd, ExcelDataReader).

**Record stream**: `type u16, size u16 (≤ 8224), data`; overflow in `Continue (0x003C)` records. Globals substream then per-sheet substreams. `BOF (0x0809)`: `vers` 0x0600 = BIFF8 (0x0500 BIFF5/7; sids 0x0409/0x0209/0x0009 for BIFF4/3/2); `dt` 0x0005 globals, 0x0010 worksheet *or dialog* (WsBool.fDialog), 0x0020 chart, 0x0040 macro.

**Globals to handle**: `FilePass (0x002F)` → encrypted (XOR obfuscation or RC4/RC4-CryptoAPI; record headers, BOF, BoundSheet8 never encrypted; default password "VelvetSweatshop") — refuse; `CodePage (0x0042)` (BIFF5 byte strings only; 1200 = Unicode); `Date1904 (0x0022)` `f1904DateSystem`; `Format (0x041E)` `ifmt u16 + XLUnicodeString` (custom ids 0xA4-0x188; built-ins are the ECMA §18.8.30 table); `XF (0x00E0)` `ifnt u16, ifmt u16, flags (fStyle bit), ixfParent`; first 16 XFs are built-in style XFs, index 15 = default cell XF, cell `ixfe` MUST be ≥15 or 0; `BoundSheet8 (0x0085)` `lbPlyPos u32 (absolute stream offset of sheet BOF), hsState 2 bits, dt 8 bits (0 sheet, 1 macro, 2 chart, 6 VBA), stName ShortXLUnicodeString (cch u8, flags u8)`; `SST (0x00FC)` `cstTotal u32, cstUnique u32, rgb[]` + `*Continue`; `ExtSST (0x00FF)` ignore; `EOF (0x000A)`.

**XLUnicodeRichExtendedString**: `cch u16; flags u8 {bit0 fHighByte: 0 = 1 byte/char Latin-1-compressed, 1 = UTF-16LE; bit2 fExtSt; bit3 fRichSt}; cRun u16 iff fRichSt; cbExtRst i32 iff fExtSt; rgb (cch or 2*cch bytes); rgRun (cRun × 4); ExtRst (cbExtRst)`. Continuation rule (`.../173d9f51-...`): non-variable fields must be in one record; "A value from the table for fHighByte MUST be specified in the first byte of the continue field of the Continue record" — encoding may change at every CONTINUE boundary; double-byte strings break on character boundaries. xlrd `unpack_SST_table` and ExcelDataReader `XlsSSTReader` (states `StartStringHeader → StringHeader → StringData → StringTail`) both re-read the flags byte at each CONTINUE while in string data and also cross records while skipping run/ext bytes.

**Worksheet records**: `Cell` prefix `rw u16, col u16, ixfe u16`. `Dimensions (0x0200)` `rwMic u32, rwMac u32 (last+1), colMic u16, colMac u16` (advisory); `Row (0x0208)` (hidden flag `fDyZero`, optional); `Blank (0x0201)`; `MulBlank (0x00BE)` `rw, colFirst, rgixfe[], colLast`; `Number (0x0203)` `cell + f64` (no inf/NaN/denormal/-0); `RK (0x027E)` `rw, col, ixfe, RK u32`; `MulRk (0x00BD)` `rw, colFirst, {ixfe u16, RK u32}[], colLast`; `LabelSst (0x00FD)` `cell + isst u32`; `Label (0x0204)` `cell + XLUnicodeString` (BIFF5/7 inline strings; BIFF8 spec only for chart labels); `BoolErr (0x0205)` `cell + bBoolErr u8 + fError u8` (errors 0x00 `#NULL!`, 0x07 `#DIV/0!`, 0x0F `#VALUE!`, 0x17 `#REF!`, 0x1D `#NAME?`, 0x24 `#NUM!`, 0x2A `#N/A`, 0x2B `#GETTING_DATA`); `Formula (0x0006)` `cell + FormulaValue(8) + flags u16 + chn u32 + parsed formula` — if last two bytes ≠ 0xFFFF the 8 bytes are a double, else byte0: 0 = string in the following `String (0x0207)` record (readers must skip an intervening `ShrFmla 0x04BC`/`Array 0x0221`), 1 = bool (byte2), 2 = error (byte2), 3 = empty string; `EOF (0x000A)`.

**RK** (`.../04fa5340-...`): bit0 `fX100`, bit1 `fInt`:
```rust
fn rk_to_f64(rk: u32) -> f64 {
    let mut v = if rk & 2 != 0 { ((rk as i32) >> 2) as f64 }
                else { f64::from_bits(((rk & 0xFFFF_FFFC) as u64) << 32) };
    if rk & 1 != 0 { v /= 100.0; } v }
```

**Minimum record set for a forward-only cell reader**: Globals: BOF, FilePass (abort), CodePage (BIFF5), Date1904, Format, XF, BoundSheet8, SST+Continue, EOF. Per sheet (seek `lbPlyPos`, check BOF dt): Number, RK, MulRk, LabelSst, Label (BIFF5/7), BoolErr, Formula+String (skip ShrFmla/Array), Blank/MulBlank (only for formatted-empty), Row (only for hidden), Dimensions (advisory), EOF. This matches ExcelDataReader `ReadSingleCell` and xlrd `Sheet.read()` dispatch sets.

### 13. Fast XML tokenizing for sheet XML

**quick-xml** (`https://raw.githubusercontent.com/tafia/quick-xml/master/src/{reader/mod.rs, reader/buffered_reader.rs, reader/slice_reader.rs, reader/state.rs, parser/element.rs, events/mod.rs, events/attributes.rs, escape.rs, encoding.rs, README.md, Changelog.md}`): events `Start/End/Empty/Text/CData/Comment/Decl/PI/DocType/GeneralRef/Eof` borrowing `'i`; `Reader<&[u8]>` returns borrowed slices; `Reader<R: BufRead>` copies into a caller `Vec<u8>` (reuse via `buf.clear()`). Scanning: text via `memchr2(b'<', b'&')`; tags via `ElementParser` — a 3-state machine `Outside|SingleQ|DoubleQ` over `memchr3_iter(b'>', b'\'', b'"')`, so `>` inside quoted attribute values is handled; `<!` via `BangType` (CData/Comment/DocType) with `memchr_iter(b'>')` and straddle handling. Attributes: lazy iterator (whitespace, key, `=`, quoted value), duplicate check linear up to 32 then hashed; `raw_attr()` for raw bytes. Unescape via `memchr2_iter(b'&', b';')`, `Cow::Borrowed` when no `&`; `&#N;`/`&#xN;` always resolved. Encoding: `encoding_rs` behind `encoding` feature with an explicit UTF-8 `from_utf8` fast path; without it UTF-8 only. Current master stores values as `Cow<str>` (0.3x used `Cow<[u8]>`). README: "~50 times faster than xml-rs": 198,866 ns vs 14,468,930 ns (hardware not stated).

**SIMD XML**: **simdxml exists** (`https://github.com/cigrainger/simdxml`, blog `https://cigrainger.com/blog/simdxml/`, Mar 2026): two-pass simdjson-style; pass 1 classifies `< > / = " '` into 64-byte bitmasks (NEON/AVX2/SSE4.2), quote masking via clmul, sequential fallback when both quote kinds appear in one block (<1%); pass 2 builds flat tag/attr/text index arrays. Claims: 1.43 GB/s structural index on M4 Max vs quick-xml 1.27 GB/s tokenizing; attribute-heavy 1 MB: 216 µs vs quick-xml 859 µs (4x). No DTD. Lemire is quoted explaining why a simdjson-for-XML is hard (tag names must be matched, attributes create structure within tags, self-closing detection). Mison (VLDB 2017) and Sparser (VLDB 2018, raw byte-level filters, "up to 22x faster than state-of-the-art parsers") are the raw-filtering ancestors. pugixml/RapidXML tricks (`https://aosabook.org/en/posa/parsing-xml-at-the-speed-of-light.html`, `https://rapidxml.sourceforge.net/manual.html`): in-situ strings, sentinel `\0` at buffer end to elide bounds checks, 256-entry char-class table, `(unsigned)(ch-'0')<10`, 4-byte-at-a-time ASCII checks, single-gap in-place entity rewriting, `bool` template params for dead-code elimination; RapidXML "approaching strlen()", "close to 1 GB/s".

**Hand-rolled sheet parsing**: xlsxio uses expat with 256-byte inflate chunks (`https://raw.githubusercontent.com/brechtsanders/xlsxio/master/lib/xlsxio_read.c`, `PARSE_BUFFER_SIZE 256`) — not a speed reference. SheetJS is regex-based and periodically breaks (issues #1031 sheet name containing `>`, #24 `xml:space` attributes, #2075 unknown namespace). excelize uses Go `encoding/xml` (`xmlWorksheet.go`), "2~3 times slower than js-xlsx" (issue #439). rusty_sheet (DuckDB extension on calamine): 1M-row XLSX 13 s on M1 (`https://duckdb.org/community_extensions/extensions/rusty_sheet`). Real-world variance: `<x:c r="A1" t="inlineStr"><x:v>0</x:v><x:is><x:t>ID</x:t></x:is></x:c>` from a third-party generator (calamine #466); Microsoft's own SDK docs show `<x:c>` OuterXml; rows carry `x14ac:dyDescent` with worksheet-level `mc:Ignorable="x14ac"` (exceljs fixtures). **Claim that Excel always emits attributes in order `r, s, t` and never comments/PI/CDATA inside `sheetData`: not verified** (ECMA-376 does not constrain attribute order) — a fast path with a general fallback, not a guarantee.

### 14. Zip / inflate in Rust

- **miniz_oxide** (`https://raw.githubusercontent.com/Frommi/miniz_oxide/master/README.md`): pure safe Rust, no_std with alloc; streaming via `inflate::stream::inflate(state, in, out, flush)` with boxed `InflateState`; versions before 0.8.4 regress badly on rustc ≥ 1.81.
- **zlib-rs** (`https://raw.githubusercontent.com/trifectatechfoundation/zlib-rs/main/{README.md, zlib-rs/Cargo.toml, zlib-rs/src/inflate.rs}`): zlib-API-compatible, no_std-capable (`rust-allocator`/`c-allocator` features), window `(1<<15)+64` allocated via zalloc; easiest via `flate2` feature `zlib-rs`. Benchmarks: 2025-02-25 x86_64 (`https://trifectatech.org/blog/zlib-rs-is-faster-than-c/`): decompression 6.9% faster than zlib-ng at 64 KiB chunks, 12.95% at 1 KiB; chunk size matters (2^8 vs 2^16 chunks: 116M vs 84.5M). 2024-11 WASM: 54.9% faster than miniz_oxide (`https://trifectatech.org/blog/fastest-wasm-zlib/`). `infback` module exists; preallocated-window API in Rust: not verified.
- **libdeflate / libdeflater** (`https://raw.githubusercontent.com/ebiggers/libdeflate/master/README.md`, `.../adamkewley/libdeflater/master/README.md`): "There is currently no support for streaming ... significantly increases complexity and slows down fast paths"; needs uncompressed size (`LIBDEFLATE_INSUFFICIENT_SPACE` otherwise); libdeflater ~2x flate2 on Calgary decompression (338→114 µs). Pure-Rust port: `streaming-libdeflate-rs` (2026-08-25, decompress-only, claims streaming; no benchmarks) — not verified beyond existence.
- **zune-inflate** (`https://raw.githubusercontent.com/etemesi254/zune-image/dev/crates/zune-inflate/README.md`): libdeflate-derived, whole-buffer only, no unsafe; third-party: "roughly on par with zlib-ng" (gitoxide #732); exrs measured ~5.7% (PR #179).
- **flate2** backends: `miniz_oxide` default, `zlib-rs` "the fastest overall", `zlib-ng`; all stream.
- **zip2** (`https://raw.githubusercontent.com/zip-rs/zip2/master/Cargo.toml`): features `deflate-flate2`, `deflate-flate2-zlib-rs` (default), `-zlib`, `-zlib-ng`; `ZipArchive::new(Read+Seek)` reads the central directory and ignores local headers; `read_zipfile_from_stream` walks local headers sequentially (not recommended); mmap via `Cursor<&[u8]>`. **rc-zip** (`https://raw.githubusercontent.com/bearcove/rc-zip/main/README.md`): sans-io state machines, central-directory-first, zip64, streaming mode explicitly discouraged. nickb.dev zip bench (2022): miniz_oxide direct 1.2x, libdeflate 1.8x vs zip crate default.
- **Streaming vs whole-entry for 300 MB sheet1.xml**: streaming needs only the 32 KiB window + your output chunk (`MAX_WBITS=15`); whole-buffer (libdeflate/zune) needs the full output Vec and the size from the local header / central directory / data descriptor; feed large input chunks (zlib-rs chunk-size data above). mmap vs read for zip: not verified.
- **XLSX-specific**: Excel writes DEFLATE (`defS`, version 4.5) (`https://support.moonpoint.com/software/office/xlsx-compression.php`, small sample 27.4% of original); "~10x for sheet XML": not verified. zip64 needed for entries > 4 GiB; Excel "is quite strict" about zip64 (`https://rzymek.github.io/post/excel-zip64/`).

---

## Takeaways for a Rust forward-only core (ranked)

1. **Two-stage CSV: SIMD structural index (clmul quote mask) + scalar walk of set bits, sequential per 64-byte block.** Verified in zsv-fast (5.44/4.56 GB/s unquoted/quoted single-thread), Polars (`prefix_xorsum_inclusive` + cached remaining bits), simdcsv, Sep. The mask `field_sep = (nl|sep) & ~in_quotes` with three carries (`in_quotes`, `cr_end`, optionally `next_is_escaped`) is the whole hot loop. `""` needs no special masking; flag needs-unescape with `quotes & (quotes<<1)`. Use `core::arch` clmul with the 6-round shift fallback (Polars `clmul.rs` is a clean template).

2. **Keep a table-driven scalar state machine as the compat/fallback path**, and bail to it from the fast path on "nonstandard" evidence (zsv's opening-quote-not-at-cell-start check; Arrow's quoting-only-at-field-start). DuckDB's `[256][19]` byte-major table with a two-state carry and SWAR skip, or csv-core's 7-class × 10-state table, are both proven; DuckDB's is the more complete dialect model (multi-byte delimiters, comments, relaxed modes).

3. **Zero-copy cell delivery with a `quoted`/`needs_unescape` flag; unescape lazily and in place.** zsv (`s++; n-=2` in the common case, `struct zsv_cell{str,len,quoted}`), DuckDB (`string_t` into the pinned buffer), Polars (slices include quotes; typed builder unescapes into scratch), FastCSV (`addField(buf, off, len, isQuoted)`) all converge here. For an FFI surface this is exactly the shape you want: `(ptr, len, flags)` triples valid until the next `parse_more`.

4. **Buffer model: single large buffer, memmove the partial row to the front on refill, rebase cell pointers, and defer that memmove until the *next* call** so delivered rows stay valid (zsv). Pair with a max-row-size guard (DuckDB: 2 MB default, buffer = 16 × that). Large input chunks matter for inflate too (zlib-rs numbers).

5. **Parallel CSV only via chunk-boundary verification, never assume newline = row.** Arrow's default chunker ignores quotes; Polars' two-state `analyze_chunk` (stats for "started inside" and "started outside") plus `accept_line` over three consecutive lines, and DuckDB's "guess state, parse one row, check column count" are the two working designs. Polars' is the cheaper to implement on top of the clmul mask. Optional for a forward-only core — but the boundary finder is the same code as the fast path.

6. **Typed conversion is a separate bulk pass over a `(offset, quoted)` descriptor array**, not per-cell during tokenizing (Arrow `ParsedValueDesc{offset:31, quoted:1}` + `VisitColumn`; arrow-csv `StringRecords` with unchecked slices; Polars `Builder` enum). Use `fast_float2` + `atoi_simd` (Polars) or Arrow's vendored fast_float; validate UTF-8 once per chunk with `simdutf8`, not per field.

7. **XLSX: stream `sheetN.xml` through an inflate stream with a hand-rolled, namespace-prefix-agnostic tokenizer for the `sheetData` subtree only**, falling back to quick-xml outside it. Evidence: calamine's parse-`<v>`-from-raw-bytes trick is where its wins come from; quick-xml itself is ~1.27 GB/s (simdxml blog) and `memchr3`-based; the regularity assumption (attribute order, no CDATA) is *not* spec-guaranteed (SheetJS breakages), so keep a fallback. simdxml shows a clmul structural pass over `< > / = " '` is viable if you want to go further. Do not load full `Range`s (calamine #433); emit cells as `(row, col, DataRef)` from `next_cell()`.

8. **Preload only `sharedStrings.xml` (as one contiguous byte arena + offsets, not `Vec<String>`) and the `cellXfs → is_date` bitmap.** Rich runs: concat `t` and `r/t`, skip `rPh`. Date-ness: built-in ids {14-22, 45-47, 27-36, 50-58, 71-81} + first-`;`-section scan of custom codes with `[h]/[m]/[s]` = duration (calamine/xlrd/POI agree). Cells may omit `r` (advance column); `dimension` is advisory.

9. **Inflate: stream with `flate2` + `zlib-rs` backend (fastest streaming; no_std-capable) for sheet parts; consider libdeflate/zune-inflate whole-buffer only for small parts (styles, workbook, rels, possibly sharedStrings)** where the size is known from the central directory. zip2 with `deflate-flate2-zlib-rs` or rc-zip (sans-io) for the container; read the central directory, not local headers.

10. **ODS: stream `content.xml`, never expand repeated *empty* rows/cells** (LibreOffice pads to 16384/1048576); typed values are attributes (`office:value`, `office:date-value`, `office:time-value`, `office:boolean-value`), so no style lookup is needed for typing; join `text:p` with `\n`, expand `text:s@text:c`, skip `office:annotation`/`draw:frame`; honor `calcext:value-type="error"`.

11. **XLS: implement the minimal record set above with a CONTINUE-aware cursor that re-reads the `fHighByte` flag at every boundary** (the one genuinely tricky part; xlrd/ExcelDataReader are the references), RK decode as given, `Formula` + trailing `String` with ShrFmla/Array skip, and refuse on `FilePass`. Calamine's omission of `Blank`/`MulBlank` is fine for a value reader. Seek per sheet via `BoundSheet8.lbPlyPos` — naturally forward-only per sheet.

12. **Lower priority / avoid**: Arrow's bloom/`pcmpistri` bulk filter (superseded by movemask+clmul); csv-core's byte-class indirection (its own author says it doesn't matter); rune/line-oriented designs (Go); loading sheet XML through a general DOM/SAX with tiny inflate chunks (xlsxio's 256 B).
