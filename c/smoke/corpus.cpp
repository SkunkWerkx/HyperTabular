// Replays the conformance corpus (corpus/delimited.json and corpus/workbook.json at the
// repository root, the files every binding's suite replays) through hypertabular.hpp: each
// case's header, its rows' row numbers, every cell's verdict, value, fault span and text, and
// its structural failure, byte for byte. A case that fails is a break in the polyglot contract.
//
// Delimited text is read from memory, from a file and from a source that hands over a byte at
// a time, in batches of one row, of two and of more than any case has, each with the plan given
// and bound after the header. A workbook is read from bytes in place, from bytes it keeps and
// from its path, by index and by name, likewise. Everything runs twice: with the buffers'
// usual starting sizes, then with every buffer one element long and nothing presized, so each
// grow-and-retry path of the core runs on every call.
//
// usage: smoke_corpus <corpus directory>; prints the library's version when every case passes,
// as the other smoke programs do. run.sh runs it against each build of the archive.

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <optional>
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <vector>

#include "hypertabular.hpp"

namespace ht = hypertabular;
namespace hc = hypercast;

namespace {

// Just enough JSON for the corpus: a number keeps its text, so a u64 or a double is read
// exactly by the one that needs it.
struct Json {
  enum class Kind { null, boolean, number, string, array, object } kind = Kind::null;
  bool boolean = false;
  std::string text;
  std::vector<Json> items;
  std::vector<std::pair<std::string, Json>> members;

  const Json *find(std::string_view key) const {
    for (const auto &member : members) {
      if (member.first == key) {
        return &member.second;
      }
    }
    return nullptr;
  }
  std::int64_t i64() const { return std::strtoll(text.c_str(), nullptr, 10); }
  std::uint64_t u64() const { return std::strtoull(text.c_str(), nullptr, 10); }
  double f64() const { return std::strtod(text.c_str(), nullptr); }
};

class Parser {
public:
  explicit Parser(std::string_view source) : source_(source) {}

