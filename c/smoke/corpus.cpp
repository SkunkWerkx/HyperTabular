// Replays the conformance corpus (corpus/delimited.json and corpus/workbook.json at the
// repository root, the files every binding's suite replays) through hypertabular.hpp: each
// case's header and its lookups, its rows' row numbers, every cell's verdict, value, fault span,
// text, detail and origin, what each batch's faults() lists, its structural failure, whole, and
// what the case says a suggestion finds. A case that fails is a break in the polyglot contract.
// A key the replay doesn't know fails it, at any level, so that one added to the corpus can't be
// passed over.
//
// Delimited text is read from memory, from a file and from a source that hands over a byte at
// a time, in batches of one row, of two and of more than any case has, each with the plan given
// and bound after the header, and from memory with the notes off. A workbook is read from bytes
// in place, from bytes it keeps and from its path, by index and by name, likewise. Everything
// runs twice: with the buffers' usual starting sizes, then with every buffer one element long and
// nothing presized, so each grow-and-retry path of the core runs on every call.
//
// usage: smoke_corpus <corpus directory>; prints the library's version when every case passes,
// as the other smoke programs do. run.sh runs it against each build of the archive.

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <initializer_list>
#include <iterator>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <variant>
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

// Every key of an object that the replay doesn't read is a failure: the corpus has moved on.
void known_keys(const Json &object, const std::string &path,
                std::initializer_list<std::string_view> keys) {
  if (object.kind != Json::Kind::object) {
    return;
  }
  for (const auto &entry : object.members) {
    if (std::find(keys.begin(), keys.end(), entry.first) == keys.end()) {
      failed(path + ": a key this replay doesn't read: \"" + entry.first + "\"");
    }
  }
}

void known_format_keys(const Json &format, const std::string &path) {
  known_keys(format, path, {"decimal_sep", "group_sep", "flags", "currency"});
}

// What a delimited case and a workbook case have in common.
void known_shared_keys(const Json &item, const std::string &name) {
  for (const Json &column : member(item, "plan").items) {
    known_keys(column, name + ".plan[]",
               {"door", "ordinal", "precision", "order", "epoch", "format", "tokens", "options"});
    known_format_keys(member(column, "format"), name + ".plan[].format");
    known_keys(member(column, "options"), name + ".plan[].options", {"flags", "pivot", "months"});
  }
  for (const Json &row : member(item, "rows").items) {
    for (const Json &cell : row.items) {
      known_keys(cell, name + ".rows[][]",
                 {"expect", "value", "text", "seconds", "nanos", "year", "month", "day",
                  "nanos_of_day", "magnitude", "scale", "negative", "fault", "raw", "raw_hex",
                  "detail", "origin"});
    }
  }
  for (const Json &problem : member(item, "problems").items) {
    known_keys(problem, name + ".problems[]",
               {"row", "line", "plan_column", "source_column", "reason", "fault", "detail",
                "origin", "expected", "found"});
  }
  known_keys(member(item, "failure"), name + ".failure",
             {"kind", "record", "line", "byte", "expected", "found"});
}

void known_suggest_keys(const Json &item, const std::string &name) {
  const Json &suggest = member(item, "suggest");
  known_keys(suggest, name + ".suggest", {"columns", "truncated", "sample_rows", "format"});
  known_format_keys(member(suggest, "format"), name + ".suggest.format");
  for (const Json &column : member(suggest, "columns").items) {
    known_keys(column, name + ".suggest.columns[]",
               {"door", "param", "status", "matched", "sampled"});
  }
}

void known_delimited_keys(const Json &item) {
  const std::string &name = member(item, "name").text;
  known_keys(item, name,
             {"name", "input", "input_hex", "dialect", "plan", "header", "rows", "problems",
              "failure", "suggest"});
  known_keys(member(item, "dialect"), name + ".dialect",
             {"separator", "quoting", "has_header", "skip_blank_lines", "skip", "header_row",
              "short_rows", "long_rows", "tokens", "encoding"});
  known_shared_keys(item, name);
  known_suggest_keys(item, name);
}

// `note` is the one key no replay reads: a sentence for whoever reads the file.
void known_workbook_keys(const Json &item) {
  const std::string &name = member(item, "name").text;
  known_keys(item, name,
             {"name", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header",
              "numbers", "rows", "problems", "failure", "note", "suggest"});
  known_keys(member(item, "options"), name + ".options",
             {"has_header", "skip_empty_rows", "range", "skip", "header_row", "tokens"});
  for (const Json &sheet : member(item, "sheets").items) {
    known_keys(sheet, name + ".sheets[]", {"name", "hidden"});
  }
  known_shared_keys(item, name);
  known_suggest_keys(item, name);
}

std::string unhex(std::string_view text) {
  const auto digit = [](char c) {
    return static_cast<unsigned>(c <= '9' ? c - '0' : c - 'a' + 10);
  };
  std::string bytes;
  for (std::size_t at = 0; at + 1 < text.size(); at += 2) {
    bytes += static_cast<char>(digit(text[at]) << 4 | digit(text[at + 1]));
  }
  return bytes;
}

std::vector<std::string> texts_of(const Json &list) {
  std::vector<std::string> texts;
  for (const Json &item : list.items) {
    texts.push_back(item.text);
  }
  return texts;
}

// A format the corpus names, its flags HyperCast's: the leniences, and the grouping from bit 8 up.
std::optional<hc::NumFormat> format_of(const Json &format) {
  const auto flags = static_cast<std::uint32_t>(member(format, "flags").u64());
  hc::NumFormat::Fields fields;
  fields.decimal_sep = member(format, "decimal_sep").text;
  fields.group_sep = member(format, "group_sep").text;
  fields.styles = static_cast<hc::NumStyles>(flags & 0xFFu);
  fields.grouping = static_cast<hc::Grouping>(flags & ~std::uint32_t{0xFF});
  fields.currency = member(format, "currency").text;
  return hc::NumFormat::declare(fields);
}

