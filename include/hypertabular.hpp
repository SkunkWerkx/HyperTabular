// The C++ face of the hypertabular core (https://github.com/SkunkWerkx/HyperTabular):
// header-only, over the C ABI in hypertabular.h, and linked against the same shared or static
// library. It stands on HyperCast's C++ header, hypercast.hpp (HyperCast's C archive, 1.1.0 or
// later), because a cell's verdict is HyperCast's: a hypercast::Verdict<T>, the value or a
// hypercast::Fault (Empty, Malformed or OutOfRange and the offending span of the cell's own
// text), and the values and declared formats are HyperCast's types. C++17 or later; under
// C++23 with <expected>, a verdict and a result also convert to std::expected.
//
//   auto opened = hypertabular::DelimitedReader::from_bytes(csv, hypertabular::Dialect::csv());
//   if (!opened.ok()) return report(opened.error());
//   hypertabular::DelimitedReader &reader = opened.value();
//   const hypertabular::Header &header = *reader.header();
//   auto bound = reader.bind({hypertabular::Column::i64(*header.ordinal("id")),
//                             hypertabular::Column::text(*header.ordinal("name"))});
//   if (!bound.ok()) return report(bound.error());
//   for (;;) {
//     auto batch = reader.read();                 // a Result<std::optional<Batch>>
//     if (!batch.ok()) return report(batch.error());
//     if (!batch.value()) break;                  // the input is over
//     for (const hypertabular::Row &row : *batch.value()) {
//       const auto id = row.get<std::int64_t>(0); // a hypercast::Verdict<std::int64_t>
//       if (const std::int64_t *value = id.value_if()) use(*value);
//     }
//   }
//
// Nothing here throws. Every function is noexcept, and the header builds with exceptions off
// (-fno-exceptions, or /EHs-c- with _HAS_EXCEPTIONS=0). A cell that does not cast is a
// Verdict's fault, never an error. What can go wrong with a read is a hypertabular::Error in a
// hypertabular::Result: the read itself, the caller's own declarations, or data that is
// structurally broken (a Failure, delivered only after every intact row before it, and again
// on every read after). Unlike HyperCast's doors this binding owns memory, the way every
// HyperTabular binding does: its buffers are standard containers, and running out of memory
// ends the program, as an allocation failure under noexcept does. A contract violation (a
// column or a row index outside the batch, a door read as a type it does not make, the value
// of a result that holds an error) ends the program with std::abort(), as Rust's panic and
// Swift's precondition do.
//
// A batch is a view of the reader's buffers, valid until the next read(), as C#'s, Java's,
// Go's and Swift's are. Fault spans count bytes of the cell's UTF-8.

#ifndef HYPERTABULAR_HPP
#define HYPERTABULAR_HPP

#if !defined(__cplusplus) ||                                                                       \
    (__cplusplus < 201703L && (!defined(_MSVC_LANG) || _MSVC_LANG < 201703L))
#error "hypertabular.hpp needs C++17 or later; C programs include hypertabular.h"
#endif

#if defined(__has_include)
#if !__has_include("hypercast.hpp")
#error "hypertabular.hpp stands on hypercast.hpp: add HyperCast's C archive (1.1.0 or later)"
#endif
#endif

#include <algorithm>
#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <initializer_list>
#include <iterator>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

#if __has_include(<version>)
#include <version>
#endif
#if defined(__cpp_lib_expected) && __cpp_lib_expected >= 202202L
#include <expected>
#define HYPERTABULAR_HAS_EXPECTED 1
#endif

#include "hypercast.hpp"
#include "hypertabular.h"

namespace hypertabular {

// HyperCast's, as every HyperTabular binding hands them out: a cell's verdict, its fault, the
// values the doors make, and what a column declares.
using hypercast::CastFailure;
using hypercast::CivilDateTime;
using hypercast::Date;
using hypercast::DateOrder;
using hypercast::Decimal;
using hypercast::Duration;
using hypercast::ExcelEpoch;
using hypercast::Fault;
using hypercast::NumFormat;
using hypercast::NumStyles;
using hypercast::optional;
using hypercast::Timestamp;
using hypercast::UnixPrecision;
using hypercast::Uuid;
using hypercast::Verdict;

namespace detail {
// The file at `path`, opened for reading in binary, or nullptr. MSVC's fopen_s, since its
// fopen is deprecated and a consumer may build with warnings as errors.
inline std::FILE *open_file(const char *path) noexcept {
#if defined(_MSC_VER)
  std::FILE *file = nullptr;
  return fopen_s(&file, path, "rb") == 0 ? file : nullptr;
#else
  return std::fopen(path, "rb");
#endif
}

[[noreturn]] inline void contract_violation() noexcept {
  // A caller bug, never data: an index outside the batch, a door read as another's type, the
  // value of a result that holds an error.
  std::abort();
}
} // namespace detail

// ---------------------------------------------------------------------------------------------
// Errors. A cell that does not cast is never one.

// What is structurally wrong with the data, numbered as the core numbers it.
enum class FailureKind : std::uint32_t {
  unclosed_quote = 1,  // the input ended inside a quoted cell
  column_count = 2,    // a record's cell count disagrees with the first record's
  row_too_long = 3,    // a record longer than a reader will buffer (1 GiB)
  not_a_zip = 16,      // the container has no end-of-central-directory record
  container = 17,      // the zip's own structure is broken
  encrypted = 18,      // the part, or the document, is encrypted
  method = 19,         // a part compressed by neither stored nor deflate; `found` is the method
  missing_part = 20,   // a part the workbook cannot be read without is not there
  xml = 21,            // the part's XML ends inside a construct; `found` says which
  deflate = 22,        // the part's bytes are not a deflate stream, or stop short
  not_a_workbook = 23, // a zip, but not an XLSX, ODS or Numbers workbook
  shared_string = 24,  // a cell names a shared string that is not there
  too_large = 25,      // more text than a span can address
  iwa = 26             // a Numbers part is not what the IWA packaging says
};

// The corpus's name of a failure kind: "column_count", "not_a_zip".
constexpr std::string_view name(FailureKind kind) noexcept {
  switch (kind) {
  case FailureKind::unclosed_quote:
    return "unclosed_quote";
  case FailureKind::column_count:
    return "column_count";
  case FailureKind::row_too_long:
    return "row_too_long";
  case FailureKind::not_a_zip:
    return "not_a_zip";
  case FailureKind::container:
    return "container";
  case FailureKind::encrypted:
    return "encrypted";
  case FailureKind::method:
    return "method";
  case FailureKind::missing_part:
    return "missing_part";
  case FailureKind::xml:
    return "xml";
  case FailureKind::deflate:
    return "deflate";
  case FailureKind::not_a_workbook:
    return "not_a_workbook";
  case FailureKind::shared_string:
    return "shared_string";
  case FailureKind::too_large:
    return "too_large";
  case FailureKind::iwa:
    return "iwa";
  }
  return "container";
}

// Where the data broke. Delimited text: `record` is the offending record's zero-based index,
// header and skipped blank lines included, `line` the one-based line it starts on, `byte` its
// absolute offset, and for column_count `expected` and `found` the two cell counts. A
// workbook: `record` is the part being read (HyperTabularFailure_PART_SHEET and the rest),
// `line` the sheet row, `byte` the offset in the part's inflated bytes, and `expected` and
// `found` what the kind says they are.
struct Failure {
  FailureKind kind;
  std::uint64_t record;
  std::uint32_t line;
  std::uint64_t byte;
  std::uint32_t expected;
  std::uint32_t found;

  friend constexpr bool operator==(const Failure &a, const Failure &b) noexcept {
    return a.kind == b.kind && a.record == b.record && a.line == b.line && a.byte == b.byte &&
           a.expected == b.expected && a.found == b.found;
  }
  friend constexpr bool operator!=(const Failure &a, const Failure &b) noexcept {
    return !(a == b);
  }
};

// Why a read, an open or a bind did not happen. Every kind but structure is said before any
// data is read; structure is the data's.
enum class ErrorKind {
  io,            // the file or the source could not be read; detail() says why
  separator,     // the dialect's separator is not tab or printable ASCII other than '"'
  plan,          // plan column column() cannot be honoured: its ordinal, or its format
  no_sheet,      // the workbook has no such sheet; detail() says which was asked for
  no_column,     // the header has no column of that name; detail() is the name
  unbound,       // the reader has no plan yet: bind one before reading
  already_bound, // the reader already has a plan: a plan is bound once
  structure      // the data is broken: failure() says what and where
};

class Error {
public:
  static Error io(std::string detail) noexcept { return Error(ErrorKind::io, std::move(detail)); }
  static Error separator(std::uint8_t byte) noexcept {
    Error error(ErrorKind::separator, {});
    error.separator_ = byte;
    return error;
  }
  static Error plan(std::size_t column) noexcept {
    Error error(ErrorKind::plan, {});
    error.column_ = column;
    return error;
  }
  static Error no_sheet(std::string which) noexcept {
    return Error(ErrorKind::no_sheet, std::move(which));
  }
  static Error no_column(std::string name) noexcept {
    return Error(ErrorKind::no_column, std::move(name));
  }
  static Error unbound() noexcept { return Error(ErrorKind::unbound, {}); }
  static Error already_bound() noexcept { return Error(ErrorKind::already_bound, {}); }
  static Error structure(const Failure &failure) noexcept {
    Error error(ErrorKind::structure, {});
    error.failure_ = failure;
    return error;
  }

  ErrorKind kind() const noexcept { return kind_; }
  // The failure, for ErrorKind::structure; a failure of kind container otherwise.
  const Failure &failure() const noexcept { return failure_; }
  // The plan column, for ErrorKind::plan.
  std::size_t column() const noexcept { return column_; }
  // The separator byte, for ErrorKind::separator.
  std::uint8_t separator() const noexcept { return separator_; }
  // What the read said (io), the sheet asked for (no_sheet), or the name (no_column).
  const std::string &detail() const noexcept { return detail_; }

