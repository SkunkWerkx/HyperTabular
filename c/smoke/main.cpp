// The C++ header against an unpacked release archive and HyperCast's beside it: delimited text
// and a workbook read through hypertabular.hpp, each cell's hypercast::Verdict taken apart the
// ways it offers, each hypertabular::Result likewise, and the version printed beside main.c's.
// One column declares its notation through hypercast::NumFormat::declare, which asks
// HyperCast's library, so the program links both. Each read's options, its notes and faults, a
// declared encoding, a suggestion and a range go through it too, and the 1.0 entry points are
// compared with a read, and columns that declare HyperCast's constraints are read back through
// get_checked(). CMakeLists.txt builds it as C++17, C++20 and, where the compiler has it, C++23,
// warnings as errors, and once more with exceptions off.
//
// usage: smoke_cxx<std> <corpus directory>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <limits>
#include <optional>
#include <string>
#if __has_include(<ranges>)
#include <ranges>
#endif
#include <string_view>
#include <type_traits>
#include <variant>
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
static_assert(noexcept(std::declval<const ht::Batch &>().faults()), "noexcept");
static_assert(noexcept(std::declval<const ht::Batch &>().detail(0, 0)), "noexcept");
static_assert(noexcept(ht::DelimitedReader::suggest_bytes(std::string_view(), ht::Dialect::csv())),
              "noexcept");
static_assert(noexcept(ht::Range::parse(std::string_view())), "noexcept");
// A cell's verdict is HyperCast's own.
static_assert(
    std::is_same_v<decltype(std::declval<const ht::Row &>().get<double>(0)), hc::Verdict<double>>,
    "a cell is a hypercast::Verdict");
static_assert(std::is_same_v<decltype(std::declval<const ht::Row &>().get_checked<double>(0)),
                             hc::Checked<double>>,
              "a constrained cell is a hypercast::Checked");
static_assert(noexcept(std::declval<const ht::Batch &>().get_checked<std::int32_t>(0, 0)),
              "noexcept");
static_assert(!std::is_constructible_v<bool, ht::Result<int>>, "a result is not a bool");
static_assert(ht::is_available(), "the core is linked in");
#if defined(__cpp_lib_ranges) && __cpp_lib_ranges >= 201911L
static_assert(std::ranges::input_range<ht::Faults>, "a batch's faults are a range");
static_assert(std::ranges::forward_range<ht::Batch>, "a batch's rows are a range");
#endif

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

// A read's options, all at once: a junk line passed over, a record discarded, a header with a name
// twice, tokens for the read and for a column, a short row padded and a long one reported.
static int options() {
  const std::string_view csv = "junk, \"not a quote\n"
                               "discarded\n"
                               "id,v,v\n"
                               "1,NA,2\n"
                               "2,x,3\n"
                               "3\n"
                               "4,5,6,7\n";
  const ht::Dialect dialect = ht::Dialect::csv()
                                  .with_skip(1)
                                  .with_header_row(1)
                                  .with_short_rows(ht::ShortRows::pad)
                                  .with_long_rows(ht::LongRows::report)
                                  .with_tokens({"NA"});
  auto opened = ht::DelimitedReader::from_bytes(csv, dialect);
  CHECK(opened.ok() && opened.value().header() != nullptr);
  ht::DelimitedReader &reader = opened.value();
  const ht::Header &header = *reader.header();
  CHECK(header.ordinal("v") == std::optional<std::size_t>(1)); // the first, with nothing said
  const auto ambiguous = header.require("v");
  CHECK(!ambiguous.ok() && ambiguous.error().kind() == ht::ErrorKind::ambiguous_column &&
        ambiguous.error().first() == 1 && ambiguous.error().second() == 2);
  CHECK(ambiguous.error().message() ==
        "the header names two columns \"v\", at positions 1 and 2: say which by position");
  CHECK(header.require("id").value() == 0);
  CHECK(
      reader.bind({ht::Column::i32(0), ht::Column::i32(1), ht::Column::i32(2).tokens({"2"})}).ok());

  auto read = reader.read();
  CHECK(read.ok() && read.value().has_value());
  const ht::Batch &batch = *read.value();
  CHECK(batch.rows() == 4 && batch.line(0) == 4 && batch.line(3) == 7);
  CHECK(batch.origin(0, 0) == ht::Origin::cell && batch.detail(0, 0) == hc::Detail::none);
  CHECK(batch.fault(1, 0)->reason == hc::CastFailure::empty);
  CHECK(batch.origin(1, 0) == ht::Origin::token && ht::name(batch.origin(1, 0)) == "token");
  CHECK(batch.origin(2, 0) == ht::Origin::token); // the column's tokens replace the read's
  CHECK(batch.fault(1, 1)->reason == hc::CastFailure::malformed);
  CHECK(batch.detail(1, 1) != hc::Detail::none && batch.row(1).detail(1) == batch.detail(1, 1));
  CHECK(batch.origin(1, 2) == ht::Origin::padded && batch.row(2).origin(1) == ht::Origin::padded);
  CHECK(batch.get<std::int32_t>(2, 3).value() == 6); // the long row is cut to the width

  std::vector<ht::Problem> problems;
  for (const ht::Problem &problem : batch.faults()) {
    problems.push_back(problem);
  }
  CHECK(problems.size() == 2);
  CHECK(problems[0].row == 1 && problems[0].line == 5 && problems[0].plan_column == 1u &&
        problems[0].source_column == 1u && problems[0].reason == hc::CastFailure::malformed &&
        problems[0].detail == batch.detail(1, 1) && problems[0].offset == 0 &&
        problems[0].length == 1);
  CHECK(problems[1].row == 3 && !problems[1].plan_column && !problems[1].reason &&
        problems[1].expected == 3 && problems[1].found == 4);
  CHECK(reader.read().value() == std::nullopt);

  // With the notes off a refused cell is still listed, with nothing said of why.
  auto quiet = ht::DelimitedReader::from_bytes(csv, dialect, {ht::Column::i32(1)},
                                               ht::DelimitedOptions{}.with_notes(false));
  CHECK(quiet.ok());
  auto silent = quiet.value().read();
  CHECK(silent.ok() && silent.value()->detail(0, 1) == hc::Detail::none &&
        silent.value()->origin(0, 2) == ht::Origin::cell);
  auto listed = silent.value()->faults();
  auto first = listed.begin();
  CHECK(first != listed.end() && (*first).detail == hc::Detail::none && (*first).row == 1);

  // A date's options and months, and tokens of none.
  auto months = hc::MonthNames();
  std::uint8_t storage[96];
  const hc::MonthName names[] = {{1, "Jan"}, {2, "Feb"},  {3, "Mar"},  {4, "Apr"},
                                 {5, "May"}, {6, "Jun"},  {7, "Jul"},  {8, "Aug"},
                                 {9, "Sep"}, {10, "Oct"}, {11, "Nov"}, {12, "Dec"}};
  const auto declared = hc::MonthNames::declare(names, hc::EnumStorage(storage));
  CHECK(declared.has_value());
  months = *declared;
  auto dated = ht::DelimitedReader::from_bytes(
      "d,e\n02-Feb-2024,NA\n", ht::Dialect::csv().with_tokens({"NA"}),
      {ht::Column::date_ordered(0, hc::DateOrder::day_month_year)
           .date_options(hc::DateOptions::none(), months),
       ht::Column::text(1).tokens(std::vector<std::string>{})});
  CHECK(dated.ok());
  // The column's copy outlives what it was declared from.
  std::fill(std::begin(storage), std::end(storage), std::uint8_t{0});
  auto day = dated.value().read();
  CHECK(day.ok() && day.value()->get<hc::Date>(0, 0).value().month == 2);
  CHECK(day.value()->text(1, 0).value() == "NA");

  // A plan the core won't take is said when it's bound, whatever it's the first to ask of.
  auto refused = ht::DelimitedReader::from_bytes(
      "a,b\n", ht::Dialect::csv(),
      {ht::Column::text(0), ht::Column::of(1, ht::Door::unix_time, 9)});
  CHECK(!refused.ok() && refused.error().kind() == ht::ErrorKind::plan &&
        refused.error().column() == 1);
  return 0;
}