// A plan entry as a column: its door, what the door declares, its numeric format, its tokens and
// its date options. Column::date_options copies the months, so this storage needn't last.
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
    const auto declared = format_of(*format);
    if (!declared) {
      return std::nullopt;
    }
    column = column->format(*declared);
  }
  if (const Json *tokens = entry.find("tokens")) {
    column = column->tokens(texts_of(*tokens));
  }
  if (const Json *options = entry.find("options")) {
    hc::DateStyles styles = hc::DateStyles::none;
    for (const Json &flag : member(*options, "flags").items) {
      if (flag.text == "compact_date") {
        styles = styles | hc::DateStyles::compact_date;
      } else if (flag.text == "space_separator") {
        styles = styles | hc::DateStyles::space_separator;
      } else if (flag.text == "hour_offset") {
        styles = styles | hc::DateStyles::hour_offset;
      } else {
        return std::nullopt;
      }
    }
    const auto date = hc::DateOptions::declare(
        styles, static_cast<std::uint32_t>(member(*options, "pivot").u64()));
    if (!date) {
      return std::nullopt;
    }
    std::vector<hc::MonthName> names;
    for (const Json &month : member(*options, "months").items) {
      names.push_back(hc::MonthName{static_cast<std::uint8_t>(month.items.at(0).u64()),
                                    std::string_view(month.items.at(1).text)});
    }
    if (names.empty()) {
      column = column->date_options(*date);
    } else {
      std::vector<std::uint8_t> storage(hc::MonthNames::storage_size(names.data(), names.size()));
      const auto months = hc::MonthNames::declare(names.data(), names.size(),
                                                  hc::EnumStorage(storage.data(), storage.size()));
      if (!months) {
        return std::nullopt;
      }
      column = column->date_options(*date, *months);
    }
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

// The note beside a verdict: with notes off there's none, and it says so.
bool check_note(const ht::Batch &batch, std::size_t column, std::size_t row, const Json &want,
                bool notes, const std::string &where) {
  const Json *detail = notes ? want.find("detail") : nullptr;
  const Json *origin = notes ? want.find("origin") : nullptr;
  const hc::Detail found = batch.detail(column, row);
  const std::string detail_name = found == hc::Detail::none ? "" : hc::name(found).c_str();
  const std::string origin_name = ht::name(batch.origin(column, row));
  if (detail_name != (detail != nullptr ? detail->text : "")) {
    return failed(where + ": detail \"" + detail_name + "\"");
  }
  if (origin_name != (origin != nullptr ? origin->text : "cell")) {
    return failed(where + ": origin \"" + origin_name + "\"");
  }
  const ht::Row view = batch.row(row);
  return (view.detail(column) == found && view.origin(column) == batch.origin(column, row)) ||
         failed(where + ": the row's note isn't the batch's");
}

// One cell against the corpus's verdict for it.
bool check_cell(const ht::Batch &batch, std::size_t column, std::size_t row, const Json &want,
                bool notes) {
  const std::string &expect = member(want, "expect").text;
  const std::string where = "row " + std::to_string(row) + ", column " + std::to_string(column);
  if (!check_note(batch, column, row, want, notes, where)) {
    return false;
  }
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
    const Json *raw_hex = want.find("raw_hex");
    if (raw != (raw_hex != nullptr ? unhex(raw_hex->text) : member(want, "raw").text)) {
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
    same = member(want, "text").text == batch.text(column, row).value() &&
           member(want, "text").text == batch.raw(column, row);
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
  // The whole of it: a key the corpus leaves out is a 0.
  const bool same = member(want, "kind").text == ht::name(failure.kind) &&
                    member(want, "record").u64() == failure.record &&
                    member(want, "line").u64() == failure.line &&
                    member(want, "byte").u64() == failure.byte &&
                    member(want, "expected").u64() == failure.expected &&
                    member(want, "found").u64() == failure.found;
  return same || failed("failure " + error.message());
}

// A header's lookups held to its names: every name is found where it first is; the strict lookup
// refuses a name two columns share, naming the two; and a name that isn't there is an error that
// names it.
bool check_lookups(const ht::Header &header) {
  std::string missing = "?";
  for (std::size_t at = 0; at < header.size(); ++at) {
    const std::string &name = header[at];
    missing += name + "|";
    std::size_t first = 0;
    while (header[first] != name) {
      ++first;
    }
    if (header.ordinal(name) != std::optional<std::size_t>(first)) {
      return failed("ordinal(\"" + name + "\")");
    }
    std::size_t second = first + 1;
    while (second < header.size() && header[second] != name) {
      ++second;
    }
    const auto required = header.require(name);
    if (second == header.size()) {
      if (!required.ok() || required.value() != first) {
        return failed("require(\"" + name + "\")");
      }
    } else if (required.ok() || required.error().kind() != ht::ErrorKind::ambiguous_column ||
               required.error().detail() != name || required.error().first() != first ||
               required.error().second() != second) {
      return failed("require(\"" + name + "\"), which two columns share");
    }
  }
  const auto absent = header.require(missing);
  return (!absent.ok() && absent.error().kind() == ht::ErrorKind::no_column &&
          absent.error().detail() == missing) ||
         failed("require() of a name that isn't there");
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
  return check_lookups(*header);
}

// An entry of a faults list, in one spelling whichever side it came from.
std::string problem_text(std::optional<std::uint64_t> row, std::uint64_t line,
                         std::optional<std::uint64_t> plan, std::optional<std::uint64_t> source,
                         std::string_view reason, std::optional<std::uint64_t> offset,
                         std::optional<std::uint64_t> length, std::string_view detail,
                         std::string_view origin, std::optional<std::uint64_t> expected,
                         std::optional<std::uint64_t> found) {
  const auto number = [](std::optional<std::uint64_t> value) {
    return value ? std::to_string(*value) : std::string("-");
  };
  std::string text = "row " + number(row) + ", line " + std::to_string(line);
  text += ", plan " + number(plan) + ", source " + number(source);
  text += ", " + std::string(reason.empty() ? "-" : reason) + " at " + number(offset) + "+" +
          number(length);
  text += ", detail " + std::string(detail.empty() ? "-" : detail);
  text += ", origin " + std::string(origin.empty() ? "-" : origin);
  text += ", widths " + number(expected) + "/" + number(found);
  return text;
}

std::optional<std::uint64_t> number_of(const Json *value) {
  return value != nullptr ? std::optional<std::uint64_t>(value->u64()) : std::nullopt;
}

// What the corpus says the faults of a read are. With notes off the list has no detail or origin to
// give, and no formula with no cached value.
std::vector<std::string> wanted_problems(const Json &problems, bool notes) {
  std::vector<std::string> wanted;
  for (const Json &entry : problems.items) {
    const std::string &origin = member(entry, "origin").text;
    if (!notes && origin == "uncached") {
      continue;
    }
    const Json &span = member(entry, "fault");
    wanted.push_back(problem_text(
        number_of(entry.find("row")), member(entry, "line").u64(),
        number_of(entry.find("plan_column")), number_of(entry.find("source_column")),
        member(entry, "reason").text,
        span.items.size() == 2 ? std::optional<std::uint64_t>(span.items[0].u64()) : std::nullopt,
        span.items.size() == 2 ? std::optional<std::uint64_t>(span.items[1].u64()) : std::nullopt,
        notes ? std::string_view(member(entry, "detail").text) : std::string_view(),
        notes ? std::string_view(origin) : std::string_view(), number_of(entry.find("expected")),
        number_of(entry.find("found"))));
  }
  return wanted;
}

// A batch's faults added to the list, rows counted from `base`.
bool append_problems(const ht::Batch &batch, std::size_t base, std::vector<std::string> &found) {
  ht::Faults faults = batch.faults();
  for (const ht::Problem &problem : faults) {
    if (problem.line != batch.line(problem.row)) {
      return failed("a problem on line " + std::to_string(problem.line) + " of a row on line " +
                    std::to_string(batch.line(problem.row)));
    }
    using Number = std::optional<std::uint64_t>;
    const bool cell = problem.plan_column.has_value();
    if (cell ? (problem.expected != 0 || problem.found != 0)
             : (problem.source_column || problem.reason || problem.offset != 0 ||
                problem.length != 0)) {
      return failed("a problem that's half a cell and half a row");
    }
    const bool spanned = cell && *problem.reason != hc::CastFailure::empty;
    if (cell && !spanned && (problem.offset != 0 || problem.length != 0)) {
      return failed("an uncached formula's span");
    }
    const std::string detail =
        problem.detail == hc::Detail::none ? "" : hc::name(problem.detail).c_str();
    const std::string origin = problem.origin == ht::Origin::cell ? "" : ht::name(problem.origin);
    found.push_back(problem_text(
        base + problem.row, problem.line, cell ? Number(*problem.plan_column) : Number(),
        cell ? Number(*problem.source_column) : Number(), cell ? reason_name(*problem.reason) : "",
        spanned ? Number(problem.offset) : Number(), spanned ? Number(problem.length) : Number(),
        detail, origin, cell ? Number() : Number(problem.expected),
        cell ? Number() : Number(problem.found)));
  }
  return true;
}

// Reads every batch of a reader or a sheet, holding its rows, their numbers (a workbook's), their
// faults and its failure to the case's; however a read ended, it ends that way again.
template <class Reader>
bool read_all(Reader &reader, const Json &item, std::size_t batch_rows, bool notes) {
  const Json &rows = member(item, "rows");
  const Json &failure = member(item, "failure");
  const Json *numbers = item.find("numbers");
  const std::vector<std::string> wanted = wanted_problems(member(item, "problems"), notes);
  std::vector<std::string> found;
  std::size_t at = 0;
  bool good = true;
  for (;;) {
    auto read = reader.read();
    if (const ht::Error *error = read.error_if()) {
      auto again = reader.read();
      if (again.ok() || again.error().message() != error->message()) {
        return failed("a failed reader did not stay failed");
      }
      good = check_failure(*error, failure) && good;
      break;
    }
    const std::optional<ht::Batch> &batch = read.value();
    if (!batch) {
      if (failure.kind != Json::Kind::null) {
        return failed("no failure");
      }
      auto again = reader.read();
      if (!again.ok() || again.value().has_value()) {
        return failed("a finished reader did not stay finished");
      }
      break;
    }
    if (batch->rows() < 1 || batch->rows() > batch_rows) {
      return failed("a batch of " + std::to_string(batch->rows()) + " rows");
    }
    if (!append_problems(*batch, at, found)) {
      return false;
    }
    for (const ht::Row &row : *batch) {
      if (at >= rows.items.size()) {
        return failed("more rows than the corpus has");
      }
      const Json &cells = rows.items[at];
      if (cells.items.size() != batch->columns()) {
        return failed("row " + std::to_string(at) + " has the wrong width");
      }
      if (numbers != nullptr &&
          (at >= numbers->items.size() || numbers->items[at].u64() != row.line())) {
        return failed("row " + std::to_string(at) + " is row number " + std::to_string(row.line()));
      }
      for (std::size_t column = 0; column < batch->columns(); ++column) {
        if (!check_cell(*batch, column, row.index(), cells.items[column], notes)) {
          return false;
        }
      }
      ++at;
    }
  }
  if (at != rows.items.size()) {
    return failed("rows: " + std::to_string(at) + ", where the corpus has " +
                  std::to_string(rows.items.size()));
  }
  if (found != wanted) {
    std::string said;
    for (const std::string &entry : found) {
      said += "\n    " + entry;
    }
    said += "\n  where the corpus has";
    for (const std::string &entry : wanted) {
      said += "\n    " + entry;
    }
    return failed("faults:" + said);
  }
  return good;
}

// The case's dialect, or nothing for a declaration this replay doesn't know.
std::optional<ht::Dialect> dialect_of(const Json &dialect) {
  ht::Dialect declared =
      ht::Dialect{static_cast<std::uint8_t>(member(dialect, "separator").text[0])}
          .with_quoting(member(dialect, "quoting").boolean)
          .with_header(member(dialect, "has_header").boolean)
          .with_blank_lines_skipped(member(dialect, "skip_blank_lines").boolean)
          .with_skip(static_cast<std::uint32_t>(member(dialect, "skip").u64()))
          .with_header_row(static_cast<std::uint32_t>(member(dialect, "header_row").u64()));
  const std::string &short_rows = member(dialect, "short_rows").text;
  if (short_rows == "pad") {
    declared = declared.with_short_rows(ht::ShortRows::pad);
  } else if (!short_rows.empty() && short_rows != "reject") {
    return std::nullopt;
  }
  const std::string &long_rows = member(dialect, "long_rows").text;
  if (long_rows == "report") {
    declared = declared.with_long_rows(ht::LongRows::report);
  } else if (long_rows == "truncate") {
    declared = declared.with_long_rows(ht::LongRows::truncate);
  } else if (!long_rows.empty() && long_rows != "reject") {
    return std::nullopt;
  }
  const std::string &encoding = member(dialect, "encoding").text;
  if (encoding == "windows-1252") {
    declared = declared.with_encoding(ht::Encoding::windows_1252);
  } else if (encoding == "latin-1") {
    declared = declared.with_encoding(ht::Encoding::latin_1);
  } else if (encoding == "utf-16le") {
    declared = declared.with_encoding(ht::Encoding::utf_16le);
  } else if (!encoding.empty()) {
    return std::nullopt;
  }
  if (const Json *tokens = dialect.find("tokens")) {
    declared = declared.with_tokens(texts_of(*tokens));
  }
  return declared;
}

const char *path_text(const std::filesystem::path &path, std::string &holder) {
  holder = path.string();
  return holder.c_str();
}

// The bytes a case reads: the original ones where they aren't UTF-8.
std::string input_of(const Json &item) {
  const Json *hex = item.find("input_hex");
  return hex != nullptr ? unhex(hex->text) : member(item, "input").text;
}

void replay_delimited(const Json &item, const std::filesystem::path &scratch,
                      std::set<ht::Door> &doors) {
  const std::string &name = member(item, "name").text;
  const std::string input = input_of(item);
  context = name;
  const auto dialect = dialect_of(member(item, "dialect"));
  const auto plan = plan_of(member(item, "plan"));
  if (!dialect || !plan) {
    failed("a dialect or a plan the replay cannot declare");
    return;
  }
  const ht::Dialect &declared = *dialect;
  for (const ht::Column &column : *plan) {
    doors.insert(column.door());
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

  // A header that is structurally broken is the opening's error: the failure, and no rows before.
  const auto refused = [&rows, &failure](const ht::Error &error) {
    if (!rows.items.empty()) {
      failed("it could not be opened: " + error.message());
      return;
    }
    check_failure(error, failure);
  };

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
            refused(*error);
            continue;
          }
          ht::DelimitedReader &reader = opened.value();
          if (!check_header(reader.header(), header)) {
            continue;
          }
          if (!bound) {
            const auto early = reader.read();
            if (early.ok() || early.error().kind() != ht::ErrorKind::unbound) {
              failed("a read before the bind");
              continue;
            }
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
          read_all(reader, item, batch_rows, true);
        }
      }
    }
    // The notes off: the same rows, and nothing said of why.
    context = name + " (memory, notes off, " + std::to_string(batch_rows) + " rows a batch)";
    auto quiet = ht::DelimitedReader::from_bytes(
        input, declared, *plan, ht::DelimitedOptions{batch_rows}.with_notes(false));
    if (const ht::Error *error = quiet.error_if()) {
      refused(*error);
    } else {
      read_all(quiet.value(), item, batch_rows, false);
    }
  }
}