  // The error in a sentence, as every binding words it.
  std::string message() const noexcept {
    const auto number = [](std::uint64_t value) noexcept { return std::to_string(value); };
    const Failure &f = failure_;
    const std::string where = "record " + number(f.record) + " (line " + number(f.line) +
                              ", byte " + number(f.byte) + ")";
    switch (kind_) {
    case ErrorKind::io:
      return "read failed: " + detail_;
    case ErrorKind::separator: {
      static const char hex[] = "0123456789ABCDEF";
      std::string text = "separator byte 0x";
      text += hex[separator_ >> 4];
      text += hex[separator_ & 0xF];
      return text + " is not tab or printable ASCII other than '\"'";
    }
    case ErrorKind::plan:
      return "plan column " + number(column_) +
             " declares a numeric format or an ordinal that cannot be honoured";
    case ErrorKind::no_sheet:
      return "the workbook has no sheet " + detail_;
    case ErrorKind::no_column:
      return "the header has no column named \"" + detail_ + "\"";
    case ErrorKind::unbound:
      return "the reader has no plan yet: bind one before reading";
    case ErrorKind::already_bound:
      return "the reader already has a plan: a plan is bound once";
    case ErrorKind::structure:
      break;
    }
    switch (f.kind) {
    case FailureKind::unclosed_quote:
      return where + ": the input ends inside a quoted cell";
    case FailureKind::column_count:
      return where + " has " + number(f.found) + " cells where the first has " + number(f.expected);
    case FailureKind::row_too_long:
      return where + " is larger than a reader will buffer";
    case FailureKind::not_a_zip:
      return "the container is not a zip file";
    case FailureKind::not_a_workbook:
      return "the container is not an XLSX, ODS or Numbers workbook";
    case FailureKind::shared_string:
      return "row " + number(f.line) + " names shared string " + number(f.found) + " of " +
             number(f.expected) + " (part " + number(f.record) + ", byte " + number(f.byte) + ")";
    default:
      return std::string(name(f.kind)) + " in part " + number(f.record) + " at byte " +
             number(f.byte) + " (row " + number(f.line) + ", detail " + number(f.found) + ")";
    }
  }

private:
  Error(ErrorKind kind, std::string detail) noexcept : kind_(kind), detail_(std::move(detail)) {}

  ErrorKind kind_;
  std::string detail_;
  Failure failure_{FailureKind::container, 0, 0, 0, 0, 0};
  std::size_t column_ = 0;
  std::uint8_t separator_ = 0;
};

// The outcome of an open, a bind or a read: exactly one of a value and an Error. Shaped as
// hypercast::Verdict is, for the same reasons:
//
//   ok()                       whether it is the value
//   value_if(), error_if()     a pointer to the case, or nullptr
//   value(), error()           the case, checked: the wrong one ends the program
//   match(on_value, on_error)  calls the one for its case and returns what it returns
//   to_expected()              std::expected<T, Error> (C++23 with <expected> only)
//
// No operator bool, as a Verdict has none: `if (auto *batch = result.value_if())` says which
// question is asked.
template <class T> class [[nodiscard]] Result {
public:
  using value_type = T;

  Result(T value) noexcept : storage_(std::in_place_index<0>, std::move(value)) {}
  Result(Error error) noexcept : storage_(std::in_place_index<1>, std::move(error)) {}

  bool ok() const noexcept { return storage_.index() == 0; }

  T *value_if() & noexcept { return std::get_if<0>(&storage_); }
  const T *value_if() const & noexcept { return std::get_if<0>(&storage_); }
  const Error *error_if() const & noexcept { return std::get_if<1>(&storage_); }
  // A pointer into a temporary would dangle at the end of the statement: keep the result.
  void value_if() && = delete;
  void error_if() && = delete;

  // Precondition: ok(). Otherwise the program ends.
  T &value() & noexcept { return *checked(value_if()); }
  const T &value() const & noexcept { return *checked(value_if()); }
  T &&value() && noexcept { return std::move(*checked(value_if())); }
  // Precondition: !ok(). Otherwise the program ends.
  const Error &error() const noexcept { return *checked(error_if()); }

  template <class OnValue, class OnError>
  decltype(auto)
  match(OnValue &&on_value,
        OnError &&on_error) & noexcept(std::is_nothrow_invocable_v<OnValue &&, T &> &&
                                       std::is_nothrow_invocable_v<OnError &&, const Error &>) {
    if (T *value = value_if()) {
      return std::forward<OnValue>(on_value)(*value);
    }
    return std::forward<OnError>(on_error)(*error_if());
  }

#ifdef HYPERTABULAR_HAS_EXPECTED
  std::expected<T, Error> to_expected() && noexcept {
    if (T *value = value_if()) {
      return std::expected<T, Error>(std::in_place, std::move(*value));
    }
    return std::expected<T, Error>(std::unexpect, *error_if());
  }
  operator std::expected<T, Error>() && noexcept { return std::move(*this).to_expected(); }
#endif

private:
  template <class P> static P *checked(P *pointer) noexcept {
    if (pointer == nullptr) {
      detail::contract_violation();
    }
    return pointer;
  }

  std::variant<T, Error> storage_;
};

// The outcome of what has no value: done, or an Error.
template <> class [[nodiscard]] Result<void> {
public:
  using value_type = void;

  Result() noexcept = default;
  Result(Error error) noexcept : error_(std::move(error)) {}

  bool ok() const noexcept { return !error_.has_value(); }
  const Error *error_if() const & noexcept { return error_ ? &*error_ : nullptr; }
  void error_if() && = delete;
  // Precondition: !ok(). Otherwise the program ends.
  const Error &error() const noexcept {
    if (!error_) {
      detail::contract_violation();
    }
    return *error_;
  }

  template <class OnValue, class OnError>
  decltype(auto) match(OnValue &&on_value, OnError &&on_error) const
      noexcept(std::is_nothrow_invocable_v<OnValue &&> &&
               std::is_nothrow_invocable_v<OnError &&, const Error &>) {
    if (!error_) {
      return std::forward<OnValue>(on_value)();
    }
    return std::forward<OnError>(on_error)(*error_);
  }

#ifdef HYPERTABULAR_HAS_EXPECTED
  std::expected<void, Error> to_expected() const noexcept {
    if (!error_) {
      return {};
    }
    return std::expected<void, Error>(std::unexpect, *error_);
  }
  operator std::expected<void, Error>() const noexcept { return to_expected(); }
#endif

private:
  std::optional<Error> error_;
};

// ---------------------------------------------------------------------------------------------
// What the caller declares.

// The door a column casts through, numbered as the core numbers it (HyperTabularColumnSpec).
enum class Door : std::uint32_t {
  boolean = 1,
  i8 = 2,
  i16 = 3,
  i32 = 4,
  i64 = 5,
  u8 = 6,
  u16 = 7,
  u32 = 8,
  u64 = 9,
  f32 = 10,
  f64 = 11,
  uuid = 12,
  timestamp = 13,
  unix_time = 14, // under a declared UnixPrecision
  date = 15,
  time = 16,
  duration = 17,
  text = 18, // the cell's bytes (or a typed workbook cell's canonical rendering), no cast
  decimal = 19,
  date_ordered = 20, // under a declared DateOrder
  datetime = 21,     // under a declared DateOrder
  excel_serial = 22  // under a declared ExcelEpoch
};

// One output column: the source column it reads (a zero-based ordinal), the door it casts
// through, and, for the numeric doors, the notation its text is written in. A plan is a list of
// these, and a projection: one source column may be read through several doors.
class Column {
public:
  static constexpr Column boolean(std::size_t ordinal) noexcept { return {ordinal, Door::boolean}; }
  static constexpr Column i8(std::size_t ordinal) noexcept { return {ordinal, Door::i8}; }
  static constexpr Column i16(std::size_t ordinal) noexcept { return {ordinal, Door::i16}; }
  static constexpr Column i32(std::size_t ordinal) noexcept { return {ordinal, Door::i32}; }
  static constexpr Column i64(std::size_t ordinal) noexcept { return {ordinal, Door::i64}; }
  static constexpr Column u8(std::size_t ordinal) noexcept { return {ordinal, Door::u8}; }
  static constexpr Column u16(std::size_t ordinal) noexcept { return {ordinal, Door::u16}; }
  static constexpr Column u32(std::size_t ordinal) noexcept { return {ordinal, Door::u32}; }
  static constexpr Column u64(std::size_t ordinal) noexcept { return {ordinal, Door::u64}; }
  static constexpr Column f32(std::size_t ordinal) noexcept { return {ordinal, Door::f32}; }
  static constexpr Column f64(std::size_t ordinal) noexcept { return {ordinal, Door::f64}; }
  static constexpr Column decimal(std::size_t ordinal) noexcept { return {ordinal, Door::decimal}; }
  static constexpr Column uuid(std::size_t ordinal) noexcept { return {ordinal, Door::uuid}; }
  static constexpr Column timestamp(std::size_t ordinal) noexcept {
    return {ordinal, Door::timestamp};
  }
  static constexpr Column date(std::size_t ordinal) noexcept { return {ordinal, Door::date}; }
  static constexpr Column time(std::size_t ordinal) noexcept { return {ordinal, Door::time}; }
  static constexpr Column duration(std::size_t ordinal) noexcept {
    return {ordinal, Door::duration};
  }
  static constexpr Column text(std::size_t ordinal) noexcept { return {ordinal, Door::text}; }
  // A Unix-epoch integer at the declared precision, never guessed from its magnitude.
  static constexpr Column unix_time(std::size_t ordinal, UnixPrecision precision) noexcept {
    return {ordinal, Door::unix_time, static_cast<std::uint32_t>(precision)};
  }
  // An Excel date serial under the declared date system.
  static constexpr Column excel_serial(std::size_t ordinal, ExcelEpoch epoch) noexcept {
    return {ordinal, Door::excel_serial, static_cast<std::uint32_t>(epoch)};
  }
  // A separated calendar date under the declared field order.
  static constexpr Column date_ordered(std::size_t ordinal, DateOrder order) noexcept {
    return {ordinal, Door::date_ordered, static_cast<std::uint32_t>(order)};
  }
  // A zone-less civil date-time under the declared field order.
  static constexpr Column datetime(std::size_t ordinal, DateOrder order) noexcept {
    return {ordinal, Door::datetime, static_cast<std::uint32_t>(order)};
  }

