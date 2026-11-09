#include <R.h>
#include <Rinternals.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>

#include "include/hypertabular.h"
#include "shim.h"

/*
 * The readers, over the core's C ABI. A delimited reader, a workbook and a sheet are external
 * pointers to the native memory a read works in (the state block, the plan's arrays, the cell
 * table, the arena, the window), all R_Calloc'd and freed by close() or the pointer's
 * finalizer, so an R error or an interrupt between calls leaks nothing. Every batch is copied
 * out into R vectors as it is made, so it outlives the reader's next read, as in every binding
 * of a garbage-collected language.
 *
 * A fill that wrote rows is held until R says it has made its batch of them (the ack entry
 * points): the core has moved past them, so an interrupt or an error during the copy, or
 * during what R makes of the copy, leaves them in the native arrays, and the next fill hands
 * the same batch over before it asks the core for more. Nothing a long jump interrupts leaves
 * the core and the shim disagreeing about where the read stands.
 *
 * R_CheckUserInterrupt runs at each batch, before the core is called, and every 2^20 elements
 * of a copy, between core calls, so no Rust frame is ever live when R long-jumps.
 *
 * Each plan column becomes the columns of one HyperCast verdict: the value (NA where the cell
 * did not cast) and its exact parts, then ok, the reason code (1 empty, 2 malformed, 3 out of
 * range), the fault's offset and length, raw (the cell's text a fault's span indexes), the
 * detail code and the origin code. Nothing here raises an R error for anything in the data: a
 * cell is a verdict, and a broken file a failure handed back as a value. The only errors are
 * caller bugs R validated before the call (which the core's contract-violation code would also
 * report) and running out of memory.
 */

/* ---- the doors ---- */

#define DOOR_COUNT 23 /* codes 1..22 */
#define UUID_DOOR 12
#define TEXT_DOOR 18

enum kind {
  LOGICAL_KIND,  /* value: logical */
  INTEGER_KIND,  /* value: integer */
  DOUBLE_KIND,   /* value: double */
  STRING_KIND,   /* value: character */
  INSTANT_KIND,  /* value: double seconds (POSIXct in R); seconds, nanos */
  DATE_KIND,     /* value: double days (Date in R) */
  CIVIL_KIND,    /* value: character wall clock; date (days), nanos_of_day */
  TIME_KIND,     /* value: double seconds (difftime in R); nanos */
  DURATION_KIND, /* value: double seconds (difftime in R); seconds, nanos */
  BYTES_KIND,    /* value: a list of raw vectors (col_text(raw = TRUE)) */
};

/* Per door code: the R kind, and the bytes one value takes in a column buffer. */
static const struct {
  enum kind kind;
  size_t size;
} DOORS[DOOR_COUNT] = {
    [1] = {LOGICAL_KIND, 1},    /* bool */
    [2] = {INTEGER_KIND, 1},    /* i8 */
    [3] = {INTEGER_KIND, 2},    /* i16 */
    [4] = {DOUBLE_KIND, 4},     /* i32 */
    [5] = {STRING_KIND, 8},     /* i64 */
    [6] = {INTEGER_KIND, 1},    /* u8 */
    [7] = {INTEGER_KIND, 2},    /* u16 */
    [8] = {DOUBLE_KIND, 4},     /* u32 */
    [9] = {STRING_KIND, 8},     /* u64 */
    [10] = {DOUBLE_KIND, 4},    /* f32 */
    [11] = {DOUBLE_KIND, 8},    /* f64 */
    [12] = {STRING_KIND, 16},   /* uuid */
    [13] = {INSTANT_KIND, 16},  /* timestamp */
    [14] = {INSTANT_KIND, 16},  /* unix */
    [15] = {DATE_KIND, 4},      /* date */
    [16] = {TIME_KIND, 8},      /* time: u64 nanoseconds since midnight */
    [17] = {DURATION_KIND, 16}, /* duration */
    [18] = {STRING_KIND, 8},    /* text: a span */
    [19] = {STRING_KIND, 16},   /* decimal */
    [20] = {DATE_KIND, 4},      /* date under an order */
    [21] = {CIVIL_KIND, 16},    /* civil date-time */
    [22] = {INSTANT_KIND, 16},  /* excel serial */
};

/* The layouts the shim reads and writes, held to the sizes the generated header states, and the
 * HyperCast value layouts a column buffer holds, to the sizes the door table gives them. */
_Static_assert(sizeof(HyperTabularSpan) == HyperTabularSpan_SIZE, "span");
_Static_assert(sizeof(HyperTabularCellVerdict) == HyperTabularCellVerdict_SIZE, "verdict");
_Static_assert(sizeof(HyperTabularCellNote) == HyperTabularCellNote_SIZE, "note");
_Static_assert(sizeof(HyperTabularColumnSpec) == HyperTabularColumnSpec_SIZE, "column spec");
_Static_assert(sizeof(HyperTabularRawNumFormat) == HyperTabularRawNumFormat_SIZE, "num format");
_Static_assert(sizeof(HyperTabularRawDateOptions) == HyperTabularRawDateOptions_SIZE, "date");
_Static_assert(sizeof(HyperTabularRawDelimitedOptions) == HyperTabularRawDelimitedOptions_SIZE,
               "delimited options");
_Static_assert(sizeof(HyperTabularRawSheetOptions) == HyperTabularRawSheetOptions_SIZE,
               "sheet options");
_Static_assert(sizeof(HyperTabularRawTranscoded) == HyperTabularRawTranscoded_SIZE, "transcoded");
_Static_assert(sizeof(HyperTabularRawProblem) == HyperTabularRawProblem_SIZE, "problem");
_Static_assert(sizeof(HyperTabularRawSuggestion) == HyperTabularRawSuggestion_SIZE, "suggestion");
_Static_assert(sizeof(HyperTabularRawRange) == HyperTabularRawRange_SIZE, "range");
_Static_assert(sizeof(HyperTabularRawBatchCheck) == HyperTabularRawBatchCheck_SIZE, "check");
_Static_assert(sizeof(HyperTabularFailure) == HyperTabularFailure_SIZE, "failure");
_Static_assert(sizeof(HyperTabularFilled) == HyperTabularFilled_SIZE, "filled");
_Static_assert(sizeof(HyperTabularOpened) == HyperTabularOpened_SIZE, "opened");
_Static_assert(sizeof(HyperTabularSlot) == HyperTabularSlot_SIZE, "slot");
_Static_assert(sizeof(HyperTabularRawDialect) == HyperTabularRawDialect_SIZE, "dialect");
_Static_assert(sizeof(HyperTabularDelimitedState) == HyperTabularDelimitedState_SIZE, "state");
_Static_assert(sizeof(HyperTabularSheetInfo) == HyperTabularSheetInfo_SIZE &&
                   HyperTabularSheetInfo_SIZE == 3 * HyperTabularSpan_SIZE,
               "sheet info");
_Static_assert(sizeof(HyperTabularTimestamp) == 16, "timestamp");
_Static_assert(sizeof(HyperTabularDate) == 4, "date value");
_Static_assert(sizeof(HyperTabularCivilDateTime) == 16, "civil");
_Static_assert(sizeof(HyperTabularDuration) == 16, "duration");
_Static_assert(sizeof(HyperTabularDecimal) == 16, "decimal");
_Static_assert(sizeof(HyperTabularBuffers) == 14 * sizeof(void *), "buffers");

/* What R packs for one plan column: ordinal, door and param, HyperCast's 92-byte RawNumFormat
 * and its 12-byte RawDateOptions, little-endian. */
#define PACKED_SPEC (12 + HyperTabularRawNumFormat_SIZE + HyperTabularRawDateOptions_SIZE)

/* A span's length, its flag bit cleared. */
#define SPAN_LENGTH (HyperTabularSpan_FLAG - 1u)

/* Elements copied between two looks at R_CheckUserInterrupt. */
#define INTERRUPT_EVERY ((size_t)1 << 20)

#if defined(__GNUC__)
__attribute__((noreturn))
#endif
static void contract_violation(void);
static void contract_violation(void) {
  Rf_error("hypertabular: the core reported a contract violation (a binding bug this package "
           "checks for first; please report it)");
}

#if defined(__GNUC__)
__attribute__((noreturn))
#endif
static void too_large(void);
static void too_large(void) {
  Rf_error("hypertabular: a size past what this platform can address");
}

/* ---- checked sizes ---- */

static size_t mul(size_t a, size_t b) {
  if (b && a > SIZE_MAX / b)
    too_large();
  return a * b;
}

static size_t add(size_t a, size_t b) {
  if (a > SIZE_MAX - b)
    too_large();
  return a + b;
}

/* ---- native memory ---- */

/* The 8-byte words that hold n bytes, at least one: R_Calloc and R_Realloc count elements. */
static size_t words_for(size_t n) {
  return n ? n / sizeof(uint64_t) + (n % sizeof(uint64_t) != 0) : 1;
}

/* A zeroed block of n bytes, at least one, 8-byte aligned (what calloc returns). Nothing here
 * writes it, so pages nobody uses stay the system's. */
static void *block(size_t n) { return R_Calloc(words_for(n), uint64_t); }

/* p grown to n bytes, what it held kept. On failure R_Realloc raises and p is untouched, so a
 * caller assigns the result, and the capacity beside it, only once this returns. */
static void *grown_block(void *p, size_t n) { return R_Realloc(p, words_for(n), uint64_t); }

static void release(void *p) {
  if (p)
    R_Free(p);
}

/* ---- little-endian words ---- */

static uint32_t le32(const uint8_t *b) {
  return (uint32_t)b[0] | (uint32_t)b[1] << 8 | (uint32_t)b[2] << 16 | (uint32_t)b[3] << 24;
}

/* HyperCast's packed RawNumFormat at b, its first three words decoded so a big-endian host reads
 * them right (the rest is bytes). */
static void format_from(const uint8_t *b, HyperTabularRawNumFormat *f) {
  memcpy(f, b, sizeof *f);
  f->size = le32(b);
  f->format.flags = le32(b + 4);
  f->format.digits = le32(b + 8);
}

static void date_from(const uint8_t *b, HyperTabularRawDateOptions *d) {
  d->size = le32(b);
  d->flags = le32(b + 4);
  d->pivot = le32(b + 8);
}

/* A count R checked is a whole number of at least 0: as the core's u64, a count past it the
 * largest. */
static uint64_t count_u64(SEXP x) {
  double d = asReal(x);
  if (ISNAN(d) || d < 0)
    Rf_error("hypertabular: a count must be a whole number of at least 0");
  return d >= 18446744073709551615.0 ? UINT64_MAX : (uint64_t)d;
}

/* A count the core has judged, as a size_t. */
static size_t count_size(SEXP x) {
  uint64_t v = count_u64(x);
  if (v > SIZE_MAX)
    too_large();
  return (size_t)v;
}

/* ---- a plan as the core takes it ---- */

/*
 * The plan's specs, decoded from what R packed, and the native copies of each column's month
 * vocabulary and tokens they point into. `specs` is PACKED_SPEC bytes a column; `lists` a list
 * with an entry a column, each list(months, tokens), each NULL or a raw vector. A null tokens
 * pointer inherits the reader's; an empty list is a real address of no bytes.
 */
struct specs {
  size_t count;
  HyperTabularColumnSpecWith *with;
  uint8_t **owned; /* 2 a column, NULL where none */
};

static void specs_free(struct specs *s) {
  for (size_t i = 0; s->owned && i < 2 * s->count; i++)
    release(s->owned[i]);
  release(s->owned);
  release(s->with);
  memset(s, 0, sizeof *s);
}

static size_t specs_count(SEXP specs, SEXP lists) {
  if (TYPEOF(specs) != RAWSXP || XLENGTH(specs) % PACKED_SPEC != 0)
    Rf_error("hypertabular: specs must be packed column specs");
  size_t count = (size_t)XLENGTH(specs) / PACKED_SPEC;
  if (TYPEOF(lists) != VECSXP || (size_t)XLENGTH(lists) != count)
    Rf_error("hypertabular: lists must hold one entry a column");
  return count;
}

static void list_into(SEXP list, uint8_t **owned, const uint8_t **ptr, uintptr_t *len) {
  if (list == R_NilValue) {
    *ptr = NULL;
    *len = 0;
    return;
  }
  if (TYPEOF(list) != RAWSXP)
    Rf_error("hypertabular: a month vocabulary or a token list is a raw vector");
  size_t n = (size_t)XLENGTH(list);
  *owned = block(n);
  if (n)
    memcpy(*owned, RAW(list), n);
  *ptr = *owned;
  *len = n;
}

/* Fills s, which owns everything it points to; on an R error part way, s holds what was
 * allocated, for specs_free. */
static void specs_build(struct specs *s, SEXP specs, SEXP lists) {
  size_t count = specs_count(specs, lists);
  const uint8_t *b = RAW(specs);
  for (size_t i = 0; i < count; i++) {
    uint32_t door = le32(b + i * PACKED_SPEC + 4);
    if (door == 0 || door >= DOOR_COUNT)
      Rf_error("hypertabular: unknown door %u", door);
  }
  s->with = block(mul(count, sizeof(HyperTabularColumnSpecWith)));
  s->owned = block(mul(2 * count, sizeof(uint8_t *)));
  s->count = count;
  for (size_t i = 0; i < count; i++) {
    const uint8_t *p = b + i * PACKED_SPEC;
    HyperTabularColumnSpecWith *spec = &s->with[i];
    spec->size = HyperTabularColumnSpecWith_SIZE;
    spec->ordinal = le32(p);
    spec->door = le32(p + 4);
    spec->param = le32(p + 8);
    format_from(p + 12, &spec->format);
    date_from(p + 12 + HyperTabularRawNumFormat_SIZE, &spec->date);
    SEXP entry = VECTOR_ELT(lists, (R_xlen_t)i);
    if (TYPEOF(entry) != VECSXP || XLENGTH(entry) != 2)
      Rf_error("hypertabular: lists must hold list(months, tokens) a column");
    list_into(VECTOR_ELT(entry, 0), &s->owned[2 * i], &spec->months, &spec->months_len);
    list_into(VECTOR_ELT(entry, 1), &s->owned[2 * i + 1], &spec->tokens, &spec->tokens_len);
  }
}