  bool parse(Json &out) {
    if (!value(out)) {
      return false;
    }
    space();
    return at_ == source_.size();
  }

private:
  void space() {
    while (at_ < source_.size() && (source_[at_] == ' ' || source_[at_] == '\n' ||
                                    source_[at_] == '\r' || source_[at_] == '\t')) {
      ++at_;
    }
  }
  bool literal(std::string_view word) {
    if (source_.substr(at_, word.size()) != word) {
      return false;
    }
    at_ += word.size();
    return true;
  }
  static void utf8(std::string &out, std::uint32_t code) {
    if (code < 0x80) {
      out += static_cast<char>(code);
    } else if (code < 0x800) {
      out += static_cast<char>(0xC0 | code >> 6);
      out += static_cast<char>(0x80 | (code & 0x3F));
    } else if (code < 0x10000) {
      out += static_cast<char>(0xE0 | code >> 12);
      out += static_cast<char>(0x80 | (code >> 6 & 0x3F));
      out += static_cast<char>(0x80 | (code & 0x3F));
    } else {
      out += static_cast<char>(0xF0 | code >> 18);
      out += static_cast<char>(0x80 | (code >> 12 & 0x3F));
      out += static_cast<char>(0x80 | (code >> 6 & 0x3F));
      out += static_cast<char>(0x80 | (code & 0x3F));
    }
  }
  bool hex4(std::uint32_t &code) {
    if (at_ + 4 > source_.size()) {
      return false;
    }
    code = 0;
    for (int i = 0; i < 4; ++i) {
      const char c = source_[at_++];
      code <<= 4;
      if (c >= '0' && c <= '9') {
        code |= static_cast<std::uint32_t>(c - '0');
      } else if (c >= 'a' && c <= 'f') {
        code |= static_cast<std::uint32_t>(c - 'a' + 10);
      } else if (c >= 'A' && c <= 'F') {
        code |= static_cast<std::uint32_t>(c - 'A' + 10);
      } else {
        return false;
      }
    }
    return true;
  }
  bool string(std::string &out) {
    if (!literal("\"")) {
      return false;
    }
    while (at_ < source_.size()) {
      const char c = source_[at_++];
      if (c == '"') {
        return true;
      }
      if (c != '\\') {
        out += c;
        continue;
      }
      if (at_ >= source_.size()) {
        return false;
      }
      switch (source_[at_++]) {
      case '"':
        out += '"';
        break;
      case '\\':
        out += '\\';
        break;
      case '/':
        out += '/';
        break;
      case 'b':
        out += '\b';
        break;
      case 'f':
        out += '\f';
        break;
      case 'n':
        out += '\n';
        break;
      case 'r':
        out += '\r';
        break;
      case 't':
        out += '\t';
        break;
      case 'u': {
        std::uint32_t code = 0;
        if (!hex4(code)) {
          return false;
        }
        if (code >= 0xD800 && code < 0xDC00) {
          std::uint32_t low = 0;
          if (!literal("\\u") || !hex4(low) || low < 0xDC00 || low >= 0xE000) {
            return false;
          }
          code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
        }
        utf8(out, code);
        break;
      }
      default:
        return false;
      }
    }
    return false;
  }
  bool value(Json &out) {
    space();
    if (at_ >= source_.size()) {
      return false;
    }
    const char c = source_[at_];
    if (c == '{') {
      ++at_;
      out.kind = Json::Kind::object;
      space();
      if (literal("}")) {
        return true;
      }
      do {
        space();
        std::string key;
        Json member;
        if (!string(key)) {
          return false;
        }
        space();
        if (!literal(":") || !value(member)) {
          return false;
        }
        out.members.emplace_back(std::move(key), std::move(member));
        space();
      } while (literal(","));
      return literal("}");
    }
    if (c == '[') {
      ++at_;
      out.kind = Json::Kind::array;
      space();
      if (literal("]")) {
        return true;
      }
      do {
        Json item;
        if (!value(item)) {
          return false;
        }
        out.items.push_back(std::move(item));
        space();
      } while (literal(","));
      return literal("]");
    }
    if (c == '"') {
      out.kind = Json::Kind::string;
      return string(out.text);
    }
    if (literal("true")) {
      out.kind = Json::Kind::boolean;
      out.boolean = true;
      return true;
    }
    if (literal("false")) {
      out.kind = Json::Kind::boolean;
      return true;
    }
    if (literal("null")) {
      return true;
    }
    const std::size_t start = at_;
    while (at_ < source_.size() && std::strchr("+-0123456789.eE", source_[at_]) != nullptr) {
      ++at_;
    }
    out.kind = Json::Kind::number;
    out.text = std::string(source_.substr(start, at_ - start));
    return at_ > start;
  }

