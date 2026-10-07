// The C ABI of the hypertabular core: the fourteen functions rust/src/kernel/exports.rs exports,
// which is everything the static libraries in this bundle define, and the #[repr(C)] shapes
// that cross them (rust/src/kernel/abi.rs, rust/src/kernel/delimited/fill.rs). The Swift
// binding imports this as the module HyperTabularCore; every other binding declares the
// same signatures in its own language.
//
// The core owns no memory. Every pointer is the caller's for the length of the call and no
// longer; a length of 0 never dereferences its pointer. Nothing is retained between calls
// except what the caller's state block holds.
#ifndef HYPERTABULAR_H
#define HYPERTABULAR_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Return codes. HYPERTABULAR_OK: the call did what it could, and the result says how far it
// got. ERR_CONTRACT: a caller bug — an undefined door, an invalid numeric format, a buffer
// the contract rules out. ERR_STRUCTURE: the data is structurally broken, the result's
// failure says where, and the same state block reports it again on every later call.
// ERR_ARENA / ERR_CELLS: one row does not fit the arena / the cell table; nothing was
// consumed, and the result's `needed` says how much one row takes. ERR_WINDOW: a workbook
// call's window is too small. For a workbook call, ERR_WINDOW, ERR_ARENA and ERR_CELLS
// undo nothing: grow the buffer named, keeping what it held, and make the same call again.
#define HYPERTABULAR_OK 0
#define HYPERTABULAR_ERR_CONTRACT (-1)
#define HYPERTABULAR_ERR_STRUCTURE (-2)
#define HYPERTABULAR_ERR_ARENA (-3)
#define HYPERTABULAR_ERR_CELLS (-4)
#define HYPERTABULAR_ERR_WINDOW (-5)

// The flag in the top bit of a span's `len`. On a cell-table entry: the cell is quoted with
// "" inside and has to be unescaped to be read. On a text value or a header name: the bytes
// are in the arena rather than in the input.
#define HYPERTABULAR_SPAN_FLAG 0x80000000u

// A byte range; `len` carries the flag above in its top bit.
typedef struct hypertabular_span {
    uint32_t offset;
    uint32_t len;
} hypertabular_span;

// One cell's verdict: HyperCast's reason code (0 ok, 1 empty, 2 malformed, 3 out of range)
// and the offending span within the cell's own text.
typedef struct hypertabular_cell_verdict {
    uint32_t offset;
    uint32_t len;
    uint32_t reason;
} hypertabular_cell_verdict;

// HyperCast's numeric notation as it crosses its ABI. All zeros is the invariant notation.
typedef struct hypertabular_num_format {
    uint32_t decimal_sep;
    uint32_t group_sep;
    uint32_t flags;
    uint32_t currency_len;
    uint8_t currency[16];
} hypertabular_num_format;

// One output column: the source ordinal it reads, the door's code (1 bool, 2-5 i8-i64,
// 6-9 u8-u64, 10 f32, 11 f64, 12 uuid, 13 timestamp, 14 unix, 15 date, 16 time,
// 17 duration, 18 text, 19 decimal, 20 date under an order, 21 civil date-time, 22 excel
// serial), what the door declares beside itself (a unix precision, a date order, an excel
// date system — numbered as HyperCast numbers them; 0 otherwise), and the notation the
// numeric doors read.
typedef struct hypertabular_column_spec {
    uint32_t ordinal;
    uint32_t door;
    uint32_t param;
    hypertabular_num_format format;
} hypertabular_column_spec;

// Where one column's output goes: `max_rows` values of the door's type and `max_rows`
// verdicts. Neither has to be aligned.
typedef struct hypertabular_column_buffer {
    void *values;
    hypertabular_cell_verdict *verdicts;
} hypertabular_column_buffer;

// Why the data could not be read. `code`: 0 none, 1 the input ended inside a quoted cell,
// 2 a record's cell count disagrees with the first record's; for a workbook, 16 not a zip,
// 17 a broken container, 18 encrypted, 19 an unsupported compression method, 20 a missing
// part, 21 unfinished XML, 22 a broken deflate stream, 23 a zip that is no workbook, 24 a
// shared string out of range, 25 more text than can be addressed. For a workbook `record`
// is the part, `line` the sheet row and `byte` the offset in the part's inflated bytes.
typedef struct hypertabular_failure {
    uint32_t code;
    uint32_t line;     // one-based line the offending record starts on
    uint64_t record;   // zero-based record index, header and skipped blank lines included
    uint64_t byte;     // absolute byte offset of the offending record's start
    uint32_t expected; // cells in the first record (code 2)
    uint32_t found;    // cells in this record (code 2)
} hypertabular_failure;