// A door as the corpus names it.
std::string_view door_name(ht::Door door) {
  switch (door) {
  case ht::Door::boolean:
    return "bool";
  case ht::Door::i8:
    return "i8";
  case ht::Door::i16:
    return "i16";
  case ht::Door::i32:
    return "i32";
  case ht::Door::i64:
    return "i64";
  case ht::Door::u8:
    return "u8";
  case ht::Door::u16:
    return "u16";
  case ht::Door::u32:
    return "u32";
  case ht::Door::u64:
    return "u64";
  case ht::Door::f32:
    return "f32";
  case ht::Door::f64:
    return "f64";
  case ht::Door::uuid:
    return "uuid";
  case ht::Door::timestamp:
    return "timestamp";
  case ht::Door::unix_time:
    return "unix";
  case ht::Door::date:
    return "date";
  case ht::Door::time:
    return "time";
  case ht::Door::duration:
    return "duration";
  case ht::Door::text:
    return "text";
  case ht::Door::decimal:
    return "decimal";
  case ht::Door::date_ordered:
    return "date_ordered";
  case ht::Door::datetime:
    return "datetime";
  case ht::Door::excel_serial:
    return "excel_serial";
  }
  return "?";
}

// A suggestion held to what the corpus says of the columns: an ambiguous column's set of candidates
// is put back together from the report, as what the core's answer held. Whatever a `,` proved of a
// resolved number, which the export's param is held to below, the plan is the same.
bool check_suggestion(const ht::Suggestion &found, const Json &want,
                      const std::optional<hc::NumFormat> &declared) {
  const Json &columns = member(want, "columns");
  if (found.truncated != member(want, "truncated").boolean) {
    return failed("truncated");
  }
  if (found.report.size() != columns.items.size() || found.plan.size() != columns.items.size()) {
    return failed(std::to_string(found.report.size()) + " columns");
  }
  for (std::size_t at = 0; at < found.report.size(); ++at) {
    const ht::SuggestedColumn &report = found.report[at];
    const ht::Column &planned = found.plan[at];
    const Json &column = columns.items[at];
    const std::string where = "column " + std::to_string(at);
    std::uint64_t param = report.param;
    if (!report.candidates.empty()) {
      param = 0;
      for (const ht::Candidate &candidate : report.candidates) {
        if (const auto *order = std::get_if<hc::DateOrder>(&candidate)) {
          param |= std::uint64_t{1} << static_cast<std::uint32_t>(*order);
        } else if (std::get<hc::NumFormat>(candidate) == hc::NumFormat::invariant()) {
          param |= HyperTabularRawSuggestion_NOTATION_COMMA_GROUPS;
        } else {
          param |= HyperTabularRawSuggestion_NOTATION_COMMA_DECIMAL;
        }
      }
    }
    std::uint64_t wanted = member(column, "param").u64();
    if (report.status == ht::SuggestStatus::resolved && ht::detail::numeric(report.door)) {
      // The plan is in the one notation the param left, and comma groups is also what's used with
      // no separator to prove it. The export's param is held to elsewhere.
      const hc::NumFormat notation = wanted == HyperTabularRawSuggestion_NOTATION_COMMA_DECIMAL
                                         ? ht::detail::comma_decimal()
                                         : hc::NumFormat::invariant();
      wanted = 0;
      const hc::NumFormat format = declared ? *declared : notation;
      if (!report.format || !(*report.format == format) || !(planned.format() == format)) {
        return failed(where + ": the format it's planned in");
      }
    } else if (report.format) {
      return failed(where + ": a format, for a column that isn't a resolved number");
    }
    if (door_name(report.door) != member(column, "door").text || param != wanted ||
        ht::name(report.status) != member(column, "status").text ||
        report.matched != member(column, "matched").u64() ||
        report.sampled != member(column, "sampled").u64()) {
      return failed(where + ": the report: " + std::string(door_name(report.door)) + " " +
                    std::to_string(param) + " " + std::string(ht::name(report.status)) + " " +
                    std::to_string(report.matched) + " of " + std::to_string(report.sampled));
    }
    if (report.ordinal != at || planned.ordinal() != at ||
        planned.door() !=
            (report.status == ht::SuggestStatus::resolved ? report.door : ht::Door::text)) {
      return failed(where + ": the plan doesn't say what the report does");
    }
  }
  return true;
}