  // The same column, its numeric text read in `format`. The invariant notation otherwise.
  constexpr Column format(const NumFormat &format) const noexcept {
    Column column = *this;
    column.format_ = format;
    return column;
  }

  constexpr std::size_t ordinal() const noexcept { return ordinal_; }
  constexpr Door door() const noexcept { return door_; }
  // What the door declares: a UnixPrecision, a DateOrder or an ExcelEpoch's value; 0 otherwise.
  constexpr std::uint32_t param() const noexcept { return param_; }
  constexpr const NumFormat &format() const noexcept { return format_; }

private:
  constexpr Column(std::size_t ordinal, Door door, std::uint32_t param = 0) noexcept
      : ordinal_(ordinal), door_(door), param_(param), format_(NumFormat::invariant()) {}

  std::size_t ordinal_;
  Door door_;
  std::uint32_t param_;
  NumFormat format_;
};

// How delimited text is written. Nothing is sniffed.
struct Dialect {
  std::uint8_t separator = ','; // tab, or printable ASCII other than '"'
  bool quoting = true;          // '"' quotes cells (RFC 4180, "" for a literal quote)
  bool has_header = true;       // the first record names the columns
  bool skip_blank_lines = true; // a completely empty line is skipped, not a one-cell row

  static constexpr Dialect csv() noexcept { return Dialect{}; }
  static constexpr Dialect tsv() noexcept { return Dialect{'\t'}; }
  static constexpr Dialect psv() noexcept { return Dialect{'|'}; }

  constexpr Dialect with_quoting(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.quoting = on;
    return dialect;
  }
  constexpr Dialect with_header(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.has_header = on;
    return dialect;
  }
  constexpr Dialect with_blank_lines_skipped(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.skip_blank_lines = on;
    return dialect;
  }
};

// How a delimited reader sizes what it owns.
struct DelimitedOptions {
  std::size_t batch_rows = 4096;         // the most rows a batch holds (at least one)
  std::size_t buffer_bytes = 256 * 1024; // a file's or a source's buffer to begin with; it grows
                                         // to hold the longest record, up to 1 GiB
};

// The names a source declared, in order.
class Header {
public:
  std::size_t size() const noexcept { return names_.size(); }
  bool empty() const noexcept { return names_.empty(); }
  // Precondition: index < size(). Otherwise the program ends.
  const std::string &operator[](std::size_t index) const noexcept {
    if (index >= names_.size()) {
      detail::contract_violation();
    }
    return names_[index];
  }
  std::vector<std::string>::const_iterator begin() const noexcept { return names_.begin(); }
  std::vector<std::string>::const_iterator end() const noexcept { return names_.end(); }
  const std::vector<std::string> &names() const noexcept { return names_; }

  // The ordinal of the first column named `name`, byte for byte, or std::nullopt.
  std::optional<std::size_t> ordinal(std::string_view name) const noexcept {
    for (std::size_t i = 0; i < names_.size(); ++i) {
      if (names_[i] == name) {
        return i;
      }
    }
    return std::nullopt;
  }
  // The same, or ErrorKind::no_column.
  Result<std::size_t> require(std::string_view name) const noexcept {
    if (auto found = ordinal(name)) {
      return *found;
    }
    return Error::no_column(std::string(name));
  }

private:
  friend class DelimitedReader;
  friend class Sheet;
  std::vector<std::string> names_;
};

namespace detail {
inline constexpr std::size_t max_ordinal = (std::size_t{1} << 20) - 1;
inline constexpr std::size_t cell_budget = 4096 * 257;
inline constexpr std::size_t window_min = 64 * 1024;

// Where every buffer of a reader starts. A test sets each to 1 to run the grow-and-keep path
// on every call; nothing else needs to.
struct StartingSizes {
  std::size_t window = window_min;
  std::size_t arena = 4096;
  std::size_t cells = 64;
  bool presize = true; // size the strings' arena and a bound plan's cell table up front
};
inline StartingSizes starting_sizes;

inline std::size_t value_size(Door door) noexcept {
  switch (door) {
  case Door::boolean:
  case Door::i8:
  case Door::u8:
    return 1;
  case Door::i16:
  case Door::u16:
    return 2;
  case Door::i32:
  case Door::u32:
  case Door::f32:
    return 4;
  case Door::i64:
  case Door::u64:
  case Door::f64:
  case Door::time:
    return 8;
  case Door::uuid:
    return 16;
  case Door::timestamp:
  case Door::unix_time:
  case Door::excel_serial:
    return sizeof(HyperTabularTimestamp);
  case Door::date:
  case Door::date_ordered:
    return sizeof(HyperTabularDate);
  case Door::datetime:
    return sizeof(HyperTabularCivilDateTime);
  case Door::duration:
    return sizeof(HyperTabularDuration);
  case Door::decimal:
    return sizeof(HyperTabularDecimal);
  case Door::text:
    return sizeof(HyperTabularSpan);
  }
  return 16;
}

inline std::size_t span_len(const HyperTabularSpan &span) noexcept {
  return span.len & ~static_cast<std::uint32_t>(HyperTabularSpan_FLAG);
}
inline bool flagged(const HyperTabularSpan &span) noexcept {
  return (span.len & static_cast<std::uint32_t>(HyperTabularSpan_FLAG)) != 0;
}

inline Failure failure(const HyperTabularFailure &raw) noexcept {
  FailureKind kind = FailureKind::container;
  switch (raw.code) {
  case 1:
  case 2:
  case 3:
  case 16:
  case 18:
  case 19:
  case 20:
  case 21:
  case 22:
  case 23:
  case 24:
  case 25:
  case 26:
    kind = static_cast<FailureKind>(raw.code);
    break;
  default: // 17, and the reading of any code the core has yet to be given
    break;
  }
  return Failure{kind, raw.record, raw.line, raw.byte, raw.expected, raw.found};
}

// A column as the core takes it, or false for one it cannot be: an ordinal past the core's
// reach, or a format this library's HyperCast does not read (one a newer HyperCast declared
// with fields past this layout).
inline bool spec(const Column &column, HyperTabularColumnSpec &out) noexcept {
  if (column.ordinal() > max_ordinal) {
    return false;
  }
  out = HyperTabularColumnSpec{};
  out.ordinal = static_cast<std::uint32_t>(column.ordinal());
  out.door = static_cast<std::uint32_t>(column.door());
  out.param = column.param();
  const HyperCastRawNumFormat &raw = column.format().raw();
  const auto *bytes = reinterpret_cast<const unsigned char *>(&raw);
  constexpr std::size_t ours = sizeof(HyperTabularRawNumFormat);
  constexpr std::size_t theirs = sizeof(HyperCastRawNumFormat);
  static_assert(ours == HyperTabularRawNumFormat_SIZE, "the header's layout is the library's");
  for (std::size_t i = ours; i < theirs; ++i) {
    if (bytes[i] != 0) {
      return false;
    }
  }
  std::memcpy(&out.format, bytes, std::min(ours, theirs));
  out.format.size = std::min<std::uint32_t>(raw.size, HyperTabularRawNumFormat_SIZE);
  return true;
}

// The plan's specs, its width (the widest ordinal plus one) and the column that sets it, or
// the column that cannot be honoured.
inline Result<std::size_t> specs(const std::vector<Column> &plan,
                                 std::vector<HyperTabularColumnSpec> &out,
                                 std::size_t &widest) noexcept {
  out.assign(plan.size(), HyperTabularColumnSpec{});
  std::size_t width = 0;
  widest = 0;
  for (std::size_t i = 0; i < plan.size(); ++i) {
    if (!spec(plan[i], out[i])) {
      return Error::plan(i);
    }
    if (plan[i].ordinal() + 1 > width) {
      width = plan[i].ordinal() + 1;
      widest = i;
    }
  }
  return width;
}

inline Result<void> within(const std::vector<Column> &plan,
                           std::optional<std::size_t> width) noexcept {
  if (width) {
    for (std::size_t i = 0; i < plan.size(); ++i) {
      if (plan[i].ordinal() >= *width) {
        return Error::plan(i);
      }
    }
  }
  return {};
}

inline std::size_t cell_rows(std::size_t per_row, std::size_t batch_rows) noexcept {
  return std::clamp<std::size_t>(cell_budget / std::max<std::size_t>(per_row, 1), 1,
                                 std::max<std::size_t>(batch_rows, 1));
}

// One column's room: batch_rows values of its door, held as u64s so they are aligned for any
// of them, and as many verdicts.
struct Store {
  std::vector<std::uint64_t> values;
  std::vector<HyperTabularCellVerdict> verdicts;
};

// A bound plan and the buffers it is read into.
struct Columns {
  std::vector<Column> plan;
  std::vector<HyperTabularColumnSpec> specs;
  std::vector<Store> stores;
  std::vector<HyperTabularColumnBuffer> pointers;
  std::size_t batch_rows = 0;

