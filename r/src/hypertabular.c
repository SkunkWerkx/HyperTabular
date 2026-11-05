#include <R.h>
#include <Rinternals.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "include/hypertabular.h"
#include "shim.h"

/*
 * The readers, over the core's C ABI. A delimited reader and a sheet are external pointers
 * to the native memory a read works in (the state block, the plan's arrays, the cell table,
 * the arena, the window), all R_Calloc'd and freed by the pointer's finalizer, so an R error
 * between calls leaks nothing. Every batch is copied out into R vectors as it is made, so it
 * outlives the reader's next read, as in every binding of a garbage-collected language.
 *
 * Each plan column becomes the columns of one HyperCast verdict: the value (NA where the cell
 * did not cast) and its exact parts, then ok, the reason code (1 empty, 2 malformed, 3 out of
 * range), the fault's offset and length, and raw, the cell's text a fault's span indexes.
 * Nothing here raises an R error for anything in the data: a cell is a verdict, and a broken
 * file a failure handed back as a value. The only errors are caller bugs R validated before
 * the call (which the core's contract-violation code would also report) and running out of
 * memory.
 */

/* ---- the doors ---- */

#define DOOR_COUNT 23 /* codes 1..22 */
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
_Static_assert(sizeof(HyperTabularColumnSpec) == HyperTabularColumnSpec_SIZE, "column spec");
_Static_assert(sizeof(HyperTabularRawNumFormat) == HyperTabularRawNumFormat_SIZE, "num format");
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
_Static_assert(sizeof(HyperTabularDate) == 4, "date");
_Static_assert(sizeof(HyperTabularCivilDateTime) == 16, "civil");
_Static_assert(sizeof(HyperTabularDuration) == 16, "duration");
_Static_assert(sizeof(HyperTabularDecimal) == 16, "decimal");
_Static_assert(sizeof(HyperTabularBuffers) == 14 * sizeof(void *), "buffers");

/* A span's length, its flag bit cleared. */
#define SPAN_LENGTH (HyperTabularSpan_FLAG - 1u)

#if defined(__GNUC__)
__attribute__((noreturn))
#endif
static void contract_violation(void);
static void contract_violation(void) {
  Rf_error("hypertabular: the core reported a contract violation (a binding bug this package "
           "checks for first; please report it)");
}

/* ---- native memory ---- */

/* The 8-byte words that hold n bytes, at least one: R_Calloc and R_Realloc count elements. */
static size_t words_for(size_t n) {
  return n ? n / sizeof(uint64_t) + (n % sizeof(uint64_t) != 0) : 1;
}

/* A zeroed block of n bytes, at least one, 8-byte aligned (what malloc returns). */
static void *block(size_t n) { return R_Calloc(words_for(n), uint64_t); }

static void *grown_block(void *p, size_t n) { return R_Realloc(p, words_for(n), uint64_t); }

static void release(void *p) {
  if (p)
    R_Free(p);
}

/* ---- the plan's arrays ---- */

struct plan {
  size_t count;
  size_t batch_rows;
  HyperTabularColumnSpec *specs;
  HyperTabularColumnBuffer *table;
  uint32_t *doors;
  uint32_t *ordinals;
};

static void plan_free(struct plan *p) {
  for (size_t i = 0; p->table && i < p->count; i++) {
    release(p->table[i].values);
    release(p->table[i].verdicts);
  }
  release(p->table);
  release(p->specs);
  release(p->doors);
  release(p->ordinals);
  memset(p, 0, sizeof *p);
}

static uint32_t le32(const uint8_t *b) {
  return (uint32_t)b[0] | (uint32_t)b[1] << 8 | (uint32_t)b[2] << 16 | (uint32_t)b[3] << 24;
}

/*
 * specs: a raw vector of 104 bytes per plan column, each a ColumnSpec R packed little-endian
 * (ordinal, door, param, then HyperCast's 92-byte RawNumFormat, whose own words are
 * little-endian and are decoded here so a big-endian host reads them right).
 */