// What the core's export says of the case's own rows: a cell with no text in the case is blank
// here, as a read's empty verdict is. This is where a resolved number's param is pinned.
bool check_export(const Json &item, const Json &want, std::size_t sample,
                  const std::optional<hc::NumFormat> &declared) {
  const Json &rows = member(item, "rows");
  const std::size_t width = member(item, "plan").items.size();
  std::string text;
  std::vector<HyperTabularSpan> cells;
  for (std::size_t column = 0; column < width; ++column) {
    for (std::size_t row = 0; row < sample; ++row) {
      const Json &cell = rows.items.at(row).items.at(column);
      std::string bytes = member(cell, "text").text;
      if (const Json *raw_hex = cell.find("raw_hex")) {
        bytes = unhex(raw_hex->text);
      }
      cells.push_back(HyperTabularSpan{static_cast<std::uint32_t>(text.size()),
                                       static_cast<std::uint32_t>(bytes.size())});
      text += bytes;
    }
  }
  std::vector<HyperTabularRawNumFormat> formats(width, HyperTabularRawNumFormat{});
  if (declared) {
    for (HyperTabularRawNumFormat &format : formats) {
      if (!ht::detail::raw_format(*declared, format)) {
        return failed("a declared format");
      }
    }
  }
  std::vector<HyperTabularRawSuggestion> direct(width, HyperTabularRawSuggestion{});
  const std::uint8_t nothing = 0;
  if (::hypertabular_suggest(text.empty() ? &nothing
                                          : reinterpret_cast<const std::uint8_t *>(text.data()),
                             text.size(), nullptr, 0, cells.data(), nullptr, sample, width,
                             formats.data(), direct.data()) != HYPERTABULAR_OK) {
    return failed("the export refused the case's rows");
  }
  const Json &columns = member(want, "columns");
  for (std::size_t column = 0; column < columns.items.size() && column < width; ++column) {
    const HyperTabularRawSuggestion &got = direct[column];
    const Json &expected = columns.items[column];
    const std::string_view status = got.status == HyperTabularRawSuggestion_RESOLVED ? "resolved"
                                    : got.status == HyperTabularRawSuggestion_AMBIGUOUS
                                        ? "ambiguous"
                                        : "unresolved";
    if (door_name(static_cast<ht::Door>(got.door)) != member(expected, "door").text ||
        got.param != member(expected, "param").u64() || status != member(expected, "status").text ||
        got.matched != member(expected, "matched").u64() ||
        got.sampled != member(expected, "sampled").u64()) {
      return failed("the export says otherwise of column " + std::to_string(column));
    }
  }
  return true;
}

