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
//   auto id = header.require("id");                // refuses a name two columns share
//   if (!id.ok()) return report(id.error());
//   auto bound = reader.bind({hypertabular::Column::i64(id.value()),
//                             hypertabular::Column::text(1)});
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
// column or a row index outside the batch, a door read as a type it does not make, a column that
// declares constraints read by get() or text() rather than get_checked(), the value of a result
// that holds an error, a faults walk past the next read()) ends the program with std::abort(), as
// Rust's panic and Swift's precondition do.
//
// A batch is a view of the reader's buffers, valid until the next read() and while the reader
// lives, as any C++ view is, and otherwise as C#'s, Java's, Go's and Swift's are. Fault spans count
// bytes of the cell's UTF-8. Nothing is sniffed: a read's dialect, encoding, tokens, plan and sheet
// options are all the caller's to declare, and suggest() is an answer to look at, never applied.

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
#include <tuple>
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
// values the doors make, and what a column declares: among it the constraints a checked column
// declares, and the Checked verdict it reads back.
using hypercast::CastFailure;
using hypercast::Checked;
using hypercast::CheckedValue;
using hypercast::CivilDateTime;
using hypercast::Constraints;
using hypercast::ConstraintValue;
using hypercast::Date;
using hypercast::DateOptions;
using hypercast::DateOrder;
using hypercast::DateStyles;
using hypercast::Decimal;
using hypercast::Detail;
using hypercast::Duration;
using hypercast::EnumStorage;
using hypercast::ExcelEpoch;
using hypercast::Fault;
using hypercast::Keyword;
using hypercast::MonthName;
using hypercast::MonthNames;
using hypercast::NumFormat;
using hypercast::NumStyles;
using hypercast::optional;
using hypercast::Rejection;
using hypercast::Rounding;
using hypercast::Timestamp;
using hypercast::UnixPrecision;
using hypercast::Uuid;
using hypercast::Verdict;
using hypercast::Violation;

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
  // value of a result that holds an error, a faults walk past the next read(), a reader destroyed
  // or moved by what its own read calls.
  std::abort();
}

// Whether a reader or a sheet is in the middle of a read: its source or a for_each_row callback
// runs inside one, and a read or bind from there would move or free what the read is using. Held
// first among the reader's members, so that a move from or onto a busy one ends the program before
// anything is moved; the reader's destructor ends it too, before its source is destroyed under
// itself. Neither can be refused: there is no result to say so in.
class Busy {
public:
  Busy() noexcept = default;
  Busy(Busy &&other) noexcept {
    if (other.on_) {
      contract_violation();
    }
  }
  Busy &operator=(Busy &&other) noexcept {
    if (on_ || other.on_) {
      contract_violation();
    }
    return *this;
  }
  Busy(const Busy &) = delete;
  Busy &operator=(const Busy &) = delete;
  ~Busy() = default;

  bool on() const noexcept { return on_; }
  void check() const noexcept {
    if (on_) {
      contract_violation();
    }
  }

  // Marks a read in progress for as long as it lives.
  class Hold {
  public:
    explicit Hold(Busy &busy) noexcept : busy_(busy) { busy_.on_ = true; }
    ~Hold() { busy_.on_ = false; }
    Hold(const Hold &) = delete;
    Hold &operator=(const Hold &) = delete;

  private:
    Busy &busy_;
  };

private:
  bool on_ = false;
};

// A list the core reads as packed bytes, shared by every copy of what declares it, so the
// address a column spec holds stays good however the plan is copied or moved. Null is none.
using Packed = std::shared_ptr<const std::vector<std::uint8_t>>;

// Tokens as the core takes them: each a little-endian 32-bit length, then its bytes.
template <class Iterator> inline Packed pack_tokens(Iterator first, Iterator last) noexcept {
  auto packed = std::make_shared<std::vector<std::uint8_t>>();
  for (; first != last; ++first) {
    const std::string_view token = *first;
    if (static_cast<std::uint64_t>(token.size()) > UINT32_MAX) {
      contract_violation();
    }
    const auto length = static_cast<std::uint32_t>(token.size());
    for (int shift = 0; shift < 32; shift += 8) {
      packed->push_back(static_cast<std::uint8_t>(length >> shift));
    }
    packed->insert(packed->end(), token.begin(), token.end());
  }
  return packed;
}

// What the core reads for a list: a real address even when it is empty, since a null means no
// list at all, and on a column that means the dialect's or the sheet's.
inline const std::uint8_t *packed_data(const Packed &packed) noexcept {
  static const std::uint8_t nothing = 0;
  if (packed == nullptr) {
    return nullptr;
  }
  return packed->empty() ? &nothing : packed->data();
}
inline std::size_t packed_size(const Packed &packed) noexcept {
  return packed == nullptr ? 0 : packed->size();
}

// The entry at `index` of a text enum packed as HyperCast packs one, each a little-endian 32-bit
// length, then its bytes: the entry the core matched, which the list therefore holds.
inline std::string_view packed_entry(const Packed &packed, std::uint32_t index) noexcept {
  const std::uint8_t *data = packed_data(packed);
  const std::size_t size = packed_size(packed);
  std::size_t at = 0;
  for (std::uint32_t i = 0;; ++i) {
    if (size - at < 4) {
      contract_violation(); // the core matched an entry the list doesn't hold
    }
    const std::size_t length =
        static_cast<std::size_t>(data[at]) | static_cast<std::size_t>(data[at + 1]) << 8 |
        static_cast<std::size_t>(data[at + 2]) << 16 | static_cast<std::size_t>(data[at + 3]) << 24;
    at += 4;
    if (size - at < length) {
      contract_violation();
    }
    if (i == index) {
      return std::string_view(reinterpret_cast<const char *>(data + at), length);
    }
    at += length;
  }
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
  iwa = 26,            // a Numbers part is not what the IWA packaging says
  encoding = 27        // text a declared encoding has no character for, or that isn't UTF-8 where
                       // no cell's verdict can carry it
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
  case FailureKind::encoding:
    return "encoding";
  }
  return "container";
}

// Where the data broke. Delimited text: `record` is the offending record's zero-based index,
// header and skipped blank lines included, `line` the one-based line it starts on, `byte` its
// absolute offset, and for column_count `expected` and `found` the two cell counts. Under a
// declared encoding `byte` counts the decoded text, except for an encoding failure's, which is
// its unit's offset in the original input. A workbook: `record` is the part being read
// (HyperTabularFailure_PART_SHEET and the rest), `line` the sheet row, `byte` the offset in the
// part's inflated bytes, and `expected` and `found` what the kind says they are.
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
  io,                // the file or the source could not be read; detail() says why
  separator,         // the dialect's separator is not tab or printable ASCII other than '"'
  plan,              // plan column column() cannot be honoured: its ordinal, format or date options
  no_sheet,          // the workbook has no such sheet; detail() says which was asked for
  no_column,         // the header has no column of that name; detail() is the name
  ambiguous_column,  // two columns share the name: detail() is the name, first() and second() where
  range,             // not a sheet range; detail() is the text
  unbound,           // the reader has no plan yet: bind one before reading
  already_bound,     // the reader already has a plan: a plan is bound once
  structure,         // the data is broken: failure() says what and where
  zero_batch_rows,   // batch_rows is 0, which reads nothing
  zero_buffer_bytes, // buffer_bytes is 0
  batch_too_large,   // a batch asks for needed() bytes, past the core's limit()
  row_too_wide,      // one row of width() columns asks for needed() bytes, past the core's limit()
  busy               // a read or bind from inside its own read; detail() says reader or sheet
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
  static Error ambiguous_column(std::string name, std::size_t first, std::size_t second) noexcept {
    Error error(ErrorKind::ambiguous_column, std::move(name));
    error.first_ = first;
    error.second_ = second;
    return error;
  }
  static Error range(std::string text) noexcept { return Error(ErrorKind::range, std::move(text)); }
  static Error unbound() noexcept { return Error(ErrorKind::unbound, {}); }
  static Error already_bound() noexcept { return Error(ErrorKind::already_bound, {}); }
  static Error busy(std::string which) noexcept { return Error(ErrorKind::busy, std::move(which)); }
  static Error zero_batch_rows() noexcept { return Error(ErrorKind::zero_batch_rows, {}); }
  static Error zero_buffer_bytes() noexcept { return Error(ErrorKind::zero_buffer_bytes, {}); }
  static Error batch_too_large(std::uint64_t needed, std::uint64_t limit) noexcept {
    Error error(ErrorKind::batch_too_large, {});
    error.needed_ = needed;
    error.limit_ = limit;
    return error;
  }
  static Error row_too_wide(std::uint64_t width, std::uint64_t needed,
                            std::uint64_t limit) noexcept {
    Error error(ErrorKind::row_too_wide, {});
    error.width_ = width;
    error.needed_ = needed;
    error.limit_ = limit;
    return error;
  }
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
  // For ErrorKind::ambiguous_column, the zero-based positions of the first two columns with the
  // name, counted as a plan's ordinals are: from a sheet range's first column.
  std::size_t first() const noexcept { return first_; }
  std::size_t second() const noexcept { return second_; }
  // For ErrorKind::batch_too_large and row_too_wide, the bytes asked for (UINT64_MAX for an ask too
  // big to count) and the most the core allows, and for row_too_wide the columns one row holds.
  std::uint64_t needed() const noexcept { return needed_; }
  std::uint64_t limit() const noexcept { return limit_; }
  std::uint64_t width() const noexcept { return width_; }
  // What the read said (io), the sheet asked for (no_sheet), the name (no_column,
  // ambiguous_column), the text that is no range (range), or "reader" or "sheet" (busy).
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
             " declares a numeric format, date options or an ordinal that cannot be honoured";
    case ErrorKind::no_sheet:
      return "the workbook has no sheet " + detail_;
    case ErrorKind::no_column:
      return "the header has no column named \"" + detail_ + "\"";
    case ErrorKind::ambiguous_column:
      return "the header names two columns \"" + detail_ + "\", at positions " + number(first_) +
             " and " + number(second_) + ": say which by position";
    case ErrorKind::range:
      return "not a range: \"" + detail_ + "\"";
    case ErrorKind::zero_batch_rows:
      return "batch_rows must be at least 1";
    case ErrorKind::zero_buffer_bytes:
      return "buffer_bytes must be at least 1";
    case ErrorKind::batch_too_large:
      return "batch_rows asks for " + number(needed_) + " bytes, past the limit of " +
             number(limit_);
    case ErrorKind::row_too_wide:
      return "a row of " + number(width_) + " columns asks for " + number(needed_) +
             " bytes, past the limit of " + number(limit_);
    case ErrorKind::unbound:
      return "the reader has no plan yet: bind one before reading";
    case ErrorKind::already_bound:
      return "the reader already has a plan: a plan is bound once";
    case ErrorKind::busy:
      return "the " + detail_ +
             " is in the middle of a read; nothing that read calls can call back into it";
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
    case FailureKind::encoding:
      return "text that isn't valid in its encoding at byte " + number(f.byte) +
             " (record or part " + number(f.record) + ", line or row " + number(f.line) + ")";
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
  std::size_t first_ = 0;
  std::size_t second_ = 0;
  std::uint64_t needed_ = 0;
  std::uint64_t limit_ = 0;
  std::uint64_t width_ = 0;
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

class Batch;
class Column;

namespace detail {
inline bool spec(const Column &column, HyperTabularColumnSpecWith &out) noexcept;
} // namespace detail