/* The plan a reader reads through: its specs, and the arrays the core casts each batch into,
 * one value, verdict and note array a column, sized for one batch, and each row's width when the
 * read keeps them. */
struct plan {
  int bound;
  size_t batch_rows;
  struct specs specs;
  HyperTabularColumnBufferWith *table;
  int *raws; /* text columns given back as raw vectors */
  uint32_t *widths;
};

static void plan_free(struct plan *p) {
  for (size_t i = 0; p->table && i < p->specs.count; i++) {
    release(p->table[i].values);
    release(p->table[i].verdicts);
    release(p->table[i].notes);
  }
  release(p->table);
  release(p->raws);
  release(p->widths);
  specs_free(&p->specs);
  memset(p, 0, sizeof *p);
}

/* Builds the plan. R asked hypertabular_batch_check about these sizes first and raised on a
 * refusal, so nothing here allocates more than the core allows. */
static void plan_bind(struct plan *p, SEXP specs, SEXP lists, SEXP raws, size_t batch_rows,
                      int widths) {
  plan_free(p);
  specs_build(&p->specs, specs, lists);
  size_t count = p->specs.count;
  if (TYPEOF(raws) != LGLSXP || (size_t)XLENGTH(raws) != count)
    Rf_error("hypertabular: raws must hold one flag a column");
  p->batch_rows = batch_rows;
  p->table = block(mul(count, sizeof(HyperTabularColumnBufferWith)));
  p->raws = block(mul(count, sizeof(int)));
  for (size_t i = 0; i < count; i++) {
    HyperTabularColumnBufferWith *c = &p->table[i];
    c->size = HyperTabularColumnBufferWith_SIZE;
    c->values = block(mul(DOORS[p->specs.with[i].door].size, batch_rows));
    c->verdicts = block(mul(sizeof(HyperTabularCellVerdict), batch_rows));
    c->notes = block(mul(sizeof(HyperTabularCellNote), batch_rows));
    p->raws[i] = LOGICAL(raws)[i] == TRUE && p->specs.with[i].door == TEXT_DOOR;
  }
  if (widths)
    p->widths = block(mul(sizeof(uint32_t), batch_rows));
  p->bound = 1;
}

/* ---- text ---- */

/* Whether b[0..n) is UTF-8 as R's validUTF8() means it: shortest forms, no surrogates, nothing
 * past U+10FFFF. */
static int utf8_valid(const uint8_t *b, size_t n) {
  size_t i = 0;
  while (i < n) {
    uint8_t c = b[i];
    if (c < 0x80) {
      i++;
      continue;
    }
    size_t need;
    uint32_t min, cp;
    if (c >= 0xC2 && c <= 0xDF) {
      need = 1;
      min = 0x80;
      cp = c & 0x1F;
    } else if (c >= 0xE0 && c <= 0xEF) {
      need = 2;
      min = 0x800;
      cp = c & 0x0F;
    } else if (c >= 0xF0 && c <= 0xF4) {
      need = 3;
      min = 0x10000;
      cp = c & 0x07;
    } else {
      return 0;
    }
    if (n - i <= need)
      return 0;
    for (size_t k = 1; k <= need; k++) {
      if ((b[i + k] & 0xC0) != 0x80)
        return 0;
      cp = cp << 6 | (b[i + k] & 0x3F);
    }
    if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF))
      return 0;
    i += need + 1;
  }
  return 1;
}

enum text_unit { UNIT_CHARS, UNIT_BYTES, UNIT_NONE };

/*
 * b[0..n) as an R string: marked UTF-8 when it is valid UTF-8 (a span over it counts
 * characters, as substr() does), marked "bytes" when it is not (a span counts bytes, as
 * substr() counts such a string), and NA when it holds a NUL byte or is over INT_MAX bytes,
 * which no R string can.
 */
static SEXP text_char(const uint8_t *b, size_t n, enum text_unit *unit) {
  if (n > (size_t)INT_MAX || memchr(b, 0, n)) {
    *unit = UNIT_NONE;
    return NA_STRING;
  }
  if (utf8_valid(b, n)) {
    *unit = UNIT_CHARS;
    return mkCharLenCE((const char *)b, (int)n, CE_UTF8);
  }
  *unit = UNIT_BYTES;
  return mkCharLenCE((const char *)b, (int)n, CE_BYTES);
}

static SEXP raw_of(const uint8_t *b, size_t n) {
  SEXP v = allocVector(RAWSXP, (R_xlen_t)n);
  if (n)
    memcpy(RAW(v), b, n);
  return v;
}

/* Code points in b[0..n): every one starts with exactly one non-continuation byte. */
static uint32_t code_points(const uint8_t *b, size_t n) {
  uint32_t count = 0;
  for (size_t k = 0; k < n; k++)
    count += (b[k] & 0xC0) != 0x80;
  return count;
}

/* ---- the columns of one verdict ---- */

struct columns {
  SEXP list;
  SEXP names;
  int count;
  enum kind kind;
  int *lgl;
  int *integer;
  double *real;
  SEXP strings;
  SEXP bytes;
  double *extra_real[2];
  int *extra_int;
  int *ok;
  int *reason;
  int *offset;
  int *length;
  SEXP raw;
  int *detail;
  int *origin;
};

/* The next column of c->list, named: the name first, so the new vector is in the list before
 * anything else allocates. */
static SEXP column(struct columns *c, const char *name, SEXPTYPE type, R_xlen_t n) {
  SET_STRING_ELT(c->names, c->count, mkChar(name));
  SEXP v = allocVector(type, n);
  SET_VECTOR_ELT(c->list, c->count, v);
  c->count++;
  return v;
}

/* Allocates every column and returns c->list, unprotected: the caller stores or protects it
 * before anything else allocates. The protect stack is as it was on entry. */
static SEXP columns_alloc(struct columns *c, enum kind kind, R_xlen_t n) {
  memset(c, 0, sizeof *c);
  c->kind = kind;
  int total = 8;
  if (kind == INSTANT_KIND || kind == CIVIL_KIND || kind == DURATION_KIND)
    total += 2;
  else if (kind == TIME_KIND)
    total += 1;
  c->list = PROTECT(allocVector(VECSXP, total));
  setAttrib(c->list, R_NamesSymbol, PROTECT(allocVector(STRSXP, total)));
  UNPROTECT(1);
  /* The names as the list holds them, which column() fills in place. */
  c->names = getAttrib(c->list, R_NamesSymbol);
  switch (kind) {
  case LOGICAL_KIND:
    c->lgl = LOGICAL(column(c, "value", LGLSXP, n));
    break;
  case INTEGER_KIND:
    c->integer = INTEGER(column(c, "value", INTSXP, n));
    break;
  case STRING_KIND:
    c->strings = column(c, "value", STRSXP, n);
    break;
  case BYTES_KIND:
    c->bytes = column(c, "value", VECSXP, n);
    break;
  case CIVIL_KIND:
    c->strings = column(c, "value", STRSXP, n);
    c->extra_real[0] = REAL(column(c, "date", REALSXP, n));
    c->extra_real[1] = REAL(column(c, "nanos_of_day", REALSXP, n));
    break;
  case INSTANT_KIND:
  case DURATION_KIND:
    c->real = REAL(column(c, "value", REALSXP, n));
    c->extra_real[0] = REAL(column(c, "seconds", REALSXP, n));
    c->extra_int = INTEGER(column(c, "nanos", INTSXP, n));
    break;
  case TIME_KIND:
    c->real = REAL(column(c, "value", REALSXP, n));
    c->extra_real[0] = REAL(column(c, "nanos", REALSXP, n));
    break;
  default: /* DOUBLE_KIND, DATE_KIND */
    c->real = REAL(column(c, "value", REALSXP, n));
    break;
  }
  c->ok = LOGICAL(column(c, "ok", LGLSXP, n));
  c->reason = INTEGER(column(c, "reason", INTSXP, n));
  c->offset = INTEGER(column(c, "offset", INTSXP, n));
  c->length = INTEGER(column(c, "length", INTSXP, n));
  c->raw = column(c, "raw", STRSXP, n);
  c->detail = INTEGER(column(c, "detail", INTSXP, n));
  c->origin = INTEGER(column(c, "origin", INTSXP, n));
  UNPROTECT(1);
  return c->list;
}

static void value_na(struct columns *c, R_xlen_t i) {
  if (c->lgl)
    c->lgl[i] = NA_LOGICAL;
  if (c->integer)
    c->integer[i] = NA_INTEGER;
  if (c->real)
    c->real[i] = NA_REAL;
  if (c->strings)
    SET_STRING_ELT(c->strings, i, NA_STRING);
  /* A list's element is NULL already. */
  for (int k = 0; k < 2; k++) {
    if (c->extra_real[k])
      c->extra_real[k][i] = NA_REAL;
  }
  if (c->extra_int)
    c->extra_int[i] = NA_INTEGER;
}

static int span_int(uint64_t v) { return v > (uint64_t)INT_MAX ? INT_MAX : (int)v; }

/*
 * Row i faulted with `reason` over the core's byte span of the cell's text b[0..n): raw is
 * that text, and the span is re-expressed in the units substr() counts raw in.
 */
static void row_fault(struct columns *c, R_xlen_t i, int reason, uint32_t offset, uint32_t len,
                      const uint8_t *b, size_t n) {
  value_na(c, i);
  c->ok[i] = FALSE;
  c->reason[i] = reason;
  enum text_unit unit;
  SET_STRING_ELT(c->raw, i, text_char(b, n, &unit));
  uint64_t start = offset < n ? offset : n;
  uint64_t end = start + len < n ? start + len : n;
  if (unit == UNIT_CHARS) {
    c->offset[i] = span_int(code_points(b, (size_t)start));
    c->length[i] = span_int(code_points(b + start, (size_t)(end - start)));
  } else {
    c->offset[i] = span_int(start);
    c->length[i] = span_int(end - start);
  }
}

/* ---- rendering exact values (as HyperCast's own R binding renders them) ---- */

static char *u64_digits(uint64_t v, char *end) {
  do {
    *--end = (char)('0' + v % 10);
    v /= 10;
  } while (v);
  return end;
}

static SEXP i64_string(int64_t v) {
  char buf[24];
  char *end = buf + sizeof buf;
  uint64_t magnitude = v < 0 ? (uint64_t)0 - (uint64_t)v : (uint64_t)v;
  char *start = u64_digits(magnitude, end);
  if (v < 0)
    *--start = '-';
  return mkCharLen(start, (int)(end - start));
}

static SEXP u64_string(uint64_t v) {
  char buf[24];
  char *end = buf + sizeof buf;
  char *start = u64_digits(v, end);
  return mkCharLen(start, (int)(end - start));
}

/* The exact decimal as HyperCast's Display renders it. */
static SEXP decimal_string(const HyperTabularDecimal *d) {
  uint32_t limbs[3] = {d->hi, (uint32_t)(d->lo >> 32), (uint32_t)d->lo};
  char digits[32];
  int len = 0;
  do {
    uint64_t rem = 0;
    for (int k = 0; k < 3; k++) {
      uint64_t cur = rem << 32 | limbs[k];
      limbs[k] = (uint32_t)(cur / 10);
      rem = cur % 10;
    }
    digits[len++] = (char)('0' + rem);
  } while (limbs[0] | limbs[1] | limbs[2]);
  char buf[300];
  int o = 0;
  int scale = d->scale;
  if (d->negative)
    buf[o++] = '-';
  if (scale >= len) {
    buf[o++] = '0';
    buf[o++] = '.';
    for (int k = 0; k < scale - len; k++)
      buf[o++] = '0';
    for (int k = len - 1; k >= 0; k--)
      buf[o++] = digits[k];
  } else {
    for (int k = len - 1; k >= 0; k--) {
      buf[o++] = digits[k];
      if (k == scale && scale > 0)
        buf[o++] = '.';
    }
  }
  return mkCharLen(buf, o);
}

static SEXP uuid_string(const uint8_t b[16]) {
  static const char HEX[17] = "0123456789abcdef";
  char buf[36];
  int o = 0;
  for (int k = 0; k < 16; k++) {
    if (k == 4 || k == 6 || k == 8 || k == 10)
      buf[o++] = '-';
    buf[o++] = HEX[b[k] >> 4];
    buf[o++] = HEX[b[k] & 0x0f];
  }
  return mkCharLen(buf, 36);
}

/* Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's days_from_civil). */
static double days_from_civil(int y, int m, int d) {
  y -= m <= 2;
  const int era = (y >= 0 ? y : y - 399) / 400;
  const int yoe = y - era * 400;
  const int doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
  const int doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  return (double)era * 146097.0 + (double)doe - 719468.0;
}

static void two(char *p, unsigned v) {
  p[0] = (char)('0' + v / 10);
  p[1] = (char)('0' + v % 10);
}

static SEXP civil_string(const HyperTabularCivilDateTime *c) {
  char buf[32];
  unsigned y = c->date.year;
  buf[0] = (char)('0' + y / 1000 % 10);
  buf[1] = (char)('0' + y / 100 % 10);
  two(buf + 2, y % 100);
  buf[4] = '-';
  two(buf + 5, c->date.month);
  buf[7] = '-';
  two(buf + 8, c->date.day);
  buf[10] = ' ';
  uint64_t secs = c->nanos_of_day / 1000000000u;
  uint32_t frac = (uint32_t)(c->nanos_of_day % 1000000000u);
  two(buf + 11, (unsigned)(secs / 3600));
  buf[13] = ':';
  two(buf + 14, (unsigned)(secs / 60 % 60));
  buf[16] = ':';
  two(buf + 17, (unsigned)(secs % 60));
  int o = 19;
  if (frac) {
    buf[o++] = '.';
    for (uint32_t scale = 100000000u; scale && frac; scale /= 10) {
      buf[o++] = (char)('0' + frac / scale);
      frac %= scale;
    }
  }
  return mkCharLen(buf, o);
}

