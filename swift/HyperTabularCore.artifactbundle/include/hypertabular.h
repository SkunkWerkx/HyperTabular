// The C ABI of the hypertabular core: the six functions rust/src/kernel/exports.rs exports,
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
// consumed, and the result's `needed` says how much one row takes.
#define HYPERTABULAR_OK 0
#define HYPERTABULAR_ERR_CONTRACT (-1)
#define HYPERTABULAR_ERR_STRUCTURE (-2)
#define HYPERTABULAR_ERR_ARENA (-3)
#define HYPERTABULAR_ERR_CELLS (-4)

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
// 2 a record's cell count disagrees with the first record's.
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
// is the widest ordinal the plan reads plus one.
int32_t hypertabular_delimited_fill(hypertabular_delimited_state *state,
                                    const uint8_t *input, uintptr_t input_len, uint32_t last,
                                    const hypertabular_column_spec *specs,
                                    const hypertabular_column_buffer *columns,
                                    uintptr_t column_count, uintptr_t max_rows,
                                    hypertabular_span *cells, uintptr_t cells_cap,
                                    uint8_t *arena, uintptr_t arena_cap,
                                    hypertabular_filled *out);

// Unescapes one quoted cell — a flagged cell-table entry names one — into `out`, and
// returns the bytes written: never more than `len - 1`, and no more than `cap`.
uintptr_t hypertabular_delimited_unescape(const uint8_t *cell, uintptr_t len,
                                          uint8_t *out, uintptr_t cap);

#ifdef __cplusplus
}
#endif

#endif