// The core decides how much a batch may ask for, and every way in asks it before allocating. An
// explicit zero is refused; an omitted option has its default; a sample of 0 is one row. A
// std::vector writes what it's asked for, so the sizes here are past what a vector can be: if the
// check went, it would throw in a noexcept function, with nothing written. main.c holds the
// just-over boundary.
static bool same_words(const ht::Suggestion &left, const ht::Suggestion &right) {
  if (left.truncated != right.truncated || left.report.size() != right.report.size()) {
    return false;
  }
  for (std::size_t at = 0; at < left.report.size(); ++at) {
    const auto &one = left.report[at];
    const auto &other = right.report[at];
    if (one.door != other.door || one.status != other.status || one.matched != other.matched ||
        one.sampled != other.sampled) {
      return false;
    }
  }
  return true;
}

static int sizes(const std::string &corpus) {
  const std::string_view input = "a\n1\n";
  const std::vector<ht::Column> plan{ht::Column::i32(0)};
  const std::size_t past_a_vector = std::numeric_limits<std::size_t>::max() / 2;
  const auto delimited_with = [&](std::size_t rows, std::size_t buffer) {
    ht::DelimitedOptions options;
    options.batch_rows = rows;
    options.buffer_bytes = buffer;
    return ht::DelimitedReader::from_bytes(input, ht::Dialect::csv(), plan, options);
  };

  auto over = delimited_with(past_a_vector, 1024);
  CHECK(!over.ok() && over.error().kind() == ht::ErrorKind::batch_too_large);
  CHECK(over.error().limit() == 4294967296u);
  CHECK(over.error().needed() > over.error().limit());
  CHECK(over.error().message() == "batch_rows asks for " + std::to_string(over.error().needed()) +
                                      " bytes, past the limit of 4294967296");

  auto zero_rows = delimited_with(0, 1024);
  CHECK(!zero_rows.ok() && zero_rows.error().kind() == ht::ErrorKind::zero_batch_rows);
  CHECK(zero_rows.error().message() == "batch_rows must be at least 1");
  auto zero_buffer = delimited_with(1, 0);
  CHECK(!zero_buffer.ok() && zero_buffer.error().kind() == ht::ErrorKind::zero_buffer_bytes);
  CHECK(zero_buffer.error().message() == "buffer_bytes must be at least 1");

  // Every way in refuses at open, with no plan to bind and before touching a file or a source.
  ht::DelimitedOptions no_rows;
  no_rows.batch_rows = 0;
  ht::DelimitedOptions no_buffer;
  no_buffer.buffer_bytes = 0;
  const auto nothing = [](std::uint8_t *, std::size_t) -> std::optional<std::size_t> {
    return std::size_t{0};
  };
  const auto kind_of = [](const auto &opened) { return opened.error().kind(); };
  const auto from_bytes = [&](const ht::DelimitedOptions &options) {
    return ht::DelimitedReader::from_bytes(input, ht::Dialect::csv(), options);
  };
  const auto from_source = [&](const ht::DelimitedOptions &options) {
    return ht::DelimitedReader::from_source(nothing, ht::Dialect::csv(), options);
  };
  const auto from_path = [&](const ht::DelimitedOptions &options) {
    return ht::DelimitedReader::open("no such file", ht::Dialect::csv(), options);
  };
  CHECK(kind_of(from_bytes(no_rows)) == ht::ErrorKind::zero_batch_rows);
  CHECK(kind_of(from_bytes(no_buffer)) == ht::ErrorKind::zero_buffer_bytes);
  CHECK(kind_of(from_source(no_rows)) == ht::ErrorKind::zero_batch_rows);
  CHECK(kind_of(from_source(no_buffer)) == ht::ErrorKind::zero_buffer_bytes);
  CHECK(kind_of(from_path(no_rows)) == ht::ErrorKind::zero_batch_rows);
  CHECK(kind_of(from_path(no_buffer)) == ht::ErrorKind::zero_buffer_bytes);

  ht::DelimitedOptions unbound;
  unbound.batch_rows = past_a_vector;
  auto late = ht::DelimitedReader::from_bytes(input, ht::Dialect::csv(), unbound);
  CHECK(late.ok());
  const auto bound = late.value().bind(plan);
  CHECK(!bound.ok() && bound.error().kind() == ht::ErrorKind::batch_too_large);
  CHECK(!late.value().is_bound());

  auto opened = ht::Workbook::open((corpus + "/workbook/basic.xlsx").c_str());
  CHECK(opened.ok());
  ht::SheetOptions large;
  large.batch_rows = past_a_vector;
  auto sheet = opened.value().sheet(std::size_t{0}, large, plan);
  CHECK(!sheet.ok() && sheet.error().kind() == ht::ErrorKind::batch_too_large);
  // A sheet with no plan holds a cell table already, so one past a vector is refused at open.
  auto refused = opened.value().sheet_unbound(std::size_t{0}, large);
  CHECK(!refused.ok() && refused.error().kind() == ht::ErrorKind::batch_too_large);
  // This one has to open, at exactly the limit, and a plan wide enough that the cell table a bind
  // asks for first is 2^49 bytes: if the check went, the vector fails with nothing written.
  ht::SheetOptions just_opens;
  just_opens.batch_rows = std::size_t{1} << 29;
  auto unplanned = opened.value().sheet_unbound(std::size_t{0}, just_opens);
  CHECK(unplanned.ok());
  const std::vector<ht::Column> wide(std::size_t{1} << 17, ht::Column::i32(0));
  const auto sheet_bound = unplanned.value().bind(wide);
  CHECK(!sheet_bound.ok() && sheet_bound.error().kind() == ht::ErrorKind::batch_too_large);
  CHECK(!unplanned.value().is_bound());
  ht::SheetOptions none;
  none.batch_rows = 0;
  auto sheet_zero = opened.value().sheet_unbound(std::size_t{0}, none);
  CHECK(!sheet_zero.ok() && sheet_zero.error().kind() == ht::ErrorKind::zero_batch_rows);

  auto modest = delimited_with(std::size_t{1} << 16, 1024);
  CHECK(modest.ok());
  const auto read = modest.value().read();
  CHECK(read.ok() && read.value().has_value() && read.value()->rows() == 1);
  CHECK(ht::DelimitedOptions{}.batch_rows == 4096 && ht::SheetOptions{}.batch_rows == 4096);
  std::string rows = "a\n";
  for (int row = 0; row <= 4096; ++row) {
    rows += "1\n";
  }
  auto omitted = ht::DelimitedReader::from_bytes(rows, ht::Dialect::csv(), plan);
  CHECK(omitted.ok());
  const auto first = omitted.value().read();
  CHECK(first.ok() && first.value().has_value() && first.value()->rows() == 4096);
  // A sampler's batch is never why a suggestion is refused: nobody set its size.
  const std::size_t columns = 268000;
  std::string names = "c0";
  std::string row = "1";
  for (std::size_t column = 1; column < columns; ++column) {
    names += ",c" + std::to_string(column);
    row += ",1";
  }
  const std::string wide_text = names + "\n" + row + "\n" + row + "\n";
  const auto wide_found = ht::DelimitedReader::suggest_bytes(wide_text, ht::Dialect::csv());
  CHECK(wide_found.ok() && wide_found.value().report.size() == columns);
  CHECK(wide_found.value().report[columns - 1].sampled == 2 && !wide_found.value().truncated);
  // At exactly the limit with no plan, and a sample past every batch.
  auto at_the_limit = opened.value().sheet_unbound(std::size_t{0}, just_opens);
  CHECK(at_the_limit.ok());
  ht::SuggestOptions many;
  many.sample_rows = std::size_t{1} << 40;
  const auto sampled = at_the_limit.value().suggest(many);
  CHECK(sampled.ok());
  auto modest_sheet = opened.value().sheet_unbound(std::size_t{0}, ht::SheetOptions{});
  CHECK(modest_sheet.ok());
  ht::SuggestOptions all;
  all.sample_rows = 4096;
  const auto modest_found = modest_sheet.value().suggest(all);
  CHECK(modest_found.ok() && same_words(sampled.value(), modest_found.value()));

  // A sample says the same in any number of batches, delimited and sheet.
  const std::string_view messy =
      "id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n"
      "3,,2024-03-04,\"a, b\"\nx,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n";
  for (const std::size_t limit : {1, 2, 3, 5, 6, 1000}) {
    ht::SuggestOptions options;
    options.sample_rows = limit;
    const auto whole = ht::DelimitedReader::suggest_bytes(messy, ht::Dialect::csv(), options);
    CHECK(whole.ok());
    for (const std::size_t in_each : {1, 2, 3, 7}) {
      ht::DelimitedOptions in_batches;
      in_batches.batch_rows = in_each;
      in_batches.notes = false;
      auto reader = ht::DelimitedReader::from_bytes(messy, ht::Dialect::csv(), in_batches);
      CHECK(reader.ok());
      ht::detail::Sample sample(reader.value().header()->size(), limit);
      CHECK(sample.read_from(reader.value()).ok());
      const auto found = sample.judge(reader.value().header(), options);
      CHECK(found.ok() && same_words(found.value(), whole.value()));
    }
  }
  for (const std::size_t limit : {1, 2, 3, 1000}) {
    ht::SuggestOptions options;
    options.sample_rows = limit;
    auto whole_sheet = opened.value().sheet_unbound(std::size_t{0}, ht::SheetOptions{});
    CHECK(whole_sheet.ok());
    const auto whole = whole_sheet.value().suggest(options);
    CHECK(whole.ok());
    for (const std::size_t in_each : {1, 2, 3}) {
      ht::SheetOptions in_batches;
      in_batches.batch_rows = in_each;
      in_batches.notes = true;
      auto sheet_in = opened.value().sheet_unbound(std::size_t{0}, in_batches);
      CHECK(sheet_in.ok());
      ht::detail::Sample sample(sheet_in.value().header()->size(), limit, true);
      CHECK(sample.read_from(sheet_in.value()).ok());
      const auto found = sample.judge(sheet_in.value().header(), options);
      CHECK(found.ok() && same_words(found.value(), whole.value()));
    }
  }
  // With no header, a short and a long row, and row widths kept, across each batch boundary.
  const std::string_view ragged = "1,x\n2,y,long\n3\n4,z\n5,w\n";
  ht::Dialect headerless = ht::Dialect::csv();
  headerless.has_header = false;
  headerless.short_rows = ht::ShortRows::pad;
  headerless.long_rows = ht::LongRows::report;
  for (const std::size_t limit : {1, 2, 3, 4, 5, 6}) {
    ht::SuggestOptions options;
    options.sample_rows = limit;
    const auto whole = ht::DelimitedReader::suggest_bytes(ragged, headerless, options);
    CHECK(whole.ok() && whole.value().report[0].sampled == std::min<std::size_t>(limit, 5));
    CHECK(whole.value().truncated == (limit < 5));
    for (const std::size_t in_each : {1, 2, 3, 4}) {
      ht::DelimitedOptions in_batches;
      in_batches.batch_rows = in_each;
      in_batches.notes = false;
      auto reader = ht::DelimitedReader::from_bytes(ragged, headerless, in_batches);
      CHECK(reader.ok());
      ht::detail::Sample sample(2, limit);
      CHECK(sample.read_from(reader.value()).ok());
      const auto found = sample.judge(reader.value().header(), options);
      CHECK(found.ok() && same_words(found.value(), whole.value()));
    }
  }
  // A control: the sample's size does move the answer, so the comparison above can fail.
  ht::SuggestOptions just_one;
  just_one.sample_rows = 1;
  ht::SuggestOptions six;
  six.sample_rows = 6;
  CHECK(!same_words(ht::DelimitedReader::suggest_bytes(messy, ht::Dialect::csv(), just_one).value(),
                    ht::DelimitedReader::suggest_bytes(messy, ht::Dialect::csv(), six).value()));

  // One row past the limit at any batch size names its width: no public plan gets there, so this
  // asks directly.
  HyperTabularColumnSpecWith far{};
  far.size = HyperTabularColumnSpecWith_SIZE;
  far.ordinal = 0xFFFFFFFFu;
  far.door = 4; // i32
  const auto too_wide = ht::detail::check_sizes({far}, 1, 1, HyperTabularRawBatchCheck_SHEET);
  CHECK(!too_wide.ok() && too_wide.error().kind() == ht::ErrorKind::row_too_wide);
  CHECK(too_wide.error().width() == 4294967296u && too_wide.error().needed() == 68719476768u);
  CHECK(too_wide.error().message() ==
        "a row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296");

  ht::SuggestOptions one;
  one.sample_rows = 0;
  const auto suggested = ht::DelimitedReader::suggest_bytes(input, ht::Dialect::csv(), one);
  CHECK(suggested.ok() && suggested.value().report[0].sampled == 1);
  return 0;
}