// Every case that says what to suggest: from memory and from a file; through the export, over the
// cells the case's own rows say; and a plan that reads back without refusing what it resolved. And
// every case that doesn't say: a suggestion fails where a read of the case fails, and nowhere else.
void replay_suggestion(const Json &item, const std::filesystem::path &scratch, int &seen,
                       int &failing) {
  const std::string &name = member(item, "name").text;
  context = name + " (suggest)";
  const std::string input = input_of(item);
  const auto dialect = dialect_of(member(item, "dialect"));
  if (!dialect) {
    failed("a dialect the replay cannot declare");
    return;
  }
  const Json *want = item.find("suggest");
  if (want == nullptr) {
    const auto answer = ht::DelimitedReader::suggest_bytes(input, *dialect);
    if (const ht::Error *error = answer.error_if()) {
      ++failing;
      if (member(item, "failure").kind == Json::Kind::null) {
        failed(error->message());
      }
    }
    return;
  }
  ++seen;
  ht::SuggestOptions options;
  if (const Json *rows = want->find("sample_rows")) {
    options = options.with_sample_rows(rows->u64());
  }
  std::optional<hc::NumFormat> declared;
  if (const Json *format = want->find("format")) {
    declared = format_of(*format);
    if (!declared) {
      failed("a format the replay cannot declare");
      return;
    }
    options = options.with_format(*declared);
  }
  const auto found = ht::DelimitedReader::suggest_bytes(input, *dialect, options);
  if (const ht::Error *error = found.error_if()) {
    failed(error->message());
    return;
  }
  if (!check_suggestion(found.value(), *want, declared)) {
    return;
  }
  {
    std::ofstream file(scratch, std::ios::binary | std::ios::trunc);
    file.write(input.data(), static_cast<std::streamsize>(input.size()));
  }
  std::string holder;
  context = name + " (suggest, from a file)";
  const auto from_file =
      ht::DelimitedReader::suggest(path_text(scratch, holder), *dialect, options);
  if (const ht::Error *error = from_file.error_if()) {
    failed(error->message());
    return;
  }
  if (!check_suggestion(from_file.value(), *want, declared)) {
    return;
  }
  context = name + " (suggest, the export)";
  const std::size_t sample = want->find("sample_rows") != nullptr
                                 ? want->find("sample_rows")->u64()
                                 : member(item, "rows").items.size();
  if (!check_export(item, *want, sample, declared)) {
    return;
  }
  // What's resolved takes every cell it saw, and refuses none.
  context = name + " (suggest, read back)";
  const ht::Suggestion &suggestion = found.value();
  auto reader = ht::DelimitedReader::from_bytes(input, *dialect, suggestion.plan);
  if (const ht::Error *error = reader.error_if()) {
    failed(error->message());
    return;
  }
  std::size_t read = 0;
  while (read < sample) {
    auto batch = reader.value().read();
    if (const ht::Error *error = batch.error_if()) {
      failed(error->message());
      return;
    }
    if (!batch.value()) {
      break;
    }
    ht::Faults faults = batch.value()->faults();
    for (const ht::Problem &problem : faults) {
      if (!problem.plan_column) {
        continue;
      }
      const ht::SuggestedColumn &report = suggestion.report[*problem.plan_column];
      if (report.status == ht::SuggestStatus::resolved && report.door != ht::Door::text &&
          problem.row + read < sample) {
        failed("it read back refused, row " + std::to_string(problem.row + read));
      }
    }
    read += batch.value()->rows();
  }
}