/* Row i cast: its value, read from the column buffer at v. */
static void row_ok(struct columns *c, R_xlen_t i, uint32_t door, const uint8_t *v) {
  c->ok[i] = TRUE;
  c->reason[i] = NA_INTEGER;
  c->offset[i] = NA_INTEGER;
  c->length[i] = NA_INTEGER;
  SET_STRING_ELT(c->raw, i, NA_STRING);
  switch (door) {
  case 1:
    c->lgl[i] = v[0] != 0;
    break;
  case 2:
    c->integer[i] = (int8_t)v[0];
    break;
  case 6:
    c->integer[i] = v[0];
    break;
  case 3: {
    int16_t x;
    memcpy(&x, v, sizeof x);
    c->integer[i] = x;
    break;
  }
  case 7: {
    uint16_t x;
    memcpy(&x, v, sizeof x);
    c->integer[i] = x;
    break;
  }
  case 4: {
    int32_t x;
    memcpy(&x, v, sizeof x);
    c->real[i] = (double)x;
    break;
  }
  case 8: {
    uint32_t x;
    memcpy(&x, v, sizeof x);
    c->real[i] = (double)x;
    break;
  }
  case 10: {
    float x;
    memcpy(&x, v, sizeof x);
    c->real[i] = (double)x;
    break;
  }
  case 11: {
    double x;
    memcpy(&x, v, sizeof x);
    c->real[i] = x;
    break;
  }
  case 5: {
    int64_t x;
    memcpy(&x, v, sizeof x);
    SET_STRING_ELT(c->strings, i, i64_string(x));
    break;
  }
  case 9: {
    uint64_t x;
    memcpy(&x, v, sizeof x);
    SET_STRING_ELT(c->strings, i, u64_string(x));
    break;
  }
  case 12:
    SET_STRING_ELT(c->strings, i, uuid_string(v));
    break;
  case 19: {
    HyperTabularDecimal d;
    memcpy(&d, v, sizeof d);
    SET_STRING_ELT(c->strings, i, decimal_string(&d));
    break;
  }
  case 13:
  case 14:
  case 22: {
    HyperTabularTimestamp t;
    memcpy(&t, v, sizeof t);
    /* Cut to the millisecond, as hypercast does: a double would round the last nanoseconds up. */
    c->real[i] = (double)t.seconds + (double)(t.nanos / 1000000) / 1e3;
    c->extra_real[0][i] = (double)t.seconds;
    c->extra_int[i] = t.nanos;
    break;
  }
  case 15:
  case 20: {
    HyperTabularDate d;
    memcpy(&d, v, sizeof d);
    c->real[i] = days_from_civil(d.year, d.month, d.day);
    break;
  }
  case 21: {
    HyperTabularCivilDateTime d;
    memcpy(&d, v, sizeof d);
    SET_STRING_ELT(c->strings, i, civil_string(&d));
    c->extra_real[0][i] = days_from_civil(d.date.year, d.date.month, d.date.day);
    c->extra_real[1][i] = (double)d.nanos_of_day;
    break;
  }
  case 16: {
    uint64_t nanos;
    memcpy(&nanos, v, sizeof nanos);
    c->real[i] = (double)(nanos / 1000000u) / 1e3;
    c->extra_real[0][i] = (double)nanos;
    break;
  }
  case 17: {
    HyperTabularDuration d;
    memcpy(&d, v, sizeof d);
    c->real[i] = (double)d.seconds + (double)(d.nanos / 1000000) / 1e3;
    c->extra_real[0][i] = (double)d.seconds;
    c->extra_int[i] = d.nanos;
    break;
  }
  default:
    break;
  }
}

/* ---- a batch ---- */

/*
 * Where a batch's text lives. A text value's span names the arena when flagged, and the
 * input (delimited text, from where the call started reading) or the shared strings (a
 * workbook) when not. A cell-table entry likewise, except that a delimited entry's flag says
 * the quoted cell has "" in it, to be unescaped from the input.
 */
struct texts {
  const uint8_t *base;
  size_t base_len;
  const uint8_t *arena;
  size_t arena_len;
  int delimited;
};

static const uint8_t *slice(const uint8_t *from, size_t from_len, uint32_t offset, uint32_t len,
                            size_t *n) {
  if ((uint64_t)offset + len > from_len) {
    *n = 0;
    return (const uint8_t *)"";
  }
  *n = len;
  return from + offset;
}

/* A text value's bytes. */
static const uint8_t *value_text(const struct texts *t, HyperTabularSpan s, size_t *n) {
  if (s.len & HyperTabularSpan_FLAG)
    return slice(t->arena, t->arena_len, s.offset, s.len & SPAN_LENGTH, n);
  return slice(t->base, t->base_len, s.offset, s.len, n);
}

/* A cell-table entry's bytes: the text a fault indexes. Unescaped text is R_alloc'd. */
static const uint8_t *cell_text(const struct texts *t, HyperTabularSpan s, size_t *n) {
  if (!(s.len & HyperTabularSpan_FLAG))
    return slice(t->base, t->base_len, s.offset, s.len, n);
  if (!t->delimited)
    return slice(t->arena, t->arena_len, s.offset, s.len & SPAN_LENGTH, n);
  size_t quoted_len;
  const uint8_t *quoted = slice(t->base, t->base_len, s.offset, s.len & SPAN_LENGTH, &quoted_len);
  uint8_t *out = (uint8_t *)R_alloc(quoted_len ? quoted_len : 1, 1);
  *n = hypertabular_delimited_unescape(quoted, quoted_len, out, quoted_len);
  return out;
}

/* Counts the elements a copy has made, and looks for an interrupt every INTERRUPT_EVERY. */
static void tick(size_t *ticks) {
  if (++*ticks >= INTERRUPT_EVERY) {
    *ticks = 0;
    R_CheckUserInterrupt();
  }
}

/* The verdict and note of row r of plan column k. */
static void verdict_of(const struct plan *p, size_t k, size_t r, HyperTabularCellVerdict *verdict,
                       HyperTabularCellNote *note) {
  memcpy(verdict, (const uint8_t *)p->table[k].verdicts + r * sizeof *verdict, sizeof *verdict);
  memcpy(note, (const uint8_t *)p->table[k].notes + r * sizeof *note, sizeof *note);
}

/*
 * The batch the core just wrote: a list of `columns` (one verdict's columns per plan column),
 * `line` (each row's line, or sheet row number) and `widths` (each row's cells before padding
 * or cutting, or NULL where the read keeps none). `cells` is the cell table, `per_row` entries a
 * row: a delimited row's entry for source column c is at c and its line is the last entry's len;
 * a workbook row's entry for plan column c is at c and its row number the last entry's offset.
 */
static SEXP batch(const struct plan *p, size_t rows, const HyperTabularSpan *cells, size_t per_row,
                  const struct texts *t) {
  size_t ticks = 0;
  SEXP out = PROTECT(allocVector(VECSXP, 3));
  SEXP names = PROTECT(allocVector(STRSXP, 3));
  SET_STRING_ELT(names, 0, mkChar("columns"));
  SET_STRING_ELT(names, 1, mkChar("line"));
  SET_STRING_ELT(names, 2, mkChar("widths"));
  setAttrib(out, R_NamesSymbol, names);
  SEXP cols = allocVector(VECSXP, (R_xlen_t)p->specs.count);
  SET_VECTOR_ELT(out, 0, cols);
  for (size_t k = 0; k < p->specs.count; k++) {
    uint32_t door = p->specs.with[k].door;
    size_t size = DOORS[door].size;
    const uint8_t *values = p->table[k].values;
    struct columns c;
    SET_VECTOR_ELT(cols, (R_xlen_t)k,
                   columns_alloc(&c, p->raws[k] ? BYTES_KIND : DOORS[door].kind, (R_xlen_t)rows));
    size_t entry = t->delimited ? p->specs.with[k].ordinal : k;
    for (size_t r = 0; r < rows; r++) {
      tick(&ticks);
      const void *vmax = vmaxget();
      HyperTabularCellVerdict verdict;
      HyperTabularCellNote note;
      verdict_of(p, k, r, &verdict, &note);
      c.detail[r] = note.detail ? (int)note.detail : NA_INTEGER;
      c.origin[r] = note.origin;
      const uint8_t *v = values + r * size;
      if (verdict.reason == 0 && door == TEXT_DOOR) {
        HyperTabularSpan s;
        memcpy(&s, v, sizeof s);
        size_t n;
        const uint8_t *b = value_text(t, s, &n);
        row_ok(&c, (R_xlen_t)r, door, v);
        if (c.bytes) {
          SET_VECTOR_ELT(c.bytes, (R_xlen_t)r, raw_of(b, n));
        } else {
          /* Text R can't hold (a NUL inside, or over INT_MAX bytes) is ok with an NA value: the
           * core accepted it, and raw = TRUE gives it exactly. */
          enum text_unit unit;
          SET_STRING_ELT(c.strings, (R_xlen_t)r, text_char(b, n, &unit));
        }
      } else if (verdict.reason == 0) {
        row_ok(&c, (R_xlen_t)r, door, v);
      } else if (verdict.reason <= 3) {
        size_t n;
        const uint8_t *b = cell_text(t, cells[r * per_row + entry], &n);
        row_fault(&c, (R_xlen_t)r, (int)verdict.reason, verdict.offset, verdict.len, b, n);
      } else {
        contract_violation();
      }
      vmaxset(vmax);
    }
  }
  SEXP line = allocVector(REALSXP, (R_xlen_t)rows);
  SET_VECTOR_ELT(out, 1, line);
  for (size_t r = 0; r < rows; r++) {
    HyperTabularSpan last = cells[r * per_row + per_row - 1];
    REAL(line)[r] = (double)(t->delimited ? last.len : last.offset);
  }
  if (p->widths) {
    SEXP widths = allocVector(REALSXP, (R_xlen_t)rows);
    SET_VECTOR_ELT(out, 2, widths);
    for (size_t r = 0; r < rows; r++)
      REAL(widths)[r] = p->widths[r];
  }
  UNPROTECT(2);
  return out;
}

/*
 * The batch as a sample takes it: per plan column, the text door's cells as raw vectors (the
 * text read, a refused cell's own text for the core's text door to refuse again, and nothing for
 * an empty cell: a blank, a token, padding or a formula with no result), and per UUID column a
 * raw vector of marks, 1 for a cell that door refused as malformed with no detail, which is a
 * workbook's typed cell.
 */
static SEXP sample_batch(const struct plan *p, size_t rows, const HyperTabularSpan *cells,
                         size_t per_row, const struct texts *t) {
  size_t ticks = 0;
  SEXP out = PROTECT(allocVector(VECSXP, (R_xlen_t)p->specs.count));
  for (size_t k = 0; k < p->specs.count; k++) {
    uint32_t door = p->specs.with[k].door;
    const uint8_t *values = p->table[k].values;
    size_t entry = t->delimited ? p->specs.with[k].ordinal : k;
    if (door == UUID_DOOR) {
      SEXP marks = allocVector(RAWSXP, (R_xlen_t)rows);
      SET_VECTOR_ELT(out, (R_xlen_t)k, marks);
      for (size_t r = 0; r < rows; r++) {
        HyperTabularCellVerdict verdict;
        HyperTabularCellNote note;
        verdict_of(p, k, r, &verdict, &note);
        RAW(marks)[r] = verdict.reason == 2 && note.detail == 0;
      }
      continue;
    }
    if (door != TEXT_DOOR)
      contract_violation();
    SEXP texts = allocVector(VECSXP, (R_xlen_t)rows);
    SET_VECTOR_ELT(out, (R_xlen_t)k, texts);
    for (size_t r = 0; r < rows; r++) {
      tick(&ticks);
      const void *vmax = vmaxget();
      HyperTabularCellVerdict verdict;
      HyperTabularCellNote note;
      verdict_of(p, k, r, &verdict, &note);
      size_t n = 0;
      const uint8_t *b = (const uint8_t *)"";
      if (verdict.reason == 0) {
        HyperTabularSpan s;
        memcpy(&s, values + r * sizeof s, sizeof s);
        b = value_text(t, s, &n);
      } else if (verdict.reason != 1) {
        b = cell_text(t, cells[r * per_row + entry], &n);
      }
      SET_VECTOR_ELT(texts, (R_xlen_t)r, raw_of(b, n));
      vmaxset(vmax);
    }
  }
  UNPROTECT(1);
  return out;
}

/* A failure as R gets it: code, line, record, byte, expected, found. */
static SEXP failure_vector(const HyperTabularFailure *f) {
  SEXP v = allocVector(REALSXP, 6);
  double *d = REAL(v);
  d[0] = f->code;
  d[1] = f->line;
  d[2] = (double)f->record;
  d[3] = (double)f->byte;
  d[4] = f->expected;
  d[5] = f->found;
  return v;
}

/* What a read call hands R: status (0 rows, 1 more input wanted, 2 end, 3 failure), the
 * payload (a batch, the header's names), and the failure. */
static SEXP outcome(int status, SEXP payload, SEXP failure) {
  PROTECT(payload);
  PROTECT(failure);
  SEXP out = PROTECT(allocVector(VECSXP, 3));
  SEXP names = PROTECT(allocVector(STRSXP, 3));
  SET_STRING_ELT(names, 0, mkChar("status"));
  SET_STRING_ELT(names, 1, mkChar("payload"));
  SET_STRING_ELT(names, 2, mkChar("failure"));
  setAttrib(out, R_NamesSymbol, names);
  SET_VECTOR_ELT(out, 0, ScalarInteger(status));
  SET_VECTOR_ELT(out, 1, payload);
  SET_VECTOR_ELT(out, 2, failure);
  UNPROTECT(4);
  return out;
}

enum { STATUS_ROWS, STATUS_MORE, STATUS_END, STATUS_FAILURE };

/* Names as R strings: NA for one holding a NUL byte. */
static SEXP names_vector(const HyperTabularSpan *spans, size_t count, const struct texts *t) {
  SEXP v = PROTECT(allocVector(STRSXP, (R_xlen_t)count));
  for (size_t i = 0; i < count; i++) {
    size_t n;
    const uint8_t *b = value_text(t, spans[i], &n);
    enum text_unit unit;
    SET_STRING_ELT(v, (R_xlen_t)i, text_char(b, n, &unit));
  }
  UNPROTECT(1);
  return v;
}

/* ---- what the core says before any read ---- */