// An entry already in hand stays what it said when the reader moves on. Walking on from it ends the
// program: aborts.cpp proves that.
static int faults_keep_the_entry_in_hand() {
  auto opened = ht::DelimitedReader::from_bytes("a,b\nx,x\ny,y\n", ht::Dialect::csv(),
                                                {ht::Column::i32(0), ht::Column::i32(1)},
                                                ht::DelimitedOptions{1});
  CHECK(opened.ok());
  auto read = opened.value().read();
  CHECK(read.ok());
  const ht::Batch batch = *read.value();
  ht::Faults faults = batch.faults();
  auto walking = faults.begin();
  CHECK(walking != faults.end() && (*walking).row == 0);
  const std::uint32_t line = (*walking).line;
  auto next = opened.value().read();
  CHECK(next.ok() && next.value()->fault(0, 0).has_value());
  // An iterator held over the read still says what it was holding, not the next batch's line.
  CHECK((*walking).line == line && line == 2 && next.value()->line(0) == 3);
  return 0;
}

// A source calling back into the reader it feeds, and a for_each_row callback into the reader or
// sheet it walks: a read() or bind() from there is ErrorKind::busy, and the read it is in, and
// every one after, delivers each row once, in order, from the right line. Before the reader marked
// a read in progress, the inner read reallocated the buffer the outer source was writing into, and
// the callback's moved the cells under the walk (AddressSanitizer: heap-use-after-free, both). A
// reader or sheet destroyed or moved from there ends the program: aborts.cpp proves that.
struct ReentryRow {
  std::int32_t id;
  std::string name;
  std::uint32_t line;
  bool operator==(const ReentryRow &other) const {
    return id == other.id && name == other.name && line == other.line;
  }
};

static std::string reentry_name(int i) {
  return i == 5 ? std::string(40, 'x') : "name-" + std::to_string(i);
}