void replay_workbook(const Json &item, const std::filesystem::path &corpus) {
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
    ht::SheetOptions base =
        ht::SheetOptions{}
            .with_header(member(options, "has_header").boolean)
            .with_empty_rows_skipped(member(options, "skip_empty_rows").boolean)
            .with_skip(static_cast<std::uint32_t>(member(options, "skip").u64()))
            .with_header_row(static_cast<std::uint32_t>(member(options, "header_row").u64()));
    if (const Json *tokens = options.find("tokens")) {
      base = base.with_tokens(texts_of(*tokens));
    }
    if (const Json *range = options.find("range")) {
      const auto parsed = ht::Range::parse(range->text);
      if (!parsed.ok()) {
        failed(parsed.error().message());
        continue;
      }
      base = base.with_range(parsed.value());
    }
    for (const std::size_t batch_rows : {std::size_t{1}, std::size_t{2}, std::size_t{1024}}) {
      const ht::SheetOptions declared = base.with_batch_rows(batch_rows);
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
            const auto early = sheet.value().read();
            if (early.ok() || early.error().kind() != ht::ErrorKind::unbound) {
              failed("a read before the bind");
              continue;
            }
            const auto binding = sheet.value().bind(*plan);
            if (const ht::Error *error = binding.error_if()) {
              failed("bind: " + error->message());
              continue;
            }
            const auto again = sheet.value().bind(*plan);
            if (again.ok() || again.error().kind() != ht::ErrorKind::already_bound) {
              failed("a second bind");
              continue;
            }
          }
          if (check_header(sheet.value().header(), member(item, "header"))) {
            read_all(sheet.value(), item, batch_rows, true);
          }
        }
      }
      // The notes off: the same rows, and nothing said of why.
      context = name + " (" + how + ", " + std::to_string(batch_rows) + " rows a batch, notes off)";
      auto quiet = book.sheet(index, declared.with_notes(false), *plan);
      if (const ht::Error *error = quiet.error_if()) {
        failed("sheet: " + error->message());
      } else {
        read_all(quiet.value(), item, batch_rows, false);
      }
    }
  }
}

bool read_fails(const ht::Workbook &book, std::size_t at) {
  auto sheet = book.sheet(at, {}, {ht::Column::text(0)});
  if (!sheet.ok()) {
    return true;
  }
  for (;;) {
    auto read = sheet.value().read();
    if (!read.ok()) {
      return true;
    }
    if (!read.value()) {
      return false;
    }
  }
}

// What was resolved reads back through its door over every sampled row of every sheet of every
// package the corpus names, and a package that's broken among its sampled rows fails only if its
// read does.
void every_sheets_suggestion(const std::filesystem::path &corpus) {
  int planned = 0;
  int typed = 0;
  int refused = 0;
  ht::SuggestOptions western;
  western.format = hc::NumFormat::invariant();
  ht::SuggestOptions comma;
  comma.format = ht::detail::comma_decimal();
  ht::SuggestOptions indian;
  indian.format = hc::NumFormat::invariant().with_grouping(hc::Grouping::indian);
  const std::array<ht::SuggestOptions, 4> declarations{ht::SuggestOptions{}, western, comma,
                                                       indian};
  std::error_code error;
  for (const auto &entry : std::filesystem::directory_iterator(corpus / "workbook", error)) {
    if (entry.path().extension() == ".py") {
      continue;
    }
    std::string holder;
    context = entry.path().filename().string() + " (suggest)";
    const auto opened = ht::Workbook::open(path_text(entry.path(), holder));
    if (!opened.ok()) {
      continue;
    }
    const ht::Workbook &book = opened.value();
    for (std::size_t at = 0; at < book.sheets().size() * declarations.size(); ++at) {
      const ht::SuggestOptions &asked = declarations[at % declarations.size()];
      const std::size_t declared_at = at % declarations.size();
      const std::size_t sheet_at = at / declarations.size();
      auto sheet = book.sheet_unbound(sheet_at);
      if (!sheet.ok()) {
        continue;
      }
      const auto found = sheet.value().suggest(asked);
      if (!found.ok()) {
        if (!read_fails(book, sheet_at)) {
          failed("sheet " + std::to_string(sheet_at) + ": " + found.error().message() +
                 ", where a read has no failure");
        }
        ++refused;
        continue;
      }
      const std::vector<ht::Column> &plan = found.value().plan;
      if (plan.empty()) {
        continue;
      }
      ++planned;
      typed += static_cast<int>(
          std::count_if(plan.begin(), plan.end(), [](const ht::Column &column) noexcept {
            return column.door() != ht::Door::text;
          }));
      auto read = book.sheet(sheet_at, {}, plan);
      if (!read.ok()) {
        failed("read back: " + read.error().message());
        continue;
      }
      std::size_t rows = 0;
      while (rows < 1000) {
        auto batch = read.value().read();
        if (!batch.ok()) {
          failed("read back: " + batch.error().message());
          break;
        }
        if (!batch.value()) {
          break;
        }
        ht::Faults faults = batch.value()->faults();
        for (const ht::Problem &problem : faults) {
          if (problem.row + rows < 1000 && problem.reason &&
              *problem.reason != hc::CastFailure::empty &&
              plan[*problem.plan_column].door() != ht::Door::text) {
            failed("sheet " + std::to_string(sheet_at) + ", declaration " +
                   std::to_string(declared_at) + " read back refused, row " +
                   std::to_string(problem.row + rows));
          }
        }
        rows += batch.value()->rows();
      }
    }
  }
  context = "every sheet's suggestion";
  if (planned <= 80 || refused < 2 || typed <= 350) {
    failed(std::to_string(planned) + " sheets planned, " + std::to_string(refused) + " refused, " +
           std::to_string(typed) + " typed columns");
  }
}