/* The sizes of HyperCast's layouts this core reads: a number format's (most, least) and the date
 * options' (most, least), for R to name both packages' versions when the two disagree. */
SEXP C_layout(void) {
  SEXP v = allocVector(INTSXP, 4);
  INTEGER(v)[0] = HyperTabularRawNumFormat_SIZE;
  INTEGER(v)[1] = HyperTabularRawNumFormat_MIN_SIZE;
  INTEGER(v)[2] = HyperTabularRawDateOptions_SIZE;
  INTEGER(v)[3] = HyperTabularRawDateOptions_MIN_SIZE;
  return v;
}

/*
 * hypertabular_batch_check, before anything is allocated for a read: c(code, needed, limit,
 * buffer_bytes, sample_rows, width, refused). `specs` and `lists` are a plan as for bind, or
 * empty for a read whose plan comes later. The specs are read in place, held by R for the call.
 */
SEXP C_batch_check(SEXP specs, SEXP lists, SEXP batch_rows, SEXP buffer_bytes, SEXP flags) {
  size_t count = specs_count(specs, lists);
  HyperTabularColumnSpecWith *with =
      (HyperTabularColumnSpecWith *)R_alloc(count ? count : 1, sizeof *with);
  const uint8_t *b = RAW(specs);
  for (size_t i = 0; i < count; i++) {
    memset(&with[i], 0, sizeof with[i]);
    with[i].size = HyperTabularColumnSpecWith_SIZE;
    with[i].ordinal = le32(b + i * PACKED_SPEC);
    with[i].door = le32(b + i * PACKED_SPEC + 4);
    with[i].param = le32(b + i * PACKED_SPEC + 8);
  }
  HyperTabularRawBatchCheck out;
  memset(&out, 0, sizeof out);
  int32_t code =
      hypertabular_batch_check(count ? with : NULL, count, count_u64(batch_rows),
                               count_u64(buffer_bytes), (uint32_t)asInteger(flags), &out);
  if (code != HYPERTABULAR_OK && code != HYPERTABULAR_ERR_BATCH)
    contract_violation();
  SEXP v = PROTECT(allocVector(REALSXP, 7));
  double *d = REAL(v);
  d[0] = code;
  d[1] = (double)out.needed;
  d[2] = (double)out.limit;
  d[3] = (double)out.buffer_bytes;
  d[4] = (double)out.sample_rows;
  d[5] = (double)out.width;
  d[6] = out.refused;
  /* needed and limit exactly, for a message: a double holds neither past 2^53. */
  SEXP exact = PROTECT(allocVector(STRSXP, 2));
  SET_STRING_ELT(exact, 0, u64_string(out.needed));
  SET_STRING_ELT(exact, 1, u64_string(out.limit));
  setAttrib(v, install("exact"), exact);
  UNPROTECT(2);
  return v;
}

/*
 * The first plan column the core refuses, counted from 1, or 0: a fill of no rows judges a
 * column's door, what it declares, its format, its date options, its vocabulary and its tokens,
 * and writes nothing. It answers on a delimited state of its own, so a sheet's plan is asked the
 * same way. Everything it uses is R_alloc'd, released however the call ends.
 */
SEXP C_judge(SEXP specs, SEXP lists) {
  size_t count = specs_count(specs, lists);
  HyperTabularDelimitedState *state =
      (HyperTabularDelimitedState *)R_alloc(1, hypertabular_delimited_state_size());
  HyperTabularRawDelimitedOptions options;
  memset(&options, 0, sizeof options);
  options.size = HyperTabularRawDelimitedOptions_SIZE;
  options.separator = ',';
  options.quoting = 1;
  options.skip_blank_lines = 1;
  if (hypertabular_delimited_init_with(state, &options) != HYPERTABULAR_OK)
    contract_violation();
  uint64_t value[4] = {0};
  HyperTabularCellVerdict verdict;
  HyperTabularColumnBufferWith table;
  memset(&table, 0, sizeof table);
  table.size = HyperTabularColumnBufferWith_SIZE;
  table.values = value;
  table.verdicts = &verdict;
  HyperTabularBuffersWith buffers;
  memset(&buffers, 0, sizeof buffers);
  buffers.size = HyperTabularBuffersWith_SIZE;
  HyperTabularFilled out;
  const uint8_t *b = RAW(specs);
  for (size_t i = 0; i < count; i++) {
    const uint8_t *p = b + i * PACKED_SPEC;
    HyperTabularColumnSpecWith spec;
    memset(&spec, 0, sizeof spec);
    spec.size = HyperTabularColumnSpecWith_SIZE;
    spec.ordinal = le32(p);
    spec.door = le32(p + 4);
    spec.param = le32(p + 8);
    format_from(p + 12, &spec.format);
    date_from(p + 12 + HyperTabularRawNumFormat_SIZE, &spec.date);
    SEXP entry = VECTOR_ELT(lists, (R_xlen_t)i);
    if (TYPEOF(entry) != VECSXP || XLENGTH(entry) != 2)
      Rf_error("hypertabular: lists must hold list(months, tokens) a column");
    for (int k = 0; k < 2; k++) {
      SEXP list = VECTOR_ELT(entry, k);
      const uint8_t *ptr = NULL;
      uintptr_t len = 0;
      if (list != R_NilValue) {
        if (TYPEOF(list) != RAWSXP)
          Rf_error("hypertabular: a month vocabulary or a token list is a raw vector");
        len = (uintptr_t)XLENGTH(list);
        uint8_t *copy = (uint8_t *)R_alloc(len ? len : 1, 1);
        if (len)
          memcpy(copy, RAW(list), len);
        ptr = copy;
      }
      if (k == 0) {
        spec.months = ptr;
        spec.months_len = len;
      } else {
        spec.tokens = ptr;
        spec.tokens_len = len;
      }
    }
    int32_t code =
        hypertabular_delimited_fill_with(state, NULL, 0, 0, &spec, &table, 1, 0, &buffers, &out);
    if (code == HYPERTABULAR_ERR_CONTRACT)
      return ScalarInteger((int)i + 1);
    if (code != HYPERTABULAR_OK)
      contract_violation();
  }
  return ScalarInteger(0);
}

/* A range such as "A5:F200" as the core reads it: c(first_row, first_column, last_row,
 * last_column), or NULL for text that isn't one. */
SEXP C_range_parse(SEXP text) {
  if (TYPEOF(text) != RAWSXP)
    Rf_error("hypertabular: a range is a raw vector");
  HyperTabularRawRange out;
  if (hypertabular_range_parse(RAW(text), (uintptr_t)XLENGTH(text), &out) != HYPERTABULAR_OK)
    return R_NilValue;
  SEXP v = allocVector(REALSXP, 4);
  REAL(v)[0] = out.first_row;
  REAL(v)[1] = out.first_column;
  REAL(v)[2] = out.last_row;
  REAL(v)[3] = out.last_column;
  return v;
}

/*
 * What hypertabular_faults lists of a read's rows, from the verdict columns R kept:
 * list(row, plan_column, reason, detail, origin, expected, found), row and plan column from 0,
 * plan column -1 for a row-level entry. `columns` holds, per plan column, list(reason, detail,
 * origin) as integer vectors (NA a 0); `widths` each row's cells or NULL. Pure: it holds no
 * reader's state, so an interrupt between its calls changes nothing.
 */
SEXP C_faults(SEXP columns, SEXP rows_arg, SEXP widths, SEXP expected, SEXP long_rows) {
  if (TYPEOF(columns) != VECSXP)
    Rf_error("hypertabular: columns must be a list");
  size_t count = (size_t)XLENGTH(columns);
  size_t rows = count_size(rows_arg);
  HyperTabularColumnBufferWith *table =
      (HyperTabularColumnBufferWith *)R_alloc(count ? count : 1, sizeof *table);
  for (size_t k = 0; k < count; k++) {
    SEXP c = VECTOR_ELT(columns, (R_xlen_t)k);
    if (TYPEOF(c) != VECSXP || XLENGTH(c) != 3)
      Rf_error("hypertabular: a column is list(reason, detail, origin)");
    SEXP reason = VECTOR_ELT(c, 0), detail = VECTOR_ELT(c, 1), origin = VECTOR_ELT(c, 2);
    if (TYPEOF(reason) != INTSXP || TYPEOF(detail) != INTSXP || TYPEOF(origin) != INTSXP ||
        (size_t)XLENGTH(reason) != rows || (size_t)XLENGTH(detail) != rows ||
        (size_t)XLENGTH(origin) != rows)
      Rf_error("hypertabular: a column's codes are integer vectors of the read's rows");
    HyperTabularCellVerdict *verdicts =
        (HyperTabularCellVerdict *)R_alloc(rows ? rows : 1, sizeof *verdicts);
    HyperTabularCellNote *notes = (HyperTabularCellNote *)R_alloc(rows ? rows : 1, sizeof *notes);
    for (size_t r = 0; r < rows; r++) {
      int code = INTEGER(reason)[r], d = INTEGER(detail)[r], o = INTEGER(origin)[r];
      verdicts[r].offset = 0;
      verdicts[r].len = 0;
      verdicts[r].reason = code == NA_INTEGER ? 0 : (uint32_t)code;
      notes[r].detail = d == NA_INTEGER ? 0 : (uint16_t)d;
      notes[r].origin = o == NA_INTEGER ? 0 : (uint16_t)o;
    }
    table[k].size = HyperTabularColumnBufferWith_SIZE;
    table[k].values = NULL;
    table[k].verdicts = verdicts;
    table[k].notes = notes;
  }
  uint32_t *row_widths = NULL;
  if (widths != R_NilValue) {
    if (TYPEOF(widths) != REALSXP || (size_t)XLENGTH(widths) != rows)
      Rf_error("hypertabular: widths must be a double vector of the read's rows");
    row_widths = (uint32_t *)R_alloc(rows ? rows : 1, sizeof *row_widths);
    for (size_t r = 0; r < rows; r++)
      row_widths[r] = (uint32_t)REAL(widths)[r];
  }
  enum { CAP = 256 };
  HyperTabularRawProblem chunk[CAP];
  uint32_t policy = (uint32_t)asInteger(long_rows);
  uint32_t width = (uint32_t)asReal(expected);
  /* Twice: once to count, once to copy, so the R vectors are allocated once. */
  size_t total = 0;
  SEXP out = PROTECT(allocVector(VECSXP, 7));
  for (int pass = 0; pass < 2; pass++) {
    uint64_t cursor = 0;
    size_t at = 0;
    if (pass == 1) {
      for (int f = 0; f < 7; f++)
        SET_VECTOR_ELT(out, f, allocVector(REALSXP, (R_xlen_t)total));
    }
    for (;;) {
      uintptr_t written = 0;
      int32_t code = hypertabular_faults(count ? table : NULL, count, rows, row_widths, width,
                                         policy, &cursor, chunk, CAP, &written);
      if (code != HYPERTABULAR_OK)
        contract_violation();
      if (pass == 0) {
        total = add(total, written);
      } else {
        for (uintptr_t e = 0; e < written && at < total; e++, at++) {
          const HyperTabularRawProblem *q = &chunk[e];
          REAL(VECTOR_ELT(out, 0))[at] = q->row;
          REAL(VECTOR_ELT(out, 1))
          [at] = q->plan_column == HyperTabularRawProblem_ROW_LEVEL ? -1 : (double)q->plan_column;
          REAL(VECTOR_ELT(out, 2))[at] = q->reason;
          REAL(VECTOR_ELT(out, 3))[at] = q->detail;
          REAL(VECTOR_ELT(out, 4))[at] = q->origin;
          REAL(VECTOR_ELT(out, 5))[at] = q->expected;
          REAL(VECTOR_ELT(out, 6))[at] = q->found;
        }
      }
      if (written < CAP)
        break;
      R_CheckUserInterrupt();
    }
  }
  UNPROTECT(1);
  return out;
}

/*
 * What hypertabular_suggest makes of a sample: list(door, param, status, matched, sampled), a
 * value a column, or NULL where the core refuses it. `cells` holds, per column, a list of the
 * rows' raw vectors; `typed`, NULL or per column a raw vector of a mark a row; `formats`, 92 bytes
 * a column, a size of 0 declaring nothing. R stops a sample before its bytes outgrow a span.
 */