static int reentry(const std::string &corpus) {
  const int count = 100;
  std::string text = "id,name\n";
  std::vector<ReentryRow> whole;
  for (int i = 0; i < count; ++i) {
    text += std::to_string(i) + "," + reentry_name(i) + "\n";
    whole.push_back({i, reentry_name(i), static_cast<std::uint32_t>(i + 2)});
  }
  const std::vector<ht::Column> plan{ht::Column::i32(0), ht::Column::text(1)};
  const auto busy = [](const ht::Error &error, const char *which) {
    return error.kind() == ht::ErrorKind::busy && error.detail() == which &&
           error.message() == std::string("the ") + which +
                                  " is in the middle of a read; nothing that read calls can call "
                                  "back into it";
  };
  for (const ht::Encoding encoding : {ht::Encoding::utf8, ht::Encoding::latin_1}) {
    for (const std::size_t chunk : {1, 7, 64}) {
      for (const std::size_t buffer : {8, 64}) {
        for (const int at : {2, 3, 5, 8, 13}) {
          std::size_t position = 0;
          int calls = 0;
          int fire_at = -1;
          bool fired = false;
          bool refused = true;
          ht::DelimitedReader *self = nullptr;
          ht::DelimitedReader::Source source =
              [&](std::uint8_t *into, std::size_t capacity) noexcept -> std::optional<std::size_t> {
            if (++calls == fire_at) {
              fired = true;
              auto inner = self->read();
              refused = refused && !inner.ok() && busy(inner.error(), "reader");
              auto bound = self->bind({ht::Column::i32(0)});
              refused = refused && !bound.ok() && busy(bound.error(), "reader");
              auto walked = self->for_each_row([](const ht::Row &) {});
              refused = refused && !walked.ok() && busy(walked.error(), "reader");
            }
            const std::size_t length = std::min({capacity, chunk, text.size() - position});
            std::memcpy(into, text.data() + position, length);
            position += length;
            return length;
          };
          ht::Dialect dialect = ht::Dialect::csv();
          dialect.encoding = encoding;
          ht::DelimitedOptions options{1};
          options.buffer_bytes = buffer;
          auto opened = ht::DelimitedReader::from_source(std::move(source), dialect, plan, options);
          CHECK(opened.ok());
          ht::DelimitedReader reader = std::move(opened.value());
          // The header was read before there was a reader to call: count from after it.
          self = &reader;
          fire_at = calls + at;
          std::vector<ReentryRow> read;
          for (;;) {
            auto batch = reader.read();
            CHECK(batch.ok());
            if (!*batch.value_if()) {
              break;
            }
            for (const ht::Row &row : **batch.value_if()) {
              read.push_back({row.get<std::int32_t>(0).value_or(-1),
                              std::string(row.text(1).value_or("")), row.line()});
            }
          }
          CHECK(fired && refused);
          CHECK(read == whole);
        }
      }
    }
  }

  // A callback: its read(), bind() and a nested for_each_row are refused, and the walk delivers
  // every row, though the next batch's arena grows for a long escaped cell: the shape that freed it
  // under the walk.
  std::string escaped = "a\n";
  for (int i = 0; i < 8; ++i) {
    escaped += "\"x\"\"" + std::to_string(i) + "\"\n";
  }
  escaped += "\"" + std::string(20000, 'y') + "\"\"\"\n";
  auto opened = ht::DelimitedReader::from_bytes(escaped, ht::Dialect::csv(), {ht::Column::text(0)},
                                                ht::DelimitedOptions{8});
  CHECK(opened.ok());
  ht::DelimitedReader &walked = opened.value();
  std::vector<std::string> cells;
  bool refused = true;
  auto walk = walked.for_each_row([&](const ht::Row &row) {
    cells.emplace_back(row.text(0).value_or(""));
    if (cells.size() == 2) {
      auto inner = walked.read();
      refused = refused && !inner.ok() && busy(inner.error(), "reader");
      auto bound = walked.bind({ht::Column::text(0)});
      refused = refused && !bound.ok() && busy(bound.error(), "reader");
      auto nested = walked.for_each_row([](const ht::Row &) {});
      refused = refused && !nested.ok() && busy(nested.error(), "reader");
    }
  });
  CHECK(walk.ok() && refused && cells.size() == 9);
  for (int i = 0; i < 8; ++i) {
    CHECK(cells[static_cast<std::size_t>(i)] == "x\"" + std::to_string(i));
  }
  CHECK(cells[8] == std::string(20000, 'y') + "\"");
  // Once the walk is over the reader is the caller's again.
  auto after = walked.read();
  CHECK(after.ok() && !*after.value_if());

  // A sheet's callback, the same, a nested walk included.
  auto book = ht::Workbook::open((corpus + "/workbook/basic.xlsx").c_str());
  CHECK(book.ok());
  ht::SheetOptions one_row;
  one_row.batch_rows = 1;
  auto sheet = book.value().sheet(0, one_row, {ht::Column::text(0)});
  CHECK(sheet.ok());
  ht::Sheet &walking = sheet.value();
  std::size_t rows = 0;
  refused = true;
  auto sheet_walk = walking.for_each_row([&](const ht::Row &) {
    if (++rows == 1) {
      auto inner = walking.read();
      refused = refused && !inner.ok() && busy(inner.error(), "sheet");
      auto bound = walking.bind({ht::Column::text(0)});
      refused = refused && !bound.ok() && busy(bound.error(), "sheet");
      auto nested = walking.for_each_row([](const ht::Row &) {});
      refused = refused && !nested.ok() && busy(nested.error(), "sheet");
    }
  });
  CHECK(sheet_walk.ok() && refused && rows > 1);
  auto again = book.value().sheet(0, one_row, {ht::Column::text(0)});
  CHECK(again.ok());
  std::size_t whole_rows = 0;
  CHECK(again.value().for_each_row([&](const ht::Row &) { ++whole_rows; }).ok());
  CHECK(rows == whole_rows);
  return 0;
}

static int encodings() {
  auto latin = ht::DelimitedReader::from_bytes(
      "name\ncaf\xE9\n", ht::Dialect::csv().with_encoding(ht::Encoding::windows_1252),
      {ht::Column::text(0)});
  CHECK(latin.ok());
  auto read = latin.value().read();
  CHECK(read.ok() && read.value()->text(0, 0).value() == "caf\xC3\xA9");

  // Every intact row first, then the failure, at the unit's place in the original bytes.
  static const char wide[] = {'a', 0, '\n', 0, '1', 0, '\n', 0, 0, '\xDC'};
  auto broken = ht::DelimitedReader::from_bytes(
      std::string_view(wide, sizeof wide), ht::Dialect::csv().with_encoding(ht::Encoding::utf_16le),
      {ht::Column::i32(0)});
  CHECK(broken.ok());
  std::size_t rows = 0;
  const auto ended = broken.value().for_each_row([&rows](const ht::Row &) noexcept { ++rows; });
  CHECK(rows == 1 && ended.error().kind() == ht::ErrorKind::structure &&
        ended.error().failure().kind == ht::FailureKind::encoding &&
        ended.error().failure().byte == 8);
  return 0;
}

