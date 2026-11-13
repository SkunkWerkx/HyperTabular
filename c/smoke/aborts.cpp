// What ends the program rather than answer: a caller bug, never data. CMakeLists.txt builds this
// once for each way into it, SMOKE_WAY naming the way:
//
//   faults_begin    a batch's faults walk started over a list the reader has left: stopping there
//                   would read as the end of the list
//   faults_advance  a faults walk under way, gone on past the next read()
//   rows_deref      a batch's row walk, a row asked for past the next read(): the rows are the next
//                   batch's
//   rows_advance    a row walk gone on past the next read(), by prefix ++
//   rows_postfix    the same by postfix ++
//   destroyed       a reader destroyed by its own source, in the middle of a read: its source would
//                   be destroyed under itself
//   moved_from      a reader moved from by its own source, in the middle of a read
//   moved_onto      a reader moved onto by its own source, in the middle of a read
//   sheet_destroyed a sheet destroyed by its own for_each_row callback
//   checked_get     get() of a column that declares constraints: a violation is no fault, so
//                   get() would say it passed (get_checked() reads it)
//   checked_text    text() of a text column that declares constraints, likewise
//
// The version goes out first, as every smoke program's does. Only the handler then exits 0: it's
// reached by the header's std::abort(), and a program that comes back is a failure. Each way sets
// came_back on the statement after the one the header must end at, so an abort that comes later,
// from a crash on freed memory, exits 1 too: with no sanitizer that is the one way to tell them
// apart. It takes a program of its own for each because the check ends the process.
//
// usage: smoke_aborts_<way> <corpus directory>
#include <algorithm>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <optional>
#include <string>
#include <string_view>

#include "hypertabular.hpp"

namespace ht = hypertabular;

#ifndef SMOKE_WAY
#error "SMOKE_WAY names the way in"
#endif

#define WAY_faults_begin 1
#define WAY_faults_advance 2
#define WAY_rows_deref 3
#define WAY_rows_advance 4
#define WAY_destroyed 5
#define WAY_moved_from 6
#define WAY_moved_onto 7
#define WAY_sheet_destroyed 8
#define WAY_rows_postfix 9
#define WAY_checked_get 10
#define WAY_checked_text 11
#define WAY_OF(way) WAY_##way
#define WAY_IS(way) (WAY_OF(way) == WAY_OF_SMOKE)
#define WAY_OF_SMOKE_(way) WAY_OF(way)
#define WAY_OF_SMOKE WAY_OF_SMOKE_(SMOKE_WAY)

// Set past the statement the header must end the program at: the header didn't.
static volatile std::sig_atomic_t came_back = 0;

extern "C" void aborted(int) { std::_Exit(came_back ? 1 : 0); }

static void version_then_expect_abort() {
  const std::string_view version = ht::native_version();
  std::printf("%.*s\n", static_cast<int>(version.size()), version.data());
  std::fflush(stdout);
  std::signal(SIGABRT, aborted);
}