// What a call did.
typedef struct hypertabular_filled {
    uint64_t rows;       // rows written to every column (or, for a header, names written)
    uint64_t consumed;   // bytes of the input that are finished with
    uint64_t arena_used; // bytes of the arena written
    uint64_t needed;     // on ERR_ARENA, arena bytes one row needs; on ERR_CELLS, entries
    hypertabular_failure failure;
} hypertabular_filled;

// The dialect, declared. `engine` is 0 for the best scanner this CPU has.
typedef struct hypertabular_dialect {
    uint8_t separator;
    uint8_t quoting;
    uint8_t skip_blank_lines;
    uint8_t engine;
} hypertabular_dialect;

// Where one input stands: 64 bytes the core defines and writes, 8-byte aligned. The caller
// allocates it and hands it to hypertabular_delimited_init once before anything else.
typedef struct hypertabular_delimited_state {
    uint8_t separator;
    uint8_t quoting;
    uint8_t skip_blank_lines;
    uint8_t engine;
    uint32_t line;     // one-based line number of the next unread byte
    uint64_t records;  // records finished so far, header and skipped blank lines included
    uint64_t offset;   // absolute byte offset of the next unread byte
    uint32_t expected; // cells per record, fixed by the first; 0 before it
    uint8_t started;
    uint8_t reserved[3];
    hypertabular_failure failure;
} hypertabular_delimited_state;

// The value a column buffer holds per door, where it is not a plain integer or float:
// HyperCast's own #[repr(C)] values.
typedef struct hypertabular_timestamp { // timestamp, unix, excel serial
    int64_t seconds;
    int32_t nanos;
} hypertabular_timestamp;

typedef struct hypertabular_date { // date, date under an order
    uint16_t year;
    uint8_t month;
    uint8_t day;
} hypertabular_date;

typedef struct hypertabular_civil { // civil date-time
    hypertabular_date date;
    uint64_t nanos_of_day;
} hypertabular_civil;

typedef struct hypertabular_duration {
    int64_t seconds;
    int32_t nanos;
} hypertabular_duration;

typedef struct hypertabular_decimal { // value = ±(hi·2⁶⁴ + lo) × 10^-scale
    uint64_t lo;
    uint32_t hi;
    uint8_t scale;
    uint8_t negative;
} hypertabular_decimal;

// One cell of the row a workbook read assembles: scratch the caller supplies and never reads.
typedef struct hypertabular_slot {
    uint32_t tag;
    uint32_t aux;
    uint64_t bits;
} hypertabular_slot;

// The memory a workbook call works in, all the caller's, and the delimited fill's `arena` and
// `cells`; a call uses the buffers its own comment names and ignores the rest (NULL with a
// zero size is fine for those).
typedef struct hypertabular_buffers {
    uint8_t *window;            // where a part is inflated: at least 64 KiB; ERR_WINDOW grows it
    uintptr_t window_cap;
    uint8_t *arena;             // text the call produces; ERR_ARENA grows it
    uintptr_t arena_cap;
    hypertabular_span *cells;   // spans the call produces; ERR_CELLS grows it
    uintptr_t cells_cap;
    hypertabular_slot *row;     // one per source column the plan reaches; never grown
    uintptr_t row_cap;
    const uint8_t *strings;     // the shared strings' bytes, as _strings wrote them
    uintptr_t strings_len;
    const hypertabular_span *table; // the span of each shared string
    uintptr_t table_len;
    const uint8_t *kinds;       // the kind of each cell format, as _styles wrote them
    uintptr_t kinds_len;
} hypertabular_buffers;

// What opening a workbook found.
typedef struct hypertabular_opened {
    uint32_t format;        // 1 XLSX, 2 ODS
    uint32_t epoch;         // HyperCast's ExcelEpoch: 1 1900, 2 1904
    uint64_t strings_bytes; // an upper bound on the bytes the shared strings take
    uint64_t strings_count; // the count the part declares: a hint
    uint64_t needed;        // on ERR_WINDOW or ERR_ARENA, the size that buffer needs
    hypertabular_failure failure;
} hypertabular_opened;

// The crate version, packed major << 16 | minor << 8 | patch.
uint32_t hypertabular_version(void);