static int suggestions(const std::string &corpus) {
  const std::string_view csv = "id,when,amount,note\n"
                               "1,01/02/2024,\"1,234\",a\n"
                               "2,03/04/2024,2.5,\n";
  auto found = ht::DelimitedReader::suggest_bytes(csv, ht::Dialect::csv());
  CHECK(found.ok());
  const ht::Suggestion &suggestion = found.value();
  CHECK(suggestion.report.size() == 4 && suggestion.plan.size() == 4 && !suggestion.truncated);
  const ht::SuggestedColumn &id = suggestion.report[0];
  CHECK(id.ordinal == 0 && id.name == std::optional<std::string>("id") &&
        id.door == ht::Door::i32 && id.status == ht::SuggestStatus::resolved && id.matched == 2 &&
        id.sampled == 2 && id.format == hc::NumFormat::invariant() && id.candidates.empty());
  // A date's order is the caller's to choose: the plan reads it as text until then, and nothing
  // is applied.
  const ht::SuggestedColumn &when = suggestion.report[1];
  CHECK(when.door == ht::Door::date_ordered && when.status == ht::SuggestStatus::ambiguous &&
        ht::name(when.status) == "ambiguous" && when.candidates.size() == 2);
  CHECK(std::get<hc::DateOrder>(when.candidates[0]) == hc::DateOrder::month_day_year &&
        std::get<hc::DateOrder>(when.candidates[1]) == hc::DateOrder::day_month_year);
  CHECK(suggestion.plan[1].door() == ht::Door::text);
  CHECK(suggestion.plan[2].door() == ht::Door::f64 &&
        suggestion.plan[2].format() == hc::NumFormat::invariant());
  CHECK(suggestion.report[3].door == ht::Door::text);

  // A plan reads back what was sampled.
  auto back = ht::DelimitedReader::from_bytes(csv, ht::Dialect::csv(), suggestion.plan);
  CHECK(back.ok());
  auto read = back.value().read();
  CHECK(read.ok() && read.value()->get<double>(2, 0).value() == 1234.0);

  const auto part = ht::DelimitedReader::suggest_bytes(csv, ht::Dialect::csv(),
                                                       ht::SuggestOptions{}.with_sample_rows(1));
  CHECK(part.ok() && part.value().truncated && part.value().report[0].sampled == 1);

  // What the notation settles: the last declared for a column wins, ahead of everyone's.
  hc::NumFormat::Fields fields;
  fields.decimal_sep = ",";
  fields.group_sep = ".";
  const hc::NumFormat european = hc::NumFormat::declare(fields).value();
  const auto declared = ht::DelimitedReader::suggest_bytes(
      "n,m\n\"1,5\",\"1,5\"\n", ht::Dialect::csv(),
      ht::SuggestOptions{}.with_format(hc::NumFormat::invariant()).with_column_format(1, european));
  CHECK(declared.ok() && declared.value().report[1].door == ht::Door::f64 &&
        declared.value().report[1].format == european);
  CHECK(declared.value().report[0].status != ht::SuggestStatus::resolved ||
        declared.value().report[0].door != ht::Door::f64);

  // Comma groups or a comma decimal: a question for the caller, in formats to hand back.
  const auto notation = ht::DelimitedReader::suggest_bytes("n\n\"1,234\"\n", ht::Dialect::csv());
  CHECK(notation.ok() && notation.value().report[0].status == ht::SuggestStatus::ambiguous &&
        notation.value().report[0].candidates.size() == 2);
  CHECK(std::get<hc::NumFormat>(notation.value().report[0].candidates[0]) ==
        hc::NumFormat::invariant());
  CHECK(std::get<hc::NumFormat>(notation.value().report[0].candidates[1]) == european);

  // What only the comma decimal reads is resolved in it, and the plan reads it back; what neither
  // reads alone settles nothing.
  const std::string_view only_comma = "n,m\n\"0,125\",1.234.567\n\"1.234,5\",\"1,234\"\n";
  const auto settled = ht::DelimitedReader::suggest_bytes(only_comma, ht::Dialect::csv());
  CHECK(settled.ok() && settled.value().report[0].status == ht::SuggestStatus::resolved &&
        settled.value().report[0].door == ht::Door::f64 &&
        settled.value().report[0].format == european &&
        settled.value().report[0].candidates.empty());
  CHECK(settled.value().plan[0].format() == european);
  CHECK(settled.value().report[1].status == ht::SuggestStatus::resolved &&
        settled.value().report[1].door == ht::Door::f64 &&
        settled.value().report[1].format == european);
  auto settled_read =
      ht::DelimitedReader::from_bytes(only_comma, ht::Dialect::csv(), {settled.value().plan[0]});
  CHECK(settled_read.ok());
  auto settled_batch = settled_read.value().read();
  CHECK(settled_batch.ok() && settled_batch.value()->get<double>(0, 0).value() == 0.125 &&
        settled_batch.value()->get<double>(0, 1).value() == 1234.5);
  const auto conflict = ht::DelimitedReader::suggest_bytes("n\n\"2,5\"\n2.5\n", ht::Dialect::csv());
  CHECK(conflict.ok() && conflict.value().report[0].status == ht::SuggestStatus::unresolved &&
        conflict.value().report[0].matched == 1);
  // `1.234` is the same question as `1,234`, a zero's group isn't: 0.125 is settled either way.
  const auto point = ht::DelimitedReader::suggest_bytes("n,m\n1.234,0.125\n", ht::Dialect::csv());
  CHECK(point.ok() && point.value().report[0].status == ht::SuggestStatus::ambiguous &&
        point.value().report[0].candidates.size() == 2 &&
        point.value().report[1].status == ht::SuggestStatus::resolved &&
        point.value().report[1].format == hc::NumFormat::invariant());

  CHECK(ht::DelimitedReader::suggest("/no/such/file.csv", ht::Dialect::csv()).error().kind() ==
        ht::ErrorKind::io);

  // A sheet's, a read of its own: the one in hand stays where it was.
  auto book = ht::Workbook::open((corpus + "/workbook/basic.xlsx").c_str());
  CHECK(book.ok());
  auto sheet = book.value().sheet_unbound(std::string_view("Data"));
  CHECK(sheet.ok());
  const auto answer = sheet.value().suggest();
  CHECK(answer.ok() && answer.value().report.size() == 9 &&
        answer.value().report[0].name == std::optional<std::string>("id") &&
        answer.value().report[0].door != ht::Door::text);
  CHECK(sheet.value().bind({ht::Column::i64(0)}).ok());
  auto rows = sheet.value().read();
  CHECK(rows.ok() && rows.value()->get<std::int64_t>(0, 0).value() == 1);

  // With no header the width is the first row's, and nothing has a name.
  auto headless =
      book.value().sheet_unbound(std::string_view("Data"), ht::SheetOptions{}.with_header(false));
  CHECK(headless.ok());
  const auto nameless = headless.value().suggest();
  CHECK(nameless.ok() && nameless.value().report.size() == 9 && !nameless.value().report[0].name);
  return 0;
}

static int ranges(const std::string &corpus) {
  const auto parsed = ht::Range::parse("B2:C30");
  CHECK(parsed.ok() && parsed.value().first_row() == 2 && parsed.value().first_column() == 2 &&
        parsed.value().last_row() == 30 && parsed.value().last_column() == 3);
  const auto lower = ht::Range::parse("a5:f200");
  CHECK(!lower.ok() && lower.error().kind() == ht::ErrorKind::range &&
        lower.error().message() == "not a range: \"a5:f200\"");
  CHECK(ht::Range::of(5, 0, 0, 0).ok()); // every row from the fifth
  const auto inverted = ht::Range::of(5, 1, 2, 1);
  CHECK(!inverted.ok() && inverted.error().kind() == ht::ErrorKind::range &&
        inverted.error().message() == "not a range: \"rows 5 to 2, columns 1 to 1\"");

  auto opened = ht::Workbook::open((corpus + "/workbook/basic.xlsx").c_str());
  CHECK(opened.ok());
  const ht::Workbook &book = opened.value();
  // A plan's ordinal 0 is the range's first column; a row's line and a problem's source column
  // are the sheet's own.
  auto sheet = book.sheet(
      std::string_view("Data"),
      ht::SheetOptions{}.with_header(false).with_range(ht::Range::parse("B2:B2").value()),
      {ht::Column::i32(0)});
  CHECK(sheet.ok());
  auto read = sheet.value().read();
  CHECK(read.ok() && read.value()->rows() == 1 && read.value()->line(0) == 2);
  CHECK(read.value()->raw(0, 0) == "alice");
  ht::Faults faults = read.value()->faults();
  auto problem = faults.begin();
  CHECK(problem != faults.end() && (*problem).plan_column == 0u && (*problem).source_column == 1u &&
        (*problem).line == 2);

  // A range bounds the header too: its last column is the width.
  auto bounded = book.sheet_unbound(
      std::string_view("Data"), ht::SheetOptions{}.with_range(ht::Range::parse("A1:B3").value()));
  CHECK(bounded.ok() && bounded.value().header() != nullptr &&
        bounded.value().header()->size() == 2);

  // Skip, a discarded row and tokens, as a delimited read has them.
  auto skipping = book.sheet(std::string_view("Data"),
                             ht::SheetOptions{}
                                 .with_skip(1)
                                 .with_header_row(1)
                                 .with_tokens({"bob, jr"})
                                 .with_header(false),
                             {ht::Column::text(1)});
  CHECK(skipping.ok());
  auto rows = skipping.value().read();
  CHECK(rows.ok() && rows.value()->origin(0, 0) == ht::Origin::token);
  return 0;
}

// Reads a file whole.
static std::vector<std::uint8_t> bytes_of(const std::string &path) {
  std::ifstream file(path, std::ios::binary);
  return std::vector<std::uint8_t>(std::istreambuf_iterator<char>(file),
                                   std::istreambuf_iterator<char>());
}