// One output column: the source column it reads (a zero-based ordinal), the door it casts
// through, and, for the numeric doors, the notation its text is written in. A plan is a list of
// these, and a projection: one source column may be read through several doors.
class Column {
public:
  static Column boolean(std::size_t ordinal) noexcept { return {ordinal, Door::boolean}; }
  static Column i8(std::size_t ordinal) noexcept { return {ordinal, Door::i8}; }
  static Column i16(std::size_t ordinal) noexcept { return {ordinal, Door::i16}; }
  static Column i32(std::size_t ordinal) noexcept { return {ordinal, Door::i32}; }
  static Column i64(std::size_t ordinal) noexcept { return {ordinal, Door::i64}; }
  static Column u8(std::size_t ordinal) noexcept { return {ordinal, Door::u8}; }
  static Column u16(std::size_t ordinal) noexcept { return {ordinal, Door::u16}; }
  static Column u32(std::size_t ordinal) noexcept { return {ordinal, Door::u32}; }
  static Column u64(std::size_t ordinal) noexcept { return {ordinal, Door::u64}; }
  static Column f32(std::size_t ordinal) noexcept { return {ordinal, Door::f32}; }
  static Column f64(std::size_t ordinal) noexcept { return {ordinal, Door::f64}; }
  static Column decimal(std::size_t ordinal) noexcept { return {ordinal, Door::decimal}; }
  static Column uuid(std::size_t ordinal) noexcept { return {ordinal, Door::uuid}; }
  static Column timestamp(std::size_t ordinal) noexcept { return {ordinal, Door::timestamp}; }
  static Column date(std::size_t ordinal) noexcept { return {ordinal, Door::date}; }
  static Column time(std::size_t ordinal) noexcept { return {ordinal, Door::time}; }
  static Column duration(std::size_t ordinal) noexcept { return {ordinal, Door::duration}; }
  static Column text(std::size_t ordinal) noexcept { return {ordinal, Door::text}; }
  // A Unix-epoch integer at the declared precision, never guessed from its magnitude.
  static Column unix_time(std::size_t ordinal, UnixPrecision precision) noexcept {
    return {ordinal, Door::unix_time, static_cast<std::uint32_t>(precision)};
  }
  // An Excel date serial under the declared date system.
  static Column excel_serial(std::size_t ordinal, ExcelEpoch epoch) noexcept {
    return {ordinal, Door::excel_serial, static_cast<std::uint32_t>(epoch)};
  }
  // A separated calendar date under the declared field order.
  static Column date_ordered(std::size_t ordinal, DateOrder order) noexcept {
    return {ordinal, Door::date_ordered, static_cast<std::uint32_t>(order)};
  }
  // A zone-less civil date-time under the declared field order.
  static Column datetime(std::size_t ordinal, DateOrder order) noexcept {
    return {ordinal, Door::datetime, static_cast<std::uint32_t>(order)};
  }
  // A column by its door and what the door declares (param(), which a report of suggest() says).
  // A door or a param the core doesn't know is ErrorKind::plan when the plan is bound.
  static Column of(std::size_t ordinal, Door door, std::uint32_t param = 0) noexcept {
    return {ordinal, door, param};
  }
  // A column whose cells must also meet `constraints`, read through HyperCast's checked door for
  // T, the door T names: std::int8_t to std::uint64_t, float, double, Decimal, or std::string_view
  // for text. Batch::get_checked<T> reads each cell: its value, flagged when multipleOf's rounding
  // adjusted it; a Violation of a declared rule, carrying the value as cast; or the cell's Fault,
  // a cell no door was asked about included. The plain getters refuse the column. The declaration
  // and its enum list are copied here, so its EnumStorage needn't outlive this call; the numeric
  // notation, tokens and date options are set as on any column.
  template <class T>
  static Column checked(std::size_t ordinal, const Constraints<T> &constraints) noexcept {
    static_assert(sizeof(HyperCastRawConstraints) == sizeof(HyperTabularRawConstraints),
                  "HyperCast's constraints layout is the one the core reads");
    Column column(ordinal, static_cast<Door>(Constraints<T>::door));
    auto raw = std::make_shared<HyperTabularRawConstraints>();
    std::memcpy(raw.get(), &constraints.raw(), sizeof *raw);
    column.constraints_ = std::move(raw);
    const std::uint8_t *entries = constraints.enum_data();
    column.enum_ = std::make_shared<const std::vector<std::uint8_t>>(
        entries, entries == nullptr ? entries : entries + constraints.enum_size());
    return column;
  }

  // The same column, its numeric text read in `format`, this column's alone. With none,
  // NumFormat::invariant(), as every binding reads no format: Western grouping, so `1,234,567`
  // reads and `1,23,456` is malformed.
  // `NumFormat::invariant().with_grouping(hypercast::Grouping::indian)` reads that shape and
  // refuses the other, beside a Western column in the same plan.
  Column format(const NumFormat &format) const noexcept {
    Column column = *this;
    column.format_ = format;
    return column;
  }
  // The same column with its own tokens, replacing the dialect's or the sheet's; an empty list
  // means none. A token is an unquoted cell's exact bytes, and that cell is Empty at Origin::token,
  // whatever the door. On a sheet only a text cell can be one. Each column keeps its own copy.
  Column tokens(std::initializer_list<std::string_view> tokens) const noexcept {
    Column column = *this;
    column.tokens_ = detail::pack_tokens(tokens.begin(), tokens.end());
    return column;
  }
  Column tokens(const std::vector<std::string> &tokens) const noexcept {
    Column column = *this;
    column.tokens_ = detail::pack_tokens(tokens.begin(), tokens.end());
    return column;
  }
  // The same column with what the date doors read: `options`, and a vocabulary for the month
  // field. The timestamp, date, date_ordered and datetime doors read them; on a sheet, for text
  // cells only, since a stored date has neither. The vocabulary's bytes are copied here, so its
  // storage needn't outlive this call.
  Column date_options(const DateOptions &options,
                      const MonthNames &months = MonthNames()) const noexcept {
    Column column = *this;
    column.date_ = options;
    column.months_ = months.empty() ? nullptr
                                    : std::make_shared<const std::vector<std::uint8_t>>(
                                          months.data(), months.data() + months.size());
    return column;
  }

  std::size_t ordinal() const noexcept { return ordinal_; }
  Door door() const noexcept { return door_; }
  // What the door declares: a UnixPrecision, a DateOrder or an ExcelEpoch's value; 0 otherwise.
  std::uint32_t param() const noexcept { return param_; }
  const NumFormat &format() const noexcept { return format_; }
  const DateOptions &date_options() const noexcept { return date_; }
  // Whether the column declares constraints: Column::checked.
  bool is_checked() const noexcept { return constraints_ != nullptr; }

private:
  friend bool detail::spec(const Column &column, HyperTabularColumnSpecWith &out) noexcept;
  friend class Batch;

  Column(std::size_t ordinal, Door door, std::uint32_t param = 0) noexcept
      : ordinal_(ordinal), door_(door), param_(param), format_(NumFormat::invariant()) {}

  std::size_t ordinal_;
  Door door_;
  std::uint32_t param_;
  NumFormat format_;
  DateOptions date_;
  detail::Packed months_;
  detail::Packed tokens_;
  std::shared_ptr<const HyperTabularRawConstraints> constraints_; // Column::checked's, or none
  detail::Packed enum_;                                           // and its enum list, packed
};

// A record with fewer cells than the width the first record fixed.
enum class ShortRows : std::uint32_t {
  reject = HyperTabularRawDelimitedOptions_SHORT_ROWS_REJECT, // the read's structural failure
  pad = HyperTabularRawDelimitedOptions_SHORT_ROWS_PAD        // the cells it lacks are Empty,
                                                              // at Origin::padded
};

// A record with more cells than the width the first record fixed.
enum class LongRows : std::uint32_t {
  reject = HyperTabularRawDelimitedOptions_LONG_ROWS_REJECT,    // the read's structural failure
  report = HyperTabularRawDelimitedOptions_LONG_ROWS_REPORT,    // cut to the width, and listed by
                                                                // Batch::faults()
  truncate = HyperTabularRawDelimitedOptions_LONG_ROWS_TRUNCATE // cut to the width, said nowhere
};

// What delimited text's bytes are. Never sniffed, so a byte-order mark is no declaration:
// Windows-1252 and Latin-1 have none, and EF BB BF is three characters there.
enum class Encoding : std::uint32_t {
  utf8 = 0,
  windows_1252 = HyperTabularRawTranscoded_WINDOWS_1252,
  latin_1 = HyperTabularRawTranscoded_LATIN_1,
  utf_16le = HyperTabularRawTranscoded_UTF_16LE
};

// How delimited text is written. Nothing is sniffed.
struct Dialect {
  std::uint8_t separator = ','; // tab, or printable ASCII other than '"'
  bool quoting = true;          // '"' quotes cells (RFC 4180, "" for a literal quote)
  bool has_header = true;       // the first record names the columns
  bool skip_blank_lines = true; // a completely empty line is skipped, not a one-cell row
  std::uint32_t skip = 0;       // physical lines to pass over after a byte-order mark, quotes
                                // meaning nothing in them
  std::uint32_t header_row = 0; // records to discard after those, ahead of the header; a blank
                                // line the dialect skips isn't one
  ShortRows short_rows = ShortRows::reject;
  LongRows long_rows = LongRows::reject;
  Encoding encoding = Encoding::utf8;
  detail::Packed tokens = nullptr; // set with with_tokens

  static Dialect csv() noexcept { return Dialect{}; }
  static Dialect tsv() noexcept { return Dialect{'\t'}; }
  static Dialect psv() noexcept { return Dialect{'|'}; }

  Dialect with_quoting(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.quoting = on;
    return dialect;
  }
  Dialect with_header(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.has_header = on;
    return dialect;
  }
  Dialect with_blank_lines_skipped(bool on) const noexcept {
    Dialect dialect = *this;
    dialect.skip_blank_lines = on;
    return dialect;
  }
  Dialect with_skip(std::uint32_t lines) const noexcept {
    Dialect dialect = *this;
    dialect.skip = lines;
    return dialect;
  }
  Dialect with_header_row(std::uint32_t records) const noexcept {
    Dialect dialect = *this;
    dialect.header_row = records;
    return dialect;
  }
  Dialect with_short_rows(ShortRows policy) const noexcept {
    Dialect dialect = *this;
    dialect.short_rows = policy;
    return dialect;
  }
  Dialect with_long_rows(LongRows policy) const noexcept {
    Dialect dialect = *this;
    dialect.long_rows = policy;
    return dialect;
  }
  Dialect with_encoding(Encoding declared) const noexcept {
    Dialect dialect = *this;
    dialect.encoding = declared;
    return dialect;
  }
  // The exact unquoted bytes that stand for no value, for every column: such a cell is Empty, at
  // Origin::token, on every door. A quoted cell never matches. Column::tokens replaces this.
  Dialect with_tokens(std::initializer_list<std::string_view> list) const noexcept {
    Dialect dialect = *this;
    dialect.tokens = detail::pack_tokens(list.begin(), list.end());
    return dialect;
  }
  Dialect with_tokens(const std::vector<std::string> &list) const noexcept {
    Dialect dialect = *this;
    dialect.tokens = detail::pack_tokens(list.begin(), list.end());
    return dialect;
  }
};

// How a delimited reader sizes what it owns.
struct DelimitedOptions {
  std::size_t batch_rows = 4096;         // the most rows a batch holds; the core refuses 0, and a
                                         // batch asking for more than it allows
  std::size_t buffer_bytes = 256 * 1024; // a file's or a source's buffer to begin with; it grows
                                         // to hold the longest record, up to 1 GiB; the core
                                         // refuses 0, and starts one past that there
  bool notes = true;                     // whether a batch carries detail() and origin()

  // Off, every cell says Detail::none and Origin::cell, faults() lists no formula with no cached
  // value, and a read saves the little it costs to ask a door why a cell failed.
  constexpr DelimitedOptions with_notes(bool on) const noexcept {
    DelimitedOptions options = *this;
    options.notes = on;
    return options;
  }
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

  // The ordinal of the first column named `name`, or std::nullopt. The match is exact, byte for
  // byte, case and spaces included, and of two columns with the name the first is found, with
  // nothing said of the other: require() is the lookup that says.
  std::optional<std::size_t> ordinal(std::string_view name) const noexcept {
    for (std::size_t i = 0; i < names_.size(); ++i) {
      if (names_[i] == name) {
        return i;
      }
    }
    return std::nullopt;
  }
  // The ordinal of the column named `name`, matched as ordinal() does: ErrorKind::no_column for a
  // name no column has, and ErrorKind::ambiguous_column, naming the first two positions, for a name
  // several share, so that a plan built from names doesn't read the wrong one. A plan can still
  // name either by position.
  Result<std::size_t> require(std::string_view name) const noexcept {
    const auto found = ordinal(name);
    if (!found) {
      return Error::no_column(std::string(name));
    }
    for (std::size_t second = *found + 1; second < names_.size(); ++second) {
      if (names_[second] == name) {
        return Error::ambiguous_column(std::string(name), *found, second);
      }
    }
    return *found;
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
  case 27:
    kind = static_cast<FailureKind>(raw.code);
    break;
  default: // 17, and the reading of any code the core has yet to be given
    break;
  }
  return Failure{kind, raw.record, raw.line, raw.byte, raw.expected, raw.found};
}

// A format in the layout the core reads, or false for one this library's HyperCast does not read
// (one a newer HyperCast declared with fields past this layout).
inline bool raw_format(const NumFormat &format, HyperTabularRawNumFormat &out) noexcept {
  const HyperCastRawNumFormat &raw = format.raw();
  const auto *bytes = reinterpret_cast<const unsigned char *>(&raw);
  constexpr std::size_t ours = sizeof(HyperTabularRawNumFormat);
  constexpr std::size_t theirs = sizeof(HyperCastRawNumFormat);
  static_assert(ours == HyperTabularRawNumFormat_SIZE, "the header's layout is the library's");
  for (std::size_t i = ours; i < theirs; ++i) {
    if (bytes[i] != 0) {
      return false;
    }
  }
  std::memcpy(&out, bytes, std::min(ours, theirs));
  out.size = std::min<std::uint32_t>(raw.size, HyperTabularRawNumFormat_SIZE);
  return true;
}

// A column as the core takes it, or false for one it cannot be: an ordinal past the core's
// reach, or a format this library's HyperCast does not read. What it points at is the column's
// own, shared by every copy of it.
inline bool spec(const Column &column, HyperTabularColumnSpecWith &out) noexcept {
  if (column.ordinal() > max_ordinal) {
    return false;
  }
  out = HyperTabularColumnSpecWith{};
  out.size = HyperTabularColumnSpecWith_SIZE;
  out.ordinal = static_cast<std::uint32_t>(column.ordinal());
  out.door = static_cast<std::uint32_t>(column.door());
  out.param = column.param();
  if (!raw_format(column.format(), out.format)) {
    return false;
  }
  const HyperCastRawDateOptions &date = column.date_.raw();
  out.date = HyperTabularRawDateOptions{date.size, date.flags, date.pivot};
  out.months = packed_data(column.months_);
  out.months_len = packed_size(column.months_);
  out.tokens = packed_data(column.tokens_);
  out.tokens_len = packed_size(column.tokens_);
  if (column.constraints_ != nullptr) {
    out.constraints = column.constraints_.get();
    // The list is read under HAS_ENUM alone, and one passed without it is a malformed layout.
    if ((column.constraints_->flags & HyperTabularRawConstraints_HAS_ENUM) != 0) {
      out.enum_ptr = packed_data(column.enum_);
      out.enum_len = packed_size(column.enum_);
    }
  }
  return true;
}