  Columns(std::vector<Column> columns, std::vector<HyperTabularColumnSpec> declared,
          std::size_t rows) noexcept
      : plan(std::move(columns)), specs(std::move(declared)), batch_rows(rows) {
    stores.resize(plan.size());
    pointers.resize(plan.size());
    for (std::size_t i = 0; i < plan.size(); ++i) {
      stores[i].values.assign((value_size(plan[i].door()) * batch_rows + 7) / 8, 0);
      stores[i].verdicts.assign(batch_rows, HyperTabularCellVerdict{});
      pointers[i] = HyperTabularColumnBuffer{stores[i].values.data(), stores[i].verdicts.data()};
    }
  }
};

enum class Origin { delimited, workbook };
} // namespace detail

// ---------------------------------------------------------------------------------------------
// What a read hands out.

class Batch;

// One row of a batch: the batch read across instead of down.
class Row {
public:
  // The row's index in its batch.
  std::size_t index() const noexcept { return index_; }
  // Where the row came from: for delimited text the one-based line its record starts on, for a
  // sheet its one-based row number.
  inline std::uint32_t line() const noexcept;
  template <class T> Verdict<T> get(std::size_t column) const noexcept;
  inline Verdict<std::string_view> text(std::size_t column) const noexcept;
  inline std::string raw(std::size_t column) const noexcept;
  inline std::optional<Fault> fault(std::size_t column) const noexcept;

private:
  friend class Batch;
  Row(const Batch *batch, std::size_t index) noexcept : batch_(batch), index_(index) {}
  const Batch *batch_;
  std::size_t index_;
};

// The rows of one read, column-major: for each plan column the values its door made and a
// verdict beside each. A view of the reader's buffers, valid until its next read().
class Batch {
public:
  std::size_t rows() const noexcept { return rows_; }
  std::size_t columns() const noexcept { return columns_->plan.size(); }
  const std::vector<Column> &plan() const noexcept { return columns_->plan; }

  // Where row `row` came from: for delimited text the one-based line its record starts on,
  // for a sheet its one-based row number.
  std::uint32_t line(std::size_t row) const noexcept {
    check_row(row);
    const HyperTabularSpan &entry = cells_[row * per_row_ + per_row_ - 1];
    return origin_ == detail::Origin::delimited ? entry.len : entry.offset;
  }

  // The cell at `row` of column `column`, as its door made it: a value, or HyperCast's fault.
  // T is the type the column's door makes: bool, std::int8_t to std::uint64_t, float and
  // double for the door of the same name; Decimal; Uuid; Timestamp for timestamp, unix_time
  // and excel_serial; Date for date and date_ordered; CivilDateTime for datetime;
  // std::chrono::nanoseconds (or std::uint64_t) for time, since midnight; Duration. Text is
  // read with text(). Any other T ends the program.
  template <class T> Verdict<T> get(std::size_t column, std::size_t row) const noexcept {
    const detail::Store &store = this->store(column);
    check_row(row);
    const Door door = columns_->plan[column].door();
    if (!reads<T>(door)) {
      detail::contract_violation();
    }
    if (auto fault = fault_of(store.verdicts[row])) {
      return *fault;
    }
    return value<T>(store, door, row);
  }

  // The cell at `row` of a text column: its bytes, untrimmed, or an Empty fault for a cell with
  // none. A typed workbook cell is said the canonical way: 42, true, 2024-01-31T10:30:00.
  Verdict<std::string_view> text(std::size_t column, std::size_t row) const noexcept {
    const detail::Store &store = this->store(column);
    check_row(row);
    if (columns_->plan[column].door() != Door::text) {
      detail::contract_violation();
    }
    if (auto fault = fault_of(store.verdicts[row])) {
      return *fault;
    }
    HyperTabularSpan span;
    std::memcpy(&span,
                reinterpret_cast<const unsigned char *>(store.values.data()) + row * sizeof span,
                sizeof span);
    return bytes(span);
  }

  // The cell's fault, or std::nullopt for a cell that cast.
  std::optional<Fault> fault(std::size_t column, std::size_t row) const noexcept {
    const detail::Store &store = this->store(column);
    check_row(row);
    return fault_of(store.verdicts[row]);
  }

  // The text of the cell at `row` of column `column`, whatever its door and its verdict: what a
  // fault's span is a span of. A copy, since quoted delimited text with a doubled quote in it
  // has to be unescaped to be read. For a workbook, a typed cell that cast has no text, and
  // neither has an empty cell.
  std::string raw(std::size_t column, std::size_t row) const noexcept {
    store(column);
    check_row(row);
    const std::size_t at =
        origin_ == detail::Origin::delimited ? columns_->plan[column].ordinal() : column;
    const HyperTabularSpan cell = cells_[row * per_row_ + at];
    if (origin_ == detail::Origin::workbook) {
      return std::string(bytes(cell));
    }
    const std::size_t from = cell.offset;
    const std::size_t length = detail::span_len(cell);
    if (from + length > base_.size()) {
      return std::string();
    }
    if (!detail::flagged(cell)) {
      return std::string(base_.substr(from, length));
    }
    std::string text(length, '\0');
    const auto *quoted = reinterpret_cast<const std::uint8_t *>(base_.data() + from);
    const std::size_t written = ::hypertabular_delimited_unescape(
        quoted, length, reinterpret_cast<std::uint8_t *>(&text[0]), text.size());
    text.resize(written);
    return text;
  }

  // Precondition: row < rows(). Otherwise the program ends.
  Row row(std::size_t row) const noexcept {
    check_row(row);
    return Row(this, row);
  }

  class iterator {
  public:
    using iterator_category = std::forward_iterator_tag;
    using value_type = Row;
    using difference_type = std::ptrdiff_t;
    using pointer = void;
    using reference = Row;

    Row operator*() const noexcept { return batch_->row(index_); }
    iterator &operator++() noexcept {
      ++index_;
      return *this;
    }
    iterator operator++(int) noexcept {
      iterator before = *this;
      ++index_;
      return before;
    }
    friend bool operator==(const iterator &a, const iterator &b) noexcept {
      return a.index_ == b.index_ && a.batch_ == b.batch_;
    }
    friend bool operator!=(const iterator &a, const iterator &b) noexcept { return !(a == b); }

  private:
    friend class Batch;
    iterator(const Batch *batch, std::size_t index) noexcept : batch_(batch), index_(index) {}
    const Batch *batch_;
    std::size_t index_;
  };

  iterator begin() const noexcept { return iterator(this, 0); }
  iterator end() const noexcept { return iterator(this, rows_); }

private:
  friend class DelimitedReader;
  friend class Sheet;

  Batch(const detail::Columns *columns, std::size_t rows, const HyperTabularSpan *cells,
        std::size_t per_row, std::string_view base, std::string_view arena,
        detail::Origin origin) noexcept
      : columns_(columns), rows_(rows), cells_(cells), per_row_(per_row), base_(base),
        arena_(arena), origin_(origin) {}

  const detail::Store &store(std::size_t column) const noexcept {
    if (column >= columns_->stores.size()) {
      detail::contract_violation();
    }
    return columns_->stores[column];
  }
  void check_row(std::size_t row) const noexcept {
    if (row >= rows_) {
      detail::contract_violation();
    }
  }

  static std::optional<Fault> fault_of(const HyperTabularCellVerdict &verdict) noexcept {
    if (verdict.reason == 0) {
      return std::nullopt;
    }
    return Fault{static_cast<CastFailure>(verdict.reason), verdict.offset, verdict.len};
  }

  std::string_view bytes(const HyperTabularSpan &span) const noexcept {
    const std::string_view store = detail::flagged(span) ? arena_ : base_;
    const std::size_t from = span.offset;
    const std::size_t length = detail::span_len(span);
    if (from > store.size() || length > store.size() - from) {
      return std::string_view();
    }
    return store.substr(from, length);
  }

  template <class T> static constexpr bool reads(Door door) noexcept {
    if constexpr (std::is_same_v<T, bool>) {
      return door == Door::boolean;
    } else if constexpr (std::is_same_v<T, std::int8_t>) {
      return door == Door::i8;
    } else if constexpr (std::is_same_v<T, std::int16_t>) {
      return door == Door::i16;
    } else if constexpr (std::is_same_v<T, std::int32_t>) {
      return door == Door::i32;
    } else if constexpr (std::is_same_v<T, std::int64_t>) {
      return door == Door::i64;
    } else if constexpr (std::is_same_v<T, std::uint8_t>) {
      return door == Door::u8;
    } else if constexpr (std::is_same_v<T, std::uint16_t>) {
      return door == Door::u16;
    } else if constexpr (std::is_same_v<T, std::uint32_t>) {
      return door == Door::u32;
    } else if constexpr (std::is_same_v<T, std::uint64_t>) {
      return door == Door::u64 || door == Door::time;
    } else if constexpr (std::is_same_v<T, float>) {
      return door == Door::f32;
    } else if constexpr (std::is_same_v<T, double>) {
      return door == Door::f64;
    } else if constexpr (std::is_same_v<T, Decimal>) {
      return door == Door::decimal;
    } else if constexpr (std::is_same_v<T, Uuid>) {
      return door == Door::uuid;
    } else if constexpr (std::is_same_v<T, Timestamp>) {
      return door == Door::timestamp || door == Door::unix_time || door == Door::excel_serial;
    } else if constexpr (std::is_same_v<T, Date>) {
      return door == Door::date || door == Door::date_ordered;
    } else if constexpr (std::is_same_v<T, CivilDateTime>) {
      return door == Door::datetime;
    } else if constexpr (std::is_same_v<T, std::chrono::nanoseconds>) {
      return door == Door::time;
    } else if constexpr (std::is_same_v<T, Duration>) {
      return door == Door::duration;
    } else {
      static_assert(!std::is_same_v<T, T>, "no door makes this type; text is read with text()");
      return false;
    }
  }

