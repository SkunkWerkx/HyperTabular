# hypertabular for C and C++

[![CI](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml/badge.svg)](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml)
[![C/C++](https://img.shields.io/github/v/release/SkunkWerkx/HyperTabular?label=c%2Fc%2B%2B&sort=semver)](https://github.com/SkunkWerkx/HyperTabular/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)

Forward-only reads of delimited text (CSV, TSV, PSV) and of XLSX, ODS and Numbers workbooks,
into column buffers the caller owns, a batch at a time. Every cell is cast through the column's
declared HyperCast door and comes back as its value or a fault, a closed reason (empty,
malformed, out of range) with the byte span of the cell's text that caused it. A structurally
broken input is a failure that says what and where, delivered only after every intact row
before it. It is the same core every HyperTabular binding uses, here through the C ABI in
`include/hypertabular.h`. C++17 and later have their own header over it,
`include/hypertabular.hpp`, linked against the same library, which stands on HyperCast's C++
header for the verdicts and values.

## Install

Each GitHub Release carries one archive per platform, `hypertabular-<version>-<rid>.tar.gz`
(`.zip` on Windows), for linux-x64, linux-arm64, linux-musl-x64, linux-musl-arm64, osx-x64,
osx-arm64, win-x64, win-arm64, android-arm64, android-x64, ios-arm64, iossimulator-arm64,
maccatalyst-arm64, maccatalyst-x64 and wasi-wasm32. Verify one before use; the release
attestation lists every file the release shipped:

```sh
gh release download v<version> --repo SkunkWerkx/HyperTabular --pattern 'hypertabular-*-linux-x64.tar.gz'
gh attestation verify hypertabular-<version>-linux-x64.tar.gz --repo SkunkWerkx/HyperTabular
```

Unpack it anywhere:

```
include/hypertabular.h, include/hypertabular.hpp
lib/libhypertabular.a                    static (Windows: lib/hypertabular.lib)
lib/libhypertabular.so | lib/libhypertabular.dylib
                                         shared (Windows: bin/hypertabular.dll and its
                                         import library lib/hypertabular.dll.lib)
lib/pkgconfig/hypertabular.pc
lib/cmake/hypertabular/
LICENSE, README.md                       this file
```

The iOS, iOS simulator and Mac Catalyst archives have the static library only, linked into
the app, and so does wasi-wasm32, for wasi-sdk.

C needs this archive alone: `hypertabular.h` carries every layout it uses, HyperCast's value
types among them, at the layout this library was built with. C++ needs HyperCast's archive
for the same platform beside it (HyperCast 1.1.0 or later), for `hypercast.hpp`; see below.

## Build against it

CMake, with the unpacked directory on `CMAKE_PREFIX_PATH`:

```cmake
find_package(hypertabular 1.1 REQUIRED)
target_link_libraries(app PRIVATE hypertabular::hypertabular)          # shared
target_link_libraries(app PRIVATE hypertabular::hypertabular_static)   # or static
```

Or let CMake fetch the archive at configure time. Name your platform's archive and its
SHA-256, which the release's `inventory.json` lists for every file:

```cmake
include(FetchContent)
FetchContent_Declare(hypertabular
  URL https://github.com/SkunkWerkx/HyperTabular/releases/download/v<version>/hypertabular-<version>-linux-x64.tar.gz
  URL_HASH SHA256=<sha256>)
FetchContent_MakeAvailable(hypertabular)
find_package(hypertabular CONFIG REQUIRED PATHS "${hypertabular_SOURCE_DIR}" NO_DEFAULT_PATH NO_CMAKE_FIND_ROOT_PATH)
target_link_libraries(app PRIVATE hypertabular::hypertabular_static)
```

The archive holds no `CMakeLists.txt`, so `FetchContent_MakeAvailable` only downloads,
checks and unpacks it; `find_package` then reads its CMake files from where it landed.
`NO_CMAKE_FIND_ROOT_PATH` keeps a cross build (iOS, Android) from looking for them inside its
SDK instead. Without FetchContent, a cross build names the archive with
`-Dhypertabular_DIR=<dir>/lib/cmake/hypertabular`, for the same reason.

pkg-config, with `<dir>/lib/pkgconfig` on `PKG_CONFIG_PATH`:

```sh
cc app.c $(pkg-config --cflags --libs hypertabular)
```

The static library needs nothing past the C runtime. To link it with pkg-config, name it
directly: `cc app.c $(pkg-config --cflags hypertabular) <dir>/lib/libhypertabular.a`.

The shared library is found at run time by its own name: `@rpath/libhypertabular.dylib` on
macOS and the soname `libhypertabular.so` on Linux and Android, so a program needs an rpath
(`-Wl,-rpath,<dir>/lib`) or the library on the loader's path. On Windows the DLL goes beside
the program.

## C

The C ABI is the one every binding drives: the core owns no memory and keeps nothing between
calls but the caller's state block, so a binding (or a C program) supplies the input, the
column buffers and the scratch buffers, and grows a buffer when the core says how much it
needs. A minimal read:

```c
#include <stdio.h>
#include <string.h>
#include "hypertabular.h"

int main(void) {
  const char csv[] = "id,name\n1,alice\n2,x\n";
  const uint8_t *input = (const uint8_t *)csv;
  uintptr_t at = 0, len = sizeof csv - 1;
  uint64_t block[HyperTabularDelimitedState_SIZE / 8]; /* 8-byte aligned */
  HyperTabularDelimitedState *state = (HyperTabularDelimitedState *)block;
  HyperTabularRawDialect dialect = {',', 1, 1, 0}; /* separator, quoting, skip blank lines */
  HyperTabularSpan names[8], cells[64];
  uint8_t arena[256];
  HyperTabularFilled out;

  if (hypertabular_delimited_init(state, &dialect) != HYPERTABULAR_OK ||
      hypertabular_delimited_header(state, input, len, 1, names, 8, arena, sizeof arena, &out) !=
          HYPERTABULAR_OK) {
    return 1;
  }
  at += out.consumed;

  /* One column: the first, through the i32 door (4), in the invariant notation. */
  HyperTabularColumnSpec spec;
  memset(&spec, 0, sizeof spec);
  spec.door = 4;
  spec.format.size = HyperTabularRawNumFormat_MIN_SIZE;
  spec.format.format.flags = HyperTabularNumFormat_ALL;
  spec.format.format.decimal_sep.bytes[0] = '.';
  spec.format.format.decimal_sep.len = 1;
  spec.format.format.group_sep.bytes[0] = ',';
  spec.format.format.group_sep.len = 1;
  int32_t ids[16];
  HyperTabularCellVerdict verdicts[16];
  HyperTabularColumnBuffer column = {ids, verdicts};
  HyperTabularBuffers buffers;
  memset(&buffers, 0, sizeof buffers);
  buffers.arena = arena;
  buffers.arena_cap = sizeof arena;
  buffers.cells = cells;
  buffers.cells_cap = 64;

  if (hypertabular_delimited_fill(state, input + at, len - at, 1, &spec, &column, 1, 16, &buffers,
                                  &out) != HYPERTABULAR_OK) {
    return 1; /* HYPERTABULAR_ERR_STRUCTURE: out.failure says what and where */
  }
  for (uint64_t row = 0; row < out.rows; ++row) {
    if (verdicts[row].reason == 0) {
      printf("id %d\n", ids[row]);
    } else { /* 1 empty, 2 malformed, 3 out of range, with the span of the cell's text */
      printf("fault %u at byte %u, %u long\n", verdicts[row].reason, verdicts[row].offset,
             verdicts[row].len);
    }
  }
  return 0;
}
```

Every call returns `HYPERTABULAR_OK` or a `HYPERTABULAR_ERR_` code: `ERR_CONTRACT` for a
caller bug (a null where memory is needed, an undefined door or a numeric format the core
refuses), `ERR_STRUCTURE` for broken input with `out->failure` filled in, and `ERR_ARENA`,
`ERR_CELLS` or `ERR_WINDOW` for a buffer too small, with `out->needed` the size it needs. A
delimited call that stops for room consumed nothing and is simply made again; a workbook call
stops in place, and the caller grows that buffer keeping its contents (a `realloc`) and repeats
the call. No return code is a cell's verdict: that is the `HyperTabularCellVerdict` beside each
value. The header documents every function, type, door code and layout, and each layout's size
is a `_SIZE` constant to check against. `hypertabular_version()` is the one way to read the
version at run time. A minor release may add functions; only a major release changes or
removes one.

The calls above are the 1.0 ones, with the layouts they always had. What they answer follows
HyperCast's doors, and a few answers have moved since 1.0: `1,23,456`, `12,34` and `0,125` are malformed
where 1.0 read 123456, 1234 and 125 (`CHANGELOG.md`). They read the low 8 bits of a format's flags, so
they read every format as Western, and neither refuses a stale policy nor reads a declared one. A messier
file goes through the size-tagged ones, each struct naming its own size first. A delimited state is
`HyperTabularDelimitedState_SIZE` bytes, 80, what `hypertabular_delimited_state_size()` reports and
`_init_with` needs; 1.0 said 64, and a block only `hypertabular_delimited_init` writes may still be 64
bytes, so a 1.0 program that requires the reported size to equal its 64 now fails that check. The
new calls:

- `hypertabular_batch_check` says how much a batch may ask for, ahead of allocating it: it takes the
  plan's specs, `batch_rows`, `buffer_bytes` and flags (`HyperTabularRawBatchCheck_NOTES`, `_WIDTHS`,
  `_SHEET`), and fills a `HyperTabularRawBatchCheck` with the bytes needed, the limit (4 GiB), the size to
  start the input buffer at and, with `HYPERTABULAR_ERR_BATCH`, the reason (`_REFUSED_...`). `width` is the
  row's columns, the furthest it reaches or, on a sheet with more plan columns than that, the plan's; `sample_rows` is the most rows up to `batch_rows` that fit a sampler's budget (64 MiB, at
  least one row, and 0 when one row alone is past the limit, `_REFUSED_ROW_TOO_WIDE`). It allocates
  nothing, and any size can be asked about.