// The core's own word on a column, asked before any input is read: a fill of no rows judges its
// door, what the door declares, its format, date options, vocabulary, tokens and constraints, and
// writes nothing. It answers on a state of its own, so a workbook's plan is asked the same way.
class Judge {
public:
  Judge() noexcept {
    const HyperTabularRawDelimitedOptions options{
        HyperTabularRawDelimitedOptions_SIZE, ',', 1, 1, 0, 0, 0, 0, 0};
    if (::hypertabular_delimited_init_with(state(), &options) != HYPERTABULAR_OK) {
      contract_violation();
    }
  }
  bool accepts(const HyperTabularColumnSpecWith &spec) noexcept {
    std::uint64_t value[4] = {};
    HyperTabularCellVerdict verdict{};
    HyperTabularRawCheck check{};
    const HyperTabularColumnBufferWith buffer{HyperTabularColumnBufferWith_SIZE, value, &verdict,
                                              nullptr,
                                              spec.constraints != nullptr ? &check : nullptr};
    HyperTabularBuffersWith memory{};
    memory.size = HyperTabularBuffersWith_SIZE;
    HyperTabularFilled out{};
    return ::hypertabular_delimited_fill_with(state(), nullptr, 0, 0, &spec, &buffer, 1, 0, &memory,
                                              &out) != HYPERTABULAR_ERR_CONTRACT;
  }

private:
  HyperTabularDelimitedState *state() noexcept {
    return reinterpret_cast<HyperTabularDelimitedState *>(block_);
  }
  std::uint64_t block_[HyperTabularDelimitedState_SIZE / 8] = {};
};

