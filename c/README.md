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
find_package(hypertabular 1.0 REQUIRED)
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

## C++

`hypertabular.hpp` is header-only and needs C++17. It includes HyperCast's `hypercast.hpp`
(1.1.0 or later): a cell is a `hypercast::Verdict<T>`, its values are HyperCast's `Decimal`,
`Uuid`, `Timestamp`, `Date`, `CivilDateTime` and `Duration`, and a column's notation is a
`hypercast::NumFormat`, so a program that also casts through HyperCast's doors directly has
one set of types. So a C++ program finds both archives, and links HyperCast's library too
when it calls HyperCast's doors itself or `NumFormat::declare`, which asks HyperCast's core:

```cmake
find_package(hypertabular 1.0 REQUIRED)
find_package(hypercast 1.1 REQUIRED)
target_link_libraries(app PRIVATE hypertabular::hypertabular_static hypercast::hypercast_static)
target_compile_features(app PRIVATE cxx_std_17)
```

or, with both `lib/pkgconfig` directories on `PKG_CONFIG_PATH`,
`c++ -std=c++17 app.cpp $(pkg-config --cflags --libs hypertabular hypercast)`.

Every function is `noexcept`, and it builds with exceptions off (`-fno-exceptions`, or
`/EHs-c-` with `_HAS_EXCEPTIONS=0`). Unlike HyperCast's doors it owns memory, as every
HyperTabular binding does: its buffers are standard containers, and running out of memory ends
the program. CI builds it with GCC on Linux, Apple Clang on macOS, iOS and Mac Catalyst, and
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
  if (!reader.bind({ht::Column::i32(*header.ordinal("id")), ht::Column::text(1)}).ok()) {
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
`separator`, `plan`, `no_sheet`, `no_column`, `unbound`, `already_bound`, `structure`), what
it is about (`failure()`, with its `FailureKind`, record, line, byte, expected and found;
`column()`; `separator()`; `detail()`), and `message()`, worded as every binding words it.

`DelimitedReader` opens over bytes in memory (`from_bytes`, read in place), a file (`open`)
or a source function (`from_source`), with a `Dialect` (`csv()`, `tsv()`, `psv()`, or any
separator, quoting, header and blank-line choice) and optionally a `DelimitedOptions` (rows a
batch, starting buffer). `Workbook` opens bytes in place (`from_bytes`), bytes it keeps
(`from_vector`) or a file (`open`), lists its `sheets()`, and opens a `Sheet` by index or name
with `SheetOptions`. Either is given its plan, a list of `Column`s (`Column::i64(ordinal)`,
`Column::decimal(ordinal).format(numformat)`, `Column::date_ordered(ordinal, DateOrder)`, one
per door), when it opens or with `bind` after its header is read. `read()` hands back a
`Result<std::optional<Batch>>`, the batch empty at the end; a `Batch` is a view of the reader's
buffers, valid until the next `read()`, with `get<T>(column, row)`, `text(column, row)` (a
`Verdict<std::string_view>`), `fault`, `raw` (the cell's text, whatever its verdict), `line`
and its `Row`s. `for_each_row` does the loop. Reading a column as a type its door does not
make, or an index outside the batch, is a caller bug that ends the program with
`std::abort()`. `native_version()` is the linked library's version, and `is_available()` is
always true: the core is linked in.