// The 1.0 entry points, which every read here went past, keep their names, arguments and layouts: a
// program that calls them reads what a read does, with what HyperCast answers on a shape today.
static int one_dot_zero(const std::string &corpus) {
  namespace core = ht::detail;
  // Delimited text.
  const std::string_view csv = "id,v\n1,2\n3,x\n";
  const auto *input = reinterpret_cast<const std::uint8_t *>(csv.data());
  HyperTabularRawNumFormat invariant{};
  CHECK(core::raw_format(hc::NumFormat::invariant(), invariant));
  std::uint64_t block[HyperTabularDelimitedState_SIZE / 8];
  auto *state = reinterpret_cast<HyperTabularDelimitedState *>(block);
  const HyperTabularRawDialect dialect{',', 1, 1, 0};
  CHECK(core::init_1_0(state, &dialect) == HYPERTABULAR_OK);
  HyperTabularSpan names[4];
  std::uint8_t arena[64];
  HyperTabularFilled out{};
  CHECK(::hypertabular_delimited_header(state, input, csv.size(), 1, names, 4, arena, sizeof arena,
                                        &out) == HYPERTABULAR_OK);
  const auto at = static_cast<std::size_t>(out.consumed);
  HyperTabularColumnSpec specs[2] = {};
  std::int32_t values[2][4];
  HyperTabularCellVerdict verdicts[2][4];
  HyperTabularColumnBuffer columns[2] = {};
  for (std::uint32_t i = 0; i < 2; ++i) {
    specs[i].ordinal = i;
    specs[i].door = static_cast<std::uint32_t>(ht::Door::i32);
    specs[i].format = invariant;
    columns[i].values = values[i];
    columns[i].verdicts = verdicts[i];
  }
  HyperTabularSpan cells[32];
  HyperTabularBuffers memory{};
  memory.arena = arena;
  memory.arena_cap = sizeof arena;
  memory.cells = cells;
  memory.cells_cap = 32;
  CHECK(core::fill_1_0(state, input + at, csv.size() - at, true, specs, columns, 2, 4, &memory,
                       &out) == HYPERTABULAR_OK);
  auto reader = ht::DelimitedReader::from_bytes(csv, ht::Dialect::csv(),
                                                {ht::Column::i32(0), ht::Column::i32(1)});
  CHECK(reader.ok());
  auto read = reader.value().read();
  CHECK(read.ok() && read.value()->rows() == out.rows);
  for (std::size_t column = 0; column < 2; ++column) {
    for (std::size_t row = 0; row < 2; ++row) {
      const hc::Verdict<std::int32_t> cell = read.value()->get<std::int32_t>(column, row);
      CHECK(cell.ok() == (verdicts[column][row].reason == 0));
      CHECK(!cell.ok() || cell.value() == values[column][row]);
    }
  }

  // The one difference, kept: a 1.0 fill doesn't read a format's grouping, so it refuses what a
  // read, through the size-tagged fill, takes under the Indian format it was declared.
  const std::string_view grouped = "\"1,23,456\"\n";
  const hc::NumFormat indian = hc::NumFormat::invariant().with_grouping(hc::Grouping::indian);
  HyperTabularRawDialect headerless{',', 1, 1, 0};
  CHECK(core::init_1_0(state, &headerless) == HYPERTABULAR_OK);
  specs[0].ordinal = 0;
  std::memcpy(&specs[0].format, &indian.raw(), sizeof specs[0].format);
  CHECK(core::fill_1_0(state, reinterpret_cast<const std::uint8_t *>(grouped.data()),
                       grouped.size(), true, specs, columns, 1, 4, &memory,
                       &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 1 && verdicts[0][0].reason == 2);
  auto declared = ht::DelimitedReader::from_bytes(grouped, ht::Dialect::csv().with_header(false),
                                                  {ht::Column::i32(0).format(indian)});
  CHECK(declared.ok());
  auto taken = declared.value().read();
  CHECK(taken.ok() && taken.value()->get<std::int32_t>(0, 0).value() == 123456);
  // A column given no format is Western, the same as NumFormat::invariant() declared.
  for (const ht::Column &western :
       {ht::Column::i32(0), ht::Column::i32(0).format(hc::NumFormat::invariant())}) {
    auto undeclared =
        ht::DelimitedReader::from_bytes(grouped, ht::Dialect::csv().with_header(false), {western});
    CHECK(undeclared.ok());
    auto refused = undeclared.value().read();
    CHECK(refused.ok() && refused.value()->fault(0, 0)->reason == hc::CastFailure::malformed);
  }
  // And `12,34` is refused by every policy, and by the 1.0 fill.
  const std::string_view short_group = "\"12,34\"\n";
  CHECK(core::init_1_0(state, &headerless) == HYPERTABULAR_OK);
  CHECK(core::fill_1_0(state, reinterpret_cast<const std::uint8_t *>(short_group.data()),
                       short_group.size(), true, specs, columns, 1, 4, &memory,
                       &out) == HYPERTABULAR_OK);
  CHECK(out.rows == 1 && verdicts[0][0].reason != 0);

  // A sheet. What it opens with is the same exports a read opens with.
  const std::vector<std::uint8_t> file = bytes_of(corpus + "/workbook/basic.xlsx");
  std::vector<std::uint64_t> book_state((::hypertabular_workbook_state_size() + 7) / 8);
  auto *book_block = reinterpret_cast<HyperTabularWorkbookState *>(book_state.data());
  std::vector<std::uint8_t> window(1 << 20);
  std::vector<std::uint8_t> space(1 << 20);
  std::vector<HyperTabularSpan> table(1 << 14);
  std::vector<HyperTabularSlot> slots(64);
  HyperTabularBuffers buffers{};
  buffers.window = window.data();
  buffers.window_cap = window.size();
  buffers.arena = space.data();
  buffers.arena_cap = space.size();
  buffers.cells = table.data();
  buffers.cells_cap = table.size();
  buffers.row = slots.data();
  buffers.row_cap = slots.size();
  HyperTabularOpened opening{};
  CHECK(::hypertabular_workbook_open(book_block, file.data(), file.size(), &buffers, &opening) ==
        HYPERTABULAR_OK);
  CHECK(::hypertabular_workbook_sheets(book_block, file.data(), file.size(), &buffers, &out) ==
        HYPERTABULAR_OK);
  HyperTabularSheetInfo info{};
  std::memcpy(&info, table.data(), sizeof info);
  const std::string part(reinterpret_cast<const char *>(space.data()) + info.part.offset,
                         info.part.len & ~static_cast<std::uint32_t>(HyperTabularSpan_FLAG));
  CHECK(::hypertabular_workbook_strings(book_block, file.data(), file.size(), &buffers, &out) ==
        HYPERTABULAR_OK);
  const std::vector<std::uint8_t> strings(
      space.begin(), space.begin() + static_cast<std::ptrdiff_t>(out.arena_used));
  const std::vector<HyperTabularSpan> shared(table.begin(),
                                             table.begin() + static_cast<std::ptrdiff_t>(out.rows));
  CHECK(::hypertabular_workbook_styles(book_block, file.data(), file.size(), &buffers, &out) ==
        HYPERTABULAR_OK);
  const std::vector<std::uint8_t> kinds(space.begin(),
                                        space.begin() + static_cast<std::ptrdiff_t>(out.rows));
  buffers.strings = strings.data();
  buffers.strings_len = strings.size();
  buffers.table = shared.data();
  buffers.table_len = shared.size();
  buffers.kinds = kinds.data();
  buffers.kinds_len = kinds.size();
  CHECK(core::sheet_1_0(book_block, file.data(), file.size(),
                        reinterpret_cast<const std::uint8_t *>(part.data()), part.size(),
                        info.index, true, true, &out) == HYPERTABULAR_OK);
  CHECK(::hypertabular_workbook_header(book_block, file.data(), file.size(), &buffers, &out) ==
        HYPERTABULAR_OK);
  HyperTabularColumnSpec id{};
  id.door = static_cast<std::uint32_t>(ht::Door::i64);
  id.format = invariant;
  std::int64_t ids[8];
  HyperTabularCellVerdict id_verdicts[8];
  const HyperTabularColumnBuffer id_column{ids, id_verdicts};
  CHECK(core::book_fill_1_0(book_block, file.data(), file.size(), &id, &id_column, 1, 8, &buffers,
                            &out) == HYPERTABULAR_OK);
  auto book = ht::Workbook::from_vector(file);
  CHECK(book.ok());
  auto sheet = book.value().sheet(std::size_t{0}, {}, {ht::Column::i64(0)});
  CHECK(sheet.ok());
  auto rows = sheet.value().read();
  CHECK(rows.ok() && rows.value()->rows() == out.rows && out.rows > 0);
  for (std::size_t row = 0; row < out.rows; ++row) {
    CHECK(rows.value()->get<std::int64_t>(0, row).value() == ids[row]);
  }
  return 0;
}

// A format as it can only arrive from outside the type: a stale word laid over a real one, which
// neither NumFormat::declare nor with_grouping would make.
hc::NumFormat worded(std::uint32_t flags) {
  hc::NumFormat format = hc::NumFormat::invariant();
  HyperCastRawNumFormat raw = format.raw();
  raw.format.flags = flags;
  std::memcpy(static_cast<void *>(&format), &raw, sizeof raw);
  return format;
}

// A column's grouping is its own, and a stale policy is refused where it's handed over.
int grouping(const std::string &corpus) {
  const hc::NumFormat indian = hc::NumFormat::invariant().with_grouping(hc::Grouping::indian);
  const std::string_view both =
      "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";
  auto reader = ht::DelimitedReader::from_bytes(
      both, ht::Dialect::csv(), {ht::Column::i32(0).format(indian), ht::Column::i32(1)});
  CHECK(reader.ok());
  auto batch = reader.value().read();
  CHECK(batch.ok() && batch.value()->rows() == 2);
  CHECK(batch.value()->get<std::int32_t>(0, 0).value() == 123456);
  CHECK(batch.value()->fault(1, 0)->reason == hc::CastFailure::malformed);
  CHECK(batch.value()->fault(0, 1)->reason == hc::CastFailure::malformed);
  CHECK(batch.value()->get<std::int32_t>(1, 1).value() == 1234567);

  for (const char *name : {"grouping.xlsx", "grouping.ods", "grouping.numbers"}) {
    auto book = ht::Workbook::open((corpus + "/workbook/" + name).c_str());
    CHECK(book.ok());
    auto sheet = book.value().sheet(0, ht::SheetOptions{},
                                    {ht::Column::i32(2).format(indian), ht::Column::i32(2)});
    CHECK(sheet.ok());
    auto cells = sheet.value().read();
    CHECK(cells.ok() && cells.value()->rows() == 3);
    CHECK(cells.value()->get<std::int32_t>(0, 0).value() == 123456);
    CHECK(cells.value()->fault(1, 0)->reason == hc::CastFailure::malformed);
    CHECK(cells.value()->fault(0, 1)->reason == hc::CastFailure::malformed);
    CHECK(cells.value()->get<std::int32_t>(1, 1).value() == 1234567);
    CHECK(cells.value()->get<std::int32_t>(0, 2).value() == 1234);
    CHECK(cells.value()->get<std::int32_t>(1, 2).value() == 1234);
  }

  // Each word through every door, with 95 and 607 the control: the same call takes the same format
  // without the retired bit.
  const std::string path = corpus + "/workbook/grouping.xlsx";
  for (const std::uint32_t flags : {351u, 863u, 95u, 607u}) {
    const bool stale = (flags & 256u) != 0;
    const hc::NumFormat format = worded(flags);
    CHECK((format.raw().format.flags == flags));
    const std::vector<ht::Column> plan{ht::Column::i32(0).format(format)};
    auto is_refused = [](const auto &opened) {
      return !opened.ok() && opened.error().kind() == ht::ErrorKind::plan;
    };
    auto check = [&](const auto &opened) { return stale ? is_refused(opened) : opened.ok(); };
    CHECK(check(ht::DelimitedReader::from_bytes(both, ht::Dialect::csv(), plan)));
    auto unbound = ht::DelimitedReader::from_bytes(both, ht::Dialect::csv());
    CHECK(unbound.ok());
    const auto bound = unbound.value().bind(plan);
    CHECK(stale != bound.ok());
    if (stale) {
      CHECK(bound.error().kind() == ht::ErrorKind::plan);
    }
    auto book = ht::Workbook::open(path.c_str());
    CHECK(book.ok());
    CHECK(check(book.value().sheet(0, ht::SheetOptions{}, plan)));
    auto sheet = book.value().sheet_unbound(0, ht::SheetOptions{});
    CHECK(sheet.ok());
    const auto sheet_bound = sheet.value().bind(plan);
    CHECK(stale != sheet_bound.ok());

    ht::SuggestOptions everywhere;
    everywhere.format = format;
    const ht::SuggestOptions one = ht::SuggestOptions{}.with_column_format(0, format);
    for (const ht::SuggestOptions &asked : {everywhere, one}) {
      CHECK(check(ht::DelimitedReader::suggest_bytes(both, ht::Dialect::csv(), asked)));
      auto unread = book.value().sheet_unbound(0, ht::SheetOptions{});
      CHECK(unread.ok());
      CHECK(check(unread.value().suggest(asked)));
    }
  }

  // A suggest judges every format it's given before it opens anything: one never met, one replaced,
  // one past the sample, an empty source and a broken one. A word without the retired bit is the
  // control, which gets as far as the source does.
  const ht::Dialect headerless = ht::Dialect::csv().with_header(false);
  const std::string broken_book = corpus + "/workbook/broken.xlsx";
  for (const std::uint32_t flags : {351u, 863u, 95u, 607u}) {
    const bool stale = (flags & 256u) != 0;
    const hc::NumFormat format = worded(flags);
    ht::SuggestOptions every;
    every.format = format;
    const ht::SuggestOptions past = ht::SuggestOptions{}.with_column_format(99, format);
    const ht::SuggestOptions replaced = every.with_column_format(0, indian);
    const ht::SuggestOptions twice =
        ht::SuggestOptions{}.with_column_format(0, format).with_column_format(0, indian);
    auto refused = [](const auto &found, std::size_t column) {
      return !found.ok() && found.error().kind() == ht::ErrorKind::plan &&
             found.error().column() == column;
    };
    auto sound = [&](const auto &found, std::size_t column) {
      return stale ? refused(found, column) : found.ok();
    };
    auto of_the_source = [&](const auto &found, ht::ErrorKind kind) {
      return stale ? refused(found, 0) : (!found.ok() && found.error().kind() == kind);
    };
    const ht::Dialect csv = ht::Dialect::csv();
    CHECK(sound(ht::DelimitedReader::suggest_bytes("a\n1\n", csv, every), 0));
    CHECK(sound(ht::DelimitedReader::suggest_bytes("", csv, every), 0));
    CHECK(sound(ht::DelimitedReader::suggest_bytes("", headerless, every), 0));
    CHECK(sound(ht::DelimitedReader::suggest_bytes("a\n1\n", csv, past), 99));
    CHECK(sound(ht::DelimitedReader::suggest_bytes("a\n1\n", csv, replaced), 0));
    CHECK(sound(ht::DelimitedReader::suggest_bytes("a\n1\n", csv, twice), 0));
    CHECK(of_the_source(ht::DelimitedReader::suggest_bytes("a\n\"1\n", csv, every),
                        ht::ErrorKind::structure));
    CHECK(of_the_source(ht::DelimitedReader::suggest_bytes("\"1\n", headerless, every),
                        ht::ErrorKind::structure));
    CHECK(of_the_source(ht::DelimitedReader::suggest("no-such-file.csv", csv, every),
                        ht::ErrorKind::io));

    auto grouped = ht::Workbook::open(path.c_str());
    CHECK(grouped.ok());
    auto sheet = grouped.value().sheet_unbound(0, ht::SheetOptions{});
    CHECK(sheet.ok());
    CHECK(sound(sheet.value().suggest(every), 0));
    CHECK(sound(sheet.value().suggest(past), 99));
    CHECK(sound(sheet.value().suggest(replaced), 0));
    CHECK(sound(sheet.value().suggest(twice), 0));
    auto broken = ht::Workbook::open(broken_book.c_str());
    CHECK(broken.ok());
    auto damaged = broken.value().sheet_unbound(0, ht::SheetOptions{});
    CHECK(damaged.ok());
    CHECK(of_the_source(damaged.value().suggest(every), ht::ErrorKind::structure));
  }

  // Undeclared, an Indian shape has no culture. Declared, it's a number, and a Western shape isn't.
  const auto report = [](std::string_view text, const ht::SuggestOptions &asked) {
    auto found = ht::DelimitedReader::suggest_bytes(text, ht::Dialect::csv(), asked);
    return std::move(found).value().report.at(0);
  };
  const std::string_view shape = "a\n\"1,23,456\"\n";
  CHECK(report(shape, {}).door == ht::Door::text);
  CHECK(report(shape, {}).status == ht::SuggestStatus::resolved);
  CHECK(report("a\n\"1,23,456\"\n5\n", {}).status == ht::SuggestStatus::unresolved);
  CHECK(report("a\n\"1,234,567\"\n", {}).format->raw().format.flags == 95);
  const ht::SuggestOptions declared = ht::SuggestOptions{}.with_column_format(0, indian);
  CHECK(report(shape, declared).door == ht::Door::i32);
  CHECK(report(shape, declared).status == ht::SuggestStatus::resolved);
  CHECK(report(shape, declared).format->raw().format.flags == 607);
  CHECK(report("a\n\"1,234,567\"\n", declared).door == ht::Door::text);
  return 0;
}

// Columns that declare HyperCast's constraints: what the corpus replay doesn't reach.
static int constraints() {
  // The declaration's enum list lives in the caller's storage, which Column::checked copies: the
  // storage is scribbled on before the read, and the match is still the entry as declared.
  std::uint8_t storage[32] = {};
  const auto sex = hc::Constraints<std::string_view>::declare({}, {"Female", "Male"}, storage);
  hc::Constraints<std::int32_t>::Fields bound;
  bound.maximum = 10;
  const auto at_most = hc::Constraints<std::int32_t>::declare(bound);
  CHECK(sex.value_if() != nullptr && at_most.value_if() != nullptr);
  const ht::Column text = ht::Column::checked(1, *sex.value_if());
  // The numeric notation and tokens are set as on any column, and keep the declaration.
  const ht::Column number = ht::Column::checked(0, *at_most.value_if()).tokens({"NA"});
  std::fill(std::begin(storage), std::end(storage), std::uint8_t{'?'});
  CHECK(text.is_checked() && text.door() == ht::Door::text);
  CHECK(number.is_checked() && number.door() == ht::Door::i32);
  CHECK(!ht::Column::i32(0).is_checked());

  auto opened =
      ht::DelimitedReader::from_bytes("n,s,m\n7,MALE,7\n11,other,x\nNA,Female,3\n",
                                      ht::Dialect::csv(), {number, text, ht::Column::i32(2)});
  CHECK(opened.ok());
  auto read = opened.value().read();
  CHECK(read.ok() && read.value());
  const ht::Batch &batch = *read.value();

  const auto seven = batch.get_checked<std::int32_t>(0, 0);
  CHECK(seven.ok() && seven.value().value == 7 && !seven.value().adjusted &&
        !seven.value().enum_index);
  const auto male = batch.get_checked<std::string_view>(1, 0);
  CHECK(male.ok() && male.value().value == "Male" && male.value().enum_index == 1u);
  CHECK(batch.raw(1, 0) == "MALE"); // the text as read

  // A violation keeps the value as cast, its span the cell's; it's no fault.
  const auto eleven = batch.get_checked<std::int32_t>(0, 1);
  const hc::Violation<std::int32_t> *broken =
      eleven.ok() ? nullptr : eleven.rejection().violation_if();
  CHECK(broken != nullptr && broken->keyword == ht::Keyword::maximum && broken->value == 11 &&
        broken->offset == 0 && broken->length == 2);
  CHECK(!batch.fault(0, 1).has_value());
  const auto other = batch.get_checked<std::string_view>(1, 1);
  CHECK(!other.ok() && !other.rejection().is_fault() &&
        other.rejection().violation().keyword == ht::Keyword::enum_ &&
        other.rejection().violation().value == "other");
  // A token is Empty with no door asked, and a fault is the fault get() would give.
  const auto token = batch.get_checked<std::int32_t>(0, 2);
  CHECK(!token.ok() && token.rejection().is_fault() &&
        token.rejection().fault().reason == hc::CastFailure::empty &&
        batch.origin(0, 2) == ht::Origin::token);

  // A column that declares none reads through get_checked() as through get(): passed, unadjusted.
  const auto plain = batch.get_checked<std::int32_t>(2, 0);
  CHECK(plain.ok() && plain.value().value == 7 && !plain.value().adjusted &&
        !plain.value().enum_index);
  const auto refused = batch.get_checked<std::int32_t>(2, 1);
  CHECK(!refused.ok() && refused.rejection().is_fault() &&
        refused.rejection().fault() == *batch.fault(2, 1));
  CHECK(batch.row(1).get_checked<std::int32_t>(0) == eleven);

  // Faults list a violation by its keyword, with no reason.
  std::vector<ht::Problem> problems;
  ht::Faults faults = batch.faults();
  for (const ht::Problem &problem : faults) {
    problems.push_back(problem);
  }
  CHECK(problems.size() == 3);
  CHECK(problems[0].plan_column == std::optional<std::size_t>(0) &&
        problems[0].keyword == ht::Keyword::maximum && !problems[0].reason &&
        problems[0].offset == 0 && problems[0].length == 2);
  CHECK(problems[1].plan_column == std::optional<std::size_t>(1) &&
        problems[1].keyword == ht::Keyword::enum_ && !problems[1].reason);
  CHECK(problems[2].plan_column == std::optional<std::size_t>(2) && !problems[2].keyword &&
        problems[2].reason == hc::CastFailure::malformed);

  // The checks are sized like the verdicts, across every batch.
  std::string many;
  for (int i = 0; i < 37; ++i) {
    many += "11\n";
  }
  ht::DelimitedOptions small;
  small.batch_rows = 5;
  auto reader =
      ht::DelimitedReader::from_bytes(many, ht::Dialect::csv().with_header(false),
                                      {ht::Column::checked(0, *at_most.value_if())}, small);
  CHECK(reader.ok());
  std::size_t violated = 0;
  for (;;) {
    auto next = reader.value().read();
    CHECK(next.ok());
    if (!next.value()) {
      break;
    }
    CHECK(next.value()->rows() <= 5);
    for (const ht::Row &row : *next.value()) {
      const auto cell = row.get_checked<std::int32_t>(0);
      violated += !cell.ok() && !cell.rejection().is_fault() &&
                  cell.rejection().violation().keyword == ht::Keyword::maximum;
    }
  }
  CHECK(violated == 37);
  return 0;
}

int main(int argc, char **argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: %s <corpus directory>\n", argv[0]);
    return 2;
  }
  if (delimited() != 0 || workbook(argv[1]) != 0 || options() != 0 || sizes(argv[1]) != 0 ||
      faults_keep_the_entry_in_hand() != 0 || reentry(argv[1]) != 0 || encodings() != 0 ||
      suggestions(argv[1]) != 0 || ranges(argv[1]) != 0 || one_dot_zero(argv[1]) != 0 ||
      grouping(argv[1]) != 0 || constraints() != 0) {
    return 1;
  }
  const std::string_view version = ht::native_version();
  std::printf("%.*s\n", static_cast<int>(version.size()), version.data());
  return 0;
}