  template <class Raw> static Raw load(const detail::Store &store, std::size_t row) noexcept {
    Raw raw;
    std::memcpy(&raw,
                reinterpret_cast<const unsigned char *>(store.values.data()) + row * sizeof(Raw),
                sizeof(Raw));
    return raw;
  }

  template <class T>
  static T value(const detail::Store &store, Door door, std::size_t row) noexcept {
    (void)door;
    if constexpr (std::is_same_v<T, bool>) {
      return load<std::uint8_t>(store, row) != 0;
    } else if constexpr (std::is_arithmetic_v<T>) {
      return load<T>(store, row);
    } else if constexpr (std::is_same_v<T, Decimal>) {
      const auto raw = load<HyperTabularDecimal>(store, row);
      return Decimal{raw.lo, raw.hi, raw.scale, raw.negative};
    } else if constexpr (std::is_same_v<T, Uuid>) {
      return load<Uuid>(store, row);
    } else if constexpr (std::is_same_v<T, Timestamp>) {
      const auto raw = load<HyperTabularTimestamp>(store, row);
      return Timestamp{raw.seconds, raw.nanos};
    } else if constexpr (std::is_same_v<T, Date>) {
      const auto raw = load<HyperTabularDate>(store, row);
      return Date{raw.year, raw.month, raw.day};
    } else if constexpr (std::is_same_v<T, CivilDateTime>) {
      const auto raw = load<HyperTabularCivilDateTime>(store, row);
      return CivilDateTime{Date{raw.date.year, raw.date.month, raw.date.day}, raw.nanos_of_day};
    } else if constexpr (std::is_same_v<T, std::chrono::nanoseconds>) {
      return std::chrono::nanoseconds(
          static_cast<std::chrono::nanoseconds::rep>(load<std::uint64_t>(store, row)));
    } else {
      const auto raw = load<HyperTabularDuration>(store, row);
      return Duration{raw.seconds, raw.nanos};
    }
  }

  const detail::Columns *columns_;
  std::size_t rows_;
  const HyperTabularSpan *cells_;
  std::size_t per_row_;
  std::string_view base_;
  std::string_view arena_;
  detail::Origin origin_;
};

inline std::uint32_t Row::line() const noexcept { return batch_->line(index_); }
template <class T> Verdict<T> Row::get(std::size_t column) const noexcept {
  return batch_->get<T>(column, index_);
}
inline Verdict<std::string_view> Row::text(std::size_t column) const noexcept {
  return batch_->text(column, index_);
}
inline std::string Row::raw(std::size_t column) const noexcept {
  return batch_->raw(column, index_);
}
inline std::optional<Fault> Row::fault(std::size_t column) const noexcept {
  return batch_->fault(column, index_);
}

// ---------------------------------------------------------------------------------------------
// Delimited text.

// A forward-only reader of delimited text: opened over bytes in memory, a file or a source,
// its header read (when the dialect has one), then bound to a plan once and read a batch at a
// time. Move-only.
class DelimitedReader {
public:
  // Reads up to `capacity` bytes into `into`: how many it read, 0 at the end of the input, or
  // std::nullopt for a read that failed (ErrorKind::io).
  using Source =
      std::function<std::optional<std::size_t>(std::uint8_t *into, std::size_t capacity)>;

  // The longest record a file or a source is buffered for.
  static constexpr std::size_t max_row_bytes = std::size_t{1} << 30;

  // Over `input`, read in place: the bytes must outlive the reader.
  static Result<DelimitedReader> from_bytes(std::string_view input, Dialect dialect,
                                            DelimitedOptions options = {}) noexcept {
    DelimitedReader reader(dialect, options);
    reader.slice_ = input;
    return start(std::move(reader), nullptr);
  }
  static Result<DelimitedReader> from_bytes(std::string_view input, Dialect dialect,
                                            const std::vector<Column> &plan,
                                            DelimitedOptions options = {}) noexcept {
    DelimitedReader reader(dialect, options);
    reader.slice_ = input;
    return start(std::move(reader), &plan);
  }

  // Over a source, read through a buffer that grows to hold the longest record.
  static Result<DelimitedReader> from_source(Source source, Dialect dialect,
                                             DelimitedOptions options = {}) noexcept {
    return streamed(std::move(source), dialect, options, nullptr);
  }
  static Result<DelimitedReader> from_source(Source source, Dialect dialect,
                                             const std::vector<Column> &plan,
                                             DelimitedOptions options = {}) noexcept {
    return streamed(std::move(source), dialect, options, &plan);
  }

  // Over the file at `path`, which the reader keeps open until it is destroyed.
  static Result<DelimitedReader> open(const char *path, Dialect dialect,
                                      DelimitedOptions options = {}) noexcept {
    return opened(path, dialect, options, nullptr);
  }
  static Result<DelimitedReader> open(const char *path, Dialect dialect,
                                      const std::vector<Column> &plan,
                                      DelimitedOptions options = {}) noexcept {
    return opened(path, dialect, options, &plan);
  }

  DelimitedReader(DelimitedReader &&) noexcept = default;
  DelimitedReader &operator=(DelimitedReader &&) noexcept = default;
  DelimitedReader(const DelimitedReader &) = delete;
  DelimitedReader &operator=(const DelimitedReader &) = delete;

  // Declares the plan of a reader opened without one, once, before the first read; held to the
  // width a header has given every record. ErrorKind::already_bound, or ErrorKind::plan.
  Result<void> bind(std::vector<Column> plan) noexcept {
    if (columns_) {
      return Error::already_bound();
    }
    std::vector<HyperTabularColumnSpec> specs;
    std::size_t widest = 0;
    auto width = detail::specs(plan, specs, widest);
    if (!width.ok()) {
      return width.error();
    }
    if (auto inside = detail::within(plan, column_count()); !inside.ok()) {
      return inside;
    }
    const std::size_t per_row = *width.value_if() + 1;
    const std::size_t rows = detail::cell_rows(per_row, options_.batch_rows);
    cells_.assign(detail::starting_sizes.presize ? per_row * rows : detail::starting_sizes.cells,
                  HyperTabularSpan{});
    per_row_ = per_row;
    columns_ =
        std::make_unique<detail::Columns>(std::move(plan), std::move(specs), options_.batch_rows);
    return {};
  }
  Result<void> bind(std::initializer_list<Column> plan) noexcept {
    return bind(std::vector<Column>(plan));
  }

  bool is_bound() const noexcept { return columns_ != nullptr; }
  const Dialect &dialect() const noexcept { return dialect_; }
  // The plan the reader reads through: empty until one is bound.
  const std::vector<Column> &plan() const noexcept {
    static const std::vector<Column> none;
    return columns_ ? columns_->plan : none;
  }
  // The header's names, or nullptr if the dialect declares no header. A header with no names
  // is an input that had no record.
  const Header *header() const noexcept { return header_ ? &*header_ : nullptr; }
  // How many cells a record has, once the header or the first record says.
  std::optional<std::size_t> column_count() const noexcept {
    if (state().expected == 0) {
      return std::nullopt;
    }
    return static_cast<std::size_t>(state().expected);
  }
  // How many records have been read, the header among them.
  std::uint64_t records() const noexcept { return state().records; }

  // The next batch: up to the batch size of rows, or std::nullopt when the input has no more.
  // The batch is a view, over when the next is asked for. A structural failure is returned
  // once every intact row before it has been delivered, and again on every call after.
  Result<std::optional<Batch>> read() noexcept {
    if (error_) {
      return *error_;
    }
    if (!columns_) {
      return Error::unbound();
    }
    advance(pending_);
    pending_ = 0;
    if (cramped_) {
      cramped_ = false;
      arena_.resize(arena_.size() * 2);
    }
    for (;;) {
      auto step = fill_step();
      if (!step.ok()) {
        return step.error();
      }
      const std::ptrdiff_t rows = *step.value_if();
      if (rows > 0) {
        return std::optional<Batch>(Batch(
            columns_.get(), static_cast<std::size_t>(rows), cells_.data(), per_row_, window().first,
            std::string_view(reinterpret_cast<const char *>(arena_.data()), arena_.size()),
            detail::Origin::delimited));
      }
      if (rows == 0) {
        return std::optional<Batch>();
      }
      if (auto more = refill(); !more.ok()) {
        return more.error();
      }
    }
  }

  // Reads every row left, batch by batch, handing each to `f`: f(const Row &).
  template <class F> Result<void> for_each_row(F &&f) noexcept {
    for (;;) {
      auto batch = read();
      if (!batch.ok()) {
        return batch.error();
      }
      if (!*batch.value_if()) {
        return {};
      }
      for (const Row &row : **batch.value_if()) {
        f(row);
      }
    }
  }

private:
  DelimitedReader(Dialect dialect, DelimitedOptions options) noexcept
      : dialect_(dialect), options_(options) {
    options_.batch_rows = std::max<std::size_t>(options_.batch_rows, 1);
    options_.buffer_bytes =
        std::min(std::max<std::size_t>(options_.buffer_bytes, 1), max_row_bytes);
    state_.assign((::hypertabular_delimited_state_size() + 7) / 8, 0);
    arena_.assign(detail::starting_sizes.arena, 0);
  }

  HyperTabularDelimitedState &state() noexcept {
    return *reinterpret_cast<HyperTabularDelimitedState *>(state_.data());
  }
  const HyperTabularDelimitedState &state() const noexcept {
    return *reinterpret_cast<const HyperTabularDelimitedState *>(state_.data());
  }

  static Result<DelimitedReader> streamed(Source source, Dialect dialect, DelimitedOptions options,
                                          const std::vector<Column> *plan) noexcept {
    DelimitedReader reader(dialect, options);
    reader.source_ = std::move(source);
    reader.limit_ = reader.options_.buffer_bytes;
    reader.stream_ = true;
    return start(std::move(reader), plan);
  }

