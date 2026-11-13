// A C program built against an unpacked release archive, the way a consumer would: through
// CMake's find_package or pkg-config, linked to the shared or the static library. It calls all
// 23 functions hypertabular.h declares: it reads a CSV through the delimited ones, buffers
// grown when the core asks, and a sheet of corpus/workbook/basic.xlsx through the workbook
// ones, and checks the header, every cell's value or verdict and every row's line. It reads
// each again through the size-tagged calls, with tokens, notes and row widths, lists what went
// wrong, decodes an encoding, asks for a suggestion and parses a range, and reads two columns that
// declare HyperCast's constraints, their checks beside their verdicts. Then it prints the
// library's version, which run.sh holds to the package's. CMakeLists.txt builds it as C99, C11
// and, where the compiler has it, C23, warnings as errors.
//
// usage: smoke_c<std> <corpus directory>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hypertabular.h"

#define CHECK(condition)                                                                           \
  do {                                                                                             \
    if (!(condition)) {                                                                            \
      fprintf(stderr, "smoke: %s:%d: failed: %s\n", __FILE__, __LINE__, #condition);               \
      return 1;                                                                                    \
    }                                                                                              \
  } while (0)

// The length of a span, without the flag that says where its bytes are.
#define LEN(span) ((span).len & ~(uint32_t)HyperTabularSpan_FLAG)
#define FLAGGED(span) (((span).len & (uint32_t)HyperTabularSpan_FLAG) != 0)

// Whether a span of `base` holds exactly `text`.
static int is(const uint8_t *base, HyperTabularSpan span, const char *text) {
  return LEN(span) == strlen(text) && memcmp(base + span.offset, text, LEN(span)) == 0;
}

// A buffer the caller owns and grows when the core says how much it needs, keeping what it
// holds: a workbook call stops in place and resumes in the grown buffer.
static int grow(void **buffer, uintptr_t *cap, uint64_t needed, size_t element) {
  uintptr_t want = (uintptr_t)needed > *cap ? (uintptr_t)needed : *cap + 1;
  void *grown = realloc(*buffer, want * element);
  if (grown == NULL) {
    return 0;
  }
  memset((uint8_t *)grown + *cap * element, 0, (want - *cap) * element);
  *buffer = grown;
  *cap = want;
  return 1;
}

// The workbook buffers, each grown on the code that names it. Any other code is returned.
static int32_t settle(int32_t code, HyperTabularBuffers *buffers, uint64_t needed, int *grown) {
  *grown = 0;
  if (code == HYPERTABULAR_ERR_WINDOW) {
    *grown = grow((void **)&buffers->window, &buffers->window_cap, needed, 1);
  } else if (code == HYPERTABULAR_ERR_ARENA) {
    *grown = grow((void **)&buffers->arena, &buffers->arena_cap, needed, 1);
  } else if (code == HYPERTABULAR_ERR_CELLS) {
    *grown = grow((void **)&buffers->cells, &buffers->cells_cap, needed, sizeof(HyperTabularSpan));
  } else {
    return code;
  }
  return *grown ? 1 : code;
}

static int delimited(void) {
  static const char csv[] = "id,name,amount\n1,\"bob, \"\"jr\"\"\",12.50\n\n2,,x\n";
  const uint8_t *input = (const uint8_t *)csv;
  const uintptr_t input_len = sizeof(csv) - 1;
  uint64_t block[HyperTabularDelimitedState_SIZE / 8];
  HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
  HyperTabularRawDialect dialect = {'"', 1, 1, 0};
  HyperTabularSpan names[8];
  uint8_t arena[256];
  HyperTabularFilled out;
  uintptr_t at = 0;

  CHECK(hypertabular_delimited_state_size() == HyperTabularDelimitedState_SIZE);
  // A quote cannot separate cells; tab or other printable ASCII can.
  CHECK(hypertabular_delimited_init(state, &dialect) == HYPERTABULAR_ERR_CONTRACT);
  dialect.separator = ',';
  CHECK(hypertabular_delimited_init(state, &dialect) == HYPERTABULAR_OK);

  // The header, into a name table too small for it: nothing is consumed, and the core says
  // how many names it needs.
  memset(&out, 0, sizeof out);
  CHECK(hypertabular_delimited_header(state, input, input_len, 1, names, 1, arena, sizeof arena,
                                      &out) == HYPERTABULAR_ERR_CELLS);
  CHECK(out.needed == 3);
  CHECK(hypertabular_delimited_header(state, input, input_len, 1, names, 8, arena, sizeof arena,
                                      &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 3 && out.consumed == 15);
  CHECK(is(input, names[0], "id") && is(input, names[1], "name") && is(input, names[2], "amount"));
  CHECK(state->records == 1 && state->expected == 3);
  at += (uintptr_t)out.consumed;

  {
    // id as i32, name as text, amount as a decimal: a value and a verdict per row each.
    HyperTabularColumnSpec specs[3];
    int32_t ids[4];
    HyperTabularSpan texts[4];
    HyperTabularDecimal amounts[4];
    HyperTabularCellVerdict verdicts[3][4];
    HyperTabularColumnBuffer columns[3];
    HyperTabularSpan cells[64];
    HyperTabularBuffers buffers;
    uint8_t unescaped[16];
    uintptr_t written;
    const uintptr_t per_row = 4; // the widest ordinal's cells, and the row's line
    int i;

    memset(specs, 0, sizeof specs);
    for (i = 0; i < 3; ++i) {
      specs[i].ordinal = (uint32_t)i;
      // The invariant notation, `.` and `,`, declared in full: size, separators and flags.
      specs[i].format.size = HyperTabularRawNumFormat_SIZE;
      specs[i].format.format.flags = HyperTabularNumFormat_ALL;
      specs[i].format.format.decimal_sep.bytes[0] = '.';
      specs[i].format.format.decimal_sep.len = 1;
      specs[i].format.format.group_sep.bytes[0] = ',';
      specs[i].format.format.group_sep.len = 1;
    }
    specs[0].door = 4;  // i32
    specs[1].door = 18; // text
    specs[2].door = 19; // decimal
    columns[0].values = ids;
    columns[1].values = texts;
    columns[2].values = amounts;
    for (i = 0; i < 3; ++i) {
      columns[i].verdicts = verdicts[i];
    }
    memset(&buffers, 0, sizeof buffers);
    buffers.arena = arena;
    buffers.arena_cap = sizeof arena;
    buffers.cells = cells;
    buffers.cells_cap = sizeof cells / sizeof cells[0];

    CHECK(hypertabular_delimited_fill(state, input + at, input_len - at, 1, specs, columns, 3, 4,
                                      &buffers, &out) == HYPERTABULAR_OK);
    CHECK(out.rows == 2 && at + out.consumed == input_len);
    // The blank line between the rows is skipped: the second starts on line 4.
    CHECK(cells[per_row - 1].len == 2 && cells[2 * per_row - 1].len == 4);

    CHECK(verdicts[0][0].reason == 0 && ids[0] == 1);
    CHECK(verdicts[0][1].reason == 0 && ids[1] == 2);
    // A quoted cell with a doubled quote in it is unescaped into the arena, its span flagged.
    CHECK(verdicts[1][0].reason == 0 && FLAGGED(texts[0]) && is(arena, texts[0], "bob, \"jr\""));
    // No bytes is the one way text fails: Empty.
    CHECK(verdicts[1][1].reason == 1);
    CHECK(verdicts[2][0].reason == 0);
    // 12.50, canonical: 125 at scale 1.
    CHECK(amounts[0].lo == 125 && amounts[0].hi == 0 && amounts[0].scale == 1 &&
          !amounts[0].negative);
    // Malformed, with the span of the cell's own text.
    CHECK(verdicts[2][1].reason == 2 && verdicts[2][1].offset == 0 && verdicts[2][1].len == 1);

    // The cell table holds the raw cells: the quoted one flagged, read with unescape.
    CHECK(FLAGGED(cells[1]));
    written = hypertabular_delimited_unescape(input + at + cells[1].offset, LEN(cells[1]),
                                              unescaped, sizeof unescaped);
    CHECK(written == 9 && memcmp(unescaped, "bob, \"jr\"", 9) == 0);

    // The input is over: four records read, the skipped blank line among them.
    at = input_len;
    CHECK(hypertabular_delimited_fill(state, input + at, 0, 1, specs, columns, 3, 4, &buffers,
                                      &out) == HYPERTABULAR_OK);
    CHECK(out.rows == 0 && state->records == 4);
  }
  return 0;
}

// The invariant notation, declared in full: size, separators and flags.
static HyperTabularRawNumFormat invariant(void) {
  HyperTabularRawNumFormat format;
  memset(&format, 0, sizeof format);
  format.size = HyperTabularRawNumFormat_SIZE;
  format.format.flags = HyperTabularNumFormat_ALL;
  format.format.decimal_sep.bytes[0] = '.';
  format.format.decimal_sep.len = 1;
  format.format.group_sep.bytes[0] = ',';
  format.format.group_sep.len = 1;
  return format;
}

// A delimited read through the size-tagged calls: a junk line passed over, a token, a short row
// padded, a long row reported, and what went wrong listed afterward.
static int delimited_with(void) {
  static const char csv[] = "junk\nid,v\n1,NA\n2,x\n3,4,5\n6\n";
  static const uint8_t tokens[] = {2, 0, 0, 0, 'N', 'A'}; // a length, then its bytes
  const uint8_t *input = (const uint8_t *)csv;
  const uintptr_t input_len = sizeof(csv) - 1;
  uint64_t block[HyperTabularDelimitedState_SIZE / 8];
  HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
  HyperTabularRawDelimitedOptions options;
  HyperTabularSpan names[4];
  uint8_t arena[64];
  HyperTabularFilled out;
  HyperTabularColumnSpecWith specs[2];
  int32_t values[2][8];
  HyperTabularCellVerdict verdicts[2][8];
  HyperTabularCellNote notes[2][8];
  HyperTabularColumnBufferWith columns[2];
  HyperTabularSpan cells[64];
  uint32_t widths[8];
  HyperTabularBuffersWith buffers;
  HyperTabularRawProblem problems[1];
  uint64_t cursor = 0;
  uintptr_t written = 0;
  uintptr_t at;
  int i;

  memset(&options, 0, sizeof options);
  options.size = HyperTabularRawDelimitedOptions_SIZE;
  options.separator = '"';
  CHECK(hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_ERR_CONTRACT);
  options.separator = ',';
  options.quoting = 1;
  options.skip_blank_lines = 1;
  options.skip = 1;
  options.short_rows = HyperTabularRawDelimitedOptions_SHORT_ROWS_PAD;
  options.long_rows = HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT;
  CHECK(hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_OK);

  memset(&out, 0, sizeof out);
  CHECK(hypertabular_delimited_header(state, input, input_len, 1, names, 4, arena, sizeof arena,
                                      &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 2 && is(input, names[0], "id") && is(input, names[1], "v"));
  CHECK(state->line == 3);
  at = (uintptr_t)out.consumed;

  memset(specs, 0, sizeof specs);
  memset(columns, 0, sizeof columns);
  for (i = 0; i < 2; ++i) {
    specs[i].size = HyperTabularColumnSpecWith_SIZE;
    specs[i].ordinal = (uint32_t)i;
    specs[i].door = 4; // i32
    specs[i].format = invariant();
    columns[i].size = HyperTabularColumnBufferWith_SIZE;
    columns[i].values = values[i];
    columns[i].verdicts = verdicts[i];
    columns[i].notes = notes[i];
  }
  memset(&buffers, 0, sizeof buffers);
  buffers.size = HyperTabularBuffersWith_SIZE;
  buffers.arena = arena;
  buffers.arena_cap = sizeof arena;
  buffers.cells = cells;
  buffers.cells_cap = sizeof cells / sizeof cells[0];
  buffers.tokens = tokens;
  buffers.tokens_len = sizeof tokens;
  buffers.row_widths = widths;

  CHECK(hypertabular_delimited_fill_with(state, input + at, input_len - at, 1, specs, columns, 2, 8,
                                         &buffers, &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 4 && at + out.consumed == input_len);
  CHECK(widths[0] == 2 && widths[1] == 2 && widths[2] == 3 && widths[3] == 1);
  CHECK(verdicts[0][0].reason == 0 && values[0][0] == 1);
  // The token is Empty with no door asked.
  CHECK(verdicts[1][0].reason == 1 && notes[1][0].origin == HyperTabularCellNote_ORIGIN_TOKEN);
  // The door's own refusal says what failed.
  CHECK(verdicts[1][1].reason == 2 && notes[1][1].origin == HyperTabularCellNote_ORIGIN_CELL &&
        notes[1][1].detail != 0);
  // The long row is cut to the width, and the short one padded.
  CHECK(verdicts[1][2].reason == 0 && values[1][2] == 4);
  CHECK(verdicts[1][3].reason == 1 && notes[1][3].origin == HyperTabularCellNote_ORIGIN_PADDED);

  // What went wrong, in a list too short for it: the cursor carries on from where it stopped.
  CHECK(hypertabular_faults(columns, 2, 4, widths, state->expected,
                            HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT, &cursor, problems, 1,
                            &written) == HYPERTABULAR_OK);
  CHECK(written == 1 && problems[0].row == 1 && problems[0].plan_column == 1 &&
        problems[0].reason == 2 && problems[0].detail == notes[1][1].detail &&
        problems[0].offset == 0 && problems[0].len == 1);
  CHECK(hypertabular_faults(columns, 2, 4, widths, state->expected,
                            HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT, &cursor, problems, 1,
                            &written) == HYPERTABULAR_OK);
  CHECK(written == 1 && problems[0].row == 2 &&
        problems[0].plan_column == HyperTabularRawProblem_ROW_LEVEL && problems[0].expected == 2 &&
        problems[0].found == 3);
  CHECK(hypertabular_faults(columns, 2, 4, widths, state->expected,
                            HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT, &cursor, problems, 1,
                            &written) == HYPERTABULAR_OK);
  CHECK(written == 0);
  return 0;
}

// Two columns that declare HyperCast's constraints: an i32 at most 10, and text one of `Female` or
// `Male`. A cell that cast and broke one is a violation, its check naming the keyword; a text match
// names the entry. A constrained column's checks are required, and its declaration is the core's to
// judge, as the batch's size is.
static int constrained(void) {
  static const char csv[] = "7,MALE\n11,other\nx,Female\n";
  // The enum as HyperCast packs text: a little-endian length, then the bytes, per entry.
  static const uint8_t sexes[] = {6,   0, 0, 0, 'F', 'e', 'm', 'a', 'l',
                                  'e', 4, 0, 0, 0,   'M', 'a', 'l', 'e'};
  uint64_t block[HyperTabularDelimitedState_SIZE / 8];
  HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
  HyperTabularRawDelimitedOptions options;
  HyperTabularRawConstraints at_most;
  HyperTabularRawConstraints one_of;
  HyperTabularColumnSpecWith specs[2];
  int32_t numbers[4];
  HyperTabularSpan texts[4];
  HyperTabularCellVerdict verdicts[2][4];
  HyperTabularRawCheck checks[2][4];
  HyperTabularColumnBufferWith columns[2];
  HyperTabularSpan cells[16];
  uint8_t arena[16];
  HyperTabularBuffersWith buffers;
  HyperTabularFilled out;
  HyperTabularRawProblem problems[4];
  HyperTabularRawBatchCheck plain;
  HyperTabularRawBatchCheck checked;
  uint64_t cursor = 0;
  uintptr_t written = 0;
  const uint8_t *input = (const uint8_t *)csv;
  int i;

  memset(&at_most, 0, sizeof at_most);
  at_most.size = HyperTabularRawConstraints_SIZE;
  at_most.flags = HyperTabularRawConstraints_HAS_MAXIMUM;
  at_most.maximum.lo = 10;
  memset(&one_of, 0, sizeof one_of);
  one_of.size = HyperTabularRawConstraints_SIZE;
  one_of.flags = HyperTabularRawConstraints_HAS_ENUM;

  memset(&options, 0, sizeof options);
  options.size = HyperTabularRawDelimitedOptions_SIZE;
  options.separator = ',';
  options.quoting = 1;
  CHECK(hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_OK);
  memset(specs, 0, sizeof specs);
  memset(columns, 0, sizeof columns);
  for (i = 0; i < 2; ++i) {
    specs[i].size = HyperTabularColumnSpecWith_SIZE;
    specs[i].ordinal = (uint32_t)i;
    specs[i].format = invariant();
    columns[i].size = HyperTabularColumnBufferWith_SIZE;
    columns[i].verdicts = verdicts[i];
    columns[i].checks = checks[i];
  }
  specs[0].door = 4; // i32
  specs[0].constraints = &at_most;
  specs[1].door = 18; // text
  specs[1].constraints = &one_of;
  specs[1].enum_ptr = sexes;
  specs[1].enum_len = sizeof sexes;
  columns[0].values = numbers;
  columns[1].values = texts;
  memset(&buffers, 0, sizeof buffers);
  buffers.size = HyperTabularBuffersWith_SIZE;
  buffers.arena = arena;
  buffers.arena_cap = sizeof arena;
  buffers.cells = cells;
  buffers.cells_cap = sizeof cells / sizeof cells[0];

  // A constrained column without its checks is refused before a cell is read; so is a declaration
  // the core refuses, here an enum list passed with HAS_ENUM clear.
  columns[1].checks = NULL;
  CHECK(hypertabular_delimited_fill_with(state, input, sizeof(csv) - 1, 1, specs, columns, 2, 4,
                                         &buffers, &out) == HYPERTABULAR_ERR_CONTRACT);
  columns[1].checks = checks[1];
  one_of.flags = 0;
  CHECK(hypertabular_delimited_fill_with(state, input, sizeof(csv) - 1, 1, specs, columns, 2, 4,
                                         &buffers, &out) == HYPERTABULAR_ERR_CONTRACT);
  one_of.flags = HyperTabularRawConstraints_HAS_ENUM;

  memset(&out, 0, sizeof out);
  CHECK(hypertabular_delimited_fill_with(state, input, sizeof(csv) - 1, 1, specs, columns, 2, 4,
                                         &buffers, &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 3);
  // 7 passes; MALE matches the second entry, under simple case folding.
  CHECK(verdicts[0][0].reason == 0 && numbers[0] == 7 && checks[0][0].adjusted == 0 &&
        checks[0][0].enum_index == UINT32_MAX);
  CHECK(verdicts[1][0].reason == 0 && is(input, texts[0], "MALE") && checks[1][0].enum_index == 1);
  // 11 breaks maximum (3), cast all the same; other breaks enum (8).
  CHECK(verdicts[0][1].reason == HyperTabularCellVerdict_VIOLATION && numbers[1] == 11 &&
        checks[0][1].keyword == 3 && verdicts[0][1].offset == 0 && verdicts[0][1].len == 2);
  CHECK(verdicts[1][1].reason == HyperTabularCellVerdict_VIOLATION && checks[1][1].keyword == 8 &&
        is(input, texts[1], "other"));
  // x is the door's own fault; Female matches the first entry.
  CHECK(verdicts[0][2].reason == 2);
  CHECK(verdicts[1][2].reason == 0 && checks[1][2].enum_index == 0);

  // The list of what went wrong names each violation's keyword, and a fault's reason alone.
  CHECK(hypertabular_faults(columns, 2, 3, NULL, 0,
                            HyperTabularRawDelimitedOptions_LONG_ROWS_REJECT, &cursor, problems, 4,
                            &written) == HYPERTABULAR_OK);
  CHECK(written == 3);
  CHECK(problems[0].row == 1 && problems[0].plan_column == 0 &&
        problems[0].reason == HyperTabularCellVerdict_VIOLATION && problems[0].keyword == 3 &&
        problems[0].offset == 0 && problems[0].len == 2);
  CHECK(problems[1].row == 1 && problems[1].plan_column == 1 && problems[1].keyword == 8);
  CHECK(problems[2].row == 2 && problems[2].plan_column == 0 && problems[2].reason == 2 &&
        problems[2].keyword == 0);

  // A constrained column's checks count toward what a batch asks for.
  memset(&plain, 0, sizeof plain);
  memset(&checked, 0, sizeof checked);
  CHECK(hypertabular_batch_check(&specs[1], 1, 1024, 1024, 0, &checked) == HYPERTABULAR_OK);
  specs[1].constraints = NULL;
  specs[1].enum_ptr = NULL;
  specs[1].enum_len = 0;
  CHECK(hypertabular_batch_check(&specs[1], 1, 1024, 1024, 0, &plain) == HYPERTABULAR_OK);
  CHECK(checked.needed == plain.needed + 1024 * HyperTabularRawCheck_SIZE);
  return 0;
}

// One read of a plan of `flags` over two columns, and what its exports say of a stale format: `351`
// and `863` hold the retired grouping bit. `95` and `607`, Western and Indian, are their controls.
static int32_t fill_grouping(const char *input, uint32_t first, uint32_t second,
                             int32_t values[2][2], HyperTabularCellVerdict verdicts[2][2]) {
  uint64_t block[HyperTabularDelimitedState_SIZE / 8];
  HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
  HyperTabularRawDelimitedOptions options;
  HyperTabularColumnSpecWith specs[2];
  HyperTabularColumnBufferWith columns[2];
  HyperTabularBuffersWith buffers;
  HyperTabularSpan cells[16];
  uint8_t arena[64];
  HyperTabularFilled out;
  int i;

  memset(&options, 0, sizeof options);
  options.size = HyperTabularRawDelimitedOptions_SIZE;
  options.separator = ',';
  options.quoting = 1;
  options.skip_blank_lines = 1;
  CHECK(hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_OK);
  memset(specs, 0, sizeof specs);
  memset(columns, 0, sizeof columns);
  for (i = 0; i < 2; ++i) {
    specs[i].size = HyperTabularColumnSpecWith_SIZE;
    specs[i].ordinal = (uint32_t)i;
    specs[i].door = 4; // i32
    specs[i].format = invariant();
    specs[i].format.format.flags = i == 0 ? first : second;
    columns[i].size = HyperTabularColumnBufferWith_SIZE;
    columns[i].values = values[i];
    columns[i].verdicts = verdicts[i];
  }
  memset(&buffers, 0, sizeof buffers);
  buffers.size = HyperTabularBuffersWith_SIZE;
  buffers.arena = arena;
  buffers.arena_cap = sizeof arena;
  buffers.cells = cells;
  buffers.cells_cap = sizeof cells / sizeof cells[0];
  memset(&out, 0, sizeof out);
  memset(values, 0, 2 * 2 * sizeof(int32_t));
  memset(verdicts, 0xAA, 2 * 2 * sizeof(HyperTabularCellVerdict));
  return hypertabular_delimited_fill_with(state, (const uint8_t *)input, strlen(input), 1, specs,
                                          columns, 2, 2, &buffers, &out);
}

// A column's grouping is its own: an Indian column beside a Western one, each refusing what the
// other reads. No format is Western, and a stale word is refused before any cell, by the fills and
// by the suggester.
static int grouping(void) {
  static const char both[] = "\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";
  static const uint32_t stale[] = {351, 863};
  static const char shape[] = "1,23,456";
  static const HyperTabularSpan one[1] = {{0, 8}};
  const uint32_t western = HyperTabularNumFormat_ALL;
  const uint32_t indian = HyperTabularNumFormat_ALL | HyperTabularNumFormat_GROUPING_INDIAN;
  int32_t values[2][2];
  HyperTabularCellVerdict verdicts[2][2];
  HyperTabularRawNumFormat format;
  HyperTabularRawSuggestion found;
  size_t i;
  int declared;

  CHECK(HyperTabularRawSuggestion_NOTATION_FLAGS == 95 && western == 95 && indian == 607);
  CHECK(fill_grouping(both, indian, western, values, verdicts) == HYPERTABULAR_OK);
  CHECK(verdicts[0][0].reason == 0 && values[0][0] == 123456);
  CHECK(verdicts[1][0].reason == 2);
  CHECK(verdicts[0][1].reason == 2);
  CHECK(verdicts[1][1].reason == 0 && values[1][1] == 1234567);

  for (i = 0; i < sizeof stale / sizeof stale[0]; ++i) {
    CHECK(fill_grouping(both, stale[i], western, values, verdicts) == HYPERTABULAR_ERR_CONTRACT);
    CHECK(fill_grouping(both, western, stale[i], values, verdicts) == HYPERTABULAR_ERR_CONTRACT);
    CHECK(verdicts[0][0].reason == 0xAAAAAAAAu &&
          verdicts[1][1].reason == 0xAAAAAAAAu); // no cell was read

    format = invariant();
    format.format.flags = stale[i];
    CHECK(hypertabular_suggest((const uint8_t *)shape, 8, NULL, 0, one, NULL, 1, 1, &format,
                               &found) == HYPERTABULAR_ERR_CONTRACT);
    format.size = 0; // the word isn't lost because the size is
    CHECK(hypertabular_suggest((const uint8_t *)shape, 8, NULL, 0, one, NULL, 1, 1, &format,
                               &found) == HYPERTABULAR_ERR_CONTRACT);
  }

  // A plan judged before any input: a fill of no rows, on a state of its own, and nothing written.
  for (i = 0; i < 3; ++i) {
    static const uint32_t words[] = {351, 863, 607};
    uint64_t block[HyperTabularDelimitedState_SIZE / 8];
    HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
    HyperTabularRawDelimitedOptions options;
    HyperTabularColumnSpecWith spec;
    HyperTabularColumnBufferWith column;
    HyperTabularBuffersWith memory;
    HyperTabularFilled out;
    uint64_t value = 0;
    HyperTabularCellVerdict verdict;

    memset(&options, 0, sizeof options);
    options.size = HyperTabularRawDelimitedOptions_SIZE;
    options.separator = ',';
    CHECK(hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_OK);
    memset(&spec, 0, sizeof spec);
    spec.size = HyperTabularColumnSpecWith_SIZE;
    spec.door = 4; // i32
    spec.format = invariant();
    spec.format.format.flags = words[i];
    memset(&column, 0, sizeof column);
    column.size = HyperTabularColumnBufferWith_SIZE;
    column.values = &value;
    column.verdicts = &verdict;
    memset(&memory, 0, sizeof memory);
    memory.size = HyperTabularBuffersWith_SIZE;
    memset(&out, 0, sizeof out);
    verdict.reason = 0xAAAAAAAAu;
    CHECK(
        (hypertabular_delimited_fill_with(state, NULL, 0, 0, &spec, &column, 1, 0, &memory, &out) ==
         HYPERTABULAR_ERR_CONTRACT) == (words[i] != 607));
    CHECK(verdict.reason == 0xAAAAAAAAu && out.rows == 0);
  }

  // Undeclared, an Indian shape has no culture. Declared, it's a number.
  for (declared = 0; declared < 3; ++declared) {
    format = invariant();
    format.format.flags = declared == 2 ? indian : western;
    if (declared == 0) {
      format.size = 0;
    }
    CHECK(hypertabular_suggest((const uint8_t *)shape, 8, NULL, 0, one, NULL, 1, 1, &format,
                               &found) == HYPERTABULAR_OK);
    CHECK(found.status == HyperTabularRawSuggestion_RESOLVED);
    CHECK(found.door == (declared == 2 ? 4u : 18u));
  }
  return 0;
}

// One whole read of grouping.xlsx's third column through two formats: an Indian column beside a
// Western one, the buffers grown as a workbook call asks. A plan's stale word stops the first fill,
// before any cell; the file's in `bytes`.
static int32_t sheet_grouping(const uint8_t *bytes, size_t size, uint32_t first, uint32_t second,
                              int32_t values[2][4], HyperTabularCellVerdict verdicts[2][4],
                              uint64_t *rows) {
  uint64_t *block = (uint64_t *)calloc((hypertabular_workbook_state_size() + 7) / 8, 8);
  HyperTabularWorkbookState *state = (HyperTabularWorkbookState *)block;
  HyperTabularBuffers buffers;
  HyperTabularBuffersWith with;
  HyperTabularOpened opened;
  HyperTabularFilled out;
  HyperTabularSheetInfo info;
  HyperTabularColumnSpecWith specs[2];
  HyperTabularColumnBufferWith columns[2];
  uint8_t *strings, *kinds;
  HyperTabularSpan *table;
  uintptr_t strings_len, table_len, kinds_len, part_len;
  char part[64];
  int32_t code;
  int grown, i;

  if (block == NULL) {
    return -100;
  }
  memset(&buffers, 0, sizeof buffers);
  buffers.window = (uint8_t *)calloc(1, 1);
  buffers.window_cap = 1;
  buffers.arena = (uint8_t *)calloc(1, 1);
  buffers.arena_cap = 1;
  buffers.cells = (HyperTabularSpan *)calloc(1, sizeof(HyperTabularSpan));
  buffers.cells_cap = 1;
  do {
    memset(&opened, 0, sizeof opened);
    code = hypertabular_workbook_open(state, bytes, (uintptr_t)size, &buffers, &opened);
  } while (settle(code, &buffers, opened.needed, &grown) == 1);
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_sheets(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  memcpy(&info, buffers.cells, sizeof info);
  part_len = LEN(info.part);
  memcpy(part, buffers.arena + info.part.offset, part_len);
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_strings(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  strings_len = (uintptr_t)out.arena_used;
  table_len = (uintptr_t)out.rows;
  strings = (uint8_t *)malloc(strings_len + 1);
  table = (HyperTabularSpan *)malloc((table_len + 1) * sizeof(HyperTabularSpan));
  memcpy(strings, buffers.arena, strings_len);
  memcpy(table, buffers.cells, table_len * sizeof(HyperTabularSpan));
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_styles(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  kinds_len = (uintptr_t)out.rows;
  kinds = (uint8_t *)malloc(kinds_len + 1);
  memcpy(kinds, buffers.arena, kinds_len);
  buffers.strings = strings;
  buffers.strings_len = strings_len;
  buffers.table = table;
  buffers.table_len = table_len;
  buffers.kinds = kinds;
  buffers.kinds_len = kinds_len;
  buffers.row = (HyperTabularSlot *)calloc(16, sizeof(HyperTabularSlot));
  buffers.row_cap = 16;

  memset(&out, 0, sizeof out);
  code = hypertabular_workbook_sheet(state, bytes, (uintptr_t)size, (const uint8_t *)part, part_len,
                                     info.index, 1, 1, &out);
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_header(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);

  memset(specs, 0, sizeof specs);
  memset(columns, 0, sizeof columns);
  for (i = 0; i < 2; ++i) {
    specs[i].size = HyperTabularColumnSpecWith_SIZE;
    specs[i].ordinal = 2;
    specs[i].door = 4; // i32
    specs[i].format = invariant();
    specs[i].format.format.flags = i == 0 ? first : second;
    columns[i].size = HyperTabularColumnBufferWith_SIZE;
    columns[i].values = values[i];
    columns[i].verdicts = verdicts[i];
  }
  do {
    memset(&with, 0, sizeof with);
    with.size = HyperTabularBuffersWith_SIZE;
    with.window = buffers.window;
    with.window_cap = buffers.window_cap;
    with.arena = buffers.arena;
    with.arena_cap = buffers.arena_cap;
    with.cells = buffers.cells;
    with.cells_cap = buffers.cells_cap;
    with.row = buffers.row;
    with.row_cap = buffers.row_cap;
    with.strings = buffers.strings;
    with.strings_len = buffers.strings_len;
    with.table = buffers.table;
    with.table_len = buffers.table_len;
    with.kinds = buffers.kinds;
    with.kinds_len = buffers.kinds_len;
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_fill_with(state, bytes, (uintptr_t)size, specs, columns, 2, 4,
                                           &with, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  *rows = out.rows;

  free(buffers.window);
  free(buffers.arena);
  free(buffers.cells);
  free(buffers.row);
  free(strings);
  free(table);
  free(kinds);
  free(block);
  return code;
}

// The same in a workbook: each column's grouping its own, on a text cell, and a native number the
// same under both. A stale word in either column stops the first fill, with no cell read.
static int workbook_grouping(const char *corpus) {
  static const uint32_t stale[] = {351, 863};
  const uint32_t western = HyperTabularNumFormat_ALL;
  const uint32_t indian = HyperTabularNumFormat_ALL | HyperTabularNumFormat_GROUPING_INDIAN;
  char path[4096];
  FILE *file;
  uint8_t *bytes;
  long size;
  int32_t values[2][4];
  HyperTabularCellVerdict verdicts[2][4];
  uint64_t rows = 0;
  size_t i;

  CHECK(strlen(corpus) + 32 < sizeof path);
  sprintf(path, "%s/workbook/grouping.xlsx", corpus);
  file = fopen(path, "rb");
  CHECK(file != NULL);
  CHECK(fseek(file, 0, SEEK_END) == 0 && (size = ftell(file)) > 0 && fseek(file, 0, SEEK_SET) == 0);
  bytes = (uint8_t *)malloc((size_t)size);
  CHECK(bytes != NULL && fread(bytes, 1, (size_t)size, file) == (size_t)size);
  fclose(file);

  CHECK(sheet_grouping(bytes, (size_t)size, indian, western, values, verdicts, &rows) ==
        HYPERTABULAR_OK);
  CHECK(rows == 3);
  CHECK(verdicts[0][0].reason == 0 && values[0][0] == 123456);
  CHECK(verdicts[1][0].reason == 2);
  CHECK(verdicts[0][1].reason == 2);
  CHECK(verdicts[1][1].reason == 0 && values[1][1] == 1234567);
  CHECK(verdicts[0][2].reason == 0 && values[0][2] == 1234);
  CHECK(verdicts[1][2].reason == 0 && values[1][2] == 1234);

  for (i = 0; i < sizeof stale / sizeof stale[0]; ++i) {
    memset(verdicts, 0xAA, sizeof verdicts);
    rows = 99;
    CHECK(sheet_grouping(bytes, (size_t)size, stale[i], western, values, verdicts, &rows) ==
          HYPERTABULAR_ERR_CONTRACT);
    CHECK(sheet_grouping(bytes, (size_t)size, western, stale[i], values, verdicts, &rows) ==
          HYPERTABULAR_ERR_CONTRACT);
    CHECK(rows == 0 && verdicts[0][0].reason == 0xAAAAAAAAu &&
          verdicts[1][0].reason == 0xAAAAAAAAu); // no cell was read
  }
  free(bytes);
  return 0;
}

// A declared encoding, decoded ahead of the fill, stopping at a unit with no character.
static int transcode(void) {
  static const uint8_t input[] = {'c', 'a', 'f', 0xE9};
  static const uint8_t wide[] = {'x', 0, 0x00, 0xDC, 'y', 0}; // a low surrogate alone
  uint8_t out[16];
  HyperTabularRawTranscoded progress;

  memset(&progress, 0, sizeof progress);
  CHECK(hypertabular_transcode(HyperTabularRawTranscoded_WINDOWS_1252, input, sizeof input, 1, out,
                               sizeof out, &progress) == HYPERTABULAR_OK);
  CHECK(progress.consumed == 4 && progress.written == 5 && !progress.failed);
  CHECK(memcmp(out, "caf\xC3\xA9", 5) == 0);
  CHECK(hypertabular_transcode(HyperTabularRawTranscoded_UTF_16LE, wide, sizeof wide, 1, out,
                               sizeof out, &progress) == HYPERTABULAR_OK);
  CHECK(progress.failed && progress.failure_at == 2 && progress.written == 1 && out[0] == 'x');
  CHECK(hypertabular_transcode(0, input, sizeof input, 1, out, sizeof out, &progress) ==
        HYPERTABULAR_ERR_CONTRACT);
  return 0;
}

// A sample's columns against the doors: one settled, and one a declaration has to settle. The
// cells run down each column.
static int suggest(void) {
  static const char text[] = "1201/02/202403/04/2024";
  static const HyperTabularSpan cells[4] = {{0, 1}, {1, 1}, {2, 10}, {12, 10}};
  static const char number[] = "1.234";
  static const HyperTabularSpan spans[2] = {{0, 5}, {0, 5}};
  static const uint8_t marks[2] = {1, 0};
  HyperTabularRawNumFormat formats[2];
  HyperTabularRawSuggestion found[2];

  memset(formats, 0, sizeof formats); // a size of 0 declares nothing
  memset(found, 0, sizeof found);
  CHECK(hypertabular_suggest((const uint8_t *)text, sizeof(text) - 1, NULL, 0, cells, NULL, 2, 2,
                             formats, found) == HYPERTABULAR_OK);
  CHECK(found[0].status == HyperTabularRawSuggestion_RESOLVED && found[0].door == 4 &&
        found[0].matched == 2 && found[0].sampled == 2);
  // Month-first or day-first, code 2 and code 3, and the door under the first of them.
  CHECK(found[1].status == HyperTabularRawSuggestion_AMBIGUOUS && found[1].door == 20 &&
        found[1].param == 12 && found[1].matched == 2);
  CHECK(hypertabular_suggest((const uint8_t *)text, sizeof(text) - 1, NULL, 0, NULL, NULL, 2, 2,
                             formats, found) == HYPERTABULAR_ERR_CONTRACT);
  // The same digits as a workbook's typed number and as text: only the text asks.
  CHECK(hypertabular_suggest((const uint8_t *)number, sizeof(number) - 1, NULL, 0, spans, marks, 1,
                             2, NULL, found) == HYPERTABULAR_OK);
  CHECK(found[0].status == HyperTabularRawSuggestion_RESOLVED && found[0].door == 11 &&
        found[0].param == 0);
  CHECK(found[1].status == HyperTabularRawSuggestion_AMBIGUOUS && found[1].door == 11 &&
        found[1].param == 6);
  return 0;
}

// How much a batch may ask for is the core's to say: the bytes it would take, and the limit.
static int batch_check(void) {
  HyperTabularColumnSpecWith spec;
  HyperTabularRawBatchCheck found;

  memset(&spec, 0, sizeof spec);
  spec.size = HyperTabularColumnSpecWith_SIZE;
  spec.door = 4; // i32
  memset(&found, 0, sizeof found);
  CHECK(hypertabular_batch_check(&spec, 1, UINT64_C(1) << 16, 1024, HyperTabularRawBatchCheck_NOTES,
                                 &found) == HYPERTABULAR_OK);
  CHECK(found.refused == HyperTabularRawBatchCheck_REFUSED_NONE && found.buffer_bytes == 1024);
  CHECK(found.limit == UINT64_C(4294967296) && found.needed > 0 && found.needed < found.limit);
  CHECK(hypertabular_batch_check(&spec, 1, UINT64_C(1) << 28, 1024, HyperTabularRawBatchCheck_NOTES,
                                 &found) == HYPERTABULAR_ERR_BATCH);
  CHECK(found.refused == HyperTabularRawBatchCheck_REFUSED_OVER_LIMIT);
  CHECK(found.needed == UINT64_C(5377130496));
  CHECK(hypertabular_batch_check(&spec, 1, 0, 1024, 0, &found) == HYPERTABULAR_ERR_BATCH);
  CHECK(found.refused == HyperTabularRawBatchCheck_REFUSED_BATCH_ROWS_ZERO);
  CHECK(hypertabular_batch_check(&spec, 1, 1, 0, 0, &found) == HYPERTABULAR_ERR_BATCH);
  CHECK(found.refused == HyperTabularRawBatchCheck_REFUSED_BUFFER_BYTES_ZERO);
  /* A sampler's rows: within the core's budget, at a size it would refuse a read. */
  CHECK(hypertabular_batch_check(&spec, 1, UINT64_C(1) << 28, 1024, HyperTabularRawBatchCheck_NOTES,
                                 &found) == HYPERTABULAR_ERR_BATCH);
  CHECK(found.sample_rows > 0 && found.sample_rows < (UINT64_C(1) << 28));
  /* One row past the limit, at any batch size, with its width. */
  spec.ordinal = 0xFFFFFFFFu;
  CHECK(hypertabular_batch_check(&spec, 1, 1, 0, HyperTabularRawBatchCheck_SHEET, &found) ==
        HYPERTABULAR_ERR_BATCH);
  CHECK(found.refused == HyperTabularRawBatchCheck_REFUSED_ROW_TOO_WIDE &&
        found.width == UINT64_C(4294967296));
  CHECK(found.needed == UINT64_C(68719476768) && found.sample_rows == 0);
  spec.ordinal = 0;
  CHECK(hypertabular_batch_check(NULL, 1, 1, 1, 0, &found) == HYPERTABULAR_ERR_CONTRACT);
  return 0;
}

static int range(void) {
  static const char text[] = "B2:C30";
  HyperTabularRawRange parsed;

  memset(&parsed, 0, sizeof parsed);
  CHECK(hypertabular_range_parse((const uint8_t *)text, sizeof(text) - 1, &parsed) ==
        HYPERTABULAR_OK);
  CHECK(parsed.first_row == 2 && parsed.first_column == 2 && parsed.last_row == 30 &&
        parsed.last_column == 3);
  CHECK(hypertabular_range_parse((const uint8_t *)"b2:c30", 6, &parsed) ==
        HYPERTABULAR_ERR_CONTRACT);
  return 0;
}

static int workbook(const char *corpus) {
  char path[4096];
  FILE *file;
  uint8_t *bytes = NULL;
  long size;
  uint64_t *block;
  HyperTabularWorkbookState *state;
  HyperTabularBuffers buffers;
  HyperTabularOpened opened;
  HyperTabularFilled out;
  HyperTabularSpan *table;
  uint8_t *strings, *kinds;
  uintptr_t strings_len, table_len, kinds_len, part_len, row;
  HyperTabularSheetInfo info;
  char part[64];
  int32_t code;
  int grown;
  uintptr_t i;

  CHECK(strlen(corpus) + 32 < sizeof path);
  sprintf(path, "%s/workbook/basic.xlsx", corpus);
  file = fopen(path, "rb");
  CHECK(file != NULL);
  CHECK(fseek(file, 0, SEEK_END) == 0 && (size = ftell(file)) > 0 && fseek(file, 0, SEEK_SET) == 0);
  bytes = (uint8_t *)malloc((size_t)size);
  CHECK(bytes != NULL && fread(bytes, 1, (size_t)size, file) == (size_t)size);
  fclose(file);

  // The state block is the caller's: 8-byte aligned, and opaque.
  block = (uint64_t *)calloc((hypertabular_workbook_state_size() + 7) / 8, 8);
  CHECK(block != NULL);
  state = (HyperTabularWorkbookState *)block;

  // Each buffer one element long, so every call grows them on the way.
  memset(&buffers, 0, sizeof buffers);
  buffers.window = (uint8_t *)calloc(1, 1);
  buffers.window_cap = 1;
  buffers.arena = (uint8_t *)calloc(1, 1);
  buffers.arena_cap = 1;
  buffers.cells = (HyperTabularSpan *)calloc(1, sizeof(HyperTabularSpan));
  buffers.cells_cap = 1;
  CHECK(buffers.window && buffers.arena && buffers.cells);

  do {
    memset(&opened, 0, sizeof opened);
    code = hypertabular_workbook_open(state, bytes, (uintptr_t)size, &buffers, &opened);
  } while (settle(code, &buffers, opened.needed, &grown) == 1);
  CHECK(code == HYPERTABULAR_OK);
  CHECK(opened.format == 1 && opened.epoch == 1); // XLSX, the 1900 date system

  // The sheets: three spans each, read through HyperTabularSheetInfo.
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_sheets(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  CHECK(code == HYPERTABULAR_OK && out.rows >= 1);
  memcpy(&info, buffers.cells, sizeof info);
  CHECK(is(buffers.arena, info.name, "Data") && (info.flags & 1) == 0);
  CHECK(LEN(info.part) < sizeof part);
  memcpy(part, buffers.arena + info.part.offset, LEN(info.part));
  part_len = LEN(info.part);

  // The shared strings, then the cell formats: the caller keeps both for every sheet.
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_strings(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  CHECK(code == HYPERTABULAR_OK);
  strings_len = (uintptr_t)out.arena_used;
  table_len = (uintptr_t)out.rows;
  strings = (uint8_t *)malloc(strings_len + 1);
  table = (HyperTabularSpan *)malloc((table_len + 1) * sizeof(HyperTabularSpan));
  CHECK(strings != NULL && table != NULL);
  memcpy(strings, buffers.arena, strings_len);
  memcpy(table, buffers.cells, table_len * sizeof(HyperTabularSpan));

  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_styles(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  CHECK(code == HYPERTABULAR_OK);
  kinds_len = (uintptr_t)out.rows;
  kinds = (uint8_t *)malloc(kinds_len + 1);
  CHECK(kinds != NULL);
  memcpy(kinds, buffers.arena, kinds_len);

  buffers.strings = strings;
  buffers.strings_len = strings_len;
  buffers.table = table;
  buffers.table_len = table_len;
  buffers.kinds = kinds;
  buffers.kinds_len = kinds_len;
  buffers.row = (HyperTabularSlot *)calloc(16, sizeof(HyperTabularSlot));
  buffers.row_cap = 16;
  CHECK(buffers.row != NULL);

  // Sheet 0, a header and empty rows skipped.
  memset(&out, 0, sizeof out);
  CHECK(hypertabular_workbook_sheet(state, bytes, (uintptr_t)size, (const uint8_t *)part, part_len,
                                    info.index, 1, 1, &out) == HYPERTABULAR_OK);
  do {
    memset(&out, 0, sizeof out);
    code = hypertabular_workbook_header(state, bytes, (uintptr_t)size, &buffers, &out);
  } while (settle(code, &buffers, out.needed, &grown) == 1);
  CHECK(code == HYPERTABULAR_OK && out.rows == 9);
  {
    static const char *const expected[9] = {"id",   "name",  "amount", "when", "flag",
                                            "note", "ratio", "day",    "at"};
    for (i = 0; i < 9; ++i) {
      const HyperTabularSpan name = buffers.cells[i];
      CHECK(is(FLAGGED(name) ? buffers.arena : strings, name, expected[i]));
    }
  }

  {
    // id as i64, name as text, every row in one batch.
    HyperTabularColumnSpec specs[2];
    int64_t ids[8];
    HyperTabularSpan names[8];
    HyperTabularCellVerdict verdicts[2][8];
    HyperTabularColumnBuffer columns[2];
    static const int64_t want_ids[4] = {1, 2, 3, 6};
    static const uint32_t want_lines[4] = {2, 3, 4, 6};
    const uintptr_t per_row = 3; // a cell per plan column, and the row's number

    memset(specs, 0, sizeof specs);
    specs[0].door = 5;  // i64
    specs[1].door = 18; // text
    specs[1].ordinal = 1;
    for (i = 0; i < 2; ++i) {
      specs[i].format.size = HyperTabularRawNumFormat_MIN_SIZE; // the invariant notation
      specs[i].format.format.flags = HyperTabularNumFormat_ALL;
      specs[i].format.format.decimal_sep.bytes[0] = '.';
      specs[i].format.format.decimal_sep.len = 1;
      specs[i].format.format.group_sep.bytes[0] = ',';
      specs[i].format.format.group_sep.len = 1;
      columns[i].verdicts = verdicts[i];
    }
    columns[0].values = ids;
    columns[1].values = names;
    do {
      memset(&out, 0, sizeof out);
      code = hypertabular_workbook_fill(state, bytes, (uintptr_t)size, specs, columns, 2, 8,
                                        &buffers, &out);
    } while (settle(code, &buffers, out.needed, &grown) == 1);
    CHECK(code == HYPERTABULAR_OK && out.rows == 4);
    for (row = 0; row < 4; ++row) {
      CHECK(buffers.cells[row * per_row + per_row - 1].offset == want_lines[row]);
      CHECK(verdicts[0][row].reason == 0 && ids[row] == want_ids[row]);
    }
    CHECK(is(FLAGGED(names[0]) ? buffers.arena : strings, names[0], "alice"));
    CHECK(is(FLAGGED(names[1]) ? buffers.arena : strings, names[1], "bob, jr"));
    CHECK(verdicts[1][2].reason == 1 && verdicts[1][3].reason == 1);

    do {
      memset(&out, 0, sizeof out);
      code = hypertabular_workbook_fill(state, bytes, (uintptr_t)size, specs, columns, 2, 8,
                                        &buffers, &out);
    } while (settle(code, &buffers, out.needed, &grown) == 1);
    CHECK(code == HYPERTABULAR_OK && out.rows == 0);
  }

  {
    // Through the size-tagged calls: only cell B3, with its text a token.
    static const uint8_t tokens[] = {7, 0, 0, 0, 'b', 'o', 'b', ',', ' ', 'j', 'r'};
    HyperTabularRawSheetOptions options;
    HyperTabularColumnSpecWith spec;
    HyperTabularSpan names[4];
    HyperTabularCellVerdict verdicts[4];
    HyperTabularCellNote notes[4];
    HyperTabularColumnBufferWith column;
    uint32_t widths[4];

    memset(&options, 0, sizeof options);
    options.size = HyperTabularRawSheetOptions_SIZE;
    options.first_row = 3;
    options.first_column = 2;
    options.last_row = 3;
    options.last_column = 2;
    memset(&out, 0, sizeof out);
    CHECK(hypertabular_workbook_sheet_with(state, bytes, (uintptr_t)size, (const uint8_t *)part,
                                           part_len, info.index, &options,
                                           &out) == HYPERTABULAR_OK);

    memset(&spec, 0, sizeof spec);
    spec.size = HyperTabularColumnSpecWith_SIZE;
    spec.door = 18; // text
    spec.format = invariant();
    memset(&column, 0, sizeof column);
    column.size = HyperTabularColumnBufferWith_SIZE;
    column.values = names;
    column.verdicts = verdicts;
    column.notes = notes;
    do {
      HyperTabularBuffersWith with;
      memset(&with, 0, sizeof with);
      with.size = HyperTabularBuffersWith_SIZE;
      with.window = buffers.window;
      with.window_cap = buffers.window_cap;
      with.arena = buffers.arena;
      with.arena_cap = buffers.arena_cap;
      with.cells = buffers.cells;
      with.cells_cap = buffers.cells_cap;
      with.row = buffers.row;
      with.row_cap = buffers.row_cap;
      with.strings = buffers.strings;
      with.strings_len = buffers.strings_len;
      with.table = buffers.table;
      with.table_len = buffers.table_len;
      with.kinds = buffers.kinds;
      with.kinds_len = buffers.kinds_len;
      with.tokens = tokens;
      with.tokens_len = sizeof tokens;
      with.row_widths = widths;
      memset(&out, 0, sizeof out);
      code = hypertabular_workbook_fill_with(state, bytes, (uintptr_t)size, &spec, &column, 1, 4,
                                             &with, &out);
    } while (settle(code, &buffers, out.needed, &grown) == 1);
    CHECK(code == HYPERTABULAR_OK && out.rows == 1);
    CHECK(buffers.cells[1].offset == 3); // the sheet's own row, wherever the range starts
    CHECK(widths[0] == 1);
    CHECK(verdicts[0].reason == 1 && notes[0].origin == HyperTabularCellNote_ORIGIN_TOKEN);
  }

  free(buffers.window);
  free(buffers.arena);
  free(buffers.cells);
  free(buffers.row);
  free(strings);
  free(table);
  free(kinds);
  free(block);
  free(bytes);
  return 0;
}

int main(int argc, char **argv) {
  uint32_t version;

  // Every layout is the size the library was built with.
  CHECK(sizeof(HyperTabularSpan) == HyperTabularSpan_SIZE);
  CHECK(sizeof(HyperTabularCellVerdict) == HyperTabularCellVerdict_SIZE);
  CHECK(sizeof(HyperTabularColumnSpec) == HyperTabularColumnSpec_SIZE);
  CHECK(sizeof(HyperTabularRawNumFormat) == HyperTabularRawNumFormat_SIZE);
  CHECK(sizeof(HyperTabularFailure) == HyperTabularFailure_SIZE);
  CHECK(sizeof(HyperTabularFilled) == HyperTabularFilled_SIZE);
  CHECK(sizeof(HyperTabularSlot) == HyperTabularSlot_SIZE);
  CHECK(sizeof(HyperTabularOpened) == HyperTabularOpened_SIZE);
  CHECK(sizeof(HyperTabularSheetInfo) == HyperTabularSheetInfo_SIZE);
  CHECK(sizeof(HyperTabularRawDialect) == HyperTabularRawDialect_SIZE);
  CHECK(sizeof(HyperTabularDelimitedState) == HyperTabularDelimitedState_SIZE);
  CHECK(sizeof(HyperTabularBuffers) == 14 * sizeof(void *));
  CHECK(sizeof(HyperTabularRawDelimitedOptions) == HyperTabularRawDelimitedOptions_SIZE);
  CHECK(sizeof(HyperTabularRawTranscoded) == HyperTabularRawTranscoded_SIZE);
  CHECK(sizeof(HyperTabularRawDateOptions) == HyperTabularRawDateOptions_SIZE);
  CHECK(sizeof(HyperTabularCellNote) == HyperTabularCellNote_SIZE);
  CHECK(sizeof(HyperTabularRawProblem) == HyperTabularRawProblem_SIZE);
  CHECK(sizeof(HyperTabularRawSheetOptions) == HyperTabularRawSheetOptions_SIZE);
  CHECK(sizeof(HyperTabularRawSuggestion) == HyperTabularRawSuggestion_SIZE);
  CHECK(sizeof(HyperTabularRawRange) == HyperTabularRawRange_SIZE);
  CHECK(sizeof(HyperTabularRawBatchCheck) == HyperTabularRawBatchCheck_SIZE);
  CHECK(sizeof(HyperTabularRawConstraints) == HyperTabularRawConstraints_SIZE);
  CHECK(sizeof(HyperTabularRawCheck) == HyperTabularRawCheck_SIZE);
  CHECK(sizeof(HyperTabularColumnSpecWith) == HyperTabularColumnSpecWith_SIZE);
  CHECK(sizeof(HyperTabularColumnBufferWith) == HyperTabularColumnBufferWith_SIZE);

  if (argc != 2) {
    fprintf(stderr, "usage: %s <corpus directory>\n", argv[0]);
    return 2;
  }
  if (delimited() != 0 || delimited_with() != 0 || constrained() != 0 || grouping() != 0 ||
      transcode() != 0 || suggest() != 0 || batch_check() != 0 || range() != 0 ||
      workbook(argv[1]) != 0 || workbook_grouping(argv[1]) != 0) {
    return 1;
  }

  version = hypertabular_version();
  printf("%u.%u.%u\n", (unsigned)(version >> 16), (unsigned)((version >> 8) & 0xFF),
         (unsigned)(version & 0xFF));
  return 0;
}