SEXP C_suggest(SEXP cells, SEXP typed, SEXP rows_arg, SEXP formats) {
  if (TYPEOF(cells) != VECSXP || TYPEOF(formats) != RAWSXP)
    Rf_error("hypertabular: a sample is a list of columns and their formats");
  size_t columns = (size_t)XLENGTH(cells);
  size_t rows = count_size(rows_arg);
  if ((size_t)XLENGTH(formats) != mul(columns, HyperTabularRawNumFormat_SIZE))
    Rf_error("hypertabular: formats must hold 92 bytes a column");
  if (typed != R_NilValue && (TYPEOF(typed) != VECSXP || (size_t)XLENGTH(typed) != columns))
    Rf_error("hypertabular: typed must hold a mark vector a column");
  size_t cell_count = mul(rows, columns);
  size_t bytes = 0;
  for (size_t k = 0; k < columns; k++) {
    SEXP column = VECTOR_ELT(cells, (R_xlen_t)k);
    if (TYPEOF(column) != VECSXP || (size_t)XLENGTH(column) != rows)
      Rf_error("hypertabular: each sample column holds a raw vector a row");
    for (size_t r = 0; r < rows; r++) {
      SEXP cell = VECTOR_ELT(column, (R_xlen_t)r);
      if (TYPEOF(cell) != RAWSXP)
        Rf_error("hypertabular: a sample cell is a raw vector");
      bytes = add(bytes, (size_t)XLENGTH(cell));
    }
  }
  if ((uint64_t)bytes > UINT32_MAX)
    Rf_error("hypertabular: a sample past what a span can address");
  uint8_t *input = (uint8_t *)R_alloc(bytes ? bytes : 1, 1);
  HyperTabularSpan *spans = (HyperTabularSpan *)R_alloc(cell_count ? cell_count : 1, sizeof *spans);
  uint8_t *marks = typed == R_NilValue ? NULL : (uint8_t *)R_alloc(cell_count ? cell_count : 1, 1);
  size_t at = 0;
  for (size_t k = 0; k < columns; k++) {
    SEXP column = VECTOR_ELT(cells, (R_xlen_t)k);
    for (size_t r = 0; r < rows; r++) {
      SEXP cell = VECTOR_ELT(column, (R_xlen_t)r);
      size_t n = (size_t)XLENGTH(cell);
      spans[k * rows + r].offset = (uint32_t)at;
      spans[k * rows + r].len = (uint32_t)n;
      if (n)
        memcpy(input + at, RAW(cell), n);
      at += n;
    }
    if (marks) {
      SEXP m = VECTOR_ELT(typed, (R_xlen_t)k);
      if (TYPEOF(m) != RAWSXP || (size_t)XLENGTH(m) != rows)
        Rf_error("hypertabular: a column's marks are a raw vector of its rows");
      if (rows)
        memcpy(marks + k * rows, RAW(m), rows);
    }
  }
  HyperTabularRawNumFormat *declared =
      (HyperTabularRawNumFormat *)R_alloc(columns ? columns : 1, sizeof *declared);
  for (size_t k = 0; k < columns; k++)
    format_from(RAW(formats) + k * HyperTabularRawNumFormat_SIZE, &declared[k]);
  HyperTabularRawSuggestion *out =
      (HyperTabularRawSuggestion *)R_alloc(columns ? columns : 1, sizeof *out);
  int32_t code =
      hypertabular_suggest(bytes ? input : NULL, bytes, NULL, 0, cell_count ? spans : NULL, marks,
                           rows, columns, columns ? declared : NULL, out);
  if (code != HYPERTABULAR_OK)
    return R_NilValue;
  SEXP v = PROTECT(allocVector(VECSXP, 5));
  for (int f = 0; f < 5; f++)
    SET_VECTOR_ELT(v, f, allocVector(REALSXP, (R_xlen_t)columns));
  for (size_t k = 0; k < columns; k++) {
    REAL(VECTOR_ELT(v, 0))[k] = out[k].door;
    REAL(VECTOR_ELT(v, 1))[k] = out[k].param;
    REAL(VECTOR_ELT(v, 2))[k] = out[k].status;
    REAL(VECTOR_ELT(v, 3))[k] = out[k].matched;
    REAL(VECTOR_ELT(v, 4))[k] = out[k].sampled;
  }
  UNPROTECT(1);
  return v;
}

/* ---- delimited text ---- */

/* The core's MAX_ROW_BYTES (rust/src/kernel/size.rs): the largest unfinished record a reader
 * holds before it reports the record as too long. */
#define MAX_ROW_BYTES ((size_t)1 << 30)
#define ARENA_BYTES 4096
#define NAMES 64
/* The core's CELL_BUDGET (rust/src/kernel/size.rs): the cell-table entries a reader holds at
 * most, unless one row takes more. */
#define CELL_BUDGET ((size_t)4096 * 257)
/* The synthetic failure code of a row past MAX_ROW_BYTES, as the other bindings number it. */
#define ROW_TOO_LONG 3

struct delimited {
  HyperTabularDelimitedState *state;
  /* The UTF-8 the core reads: what the source gave, or what a declared encoding decoded to. */
  uint8_t *input;
  size_t input_cap, input_len, start;
  int eof;
  /* A declared encoding: its code, the original bytes not yet decoded, how many were, and the
   * original offset of a unit with no character once one has been met. */
  uint32_t encoding;
  uint8_t *raw;
  size_t raw_cap, raw_at, raw_len;
  uint64_t raw_done;
  int raw_eof, failed, mended;
  uint64_t failed_at;
  uint8_t *tokens;
  size_t tokens_len;
  int has_tokens;
  uint8_t *arena;
  size_t arena_cap;
  HyperTabularSpan *names;
  size_t names_cap;
  HyperTabularSpan *cells;
  size_t cells_cap, per_row;
  struct plan plan;
  int cramped, stingy, widths;
  HyperTabularFilled out;
  /* A batch the core wrote and R doesn't have yet: its rows, where its input started, how long
   * the window was and how much of the arena it used. */
  size_t held_rows, held_start, held_len, held_arena;
  /* The rows R has acknowledged, the next batch's first row counting from here, and the rows of
   * the last acknowledgement, which C_delimited_unack takes back. */
  double delivered;
  size_t acked;
  /* A connection read is under way: set before readBin is called, cleared once its bytes are
   * fed. Left set, an interrupt or an error came out of readBin; `whole` says whether that
   * connection can have taken bytes it didn't hand back (one that reads in one step can't). */
  int pulling, whole;
};

static void delimited_free(struct delimited *d) {
  release(d->state);
  release(d->input);
  release(d->raw);
  release(d->tokens);
  release(d->arena);
  release(d->names);
  release(d->cells);
  plan_free(&d->plan);
  R_Free(d);
}

static void delimited_finalize(SEXP ptr) {
  struct delimited *d = R_ExternalPtrAddr(ptr);
  if (d) {
    R_ClearExternalPtr(ptr);
    delimited_free(d);
  }
}

static struct delimited *delimited_of(SEXP ptr) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_delimited"))
    Rf_error("hypertabular: not a delimited reader");
  struct delimited *d = R_ExternalPtrAddr(ptr);
  if (!d)
    Rf_error("hypertabular: the reader's native memory is gone (it was closed, or a reader does "
             "not survive saveRDS() or a new session)");
  return d;
}

/*
 * options: RawDelimitedOptions as R packed it, 24 bytes little-endian; tokens: the read's token
 * list as the core takes it, or NULL for none; encoding: 0 for UTF-8, else the core's code. The
 * core refuses what it can't honour, and nothing is allocated past that.
 */
SEXP C_delimited_new(SEXP options, SEXP tokens, SEXP encoding, SEXP stingy) {
  if (TYPEOF(options) != RAWSXP || XLENGTH(options) != HyperTabularRawDelimitedOptions_SIZE)
    Rf_error("hypertabular: options must be 24 bytes");
  if (tokens != R_NilValue && TYPEOF(tokens) != RAWSXP)
    Rf_error("hypertabular: tokens must be a raw vector or NULL");
  const uint8_t *o = RAW(options);
  HyperTabularRawDelimitedOptions raw;
  memset(&raw, 0, sizeof raw);
  raw.size = le32(o);
  raw.separator = o[4];
  raw.quoting = o[5];
  raw.skip_blank_lines = o[6];
  raw.engine = o[7];
  raw.skip = le32(o + 8);
  raw.header_row = le32(o + 12);
  raw.short_rows = le32(o + 16);
  raw.long_rows = le32(o + 20);
  /* The pointer and its finalizer first, so the block is never held by nothing. */
  SEXP ptr = PROTECT(R_MakeExternalPtr(NULL, install("hypertabular_delimited"), R_NilValue));
  R_RegisterCFinalizerEx(ptr, delimited_finalize, TRUE);
  struct delimited *d = R_Calloc(1, struct delimited);
  R_SetExternalPtrAddr(ptr, d);
  d->stingy = asLogical(stingy) == TRUE;
  d->encoding = (uint32_t)asInteger(encoding);
  d->widths = raw.long_rows == HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT;
  d->state = block(hypertabular_delimited_state_size());
  d->arena_cap = d->stingy ? 1 : ARENA_BYTES;
  d->arena = block(d->arena_cap);
  d->names_cap = d->stingy ? 1 : NAMES;
  d->names = block(mul(d->names_cap, sizeof(HyperTabularSpan)));
  d->input = block(0);
  d->raw = block(0);
  if (tokens != R_NilValue) {
    d->tokens_len = (size_t)XLENGTH(tokens);
    d->tokens = block(d->tokens_len);
    if (d->tokens_len)
      memcpy(d->tokens, RAW(tokens), d->tokens_len);
    d->has_tokens = 1;
  }
  if (hypertabular_delimited_init_with(d->state, &raw) != HYPERTABULAR_OK)
    Rf_error("hypertabular: the core refused the dialect");
  UNPROTECT(1);
  return ptr;
}

/* Makes room for `more` bytes after what the input holds unread, moving that to the front. */
static void input_room(struct delimited *d, size_t more) {
  size_t pending = d->input_len - d->start;
  if (d->start) {
    memmove(d->input, d->input + d->start, pending);
    d->start = 0;
    d->input_len = pending;
  }
  size_t needed = add(pending, more);
  if (needed > d->input_cap) {
    size_t cap = d->input_cap > SIZE_MAX / 2 ? needed
                 : d->input_cap * 2 > needed ? d->input_cap * 2
                                             : needed;
    d->input = grown_block(d->input, cap);
    d->input_cap = cap;
  }
}

/* Decodes what the source has given, as far as whole characters go, ahead of the fill. A unit
 * with no character stops it there, its original offset kept for the failure the fill raises
 * once every intact row ahead of it is delivered. */
static void decode(struct delimited *d) {
  while (!d->failed) {
    size_t pending = d->raw_len - d->raw_at;
    if (!pending && !d->raw_eof)
      break;
    /* A unit of one or two bytes is at most three of UTF-8, and a surrogate pair four. */
    input_room(d, add(mul(pending, 3), 4));
    HyperTabularRawTranscoded progress;
    memset(&progress, 0, sizeof progress);
    if (hypertabular_transcode(d->encoding, d->raw + d->raw_at, pending, d->raw_eof ? 1 : 0,
                               d->input + d->input_len, d->input_cap - d->input_len,
                               &progress) != HYPERTABULAR_OK)
      contract_violation();
    if (progress.failed) {
      d->failed = 1;
      d->failed_at = d->raw_done + progress.failure_at;
    }
    d->raw_at += (size_t)progress.consumed;
    d->raw_done += progress.consumed;
    d->input_len += (size_t)progress.written;
    if (!progress.consumed)
      break;
  }
  d->eof = d->raw_eof && !d->failed && d->raw_at == d->raw_len;
}

/* Appends `chunk` to what the reader holds unread, or with NULL marks the input's end. Returns
 * the bytes of UTF-8 now held unread. */
/* Takes `n` bytes of input, or its end when `bytes` is NULL. */
static void feed_bytes(struct delimited *d, const uint8_t *bytes, size_t n) {
  if (!d->encoding) {
    if (!bytes) {
      d->eof = 1;
    } else {
      input_room(d, n);
      if (n)
        memcpy(d->input + d->input_len, bytes, n);
      d->input_len += n;
    }
  } else {
    if (!bytes) {
      d->raw_eof = 1;
    } else if (!d->failed) {
      size_t left = d->raw_len - d->raw_at;
      if (d->raw_at) {
        memmove(d->raw, d->raw + d->raw_at, left);
        d->raw_at = 0;
        d->raw_len = left;
      }
      size_t needed = add(left, n);
      if (needed > d->raw_cap) {
        d->raw = grown_block(d->raw, needed);
        d->raw_cap = needed;
      }
      if (n)
        memcpy(d->raw + d->raw_len, bytes, n);
      d->raw_len += n;
    }
    decode(d);
  }
}

/* A raw vector's bytes, or with NULL the input's end. Returns the bytes held unread. */
SEXP C_delimited_feed(SEXP ptr, SEXP chunk) {
  struct delimited *d = delimited_of(ptr);
  if (chunk != R_NilValue && TYPEOF(chunk) != RAWSXP)
    Rf_error("hypertabular: a chunk must be a raw vector");
  feed_bytes(d, chunk == R_NilValue ? NULL : RAW(chunk),
             chunk == R_NilValue ? 0 : (size_t)XLENGTH(chunk));
  return ScalarReal((double)(d->input_len - d->start));
}

/* Whether the input's end has been fed. */
static int fed_to_end(const struct delimited *d) { return d->encoding ? d->raw_eof : d->eof; }

/*
 * Reads the next chunk of `con` and feeds it, in one call: `readBin(con, "raw", n)` is evaluated
 * in `env` (so a trace on readBin there sees it), and from its return to the feed no R code runs,
 * so no interrupt can drop a chunk between the two. `n` is the larger of `chunk` and what the
 * reader already holds unread. Returns TRUE once the connection's end has been fed.
 *
 * An interrupt or an error out of readBin leaves `pulling` set. Through a connection that reads
 * in one step (`whole`), that took nothing, and the next pull reads on. Through any other, it may
 * have taken bytes R never handed back, so every later pull returns NA, which R refuses.
 */
SEXP C_delimited_pull(SEXP ptr, SEXP con, SEXP chunk, SEXP whole, SEXP env) {
  struct delimited *d = delimited_of(ptr);
  if (d->pulling && !d->whole)
    return ScalarLogical(NA_LOGICAL);
  if (fed_to_end(d))
    return ScalarLogical(TRUE);
  if (TYPEOF(env) != ENVSXP)
    Rf_error("hypertabular: env must be an environment");
  size_t unread = d->input_len - d->start;
  double want = asReal(chunk);
  if ((double)unread > want)
    want = (double)unread;
  if (want > INT_MAX)
    want = INT_MAX;
  if (!(want >= 1))
    want = 1;
  /* Each argument held as the next is made: an unheld one can be collected by that allocation. */
  SEXP what = PROTECT(mkString("raw"));
  SEXP n = PROTECT(ScalarReal(want));
  SEXP call = PROTECT(lang4(install("readBin"), con, what, n));
  d->whole = asLogical(whole) == TRUE;
  d->pulling = 1;
  SEXP got = PROTECT(eval(call, env));
  if (TYPEOF(got) != RAWSXP)
    Rf_error("hypertabular: readBin returned no raw vector");
  /* From here to the return, C only. */
  int end = XLENGTH(got) == 0;
  feed_bytes(d, end ? NULL : RAW(got), (size_t)XLENGTH(got));
  d->pulling = 0;
  UNPROTECT(4);
  return ScalarLogical(end);
}

static void delimited_window(const struct delimited *d, size_t *len, uint32_t *last) {
  *len = d->input_len - d->start;
  *last = d->eof ? 1 : 0;
  if (*len > MAX_ROW_BYTES) {
    *len = MAX_ROW_BYTES;
    *last = 0;
  }
}

/* The arena grown to at least `needed`, at least doubled. The old one is released only once the
 * new one is had, so a refused allocation leaves the reader as it was. */