  static Result<DelimitedReader> opened(const char *path, Dialect dialect, DelimitedOptions options,
                                        const std::vector<Column> *plan) noexcept {
    std::FILE *file = detail::open_file(path);
    if (file == nullptr) {
      return Error::io(std::string("cannot open ") + path);
    }
    std::shared_ptr<std::FILE> handle(file, [](std::FILE *f) { std::fclose(f); });
    Source source = [handle](std::uint8_t *into,
                             std::size_t capacity) noexcept -> std::optional<std::size_t> {
      const std::size_t read = std::fread(into, 1, capacity, handle.get());
      if (read == 0 && std::ferror(handle.get())) {
        return std::nullopt;
      }
      return read;
    };
    return streamed(std::move(source), dialect, options, plan);
  }

  static Result<DelimitedReader> start(DelimitedReader reader,
                                       const std::vector<Column> *plan) noexcept {
    const HyperTabularRawDialect raw{
        reader.dialect_.separator, static_cast<std::uint8_t>(reader.dialect_.quoting ? 1 : 0),
        static_cast<std::uint8_t>(reader.dialect_.skip_blank_lines), 0};
    if (::hypertabular_delimited_init(&reader.state(), &raw) != HYPERTABULAR_OK) {
      return Error::separator(reader.dialect_.separator);
    }
    if (plan != nullptr) {
      if (auto bound = reader.bind(*plan); !bound.ok()) {
        return bound.error();
      }
    }
    if (reader.dialect_.has_header) {
      std::vector<HyperTabularSpan> names(detail::starting_sizes.cells);
      for (;;) {
        auto step = reader.header_step(names);
        if (!step.ok()) {
          return step.error();
        }
        if (*step.value_if()) {
          break;
        }
        if (auto more = reader.refill(); !more.ok()) {
          return more.error();
        }
      }
    }
    if (auto inside = detail::within(reader.plan(), reader.column_count()); !inside.ok()) {
      return inside.error();
    }
    return std::move(reader);
  }

  // The unconsumed input, and whether it is the last of it.
  std::pair<std::string_view, bool> window() const noexcept {
    if (!stream_) {
      const std::string_view rest = slice_.substr(at_);
      constexpr std::size_t most = 0x7FFFFFFF; // the core's offsets are 31 bits
      if (rest.size() > most) {
        return {rest.substr(0, most), false};
      }
      return {rest, true};
    }
    return {std::string_view(reinterpret_cast<const char *>(buffer_.data()) + at_,
                             buffer_.size() - at_),
            eof_};
  }

  void advance(std::size_t consumed) noexcept { at_ += consumed; }

  Error fail(Error error) noexcept {
    error_ = error;
    return error;
  }

  Error row_too_long() noexcept {
    return fail(Error::structure(
        Failure{FailureKind::row_too_long, state().records, state().line, state().offset, 0, 0}));
  }

  // Moves the unconsumed tail to the front of the buffer and reads more behind it, growing
  // the buffer when the tail fills it, up to max_row_bytes.
  Result<void> refill() noexcept {
    if (!stream_) {
      return row_too_long();
    }
    buffer_.erase(buffer_.begin(), buffer_.begin() + static_cast<std::ptrdiff_t>(at_));
    at_ = 0;
    if (buffer_.size() >= limit_) {
      if (limit_ >= max_row_bytes) {
        return row_too_long();
      }
      limit_ = std::min(limit_ * 2, max_row_bytes);
    }
    const std::size_t filled = buffer_.size();
    buffer_.resize(limit_);
    const auto read = source_(buffer_.data() + filled, limit_ - filled);
    if (!read) {
      buffer_.resize(filled);
      return fail(Error::io("the source failed"));
    }
    if (*read == 0) {
      eof_ = true;
    }
    buffer_.resize(filled + std::min(*read, limit_ - filled));
    return {};
  }

  HyperTabularBuffers buffers() noexcept {
    HyperTabularBuffers buffers{};
    buffers.arena = arena_.data();
    buffers.arena_cap = arena_.size();
    buffers.cells = cells_.data();
    buffers.cells_cap = cells_.size();
    return buffers;
  }

  // The header from what is buffered: true once read, false when the core needs more input.
  Result<bool> header_step(std::vector<HyperTabularSpan> &names) noexcept {
    for (;;) {
      const auto [input, last] = window();
      HyperTabularFilled out{};
      const std::int32_t code = ::hypertabular_delimited_header(
          &state(), reinterpret_cast<const std::uint8_t *>(input.data()), input.size(),
          last ? 1 : 0, names.data(), names.size(), arena_.data(), arena_.size(), &out);
      switch (code) {
      case HYPERTABULAR_OK: {
        const auto consumed = static_cast<std::size_t>(out.consumed);
        if (out.rows > 0) {
          Header header;
          for (std::size_t i = 0; i < out.rows; ++i) {
            const HyperTabularSpan &name = names[i];
            const std::string_view from =
                detail::flagged(name)
                    ? std::string_view(reinterpret_cast<const char *>(arena_.data()), arena_.size())
                    : input;
            header.names_.emplace_back(from.substr(name.offset, detail::span_len(name)));
          }
          header_ = std::move(header);
          advance(consumed);
          return true;
        }
        advance(consumed);
        if (last && (consumed == input.size() || consumed == 0)) {
          header_ = Header(); // an empty input: no header and no rows
          return true;
        }
        if (consumed == 0) {
          return false;
        }
        break;
      }
      case HYPERTABULAR_ERR_CELLS:
        names.resize(static_cast<std::size_t>(out.needed));
        break;
      case HYPERTABULAR_ERR_ARENA:
        arena_.resize(static_cast<std::size_t>(out.needed));
        break;
      case HYPERTABULAR_ERR_STRUCTURE:
        return fail(Error::structure(detail::failure(out.failure)));
      default:
        return fail(Error::separator(dialect_.separator));
      }
    }
  }

  // A batch from what is buffered: its rows, 0 at the end of the input, -1 for more input.
  Result<std::ptrdiff_t> fill_step() noexcept {
    const std::size_t max_rows = columns_->batch_rows;
    for (;;) {
      const auto [input, last] = window();
      const HyperTabularBuffers memory = buffers();
      HyperTabularFilled out{};
      const std::int32_t code = ::hypertabular_delimited_fill(
          &state(), reinterpret_cast<const std::uint8_t *>(input.data()), input.size(),
          last ? 1 : 0, columns_->specs.data(), columns_->pointers.data(), columns_->plan.size(),
          max_rows, &memory, &out);
      switch (code) {
      case HYPERTABULAR_OK: {
        const auto consumed = static_cast<std::size_t>(out.consumed);
        if (out.rows > 0) {
          const auto rows = static_cast<std::size_t>(out.rows);
          pending_ = consumed;
          // A batch the cell table ends early is no reason to grow the arena.
          const std::size_t held = std::min(max_rows, cells_.size() / per_row_);
          cramped_ = rows < held && out.arena_used * 2 >= arena_.size();
          return static_cast<std::ptrdiff_t>(rows);
        }
        advance(consumed);
        if (last && (consumed == input.size() || consumed == 0)) {
          return std::ptrdiff_t{0};
        }
        if (consumed == 0) {
          return std::ptrdiff_t{-1};
        }
        break;
      }
      case HYPERTABULAR_ERR_CELLS: {
        const auto needed = static_cast<std::size_t>(out.needed);
        per_row_ = std::max(per_row_, needed);
        cells_.resize(needed * detail::cell_rows(needed, max_rows));
        break;
      }
      case HYPERTABULAR_ERR_ARENA:
        arena_.resize(std::max(static_cast<std::size_t>(out.needed), arena_.size() * 2));
        break;
      case HYPERTABULAR_ERR_STRUCTURE:
        return fail(Error::structure(detail::failure(out.failure)));
      default:
        return fail(Error::separator(dialect_.separator));
      }
    }
  }

  Dialect dialect_;
  DelimitedOptions options_;
  std::vector<std::uint64_t> state_;
  // A slice: the bytes, read in place. A stream: the buffer, its limit, the source.
  bool stream_ = false;
  std::string_view slice_;
  std::vector<std::uint8_t> buffer_;
  std::size_t limit_ = 0;
  Source source_;
  bool eof_ = false;
  std::size_t at_ = 0;
  // Bytes the batch last lent covers: consumed when the next read begins.
  std::size_t pending_ = 0;
  std::unique_ptr<detail::Columns> columns_;
  std::size_t per_row_ = 0;
  std::vector<HyperTabularSpan> cells_;
  std::vector<std::uint8_t> arena_;
  bool cramped_ = false;
  std::optional<Header> header_;
  std::optional<Error> error_;
};

// ---------------------------------------------------------------------------------------------
// Workbooks.

enum class Format : std::uint32_t { xlsx = 1, ods = 2, numbers = 3 };

// One sheet as the workbook lists it.
class SheetInfo {
public:
  const std::string &name() const noexcept { return name_; }
  bool hidden() const noexcept { return hidden_; }

private:
  friend class Workbook;
  friend class Sheet;
  std::string name_;
  bool hidden_ = false;
  std::string part_;
  std::uint32_t index_ = 0;
};

// How a sheet is read.
struct SheetOptions {
  bool has_header = true;        // the first row names the columns
  bool skip_empty_rows = true;   // a row with no cells is not delivered
  std::size_t batch_rows = 4096; // the most rows a batch holds (at least one)