- `hypertabular_delimited_init_with` takes a `HyperTabularRawDelimitedOptions`: lines to pass over
  (`skip`), records to discard (`header_row`), and what a short or a long row does.
- `hypertabular_delimited_fill_with` and `hypertabular_workbook_fill_with` take
  `HyperTabularColumnSpecWith` (a column's tokens, date options and month names, and the
  constraints it declares: HyperCast's 104-byte `HyperTabularRawConstraints` and its packed `enum`
  list), `HyperTabularColumnBufferWith` (with a `HyperTabularCellNote` per row, saying what failed and
  where a verdict came from, and for a column that declares constraints a `HyperTabularRawCheck` per
  row) and `HyperTabularBuffersWith` (tokens for every column, and each row's width). A cell that cast
  and broke a declared constraint has the verdict `HyperTabularCellVerdict_VIOLATION`, its value
  written as cast and its check naming the keyword (`1` minimum … `8` enum); a cell that passed has a
  check saying whether rounding adjusted it and which `enum` entry a text matched. They judge a numeric format's flags in full, a column at a time. Flags `95`
  (`HyperTabularNumFormat_ALL`, Western) read `1,234,567` and refuse `1,23,456`, flags `607` (with
  `HyperTabularNumFormat_GROUPING_INDIAN`) do the reverse, and an all-zero format is Western. Flags holding
  the retired grouping bit `256`, as `351` and `863` do, are `HYPERTABULAR_ERR_CONTRACT` before any cell,
  size or no size. `hypertabular_suggest` refuses them too.
- `hypertabular_workbook_sheet_with` takes a `HyperTabularRawSheetOptions`, with the rows to pass
  over and discard and a range, which `hypertabular_range_parse` reads from `A5:F200`.
- `hypertabular_faults` lists what went wrong in the rows a fill just wrote, from a cursor, a
  violation with its `keyword`.
- `hypertabular_transcode` turns Windows-1252, Latin-1 or UTF-16LE into the UTF-8 the delimited
  calls read, on the caller's buffers.
- `hypertabular_suggest` says what a sample's columns look like. Nothing reads its answer but the
  caller, who passes a byte a cell for a sheet's typed cells: a `Uuid` door beside the `Text` door, with
  notes, says which, as `Malformed` with a detail of `0`. Delimited text passes null.

`c/smoke/main.c` calls each of them.

## C++

`hypertabular.hpp` is header-only and needs C++17. It includes HyperCast's `hypercast.hpp`
(1.1.0 or later): a cell is a `hypercast::Verdict<T>`, its values are HyperCast's `Decimal`,
`Uuid`, `Timestamp`, `Date`, `CivilDateTime` and `Duration`, and a column's notation is a
`hypercast::NumFormat`, so a program that also casts through HyperCast's doors directly has
one set of types. So a C++ program finds both archives, and links HyperCast's library too
when it calls HyperCast's doors itself, `NumFormat::declare`, `DateOptions::declare`,
`MonthNames::declare` or `Constraints<T>::declare`, which ask HyperCast's core, or `suggest`, which
declares a comma-decimal candidate that way:

```cmake
find_package(hypertabular 1.1 REQUIRED)
find_package(hypercast 1.1 REQUIRED)
target_link_libraries(app PRIVATE hypertabular::hypertabular_static hypercast::hypercast_static)
target_compile_features(app PRIVATE cxx_std_17)
```

or, with both `lib/pkgconfig` directories on `PKG_CONFIG_PATH`,
`c++ -std=c++17 app.cpp $(pkg-config --cflags --libs hypertabular hypercast)`.

Every function is `noexcept`, and it builds with exceptions off (`-fno-exceptions`, or
`/EHs-c-` with `_HAS_EXCEPTIONS=0`). Unlike HyperCast's doors it owns memory, as every
HyperTabular binding does: its buffers are standard containers, and running out of memory ends
the program, and a source function or a `for_each_row` callback that throws ends it too, by
`std::terminate`: no `catch` of the caller's is reached. A `Sheet` holds its `Workbook`'s address, so
the workbook outlives it and isn't moved while it's in use.

A source function runs in the middle of its reader's `read()`, and a `for_each_row` callback in the
middle of its reader's or sheet's walk. A `read()` or `bind()` from there on that same reader or
sheet is `ErrorKind::busy` (`the reader is in the middle of a read; nothing that read calls can call
back into it`, `detail()` saying `reader` or `sheet`), and the read it was called from goes on
unharmed. Destroying or moving the reader or sheet from there ends the program with
`std::abort()`, since nothing is left to answer. Other readers and sheets are there to use. CI builds it with GCC on Linux, Apple Clang on macOS, iOS and Mac Catalyst, and
MSVC on Windows, as C++17, C++20 and C++23, with exceptions on and off.

```cpp
#include <cstdio>
#include "hypertabular.hpp"

namespace ht = hypertabular;

int main() {
  auto opened = ht::DelimitedReader::from_bytes("id,name\n1,alice\n2,x\n3\n", ht::Dialect::csv());
  if (!opened.ok()) {
    std::printf("%s\n", opened.error().message().c_str());
    return 1;
  }
  ht::DelimitedReader &reader = opened.value();
  const ht::Header &header = *reader.header();
  const auto id = header.require("id"); // refuses a name two columns share
  if (!id.ok() || !reader.bind({ht::Column::i32(id.value()), ht::Column::text(1)}).ok()) {
    return 1;
  }
  const auto read = reader.for_each_row([](const ht::Row &row) noexcept {
    const hypercast::Verdict<std::int32_t> id = row.get<std::int32_t>(0);
    if (const std::int32_t *value = id.value_if()) {
      std::printf("line %u: id %d\n", row.line(), *value);
    }
  });
  if (const ht::Error *error = read.error_if()) {
    std::printf("%s\n", error->message().c_str()); // record 3 (line 4, byte 20) has 1 cells
  }                                                 // where the first has 2

  auto book = ht::Workbook::open("people.xlsx");
  if (const auto *workbook = book.value_if()) {
    auto sheet = workbook->sheet(0, ht::SheetOptions{}, {ht::Column::text(0)});
    // sheet.value().read(), or for_each_row, as above
  }
  return 0;
}
```

A structural error is a `hypertabular::Result<T>`, shaped as a `Verdict` is: `ok()`,
`value_if()` and `error_if()` (a pointer to the case, or null), `value()` and `error()`
(checked: the wrong one ends the program with `std::abort()`, since a reader has no value to
make up), `match(on_value, on_error)`, and under C++23 with `<expected>` a conversion to
`std::expected<T, hypertabular::Error>`. A `hypertabular::Error` has a `kind()` (`io`,
`separator`, `plan`, `no_sheet`, `no_column`, `ambiguous_column`, `range`, `unbound`,
`already_bound`, `structure`), what it is about (`failure()`, with its `FailureKind`, record,
line, byte, expected and found; `column()`; `separator()`; `detail()`; `first()` and `second()`,
the two columns a name is shared by), and `message()`, worded as every binding words it.

A `Header` looks a name up two ways. `require` is strict: `no_column` for a name it hasn't, and
`ambiguous_column`, naming both positions, for one two columns share, because a plan built from
names mustn't silently read the wrong column. `ordinal` is optional, a `std::optional`, and keeps
the first match. A plan by position reads either column.

`DelimitedReader` opens over bytes in memory (`from_bytes`, read in place), a file (`open`)
or a source function (`from_source`), with a `Dialect` (`csv()`, `tsv()`, `psv()`, or any
separator, quoting, header and blank-line choice, and `with_skip`, `with_header_row`,
`with_short_rows`, `with_long_rows`, `with_encoding` and `with_tokens`) and optionally a
`DelimitedOptions` (rows a batch, starting buffer, `with_notes`; a 0 for either size, or a batch past
the core's 4 GiB limit, is `ErrorKind::zero_batch_rows`, `zero_buffer_bytes` or `batch_too_large`, which
says `needed()` and `limit()`, before anything is allocated; a `std::vector` writes what it's asked for, so a
batch near the limit holds that memory from the moment it's bound; `suggest` is never refused for the size
of its own batches, and a row that alone is past the limit is `ErrorKind::row_too_wide`, which says `width()`). `Workbook` opens bytes in place
(`from_bytes`), bytes it keeps (`from_vector`) or a file (`open`), lists its `sheets()`, and opens a
`Sheet` by index or name with `SheetOptions` (`with_skip`, `with_header_row`, `with_range`,
`with_tokens`, `with_notes` and the rest). A `Range` comes from `Range::parse("A5:F200")`, or
`Range::of(first_row, first_column, last_row, last_column)` with 0 for no limit on a side. Either is
given its plan, a list of `Column`s (`Column::i64(ordinal)`,
`Column::decimal(ordinal).format(numformat)`, `Column::date_ordered(ordinal, DateOrder)`, one
per door, each with `.tokens(...)` and `.date_options(options, months)` if it needs them), when it
opens or with `bind` after its header is read. A `Column`, a `Dialect` and `SheetOptions` can own
lists, so they aren't `constexpr`. `read()` hands back a `Result<std::optional<Batch>>`, the batch
empty at the end; a `Batch` is a view of the reader's buffers, valid until the next `read()` and no longer than the
reader or sheet that read it, with
`get<T>(column, row)`, `text(column, row)` (a `Verdict<std::string_view>`), `get_checked<T>(column,
row)` (a `hypercast::Checked<T>`, see [Constraints](#constraints)), `fault`, `raw` (the
cell's text, whatever its verdict), `line`, `detail` (HyperCast's name for what failed), `origin`
(`Origin::cell`, `token`, `padded` or `uncached`), `faults()` and its `Row`s. `for_each_row` does the
loop. Reading a column as a type its door does not make, an index outside the batch, or walking
the batch's rows or `faults()` past the next `read()` is a caller bug that ends the program with
`std::abort()`.
`native_version()` is the linked library's version,
and `is_available()` is always true: the core is linked in.

```cpp
#include <cstdio>
#include "hypertabular.hpp"

namespace ht = hypertabular;

int main() {
  const ht::Dialect messy = ht::Dialect::csv()
                                .with_skip(1)                         // a title line
                                .with_short_rows(ht::ShortRows::pad)  // a short row's cells are Empty
                                .with_long_rows(ht::LongRows::report) // a long row is cut, and listed
                                .with_tokens({"N/A"});                // stands for no value
  auto reader = ht::DelimitedReader::from_bytes("Sales\nid,qty\n1,N/A\n2,x\n3\n4,5,6\n", messy,
                                                {ht::Column::i32(0), ht::Column::i32(1)});
  if (!reader.ok()) {
    return 1;
  }
  auto read = reader.value().read();
  if (!read.ok() || !read.value()) {
    return 1;
  }
  const ht::Batch &batch = *read.value();
  std::printf("%s, %s\n", ht::name(batch.origin(1, 0)).c_str(), ht::name(batch.origin(1, 2)).c_str());
  for (const ht::Problem &problem : batch.faults()) {
    if (problem.plan_column) {
      std::printf("line %u, column %zu: %s\n", problem.line, *problem.source_column,
                  hypercast::name(problem.detail).c_str());
    } else {
      std::printf("line %u: %u cells where the first has %u\n", problem.line, problem.found,
                  problem.expected);
    }
  }

  // Asking what a file's columns look like. Nothing here reaches a read.
  auto found = ht::DelimitedReader::suggest_bytes("id,day\n1,01/02/2024\n2,03/04/2024\n",
                                                  ht::Dialect::csv());
  if (!found.ok()) {
    return 1;
  }
  for (const ht::SuggestedColumn &column : found.value().report) {
    std::printf("%s: %s, %zu candidates\n", column.name->c_str(),
                std::string(ht::name(column.status)).c_str(), column.candidates.size());
  }
  return 0;
}
```

prints

```
token, padded
line 4, column 1: unexpected_character
line 6: 3 cells where the first has 2
id: resolved, 0 candidates
day: ambiguous, 2 candidates
```

A `batch.faults()` range holds a read's cells at `malformed` or `out_of_range`, formulas with no
cached value, cells that broke a constraint their column declares and rows `LongRows::report`
declared, each a `Problem`: `row`, `line`, `plan_column`, `source_column` (the source's, a sheet's
own whatever its range), `reason` (none for a violation), `keyword` (a violation's), `detail`,
`origin`, `offset` and `length`, or `expected` and `found` for a row. It's a view too: a walk past the next
`read()` ends the program with `std::abort()`, a caller's bug like an index outside the batch, since
stopping there would read as the end of the list. Finish the walk first. With notes off it has no `detail` and no uncached formula, and `detail()` and `origin()`
say nothing.

**To keep every row of a sheet, set `SheetOptions::skip_empty_rows` to `false`.** By default a row
whose only cells are formulas with no cached value is filtered out as empty, with nothing to say so;
off, those rows are delivered, their cells at `Origin::uncached`. In C it's the `skip_empty_rows`
argument of `hypertabular_workbook_sheet` and `hypertabular_workbook_sheet_with`'s options.

A column given no format reads `NumFormat::invariant()`, as a plan with no format does in every binding,
whose grouping is Western: `1,234,567` is 1234567 and `1,23,456` is malformed. If a column of Indian
numbers comes back refused, declare it, one column at a time, so an Indian column reads beside a Western
one in the same read:

```cpp
const auto indian = hypercast::NumFormat::invariant().with_grouping(hypercast::Grouping::indian);
auto reader = hypertabular::DelimitedReader::from_bytes(
    bytes, hypertabular::Dialect::csv(),
    {hypertabular::Column::i32(0).format(indian), // 1,23,456 is 123456, and 1,234,567 is malformed
     hypertabular::Column::i32(1)});              // 1,234,567 is 1234567, and 1,23,456 is malformed
```

A plan can be judged before any input: on a state of its own from `hypertabular_delimited_init_with`,
`hypertabular_delimited_fill_with` with no input, `max_rows` 0 and a buffer for each column writes nothing,
and returns `HYPERTABULAR_ERR_CONTRACT` for a column it refuses, a stale format among them. A workbook's plan
is asked the same way. That is how the C++ header refuses a plan when it's bound. The exports that open a
source take no plan, so in C this is the earliest a format can be refused; otherwise the first fill is.

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat::detect()` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `f64` reads `1,2345` as 1.2345, `12,34`
as 12.34 and `0,125` as 0.125. A format holding the retired
grouping bit `256`, which `NumFormat::declare` and `with_grouping` can't make, is `ErrorKind::plan` when
the plan is bound, before any cell, and in a suggest's options before it opens anything. C++ has no locale bridge, so nothing reads a locale's name.

### Constraints

A column can declare what its cells must also meet, as OpenAPI 3.2 says it: bounds, `multipleOf` with
a rounding, lengths, `enum` and `trim`, through HyperCast's own `Constraints<T>::declare` (its README
has every keyword). `Column::checked(ordinal, constraints)` reads the column through the checked door
for `T`, the declaration's: `std::int8_t` to `std::uint64_t`, `float`, `double`, `Decimal`, or
`std::string_view` for text. It copies the declaration and its `enum` list, so their storage needn't
outlive the call, and `.format(...)`, `.tokens(...)` and `.date_options(...)` are set on it as on any
column. `is_checked()` says whether a column declares any.

`get_checked<T>(column, row)`, and `Row::get_checked<T>(column)`, give HyperCast's `Checked<T>`: a
`CheckedValue` (the value, `adjusted` when rounding to `multipleOf` changed it, and at the text door,
when an `enum` entry matched, its `enum_index` and the entry as declared for the value), or a
`Rejection`, the cell's `Fault` as `get` gives it or a `Violation` naming the `Keyword` broken and
carrying the value as cast and its span. A column that declares none reads through it too, every cell
that cast passing unadjusted. `get` and `text` refuse a constrained column with `std::abort()`, since a
violation is no fault and they would say it passed; `fault` gives `std::nullopt` for one.

```cpp
#include <cstdio>
#include "hypertabular.hpp"

namespace ht = hypertabular;

int main() {
  std::uint8_t entries[32]; // the packed list: 4 bytes and the text per entry
  const auto sexes = ht::Constraints<std::string_view>::declare({}, {"Female", "Male"}, entries);
  ht::Constraints<std::int32_t>::Fields age;
  age.minimum = 0;
  age.maximum = 120;
  const auto ages = ht::Constraints<std::int32_t>::declare(age);
  if (ages.value_if() == nullptr || sexes.value_if() == nullptr) {
    return 1;
  }
  auto reader = ht::DelimitedReader::from_bytes(
      "age,sex\n34,MALE\n130,other\n", ht::Dialect::csv(),
      {ht::Column::checked(0, *ages.value_if()), ht::Column::checked(1, *sexes.value_if())});
  if (!reader.ok()) {
    return 1;
  }
  auto read = reader.value().read();
  if (!read.ok() || !read.value()) {
    return 1;
  }
  const ht::Batch &batch = *read.value();
  for (const ht::Row &row : batch) {
    const ht::Checked<std::int32_t> years = row.get_checked<std::int32_t>(0);
    const ht::Checked<std::string_view> sex = row.get_checked<std::string_view>(1);
    if (years.ok() && sex.ok()) { // MALE matched the entry Male, under simple case folding
      const std::string_view entry = sex.value().value;
      std::printf("line %u: %d, %.*s\n", row.line(), years.value().value,
                  static_cast<int>(entry.size()), entry.data());
    } else if (const ht::Violation<std::int32_t> *broken = years.rejection().violation_if()) {
      std::printf("line %u: %d breaks %s\n", row.line(), broken->value,
                  hypercast::name(broken->keyword).c_str());
    }
  }
  for (const ht::Problem &problem : batch.faults()) {
    if (problem.keyword) {
      std::printf("line %u, column %zu: %s\n", problem.line, *problem.source_column,
                  hypercast::name(*problem.keyword).c_str());
    }
  }
  return 0;
}
```

prints

```
line 2: 34, Male
line 3: 130 breaks maximum
line 3, column 0: maximum
line 3, column 1: enum
```

In C, a `HyperTabularColumnSpecWith`'s `constraints` and `enum_ptr` carry the declaration, and the
column's `HyperTabularColumnBufferWith` needs its `checks`; [C](#c) says how a violation reads there.
HyperCast's [grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape and the locales neither policy matches.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied.
`DelimitedReader::suggest(path, dialect)`, `suggest_bytes(bytes, dialect)` and `Sheet::suggest()`
sample up to `SuggestOptions::sample_rows` rows and return a `Suggestion`: a `plan` to change or to
hand to a read, a `report` saying what each column is and why (its `status`, and `candidates` to
choose between, a `DateOrder` or a `NumFormat`), and whether the source was `truncated` past the
sample. An ambiguous or unresolved column plans as text until the caller declares it. With no format
declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read only with
`,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the plan is in
whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous until a format is
declared or another cell settles them, which takes in a column of prices at three decimals;
`SuggestOptions::with_date_order` does the same for `01/02/2026`. `2,5` beside `2.5` is unresolved, and
`1,23,456`, Indian grouping, is text until the column declares Indian grouping, because nothing says its
culture. A sheet's number
cell has no notation, so a column of them is a number under any format, where text holding the same
digits asks. With nothing to choose between, a resolved number plans in
`NumFormat::invariant()`, or the format `SuggestOptions` declares for it. Some answers can surprise:
`0` and `1` are booleans, and `007` stays text. A sheet's date column suggests text,
since a typed cell reads as its canonical text. A headerless file is opened twice, so a path that
can be read once, as a FIFO's, is not for this.