static void grow_arena(uint8_t **arena, size_t *cap, uint64_t needed) {
  if (needed > SIZE_MAX)
    too_large();
  size_t want = (size_t)needed > mul(*cap, 2) ? (size_t)needed : *cap * 2;
  uint8_t *fresh = block(want);
  release(*arena);
  *arena = fresh;
  *cap = want;
}

static SEXP failure_at(uint32_t code, const struct delimited *d, uint64_t byte) {
  HyperTabularFailure f;
  memset(&f, 0, sizeof f);
  f.code = code;
  f.line = d->state->line;
  f.record = d->state->records;
  f.byte = byte;
  return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&f));
}

/*
 * The core wants more input. A declared encoding's unit with no character is raised now, every
 * intact row ahead of it having been delivered: only a `\r` the core is holding can still finish
 * a record, and since nothing is coming to say whether a `\n` follows it, one is given, once.
 * Otherwise more is refused once the unfinished record holds MAX_ROW_BYTES. NULL to try again.
 */
static SEXP delimited_more(struct delimited *d) {
  size_t pending = d->input_len - d->start;
  if (d->failed) {
    if (!d->mended && pending && d->input[d->input_len - 1] == '\r') {
      input_room(d, 1);
      d->input[d->input_len++] = '\n';
      d->mended = 1;
      return NULL;
    }
    return failure_at(HyperTabularFailure_ENCODING, d, d->failed_at);
  }
  if (pending >= MAX_ROW_BYTES || d->eof)
    return failure_at(ROW_TOO_LONG, d, d->state->offset);
  return outcome(STATUS_MORE, R_NilValue, R_NilValue);
}

/* Reads the header record: its names, or the end (no record at all), more input wanted, or a
 * failure. */
SEXP C_delimited_header(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  for (;;) {
    R_CheckUserInterrupt();
    size_t len;
    uint32_t last;
    delimited_window(d, &len, &last);
    int32_t code = hypertabular_delimited_header(d->state, d->input + d->start, len, last, d->names,
                                                 d->names_cap, d->arena, d->arena_cap, &d->out);
    switch (code) {
    case HYPERTABULAR_OK: {
      size_t consumed = (size_t)d->out.consumed;
      if (d->out.rows) {
        /* The names are R's before the reader moves past them. */
        struct texts t = {d->input + d->start, len, d->arena, d->arena_cap, 1};
        SEXP names = PROTECT(names_vector(d->names, (size_t)d->out.rows, &t));
        d->start += consumed;
        SEXP result = outcome(STATUS_ROWS, names, R_NilValue);
        UNPROTECT(1);
        return result;
      }
      d->start += consumed;
      if (last && (consumed == len || consumed == 0))
        return outcome(STATUS_END, allocVector(STRSXP, 0), R_NilValue);
      if (consumed == 0) {
        SEXP more = delimited_more(d);
        if (more)
          return more;
      }
      break;
    }
    case HYPERTABULAR_ERR_STRUCTURE:
      return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&d->out.failure));
    case HYPERTABULAR_ERR_CELLS: {
      HyperTabularSpan *fresh = block(mul((size_t)d->out.needed, sizeof(HyperTabularSpan)));
      release(d->names);
      d->names = fresh;
      d->names_cap = (size_t)d->out.needed;
      break;
    }
    case HYPERTABULAR_ERR_ARENA:
      grow_arena(&d->arena, &d->arena_cap, d->out.needed);
      break;
    default:
      contract_violation();
    }
  }
}

static size_t cell_rows(size_t per_row, size_t batch_rows, int stingy) {
  if (stingy)
    return 1;
  size_t rows = CELL_BUDGET / (per_row ? per_row : 1);
  return rows < 1 ? 1 : rows > batch_rows ? batch_rows : rows;
}

static void grow_cells(struct delimited *d, size_t per_row) {
  size_t cap = mul(per_row, cell_rows(per_row, d->plan.batch_rows, d->stingy));
  HyperTabularSpan *fresh = block(mul(cap, sizeof(HyperTabularSpan)));
  release(d->cells);
  d->cells = fresh;
  d->cells_cap = cap;
  d->per_row = per_row;
}

/* Takes the plan: `specs` and `lists` packed, `raws` which text columns come back as raw
 * vectors, `per_row` the widest ordinal it reads plus two. R asked the core about the sizes. */
SEXP C_delimited_bind(SEXP ptr, SEXP specs, SEXP lists, SEXP raws, SEXP batch_rows, SEXP per_row) {
  struct delimited *d = delimited_of(ptr);
  /* R binds once; a plan here that R never recorded (an interrupt landed before it could) is
   * replaced. No fill has read through it: R reads only once it has its plan. */
  if (d->plan.bound)
    plan_free(&d->plan);
  plan_bind(&d->plan, specs, lists, raws, count_size(batch_rows), d->widths);
  d->plan.bound = 0;
  grow_cells(d, count_size(per_row));
  d->plan.bound = 1;
  return R_NilValue;
}

/* Hands R the held batch. It stays held until R says it has made what it makes of it
 * (C_delimited_ack), so an interrupt before then hands the same batch over again. */
static SEXP delimited_deliver(struct delimited *d, int sample) {
  struct texts t = {d->input + d->held_start, d->held_len, d->arena, d->held_arena, 1};
  SEXP b = PROTECT(sample ? sample_batch(&d->plan, d->held_rows, d->cells, d->per_row, &t)
                          : batch(&d->plan, d->held_rows, d->cells, d->per_row, &t));
  SEXP result = PROTECT(outcome(STATUS_ROWS, b, R_NilValue));
  SEXP first = PROTECT(ScalarReal(d->delivered));
  setAttrib(result, install("first"), first);
  UNPROTECT(3);
  return result;
}

/* R has the batch it was last handed: its rows are counted and the next fill reads on, in one
 * step no interrupt can split. */
SEXP C_delimited_ack(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  d->delivered += (double)d->held_rows;
  d->acked = d->held_rows;
  d->held_rows = 0;
  return R_NilValue;
}

/* R acknowledged the last batch but never returned it: an interrupt landed in between. The batch
 * is held again, its rows uncounted, for the next fill to hand over. Its input, cells and arena
 * are as they were: nothing has been filled since. */
SEXP C_delimited_unack(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  if (!d->held_rows && d->acked) {
    d->held_rows = d->acked;
    d->delivered -= (double)d->acked;
  }
  d->acked = 0;
  return R_NilValue;
}

/* Fills the next batch: rows, more input wanted, the end, or a failure. With `sample`, the batch
 * comes back as a sample takes it. */
SEXP C_delimited_fill(SEXP ptr, SEXP sample_arg) {
  struct delimited *d = delimited_of(ptr);
  int sample = asLogical(sample_arg) == TRUE;
  if (!d->plan.bound)
    Rf_error("hypertabular: the reader has no plan");
  if (d->held_rows)
    return delimited_deliver(d, sample);
  if (d->cramped)
    grow_arena(&d->arena, &d->arena_cap, mul(d->arena_cap, 2));
  d->cramped = 0;
  for (;;) {
    R_CheckUserInterrupt();
    size_t len;
    uint32_t last;
    delimited_window(d, &len, &last);
    HyperTabularBuffersWith buffers;
    memset(&buffers, 0, sizeof buffers);
    buffers.size = HyperTabularBuffersWith_SIZE;
    buffers.arena = d->arena;
    buffers.arena_cap = d->arena_cap;
    buffers.cells = d->cells;
    buffers.cells_cap = d->cells_cap;
    buffers.tokens = d->has_tokens ? d->tokens : NULL;
    buffers.tokens_len = d->tokens_len;
    buffers.row_widths = d->plan.widths;
    int32_t code = hypertabular_delimited_fill_with(
        d->state, d->input + d->start, len, last, d->plan.specs.with, d->plan.table,
        d->plan.specs.count, d->plan.batch_rows, &buffers, &d->out);
    switch (code) {
    case HYPERTABULAR_OK: {
      size_t consumed = (size_t)d->out.consumed;
      size_t rows = (size_t)d->out.rows;
      size_t held = d->cells_cap / d->per_row;
      if (held > d->plan.batch_rows)
        held = d->plan.batch_rows;
      d->cramped = rows > 0 && rows < held && d->out.arena_used * 2 >= d->arena_cap;
      if (rows) {
        /* Held first, then moved past: an interrupt from here on leaves the batch to hand over
         * again, never a read that has lost it or will read it twice. */
        d->held_rows = rows;
        d->held_start = d->start;
        d->held_len = len;
        d->held_arena = (size_t)d->out.arena_used;
        d->start += consumed;
        return delimited_deliver(d, sample);
      }
      d->start += consumed;
      if (last && (consumed == len || consumed == 0))
        return outcome(STATUS_END, R_NilValue, R_NilValue);
      if (consumed == 0) {
        SEXP more = delimited_more(d);
        if (more)
          return more;
      }
      break;
    }
    case HYPERTABULAR_ERR_STRUCTURE:
      return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&d->out.failure));
    case HYPERTABULAR_ERR_CELLS:
      grow_cells(d, d->out.needed > d->per_row ? (size_t)d->out.needed : d->per_row);
      break;
    case HYPERTABULAR_ERR_ARENA:
      grow_arena(&d->arena, &d->arena_cap, d->out.needed);
      break;
    default:
      contract_violation();
    }
  }
}

/* Cells per record, fixed by the header or the first record; 0 before either. */
SEXP C_delimited_width(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  return ScalarReal(d->state->expected);
}

/* Frees the reader's native memory now, rather than at the next garbage collection. Safe to call
 * again; every later call on the reader is an R error. */
SEXP C_delimited_close(SEXP ptr) {
  if (TYPEOF(ptr) == EXTPTRSXP)
    delimited_finalize(ptr);
  return R_NilValue;
}

/* ---- workbooks ---- */

#define WINDOW_MIN (64 * 1024)
#define STRINGS_BOUND ((uint64_t)1 << 28)

/* The buffers a workbook call may ask to have grown. */
struct scratch {
  uint8_t *window;
  size_t window_cap;
  uint8_t *arena;
  size_t arena_cap;
  HyperTabularSpan *cells;
  size_t cells_cap;
  HyperTabularSlot *row;
  size_t row_cap;
  int row_follows_cells;
};

static void scratch_free(struct scratch *s) {
  release(s->window);
  release(s->arena);
  release(s->cells);
  release(s->row);
  memset(s, 0, sizeof *s);
}

/* Each capacity is set once its block is had, so a refused allocation leaves s freeable. */
static void scratch_init(struct scratch *s, size_t window, size_t arena, size_t cells) {
  memset(s, 0, sizeof *s);
  s->window = block(window);
  s->window_cap = window;
  s->arena = block(arena);
  s->arena_cap = arena;
  s->cells = block(mul(cells, sizeof(HyperTabularSpan)));
  s->cells_cap = cells;
  s->row = block(0);
}

/* A buffer at least `needed` units long, what it held kept (the core resumes from it). */
static void *grow_keep(void *p, size_t *cap, uint64_t needed, size_t unit) {
  if (needed > SIZE_MAX)
    too_large();
  size_t length = (size_t)needed > add(*cap, 1) ? (size_t)needed : *cap + 1;
  p = grown_block(p, mul(length, unit));
  *cap = length;
  return p;
}

static void reserve_row(struct scratch *s, size_t needed) {
  if (s->row_cap < needed)
    s->row = grow_keep(s->row, &s->row_cap, needed, sizeof(HyperTabularSlot));
}

static void reserve_cells(struct scratch *s, size_t needed) {
  if (s->cells_cap < needed)
    s->cells = grow_keep(s->cells, &s->cells_cap, needed, sizeof(HyperTabularSpan));
}

/*
 * An open workbook: its state and tables, shared by the workbook and each sheet read from it,
 * each holding a reference and the last to let go freeing it, in whatever order close() and the
 * collector get to them. The container's bytes are a raw vector each one's pointer protects.
 */
struct book {
  int refs;
  const uint8_t *container;
  size_t length;
  HyperTabularWorkbookState *state;
  size_t state_size;
  uint8_t *strings;
  size_t strings_len;
  HyperTabularSpan *table;
  size_t table_len;
  uint8_t *kinds;
  size_t kinds_len;
  /* What opening works in, freed once it's open, and by the finalizer if an R error came first. */
  struct scratch scratch;
};

static void book_release(struct book *b) {
  if (--b->refs > 0)
    return;
  release(b->state);
  release(b->strings);
  release(b->table);
  release(b->kinds);
  scratch_free(&b->scratch);
  R_Free(b);
}

static void book_finalize(SEXP ptr) {
  struct book *b = R_ExternalPtrAddr(ptr);
  if (b) {
    R_ClearExternalPtr(ptr);
    book_release(b);
  }
}

static struct book *book_of(SEXP ptr) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_workbook"))
    Rf_error("hypertabular: not a workbook");
  struct book *b = R_ExternalPtrAddr(ptr);
  if (!b)
    Rf_error("hypertabular: the workbook's native memory is gone (it was closed, or a workbook "
             "does not survive saveRDS() or a new session)");
  return b;
}

static HyperTabularBuffers buffers_of(const struct scratch *s, const struct book *b) {
  HyperTabularBuffers out = {
      s->window,  s->window_cap, s->arena, s->arena_cap, s->cells, s->cells_cap, s->row,
      s->row_cap, NULL,          0,        NULL,         0,        NULL,         0};
  if (b) {
    out.strings = b->strings;
    out.strings_len = b->strings_len;
    out.table = b->table;
    out.table_len = b->table_len;
    out.kinds = b->kinds;
    out.kinds_len = b->kinds_len;
  }
  return out;
}

enum call { SHEETS, STRINGS, STYLES, HEADER };

/* Makes the call until it stops asking for room, growing the buffer it names each time, what
 * it held kept. Returns the code it ended on. */