// The size of a state block, for a caller that allocates it as bytes.
uintptr_t hypertabular_delimited_state_size(void);

// Starts an input: writes a fresh state for the dialect. ERR_CONTRACT for a null or a
// separator that is not tab or printable ASCII other than '"'.
int32_t hypertabular_delimited_init(hypertabular_delimited_state *state,
                                    const hypertabular_dialect *dialect);

// Reads the next record as a header: each name located in `names` as a text value is — a
// span into the input, flagged when it is in the arena instead. `out->rows` is the number
// of names.
int32_t hypertabular_delimited_header(hypertabular_delimited_state *state,
                                      const uint8_t *input, uintptr_t input_len, uint32_t last,
                                      hypertabular_span *names, uintptr_t names_cap,
                                      uint8_t *arena, uintptr_t arena_cap,
                                      hypertabular_filled *out);

// Reads up to `max_rows` whole rows of `input` through `specs` into `columns`. `cells` is
// the cell table: row r's entry for source column c is at r * (width + 1) + c, where width
// is the widest ordinal the plan reads plus one. Uses `buffers->cells` and `buffers->arena`
// only: they travel in the struct because Mono's interpreter (.NET in the browser, an iOS
// debug build) passes no more than twelve integer arguments to a native function.
int32_t hypertabular_delimited_fill(hypertabular_delimited_state *state,
                                    const uint8_t *input, uintptr_t input_len, uint32_t last,
                                    const hypertabular_column_spec *specs,
                                    const hypertabular_column_buffer *columns,
                                    uintptr_t column_count, uintptr_t max_rows,
                                    const hypertabular_buffers *buffers,
                                    hypertabular_filled *out);

// Unescapes one quoted cell — a flagged cell-table entry names one — into `out`, and
// returns the bytes written: never more than `len - 1`, and no more than `cap`.
uintptr_t hypertabular_delimited_unescape(const uint8_t *cell, uintptr_t len,
                                          uint8_t *out, uintptr_t cap);

// The size of a workbook state block, 8-byte aligned. A block that has been opened may be
// copied, bytes and all, to read another sheet.
uintptr_t hypertabular_workbook_state_size(void);

// Opens the workbook in `container`. Uses window and arena; starts over when called again.
int32_t hypertabular_workbook_open(void *state, const uint8_t *container, uintptr_t container_len,
                                   const hypertabular_buffers *buffers, hypertabular_opened *out);

// Lists the sheets: three spans of `cells` each — the name and the part, both in the arena,
// then one whose offset bit 0 says hidden and whose len is the sheet's index.
int32_t hypertabular_workbook_sheets(void *state, const uint8_t *container, uintptr_t container_len,
                                     const hypertabular_buffers *buffers, hypertabular_filled *out);

// Loads the shared strings: the bytes in the arena (arena_used), a span each in cells (rows).
int32_t hypertabular_workbook_strings(void *state, const uint8_t *container, uintptr_t container_len,
                                      const hypertabular_buffers *buffers, hypertabular_filled *out);

// Loads the kind of each cell format: a byte each in the arena (rows).
int32_t hypertabular_workbook_styles(void *state, const uint8_t *container, uintptr_t container_len,
                                     const hypertabular_buffers *buffers, hypertabular_filled *out);

// Positions the state on one sheet: the part and index its listing gave.
int32_t hypertabular_workbook_sheet(void *state, const uint8_t *container, uintptr_t container_len,
                                    const uint8_t *part, uintptr_t part_len, uint32_t index,
                                    uint32_t has_header, uint32_t skip_empty_rows,
                                    hypertabular_filled *out);

// Reads the header row: a span per name in cells — in the shared strings, or flagged, in the
// arena.
int32_t hypertabular_workbook_header(void *state, const uint8_t *container, uintptr_t container_len,
                                     const hypertabular_buffers *buffers, hypertabular_filled *out);

// Reads up to `max_rows` rows into `columns`. `cells` is (plan columns + 1) entries a row: a
// raw cell per plan column, and the row's number in the last entry's offset.
int32_t hypertabular_workbook_fill(void *state, const uint8_t *container, uintptr_t container_len,
                                   const hypertabular_column_spec *specs,
                                   const hypertabular_column_buffer *columns,
                                   uintptr_t column_count, uintptr_t max_rows,
                                   const hypertabular_buffers *buffers, hypertabular_filled *out);

#ifdef __cplusplus
}
#endif

#endif