// The plan's specs, its width (the widest ordinal plus one) and the column that sets it, or
// the column that cannot be honoured.
inline Result<std::size_t> specs(const std::vector<Column> &plan,
                                 std::vector<HyperTabularColumnSpecWith> &out,
                                 std::size_t &widest) noexcept {
  out.assign(plan.size(), HyperTabularColumnSpecWith{});
  Judge judge;
  std::size_t width = 0;
  widest = 0;
  for (std::size_t i = 0; i < plan.size(); ++i) {
    if (!spec(plan[i], out[i]) || !judge.accepts(out[i])) {
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

// The core's answer on a read's sizes, ahead of everything the read allocates: the core decides
// how much a batch may ask for, and says what size to start an input buffer at.
inline Result<std::size_t> check_sizes(const std::vector<HyperTabularColumnSpecWith> &specs,
                                       std::size_t batch_rows, std::size_t buffer_bytes,
                                       std::uint32_t flags) noexcept {
  HyperTabularRawBatchCheck found{};
  const std::int32_t code = ::hypertabular_batch_check(specs.data(), specs.size(), batch_rows,
                                                       buffer_bytes, flags, &found);
  if (code == HYPERTABULAR_OK) {
    return static_cast<std::size_t>(found.buffer_bytes);
  }
  if (code != HYPERTABULAR_ERR_BATCH) {
    contract_violation(); // the specs were judged when they were made
  }
  switch (found.refused) {
  case HyperTabularRawBatchCheck_REFUSED_BATCH_ROWS_ZERO:
    return Error::zero_batch_rows();
  case HyperTabularRawBatchCheck_REFUSED_BUFFER_BYTES_ZERO:
    return Error::zero_buffer_bytes();
  case HyperTabularRawBatchCheck_REFUSED_ROW_TOO_WIDE:
    return Error::row_too_wide(found.width, found.needed, found.limit);
  default:
    return Error::batch_too_large(found.needed, found.limit);
  }
}

// The rows of a sampler's batch for a sampler wanting `wanted` rows: the core's answer, however
// many batches that takes. Only a row past the limit is refused, in its words.
inline Result<std::size_t> sample_rows(const std::vector<HyperTabularColumnSpecWith> &specs,
                                       std::size_t wanted, std::uint32_t flags) noexcept {
  HyperTabularRawBatchCheck found{};
  (void)::hypertabular_batch_check(specs.data(), specs.size(), wanted, 1, flags, &found);
  if (found.sample_rows == 0) {
    return check_sizes(specs, wanted, 1, flags);
  }
  return static_cast<std::size_t>(found.sample_rows);
}

inline std::size_t cell_rows(std::size_t per_row, std::size_t batch_rows) noexcept {
  return std::clamp<std::size_t>(cell_budget / std::max<std::size_t>(per_row, 1), 1,
                                 std::max<std::size_t>(batch_rows, 1));
}

// One column's room: batch_rows values of its door, held as u64s so they are aligned for any
// of them, as many verdicts, as many notes unless a read asks for none, and for a column that
// declares constraints as many checks.
struct Store {
  std::vector<std::uint64_t> values;
  std::vector<HyperTabularCellVerdict> verdicts;
  std::vector<HyperTabularCellNote> notes;
  std::vector<HyperTabularRawCheck> checks;
};

// A bound plan and the buffers it is read into.
struct Columns {
  std::vector<Column> plan;
  std::vector<HyperTabularColumnSpecWith> specs;
  std::vector<Store> stores;
  std::vector<HyperTabularColumnBufferWith> pointers;
  std::vector<std::uint32_t> row_widths; // batch_rows counts, or empty for a read that asks none
  std::uint32_t long_rows;               // the read's policy, RawDelimitedOptions' code
  std::size_t batch_rows = 0;
  std::uint64_t generation = 0; // names the rows in hand: each read begins by moving it

  Columns(std::vector<Column> columns, std::vector<HyperTabularColumnSpecWith> declared,
          std::size_t rows, bool notes, bool widths, std::uint32_t long_row_policy) noexcept
      : plan(std::move(columns)), specs(std::move(declared)), long_rows(long_row_policy),
        batch_rows(rows) {
    stores.resize(plan.size());
    pointers.resize(plan.size());
    if (widths) {
      row_widths.assign(batch_rows, 0);
    }
    for (std::size_t i = 0; i < plan.size(); ++i) {
      stores[i].values.assign((value_size(plan[i].door()) * batch_rows + 7) / 8, 0);
      stores[i].verdicts.assign(batch_rows, HyperTabularCellVerdict{});
      if (notes) {
        stores[i].notes.assign(batch_rows, HyperTabularCellNote{});
      }
      if (plan[i].is_checked()) {
        stores[i].checks.assign(batch_rows, HyperTabularRawCheck{});
      }
      pointers[i] = HyperTabularColumnBufferWith{
          HyperTabularColumnBufferWith_SIZE, stores[i].values.data(), stores[i].verdicts.data(),
          notes ? stores[i].notes.data() : nullptr,
          stores[i].checks.empty() ? nullptr : stores[i].checks.data()};
    }
  }
};

enum class Reading { delimited, workbook };

// The 1.0 entry points, which stay exports for a program that calls them. Every read here goes
// through the size-tagged ones, since a 1.0 fill ignores a format's flag bits from 8 up, and with
// them a declared grouping policy, so it reads every format as Western. c/smoke/main.cpp calls
// these and compares what they read with a read.
inline std::int32_t init_1_0(HyperTabularDelimitedState *state,
                             const HyperTabularRawDialect *dialect) noexcept {
  return ::hypertabular_delimited_init(state, dialect);
}
inline std::int32_t fill_1_0(HyperTabularDelimitedState *state, const std::uint8_t *input,
                             std::size_t input_len, bool last, const HyperTabularColumnSpec *specs,
                             const HyperTabularColumnBuffer *columns, std::size_t column_count,
                             std::size_t max_rows, const HyperTabularBuffers *buffers,
                             HyperTabularFilled *out) noexcept {
  return ::hypertabular_delimited_fill(state, input, input_len, last ? 1 : 0, specs, columns,
                                       column_count, max_rows, buffers, out);
}
inline std::int32_t sheet_1_0(HyperTabularWorkbookState *state, const std::uint8_t *container,
                              std::size_t container_len, const std::uint8_t *part,
                              std::size_t part_len, std::uint32_t index, bool has_header,
                              bool skip_empty_rows, HyperTabularFilled *out) noexcept {
  return ::hypertabular_workbook_sheet(state, container, container_len, part, part_len, index,
                                       has_header ? 1 : 0, skip_empty_rows ? 1 : 0, out);
}
inline std::int32_t book_fill_1_0(HyperTabularWorkbookState *state, const std::uint8_t *container,
                                  std::size_t container_len, const HyperTabularColumnSpec *specs,
                                  const HyperTabularColumnBuffer *columns, std::size_t column_count,
                                  std::size_t max_rows, const HyperTabularBuffers *buffers,
                                  HyperTabularFilled *out) noexcept {
  return ::hypertabular_workbook_fill(state, container, container_len, specs, columns, column_count,
                                      max_rows, buffers, out);
}
} // namespace detail

// Where a sheet's cells are read from: a rectangle of it. Both corners of `A5:F200` are included,
// and a plan's ordinal 0 is the first column; a row's line and a problem's source column stay the
// sheet's own. A range past the end of the sheet invents no rows.
class Range {
public:
  // `A5:F200`: both corners, in capitals, the second no less than the first in either direction.
  // The core reads it, so every binding agrees on what one is. ErrorKind::range for anything else,
  // `A5` alone, `a5:f200` or `F200:A5`.
  static Result<Range> parse(std::string_view text) noexcept {
    HyperTabularRawRange raw{};
    if (::hypertabular_range_parse(reinterpret_cast<const std::uint8_t *>(text.data()), text.size(),
                                   &raw) != HYPERTABULAR_OK) {
      return Error::range(std::string(text));
    }
    return Range(raw.first_row, raw.first_column, raw.last_row, raw.last_column);
  }
  // A range from its four numbers, each 1-based, with 0 for no limit on that side: every row from
  // the fifth, or columns B to D of all of them, which `A5:F200` can't say. ErrorKind::range if a
  // last is before its first.
  static Result<Range> of(std::uint32_t first_row, std::uint32_t first_column,
                          std::uint32_t last_row, std::uint32_t last_column) noexcept {
    const auto inverted = [](std::uint32_t first, std::uint32_t last) noexcept {
      return first != 0 && last != 0 && first > last;
    };
    if (inverted(first_row, last_row) || inverted(first_column, last_column)) {
      return Error::range("rows " + std::to_string(first_row) + " to " + std::to_string(last_row) +
                          ", columns " + std::to_string(first_column) + " to " +
                          std::to_string(last_column));
    }
    return Range(first_row, first_column, last_row, last_column);
  }

  // Each 1-based, A being column 1, or 0 for no limit.
  constexpr std::uint32_t first_row() const noexcept { return first_row_; }
  constexpr std::uint32_t first_column() const noexcept { return first_column_; }
  constexpr std::uint32_t last_row() const noexcept { return last_row_; }
  constexpr std::uint32_t last_column() const noexcept { return last_column_; }

  friend constexpr bool operator==(const Range &a, const Range &b) noexcept {
    return a.first_row_ == b.first_row_ && a.first_column_ == b.first_column_ &&
           a.last_row_ == b.last_row_ && a.last_column_ == b.last_column_;
  }
  friend constexpr bool operator!=(const Range &a, const Range &b) noexcept { return !(a == b); }

private:
  constexpr Range(std::uint32_t first_row, std::uint32_t first_column, std::uint32_t last_row,
                  std::uint32_t last_column) noexcept
      : first_row_(first_row), first_column_(first_column), last_row_(last_row),
        last_column_(last_column) {}

  std::uint32_t first_row_;
  std::uint32_t first_column_;
  std::uint32_t last_row_;
  std::uint32_t last_column_;
};

// ---------------------------------------------------------------------------------------------
// What a read hands out.

// Where a cell's verdict came from, numbered as the core numbers it. An unknown number is kept, and
// name() spells it "origin N".
enum class Origin : std::uint16_t {
  cell = HyperTabularCellNote_ORIGIN_CELL,     // the door's: what it made of the cell's text
  token = HyperTabularCellNote_ORIGIN_TOKEN,   // a declared token, Empty with no door asked
  padded = HyperTabularCellNote_ORIGIN_PADDED, // a cell its row doesn't have, under ShortRows::pad
  uncached = HyperTabularCellNote_ORIGIN_UNCACHED // a formula with no cached result
};

// The corpus's name of an origin: "cell", "token", "padded", "uncached".
inline std::string name(Origin origin) noexcept {
  switch (origin) {
  case Origin::cell:
    return "cell";
  case Origin::token:
    return "token";
  case Origin::padded:
    return "padded";
  case Origin::uncached:
    return "uncached";
  }
  return "origin " + std::to_string(static_cast<std::uint16_t>(origin));
}

// One entry of Batch::faults(): a cell that went wrong, or a row. An entry about the row as a whole
// has no plan_column, source_column or reason, says Detail::none and Origin::cell, and carries
// a long row's widths in expected and found. A cell that cast and broke a constraint its column
// declares has no reason: its keyword says what it broke, its span is the value's.
struct Problem {
  std::size_t row;                          // the row in the batch, from zero
  std::uint32_t line;                       // where the row came from, as Batch::line says
  std::optional<std::size_t> plan_column;   // the column of the plan
  std::optional<std::size_t> source_column; // the source's, from zero: a plan column's ordinal,
                                            // and on a sheet its own column, A being 0, wherever a
                                            // range starts
  std::optional<CastFailure> reason;        // malformed, out_of_range, or empty for a formula with
                                            // no cached value; none for a violation
  Detail detail;                            // what failed, as Batch::detail says
  Origin origin;                            // where the verdict came from, as Batch::origin says
  std::uint32_t offset;                     // the span in the cell's decoded, unescaped text, in
  std::uint32_t length;                     // bytes of its UTF-8: 0 where there's none
  std::uint32_t expected;                   // a long row's width, the first record's: 0 otherwise
  std::uint32_t found;                      // and the cells it had: 0 otherwise
  std::optional<Keyword> keyword;           // the rule a violation broke; none otherwise
};

class Batch;
class Faults;

// One row of a batch: the batch read across instead of down.
class Row {
public:
  // The row's index in its batch.
  std::size_t index() const noexcept { return index_; }
  // Where the row came from: for delimited text the one-based line its record starts on, for a
  // sheet its one-based row number.
  inline std::uint32_t line() const noexcept;
  template <class T> Verdict<T> get(std::size_t column) const noexcept;
  template <class T> Checked<T> get_checked(std::size_t column) const noexcept;
  inline Verdict<std::string_view> text(std::size_t column) const noexcept;
  inline std::string raw(std::size_t column) const noexcept;
  inline std::optional<Fault> fault(std::size_t column) const noexcept;
  inline Detail detail(std::size_t column) const noexcept;
  inline Origin origin(std::size_t column) const noexcept;

private:
  friend class Batch;
  Row(const Batch *batch, std::size_t index) noexcept : batch_(batch), index_(index) {}
  const Batch *batch_;
  std::size_t index_;
};

// The rows of one read, column-major: for each plan column the values its door made and a
// verdict beside each. A view of the reader's buffers, valid until its next read() and no longer
// than the reader or sheet that read it: unlike the other bindings' batches, nothing here can tell.
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
    return reading_ == hypertabular::detail::Reading::delimited ? entry.len : entry.offset;
  }

  // The cell at `row` of column `column`, as its door made it: a value, or HyperCast's fault.
  // T is the type the column's door makes: bool, std::int8_t to std::uint64_t, float and
  // double for the door of the same name; Decimal; Uuid; Timestamp for timestamp, unix_time
  // and excel_serial; Date for date and date_ordered; CivilDateTime for datetime;
  // std::chrono::nanoseconds (or std::uint64_t) for time, since midnight; Duration. Text is
  // read with text(). Any other T ends the program, as does a column that declares constraints,
  // whose cells get_checked() reads: a violation is no fault, so get() would say it passed.
  template <class T> Verdict<T> get(std::size_t column, std::size_t row) const noexcept {
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    unchecked(column);
    const Door door = columns_->plan[column].door();
    if (!reads<T>(door)) {
      hypertabular::detail::contract_violation();
    }
    if (auto fault = fault_of(store.verdicts[row])) {
      return *fault;
    }
    return value<T>(store, door, row);
  }

  // The cell at `row` of a text column: its bytes, untrimmed, or an Empty fault for a cell with
  // none. A typed workbook cell is said the canonical way: 42, true, 2024-01-31T10:30:00. A text
  // column that declares constraints ends the program, as get() does.
  Verdict<std::string_view> text(std::size_t column, std::size_t row) const noexcept {
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    unchecked(column);
    if (columns_->plan[column].door() != Door::text) {
      hypertabular::detail::contract_violation();
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

  // The cell at `row` of column `column` as HyperCast's checked door judged it: a CheckedValue,
  // flagged when multipleOf's rounding adjusted it and, at the text door, naming the enum entry
  // it matched; or a Rejection, the cell's Fault as get() gives it or a Violation of a rule the
  // column declares (Column::checked), carrying the value as cast and its span. T is the checked
  // door's type: std::int8_t to std::uint64_t, float, double, Decimal, or std::string_view for a
  // text column, the text it judged (after the trim it declares), a view of the batch, or on an
  // enum match the entry as declared, a view of the column's copy of its declaration. A column
  // that declares none reads as get() and text() read it, every cell that cast passing,
  // unadjusted. Any other T, or a column whose door doesn't make it, ends the program.
  template <class T> Checked<T> get_checked(std::size_t column, std::size_t row) const noexcept {
    static_assert(checks_through<T>(), "no checked door makes this type");
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    const Column &declared = columns_->plan[column];
    if constexpr (std::is_same_v<T, std::string_view>) {
      if (declared.door() != Door::text) {
        hypertabular::detail::contract_violation();
      }
    } else if (!reads<T>(declared.door())) {
      hypertabular::detail::contract_violation();
    }
    const HyperTabularCellVerdict &verdict = store.verdicts[row];
    const bool broke = verdict.reason == HyperTabularCellVerdict_VIOLATION;
    if (!broke) {
      if (auto fault = fault_of(verdict)) {
        return Rejection<T>(*fault);
      }
    }
    T cast{};
    if constexpr (std::is_same_v<T, std::string_view>) {
      HyperTabularSpan span;
      std::memcpy(&span,
                  reinterpret_cast<const unsigned char *>(store.values.data()) + row * sizeof span,
                  sizeof span);
      cast = bytes(span);
    } else {
      cast = value<T>(store, declared.door(), row);
    }
    if (store.checks.empty()) {
      return CheckedValue<T>{cast, false, std::nullopt};
    }
    const HyperTabularRawCheck &check = store.checks[row];
    if (broke) {
      return Rejection<T>(
          Violation<T>{static_cast<Keyword>(check.keyword), cast, verdict.offset, verdict.len});
    }
    std::optional<std::uint32_t> index;
    if (check.enum_index != UINT32_MAX) {
      index = check.enum_index;
      if constexpr (std::is_same_v<T, std::string_view>) {
        cast = hypertabular::detail::packed_entry(declared.enum_, check.enum_index);
      }
    }
    return CheckedValue<T>{cast, check.adjusted != 0, index};
  }

  // The cell's fault, or std::nullopt for a cell that cast, whether or not it broke a constraint
  // its column declares (get_checked() says which).
  std::optional<Fault> fault(std::size_t column, std::size_t row) const noexcept {
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    if (store.verdicts[row].reason == HyperTabularCellVerdict_VIOLATION) {
      return std::nullopt;
    }
    return fault_of(store.verdicts[row]);
  }

  // What failed in the cell, HyperCast's own name for it: Detail::none for a cell that cast, for an
  // Empty no door gave, and for every cell with notes off. A number this header doesn't name is
  // kept, as Detail says. hypercast::Fault has no room for this, so it's read here and on the
  // entries of faults().
  Detail detail(std::size_t column, std::size_t row) const noexcept {
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    return store.notes.empty() ? Detail::none : static_cast<Detail>(store.notes[row].detail);
  }

  // Where the cell's verdict came from: Origin::cell for what its door said, and for every cell
  // with notes off.
  Origin origin(std::size_t column, std::size_t row) const noexcept {
    const hypertabular::detail::Store &store = this->store(column);
    check_row(row);
    return store.notes.empty() ? Origin::cell : static_cast<Origin>(store.notes[row].origin);
  }

  // What went wrong in this batch, by row and then by column, a row's own entry first: cells at
  // malformed or out_of_range, formulas with no cached value (Origin::uncached), cells that broke
  // a constraint their column declares (Problem::keyword), and rows longer than the width under
  // LongRows::report. Blanks, tokens, padding and a row cut under
  // LongRows::truncate aren't problems, and aren't listed.
  //
  //   for (const hypertabular::Problem &problem : batch.faults()) { ... }
  //
  // It reads what the fill already wrote, a few entries at a time. Walking it after the reader has
  // moved on is a caller bug, and aborts. With notes off it still lists every refused cell and long
  // row, with no detail, but no formula with no cached value: nothing was kept to tell one from a
  // blank. It can't name a row it wasn't given: a row of a sheet whose only cells are formulas with
  // nothing spelled where a value goes is an empty row, which SheetOptions::skip_empty_rows leaves
  // out by default.
  inline Faults faults() const noexcept;

  // The text of the cell at `row` of column `column`, whatever its door and its verdict: what a
  // fault's span is a span of. A copy, since quoted delimited text with a doubled quote in it
  // has to be unescaped to be read. For a workbook, a typed cell that cast has no text, and
  // neither has an empty cell.
  std::string raw(std::size_t column, std::size_t row) const noexcept {
    store(column);
    check_row(row);
    const std::size_t at = reading_ == hypertabular::detail::Reading::delimited
                               ? columns_->plan[column].ordinal()
                               : column;
    const HyperTabularSpan cell = cells_[row * per_row_ + at];
    if (reading_ == hypertabular::detail::Reading::workbook) {
      return std::string(bytes(cell));
    }
    const std::size_t from = cell.offset;
    const std::size_t length = hypertabular::detail::span_len(cell);
    if (from + length > base_.size()) {
      return std::string();
    }
    if (!hypertabular::detail::flagged(cell)) {
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

    iterator() noexcept = default;
    // A walk past the next read() is a caller bug, as a faults walk's is, and ends the program:
    // the rows are the next batch's.
    Row operator*() const noexcept {
      batch_->check_current();
      return batch_->row(index_);
    }
    iterator &operator++() noexcept {
      batch_->check_current();
      ++index_;
      return *this;
    }
    iterator operator++(int) noexcept {
      iterator before = *this;
      ++*this;
      return before;
    }
    friend bool operator==(const iterator &a, const iterator &b) noexcept {
      return a.index_ == b.index_ && a.batch_ == b.batch_;
    }
    friend bool operator!=(const iterator &a, const iterator &b) noexcept { return !(a == b); }

  private:
    friend class Batch;
    iterator(const Batch *batch, std::size_t index) noexcept : batch_(batch), index_(index) {}
    const Batch *batch_ = nullptr;
    std::size_t index_ = 0;
  };

  iterator begin() const noexcept { return iterator(this, 0); }
  iterator end() const noexcept { return iterator(this, rows_); }

private:
  friend class DelimitedReader;
  friend class Sheet;
  friend class Faults;

  Batch(const hypertabular::detail::Columns *columns, std::size_t rows,
        const HyperTabularSpan *cells, std::size_t per_row, std::string_view base,
        std::string_view arena, hypertabular::detail::Reading reading, std::uint32_t expected,
        std::size_t first_column) noexcept
      : columns_(columns), rows_(rows), cells_(cells), per_row_(per_row), base_(base),
        arena_(arena), reading_(reading), expected_(expected), first_column_(first_column),
        generation_(columns->generation) {}

  const hypertabular::detail::Store &store(std::size_t column) const noexcept {
    if (column >= columns_->stores.size()) {
      hypertabular::detail::contract_violation();
    }
    return columns_->stores[column];
  }
  void check_row(std::size_t row) const noexcept {
    if (row >= rows_) {
      hypertabular::detail::contract_violation();
    }
  }
  // Refuses a column that declares constraints, whose verdicts the plain getters can't say.
  void unchecked(std::size_t column) const noexcept {
    if (columns_->plan[column].is_checked()) {
      hypertabular::detail::contract_violation();
    }
  }
  // The reader has read on: what its buffers hold is the next batch's.
  void check_current() const noexcept {
    if (columns_->generation != generation_) {
      hypertabular::detail::contract_violation();
    }
  }

  static std::optional<Fault> fault_of(const HyperTabularCellVerdict &verdict) noexcept {
    if (verdict.reason == 0) {
      return std::nullopt;
    }
    return Fault{static_cast<CastFailure>(verdict.reason), verdict.offset, verdict.len};
  }

  std::string_view bytes(const HyperTabularSpan &span) const noexcept {
    const std::string_view store = hypertabular::detail::flagged(span) ? arena_ : base_;
    const std::size_t from = span.offset;
    const std::size_t length = hypertabular::detail::span_len(span);
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

  // Whether a checked door makes a T.
  template <class T> static constexpr bool checks_through() noexcept {
    return std::is_same_v<T, std::int8_t> || std::is_same_v<T, std::int16_t> ||
           std::is_same_v<T, std::int32_t> || std::is_same_v<T, std::int64_t> ||
           std::is_same_v<T, std::uint8_t> || std::is_same_v<T, std::uint16_t> ||
           std::is_same_v<T, std::uint32_t> || std::is_same_v<T, std::uint64_t> ||
           std::is_same_v<T, float> || std::is_same_v<T, double> || std::is_same_v<T, Decimal> ||
           std::is_same_v<T, std::string_view>;
  }

  template <class Raw>
  static Raw load(const hypertabular::detail::Store &store, std::size_t row) noexcept {
    Raw raw;
    std::memcpy(&raw,
                reinterpret_cast<const unsigned char *>(store.values.data()) + row * sizeof(Raw),
                sizeof(Raw));
    return raw;
  }

  template <class T>
  static T value(const hypertabular::detail::Store &store, Door door, std::size_t row) noexcept {
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

  const hypertabular::detail::Columns *columns_;
  std::size_t rows_;
  const HyperTabularSpan *cells_;
  std::size_t per_row_;
  std::string_view base_;
  std::string_view arena_;
  hypertabular::detail::Reading reading_;
  std::uint32_t expected_;   // the width a delimited read's rows are cut to
  std::size_t first_column_; // how far a sheet's range starts from its column A
  std::uint64_t generation_; // which of its reader's reads this is
};

// The entries of one batch's faults, a range to walk once. An input range: a Faults has one
// position, which its iterators share, and a second begin() moves them all back to the start. Two
// walks at once take a batch.faults() each. A Problem is a value, and stays what it said after the
// reader moves on. Walking on past the next read() is a contract violation: stopping there would
// read as the end of the list. Finish the walk first.
class Faults {
public:
  class iterator {
  public:
    using iterator_category = std::input_iterator_tag;
    using value_type = Problem;
    using difference_type = std::ptrdiff_t;
    using pointer = void;
    using reference = Problem;

    iterator() noexcept = default;
    Problem operator*() const noexcept { return faults_->current(); }
    iterator &operator++() noexcept {
      faults_->advance();
      return *this;
    }
    void operator++(int) noexcept { faults_->advance(); }
    friend bool operator==(const iterator &a, const iterator &b) noexcept {
      return a.over() == b.over();
    }
    friend bool operator!=(const iterator &a, const iterator &b) noexcept { return !(a == b); }

  private:
    friend class Faults;
    explicit iterator(Faults *faults) noexcept : faults_(faults) {}
    bool over() const noexcept { return faults_ == nullptr || faults_->done_; }
    Faults *faults_ = nullptr;
  };

  iterator begin() noexcept {
    cursor_ = 0;
    at_ = 0;
    held_count_ = 0;
    done_ = false;
    fetch();
    return iterator(this);
  }
  iterator end() noexcept { return iterator(nullptr); }

private:
  friend class Batch;
  explicit Faults(const Batch &batch) noexcept : batch_(batch) {}

  // The reader moved on: what the fill wrote is the next batch's.
  bool stale() const noexcept { return batch_.columns_->generation != batch_.generation_; }

  // The next few entries, with their rows' lines read while those are still the batch's: an
  // iterator held past the next read() still answers for the entry it holds, and goes no further.
  void fetch() noexcept {
    at_ = 0;
    held_count_ = 0;
    if (stale()) {
      hypertabular::detail::contract_violation();
    }
    const hypertabular::detail::Columns &columns = *batch_.columns_;
    const std::int32_t code = ::hypertabular_faults(
        columns.pointers.data(), columns.pointers.size(), batch_.rows_,
        columns.row_widths.empty() ? nullptr : columns.row_widths.data(), batch_.expected_,
        columns.long_rows, &cursor_, held_.data(), held_.size(), &held_count_);
    if (code != HYPERTABULAR_OK) {
      hypertabular::detail::contract_violation();
    }
    done_ = held_count_ == 0;
    for (std::size_t i = 0; i < held_count_; ++i) {
      lines_[i] = batch_.line(static_cast<std::size_t>(held_[i].row));
    }
  }

  void advance() noexcept {
    if (done_) {
      return;
    }
    if (stale()) {
      hypertabular::detail::contract_violation();
    }
    if (++at_ < held_count_) {
      return;
    }
    if (held_count_ < held_.size()) {
      done_ = true; // fewer than it could hold: the core has reached the end
      return;
    }
    fetch();
  }

  Problem current() const noexcept {
    const HyperTabularRawProblem &raw = held_[at_];
    const auto row = static_cast<std::size_t>(raw.row);
    Problem problem{row,
                    lines_[at_],
                    std::nullopt,
                    std::nullopt,
                    std::nullopt,
                    static_cast<Detail>(raw.detail),
                    static_cast<Origin>(raw.origin),
                    raw.offset,
                    raw.len,
                    raw.expected,
                    raw.found,
                    std::nullopt};
    if (raw.plan_column != HyperTabularRawProblem_ROW_LEVEL) {
      problem.plan_column = raw.plan_column;
      problem.source_column =
          batch_.columns_->plan[raw.plan_column].ordinal() + batch_.first_column_;
      if (raw.reason == HyperTabularCellVerdict_VIOLATION) {
        problem.keyword = static_cast<Keyword>(raw.keyword);
      } else {
        problem.reason = static_cast<CastFailure>(raw.reason);
      }
    }
    return problem;
  }

  Batch batch_;
  std::uint64_t cursor_ = 0;
  std::array<HyperTabularRawProblem, 32> held_{};
  std::array<std::uint32_t, 32> lines_{};
  std::size_t held_count_ = 0;
  std::size_t at_ = 0;
  bool done_ = false;
};

inline Faults Batch::faults() const noexcept { return Faults(*this); }

inline std::uint32_t Row::line() const noexcept { return batch_->line(index_); }
template <class T> Verdict<T> Row::get(std::size_t column) const noexcept {
  return batch_->get<T>(column, index_);
}
template <class T> Checked<T> Row::get_checked(std::size_t column) const noexcept {
  return batch_->get_checked<T>(column, index_);
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
inline Detail Row::detail(std::size_t column) const noexcept {
  return batch_->detail(column, index_);
}
inline Origin Row::origin(std::size_t column) const noexcept {
  return batch_->origin(column, index_);
}

// ---------------------------------------------------------------------------------------------
// Asking what a source's columns look like. A suggestion is an answer: nothing here reaches a read.

// How a sample is taken and read.
struct SuggestOptions {
  // The most rows the sample holds (at least one). It stops at 4096 rows of delimited text, and at
  // a sheet's own batch size, read in as many batches as the core says fit: a sample is never
  // refused for the size of its batch.
  std::size_t sample_rows = 1000;
  // Reads every column's numbers in this notation, which settles what `,` means in them.
  std::optional<NumFormat> format = std::nullopt;
  // Reads the numbers of a source column in a notation of its own, ahead of `format`, the last
  // given for a column winning.
  std::vector<std::pair<std::size_t, NumFormat>> column_formats = {};
  // Reads every column's dates in this order where the sample leaves the order open, as 01/02/2026
  // does. A column whose cells rule that order out keeps the order they leave: it answers what the
  // sample can't, and doesn't overrule what it can.
  std::optional<DateOrder> date_order = std::nullopt;
  // Reads the dates of a source column in an order of its own, ahead of `date_order` and as that
  // does, the last given for a column winning.
  std::vector<std::pair<std::size_t, DateOrder>> column_date_orders = {};

  SuggestOptions with_sample_rows(std::size_t rows) const noexcept {
    SuggestOptions options = *this;
    options.sample_rows = rows == 0 ? 1 : rows;
    return options;
  }
  SuggestOptions with_format(const NumFormat &declared) const noexcept {
    SuggestOptions options = *this;
    options.format = declared;
    return options;
  }
  SuggestOptions with_column_format(std::size_t ordinal, const NumFormat &declared) const noexcept {
    SuggestOptions options = *this;
    options.column_formats.emplace_back(ordinal, declared);
    return options;
  }
  SuggestOptions with_date_order(DateOrder declared) const noexcept {
    SuggestOptions options = *this;
    options.date_order = declared;
    return options;
  }
  SuggestOptions with_column_date_order(std::size_t ordinal, DateOrder declared) const noexcept {
    SuggestOptions options = *this;
    options.column_date_orders.emplace_back(ordinal, declared);
    return options;
  }
};

// How far a sample settles a column, numbered as the core numbers it.
enum class SuggestStatus : std::uint32_t {
  // One door and one declaration take every cell, and the plan reads the column through them.
  resolved = HyperTabularRawSuggestion_RESOLVED,
  // Every cell is read, but the sample can't say which declaration it's written in: candidates are
  // the choices. The plan reads the column as text until the caller chooses.
  ambiguous = HyperTabularRawSuggestion_AMBIGUOUS,
  // There's nothing to go on, or a cell no door took beside cells one did, or a cell that isn't
  // text at all. The plan reads the column as text.
  unresolved = HyperTabularRawSuggestion_UNRESOLVED
};

// "resolved", "ambiguous" or "unresolved".
constexpr std::string_view name(SuggestStatus status) noexcept {
  switch (status) {
  case SuggestStatus::resolved:
    return "resolved";
  case SuggestStatus::ambiguous:
    return "ambiguous";
  case SuggestStatus::unresolved:
    return "unresolved";
  }
  return "unresolved";
}

// A declaration that would settle an ambiguous column: the order a date's fields come in, or the
// notation numbers are in. Declare a notation with SuggestOptions::with_column_format and ask
// again: the door can differ under each, as `1,234` is an i32 under one and an f64 under the other.
using Candidate = std::variant<DateOrder, NumFormat>;

// What a sample says of one source column.
struct SuggestedColumn {
  std::size_t ordinal;               // the zero-based source column, as a Column names it
  std::optional<std::string> name;   // the header's name for it, if the source has a header
  Door door;                         // text when unresolved, the first candidate's when ambiguous
  std::uint32_t param;               // what the door declares, as Column::of takes it: a date's
                                     // DateOrder, the first candidate's when ambiguous, or 0
  std::optional<NumFormat> format;   // the notation the plan reads a resolved numeric column in:
                                     // the one declared for it, or else the one candidate the
                                     // sample left, comma decimal where only that reads the column
                                     // and otherwise NumFormat::invariant(), a `,` in the sample
                                     // or not. So a later `1,23,456` is refused under the latter:
                                     // Western. Empty for any other
  std::vector<Candidate> candidates; // what to choose between: empty unless ambiguous
  SuggestStatus status;              // how far the sample settles it
  std::uint32_t matched;             // the cells door took: all of sampled, resolved or ambiguous;
                                     // unresolved, the most any one door but text took
  std::uint32_t sampled;             // the cells that aren't blank
};

// What suggest found: a plan to change or to hand to a read, and why.
struct Suggestion {
  std::vector<Column> plan; // a column for each source column, in order. Only a resolved column
                            // reads through its door: the rest read as text, which refuses nothing
  std::vector<SuggestedColumn> report; // a report for each source column, in the plan's order
  bool truncated = false; // the source has rows past the sample. One with exactly the sample's
                          // rows hasn't
};

namespace detail {
// A door the core named, and what it declares, or false when its numbers are neither: a door a
// later core adds reads as text. Every door the core has is taken here, not only those it suggests
// today.
inline bool door_of(std::uint32_t code, std::uint32_t param, Door &door,
                    std::uint32_t &declared) noexcept {
  if (code < static_cast<std::uint32_t>(Door::boolean) ||
      code > static_cast<std::uint32_t>(Door::excel_serial)) {
    return false;
  }
  door = static_cast<Door>(code);
  declared = 0;
  switch (door) {
  case Door::unix_time:
    declared = param;
    return param >= 1 && param <= 4;
  case Door::date_ordered:
  case Door::datetime:
    declared = param;
    return param >= 1 && param <= 3;
  case Door::excel_serial:
    declared = param;
    return param >= 1 && param <= 2;
  default:
    return true;
  }
}

inline bool numeric(Door door) noexcept {
  switch (door) {
  case Door::i8:
  case Door::i16:
  case Door::i32:
  case Door::i64:
  case Door::u8:
  case Door::u16:
  case Door::u32:
  case Door::u64:
  case Door::f32:
  case Door::f64:
  case Door::decimal:
    return true;
  default:
    return false;
  }
}

// The notation NOTATION_COMMA_DECIMAL names, which HyperCast's library is asked to declare. A
// library older than its header refuses it: a deployment error, as HyperCast's doors say of one.
inline NumFormat comma_decimal() noexcept {
  NumFormat::Fields fields;
  fields.decimal_sep = ",";
  fields.group_sep = ".";
  const auto declared = NumFormat::declare(fields);
  if (!declared) {
    contract_violation();
  }
  return *declared;
}

// Every format the options declare, each as a plan's would be asked of the core, whichever would
// win for a column and whether or not the sample reaches its ordinal. A suggest asks this before it
// opens or reads anything, so a stale format is ErrorKind::plan on an empty source and on a broken
// one.
inline Result<void> formats_are_sound(const SuggestOptions &options) noexcept {
  Judge judge;
  const auto sound = [&judge](const NumFormat &format) noexcept {
    HyperTabularColumnSpecWith column{};
    return spec(Column::i32(0).format(format), column) && judge.accepts(column);
  };
  if (options.format && !sound(*options.format)) {
    return Error::plan(0);
  }
  for (const auto &entry : options.column_formats) {
    if (!sound(entry.second)) {
      return Error::plan(entry.first);
    }
  }
  return {};
}

// The sample, in the shape the core takes it.
class Sample {
public:
  // A sheet's sample (`sheet`) binds a second, UUID, door to every column, for what it says of a
  // cell: no door but a text door takes a typed cell's kind, so there it's malformed with no
  // detail, where a text cell that isn't a UUID has one. That's the read's word for which cells
  // were typed, and it's what keeps a typed number out of the question of what `,` and `.` mean.
  // Its notes are on.
  Sample(std::size_t width, std::size_t limit, bool sheet = false) noexcept
      : columns_(width), typed_(sheet ? width : 0), limit_(limit) {}

  // Reads a source through the text door until the sample is whole and a row past it, or the end,
  // says whether it's the source's last. A failure among the sampled rows is the caller's, and a
  // failure just past them only says there's more.
  template <class Source> Result<void> read_from(Source &source) noexcept {
    if (columns_.empty()) {
      return {};
    }
    if (columns_.size() > max_ordinal + 1) {
      return Error::plan(max_ordinal + 1);
    }
    std::vector<Column> plan;
    plan.reserve(columns_.size() + typed_.size());
    for (std::size_t ordinal = 0; ordinal < columns_.size(); ++ordinal) {
      plan.push_back(Column::text(ordinal));
    }
    for (std::size_t ordinal = 0; ordinal < typed_.size(); ++ordinal) {
      plan.push_back(Column::uuid(ordinal));
    }
    if (auto bound = source.bind(std::move(plan)); !bound.ok()) {
      return bound;
    }
    while (!truncated_) {
      auto read = source.read();
      if (const Error *error = read.error_if()) {
        if (rows_ != limit_) {
          return *error;
        }
        truncated_ = true;
      } else if (!*read.value_if()) {
        return {};
      } else {
        take(**read.value_if());
      }
    }
    return {};
  }

  // Asks the core, and builds the plan and the report from its answer.
  Result<Suggestion> judge(const Header *header, const SuggestOptions &options) noexcept {
    const std::size_t width = columns_.size();
    std::vector<std::optional<NumFormat>> declared(width);
    std::vector<HyperTabularRawNumFormat> formats(width, HyperTabularRawNumFormat{});
    for (std::size_t ordinal = 0; ordinal < width; ++ordinal) {
      declared[ordinal] = options.format;
      for (const auto &entry : options.column_formats) {
        if (entry.first == ordinal) {
          declared[ordinal] = entry.second;
        }
      }
      // Every format was judged by formats_are_sound, before the source was opened.
      if (declared[ordinal] && !raw_format(*declared[ordinal], formats[ordinal])) {
        return Error::plan(ordinal);
      }
    }
    std::vector<HyperTabularSpan> cells;
    cells.reserve(width * rows_);
    for (const auto &column : columns_) {
      cells.insert(cells.end(), column.begin(), column.end());
    }
    std::vector<std::uint8_t> typed;
    typed.reserve(typed_.size() * rows_);
    for (const auto &column : typed_) {
      typed.insert(typed.end(), column.begin(), column.end());
    }
    Suggestion suggestion;
    suggestion.truncated = truncated_;
    if (width == 0) {
      return suggestion;
    }
    std::vector<HyperTabularRawSuggestion> found(width);
    const std::uint8_t nothing = 0;
    if (::hypertabular_suggest(text_.empty() ? &nothing
                                             : reinterpret_cast<const std::uint8_t *>(text_.data()),
                               text_.size(), nullptr, 0, cells.empty() ? nullptr : cells.data(),
                               typed_.empty() ? nullptr : (typed.empty() ? &nothing : typed.data()),
                               rows_, width, formats.data(), found.data()) != HYPERTABULAR_OK) {
      return Error::plan(0); // every format was checked, and every span was made here
    }
    suggestion.plan.reserve(width);
    suggestion.report.reserve(width);
    for (std::size_t ordinal = 0; ordinal < width; ++ordinal) {
      suggestion.report.push_back(report_of(ordinal, found[ordinal], header, declared[ordinal]));
      std::optional<DateOrder> order = options.date_order;
      for (const auto &entry : options.column_date_orders) {
        if (entry.first == ordinal) {
          order = entry.second;
        }
      }
      settle(suggestion.report.back(), order);
      suggestion.plan.push_back(plan_of(suggestion.report.back()));
    }
    return suggestion;
  }

private:
  // What the sample still wants of a batch. A blank, a token and padding are all a blank span. A
  // refused cell keeps its bytes, for the core's own text door to refuse.
  void take(const Batch &batch) noexcept {
    for (std::size_t row = 0; row < batch.rows(); ++row) {
      if (rows_ == limit_) {
        truncated_ = true;
        return;
      }
      const std::size_t start = text_.size();
      std::vector<std::uint32_t> lengths(columns_.size());
      for (std::size_t column = 0; column < columns_.size(); ++column) {
        const std::size_t before = text_.size();
        const Verdict<std::string_view> cell = batch.text(column, row);
        if (const std::string_view *text = cell.value_if()) {
          text_.insert(text_.end(), text->begin(), text->end());
        } else if (cell.fault().reason != CastFailure::empty) {
          const std::string raw = batch.raw(column, row);
          text_.insert(text_.end(), raw.begin(), raw.end());
        }
        if (text_.size() > UINT32_MAX) {
          text_.resize(start); // a span holds a 32-bit offset: a sample that far along stops
          truncated_ = true;
          return;
        }
        lengths[column] = static_cast<std::uint32_t>(text_.size() - before);
      }
      auto at = static_cast<std::uint32_t>(start);
      for (std::size_t column = 0; column < columns_.size(); ++column) {
        columns_[column].push_back(HyperTabularSpan{at, lengths[column]});
        at += lengths[column];
      }
      for (std::size_t column = 0; column < typed_.size(); ++column) {
        const std::size_t second = columns_.size() + column;
        const std::optional<Fault> fault = batch.fault(second, row);
        const bool typed = fault && fault->reason == CastFailure::malformed &&
                           batch.detail(second, row) == Detail::none;
        typed_[column].push_back(typed ? 1 : 0);
      }
      ++rows_;
    }
  }

  static std::vector<Candidate> candidates_of(Door door, std::uint32_t set) noexcept {
    std::vector<Candidate> candidates;
    if (door == Door::date_ordered || door == Door::datetime) {
      for (const DateOrder order :
           {DateOrder::year_month_day, DateOrder::month_day_year, DateOrder::day_month_year}) {
        if ((set & (std::uint32_t{1} << static_cast<std::uint32_t>(order))) != 0) {
          candidates.emplace_back(order);
        }
      }
      return candidates;
    }
    if ((set & HyperTabularRawSuggestion_NOTATION_COMMA_GROUPS) != 0) {
      candidates.emplace_back(NumFormat::invariant());
    }
    if ((set & HyperTabularRawSuggestion_NOTATION_COMMA_DECIMAL) != 0) {
      candidates.emplace_back(comma_decimal());
    }
    return candidates;
  }

  static SuggestedColumn report_of(std::size_t ordinal, const HyperTabularRawSuggestion &found,
                                   const Header *header,
                                   const std::optional<NumFormat> &declared) noexcept {
    SuggestedColumn report{
        ordinal,       std::nullopt, Door::text, 0, std::nullopt, {}, SuggestStatus::unresolved,
        found.matched, found.sampled};
    if (header != nullptr && ordinal < header->size()) {
      report.name = (*header)[ordinal];
    }
    if (found.status == HyperTabularRawSuggestion_RESOLVED ||
        found.status == HyperTabularRawSuggestion_AMBIGUOUS) {
      report.status = static_cast<SuggestStatus>(found.status);
    }
    std::uint32_t param = found.param;
    if (report.status == SuggestStatus::ambiguous) {
      // An ambiguous param is a set of candidates, bit n for code n.
      report.candidates = candidates_of(static_cast<Door>(found.door), found.param);
      param = 0;
      while (param < 31 && (found.param & (std::uint32_t{1} << param)) == 0) {
        ++param;
      }
    }
    Door door = Door::text;
    std::uint32_t kept = 0;
    if (!door_of(found.door, param, door, kept)) {
      door = Door::text;
      kept = 0;
    }
    report.door = door;
    report.param = kept;
    if (report.status == SuggestStatus::resolved && numeric(door)) {
      report.format = declared ? *declared
                      : found.param == HyperTabularRawSuggestion_NOTATION_COMMA_DECIMAL
                          ? comma_decimal()
                          : NumFormat::invariant();
    }
    return report;
  }

  // Answers a date the sample left open with the order declared for its column, if that's one of
  // the candidates.
  static void settle(SuggestedColumn &report, const std::optional<DateOrder> &order) noexcept {
    if (!order || report.status != SuggestStatus::ambiguous) {
      return;
    }
    for (const Candidate &candidate : report.candidates) {
      const DateOrder *offered = std::get_if<DateOrder>(&candidate);
      if (offered != nullptr && *offered == *order) {
        report.status = SuggestStatus::resolved;
        report.candidates.clear();
        report.door = Door::date_ordered;
        report.param = static_cast<std::uint32_t>(*order);
        return;
      }
    }
  }

  // The column a report plans: only a resolved column reads through its door, a numeric one in the
  // format the report names.
  static Column plan_of(const SuggestedColumn &report) noexcept {
    if (report.status != SuggestStatus::resolved) {
      return Column::text(report.ordinal);
    }
    Column column = Column::of(report.ordinal, report.door, report.param);
    return report.format ? column.format(*report.format) : column;
  }

  std::string text_;
  std::vector<std::vector<HyperTabularSpan>> columns_;
  std::vector<std::vector<std::uint8_t>> typed_; // a byte for each span of a sheet's, or nothing
  std::size_t rows_ = 0;
  std::size_t limit_;
  bool truncated_ = false;
};

// One turn of a declared encoding's decoding, ahead of the fill. The source's reads land here, as
// the original bytes, and go through the core's transcoder into the reader's buffer.
struct Decoder {
  std::uint32_t encoding;
  std::vector<std::uint8_t> raw; // the original bytes read, at..end of them not yet decoded
  std::size_t at = 0;
  std::size_t end = 0;
  bool eof = false;
  std::uint64_t offset = 0;    // the original bytes decoded so far
  bool failed = false;         // a unit has no character: said once the fill has had all ahead
  std::uint64_t failed_at = 0; // where it is in the original input
};

// A workbook's buffers, in the shape the size-tagged fill takes them.
inline HyperTabularBuffersWith buffers_with(const HyperTabularBuffers &memory, const Packed &tokens,
                                            std::uint32_t *row_widths) noexcept {
  HyperTabularBuffersWith with{};
  with.size = HyperTabularBuffersWith_SIZE;
  with.window = memory.window;
  with.window_cap = memory.window_cap;
  with.arena = memory.arena;
  with.arena_cap = memory.arena_cap;
  with.cells = memory.cells;
  with.cells_cap = memory.cells_cap;
  with.row = memory.row;
  with.row_cap = memory.row_cap;
  with.strings = memory.strings;
  with.strings_len = memory.strings_len;
  with.table = memory.table;
  with.table_len = memory.table_len;
  with.kinds = memory.kinds;
  with.kinds_len = memory.kinds_len;
  with.tokens = packed_data(tokens);
  with.tokens_len = packed_size(tokens);
  with.row_widths = row_widths;
  return with;
}
} // namespace detail

// ---------------------------------------------------------------------------------------------
// Delimited text.

// A forward-only reader of delimited text: opened over bytes in memory, a file or a source,
// its header read (when the dialect has one), then bound to a plan once and read a batch at a
// time. Move-only.
class DelimitedReader {
public:
  // Reads up to `capacity` bytes into `into`: how many it read, 0 at the end of the input, or
  // std::nullopt for a read that failed (ErrorKind::io). It must not throw: reading is noexcept, so
  // an exception out of it, or out of a for_each_row callback, ends the program by std::terminate,
  // and no catch of the caller's is reached. It runs in the middle of a read: a read() or bind() it
  // calls on the same reader, as one from a for_each_row callback, is ErrorKind::busy, and the read
  // it is in goes on unharmed; destroying or moving the reader from there ends the program.
  using Source =
      std::function<std::optional<std::size_t>(std::uint8_t *into, std::size_t capacity)>;

  // The longest record a file or a source is buffered for.
  static constexpr std::size_t max_row_bytes = std::size_t{1} << 30;

  // Over `input`, read in place: the bytes must outlive the reader. A dialect that declares an
  // encoding has to be decoded, and reads the bytes through a buffer, as a source would.
  static Result<DelimitedReader> from_bytes(std::string_view input, Dialect dialect,
                                            DelimitedOptions options = {}) noexcept {
    return in_memory(input, dialect, options, nullptr);
  }
  static Result<DelimitedReader> from_bytes(std::string_view input, Dialect dialect,
                                            const std::vector<Column> &plan,
                                            DelimitedOptions options = {}) noexcept {
    return in_memory(input, dialect, options, &plan);
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

  // Reads a sample of the file at `path` through the text door and says what its columns look
  // like: a plan to look at, change and hand to a read, and a report. Nothing is sniffed in a read,
  // and nothing here reaches one: this is its own read of the file, opened and closed here. There's
  // no form of this for a source, which would lose the rows it sampled.
  //
  // The width is the header's, or the first record's, and a plan has to name the columns before
  // that record can be read. So with no header the file is opened twice: a path that can only be
  // read once, as a FIFO's can, is not for this. The dialect is what the sample is read with, its
  // tokens included, and a cell it says is empty is blank to the suggestion.
  //
  // Declaring the locale is the recommended route: a number format and a date order are the plan's
  // to say, and with them nothing needs inferring. This is a starting point for a plan to inspect.
  //
  // What it says can surprise, so read the report before the plan:
  //
  //   - A column of 0 and 1 is boolean, the first door asked, and F or No read false.
  //   - 1.234 alone is ambiguous, 1.234 or 1234, and reads as text until a format is declared or a
  //     value settles it, which takes in a column of prices at three decimals. Beside 2.5 it's a
  //     decimal, and beside 2,5 it's 1234.
  //   - 2,5, 0,125, 1.234,5 and 1.234.567 read only with , as the decimal, so they're resolved in
  //     that notation, and SuggestedColumn::format names it. 2,5 beside 2.5 is unresolved.
  //   - 1,23,456, Indian grouping, reads under neither notation with none declared, because nothing
  //     says its culture: alone it's text, resolved, and beside 5 unresolved. Declaring
  //     NumFormat::invariant().with_grouping(hypercast::Grouping::indian) for the column makes it a
  //     number.
  //   - 007 and an integer past int64 are text, with no note saying why, so as not to lose the
  //     zeros or the digits.
  //   - A sheet's date column is text: Sheet::suggest says why.
  //
  // Any format declared in the options that the core refuses is ErrorKind::plan first, before the
  // file is opened, though another replaces it or the sample never reaches its ordinal. Then what
  // opening the file can return is its error, and a structural failure among the sampled rows:
  // there's no knowing what's past it, and a read would stop there too. A failure past them isn't
  // met, or only says the source was truncated. An ambiguous column's comma-decimal candidate is
  // declared by HyperCast's library, so a program calling this links it.
  static Result<Suggestion> suggest(const char *path, const Dialect &dialect,
                                    const SuggestOptions &options = {}) noexcept {
    return suggested(dialect, options, [path, &dialect](const DelimitedOptions &opening) noexcept {
      return open(path, dialect, opening);
    });
  }
  // The same over text in memory.
  static Result<Suggestion> suggest_bytes(std::string_view input, const Dialect &dialect,
                                          const SuggestOptions &options = {}) noexcept {
    return suggested(dialect, options, [input, &dialect](const DelimitedOptions &opening) noexcept {
      return from_bytes(input, dialect, opening);
    });
  }

  DelimitedReader(DelimitedReader &&) noexcept = default;
  DelimitedReader &operator=(DelimitedReader &&) noexcept = default;
  DelimitedReader(const DelimitedReader &) = delete;
  DelimitedReader &operator=(const DelimitedReader &) = delete;
  ~DelimitedReader() { busy_.check(); }

  // Declares the plan of a reader opened without one, once, before the first read; held to the
  // width a header has given every record. ErrorKind::already_bound, or ErrorKind::plan, and
  // ErrorKind::busy from inside one of the reader's own reads.
  Result<void> bind(std::vector<Column> plan) noexcept {
    if (busy_.on()) {
      return Error::busy("reader");
    }
    if (columns_) {
      return Error::already_bound();
    }
    std::vector<HyperTabularColumnSpecWith> specs;
    std::size_t widest = 0;
    auto width = detail::specs(plan, specs, widest);
    if (!width.ok()) {
      return width.error();
    }
    if (auto inside = detail::within(plan, column_count()); !inside.ok()) {
      return inside;
    }
    if (auto sized =
            detail::check_sizes(specs, options_.batch_rows, options_.buffer_bytes, size_flags());
        !sized.ok()) {
      return sized.error();
    }
    const std::size_t per_row = *width.value_if() + 1;
    const std::size_t rows = detail::cell_rows(per_row, options_.batch_rows);
    cells_.assign(detail::starting_sizes.presize ? per_row * rows : detail::starting_sizes.cells,
                  HyperTabularSpan{});
    per_row_ = per_row;
    columns_ = std::make_unique<detail::Columns>(
        std::move(plan), std::move(specs), options_.batch_rows, options_.notes,
        dialect_.long_rows == LongRows::report, static_cast<std::uint32_t>(dialect_.long_rows));
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
  // ErrorKind::busy from inside one of the reader's own reads, by its source or a for_each_row
  // callback, which leaves that read to go on.
  Result<std::optional<Batch>> read() noexcept {
    if (busy_.on()) {
      return Error::busy("reader");
    }
    detail::Busy::Hold hold(busy_);
    return next();
  }

  // Reads every row left, batch by batch, handing each to `f`: f(const Row &). A read() or bind()
  // from `f` is ErrorKind::busy, and the walk goes on.
  template <class F> Result<void> for_each_row(F &&f) noexcept {
    if (busy_.on()) {
      return Error::busy("reader");
    }
    detail::Busy::Hold hold(busy_);
    for (;;) {
      auto batch = next();
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
  Result<std::optional<Batch>> next() noexcept {
    if (columns_) {
      ++columns_->generation; // whatever a batch in hand still has out is over
    }
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
            detail::Reading::delimited, state().expected, 0));
      }
      if (rows == 0) {
        return std::optional<Batch>();
      }
      if (auto more = refill(); !more.ok()) {
        return more.error();
      }
    }
  }

  DelimitedReader(Dialect dialect, DelimitedOptions options) noexcept
      : dialect_(dialect), options_(options) {
    state_.assign((::hypertabular_delimited_state_size() + 7) / 8, 0);
    arena_.assign(detail::starting_sizes.arena, 0);
  }

  static std::uint32_t size_flags(const Dialect &dialect,
                                  const DelimitedOptions &options) noexcept {
    return (options.notes ? HyperTabularRawBatchCheck_NOTES : 0u) |
           (dialect.long_rows == LongRows::report ? HyperTabularRawBatchCheck_WIDTHS : 0u);
  }
  std::uint32_t size_flags() const noexcept { return size_flags(dialect_, options_); }

  // The core's answer on these sizes, ahead of everything a reader allocates. A plan that can't be
  // honoured says so at bind, as ever, so this judges the sizes without it.
  static Result<std::size_t> sized(const Dialect &dialect, const DelimitedOptions &options,
                                   const std::vector<Column> *plan) noexcept {
    std::vector<HyperTabularColumnSpecWith> specs;
    std::size_t widest = 0;
    if (plan != nullptr && !detail::specs(*plan, specs, widest).ok()) {
      specs.clear();
    }
    return detail::check_sizes(specs, options.batch_rows, options.buffer_bytes,
                               size_flags(dialect, options));
  }

  HyperTabularDelimitedState &state() noexcept {
    return *reinterpret_cast<HyperTabularDelimitedState *>(state_.data());
  }
  const HyperTabularDelimitedState &state() const noexcept {
    return *reinterpret_cast<const HyperTabularDelimitedState *>(state_.data());
  }

  static Result<DelimitedReader> in_memory(std::string_view input, const Dialect &dialect,
                                           DelimitedOptions options,
                                           const std::vector<Column> *plan) noexcept {
    if (auto judged = sized(dialect, options, plan); !judged.ok()) {
      return judged.error();
    }
    if (dialect.encoding != Encoding::utf8) {
      std::size_t offset = 0;
      Source source =
          [input, offset](std::uint8_t *into,
                          std::size_t capacity) mutable noexcept -> std::optional<std::size_t> {
        const std::size_t count = std::min(capacity, input.size() - offset);
        if (count > 0) {
          std::memcpy(into, input.data() + offset, count);
          offset += count;
        }
        return count;
      };
      return streamed(std::move(source), dialect, options, plan);
    }
    DelimitedReader reader(dialect, options);
    reader.slice_ = input;
    return start(std::move(reader), plan);
  }

  static Result<DelimitedReader> streamed(Source source, Dialect dialect, DelimitedOptions options,
                                          const std::vector<Column> *plan) noexcept {
    auto judged = sized(dialect, options, plan);
    if (!judged.ok()) {
      return judged.error();
    }
    options.buffer_bytes = judged.value();
    DelimitedReader reader(dialect, options);
    reader.source_ = std::move(source);
    reader.limit_ = reader.options_.buffer_bytes;
    reader.stream_ = true;
    if (dialect.encoding != Encoding::utf8) {
      reader.decoder_ = detail::Decoder{
          static_cast<std::uint32_t>(dialect.encoding),
          std::vector<std::uint8_t>(std::max<std::size_t>(reader.options_.buffer_bytes, 4))};
    }
    return start(std::move(reader), plan);
  }

  template <class Open>
  static Result<Suggestion> suggested(const Dialect &dialect, const SuggestOptions &options,
                                      Open open_reader) noexcept {
    if (auto sound = detail::formats_are_sound(options); !sound.ok()) {
      return sound.error();
    }
    const std::size_t limit =
        std::min<std::size_t>(std::max<std::size_t>(options.sample_rows, 1), 4096);
    std::optional<std::size_t> width;
    if (!dialect.has_header) {
      // A headerless source's width is its first record's, which a read says through a plan of
      // one column.
      auto first = open_reader(DelimitedOptions{}.with_notes(false));
      if (!first.ok()) {
        return first.error();
      }
      DelimitedReader &reader = first.value();
      reader.options_.batch_rows = 1;
      if (auto bound = reader.bind({Column::text(0)}); !bound.ok()) {
        return bound.error();
      }
      if (auto read = reader.read(); !read.ok()) {
        return read.error();
      }
      width = reader.column_count().value_or(0);
    }
    DelimitedOptions opening;
    opening.batch_rows = 1;
    opening.notes = false;
    auto opened = open_reader(opening);
    if (!opened.ok()) {
      return opened.error();
    }
    DelimitedReader &reader = opened.value();
    const Header *header = reader.header();
    detail::Sample sample(width.value_or(header != nullptr ? header->size() : 0), limit);
    // One row more than the sample, so that a row past it costs no second batch to find, in as many
    // batches as the core says that takes.
    struct Batches {
      DelimitedReader &reader;
      std::size_t wanted;
      Result<void> bind(std::vector<Column> plan) noexcept {
        std::vector<HyperTabularColumnSpecWith> specs;
        std::size_t widest = 0;
        if (auto built = detail::specs(plan, specs, widest); !built.ok()) {
          return built.error();
        }
        auto rows = detail::sample_rows(specs, wanted, reader.size_flags());
        if (!rows.ok()) {
          return rows.error();
        }
        reader.options_.batch_rows = rows.value();
        return reader.bind(std::move(plan));
      }
      auto read() noexcept { return reader.read(); }
    } batches{reader, limit + 1};
    if (auto read = sample.read_from(batches); !read.ok()) {
      return read.error();
    }
    return sample.judge(header, options);
  }

  static Result<DelimitedReader> opened(const char *path, Dialect dialect, DelimitedOptions options,
                                        const std::vector<Column> *plan) noexcept {
    if (auto judged = sized(dialect, options, plan); !judged.ok()) {
      return judged.error();
    }
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
    const Dialect &dialect = reader.dialect_;
    const HyperTabularRawDelimitedOptions raw{HyperTabularRawDelimitedOptions_SIZE,
                                              dialect.separator,
                                              static_cast<std::uint8_t>(dialect.quoting ? 1 : 0),
                                              static_cast<std::uint8_t>(dialect.skip_blank_lines),
                                              0,
                                              dialect.skip,
                                              dialect.header_row,
                                              static_cast<std::uint32_t>(dialect.short_rows),
                                              static_cast<std::uint32_t>(dialect.long_rows)};
    if (::hypertabular_delimited_init_with(&reader.state(), &raw) != HYPERTABULAR_OK) {
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
    return reader;
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
    if (decoder_ && decoder_->failed) {
      // All the text ahead of the failure has been read, but for what the fill can't finish. A \r
      // it's holding ends a row whatever comes next, and only waits to see if a \n is part of it;
      // nothing is coming, and this says so. In a quote it ends nothing, and the record is as cut
      // as it was.
      if (buffer_.empty() || buffer_.back() != '\r') {
        return fail(Error::structure(Failure{FailureKind::encoding, state().records, state().line,
                                             decoder_->failed_at, 0, 0}));
      }
      buffer_.push_back('\n');
      return {};
    }
    if (buffer_.size() >= limit_) {
      if (limit_ >= max_row_bytes) {
        return row_too_long();
      }
      limit_ = std::min(limit_ * 2, max_row_bytes);
    }
    if (decoder_) {
      return decode();
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

  // Reads more of the text behind what the buffer holds, as refill does for UTF-8: one turn of the
  // transcoder, with whatever it wants first. A character takes at most four bytes of UTF-8, so
  // with that much room a transcode that writes nothing and doesn't fail wants more input.
  Result<void> decode() noexcept {
    detail::Decoder &d = *decoder_;
    for (;;) {
      const std::size_t filled = buffer_.size();
      buffer_.resize(limit_);
      HyperTabularRawTranscoded done{};
      const std::int32_t code =
          ::hypertabular_transcode(d.encoding, d.raw.data() + d.at, d.end - d.at, d.eof ? 1 : 0,
                                   buffer_.data() + filled, limit_ - filled, &done);
      if (code != HYPERTABULAR_OK) {
        detail::contract_violation(); // an encoding outside its enumerators
      }
      const auto consumed = static_cast<std::size_t>(done.consumed);
      if (done.failed != 0) {
        d.failed = true;
        d.failed_at = d.offset + done.failure_at;
      }
      d.at += consumed;
      d.offset += done.consumed;
      buffer_.resize(filled + static_cast<std::size_t>(done.written));
      if (done.written > 0 || d.failed) {
        return {}; // a failure is raised by the next refill, once the fill has had another look
      }
      if (d.at < d.end && limit_ - filled < 4) {
        if (limit_ >= max_row_bytes) {
          return row_too_long();
        }
        limit_ = std::min(limit_ * 2, max_row_bytes);
        continue;
      }
      if (d.eof) {
        eof_ = true;
        return {};
      }
      std::memmove(d.raw.data(), d.raw.data() + d.at, d.end - d.at);
      d.end -= d.at;
      d.at = 0;
      const auto read = source_(d.raw.data() + d.end, d.raw.size() - d.end);
      if (!read) {
        return fail(Error::io("the source failed"));
      }
      if (*read == 0) {
        d.eof = true;
      }
      d.end += std::min(*read, d.raw.size() - d.end);
    }
  }

  HyperTabularBuffersWith buffers() noexcept {
    HyperTabularBuffersWith buffers{};
    buffers.size = HyperTabularBuffersWith_SIZE;
    buffers.arena = arena_.data();
    buffers.arena_cap = arena_.size();
    buffers.cells = cells_.data();
    buffers.cells_cap = cells_.size();
    buffers.tokens = detail::packed_data(dialect_.tokens);
    buffers.tokens_len = detail::packed_size(dialect_.tokens);
    buffers.row_widths = columns_->row_widths.empty() ? nullptr : columns_->row_widths.data();
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
        detail::contract_violation(); // the dialect was judged when the reader was opened
      }
    }
  }

  // A batch from what is buffered: its rows, 0 at the end of the input, -1 for more input.
  Result<std::ptrdiff_t> fill_step() noexcept {
    const std::size_t max_rows = columns_->batch_rows;
    for (;;) {
      const auto [input, last] = window();
      const HyperTabularBuffersWith memory = buffers();
      HyperTabularFilled out{};
      const std::int32_t code = ::hypertabular_delimited_fill_with(
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
        detail::contract_violation(); // the plan was judged when it was bound
      }
    }
  }

  // First, so that a move checks it before moving anything else.
  detail::Busy busy_;
  Dialect dialect_;
  DelimitedOptions options_;
  std::vector<std::uint64_t> state_;
  // A slice: the bytes, read in place. A stream: the buffer, its limit, the source.
  bool stream_ = false;
  std::string_view slice_;
  std::vector<std::uint8_t> buffer_;
  std::size_t limit_ = 0;
  Source source_;
  std::optional<detail::Decoder> decoder_;
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
  bool skip_empty_rows = true;   // a row with no cells is not delivered; nor is a row whose only
                                 // cells are formulas with nothing spelled where a value goes. Set
                                 // it to false to keep every row
  std::size_t batch_rows = 4096; // the most rows a batch holds; the core refuses 0, and a batch
                                 // asking for more than it allows
  std::uint32_t skip = 0;        // rows to pass over at the range's start, empty ones included,
                                 // their cells not judged
  std::uint32_t header_row = 0;  // rows to read and discard after those, counted after the
                                 // empty-row filter, as a filtered read delivers them
  std::optional<Range> range = std::nullopt; // the rectangle read; the whole sheet otherwise
  detail::Packed tokens = nullptr;           // set with with_tokens
  bool notes = true;                         // whether a batch carries detail() and origin()

  SheetOptions with_header(bool on) const noexcept {
    SheetOptions options = *this;
    options.has_header = on;
    return options;
  }
  SheetOptions with_empty_rows_skipped(bool on) const noexcept {
    SheetOptions options = *this;
    options.skip_empty_rows = on;
    return options;
  }
  SheetOptions with_batch_rows(std::size_t rows) const noexcept {
    SheetOptions options = *this;
    options.batch_rows = rows;
    return options;
  }
  SheetOptions with_skip(std::uint32_t rows) const noexcept {
    SheetOptions options = *this;
    options.skip = rows;
    return options;
  }
  SheetOptions with_header_row(std::uint32_t rows) const noexcept {
    SheetOptions options = *this;
    options.header_row = rows;
    return options;
  }
  SheetOptions with_range(const Range &rectangle) const noexcept {
    SheetOptions options = *this;
    options.range = rectangle;
    return options;
  }
  // The exact bytes of a text cell that stand for no value, for every column: such a cell is Empty,
  // at Origin::token, whatever the door. A typed cell is never one. Column::tokens replaces this.
  SheetOptions with_tokens(std::initializer_list<std::string_view> list) const noexcept {
    SheetOptions options = *this;
    options.tokens = detail::pack_tokens(list.begin(), list.end());
    return options;
  }
  SheetOptions with_tokens(const std::vector<std::string> &list) const noexcept {
    SheetOptions options = *this;
    options.tokens = detail::pack_tokens(list.begin(), list.end());
    return options;
  }
  // Off, every cell says Detail::none and Origin::cell, faults() lists no formula with no cached
  // value, and a read saves the little it costs to ask a door why a cell failed.
  SheetOptions with_notes(bool on) const noexcept {
    SheetOptions options = *this;
    options.notes = on;
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
// cell formats loaded. Move-only; its sheets read from it, so it outlives them and isn't moved
// while they're in use: a sheet holds its address.
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
    return book;
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

// One sheet of a workbook, read forward a batch at a time. Move-only; its workbook outlives it and
// stays where it is, since a sheet holds its address.
class Sheet {
public:
  Sheet(Sheet &&) noexcept = default;
  Sheet &operator=(Sheet &&) noexcept = default;
  Sheet(const Sheet &) = delete;
  Sheet &operator=(const Sheet &) = delete;
  // A sheet destroyed or moved by a for_each_row callback of its own ends the program.
  ~Sheet() { busy_.check(); }

  const SheetOptions &options() const noexcept { return options_; }
  // The header row's names, or nullptr if the options say the sheet has none.
  const Header *header() const noexcept { return header_ ? &*header_ : nullptr; }
  bool is_bound() const noexcept { return columns_ != nullptr; }
  const std::vector<Column> &plan() const noexcept {
    static const std::vector<Column> none;
    return columns_ ? columns_->plan : none;
  }

  // Reads a sample of this sheet through the text door and says what its columns look like, as
  // DelimitedReader::suggest does. It's a read of its own, from the sheet's start with the sheet's
  // options, and this one's own read is left as it was. Its plan's ordinals count from the range's
  // first column, as any plan's do.
  //
  // A typed number is judged by its value and has no notation, whichever is declared: a column of
  // them is a number under any format, and its plan reads every one, since a typed cell reads the
  // same under them all. A text cell holding 1.234 keeps its question, in a column of its own or
  // beside typed numbers.
  //
  // A typed date, date-time, time or duration cell reads as its canonical text, which the suggester
  // doesn't take as a date: such a column of a sheet is text, resolved, for a plan to change by
  // hand.
  Result<Suggestion> suggest(const SuggestOptions &options = {}) const noexcept {
    if (auto sound = detail::formats_are_sound(options); !sound.ok()) {
      return sound.error();
    }
    const std::size_t limit =
        std::min(std::max<std::size_t>(options.sample_rows, 1), options_.batch_rows);
    SheetOptions reading = options_;
    reading.notes = false;
    std::optional<std::size_t> width;
    if (!options_.has_header) {
      // A headerless sheet's width is its first row's, which a read says through a plan of one
      // column.
      reading.batch_rows = 1;
      auto first = open(*book_, info_, reading, nullptr);
      if (!first.ok()) {
        return first.error();
      }
      auto widest = first.value().first_row_width();
      if (!widest.ok()) {
        return widest.error();
      }
      width = widest.value();
    }
    reading.batch_rows = 1;
    reading.notes = true; // for what the sample's second door says of a cell, in Sample::take
    auto opened = open(*book_, info_, reading, nullptr);
    if (!opened.ok()) {
      return opened.error();
    }
    Sheet &sheet = opened.value();
    const Header *header = sheet.header();
    detail::Sample sample(width.value_or(header != nullptr ? header->size() : 0), limit, true);
    // One row more than the sample, so that a row past it costs no second batch to find, in as many
    // batches as the core says that takes.
    struct Batches {
      Sheet &sheet;
      std::size_t wanted;
      Result<void> bind(std::vector<Column> plan) noexcept {
        std::vector<HyperTabularColumnSpecWith> specs;
        std::size_t widest = 0;
        if (auto built = detail::specs(plan, specs, widest); !built.ok()) {
          return built.error();
        }
        auto rows = detail::sample_rows(specs, wanted, size_flags(sheet.options_, false));
        if (!rows.ok()) {
          return rows.error();
        }
        sheet.options_.batch_rows = rows.value();
        return sheet.bind(std::move(plan));
      }
      auto read() noexcept { return sheet.read(); }
    } batches{sheet, limit + 1};
    if (auto read = sample.read_from(batches); !read.ok()) {
      return read.error();
    }
    return sample.judge(header, options);
  }

  // Declares the plan of a sheet opened without one, once, before the first read.
  // ErrorKind::already_bound, or ErrorKind::plan, and ErrorKind::busy from a for_each_row callback.
  Result<void> bind(std::vector<Column> plan) noexcept {
    if (busy_.on()) {
      return Error::busy("sheet");
    }
    return bind_with(std::move(plan), false);
  }
  Result<void> bind(std::initializer_list<Column> plan) noexcept {
    return bind(std::vector<Column>(plan));
  }

  // The next batch, or std::nullopt when the sheet has no more rows. A structural failure is
  // returned once every intact row before it has been delivered, and again on every call
  // after. ErrorKind::busy from a for_each_row callback, which leaves the walk to go on.
  Result<std::optional<Batch>> read() noexcept {
    if (busy_.on()) {
      return Error::busy("sheet");
    }
    return next();
  }

  // Reads every row left, batch by batch, handing each to `f`: f(const Row &). A read() or bind()
  // from `f` is ErrorKind::busy, and the walk goes on.
  template <class F> Result<void> for_each_row(F &&f) noexcept {
    if (busy_.on()) {
      return Error::busy("sheet");
    }
    detail::Busy::Hold hold(busy_);
    for (;;) {
      auto batch = next();
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

  Result<std::optional<Batch>> next() noexcept {
    if (columns_) {
      ++columns_->generation; // whatever a batch in hand still has out is over
    }
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
    std::uint32_t *row_widths = columns.row_widths.empty() ? nullptr : columns.row_widths.data();
    auto [code, out] = scratch_.drive(
        row_, false, tables(), [&](const HyperTabularBuffers *memory, HyperTabularFilled *result) {
          const HyperTabularBuffersWith with =
              detail::buffers_with(*memory, options_.tokens, row_widths);
          return ::hypertabular_workbook_fill_with(state, bytes, size, columns.specs.data(),
                                                   columns.pointers.data(), columns.plan.size(),
                                                   columns.batch_rows, &with, result);
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
    const std::uint32_t first_column = options_.range ? options_.range->first_column() : 0;
    return std::optional<Batch>(
        Batch(columns_.get(), static_cast<std::size_t>(out.rows), scratch_.cells.data(), per_row_,
              std::string_view(reinterpret_cast<const char *>(book.strings_.data()),
                               book.strings_.size()),
              std::string_view(reinterpret_cast<const char *>(scratch_.arena.data()),
                               scratch_.arena.size()),
              detail::Reading::workbook, 0, first_column > 0 ? first_column - 1 : 0));
  }

  Sheet(const Workbook *book, const SheetInfo &info, SheetOptions options) noexcept
      : book_(book), info_(info), options_(std::move(options)) {}

  static std::uint32_t size_flags(const SheetOptions &options, bool widths) noexcept {
    return HyperTabularRawBatchCheck_SHEET |
           (options.notes ? HyperTabularRawBatchCheck_NOTES : 0u) |
           (widths ? HyperTabularRawBatchCheck_WIDTHS : 0u);
  }

  Result<void> bind_with(std::vector<Column> plan, bool widths) noexcept {
    if (columns_) {
      return Error::already_bound();
    }
    std::vector<HyperTabularColumnSpecWith> specs;
    std::size_t widest = 0;
    auto width = detail::specs(plan, specs, widest);
    if (!width.ok()) {
      return width.error();
    }
    const std::size_t batch_rows = options_.batch_rows;
    if (auto sized = detail::check_sizes(specs, batch_rows, 0, size_flags(options_, widths));
        !sized.ok()) {
      return sized.error();
    }
    const std::size_t per_row = plan.size() + 1;
    if (detail::starting_sizes.presize && scratch_.cells.size() < batch_rows * per_row) {
      scratch_.cells.resize(batch_rows * per_row);
    }
    if (row_.size() < *width.value_if()) {
      row_.resize(*width.value_if());
    }
    per_row_ = per_row;
    columns_ = std::make_unique<detail::Columns>(std::move(plan), std::move(specs), batch_rows,
                                                 options_.notes, widths,
                                                 HyperTabularRawDelimitedOptions_LONG_ROWS_REJECT);
    return {};
  }

  // How many cells the sheet's next row has, 0 for no row: a read through a plan of one column
  // that keeps its row's width.
  Result<std::size_t> first_row_width() noexcept {
    if (auto bound = bind_with({Column::text(0)}, true); !bound.ok()) {
      return bound.error();
    }
    auto read = this->read();
    if (!read.ok()) {
      return read.error();
    }
    if (!*read.value_if()) {
      return std::size_t{0};
    }
    return static_cast<std::size_t>(columns_->row_widths[0]);
  }

  detail::Scratch::Tables tables() const noexcept {
    return detail::Scratch::Tables{&book_->strings_, &book_->table_, &book_->kinds_};
  }

  static Result<Sheet> open(const Workbook &book, const SheetInfo &info,
                            const SheetOptions &options, const std::vector<Column> *plan) noexcept {
    {
      // The core's answer on the size, ahead of everything a sheet allocates. A plan that can't be
      // honoured says so at bind, as ever, so this judges the size without it.
      std::vector<HyperTabularColumnSpecWith> specs;
      std::size_t widest = 0;
      if (plan != nullptr && !detail::specs(*plan, specs, widest).ok()) {
        specs.clear();
      }
      if (auto sized =
              detail::check_sizes(specs, options.batch_rows, 0, size_flags(options, false));
          !sized.ok()) {
        return sized.error();
      }
    }
    Sheet sheet(&book, info, options);
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
    const HyperTabularRawSheetOptions raw{HyperTabularRawSheetOptions_SIZE,
                                          options.has_header ? 1u : 0u,
                                          options.skip_empty_rows ? 1u : 0u,
                                          options.skip,
                                          options.header_row,
                                          options.range ? options.range->first_row() : 0,
                                          options.range ? options.range->first_column() : 0,
                                          options.range ? options.range->last_row() : 0,
                                          options.range ? options.range->last_column() : 0};
    const std::int32_t code = ::hypertabular_workbook_sheet_with(
        detail::state_of(sheet.state_),
        reinterpret_cast<const std::uint8_t *>(book.container_.data()), book.container_.size(),
        reinterpret_cast<const std::uint8_t *>(info.part_.data()), info.part_.size(), info.index_,
        &raw, &out);
    if (auto settled = detail::settle(code, out); !settled.ok()) {
      return settled.error();
    }
    if (options.has_header) {
      if (auto read = sheet.read_header(); !read.ok()) {
        return read.error();
      }
    }
    return sheet;
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

  // First, so that a move checks it before moving anything else.
  detail::Busy busy_;
  const Workbook *book_;
  SheetInfo info_;
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