static void plan_bind(struct plan *p, SEXP specs, size_t batch_rows) {
  if (TYPEOF(specs) != RAWSXP || XLENGTH(specs) % 104 != 0)
    Rf_error("hypertabular: specs must be packed column specs");
  size_t count = (size_t)XLENGTH(specs) / 104;
  const uint8_t *b = RAW(specs);
  for (size_t i = 0; i < count; i++) {
    uint32_t door = le32(b + i * 104 + 4);
    if (door == 0 || door >= DOOR_COUNT)
      Rf_error("hypertabular: unknown door %u", door);
  }
  p->count = count;
  p->batch_rows = batch_rows;
  p->specs = block(count * sizeof(HyperTabularColumnSpec));
  p->table = block(count * sizeof(HyperTabularColumnBuffer));
  p->doors = block(count * sizeof(uint32_t));
  p->ordinals = block(count * sizeof(uint32_t));
  for (size_t i = 0; i < count; i++) {
    const uint8_t *s = b + i * HyperTabularColumnSpec_SIZE;
    HyperTabularColumnSpec *spec = &p->specs[i];
    spec->ordinal = le32(s);
    spec->door = le32(s + 4);
    spec->param = le32(s + 8);
    memcpy(&spec->format, s + 12, HyperTabularRawNumFormat_SIZE);
    /* The three words before the symbols (size, flags, digits) come packed little-endian. */
    spec->format.size = le32(s + 12);
    spec->format.format.flags = le32(s + 16);
    spec->format.format.digits = le32(s + 20);
    p->doors[i] = spec->door;
    p->ordinals[i] = spec->ordinal;
    p->table[i].values = block(DOORS[spec->door].size * batch_rows);
    p->table[i].verdicts = block(sizeof(HyperTabularCellVerdict) * batch_rows);
  }
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
 * substr() counts such a string), and NA when it holds a NUL byte, which no R string can.
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
  int count;
  enum kind kind;
  int *lgl;
  int *integer;
  double *real;
  SEXP strings;
  double *extra_real[2];
  int *extra_int;
  int *ok;
  int *reason;
  int *offset;
  int *length;
  SEXP raw;
};

static SEXP column(struct columns *c, const char *name, SEXPTYPE type, R_xlen_t n) {
  SEXP v = allocVector(type, n);
  SET_VECTOR_ELT(c->list, c->count, v);
  SET_STRING_ELT(getAttrib(c->list, R_NamesSymbol), c->count, mkChar(name));
  c->count++;
  return v;
}