int main(int argc, char **argv) {
  if (argc != 2) {
    return 2;
  }
#if WAY_IS(faults_begin) || WAY_IS(faults_advance) || WAY_IS(rows_deref) ||                        \
    WAY_IS(rows_advance) || WAY_IS(rows_postfix)
  auto opened = ht::DelimitedReader::from_bytes("a\nx\ny\n", ht::Dialect::csv(),
                                                {ht::Column::i32(0)}, ht::DelimitedOptions{1});
  if (!opened.ok()) {
    return 2;
  }
  auto read = opened.value().read();
  if (!read.ok() || !read.value()) {
    return 2;
  }
#if WAY_IS(faults_begin) || WAY_IS(faults_advance)
  ht::Faults faults = read.value()->faults();
#if WAY_IS(faults_advance)
  auto walking = faults.begin();
  if (!(walking != faults.end())) {
    return 2;
  }
#endif
#else
  const ht::Batch &batch = *read.value();
  auto walking = batch.begin();
  if (!(walking != batch.end()) || (*walking).line() != 2) {
    return 2;
  }
#endif
  auto next = opened.value().read();
  if (!next.ok() || !next.value()) {
    return 2;
  }
  version_then_expect_abort();
#if WAY_IS(faults_begin)
  faults.begin();
#elif WAY_IS(faults_advance) || WAY_IS(rows_advance)
  ++walking;
#elif WAY_IS(rows_postfix)
  walking++;
#else
  (void)(*walking).line();
#endif
  came_back = 1;
#elif WAY_IS(checked_get) || WAY_IS(checked_text)
  // The control first: get_checked() reads the same cells, a violation and a pass.
  ht::Constraints<std::int32_t>::Fields bound;
  bound.maximum = 10;
  ht::Constraints<std::string_view>::Fields length;
  length.max_length = 3;
  const auto at_most = ht::Constraints<std::int32_t>::declare(bound);
  const auto short_text = ht::Constraints<std::string_view>::declare(length);
  if (at_most.value_if() == nullptr || short_text.value_if() == nullptr) {
    return 2;
  }
  auto opened = ht::DelimitedReader::from_bytes("n,s\n11,a\n", ht::Dialect::csv(),
                                                {ht::Column::checked(0, *at_most.value_if()),
                                                 ht::Column::checked(1, *short_text.value_if())});
  if (!opened.ok()) {
    return 2;
  }
  auto read = opened.value().read();
  if (!read.ok() || !read.value()) {
    return 2;
  }
  const ht::Batch &batch = *read.value();
  const auto broken = batch.get_checked<std::int32_t>(0, 0);
  const auto passed = batch.get_checked<std::string_view>(1, 0);
  if (broken.ok() || broken.rejection().is_fault() ||
      broken.rejection().violation().keyword != ht::Keyword::maximum || !passed.ok() ||
      passed.value().value != "a") {
    return 2;
  }
  version_then_expect_abort();
#if WAY_IS(checked_get)
  (void)batch.get<std::int32_t>(0, 0);
#else
  (void)batch.text(1, 0);
#endif
  came_back = 1;
#elif WAY_IS(sheet_destroyed)
  auto book = ht::Workbook::open((std::string(argv[1]) + "/workbook/basic.xlsx").c_str());
  if (!book.ok()) {
    return 2;
  }
  std::optional<ht::Sheet> sheet;
  {
    auto opened = book.value().sheet(0, ht::SheetOptions{}, {ht::Column::text(0)});
    if (!opened.ok()) {
      return 2;
    }
    sheet.emplace(std::move(opened.value()));
  }
  version_then_expect_abort();
  (void)sheet->for_each_row([&](const ht::Row &) {
    sheet.reset();
    came_back = 1;
  });
#else
  // A source that, on its call after the header, does what this way names to its own reader.
  const std::string text = "a\n1\n2\n3\n";
  std::size_t position = 0;
  bool armed = false;
  std::optional<ht::DelimitedReader> reader;
  std::optional<ht::DelimitedReader> other;
  ht::DelimitedReader::Source source =
      [&](std::uint8_t *into, std::size_t capacity) noexcept -> std::optional<std::size_t> {
    if (armed) {
      armed = false;
#if WAY_IS(destroyed)
      reader.reset();
#elif WAY_IS(moved_from)
      other.emplace(std::move(*reader));
#else
      *reader = std::move(*other);
#endif
      came_back = 1;
    }
    const std::size_t length = std::min({capacity, std::size_t{1}, text.size() - position});
    std::memcpy(into, text.data() + position, length);
    position += length;
    return length;
  };
  auto opened = ht::DelimitedReader::from_source(source, ht::Dialect::csv(), {ht::Column::i32(0)});
  auto spare = ht::DelimitedReader::from_bytes("a\n", ht::Dialect::csv(), {ht::Column::i32(0)});
  if (!opened.ok() || !spare.ok()) {
    return 2;
  }
  reader.emplace(std::move(opened.value()));
  other.emplace(std::move(spare.value()));
  version_then_expect_abort();
  armed = true;
  (void)reader->read();
#endif
  std::fprintf(stderr, "smoke: the program went on past a contract violation (%s)\n", argv[0]);
  return 1;
}
