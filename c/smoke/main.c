// A C program built against an unpacked release archive, the way a consumer would: through
// CMake's find_package or pkg-config, linked to the shared or the static library. It calls all
// 14 functions hypertabular.h declares: it reads a CSV through the delimited ones, buffers
// grown when the core asks, and a sheet of corpus/workbook/basic.xlsx through the workbook
// ones, and checks the header, every cell's value or verdict and every row's line. Then it
// prints the library's version, which run.sh holds to the package's. CMakeLists.txt builds it
// as C99, C11 and, where the compiler has it, C23, warnings as errors.
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

  if (argc != 2) {
    fprintf(stderr, "usage: %s <corpus directory>\n", argv[0]);
    return 2;
  }
  if (delimited() != 0 || workbook(argv[1]) != 0) {
    return 1;
  }

  version = hypertabular_version();
  printf("%u.%u.%u\n", (unsigned)(version >> 16), (unsigned)((version >> 8) & 0xFF),
         (unsigned)(version & 0xFF));
  return 0;
}