// A typed number has no locale: a workbook's number column is a number whether a notation is
// declared or not, and whichever.
void a_number_column_is_numeric_under_any_format(const std::filesystem::path &corpus) {
  const std::array<std::pair<const char *, std::array<const char *, 2>>, 3> columns{{
      {"basic.xlsx", {"amount", "ratio"}},
      {"basic.ods", {"amount", "pct"}},
      {"excel-web-data.xlsx", {"pct", "money"}},
  }};
  ht::SuggestOptions western;
  western.format = hc::NumFormat::invariant();
  ht::SuggestOptions comma;
  comma.format = ht::detail::comma_decimal();
  for (const auto &entry : columns) {
    context = std::string(entry.first) + " (a number column)";
    std::string holder;
    const auto opened = ht::Workbook::open(path_text(corpus / "workbook" / entry.first, holder));
    if (!opened.ok()) {
      failed(opened.error().message());
      continue;
    }
    for (const ht::SuggestOptions &asked : {ht::SuggestOptions{}, western, comma}) {
      auto sheet = opened.value().sheet_unbound(std::size_t{0});
      const auto found = sheet.value().suggest(asked);
      if (!found.ok()) {
        failed(found.error().message());
        continue;
      }
      // The sample's notes are its own, whatever a read of the sheet was to have.
      auto quiet =
          opened.value().sheet_unbound(std::size_t{0}, ht::SheetOptions{}.with_notes(false));
      const auto again = quiet.value().suggest(asked);
      if (!again.ok() || again.value().report.size() != found.value().report.size()) {
        failed("a sheet with its notes off");
        continue;
      }
      for (std::size_t at = 0; at < found.value().report.size(); ++at) {
        if (again.value().report[at].status != found.value().report[at].status ||
            again.value().report[at].door != found.value().report[at].door) {
          failed("a sheet with its notes off answers otherwise");
        }
      }
      for (const char *name : entry.second) {
        const auto &report = found.value().report;
        const auto at = std::find_if(report.begin(), report.end(), [&](const auto &column) {
          return column.name && *column.name == name;
        });
        if (at == report.end()) {
          failed(std::string("no column ") + name);
        } else if (at->status != ht::SuggestStatus::resolved || !ht::detail::numeric(at->door)) {
          failed(std::string(name) + " isn't numeric");
        }
      }
    }
  }
}

// A declared order answers the sample's question and only that.
void a_declared_date_order_settles_an_open_date() {
  context = "a declared date order";
  const std::string open = "day\n01/02/2026\n03/04/2026\n";
  const std::string settled = "day\n13/02/2026\n";
  const auto ask = [](const std::string &input, const ht::SuggestOptions &options) {
    return ht::DelimitedReader::suggest_bytes(input, ht::Dialect::csv(), options).value();
  };
  ht::Suggestion found = ask(open, {});
  if (found.report[0].status != ht::SuggestStatus::ambiguous ||
      found.plan[0].door() != ht::Door::text) {
    failed("nothing declared");
  }
  const auto day = hc::DateOrder::day_month_year;
  const auto month = hc::DateOrder::month_day_year;
  const auto param = [](hc::DateOrder order) { return static_cast<std::uint32_t>(order); };
  found = ask(open, ht::SuggestOptions{}.with_date_order(day));
  if (found.report[0].status != ht::SuggestStatus::resolved ||
      !found.report[0].candidates.empty() || found.plan[0].door() != ht::Door::date_ordered ||
      found.report[0].param != param(day)) {
    failed("day first");
  }
  found = ask(open, ht::SuggestOptions{}
                        .with_date_order(day)
                        .with_column_date_order(0, day)
                        .with_column_date_order(0, month));
  if (found.report[0].param != param(month)) {
    failed("a column's own, the last given");
  }
  found = ask(settled, ht::SuggestOptions{}.with_date_order(month));
  if (found.report[0].param != param(day)) {
    failed("an order the cells rule out");
  }
  found = ask(open, ht::SuggestOptions{}.with_date_order(hc::DateOrder::year_month_day));
  if (found.report[0].status != ht::SuggestStatus::ambiguous ||
      found.plan[0].door() != ht::Door::text) {
    failed("an order out of the set");
  }
}