  constexpr SheetOptions with_header(bool on) const noexcept {
    SheetOptions options = *this;
    options.has_header = on;
    return options;
  }
  constexpr SheetOptions with_empty_rows_skipped(bool on) const noexcept {
    SheetOptions options = *this;
    options.skip_empty_rows = on;
    return options;
  }
  constexpr SheetOptions with_batch_rows(std::size_t rows) const noexcept {
    SheetOptions options = *this;
    options.batch_rows = rows == 0 ? 1 : rows;
    return options;
  }
};

namespace detail {
// The memory a workbook call works in, and the loop that grows it: a call that stops for room
// is made again, the buffer it named grown and its contents kept.
struct Scratch {
  std::vector<std::uint8_t> window;
  std::vector<std::uint8_t> arena;
  std::vector<HyperTabularSpan> cells;

  struct Tables {
    const std::vector<std::uint8_t> *strings = nullptr;
    const std::vector<HyperTabularSpan> *table = nullptr;
    const std::vector<std::uint8_t> *kinds = nullptr;
  };

  HyperTabularBuffers buffers(std::vector<HyperTabularSlot> &row, Tables tables) noexcept {
    HyperTabularBuffers buffers{};
    buffers.window = window.data();
    buffers.window_cap = window.size();
    buffers.arena = arena.data();
    buffers.arena_cap = arena.size();
    buffers.cells = cells.data();
    buffers.cells_cap = cells.size();
    buffers.row = row.data();
    buffers.row_cap = row.size();
    if (tables.strings != nullptr) {
      buffers.strings = tables.strings->data();
      buffers.strings_len = tables.strings->size();
      buffers.table = tables.table->data();
      buffers.table_len = tables.table->size();
      buffers.kinds = tables.kinds->data();
      buffers.kinds_len = tables.kinds->size();
    }
    return buffers;
  }

  template <class Call>
  std::pair<std::int32_t, HyperTabularFilled> drive(std::vector<HyperTabularSlot> &row,
                                                    bool row_follows_cells, Tables tables,
                                                    Call call) noexcept {
    for (;;) {
      HyperTabularFilled out{};
      const HyperTabularBuffers memory = buffers(row, tables);
      const std::int32_t code = call(&memory, &out);
      const auto needed = static_cast<std::size_t>(out.needed);
      switch (code) {
      case HYPERTABULAR_ERR_WINDOW:
        window.resize(std::max(needed, window.size() + 1));
        break;
      case HYPERTABULAR_ERR_ARENA:
        arena.resize(std::max(needed, arena.size() + 1));
        break;
      case HYPERTABULAR_ERR_CELLS:
        cells.resize(std::max(needed, cells.size() + 1));
        if (row_follows_cells && row.size() < cells.size()) {
          row.resize(cells.size());
        }
        break;
      default:
        return {code, out};
      }
    }
  }
};

// OK, or the structural failure; the core refusing its own binding's call is a bug here.
inline Result<void> settle(std::int32_t code, const HyperTabularFilled &out) noexcept {
  if (code == HYPERTABULAR_OK) {
    return {};
  }
  if (code == HYPERTABULAR_ERR_STRUCTURE) {
    return Error::structure(failure(out.failure));
  }
  contract_violation();
}

inline std::vector<std::uint64_t> state_block() noexcept {
  return std::vector<std::uint64_t>((::hypertabular_workbook_state_size() + 7) / 8, 0);
}

inline HyperTabularWorkbookState *state_of(std::vector<std::uint64_t> &block) noexcept {
  return reinterpret_cast<HyperTabularWorkbookState *>(block.data());
}
} // namespace detail

class Sheet;

// An XLSX, ODS or Numbers workbook, opened whole: its sheets listed, its shared strings and
// cell formats loaded. Move-only; its sheets read from it, so it outlives them.
class Workbook {
public:
  // Over `container`, read in place: the bytes must outlive the workbook.
  static Result<Workbook> from_bytes(std::string_view container) noexcept {
    return build(Workbook(container));
  }
  // Over bytes the workbook keeps.
  static Result<Workbook> from_vector(std::vector<std::uint8_t> bytes) noexcept {
    Workbook book(std::string_view{});
    book.owned_ = std::make_unique<std::vector<std::uint8_t>>(std::move(bytes));
    book.container_ =
        std::string_view(reinterpret_cast<const char *>(book.owned_->data()), book.owned_->size());
    return build(std::move(book));
  }
  // The file at `path`, read whole.
  static Result<Workbook> open(const char *path) noexcept {
    std::FILE *file = detail::open_file(path);
    if (file == nullptr) {
      return Error::io(std::string("cannot open ") + path);
    }
    std::vector<std::uint8_t> bytes;
    std::uint8_t chunk[64 * 1024];
    std::size_t read = 0;
    while ((read = std::fread(chunk, 1, sizeof chunk, file)) > 0) {
      bytes.insert(bytes.end(), chunk, chunk + read);
    }
    const bool failed = std::ferror(file) != 0;
    std::fclose(file);
    if (failed) {
      return Error::io(std::string("cannot read ") + path);
    }
    return from_vector(std::move(bytes));
  }

  Workbook(Workbook &&) noexcept = default;
  Workbook &operator=(Workbook &&) noexcept = default;
  Workbook(const Workbook &) = delete;
  Workbook &operator=(const Workbook &) = delete;

  Format format() const noexcept { return format_; }
  // The workbook's date system: 1904 for a workbook that says so, 1900 otherwise.
  ExcelEpoch date_system() const noexcept { return epoch_; }
  const std::vector<SheetInfo> &sheets() const noexcept { return sheets_; }

  // A sheet by its index in sheets() or by its name, its header read (when the options say it
  // has one) and the plan bound. ErrorKind::no_sheet, ErrorKind::plan or a structural failure.
  inline Result<Sheet> sheet(std::size_t index, SheetOptions options,
                             const std::vector<Column> &plan) const noexcept;
  inline Result<Sheet> sheet(std::string_view name, SheetOptions options,
                             const std::vector<Column> &plan) const noexcept;
  // The same, with no plan yet: read the header, then bind.
  inline Result<Sheet> sheet_unbound(std::size_t index, SheetOptions options = {}) const noexcept;
  inline Result<Sheet> sheet_unbound(std::string_view name,
                                     SheetOptions options = {}) const noexcept;

private:
  friend class Sheet;
  explicit Workbook(std::string_view container) noexcept : container_(container) {}

  static Result<Workbook> build(Workbook book) noexcept {
    const auto *bytes = reinterpret_cast<const std::uint8_t *>(book.container_.data());
    const std::size_t size = book.container_.size();
    book.state_ = detail::state_block();
    HyperTabularWorkbookState *state = detail::state_of(book.state_);
    const detail::StartingSizes &start = detail::starting_sizes;
    detail::Scratch scratch;
    scratch.window.assign(start.window, 0);
    scratch.arena.assign(std::min<std::size_t>(start.arena, 1024), 0);
    scratch.cells.assign(start.cells, HyperTabularSpan{});
    std::vector<HyperTabularSlot> no_row;

    HyperTabularOpened opened{};
    for (;;) {
      opened = HyperTabularOpened{};
      const HyperTabularBuffers memory = scratch.buffers(no_row, {});
      const std::int32_t code = ::hypertabular_workbook_open(state, bytes, size, &memory, &opened);
      const auto needed = static_cast<std::size_t>(opened.needed);
      if (code == HYPERTABULAR_OK) {
        break;
      }
      if (code == HYPERTABULAR_ERR_WINDOW) {
        scratch.window.resize(std::max(needed, scratch.window.size() + 1));
      } else if (code == HYPERTABULAR_ERR_ARENA) {
        scratch.arena.resize(std::max(needed, scratch.arena.size() + 1));
      } else if (code == HYPERTABULAR_ERR_STRUCTURE) {
        return Error::structure(detail::failure(opened.failure));
      } else {
        detail::contract_violation();
      }
    }
    book.format_ = opened.format == 1   ? Format::xlsx
                   : opened.format == 3 ? Format::numbers
                                        : Format::ods;
    book.epoch_ = opened.epoch == 2 ? ExcelEpoch::y1904 : ExcelEpoch::y1900;

    auto [code, out] = scratch.drive(no_row, false, {}, [&](const auto *memory, auto *result) {
      return ::hypertabular_workbook_sheets(state, bytes, size, memory, result);
    });
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled.error();
    }
    const auto text = [&](const HyperTabularSpan &span) {
      return std::string(reinterpret_cast<const char *>(scratch.arena.data()) + span.offset,
                         detail::span_len(span));
    };
    for (std::size_t i = 0; i < out.rows; ++i) {
      SheetInfo info;
      info.name_ = text(scratch.cells[i * 3]);
      info.part_ = text(scratch.cells[i * 3 + 1]);
      info.hidden_ = (scratch.cells[i * 3 + 2].offset & 1) != 0;
      info.index_ = scratch.cells[i * 3 + 2].len;
      book.sheets_.push_back(std::move(info));
    }

    if (start.presize) {
      const auto bound =
          static_cast<std::size_t>(std::min<std::uint64_t>(opened.strings_bytes, 1u << 28));
      if (scratch.arena.size() < bound) {
        scratch.arena.resize(bound);
      }
    }
    std::tie(code, out) = scratch.drive(no_row, false, {}, [&](const auto *memory, auto *result) {
      return ::hypertabular_workbook_strings(state, bytes, size, memory, result);
    });
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled.error();
    }
    book.strings_.assign(scratch.arena.begin(),
                         scratch.arena.begin() + static_cast<std::ptrdiff_t>(out.arena_used));
    book.table_.assign(scratch.cells.begin(),
                       scratch.cells.begin() + static_cast<std::ptrdiff_t>(out.rows));

    std::tie(code, out) = scratch.drive(no_row, false, {}, [&](const auto *memory, auto *result) {
      return ::hypertabular_workbook_styles(state, bytes, size, memory, result);
    });
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled.error();
    }
    book.kinds_.assign(scratch.arena.begin(),
                       scratch.arena.begin() + static_cast<std::ptrdiff_t>(out.rows));
    return std::move(book);
  }

  std::unique_ptr<std::vector<std::uint8_t>> owned_;
  std::string_view container_;
  std::vector<std::uint64_t> state_;
  Format format_ = Format::xlsx;
  ExcelEpoch epoch_ = ExcelEpoch::y1900;
  std::vector<SheetInfo> sheets_;
  std::vector<std::uint8_t> strings_;
  std::vector<HyperTabularSpan> table_;
  std::vector<std::uint8_t> kinds_;
};