static int32_t drive(enum call call, HyperTabularWorkbookState *state, const struct book *b,
                     struct scratch *s, const struct book *tables, HyperTabularFilled *out) {
  for (;;) {
    HyperTabularBuffers buffers = buffers_of(s, tables);
    int32_t code;
    switch (call) {
    case SHEETS:
      code = hypertabular_workbook_sheets(state, b->container, b->length, &buffers, out);
      break;
    case STRINGS:
      code = hypertabular_workbook_strings(state, b->container, b->length, &buffers, out);
      break;
    case STYLES:
      code = hypertabular_workbook_styles(state, b->container, b->length, &buffers, out);
      break;
    default:
      code = hypertabular_workbook_header(state, b->container, b->length, &buffers, out);
      break;
    }
    switch (code) {
    case HYPERTABULAR_ERR_WINDOW:
      s->window = grow_keep(s->window, &s->window_cap, out->needed, 1);
      break;
    case HYPERTABULAR_ERR_ARENA:
      s->arena = grow_keep(s->arena, &s->arena_cap, out->needed, 1);
      break;
    case HYPERTABULAR_ERR_CELLS:
      s->cells = grow_keep(s->cells, &s->cells_cap, out->needed, sizeof(HyperTabularSpan));
      if (s->row_follows_cells)
        reserve_row(s, s->cells_cap);
      break;
    default:
      return code;
    }
  }
}

static void *copy_of(const void *from, size_t n) {
  void *to = block(n);
  if (n)
    memcpy(to, from, n);
  return to;
}

/*
 * Opens the workbook in `container` (a raw vector the pointer keeps alive), lists its sheets
 * and loads its shared strings and styles. Returns list(status, payload, failure): payload is
 * list(book, format, epoch, names, hidden, parts, indexes) when status is 0.
 */
SEXP C_workbook_open(SEXP container, SEXP stingy_arg) {
  if (TYPEOF(container) != RAWSXP)
    Rf_error("hypertabular: a workbook is a raw vector");
  int stingy = asLogical(stingy_arg) == TRUE;
  /* The pointer and its finalizer first, so the block is never held by nothing. */
  SEXP ptr = PROTECT(R_MakeExternalPtr(NULL, install("hypertabular_workbook"), container));
  R_RegisterCFinalizerEx(ptr, book_finalize, TRUE);
  struct book *b = R_Calloc(1, struct book);
  b->refs = 1;
  R_SetExternalPtrAddr(ptr, b);
  b->container = RAW(container);
  b->length = (size_t)XLENGTH(container);
  b->state_size = hypertabular_workbook_state_size();
  b->state = block(b->state_size);
  struct scratch *s = &b->scratch;
  if (stingy)
    scratch_init(s, 1, 1, 1);
  else
    scratch_init(s, WINDOW_MIN, 1024, 64);
  HyperTabularOpened opened;
  int32_t code;
  for (;;) {
    HyperTabularBuffers buffers = buffers_of(s, NULL);
    code = hypertabular_workbook_open(b->state, b->container, b->length, &buffers, &opened);
    if (code == HYPERTABULAR_ERR_WINDOW)
      s->window = grow_keep(s->window, &s->window_cap, opened.needed, 1);
    else if (code == HYPERTABULAR_ERR_ARENA)
      s->arena = grow_keep(s->arena, &s->arena_cap, opened.needed, 1);
    else
      break;
  }
  SEXP result;
  HyperTabularFilled out;
  if (code == HYPERTABULAR_ERR_STRUCTURE) {
    scratch_free(s);
    result = outcome(STATUS_FAILURE, R_NilValue, failure_vector(&opened.failure));
    UNPROTECT(1);
    return result;
  }
  if (code != HYPERTABULAR_OK)
    contract_violation();
  code = drive(SHEETS, b->state, b, s, NULL, &out);
  if (code != HYPERTABULAR_OK) {
    if (code != HYPERTABULAR_ERR_STRUCTURE)
      contract_violation();
    scratch_free(s);
    result = outcome(STATUS_FAILURE, R_NilValue, failure_vector(&out.failure));
    UNPROTECT(1);
    return result;
  }
  size_t count = (size_t)out.rows;
  SEXP payload = PROTECT(allocVector(VECSXP, 7));
  SEXP names = PROTECT(allocVector(STRSXP, (R_xlen_t)count));
  SEXP hidden = PROTECT(allocVector(LGLSXP, (R_xlen_t)count));
  SEXP parts = PROTECT(allocVector(VECSXP, (R_xlen_t)count));
  SEXP indexes = PROTECT(allocVector(REALSXP, (R_xlen_t)count));
  for (size_t i = 0; i < count; i++) {
    const HyperTabularSheetInfo *info = (const HyperTabularSheetInfo *)(s->cells + 3 * i);
    HyperTabularSpan name = info->name, part = info->part;
    size_t n;
    const uint8_t *bytes = slice(s->arena, s->arena_cap, name.offset, name.len, &n);
    enum text_unit unit;
    SET_STRING_ELT(names, (R_xlen_t)i, text_char(bytes, n, &unit));
    bytes = slice(s->arena, s->arena_cap, part.offset, part.len, &n);
    SET_VECTOR_ELT(parts, (R_xlen_t)i, raw_of(bytes, n));
    LOGICAL(hidden)[i] = (info->flags & 1) != 0;
    REAL(indexes)[i] = info->index;
  }
  /* The shared strings take no more room than their part inflates to: asking for it once
   * saves growing into it. */
  uint64_t bound = opened.strings_bytes < STRINGS_BOUND ? opened.strings_bytes : STRINGS_BOUND;
  if (!stingy && s->arena_cap < bound)
    s->arena = grow_keep(s->arena, &s->arena_cap, bound, 1);
  code = drive(STRINGS, b->state, b, s, NULL, &out);
  if (code == HYPERTABULAR_OK) {
    b->strings = copy_of(s->arena, (size_t)out.arena_used);
    b->strings_len = (size_t)out.arena_used;
    b->table = copy_of(s->cells, mul((size_t)out.rows, sizeof(HyperTabularSpan)));
    b->table_len = (size_t)out.rows;
    code = drive(STYLES, b->state, b, s, NULL, &out);
    if (code == HYPERTABULAR_OK) {
      b->kinds = copy_of(s->arena, (size_t)out.rows);
      b->kinds_len = (size_t)out.rows;
    }
  }
  scratch_free(s);
  if (code == HYPERTABULAR_ERR_STRUCTURE) {
    result = outcome(STATUS_FAILURE, R_NilValue, failure_vector(&out.failure));
    UNPROTECT(6);
    return result;
  }
  if (code != HYPERTABULAR_OK)
    contract_violation();
  SET_VECTOR_ELT(payload, 0, ptr);
  SET_VECTOR_ELT(payload, 1, ScalarInteger((int)opened.format));
  SET_VECTOR_ELT(payload, 2, ScalarInteger((int)opened.epoch));
  SET_VECTOR_ELT(payload, 3, names);
  SET_VECTOR_ELT(payload, 4, hidden);
  SET_VECTOR_ELT(payload, 5, parts);
  SET_VECTOR_ELT(payload, 6, indexes);
  result = outcome(STATUS_ROWS, payload, R_NilValue);
  UNPROTECT(6);
  return result;
}

/* Lets go of the workbook: no sheet can be opened from it after this. Sheets already open read
 * on, each with its own reference. Safe to call again. */
SEXP C_workbook_close(SEXP ptr) {
  if (TYPEOF(ptr) == EXTPTRSXP)
    book_finalize(ptr);
  return R_NilValue;
}

struct sheet {
  struct book *book;
  HyperTabularWorkbookState *state;
  struct scratch scratch;
  struct plan plan;
  size_t per_row;
  int stingy;
  uint8_t *tokens;
  size_t tokens_len;
  int has_tokens;
  HyperTabularFilled out;
  /* A batch the core wrote and R doesn't have yet, and the code it came with: rows and a
   * failure can come together. */
  size_t held_rows;
  int32_t held_code;
  /* The rows R has acknowledged, the next batch's first row counting from here, and the rows of
   * the last acknowledgement, which C_sheet_unack takes back. */
  double delivered;
  size_t acked;
};

static void sheet_free(struct sheet *s) {
  release(s->state);
  scratch_free(&s->scratch);
  plan_free(&s->plan);
  release(s->tokens);
  if (s->book)
    book_release(s->book);
  R_Free(s);
}

static void sheet_finalize(SEXP ptr) {
  struct sheet *s = R_ExternalPtrAddr(ptr);
  if (s) {
    R_ClearExternalPtr(ptr);
    sheet_free(s);
  }
}

static struct sheet *sheet_of(SEXP ptr) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_sheet"))
    Rf_error("hypertabular: not a sheet");
  struct sheet *s = R_ExternalPtrAddr(ptr);
  if (!s)
    Rf_error("hypertabular: the sheet's native memory is gone (it was closed, or a sheet does "
             "not survive saveRDS() or a new session)");
  return s;
}

/* Positions a copy of the workbook's state on one sheet: the part and index its listing gave,
 * read as `options` (RawSheetOptions as R packed it, 36 bytes little-endian) says, with the
 * sheet's tokens or NULL. Returns list(status, payload = the sheet, failure). */
SEXP C_sheet_new(SEXP book_ptr, SEXP part, SEXP index, SEXP options, SEXP tokens, SEXP stingy) {
  struct book *b = book_of(book_ptr);
  if (TYPEOF(part) != RAWSXP)
    Rf_error("hypertabular: part must be a raw vector");
  if (TYPEOF(options) != RAWSXP || XLENGTH(options) != HyperTabularRawSheetOptions_SIZE)
    Rf_error("hypertabular: options must be 36 bytes");
  if (tokens != R_NilValue && TYPEOF(tokens) != RAWSXP)
    Rf_error("hypertabular: tokens must be a raw vector or NULL");
  HyperTabularRawSheetOptions raw;
  const uint8_t *o = RAW(options);
  raw.size = le32(o);
  raw.has_header = le32(o + 4);
  raw.skip_empty_rows = le32(o + 8);
  raw.skip = le32(o + 12);
  raw.header_row = le32(o + 16);
  raw.first_row = le32(o + 20);
  raw.first_column = le32(o + 24);
  raw.last_row = le32(o + 28);
  raw.last_column = le32(o + 32);
  /* The pointer and its finalizer first, so the block is never held by nothing. */
  SEXP ptr = PROTECT(
      R_MakeExternalPtr(NULL, install("hypertabular_sheet"), R_ExternalPtrProtected(book_ptr)));
  R_RegisterCFinalizerEx(ptr, sheet_finalize, TRUE);
  struct sheet *s = R_Calloc(1, struct sheet);
  R_SetExternalPtrAddr(ptr, s);
  s->book = b;
  b->refs++;
  s->stingy = asLogical(stingy) == TRUE;
  s->state = block(b->state_size);
  memcpy(s->state, b->state, b->state_size);
  if (s->stingy)
    scratch_init(&s->scratch, 1, 1, 1);
  else
    scratch_init(&s->scratch, 0, 4096, 64);
  if (tokens != R_NilValue) {
    s->tokens_len = (size_t)XLENGTH(tokens);
    s->tokens = block(s->tokens_len);
    if (s->tokens_len)
      memcpy(s->tokens, RAW(tokens), s->tokens_len);
    s->has_tokens = 1;
  }
  int32_t code = hypertabular_workbook_sheet_with(s->state, b->container, b->length, RAW(part),
                                                  (uintptr_t)XLENGTH(part), (uint32_t)asReal(index),
                                                  &raw, &s->out);
  SEXP result = R_NilValue;
  if (code == HYPERTABULAR_OK)
    result = outcome(STATUS_ROWS, ptr, R_NilValue);
  else if (code == HYPERTABULAR_ERR_STRUCTURE)
    result = outcome(STATUS_FAILURE, ptr, failure_vector(&s->out.failure));
  else
    contract_violation();
  UNPROTECT(1);
  return result;
}

/* Reads the header row. Read before a plan, the row's slots grow with the cell table and are
 * kept, a slot for every name: a row the sheet repeats is delivered again from them. */
SEXP C_sheet_header(SEXP ptr) {
  struct sheet *s = sheet_of(ptr);
  struct book *b = s->book;
  int unbound = !s->plan.bound;
  if (unbound)
    reserve_row(&s->scratch, s->scratch.cells_cap);
  s->scratch.row_follows_cells = unbound;
  int32_t code = drive(HEADER, s->state, b, &s->scratch, b, &s->out);
  s->scratch.row_follows_cells = 0;
  if (code == HYPERTABULAR_ERR_STRUCTURE)
    return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&s->out.failure));
  if (code != HYPERTABULAR_OK)
    contract_violation();
  struct texts t = {b->strings, b->strings_len, s->scratch.arena, s->scratch.arena_cap, 0};
  return outcome(STATUS_ROWS, names_vector(s->scratch.cells, (size_t)s->out.rows, &t), R_NilValue);
}

/* Takes the plan: `specs` and `lists` packed, `raws` which text columns come back as raw
 * vectors, `width` the widest ordinal it reads plus one, and with `widths` each row's width kept.
 * R asked the core about the sizes. */
SEXP C_sheet_bind(SEXP ptr, SEXP specs, SEXP lists, SEXP raws, SEXP batch_rows, SEXP width,
                  SEXP widths) {
  struct sheet *s = sheet_of(ptr);
  /* R binds once; a plan here that R never recorded (an interrupt landed before it could) is
   * replaced. No fill has read through it: R reads only once it has its plan. */
  if (s->plan.bound)
    plan_free(&s->plan);
  plan_bind(&s->plan, specs, lists, raws, count_size(batch_rows), asLogical(widths) == TRUE);
  s->plan.bound = 0;
  s->per_row = s->plan.specs.count + 1;
  /* A stingy scratch is left to grow into the cell table the hard way, mid-read. */
  if (!s->stingy)
    reserve_cells(&s->scratch, mul(s->plan.batch_rows, s->per_row));
  reserve_row(&s->scratch, count_size(width));
  s->plan.bound = 1;
  return R_NilValue;
}