/* Allocates every column; the result is PROTECTed once, as c->list. */
static void columns_alloc(struct columns *c, enum kind kind, R_xlen_t n) {
  memset(c, 0, sizeof *c);
  c->kind = kind;
  int total = 6;
  if (kind == INSTANT_KIND || kind == CIVIL_KIND || kind == DURATION_KIND)
    total += 2;
  else if (kind == TIME_KIND)
    total += 1;
  c->list = PROTECT(allocVector(VECSXP, total));
  SEXP names = PROTECT(allocVector(STRSXP, total));
  setAttrib(c->list, R_NamesSymbol, names);
  UNPROTECT(1);
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

/* A zone-less wall clock as "YYYY-MM-DD HH:MM:SS[.f]", the fraction's trailing zeros trimmed. */
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

/*
 * The batch the core just wrote: a list of `columns` (one verdict's columns per plan column)
 * and `line` (each row's line, or sheet row number). `cells` is the cell table, `per_row`
 * entries a row: a delimited row's entry for source column c is at c and its line is the last
 * entry's len; a workbook row's entry for plan column c is at c and its row number the last
 * entry's offset.
 */
static SEXP batch(const struct plan *p, size_t rows, const HyperTabularSpan *cells, size_t per_row,
                  const struct texts *t) {
  SEXP out = PROTECT(allocVector(VECSXP, 2));
  SEXP names = PROTECT(allocVector(STRSXP, 2));
  SET_STRING_ELT(names, 0, mkChar("columns"));
  SET_STRING_ELT(names, 1, mkChar("line"));
  setAttrib(out, R_NamesSymbol, names);
  SEXP cols = allocVector(VECSXP, (R_xlen_t)p->count);
  SET_VECTOR_ELT(out, 0, cols);
  for (size_t k = 0; k < p->count; k++) {
    uint32_t door = p->doors[k];
    size_t size = DOORS[door].size;
    const uint8_t *values = p->table[k].values;
    const uint8_t *verdicts = (const uint8_t *)p->table[k].verdicts;
    struct columns c;
    columns_alloc(&c, DOORS[door].kind, (R_xlen_t)rows);
    SET_VECTOR_ELT(cols, (R_xlen_t)k, c.list);
    UNPROTECT(1);
    size_t entry = t->delimited ? p->ordinals[k] : k;
    for (size_t r = 0; r < rows; r++) {
      const void *vmax = vmaxget();
      HyperTabularCellVerdict verdict;
      memcpy(&verdict, verdicts + r * sizeof verdict, sizeof verdict);
      const uint8_t *v = values + r * size;
      if (verdict.reason == 0 && door == TEXT_DOOR) {
        HyperTabularSpan s;
        memcpy(&s, v, sizeof s);
        size_t n;
        const uint8_t *b = value_text(t, s, &n);
        const uint8_t *nul = memchr(b, 0, n);
        if (nul) {
          /* A NUL byte no R string can hold: out of range there, as HyperCast's R binding
           * reports a U+0000 it cannot return. */
          row_fault(&c, (R_xlen_t)r, 3, (uint32_t)(nul - b), 1, b, n);
        } else {
          enum text_unit unit;
          row_ok(&c, (R_xlen_t)r, door, v);
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
  UNPROTECT(2);
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

static size_t size_arg(SEXP x, const char *what) {
  if (!isReal(x) && !isInteger(x))
    Rf_error("hypertabular: %s must be a number", what);
  double d = asReal(x);
  if (ISNAN(d) || d < 1 || d > 1e15)
    Rf_error("hypertabular: %s must be a positive whole number", what);
  return (size_t)d;
}

/* ---- delimited text ---- */

/* The largest unfinished row the reader holds before it reports the row as too long. */
#define MAX_ROW_BYTES ((size_t)1 << 30)
#define ARENA_BYTES 4096
#define NAMES 64
/* The cell-table entries a reader holds at most, unless one row takes more: a batch of 4096
 * rows of a plan reaching 256 source columns. */
#define CELL_BUDGET ((size_t)4096 * 257)
/* The synthetic failure code of a row past MAX_ROW_BYTES, as the other bindings number it. */
#define ROW_TOO_LONG 3

struct delimited {
  HyperTabularDelimitedState *state;
  uint8_t *input;
  size_t input_cap, input_len, start;
  int eof;
  uint8_t *arena;
  size_t arena_cap;
  HyperTabularSpan *names;
  size_t names_cap;
  HyperTabularSpan *cells;
  size_t cells_cap, per_row;
  struct plan plan;
  int cramped, stingy;
  HyperTabularFilled out;
};

static void delimited_free(struct delimited *d) {
  release(d->state);
  release(d->input);
  release(d->arena);
  release(d->names);
  release(d->cells);
  plan_free(&d->plan);
  R_Free(d);
}

static void delimited_finalize(SEXP ptr) {
  struct delimited *d = R_ExternalPtrAddr(ptr);
  if (d) {
    delimited_free(d);
    R_ClearExternalPtr(ptr);
  }
}

static struct delimited *delimited_of(SEXP ptr) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_delimited"))
    Rf_error("hypertabular: not a delimited reader");
  struct delimited *d = R_ExternalPtrAddr(ptr);
  if (!d)
    Rf_error("hypertabular: the reader's native memory is gone (a reader does not survive "
             "saveRDS() or a new session)");
  return d;
}

/* dialect: four bytes (separator, quoting, skip_blank_lines, engine), R-checked. */
SEXP C_delimited_new(SEXP dialect, SEXP stingy) {
  if (TYPEOF(dialect) != RAWSXP || XLENGTH(dialect) != 4)
    Rf_error("hypertabular: dialect must be four bytes");
  struct delimited *d = R_Calloc(1, struct delimited);
  SEXP ptr = PROTECT(R_MakeExternalPtr(d, install("hypertabular_delimited"), R_NilValue));
  R_RegisterCFinalizerEx(ptr, delimited_finalize, TRUE);
  d->stingy = asLogical(stingy) == TRUE;
  d->state = block(hypertabular_delimited_state_size());
  d->arena_cap = d->stingy ? 1 : ARENA_BYTES;
  d->arena = block(d->arena_cap);
  d->names_cap = d->stingy ? 1 : NAMES;
  d->names = block(d->names_cap * sizeof(HyperTabularSpan));
  d->input = block(0);
  HyperTabularRawDialect raw;
  memcpy(&raw, RAW(dialect), sizeof raw);
  if (hypertabular_delimited_init(d->state, &raw) != HYPERTABULAR_OK)
    Rf_error("hypertabular: the core refused the dialect");
  UNPROTECT(1);
  return ptr;
}

/* Appends `chunk` to what the reader holds unread, or with NULL marks the input's end.
 * Returns the bytes now held unread. */
SEXP C_delimited_feed(SEXP ptr, SEXP chunk) {
  struct delimited *d = delimited_of(ptr);
  if (chunk == R_NilValue) {
    d->eof = 1;
  } else {
    if (TYPEOF(chunk) != RAWSXP)
      Rf_error("hypertabular: a chunk must be a raw vector");
    size_t pending = d->input_len - d->start;
    if (d->start) {
      memmove(d->input, d->input + d->start, pending);
      d->start = 0;
      d->input_len = pending;
    }
    size_t n = (size_t)XLENGTH(chunk);
    if (pending + n > d->input_cap) {
      size_t cap = d->input_cap * 2 > pending + n ? d->input_cap * 2 : pending + n;
      d->input = grown_block(d->input, cap);
      d->input_cap = cap;
    }
    if (n)
      memcpy(d->input + d->input_len, RAW(chunk), n);
    d->input_len += n;
  }
  return ScalarReal((double)(d->input_len - d->start));
}

static void delimited_window(const struct delimited *d, size_t *len, uint32_t *last) {
  *len = d->input_len - d->start;
  *last = d->eof ? 1 : 0;
  if (*len > MAX_ROW_BYTES) {
    *len = MAX_ROW_BYTES;
    *last = 0;
  }
}

static void grow_arena(uint8_t **arena, size_t *cap, uint64_t needed) {
  size_t want = needed > *cap * 2 ? (size_t)needed : *cap * 2;
  release(*arena);
  *arena = block(want);
  *cap = want;
}

/* Wanting more input: refused once the unfinished row holds MAX_ROW_BYTES. */
static SEXP delimited_more(const struct delimited *d) {
  if (d->input_len - d->start >= MAX_ROW_BYTES || d->eof) {
    HyperTabularFailure f = {0};
    f.code = ROW_TOO_LONG;
    f.line = d->state->line;
    f.record = d->state->records;
    f.byte = d->state->offset;
    return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&f));
  }
  return outcome(STATUS_MORE, R_NilValue, R_NilValue);
}

/* Reads the header record: its names, or the end (no record at all), more input wanted, or a
 * failure. */
SEXP C_delimited_header(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  for (;;) {
    size_t len;
    uint32_t last;
    delimited_window(d, &len, &last);
    int32_t code = hypertabular_delimited_header(d->state, d->input + d->start, len, last, d->names,
                                                 d->names_cap, d->arena, d->arena_cap, &d->out);
    switch (code) {
    case HYPERTABULAR_OK: {
      size_t consumed = (size_t)d->out.consumed;
      if (d->out.rows) {
        struct texts t = {d->input + d->start, len, d->arena, d->arena_cap, 1};
        SEXP names = names_vector(d->names, (size_t)d->out.rows, &t);
        d->start += consumed;
        return outcome(STATUS_ROWS, names, R_NilValue);
      }
      d->start += consumed;
      if (last && (consumed == len || consumed == 0))
        return outcome(STATUS_END, allocVector(STRSXP, 0), R_NilValue);
      if (consumed == 0)
        return delimited_more(d);
      break;
    }
    case HYPERTABULAR_ERR_STRUCTURE:
      return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&d->out.failure));
    case HYPERTABULAR_ERR_CELLS:
      release(d->names);
      d->names_cap = (size_t)d->out.needed;
      d->names = block(d->names_cap * sizeof(HyperTabularSpan));
      break;
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

/* Takes the plan: `specs` packed, `per_row` the widest ordinal it reads plus two. */
SEXP C_delimited_bind(SEXP ptr, SEXP specs, SEXP batch_rows, SEXP per_row) {
  struct delimited *d = delimited_of(ptr);
  if (d->plan.specs)
    Rf_error("hypertabular: the reader already has a plan");
  plan_bind(&d->plan, specs, size_arg(batch_rows, "batch_rows"));
  d->per_row = size_arg(per_row, "per_row");
  d->cells_cap = d->per_row * cell_rows(d->per_row, d->plan.batch_rows, d->stingy);
  d->cells = block(d->cells_cap * sizeof(HyperTabularSpan));
  return R_NilValue;
}

/* Fills the next batch: rows, more input wanted, the end, or a failure. */
SEXP C_delimited_fill(SEXP ptr) {
  struct delimited *d = delimited_of(ptr);
  if (!d->plan.specs)
    Rf_error("hypertabular: the reader has no plan");
  if (d->cramped)
    grow_arena(&d->arena, &d->arena_cap, d->arena_cap * 2);
  d->cramped = 0;
  for (;;) {
    size_t len;
    uint32_t last;
    delimited_window(d, &len, &last);
    HyperTabularBuffers buffers = {0};
    buffers.arena = d->arena;
    buffers.arena_cap = d->arena_cap;
    buffers.cells = d->cells;
    buffers.cells_cap = d->cells_cap;
    int32_t code = hypertabular_delimited_fill(d->state, d->input + d->start, len, last,
                                               d->plan.specs, d->plan.table, d->plan.count,
                                               d->plan.batch_rows, &buffers, &d->out);
    switch (code) {
    case HYPERTABULAR_OK: {
      size_t consumed = (size_t)d->out.consumed;
      size_t rows = (size_t)d->out.rows;
      size_t held = d->cells_cap / d->per_row;
      if (held > d->plan.batch_rows)
        held = d->plan.batch_rows;
      d->cramped = rows > 0 && rows < held && d->out.arena_used * 2 >= d->arena_cap;
      if (rows) {
        struct texts t = {d->input + d->start, len, d->arena, (size_t)d->out.arena_used, 1};
        SEXP b = batch(&d->plan, rows, d->cells, d->per_row, &t);
        d->start += consumed;
        return outcome(STATUS_ROWS, b, R_NilValue);
      }
      d->start += consumed;
      if (last && (consumed == len || consumed == 0))
        return outcome(STATUS_END, R_NilValue, R_NilValue);
      if (consumed == 0)
        return delimited_more(d);
      break;
    }
    case HYPERTABULAR_ERR_STRUCTURE:
      return outcome(STATUS_FAILURE, R_NilValue, failure_vector(&d->out.failure));
    case HYPERTABULAR_ERR_CELLS:
      if (d->out.needed > d->per_row)
        d->per_row = (size_t)d->out.needed;
      release(d->cells);
      d->cells_cap = d->per_row * cell_rows(d->per_row, d->plan.batch_rows, d->stingy);
      d->cells = block(d->cells_cap * sizeof(HyperTabularSpan));
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
  uint32_t expected;
  memcpy(&expected, (uint8_t *)d->state + 24, sizeof expected);
  return ScalarReal(expected);
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

static void scratch_init(struct scratch *s, size_t window, size_t arena, size_t cells) {
  memset(s, 0, sizeof *s);
  s->window_cap = window;
  s->window = block(window);
  s->arena_cap = arena;
  s->arena = block(arena);
  s->cells_cap = cells;
  s->cells = block(cells * sizeof(HyperTabularSpan));
  s->row = block(0);
}

static void scratch_free(struct scratch *s) {
  release(s->window);
  release(s->arena);
  release(s->cells);
  release(s->row);
}

/* A buffer at least `needed` units long, what it held kept (the core resumes from it). */
static void *grow_keep(void *p, size_t *cap, uint64_t needed, size_t unit) {
  size_t length = needed > *cap + 1 ? (size_t)needed : *cap + 1;
  p = grown_block(p, length * unit);
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

struct book {
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
};

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

static void book_free(struct book *b) {
  release(b->state);
  release(b->strings);
  release(b->table);
  release(b->kinds);
  R_Free(b);
}

static void book_finalize(SEXP ptr) {
  struct book *b = R_ExternalPtrAddr(ptr);
  if (b) {
    book_free(b);
    R_ClearExternalPtr(ptr);
  }
}

static struct book *book_of(SEXP ptr) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_workbook"))
    Rf_error("hypertabular: not a workbook");
  struct book *b = R_ExternalPtrAddr(ptr);
  if (!b)
    Rf_error("hypertabular: the workbook's native memory is gone (a workbook does not survive "
             "saveRDS() or a new session)");
  return b;
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
  struct book *b = R_Calloc(1, struct book);
  SEXP ptr = PROTECT(R_MakeExternalPtr(b, install("hypertabular_workbook"), container));
  R_RegisterCFinalizerEx(ptr, book_finalize, TRUE);
  b->container = RAW(container);
  b->length = (size_t)XLENGTH(container);
  b->state_size = hypertabular_workbook_state_size();
  b->state = block(b->state_size);
  struct scratch s;
  if (stingy)
    scratch_init(&s, 1, 1, 1);
  else
    scratch_init(&s, WINDOW_MIN, 1024, 64);
  /* The scratch is freed on every path below; an R error inside (out of memory) leaks it. */
  HyperTabularOpened opened;
  int32_t code;
  for (;;) {
    HyperTabularBuffers buffers = buffers_of(&s, NULL);
    code = hypertabular_workbook_open(b->state, b->container, b->length, &buffers, &opened);
    if (code == HYPERTABULAR_ERR_WINDOW)
      s.window = grow_keep(s.window, &s.window_cap, opened.needed, 1);
    else if (code == HYPERTABULAR_ERR_ARENA)
      s.arena = grow_keep(s.arena, &s.arena_cap, opened.needed, 1);
    else
      break;
  }
  SEXP result;
  HyperTabularFilled out;
  if (code == HYPERTABULAR_ERR_STRUCTURE) {
    scratch_free(&s);
    result = outcome(STATUS_FAILURE, R_NilValue, failure_vector(&opened.failure));
    UNPROTECT(1);
    return result;
  }
  if (code != HYPERTABULAR_OK) {
    scratch_free(&s);
    contract_violation();
  }
  code = drive(SHEETS, b->state, b, &s, NULL, &out);
  if (code != HYPERTABULAR_OK) {
    scratch_free(&s);
    if (code != HYPERTABULAR_ERR_STRUCTURE)
      contract_violation();
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
    const HyperTabularSheetInfo *info = (const HyperTabularSheetInfo *)(s.cells + 3 * i);
    HyperTabularSpan name = info->name, part = info->part;
    size_t n;
    const uint8_t *bytes = slice(s.arena, s.arena_cap, name.offset, name.len, &n);
    enum text_unit unit;
    SET_STRING_ELT(names, (R_xlen_t)i, text_char(bytes, n, &unit));
    bytes = slice(s.arena, s.arena_cap, part.offset, part.len, &n);
    SEXP raw = allocVector(RAWSXP, (R_xlen_t)n);
    if (n)
      memcpy(RAW(raw), bytes, n);
    SET_VECTOR_ELT(parts, (R_xlen_t)i, raw);
    LOGICAL(hidden)[i] = (info->flags & 1) != 0;
    REAL(indexes)[i] = info->index;
  }
  /* The shared strings take no more room than their part inflates to: asking for it once
   * saves growing into it. */
  uint64_t bound = opened.strings_bytes < STRINGS_BOUND ? opened.strings_bytes : STRINGS_BOUND;
  if (!stingy && s.arena_cap < bound)
    s.arena = grow_keep(s.arena, &s.arena_cap, bound, 1);
  code = drive(STRINGS, b->state, b, &s, NULL, &out);
  if (code == HYPERTABULAR_OK) {
    b->strings_len = (size_t)out.arena_used;
    b->strings = copy_of(s.arena, b->strings_len);
    b->table_len = (size_t)out.rows;
    b->table = copy_of(s.cells, b->table_len * sizeof(HyperTabularSpan));
    code = drive(STYLES, b->state, b, &s, NULL, &out);
    if (code == HYPERTABULAR_OK) {
      b->kinds_len = (size_t)out.rows;
      b->kinds = copy_of(s.arena, b->kinds_len);
    }
  }
  scratch_free(&s);
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

struct sheet {
  HyperTabularWorkbookState *state;
  struct scratch scratch;
  struct plan plan;
  size_t per_row;
  int stingy;
  HyperTabularFilled out;
};

static void sheet_finalize(SEXP ptr) {
  struct sheet *s = R_ExternalPtrAddr(ptr);
  if (s) {
    release(s->state);
    scratch_free(&s->scratch);
    plan_free(&s->plan);
    R_Free(s);
    R_ClearExternalPtr(ptr);
  }
}

static struct sheet *sheet_of(SEXP ptr, struct book **b) {
  if (TYPEOF(ptr) != EXTPTRSXP || R_ExternalPtrTag(ptr) != install("hypertabular_sheet"))
    Rf_error("hypertabular: not a sheet");
  struct sheet *s = R_ExternalPtrAddr(ptr);
  if (!s)
    Rf_error("hypertabular: the sheet's native memory is gone (a sheet does not survive "
             "saveRDS() or a new session)");
  *b = book_of(R_ExternalPtrProtected(ptr));
  return s;
}

/* Positions a copy of the workbook's state on one sheet: the part and index its listing gave.
 * Returns list(status, payload = the sheet, failure). */
SEXP C_sheet_new(SEXP book_ptr, SEXP part, SEXP index, SEXP has_header, SEXP skip_empty_rows,
                 SEXP stingy) {
  struct book *b = book_of(book_ptr);
  if (TYPEOF(part) != RAWSXP)
    Rf_error("hypertabular: part must be a raw vector");
  struct sheet *s = R_Calloc(1, struct sheet);
  SEXP ptr = PROTECT(R_MakeExternalPtr(s, install("hypertabular_sheet"), book_ptr));
  R_RegisterCFinalizerEx(ptr, sheet_finalize, TRUE);
  s->stingy = asLogical(stingy) == TRUE;
  s->state = block(b->state_size);
  memcpy(s->state, b->state, b->state_size);
  if (s->stingy)
    scratch_init(&s->scratch, 1, 1, 1);
  else
    scratch_init(&s->scratch, 0, 4096, 64);
  int32_t code = hypertabular_workbook_sheet(s->state, b->container, b->length, RAW(part),
                                             (uintptr_t)XLENGTH(part), (uint32_t)asReal(index),
                                             asLogical(has_header) == TRUE,
                                             asLogical(skip_empty_rows) == TRUE, &s->out);
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
  struct book *b;
  struct sheet *s = sheet_of(ptr, &b);
  int unbound = s->plan.specs == NULL;
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

/* Takes the plan: `specs` packed, `width` the widest ordinal it reads plus one. */
SEXP C_sheet_bind(SEXP ptr, SEXP specs, SEXP batch_rows, SEXP width) {
  struct book *b;
  struct sheet *s = sheet_of(ptr, &b);
  if (s->plan.specs)
    Rf_error("hypertabular: the sheet already has a plan");
  plan_bind(&s->plan, specs, size_arg(batch_rows, "batch_rows"));
  s->per_row = s->plan.count + 1;
  /* A stingy scratch is left to grow into the cell table the hard way, mid-read. */
  if (!s->stingy)
    reserve_cells(&s->scratch, s->plan.batch_rows * s->per_row);
  reserve_row(&s->scratch, (size_t)asReal(width));
  return R_NilValue;
}

/* Fills the next batch: list(status, payload = the batch or NULL, failure). Rows and a failure
 * can come together: the rows before the break. */
SEXP C_sheet_fill(SEXP ptr) {
  struct book *b;
  struct sheet *s = sheet_of(ptr, &b);
  if (!s->plan.specs)
    Rf_error("hypertabular: the sheet has no plan");
  for (;;) {
    HyperTabularBuffers buffers = buffers_of(&s->scratch, b);
    int32_t code =
        hypertabular_workbook_fill(s->state, b->container, b->length, s->plan.specs, s->plan.table,
                                   s->plan.count, s->plan.batch_rows, &buffers, &s->out);
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
    size_t rows = (size_t)s->out.rows;
    SEXP failure = code == HYPERTABULAR_OK ? R_NilValue : failure_vector(&s->out.failure);
    PROTECT(failure);
    SEXP payload = R_NilValue;
    if (rows) {
      struct texts t = {b->strings, b->strings_len, s->scratch.arena, (size_t)s->out.arena_used, 0};
      payload = batch(&s->plan, rows, s->scratch.cells, s->per_row, &t);
    }
    int status = rows ? STATUS_ROWS : code == HYPERTABULAR_OK ? STATUS_END : STATUS_FAILURE;
    SEXP result = outcome(status, payload, failure);
    UNPROTECT(1);
    return result;
  }
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
SEXP C_native_version(void) { return ScalarInteger((int)hypertabular_version()); }