// One sheet of a workbook, read forward a batch at a time. Move-only; its workbook outlives it.
class Sheet {
public:
  Sheet(Sheet &&) noexcept = default;
  Sheet &operator=(Sheet &&) noexcept = default;
  Sheet(const Sheet &) = delete;
  Sheet &operator=(const Sheet &) = delete;

  const SheetOptions &options() const noexcept { return options_; }
  // The header row's names, or nullptr if the options say the sheet has none.
  const Header *header() const noexcept { return header_ ? &*header_ : nullptr; }
  bool is_bound() const noexcept { return columns_ != nullptr; }
  const std::vector<Column> &plan() const noexcept {
    static const std::vector<Column> none;
    return columns_ ? columns_->plan : none;
  }

  // Declares the plan of a sheet opened without one, once, before the first read.
  // ErrorKind::already_bound, or ErrorKind::plan.
  Result<void> bind(std::vector<Column> plan) noexcept {
    if (columns_) {
      return Error::already_bound();
    }
    std::vector<HyperTabularColumnSpec> specs;
    std::size_t widest = 0;
    auto width = detail::specs(plan, specs, widest);
    if (!width.ok()) {
      return width.error();
    }
    const std::size_t batch_rows = std::max<std::size_t>(options_.batch_rows, 1);
    const std::size_t per_row = plan.size() + 1;
    if (detail::starting_sizes.presize && scratch_.cells.size() < batch_rows * per_row) {
      scratch_.cells.resize(batch_rows * per_row);
    }
    if (row_.size() < *width.value_if()) {
      row_.resize(*width.value_if());
    }
    per_row_ = per_row;
    columns_ = std::make_unique<detail::Columns>(std::move(plan), std::move(specs), batch_rows);
    return {};
  }
  Result<void> bind(std::initializer_list<Column> plan) noexcept {
    return bind(std::vector<Column>(plan));
  }

  // The next batch, or std::nullopt when the sheet has no more rows. A structural failure is
  // returned once every intact row before it has been delivered, and again on every call
  // after.
  Result<std::optional<Batch>> read() noexcept {
    if (!columns_) {
      return Error::unbound();
    }
    if (pending_) {
      error_ = std::move(pending_);
      pending_.reset();
    }
    if (error_) {
      return *error_;
    }
    const Workbook &book = *book_;
    HyperTabularWorkbookState *state = detail::state_of(state_);
    const auto *bytes = reinterpret_cast<const std::uint8_t *>(book.container_.data());
    const std::size_t size = book.container_.size();
    detail::Columns &columns = *columns_;
    auto [code, out] = scratch_.drive(
        row_, false, tables(), [&](const HyperTabularBuffers *memory, HyperTabularFilled *result) {
          return ::hypertabular_workbook_fill(state, bytes, size, columns.specs.data(),
                                              columns.pointers.data(), columns.plan.size(),
                                              columns.batch_rows, memory, result);
        });
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      if (out.rows == 0) {
        error_ = settled.error();
        return *error_;
      }
      pending_ = settled.error();
    }
    if (out.rows == 0) {
      return std::optional<Batch>();
    }
    return std::optional<Batch>(
        Batch(columns_.get(), static_cast<std::size_t>(out.rows), scratch_.cells.data(), per_row_,
              std::string_view(reinterpret_cast<const char *>(book.strings_.data()),
                               book.strings_.size()),
              std::string_view(reinterpret_cast<const char *>(scratch_.arena.data()),
                               scratch_.arena.size()),
              detail::Origin::workbook));
  }

  // Reads every row left, batch by batch, handing each to `f`: f(const Row &).
  template <class F> Result<void> for_each_row(F &&f) noexcept {
    for (;;) {
      auto batch = read();
      if (!batch.ok()) {
        return batch.error();
      }
      if (!*batch.value_if()) {
        return {};
      }
      for (const Row &row : **batch.value_if()) {
        f(row);
      }
    }
  }

private:
  friend class Workbook;
  Sheet(const Workbook *book, SheetOptions options) noexcept : book_(book), options_(options) {}

  detail::Scratch::Tables tables() const noexcept {
    return detail::Scratch::Tables{&book_->strings_, &book_->table_, &book_->kinds_};
  }

  static Result<Sheet> open(const Workbook &book, const SheetInfo &info, SheetOptions options,
                            const std::vector<Column> *plan) noexcept {
    Sheet sheet(&book, options);
    sheet.state_ = book.state_; // an opened block may be copied, bytes and all
    const detail::StartingSizes &start = detail::starting_sizes;
    sheet.scratch_.arena.assign(start.arena, 0);
    sheet.scratch_.cells.assign(start.cells, HyperTabularSpan{});
    if (plan != nullptr) {
      if (auto bound = sheet.bind(*plan); !bound.ok()) {
        return bound.error();
      }
    }
    HyperTabularFilled out{};
    const std::int32_t code = ::hypertabular_workbook_sheet(
        detail::state_of(sheet.state_),
        reinterpret_cast<const std::uint8_t *>(book.container_.data()), book.container_.size(),
        reinterpret_cast<const std::uint8_t *>(info.part_.data()), info.part_.size(), info.index_,
        options.has_header ? 1 : 0, options.skip_empty_rows ? 1 : 0, &out);
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled.error();
    }
    if (options.has_header) {
      if (auto read = sheet.read_header(); !read.ok()) {
        return read.error();
      }
    }
    return std::move(sheet);
  }

  Result<void> read_header() noexcept {
    const Workbook &book = *book_;
    HyperTabularWorkbookState *state = detail::state_of(state_);
    const bool unbound = columns_ == nullptr;
    if (unbound && row_.size() < scratch_.cells.size()) {
      row_.resize(scratch_.cells.size());
    }
    auto [code, out] =
        scratch_.drive(row_, unbound, tables(),
                       [&](const HyperTabularBuffers *memory, HyperTabularFilled *result) {
                         return ::hypertabular_workbook_header(
                             state, reinterpret_cast<const std::uint8_t *>(book.container_.data()),
                             book.container_.size(), memory, result);
                       });
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled;
    }
    Header header;
    for (std::size_t i = 0; i < out.rows; ++i) {
      const HyperTabularSpan &name = scratch_.cells[i];
      const auto &from = detail::flagged(name) ? scratch_.arena : book.strings_;
      const std::size_t start = name.offset;
      const std::size_t length = detail::span_len(name);
      header.names_.emplace_back(reinterpret_cast<const char *>(from.data()) + start, length);
    }
    header_ = std::move(header);
    return {};
  }

  const Workbook *book_;
  SheetOptions options_;
  std::vector<std::uint64_t> state_;
  detail::Scratch scratch_;
  std::vector<HyperTabularSlot> row_;
  std::size_t per_row_ = 0;
  std::unique_ptr<detail::Columns> columns_;
  std::optional<Header> header_;
  std::optional<Error> pending_;
  std::optional<Error> error_;
};

inline Result<Sheet> Workbook::sheet(std::size_t index, SheetOptions options,
                                     const std::vector<Column> &plan) const noexcept {
  if (index >= sheets_.size()) {
    return Error::no_sheet("at index " + std::to_string(index));
  }
  return Sheet::open(*this, sheets_[index], options, &plan);
}
inline Result<Sheet> Workbook::sheet(std::string_view name, SheetOptions options,
                                     const std::vector<Column> &plan) const noexcept {
  for (const SheetInfo &info : sheets_) {
    if (info.name_ == name) {
      return Sheet::open(*this, info, options, &plan);
    }
  }
  return Error::no_sheet("named \"" + std::string(name) + "\"");
}
inline Result<Sheet> Workbook::sheet_unbound(std::size_t index,
                                             SheetOptions options) const noexcept {
  if (index >= sheets_.size()) {
    return Error::no_sheet("at index " + std::to_string(index));
  }
  return Sheet::open(*this, sheets_[index], options, nullptr);
}
inline Result<Sheet> Workbook::sheet_unbound(std::string_view name,
                                             SheetOptions options) const noexcept {
  for (const SheetInfo &info : sheets_) {
    if (info.name_ == name) {
      return Sheet::open(*this, info, options, nullptr);
    }
  }
  return Error::no_sheet("named \"" + std::string(name) + "\"");
}

// ---------------------------------------------------------------------------------------------

// Always true: the core is linked into the program, so there is nothing to fail to load. Kept
// for parity with the bindings that load a library at run time.
constexpr bool is_available() noexcept { return true; }

// The linked library's own version, "major.minor.patch", from ::hypertabular_version().
inline std::string_view native_version() noexcept {
  struct Rendered {
    char text[16] = {};
    std::size_t size = 0;
    explicit Rendered(std::uint32_t packed) noexcept {
      const std::uint32_t parts[3] = {packed >> 16, (packed >> 8) & 0xFF, packed & 0xFF};
      for (std::size_t i = 0; i < 3; ++i) {
        if (i > 0) {
          text[size++] = '.';
        }
        char digits[5];
        std::size_t count = 0;
        std::uint32_t part = parts[i];
        do {
          digits[count++] = static_cast<char>('0' + part % 10);
          part /= 10;
        } while (part != 0);
        while (count > 0) {
          text[size++] = digits[--count];
        }
      }
    }
  };
  static const Rendered rendered(::hypertabular_version());
  return std::string_view(rendered.text, rendered.size);
}

} // namespace hypertabular

#endif // HYPERTABULAR_HPP