static SEXP sheet_deliver(struct sheet *s, int sample) {
  struct book *b = s->book;
  size_t rows = s->held_rows;
  SEXP failure =
      PROTECT(s->held_code == HYPERTABULAR_OK ? R_NilValue : failure_vector(&s->out.failure));
  SEXP payload = R_NilValue;
  if (rows) {
    struct texts t = {b->strings, b->strings_len, s->scratch.arena, (size_t)s->out.arena_used, 0};
    payload = sample ? sample_batch(&s->plan, rows, s->scratch.cells, s->per_row, &t)
                     : batch(&s->plan, rows, s->scratch.cells, s->per_row, &t);
  }
  PROTECT(payload);
  int status = rows ? STATUS_ROWS : s->held_code == HYPERTABULAR_OK ? STATUS_END : STATUS_FAILURE;
  SEXP result = PROTECT(outcome(status, payload, failure));
  SEXP first = PROTECT(ScalarReal(s->delivered));
  setAttrib(result, install("first"), first);
  UNPROTECT(4);
  return result;
}

/* R has the batch it was last handed: its rows are counted and the next fill reads on, in one
 * step no interrupt can split. */
SEXP C_sheet_ack(SEXP ptr) {
  struct sheet *s = sheet_of(ptr);
  s->delivered += (double)s->held_rows;
  s->acked = s->held_rows;
  s->held_rows = 0;
  return R_NilValue;
}

/* As C_delimited_unack, for a sheet. */
SEXP C_sheet_unack(SEXP ptr) {
  struct sheet *s = sheet_of(ptr);
  if (!s->held_rows && s->acked) {
    s->held_rows = s->acked;
    s->delivered -= (double)s->acked;
  }
  s->acked = 0;
  return R_NilValue;
}

/* Fills the next batch: list(status, payload = the batch or NULL, failure). Rows and a failure
 * can come together: the rows before the break. With `sample`, the batch comes back as a sample
 * takes it. */
SEXP C_sheet_fill(SEXP ptr, SEXP sample_arg) {
  struct sheet *s = sheet_of(ptr);
  struct book *b = s->book;
  int sample = asLogical(sample_arg) == TRUE;
  if (!s->plan.bound)
    Rf_error("hypertabular: the sheet has no plan");
  if (s->held_rows)
    return sheet_deliver(s, sample);
  for (;;) {
    R_CheckUserInterrupt();
    HyperTabularBuffers base = buffers_of(&s->scratch, b);
    HyperTabularBuffersWith buffers;
    memset(&buffers, 0, sizeof buffers);
    buffers.size = HyperTabularBuffersWith_SIZE;
    buffers.window = base.window;
    buffers.window_cap = base.window_cap;
    buffers.arena = base.arena;
    buffers.arena_cap = base.arena_cap;
    buffers.cells = base.cells;
    buffers.cells_cap = base.cells_cap;
    buffers.row = base.row;
    buffers.row_cap = base.row_cap;
    buffers.strings = base.strings;
    buffers.strings_len = base.strings_len;
    buffers.table = base.table;
    buffers.table_len = base.table_len;
    buffers.kinds = base.kinds;
    buffers.kinds_len = base.kinds_len;
    buffers.tokens = s->has_tokens ? s->tokens : NULL;
    buffers.tokens_len = s->tokens_len;
    buffers.row_widths = s->plan.widths;
    int32_t code = hypertabular_workbook_fill_with(
        s->state, b->container, b->length, s->plan.specs.with, s->plan.table, s->plan.specs.count,
        s->plan.batch_rows, &buffers, &s->out);
    switch (code) {
    case HYPERTABULAR_ERR_WINDOW:
      s->scratch.window = grow_keep(s->scratch.window, &s->scratch.window_cap, s->out.needed, 1);
      continue;
    case HYPERTABULAR_ERR_ARENA:
      s->scratch.arena = grow_keep(s->scratch.arena, &s->scratch.arena_cap, s->out.needed, 1);
      continue;
    case HYPERTABULAR_ERR_CELLS:
      s->scratch.cells = grow_keep(s->scratch.cells, &s->scratch.cells_cap, s->out.needed,
                                   sizeof(HyperTabularSpan));
      continue;
    case HYPERTABULAR_OK:
    case HYPERTABULAR_ERR_STRUCTURE:
      break;
    default:
      contract_violation();
    }
    /* Held, so an interrupt during the copy hands the same batch over next time. A batch of no
     * rows is the end or the failure, which the state block says again on every later call. */
    s->held_rows = (size_t)s->out.rows;
    s->held_code = code;
    return sheet_deliver(s, sample);
  }
}

/* Frees the sheet's native memory now. Safe to call again; every later call is an R error. */
SEXP C_sheet_close(SEXP ptr) {
  if (TYPEOF(ptr) == EXTPTRSXP)
    sheet_finalize(ptr);
  return R_NilValue;
}

/* ---- the 1.0 fills, kept for callers built against them ---- */

/*
 * One read of `input` through the 1.0 delimited exports, with no header: init with the 4-byte
 * dialect, then fills of 1.0 ColumnSpecs (104 bytes each, R-packed) until the input ends.
 * Returns, per plan column, the verdict reasons of every row: the 1.0 surface reads a format's
 * low 8 bits alone, so every format is Western there. A test proves it; no read takes this path.
 */
SEXP C_one_dot_zero_delimited(SEXP input, SEXP dialect, SEXP specs) {
  if (TYPEOF(input) != RAWSXP || TYPEOF(dialect) != RAWSXP || XLENGTH(dialect) != 4 ||
      TYPEOF(specs) != RAWSXP || XLENGTH(specs) % HyperTabularColumnSpec_SIZE != 0)
    Rf_error("hypertabular: input, a 4-byte dialect and 104-byte specs");
  size_t count = (size_t)XLENGTH(specs) / HyperTabularColumnSpec_SIZE;
  size_t rows = 1024;
  HyperTabularDelimitedState *state =
      (HyperTabularDelimitedState *)R_alloc(1, hypertabular_delimited_state_size());
  HyperTabularRawDialect raw;
  memcpy(&raw, RAW(dialect), sizeof raw);
  if (hypertabular_delimited_init(state, &raw) != HYPERTABULAR_OK)
    contract_violation();
  HyperTabularColumnSpec *with = (HyperTabularColumnSpec *)R_alloc(count ? count : 1, sizeof *with);
  HyperTabularColumnBuffer *table =
      (HyperTabularColumnBuffer *)R_alloc(count ? count : 1, sizeof *table);
  for (size_t i = 0; i < count; i++) {
    const uint8_t *p = RAW(specs) + i * HyperTabularColumnSpec_SIZE;
    with[i].ordinal = le32(p);
    with[i].door = le32(p + 4);
    with[i].param = le32(p + 8);
    if (with[i].door == 0 || with[i].door >= DOOR_COUNT)
      Rf_error("hypertabular: unknown door %u", with[i].door);
    format_from(p + 12, &with[i].format);
    table[i].values = R_alloc(rows, DOORS[with[i].door].size);
    table[i].verdicts = (HyperTabularCellVerdict *)R_alloc(rows, sizeof(HyperTabularCellVerdict));
  }
  size_t cells_cap = rows * 1024;
  HyperTabularSpan *cells = (HyperTabularSpan *)R_alloc(cells_cap, sizeof *cells);
  size_t arena_cap = 1 << 16;
  uint8_t *arena = (uint8_t *)R_alloc(arena_cap, 1);
  SEXP out = PROTECT(allocVector(VECSXP, (R_xlen_t)count));
  size_t at = 0, len = (size_t)XLENGTH(input), total = 0;
  const uint8_t *bytes = RAW(input);
  SEXP *reasons = (SEXP *)R_alloc(count ? count : 1, sizeof(SEXP));
  for (size_t i = 0; i < count; i++) {
    reasons[i] = allocVector(INTSXP, (R_xlen_t)len + 1);
    SET_VECTOR_ELT(out, (R_xlen_t)i, reasons[i]);
  }
  for (;;) {
    HyperTabularBuffers buffers;
    memset(&buffers, 0, sizeof buffers);
    buffers.arena = arena;
    buffers.arena_cap = arena_cap;
    buffers.cells = cells;
    buffers.cells_cap = cells_cap;
    HyperTabularFilled filled;
    int32_t code = hypertabular_delimited_fill(state, bytes + at, len - at, 1, with, table, count,
                                               rows, &buffers, &filled);
    if (code != HYPERTABULAR_OK)
      Rf_error("hypertabular: the 1.0 fill returned %d", code);
    for (size_t i = 0; i < count; i++)
      for (size_t r = 0; r < filled.rows && total + r <= len; r++)
        INTEGER(reasons[i])[total + r] = (int)table[i].verdicts[r].reason;
    total += (size_t)filled.rows;
    at += (size_t)filled.consumed;
    if (!filled.rows)
      break;
  }
  for (size_t i = 0; i < count; i++)
    SET_VECTOR_ELT(out, (R_xlen_t)i, Rf_lengthgets(reasons[i], (R_xlen_t)total));
  UNPROTECT(1);
  return out;
}

/*
 * One read of a sheet through the 1.0 workbook exports, its first row a header: the 1.0 sheet
 * call, then 1.0 fills of 104-byte ColumnSpecs. Returns, per plan column, every row's reason.
 */
SEXP C_one_dot_zero_sheet(SEXP book_ptr, SEXP part, SEXP index, SEXP specs) {
  struct book *b = book_of(book_ptr);
  if (TYPEOF(part) != RAWSXP || TYPEOF(specs) != RAWSXP ||
      XLENGTH(specs) % HyperTabularColumnSpec_SIZE != 0)
    Rf_error("hypertabular: a part and 104-byte specs");
  size_t count = (size_t)XLENGTH(specs) / HyperTabularColumnSpec_SIZE;
  size_t rows = 4096;
  HyperTabularWorkbookState *state = (HyperTabularWorkbookState *)R_alloc(1, b->state_size);
  memcpy(state, b->state, b->state_size);
  HyperTabularFilled filled;
  if (hypertabular_workbook_sheet(state, b->container, b->length, RAW(part),
                                  (uintptr_t)XLENGTH(part), (uint32_t)asReal(index), 1, 1,
                                  &filled) != HYPERTABULAR_OK)
    Rf_error("hypertabular: the 1.0 sheet call refused the sheet");
  HyperTabularColumnSpec *with = (HyperTabularColumnSpec *)R_alloc(count ? count : 1, sizeof *with);
  HyperTabularColumnBuffer *table =
      (HyperTabularColumnBuffer *)R_alloc(count ? count : 1, sizeof *table);
  size_t reach = 1;
  for (size_t i = 0; i < count; i++) {
    const uint8_t *p = RAW(specs) + i * HyperTabularColumnSpec_SIZE;
    with[i].ordinal = le32(p);
    with[i].door = le32(p + 4);
    with[i].param = le32(p + 8);
    if (with[i].door == 0 || with[i].door >= DOOR_COUNT)
      Rf_error("hypertabular: unknown door %u", with[i].door);
    format_from(p + 12, &with[i].format);
    if (with[i].ordinal + 1u > reach)
      reach = with[i].ordinal + 1u;
    table[i].values = R_alloc(rows, DOORS[with[i].door].size);
    table[i].verdicts = (HyperTabularCellVerdict *)R_alloc(rows, sizeof(HyperTabularCellVerdict));
  }
  /* Room enough for a test's small sheet: a 1.0 workbook call that asks for more is refused. */
  struct scratch s;
  s.window_cap = (size_t)1 << 20;
  s.window = (uint8_t *)R_alloc(s.window_cap, 1);
  s.arena_cap = (size_t)1 << 20;
  s.arena = (uint8_t *)R_alloc(s.arena_cap, 1);
  s.cells_cap = rows * (count + 1);
  s.cells = (HyperTabularSpan *)R_alloc(s.cells_cap, sizeof(HyperTabularSpan));
  s.row_cap = reach < 4096 ? 4096 : reach;
  s.row = (HyperTabularSlot *)R_alloc(s.row_cap, sizeof(HyperTabularSlot));
  HyperTabularBuffers buffers = buffers_of(&s, b);
  if (hypertabular_workbook_header(state, b->container, b->length, &buffers, &filled) !=
      HYPERTABULAR_OK)
    Rf_error("hypertabular: the 1.0 header call stopped");
  if (hypertabular_workbook_fill(state, b->container, b->length, with, table, count, rows, &buffers,
                                 &filled) != HYPERTABULAR_OK)
    Rf_error("hypertabular: the 1.0 sheet fill stopped");
  SEXP out = PROTECT(allocVector(VECSXP, (R_xlen_t)count));
  for (size_t i = 0; i < count; i++) {
    SEXP reasons = allocVector(INTSXP, (R_xlen_t)filled.rows);
    SET_VECTOR_ELT(out, (R_xlen_t)i, reasons);
    for (size_t r = 0; r < filled.rows; r++)
      INTEGER(reasons)[r] = (int)table[i].verdicts[r].reason;
  }
  UNPROTECT(1);
  return out;
}

/*
 * The one symbol the core's archive needs and does not define. compiler_builtins ships
 * precompiled with unwind tables, and the members R's linker pulls from the archive for the
 * core's 128-bit arithmetic name `rust_eh_personality`. The archive is built with
 * `panic = "abort"`, so nothing ever unwinds and nothing ever calls this: it only has to exist.
 * Hidden, so the package's shared object neither exports nor shares it.
 */
#if defined(__GNUC__) && !defined(_WIN32)
__attribute__((visibility("hidden")))
#endif
void rust_eh_personality(void);
void rust_eh_personality(void) {}

/* The packed core version (major << 16 | minor << 8 | patch); R unpacks it. */
/* Where a read lets a pending interrupt through, when R allows it: R_CheckUserInterrupt() is
 * otherwise reached only between core calls and every 2^20 elements copied. */
SEXP C_interrupt_point(void) {
  R_CheckUserInterrupt();
  return R_NilValue;
}

/* Whether a path names a regular file, whose read can't wait on anything outside the machine:
 * a FIFO or a device a `file()` connection may also name can. */
SEXP C_regular_file(SEXP path) {
  if (TYPEOF(path) != STRSXP || XLENGTH(path) != 1 || STRING_ELT(path, 0) == NA_STRING) {
    return ScalarLogical(FALSE);
  }
  struct stat st;
  const char *name = R_ExpandFileName(translateChar(STRING_ELT(path, 0)));
  return ScalarLogical(name[0] != '\0' && stat(name, &st) == 0 && S_ISREG(st.st_mode));
}

SEXP C_native_version(void) { return ScalarInteger((int)hypertabular_version()); }
