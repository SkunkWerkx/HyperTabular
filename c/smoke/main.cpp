// The C++ header against an unpacked release archive and HyperCast's beside it: delimited text
// and a workbook read through hypertabular.hpp, each cell's hypercast::Verdict taken apart the
// ways it offers, each hypertabular::Result likewise, and the version printed beside main.c's.
// One column declares its notation through hypercast::NumFormat::declare, which asks
// HyperCast's library, so the program links both. CMakeLists.txt builds it as C++17, C++20
// and, where the compiler has it, C++23, warnings as errors, and once more with exceptions off.
//
// usage: smoke_cxx<std> <corpus directory>

#include <cstdint>
#include <cstdio>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>

#include "hypertabular.hpp"

#define CHECK(condition)                                                                           \
  do {                                                                                             \
    if (!(condition)) {                                                                            \
      std::fprintf(stderr, "smoke: %s:%d: failed: %s\n", __FILE__, __LINE__, #condition);          \
      return 1;                                                                                    \
    }                                                                                              \
  } while (0)

namespace ht = hypertabular;
namespace hc = hypercast;

// Nothing a read or a batch offers throws.
static_assert(noexcept(ht::DelimitedReader::from_bytes(std::string_view(), ht::Dialect::csv())),
              "noexcept");
static_assert(noexcept(ht::Workbook::open("")), "noexcept");
static_assert(noexcept(std::declval<ht::DelimitedReader &>().read()), "noexcept");
static_assert(noexcept(std::declval<ht::Sheet &>().read()), "noexcept");
static_assert(noexcept(std::declval<const ht::Batch &>().get<std::int32_t>(0, 0)), "noexcept");
static_assert(noexcept(std::declval<const ht::Batch &>().text(0, 0)), "noexcept");
static_assert(noexcept(std::declval<const ht::Error &>().message()), "noexcept");
// A cell's verdict is HyperCast's own.
static_assert(
    std::is_same_v<decltype(std::declval<const ht::Row &>().get<double>(0)), hc::Verdict<double>>,
    "a cell is a hypercast::Verdict");
static_assert(!std::is_constructible_v<bool, ht::Result<int>>, "a result is not a bool");
static_assert(ht::is_available(), "the core is linked in");

// What a decimal displays as, through to_chars.
static std::string text(const hc::Decimal &decimal) {
  char buffer[hc::Decimal::max_chars];
  const auto written = decimal.to_chars(buffer, buffer + sizeof buffer);
  return std::string(buffer, written.ptr);
}

static int delimited() {
  const std::string_view csv = "id,name,amount,price\n"
                               "1,\"bob, \"\"jr\"\"\",12.50,\"1.234,5\"\n"
                               "\n"
                               "2,,x,\"7,25\"\n";
  // A culture's notation, declared: HyperCast's own library is the judge of it.
  hc::NumFormat::Fields fields;
  fields.decimal_sep = ",";
  fields.group_sep = ".";
  const auto eurozone = hc::NumFormat::declare(fields);
  CHECK(eurozone.has_value());

  auto opened = ht::DelimitedReader::from_bytes(csv, ht::Dialect::csv(), ht::DelimitedOptions{1});
  CHECK(opened.ok());
  ht::DelimitedReader &reader = opened.value();
  const ht::Header *header = reader.header();
  CHECK(header != nullptr && header->size() == 4 && (*header)[1] == "name");
  CHECK(header->ordinal("amount") == std::optional<std::size_t>(2));
  CHECK(header->require("missing").error().kind() == ht::ErrorKind::no_column);
  CHECK(reader.read().error().kind() == ht::ErrorKind::unbound);

  CHECK(reader
            .bind({ht::Column::i32(header->require("id").value()), ht::Column::text(1),
                   ht::Column::decimal(2), ht::Column::decimal(3).format(*eurozone)})
            .ok());
  CHECK(reader.bind({ht::Column::text(0)}).error().kind() == ht::ErrorKind::already_bound);

  // One row a batch: each is a view, over when the next read begins.
  auto first = reader.read();
  CHECK(first.ok() && first.value().has_value() && first.value()->rows() == 1);
  const ht::Row row = first.value()->row(0);
  CHECK(row.line() == 2);
  CHECK(row.get<std::int32_t>(0).value_or(-1) == 1);
  CHECK(row.text(1).value() == "bob, \"jr\"");
  CHECK(text(row.get<hc::Decimal>(2).value()) == "12.5");
  CHECK(text(row.get<hc::Decimal>(3).value()) == "1234.5");
  CHECK(row.raw(1) == "bob, \"jr\"");

  auto second = reader.read();
  CHECK(second.ok() && second.value().has_value());
  const ht::Batch &batch = *second.value();
  CHECK(batch.line(0) == 4); // the blank line is skipped
  CHECK(batch.get<std::int32_t>(0, 0).match([](std::int32_t id) noexcept { return id; },
                                            [](const hc::Fault &) noexcept { return -1; }) == 2);
  // No bytes is the one way text fails.
  CHECK(batch.text(1, 0).fault().reason == hc::CastFailure::empty);
  const hc::Verdict<hc::Decimal> amount = batch.get<hc::Decimal>(2, 0);
  const hc::Fault *fault = amount.fault_if();
  CHECK(fault != nullptr && fault->reason == hc::CastFailure::malformed && fault->offset == 0 &&
        fault->length == 1);
  CHECK(batch.raw(2, 0).substr(fault->offset, fault->length) == "x");
  CHECK(text(batch.get<hc::Decimal>(3, 0).value()) == "7.25");

  auto end = reader.read();
  CHECK(end.ok() && !end.value().has_value());
  CHECK(reader.records() == 4); // the header, two rows and the blank line between

  // A structural failure comes after every intact row before it, and stays.
  auto ragged =
      ht::DelimitedReader::from_bytes("a,b\n1,2\n3\n", ht::Dialect::csv(), {ht::Column::i64(0)});
  CHECK(ragged.ok());
  std::size_t rows = 0;
  const auto read = ragged.value().for_each_row([&rows](const ht::Row &) noexcept { ++rows; });
  CHECK(rows == 1 && read.error().kind() == ht::ErrorKind::structure);
  const ht::Failure failure = read.error().failure();
  CHECK(failure.kind == ht::FailureKind::column_count && failure.record == 2 && failure.line == 3 &&
        failure.byte == 8 && failure.expected == 2 && failure.found == 1);
  CHECK(read.error().message() == "record 2 (line 3, byte 8) has 1 cells where the first has 2");
  CHECK(ragged.value().read().error().failure() == failure);
  CHECK(ht::DelimitedReader::from_bytes("a\n", ht::Dialect{'"'}).error().kind() ==
        ht::ErrorKind::separator);

#ifdef HYPERTABULAR_HAS_EXPECTED
  std::expected<ht::DelimitedReader, ht::Error> expected =
      ht::DelimitedReader::from_bytes(csv, ht::Dialect::csv());
  CHECK(expected.has_value() && expected->column_count() == std::optional<std::size_t>(4));
  std::expected<void, ht::Error> bound = expected->bind({ht::Column::text(9)});
  CHECK(!bound.has_value() && bound.error().kind() == ht::ErrorKind::plan);
#endif
  return 0;
}

static int workbook(const std::string &corpus) {
  auto opened = ht::Workbook::open((corpus + "/workbook/basic.xlsx").c_str());
  CHECK(opened.ok());
  const ht::Workbook &book = opened.value();
  CHECK(book.format() == ht::Format::xlsx && book.date_system() == hc::ExcelEpoch::y1900);
  CHECK(book.sheets().size() >= 1 && book.sheets()[0].name() == "Data" &&
        !book.sheets()[0].hidden());
  CHECK(book.sheet(std::string_view("No such sheet"), {}, {}).error().kind() ==
        ht::ErrorKind::no_sheet);

  auto sheet = book.sheet_unbound(std::string_view("Data"));
  CHECK(sheet.ok() && sheet.value().header() != nullptr);
  const ht::Header &header = *sheet.value().header();
  CHECK(header.size() == 9 && header[0] == "id" && header[8] == "at");
  CHECK(sheet.value().bind({ht::Column::i64(0), ht::Column::text(1)}).ok());

  const std::int64_t ids[] = {1, 2, 3, 6};
  const std::uint32_t lines[] = {2, 3, 4, 6};
  const char *const names[] = {"alice", "bob, jr", nullptr, nullptr};
  std::size_t at = 0;
  bool same = true;
  const auto read = sheet.value().for_each_row([&](const ht::Row &row) noexcept {
    same = same && at < 4 && row.line() == lines[at] && row.get<std::int64_t>(0).value() == ids[at];
    const hc::Verdict<std::string_view> name = row.text(1);
    same = same && (names[at] != nullptr ? name.ok() && name.value() == names[at]
                                         : name.fault().reason == hc::CastFailure::empty);
    ++at;
  });
  CHECK(read.ok() && same && at == 4);
  return 0;
}

int main(int argc, char **argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: %s <corpus directory>\n", argv[0]);
    return 2;
  }
  if (delimited() != 0 || workbook(argv[1]) != 0) {
    return 1;
  }
  const std::string_view version = ht::native_version();
  std::printf("%.*s\n", static_cast<int>(version.size()), version.data());
  return 0;
}