  std::string_view source_;
  std::size_t at_ = 0;
};

int failures = 0;
int cases = 0;
std::string context; // the case and the way it is being read, for a failure's message

bool failed(const std::string &what) {
  if (++failures <= 20) {
    std::fprintf(stderr, "corpus: %s: %s\n", context.c_str(), what.c_str());
  }
  return false;
}

const Json &member(const Json &object, std::string_view key) {
  static const Json null;
  const Json *found = object.find(key);
  return found != nullptr ? *found : null;
}

const char *reason_name(hc::CastFailure reason) {
  switch (reason) {
  case hc::CastFailure::empty:
    return "empty";
  case hc::CastFailure::malformed:
    return "malformed";
  case hc::CastFailure::out_of_range:
    return "out_of_range";
  }
  return "?";
}

// A plan entry as a column: its door, what the door declares, and its numeric format.
std::optional<ht::Column> column_of(const Json &entry) {
  const std::size_t ordinal = member(entry, "ordinal").u64();
  const std::string &door = member(entry, "door").text;
  const auto param = static_cast<std::uint32_t>(member(entry, door == "unix"           ? "precision"
                                                              : door == "excel_serial" ? "epoch"
                                                                                       : "order")
                                                    .u64());
  std::optional<ht::Column> column;
  if (door == "bool") {
    column = ht::Column::boolean(ordinal);
  } else if (door == "i8") {
    column = ht::Column::i8(ordinal);
  } else if (door == "i16") {
    column = ht::Column::i16(ordinal);
  } else if (door == "i32") {
    column = ht::Column::i32(ordinal);
  } else if (door == "i64") {
    column = ht::Column::i64(ordinal);
  } else if (door == "u8") {
    column = ht::Column::u8(ordinal);
  } else if (door == "u16") {
    column = ht::Column::u16(ordinal);
  } else if (door == "u32") {
    column = ht::Column::u32(ordinal);
  } else if (door == "u64") {
    column = ht::Column::u64(ordinal);
  } else if (door == "f32") {
    column = ht::Column::f32(ordinal);
  } else if (door == "f64") {
    column = ht::Column::f64(ordinal);
  } else if (door == "decimal") {
    column = ht::Column::decimal(ordinal);
  } else if (door == "uuid") {
    column = ht::Column::uuid(ordinal);
  } else if (door == "timestamp") {
    column = ht::Column::timestamp(ordinal);
  } else if (door == "unix") {
    column = ht::Column::unix_time(ordinal, static_cast<hc::UnixPrecision>(param));
  } else if (door == "excel_serial") {
    column = ht::Column::excel_serial(ordinal, static_cast<hc::ExcelEpoch>(param));
  } else if (door == "date") {
    column = ht::Column::date(ordinal);
  } else if (door == "date_ordered") {
    column = ht::Column::date_ordered(ordinal, static_cast<hc::DateOrder>(param));
  } else if (door == "datetime") {
    column = ht::Column::datetime(ordinal, static_cast<hc::DateOrder>(param));
  } else if (door == "time") {
    column = ht::Column::time(ordinal);
  } else if (door == "duration") {
    column = ht::Column::duration(ordinal);
  } else if (door == "text") {
    column = ht::Column::text(ordinal);
  } else {
    return std::nullopt;
  }
  if (const Json *format = entry.find("format")) {
    hc::NumFormat::Fields fields;
    fields.decimal_sep = member(*format, "decimal_sep").text;
    fields.group_sep = member(*format, "group_sep").text;
    fields.styles = static_cast<hc::NumStyles>(member(*format, "flags").u64());
    fields.currency = member(*format, "currency").text;
    const auto declared = hc::NumFormat::declare(fields);
    if (!declared) {
      return std::nullopt;
    }
    column = column->format(*declared);
  }
  return column;
}

std::optional<std::vector<ht::Column>> plan_of(const Json &plan) {
  std::vector<ht::Column> columns;
  for (const Json &entry : plan.items) {
    auto column = column_of(entry);
    if (!column) {
      return std::nullopt;
    }
    columns.push_back(*column);
  }
  return columns;
}

// The magnitude of a decimal, a 96-bit integer, in decimal digits.
std::string magnitude(const hc::Decimal &value) {
  std::uint32_t words[3] = {static_cast<std::uint32_t>(value.lo),
                            static_cast<std::uint32_t>(value.lo >> 32), value.hi};
  std::string digits;
  do {
    std::uint64_t remainder = 0;
    for (int i = 2; i >= 0; --i) {
      const std::uint64_t part = remainder << 32 | words[i];
      words[i] = static_cast<std::uint32_t>(part / 10);
      remainder = part % 10;
    }
    digits.insert(digits.begin(), static_cast<char>('0' + remainder));
  } while (words[0] != 0 || words[1] != 0 || words[2] != 0);
  return digits;
}

bool same_number(const Json &expected, double actual) {
  return expected.kind == Json::Kind::number && expected.f64() == actual;
}

// One cell against the corpus's verdict for it.
bool check_cell(const ht::Batch &batch, std::size_t column, std::size_t row, const Json &want) {
  const std::string &expect = member(want, "expect").text;
  const std::string where = "row " + std::to_string(row) + ", column " + std::to_string(column);
  if (const auto fault = batch.fault(column, row)) {
    if (expect != reason_name(fault->reason)) {
      return failed(where + ": " + reason_name(fault->reason) + ", expected " + expect);
    }
    if (fault->reason == hc::CastFailure::empty) {
      return true;
    }
    const Json &span = member(want, "fault");
    if (span.items.size() != 2 || span.items[0].u64() != fault->offset ||
        span.items[1].u64() != fault->length) {
      return failed(where + ": the fault's span");
    }
    const std::string raw = batch.raw(column, row);
    if (raw != member(want, "raw").text) {
      return failed(where + ": raw text \"" + raw + "\"");
    }
    if (fault->offset + fault->length > raw.size()) {
      return failed(where + ": the fault's span is outside the cell");
    }
    return true;
  }
  if (expect != "ok") {
    return failed(where + ": cast, expected " + expect);
  }
  const Json &value = member(want, "value");
  bool same = false;
  switch (batch.plan()[column].door()) {
  case ht::Door::boolean:
    same =
        value.kind == Json::Kind::boolean && value.boolean == batch.get<bool>(column, row).value();
    break;
  case ht::Door::i8:
    same = value.i64() == batch.get<std::int8_t>(column, row).value();
    break;
  case ht::Door::i16:
    same = value.i64() == batch.get<std::int16_t>(column, row).value();
    break;
  case ht::Door::i32:
    same = value.i64() == batch.get<std::int32_t>(column, row).value();
    break;
  case ht::Door::i64:
    same = value.i64() == batch.get<std::int64_t>(column, row).value();
    break;
  case ht::Door::u8:
    same = value.u64() == batch.get<std::uint8_t>(column, row).value();
    break;
  case ht::Door::u16:
    same = value.u64() == batch.get<std::uint16_t>(column, row).value();
    break;
  case ht::Door::u32:
    same = value.u64() == batch.get<std::uint32_t>(column, row).value();
    break;
  case ht::Door::u64:
    same = value.u64() == batch.get<std::uint64_t>(column, row).value();
    break;
  case ht::Door::f32:
    same = same_number(value, static_cast<double>(batch.get<float>(column, row).value()));
    break;
  case ht::Door::f64:
    same = same_number(value, batch.get<double>(column, row).value());
    break;
  case ht::Door::decimal: {
    const hc::Decimal decimal = batch.get<hc::Decimal>(column, row).value();
    char text[hc::Decimal::max_chars];
    const auto written = decimal.to_chars(text, text + sizeof text);
    same = value.text == std::string(text, written.ptr) &&
           member(want, "magnitude").text == magnitude(decimal) &&
           member(want, "scale").u64() == decimal.scale &&
           member(want, "negative").boolean == decimal.negative;
    break;
  }
  case ht::Door::uuid: {
    static const char hex[] = "0123456789abcdef";
    std::string text;
    const hc::Uuid uuid = batch.get<hc::Uuid>(column, row).value();
    for (const std::uint8_t byte : uuid.bytes) {
      text += hex[byte >> 4];
      text += hex[byte & 0xF];
    }
    same = value.text == text;
    break;
  }
  case ht::Door::timestamp:
  case ht::Door::unix_time:
  case ht::Door::excel_serial: {
    const hc::Timestamp instant = batch.get<hc::Timestamp>(column, row).value();
    same = member(want, "seconds").i64() == instant.seconds &&
           member(want, "nanos").i64() == instant.nanos;
    break;
  }
  case ht::Door::date:
  case ht::Door::date_ordered: {
    const hc::Date date = batch.get<hc::Date>(column, row).value();
    same = member(want, "year").u64() == date.year && member(want, "month").u64() == date.month &&
           member(want, "day").u64() == date.day;
    break;
  }
  case ht::Door::datetime: {
    const hc::CivilDateTime civil = batch.get<hc::CivilDateTime>(column, row).value();
    same = member(want, "year").u64() == civil.date.year &&
           member(want, "month").u64() == civil.date.month &&
           member(want, "day").u64() == civil.date.day &&
           member(want, "nanos_of_day").u64() == civil.nanos_of_day;
    break;
  }
  case ht::Door::time:
    same = member(want, "nanos").u64() == batch.get<std::uint64_t>(column, row).value() &&
           static_cast<std::uint64_t>(
               batch.get<std::chrono::nanoseconds>(column, row).value().count()) ==
               member(want, "nanos").u64();
    break;
  case ht::Door::duration: {
    const hc::Duration span = batch.get<hc::Duration>(column, row).value();
    same =
        member(want, "seconds").i64() == span.seconds && member(want, "nanos").i64() == span.nanos;
    break;
  }
  case ht::Door::text:
    same = member(want, "text").text == batch.text(column, row).value();
    break;
  }
  return same || failed(where + ": the value");
}

bool check_failure(const ht::Error &error, const Json &want) {
  if (error.kind() != ht::ErrorKind::structure) {
    return failed("not a structural failure: " + error.message());
  }
  if (want.kind != Json::Kind::object) {
    return failed("failed, expected no failure: " + error.message());
  }
  const ht::Failure &failure = error.failure();
  bool same = member(want, "kind").text == ht::name(failure.kind) &&
              member(want, "record").u64() == failure.record &&
              member(want, "line").u64() == failure.line &&
              member(want, "byte").u64() == failure.byte;
  if (const Json *expected = want.find("expected")) {
    same = same && expected->u64() == failure.expected;
  }
  if (const Json *found = want.find("found")) {
    same = same && found->u64() == failure.found;
  }
  return same || failed("failure " + error.message());
}

bool check_header(const ht::Header *header, const Json &want) {
  if (header == nullptr) {
    return want.kind == Json::Kind::null || failed("no header");
  }
  if (want.kind != Json::Kind::array || want.items.size() != header->size()) {
    return failed("the header's names");
  }
  for (std::size_t i = 0; i < header->size(); ++i) {
    if ((*header)[i] != want.items[i].text) {
      return failed("header name " + std::to_string(i) + " \"" + (*header)[i] + "\"");
    }
  }
  return true;
}

// Reads every batch of a reader or a sheet, holding its rows, their numbers (a workbook's) and
// its failure to the case's; a failed read must fail again, the same.
template <class Reader>
bool read_all(Reader &reader, const Json &rows, const Json *numbers, const Json &failure,
              std::size_t batch_rows) {
  std::size_t at = 0;
  for (;;) {
    auto read = reader.read();
    if (const ht::Error *error = read.error_if()) {
      auto again = reader.read();
      if (again.ok() || again.error().message() != error->message()) {
        return failed("a failed reader did not stay failed");
      }
      return check_failure(*error, failure) &&
             (at == rows.items.size() || failed("rows before the failure"));
    }
    const std::optional<ht::Batch> &batch = read.value();
    if (!batch) {
      break;
    }
    if (batch->rows() < 1 || batch->rows() > batch_rows) {
      return failed("a batch of " + std::to_string(batch->rows()) + " rows");
    }
    for (const ht::Row &row : *batch) {
      if (at >= rows.items.size()) {
        return failed("more rows than the corpus has");
      }
      const Json &cells = rows.items[at];
      if (cells.items.size() != batch->columns()) {
        return failed("row " + std::to_string(at) + " has the wrong width");
      }
      if (numbers != nullptr && numbers->items[at].u64() != row.line()) {
        return failed("row " + std::to_string(at) + " is row number " + std::to_string(row.line()));
      }
      for (std::size_t column = 0; column < batch->columns(); ++column) {
        if (!check_cell(*batch, column, row.index(), cells.items[column])) {
          return false;
        }
      }
      ++at;
    }
  }
  if (failure.kind != Json::Kind::null) {
    return failed("no failure");
  }
  return at == rows.items.size() || failed("fewer rows than the corpus has");
}

const char *path_text(const std::filesystem::path &path, std::string &holder) {
  holder = path.string();
  return holder.c_str();
}

void replay_delimited(const Json &item, const std::filesystem::path &scratch) {
  ++cases;
  const std::string &name = member(item, "name").text;
  const std::string &input = member(item, "input").text;
  const Json &dialect = member(item, "dialect");
  const ht::Dialect declared =
      ht::Dialect{static_cast<std::uint8_t>(member(dialect, "separator").text[0])}
          .with_quoting(member(dialect, "quoting").boolean)
          .with_header(member(dialect, "has_header").boolean)
          .with_blank_lines_skipped(member(dialect, "skip_blank_lines").boolean);
  context = name;
  const auto plan = plan_of(member(item, "plan"));
  if (!plan) {
    failed("a plan the replay cannot declare");
    return;
  }
  const Json &header = member(item, "header");
  const Json &rows = member(item, "rows");
  const Json &failure = member(item, "failure");
  {
    std::ofstream file(scratch, std::ios::binary | std::ios::trunc);
    file.write(input.data(), static_cast<std::streamsize>(input.size()));
  }
  std::string holder;
  const char *path = path_text(scratch, holder);

  for (const std::size_t batch_rows : {std::size_t{1}, std::size_t{2}, std::size_t{1024}}) {
    for (const std::size_t buffer_bytes : {std::size_t{1}, std::size_t{7}, std::size_t{65536}}) {
      const ht::DelimitedOptions options{batch_rows, buffer_bytes};
      for (const bool bound : {true, false}) {
        for (const int source : {0, 1, 2}) {
          if (source == 0 && buffer_bytes != 1) {
            continue; // memory has no buffer to size
          }
          context = name + " (" +
                    (source == 0   ? "memory"
                     : source == 1 ? "a file"
                                   : "a source") +
                    ", " + std::to_string(batch_rows) + " rows a batch, buffer " +
                    std::to_string(buffer_bytes) + (bound ? ", plan given" : ", bound after") + ")";
          std::size_t offset = 0;
          const auto one_byte = [&input, &offset](std::uint8_t *into,
                                                  std::size_t capacity) noexcept {
            if (offset == input.size() || capacity == 0) {
              return std::optional<std::size_t>(0);
            }
            *into = static_cast<std::uint8_t>(input[offset++]);
            return std::optional<std::size_t>(1);
          };
          auto opened =
              source == 0
                  ? (bound ? ht::DelimitedReader::from_bytes(input, declared, *plan, options)
                           : ht::DelimitedReader::from_bytes(input, declared, options))
              : source == 1
                  ? (bound ? ht::DelimitedReader::open(path, declared, *plan, options)
                           : ht::DelimitedReader::open(path, declared, options))
                  : (bound ? ht::DelimitedReader::from_source(one_byte, declared, *plan, options)
                           : ht::DelimitedReader::from_source(one_byte, declared, options));
          if (const ht::Error *error = opened.error_if()) {
            // A failure in the header record itself.
            check_failure(*error, failure);
            continue;
          }
          ht::DelimitedReader &reader = opened.value();
          if (!check_header(reader.header(), header)) {
            continue;
          }
          if (!bound) {
            const auto binding = reader.bind(*plan);
            if (const ht::Error *error = binding.error_if()) {
              failed("bind: " + error->message());
              continue;
            }
            const auto again = reader.bind(*plan);
            if (again.ok() || again.error().kind() != ht::ErrorKind::already_bound) {
              failed("a second bind");
              continue;
            }
          }
          read_all(reader, rows, nullptr, failure, batch_rows);
        }
      }
    }
  }
}

void replay_workbook(const Json &item, const std::filesystem::path &corpus) {
  ++cases;
  const std::string &name = member(item, "name").text;
  context = name;
  const std::filesystem::path file = corpus / member(item, "file").text;
  std::string holder;
  const char *path = path_text(file, holder);
  std::vector<std::uint8_t> bytes;
  {
    std::ifstream stream(file, std::ios::binary);
    bytes.assign(std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>());
  }
  const std::string_view in_place(reinterpret_cast<const char *>(bytes.data()), bytes.size());
  const Json &failure = member(item, "failure");

  for (const int source : {0, 1, 2}) {
    const char *how = source == 0 ? "bytes in place" : source == 1 ? "bytes it keeps" : "a path";
    context = name + " (" + how + ")";
    auto opened = source == 0   ? ht::Workbook::from_bytes(in_place)
                  : source == 1 ? ht::Workbook::from_vector(bytes)
                                : ht::Workbook::open(path);
    // A package the core refuses: every way of opening it gives the one failure.
    if (item.find("sheet") == nullptr) {
      if (opened.ok()) {
        failed("it opened");
      } else {
        check_failure(opened.error(), failure);
      }
      continue;
    }
    if (const ht::Error *error = opened.error_if()) {
      failed("open: " + error->message());
      continue;
    }
    const ht::Workbook &book = opened.value();
    const char *format = book.format() == ht::Format::xlsx  ? "xlsx"
                         : book.format() == ht::Format::ods ? "ods"
                                                            : "numbers";
    if (member(item, "format").text != format) {
      failed(std::string("format ") + format);
    }
    if (member(item, "epoch").u64() != static_cast<std::uint64_t>(book.date_system())) {
      failed("the date system");
    }
    const Json &sheets = member(item, "sheets");
    bool listed = sheets.items.size() == book.sheets().size();
    for (std::size_t i = 0; listed && i < sheets.items.size(); ++i) {
      listed = member(sheets.items[i], "name").text == book.sheets()[i].name() &&
               member(sheets.items[i], "hidden").boolean == book.sheets()[i].hidden();
    }
    if (!listed) {
      failed("the sheets");
      continue;
    }
    const auto plan = plan_of(member(item, "plan"));
    if (!plan) {
      failed("a plan the replay cannot declare");
      continue;
    }
    const std::size_t index = member(item, "sheet").u64();
    const std::string &sheet_name = book.sheets()[index].name();
    // By name only where the name finds the same sheet.
    std::size_t first = 0;
    while (book.sheets()[first].name() != sheet_name) {
      ++first;
    }
    const Json &options = member(item, "options");
    for (const std::size_t batch_rows : {std::size_t{1}, std::size_t{2}, std::size_t{1024}}) {
      const ht::SheetOptions declared =
          ht::SheetOptions{}
              .with_header(member(options, "has_header").boolean)
              .with_empty_rows_skipped(member(options, "skip_empty_rows").boolean)
              .with_batch_rows(batch_rows);
      for (const bool named : {false, true}) {
        for (const bool bound : {true, false}) {
          if (named && first != index) {
            continue;
          }
          context = name + " (" + how + ", " + std::to_string(batch_rows) + " rows a batch, " +
                    (named ? "by name" : "by index") + (bound ? ", plan given" : ", bound after") +
                    ")";
          auto sheet = named ? (bound ? book.sheet(std::string_view(sheet_name), declared, *plan)
                                      : book.sheet_unbound(std::string_view(sheet_name), declared))
                             : (bound ? book.sheet(index, declared, *plan)
                                      : book.sheet_unbound(index, declared));
          if (const ht::Error *error = sheet.error_if()) {
            failed("sheet: " + error->message());
            continue;
          }
          if (!bound) {
            if (sheet.value().is_bound()) {
              failed("an unbound sheet is bound");
              continue;
            }
            const auto binding = sheet.value().bind(*plan);
            if (const ht::Error *error = binding.error_if()) {
              failed("bind: " + error->message());
              continue;
            }
          }
          if (check_header(sheet.value().header(), member(item, "header"))) {
            read_all(sheet.value(), member(item, "rows"), &member(item, "numbers"), failure,
                     batch_rows);
          }
        }
      }
    }
  }
}

// What is not there is an error, never a crash.
void absent(const std::filesystem::path &corpus) {
  context = "what is not there";
  std::string holder;
  auto book = ht::Workbook::open(path_text(corpus / "workbook/basic.xlsx", holder));
  if (!book.ok()) {
    failed("basic.xlsx: " + book.error().message());
    return;
  }
  const std::vector<ht::Column> plan{ht::Column::text(0)};
  const auto by_name = book.value().sheet(std::string_view("No such sheet"), {}, plan);
  const auto by_index = book.value().sheet(99, {}, plan);
  const auto missing = ht::Workbook::open("/no/such/file.xlsx");
  const auto not_zip = ht::Workbook::from_bytes("not a zip at all");
  const auto separator = ht::DelimitedReader::from_bytes("a\n", ht::Dialect{'"'});
  const auto too_wide =
      ht::DelimitedReader::from_bytes("a,b\n1,2\n", ht::Dialect::csv(), {ht::Column::i32(2)});
  if (by_name.ok() || by_name.error().kind() != ht::ErrorKind::no_sheet || by_index.ok() ||
      by_index.error().kind() != ht::ErrorKind::no_sheet || missing.ok() ||
      missing.error().kind() != ht::ErrorKind::io || not_zip.ok() ||
      not_zip.error().failure().kind != ht::FailureKind::not_a_zip || separator.ok() ||
      separator.error().kind() != ht::ErrorKind::separator || too_wide.ok() ||
      too_wide.error().kind() != ht::ErrorKind::plan || too_wide.error().column() != 0) {
    failed("an absence that is not its error");
  }
  auto unbound = ht::DelimitedReader::from_bytes("a\n1\n", ht::Dialect::csv());
  if (!unbound.ok() || unbound.value().read().error().kind() != ht::ErrorKind::unbound) {
    failed("a read with no plan");
  }
}

bool read_file(const std::filesystem::path &path, std::string &out) {
  std::ifstream file(path, std::ios::binary);
  out.assign(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
  return file.good() || file.eof();
}

} // namespace

int main(int argc, char **argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: %s <corpus directory>\n", argv[0]);
    return 2;
  }
  const std::filesystem::path corpus(argv[1]);
  std::error_code error;
  const std::filesystem::path scratch =
      std::filesystem::temp_directory_path(error) /
      ("hypertabular-smoke-" + std::to_string(reinterpret_cast<std::uintptr_t>(&error)) + ".csv");
  Json delimited;
  Json workbook;
  std::string source;
  if (error || !read_file(corpus / "delimited.json", source) || !Parser(source).parse(delimited) ||
      delimited.items.size() < 30) {
    std::fprintf(stderr, "corpus: cannot read %s/delimited.json\n", argv[1]);
    return 1;
  }
  std::string workbook_source;
  if (!read_file(corpus / "workbook.json", workbook_source) ||
      !Parser(workbook_source).parse(workbook) || workbook.items.size() < 80) {
    std::fprintf(stderr, "corpus: cannot read %s/workbook.json\n", argv[1]);
    return 1;
  }
  // Once with the usual starting sizes, once with every buffer one element long.
  for (const bool tiny : {false, true}) {
    ht::detail::starting_sizes =
        tiny ? ht::detail::StartingSizes{1, 1, 1, false} : ht::detail::StartingSizes{};
    for (const Json &item : delimited.items) {
      replay_delimited(item, scratch);
    }
    for (const Json &item : workbook.items) {
      replay_workbook(item, corpus);
    }
  }
  absent(corpus);
  std::filesystem::remove(scratch, error);
  if (failures > 0) {
    std::fprintf(stderr, "corpus: %d failures in %d cases\n", failures, cases);
    return 1;
  }
  std::fprintf(stderr, "corpus: %d cases pass, from every source and at every buffer size\n",
               cases);
  const std::string_view version = ht::native_version();
  std::printf("%.*s\n", static_cast<int>(version.size()), version.data());
  return 0;
}