// Every workbook case that says what to suggest: through the header, and a plan that then reads
// back refusing nothing it resolved. A typed number has no notation, and a text cell asks what text
// asks.
void replay_workbook_suggestion(const Json &item, const std::filesystem::path &corpus, int &seen) {
  const Json *want = item.find("suggest");
  if (want == nullptr) {
    return;
  }
  ++seen;
  const std::string &name = member(item, "name").text;
  context = name + " (suggest)";
  std::string holder;
  const auto opened = ht::Workbook::open(path_text(corpus / member(item, "file").text, holder));
  if (!opened.ok()) {
    failed(opened.error().message());
    return;
  }
  ht::SuggestOptions options;
  std::optional<hc::NumFormat> declared;
  if (const Json *format = want->find("format")) {
    declared = format_of(*format);
    if (!declared) {
      failed("a format the replay cannot declare");
      return;
    }
    options = options.with_format(*declared);
  }
  const std::size_t at = member(item, "sheet").u64();
  auto sheet = opened.value().sheet_unbound(at);
  if (!sheet.ok()) {
    failed(sheet.error().message());
    return;
  }
  const auto found = sheet.value().suggest(options);
  if (!found.ok()) {
    failed(found.error().message());
    return;
  }
  if (!check_suggestion(found.value(), *want, declared)) {
    return;
  }
  auto read = opened.value().sheet(at, {}, found.value().plan);
  if (!read.ok()) {
    failed(read.error().message());
    return;
  }
  for (;;) {
    auto batch = read.value().read();
    if (!batch.ok()) {
      failed(batch.error().message());
      return;
    }
    if (!batch.value()) {
      return;
    }
    ht::Faults faults = batch.value()->faults();
    for (const ht::Problem &problem : faults) {
      if (problem.reason && *problem.reason != hc::CastFailure::empty) {
        failed("the plan refused a cell of row " + std::to_string(problem.row));
      }
    }
  }
}

// A sheet part of the way through a read, or with a plan bound, answers the same, and carries on
// from where it was.
void a_sheet_stays_where_it_is(const std::filesystem::path &corpus) {
  context = "a sheet asked part of the way through";
  std::string holder;
  const auto opened =
      ht::Workbook::open(path_text(corpus / "workbook/libreoffice-data.xlsx", holder));
  if (!opened.ok()) {
    failed(opened.error().message());
    return;
  }
  const ht::Workbook &book = opened.value();
  const ht::SheetOptions one = ht::SheetOptions{}.with_batch_rows(1);
  const ht::SuggestOptions asked = ht::SuggestOptions{}.with_sample_rows(1);
  const auto spell = [](const ht::Suggestion &suggestion) {
    std::string text;
    for (const ht::SuggestedColumn &report : suggestion.report) {
      text += std::string(door_name(report.door)) + " " + std::string(ht::name(report.status)) +
              " " + std::to_string(report.matched) + "/" + std::to_string(report.sampled) + ",";
    }
    return text + (suggestion.truncated ? "more" : "all");
  };
  const auto fresh = book.sheet_unbound(0, one).value().suggest(asked);
  if (!fresh.ok() || fresh.value().plan.empty()) {
    failed("a fresh sheet's suggestion");
    return;
  }
  std::vector<ht::Column> plan;
  for (std::size_t at = 0; at < fresh.value().plan.size(); ++at) {
    plan.push_back(ht::Column::text(at));
  }
  std::vector<std::string> rows;
  auto whole = book.sheet(0, one, plan);
  (void)whole.value().for_each_row(
      [&rows](const ht::Row &row) noexcept { rows.push_back(row.raw(0)); });
  auto sheet = book.sheet(0, one, plan);
  if (rows.size() < 2 || spell(sheet.value().suggest(asked).value()) != spell(fresh.value())) {
    failed("bound, unread");
    return;
  }
  auto first = sheet.value().read();
  if (!first.ok() || !first.value() || first.value()->raw(0, 0) != rows[0]) {
    failed("the first row");
    return;
  }
  if (spell(sheet.value().suggest(asked).value()) != spell(fresh.value())) {
    failed("part of the way through");
    return;
  }
  // The batch in hand is still the row it was, since suggest is a read of its own.
  if (first.value()->raw(0, 0) != rows[0]) {
    failed("the batch in hand moved");
    return;
  }
  auto second = sheet.value().read();
  if (!second.ok() || !second.value() || second.value()->raw(0, 0) != rows[1]) {
    failed("it did not carry on from where it was");
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
  context = "the corpus's keys";
  for (const Json &item : delimited.items) {
    known_delimited_keys(item);
  }
  for (const Json &item : workbook.items) {
    known_workbook_keys(item);
  }
  // Once with the usual starting sizes, once with every buffer one element long.
  std::set<ht::Door> doors;
  int suggested = 0;
  int failing = 0;
  for (const bool tiny : {false, true}) {
    ht::detail::starting_sizes =
        tiny ? ht::detail::StartingSizes{1, 1, 1, false} : ht::detail::StartingSizes{};
    suggested = 0;
    failing = 0;
    for (const Json &item : delimited.items) {
      replay_delimited(item, scratch, doors);
    }
    for (const Json &item : workbook.items) {
      replay_workbook(item, corpus);
    }
    for (const Json &item : delimited.items) {
      replay_suggestion(item, scratch, suggested, failing);
    }
    int workbook_suggested = 0;
    for (const Json &item : workbook.items) {
      replay_workbook_suggestion(item, corpus, workbook_suggested);
    }
    if (workbook_suggested != 15) {
      context = "the workbook corpus";
      failed(std::to_string(workbook_suggested) + " cases say what to suggest");
    }
    every_sheets_suggestion(corpus);
    a_number_column_is_numeric_under_any_format(corpus);
    a_declared_date_order_settles_an_open_date();
    a_sheet_stays_where_it_is(corpus);
  }
  context = "the corpus's reach";
  if (suggested < 12 || failing == 0) {
    failed(std::to_string(suggested) + " suggestions, " + std::to_string(failing) + " failing");
  }
  // The corpus reaches every door this binding has a constructor for.
  if (doors.size() != 22) {
    failed("the corpus casts through " + std::to_string(doors.size()) + " doors");
  }
  absent(corpus);
  std::filesystem::remove(scratch, error);
  if (failures > 0) {
    std::fprintf(stderr, "corpus: %d failures\n", failures);
    return 1;
  }
  std::fprintf(stderr,
               "corpus: %zu cases and %d suggestions pass, from every source, from the usual "
               "buffers and from one-element ones\n",
               delimited.items.size() + workbook.items.size(), suggested);
  const std::string_view version = ht::native_version();
  std::printf("%.*s\n", static_cast<int>(version.size()), version.data());
  return 0;
}
