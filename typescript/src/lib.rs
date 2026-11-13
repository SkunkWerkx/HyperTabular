//! The TypeScript binding: HyperTabular's Rust binding (the readers in `rust/src`, over the
//! core in `rust/src/kernel`) linked into a Node-API addon (napi-rs), the same Rust for every
//! native `.node` and for the wasm32-wasip1 fallback.
//!
//! A cell is a HyperCast verdict, `{ ok: true, value }` or `{ ok: false, fault }`, shaped
//! exactly as `@skunkwerkx/hypercast` shapes one. Everything else that can go wrong is an
//! outcome, `{ ok: true, value }` or `{ ok: false, error }`: a file that cannot be read, a
//! declaration the core refuses, a plan bound out of turn, and a structurally broken file,
//! which a read returns once every intact row before it has been delivered. Nothing here
//! throws for data or for an argument of the wrong JavaScript type: each argument crosses as
//! an `Unknown` and is read here, because napi's own typed conversions throw a `TypeError`
//! before the method runs. What can still throw is the engine itself (out of memory building
//! a result, or an exception a caller's own setter or getter throws past an outcome) and
//! `new` on a class, which has no constructor.
//!
//! A column that declares HyperCast's constraints (`Column.checked`) is read through
//! `getChecked`, whose verdict is HyperCast's checked one: `{ ok: true, value, adjusted,
//! enumIndex }`, `{ ok: false, fault }` or `{ ok: false, violation }`.
//!
//! A batch is copied out of the reader as it is read, as Python's, Ruby's and PHP's are: a
//! garbage-collected caller cannot be held to "valid until the next read". A fault's span is
//! in bytes of the cell's raw text, which `raw` hands back as a `Uint8Array`.
//!
//! Every rule is the Rust binding's: what a size, a format, a name or a range may be, every
//! default, and what a read answers. This crate turns JavaScript values into the Rust
//! binding's types and its answers back, and never decides anything a Rust caller would get a
//! different answer to.
//!
//! JavaScript can run in the middle of a call: a getter or a Proxy trap in an argument, or a
//! setter a caller put on `Object.prototype`. A reader's and a sheet's state is therefore held
//! in a `RefCell` and every method takes `&self`, so a call back into the same reader or sheet
//! from that JavaScript finds it borrowed and answers `busy`, never a second `&mut` to state
//! the outer call is using.

use hypercast as hc;
use hypertabular as ht;
use napi::bindgen_prelude::{
    Array, BigInt, ClassInstance, Either, Either3, Either9, JavaScriptClassExt, Null, Object,
    ToNapiValue, Uint8Array, Unknown, sys,
};
use napi::{Env, JsValue, Result, ValueType};
use napi_derive::napi;
use std::cell::RefCell;
use std::ffi::CStr;
use std::sync::Arc;

// ---------------------------------------------------------------------------------------
// The shapes that cross: HyperCast's verdict, and this package's outcome.
// ---------------------------------------------------------------------------------------

/// The closed set of reasons a cell can fail its door: HyperCast's own.
#[napi(string_enum)]
pub enum CastFailure {
    /// The cell was absent, empty or whitespace.
    #[napi(value = "empty")]
    Empty,
    /// The cell was present but not recognizable as the door's type.
    #[napi(value = "malformed")]
    Malformed,
    /// Well-formed, but outside the door's range: `"256"` for a u8.
    #[napi(value = "outOfRange")]
    OutOfRange,
}

/// The failure case of a cell: the reason, and the offending span in bytes of the cell's raw
/// text, so `raw.subarray(offset, offset + length)` is the offending text.
#[napi(object, object_from_js = false)]
pub struct Fault {
    /// Why the cell failed its door.
    pub reason: CastFailure,
    /// Where the offending span starts, in bytes of the cell's raw text.
    pub offset: u32,
    /// How many bytes it runs; 0 for `empty` and for a contract violation.
    pub length: u32,
}

/// The success case of a verdict or an outcome: `ok` is `true` and `value` is the value.
#[napi(
    object,
    object_to_js = false,
    object_from_js = false,
    js_name = "Success<T>"
)]
pub struct SuccessCase {
    #[napi(ts_type = "true")]
    pub ok: bool,
    #[napi(ts_type = "T")]
    pub value: bool,
}

/// The failure case of a verdict: `ok` is `false` and `fault` says why.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct Failure {
    #[napi(ts_type = "false")]
    pub ok: bool,
    pub fault: Fault,
}

/// A cell as HyperCast judged it: exactly one of `Success<T>` or `Failure`, told apart by
/// `ok`. The same type `@skunkwerkx/hypercast` exports.
#[napi(js_name = "Verdict<T>")]
pub type VerdictType = Either<SuccessCase, Failure>;

/// The success case of a checked cell: `ok` is `true`; `value` is the value (rounded to
/// `multipleOf` when the declaration rounds; at the text door with an `enum`, the declared entry
/// it matched); `adjusted` is true only when rounding changed it; `enumIndex` is the index of the
/// entry a text matched, `null` otherwise. The same type `@skunkwerkx/hypercast` exports.
#[napi(
    object,
    object_to_js = false,
    object_from_js = false,
    js_name = "Checked<T>"
)]
pub struct CheckedCase {
    #[napi(ts_type = "true")]
    pub ok: bool,
    #[napi(ts_type = "T")]
    pub value: bool,
    pub adjusted: bool,
    #[napi(ts_type = "number | null")]
    pub enum_index: u32,
}

/// A rule a cell's value broke, in the OpenAPI spelling, with that value exactly as the door
/// cast it (never rounded; at the text door, the judged text, never an `enum` entry) and the
/// span of the judged text, in bytes of the cell's raw text. Keywords are open to additions, so
/// a `switch` over them needs a `default` arm; a code this package does not name yet arrives
/// as `"keyword N"`. The same type `@skunkwerkx/hypercast` exports.
#[napi(
    object,
    object_to_js = false,
    object_from_js = false,
    js_name = "Violation<T>"
)]
pub struct ViolationCase {
    #[napi(
        ts_type = "'minimum' | 'exclusiveMinimum' | 'maximum' | 'exclusiveMaximum' | 'multipleOf' | 'minLength' | 'maxLength' | 'enum' | (string & {})"
    )]
    pub keyword: String,
    #[napi(ts_type = "T")]
    pub value: bool,
    pub offset: u32,
    pub length: u32,
}

/// The failure case of a checked cell whose value broke a declared rule: `ok` is `false` and
/// `violation` says which rule, where, and the value. The same type `@skunkwerkx/hypercast`
/// exports.
#[napi(
    object,
    object_to_js = false,
    object_from_js = false,
    js_name = "Violated<T>"
)]
pub struct ViolatedCase {
    #[napi(ts_type = "false")]
    pub ok: bool,
    #[napi(ts_type = "Violation<T>")]
    pub violation: bool,
}

/// A cell of a column that declares constraints, as HyperCast's checked door judged it: exactly
/// one of `Checked<T>`, `Failure` or `Violated<T>`, told apart by `ok` and then by
/// `'fault' in verdict`. The same type `@skunkwerkx/hypercast` exports.
#[napi(js_name = "CheckedVerdict<T>")]
pub type CheckedVerdictType = Either3<CheckedCase, Failure, ViolatedCase>;

/// A HyperCast `Constraints` (`@skunkwerkx/hypercast`'s, made by `Constraints.declare`), as this
/// package reads it: the door it is bound to and the bytes it crosses a C ABI as.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct DeclaredConstraints {
    /// The door the declaration is bound to, by HyperCast's `Door` code.
    pub door: u32,
    /// Its 104 bytes.
    #[napi(ts_type = "Uint8Array")]
    pub packed: bool,
    /// Its `enum` list, empty for none.
    #[napi(ts_type = "Uint8Array")]
    pub packed_enum: bool,
}

/// What a cell holds once it has cast: the value of the door it was read through.
#[napi(js_name = "Value")]
pub type ValueType9 =
    Either9<bool, f64, BigInt, String, Timestamp, CalendarDate, TimeOfDay, CivilDateTime, Duration>;

/// What went wrong that is not a cell's verdict.
#[napi(string_enum)]
pub enum ErrorKind {
    /// The file could not be opened or read.
    #[napi(value = "io")]
    Io,
    /// The dialect's separator is not a tab or printable ASCII other than `"`.
    #[napi(value = "separator")]
    Separator,
    /// A plan column cannot be honoured; `column` is its place in the plan.
    #[napi(value = "plan")]
    Plan,
    /// The workbook has no such sheet.
    #[napi(value = "noSheet")]
    NoSheet,
    /// The header has no column of that name.
    #[napi(value = "noColumn")]
    NoColumn,
    /// The header has two columns of that name, `first` and `second`, and `require` will not
    /// choose between them.
    #[napi(value = "ambiguousColumn")]
    AmbiguousColumn,
    /// Text that is not a range, or a range whose last row or column is before its first.
    #[napi(value = "range")]
    Range,
    /// A reader opened without a plan was read before one was bound.
    #[napi(value = "unbound")]
    Unbound,
    /// A plan was bound to a reader that already has one.
    #[napi(value = "alreadyBound")]
    AlreadyBound,
    /// The data is structurally broken; `failure` says what and where.
    #[napi(value = "structure")]
    Structure,
    /// An argument of a type the declarations rule out, or a size the core refuses (a batch
    /// of no rows, a buffer of no bytes, a batch past the memory a batch may ask for).
    #[napi(value = "argument")]
    Argument,
    /// The reader, sheet or workbook was closed.
    #[napi(value = "closed")]
    Closed,
    /// JavaScript the call ran (a getter or a Proxy trap in an argument, a setter on
    /// `Object.prototype`) called back into the same reader or sheet, which a call is never
    /// re-entered by.
    #[napi(value = "busy")]
    Busy,
}

/// What a structural failure is, as the core numbers it.
#[napi(string_enum)]
pub enum FailureKind {
    /// The input ended inside a quoted cell.
    #[napi(value = "unclosedQuote")]
    UnclosedQuote,
    /// A record's cell count disagrees with the first record's.
    #[napi(value = "columnCount")]
    ColumnCount,
    /// One record is larger than a reader will buffer (1 GiB).
    #[napi(value = "rowTooLong")]
    RowTooLong,
    /// The container is not a zip file.
    #[napi(value = "notAZip")]
    NotAZip,
    /// The zip's own structure is broken.
    #[napi(value = "container")]
    Container,
    /// The part, or the document, is encrypted.
    #[napi(value = "encrypted")]
    Encrypted,
    /// A part is compressed by a method that is neither stored nor deflate.
    #[napi(value = "method")]
    Method,
    /// A part the workbook cannot be read without is missing.
    #[napi(value = "missingPart")]
    MissingPart,
    /// A part's XML ends inside a construct.
    #[napi(value = "xml")]
    Xml,
    /// A part's bytes are not a deflate stream, or stop before the stream does.
    #[napi(value = "deflate")]
    Deflate,
    /// The container is a zip but not an XLSX, ODS or Numbers workbook.
    #[napi(value = "notAWorkbook")]
    NotAWorkbook,
    /// A cell names a shared string that is not in the table.
    #[napi(value = "sharedString")]
    SharedString,
    /// More text than can be addressed.
    #[napi(value = "tooLarge")]
    TooLarge,
    /// A Numbers part is not what the IWA packaging says.
    #[napi(value = "iwa")]
    Iwa,
    /// A header name that is not UTF-8, or a unit the dialect's declared encoding has no
    /// character for; that one's `byte` counts the original input, not the decoded text.
    #[napi(value = "encoding")]
    Encoding,
}

/// A structural failure: what is broken, and where (the core's `Failure`).
#[napi(object, object_from_js = false)]
pub struct StructuralFailure {
    /// What is wrong.
    pub kind: FailureKind,
    /// Delimited text: the zero-based record, the header counted. A workbook: the part of the
    /// package being read.
    pub record: f64,
    /// Delimited text: the 1-based line the record starts on. A workbook: the sheet row.
    pub line: u32,
    /// Delimited text: the record's byte offset in the decoded text (an `encoding` failure's
    /// counts the original input). A workbook: how far into the part's inflated bytes the
    /// read had got.
    pub byte: f64,
    /// What was expected, for the kinds that say.
    pub expected: u32,
    /// What was found, for the kinds that say.
    pub found: u32,
}

/// Why a reader could not be made, or stopped.
#[napi(object, object_from_js = false)]
pub struct TabularError {
    /// What kind of error it is.
    pub kind: ErrorKind,
    /// What went wrong, in words.
    pub message: String,
    /// The structural failure, for `structure`.
    pub failure: Option<StructuralFailure>,
    /// The plan column, for `plan`.
    pub column: Option<u32>,
    /// The first column with the name, for `ambiguousColumn`.
    pub first: Option<u32>,
    /// The next column with the name, for `ambiguousColumn`.
    pub second: Option<u32>,
}

/// The failure case of an outcome: `ok` is `false` and `error` says why.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct Broken {
    #[napi(ts_type = "false")]
    pub ok: bool,
    pub error: TabularError,
}

/// The outcome of opening, binding or reading: the value, or the error that stopped it.
#[napi(js_name = "Outcome<T>")]
pub type OutcomeType = Either<SuccessCase, Broken>;

/// An instant: seconds since 1970-01-01T00:00:00Z and the nanoseconds after them.
#[napi(object, object_from_js = false)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanos: i32,
}

/// A calendar date with no time and no zone.
#[napi(object, object_from_js = false)]
pub struct CalendarDate {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

/// A wall-clock time of day with no date and no zone; `nanos` is `0..=999_999_999`.
#[napi(object, object_from_js = false)]
pub struct TimeOfDay {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanos: u32,
}

/// A civil date and time with no zone.
#[napi(object, object_from_js = false)]
pub struct CivilDateTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanos: u32,
}

/// A signed span: whole seconds, and nanoseconds with the same sign.
#[napi(object, object_from_js = false)]
pub struct Duration {
    pub seconds: i64,
    pub nanos: i32,
}

/// A HyperCast door, as a column names the one its cells go through.
#[napi(string_enum)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Door {
    #[napi(value = "bool")]
    Bool,
    #[napi(value = "i8")]
    I8,
    #[napi(value = "i16")]
    I16,
    #[napi(value = "i32")]
    I32,
    #[napi(value = "i64")]
    I64,
    #[napi(value = "u8")]
    U8,
    #[napi(value = "u16")]
    U16,
    #[napi(value = "u32")]
    U32,
    #[napi(value = "u64")]
    U64,
    #[napi(value = "f32")]
    F32,
    #[napi(value = "f64")]
    F64,
    #[napi(value = "decimal")]
    Decimal,
    #[napi(value = "uuid")]
    Uuid,
    #[napi(value = "timestamp")]
    Timestamp,
    #[napi(value = "unix")]
    Unix,
    #[napi(value = "excelSerial")]
    ExcelSerial,
    #[napi(value = "date")]
    Date,
    #[napi(value = "dateOrdered")]
    DateOrdered,
    #[napi(value = "datetime")]
    Datetime,
    #[napi(value = "time")]
    Time,
    #[napi(value = "duration")]
    Duration,
    #[napi(value = "text")]
    Text,
}

/// The kind of workbook a container holds, told by what is in it.
#[napi(string_enum)]
pub enum WorkbookFormat {
    #[napi(value = "xlsx")]
    Xlsx,
    #[napi(value = "ods")]
    Ods,
    #[napi(value = "numbers")]
    Numbers,
}

/// One sheet of a workbook, as its listing names it.
#[napi(object, object_from_js = false)]
pub struct SheetInfo {
    /// The sheet's name.
    pub name: String,
    /// Whether the workbook hides it. A hidden sheet reads like any other.
    pub hidden: bool,
}

/// What a record with fewer cells than the width is: refused (the default), or padded with
/// `empty` verdicts whose origin is `padded`.
#[napi(string_enum)]
pub enum ShortRows {
    #[napi(value = "reject")]
    Reject,
    #[napi(value = "pad")]
    Pad,
}

/// What a record with more cells than the width is: refused (the default); its extra cells
/// dropped and the row listed by `Batch.faults`; or its extra cells dropped silently.
#[napi(string_enum)]
pub enum LongRows {
    #[napi(value = "reject")]
    Reject,
    #[napi(value = "report")]
    Report,
    #[napi(value = "truncate")]
    Truncate,
}

/// What the input's bytes are: UTF-8 (the default), or one decoded to it ahead of the read.
/// Never sniffed.
#[napi(string_enum)]
pub enum Encoding {
    #[napi(value = "utf8")]
    Utf8,
    #[napi(value = "windows1252")]
    Windows1252,
    #[napi(value = "latin1")]
    Latin1,
    #[napi(value = "utf16le")]
    Utf16le,
}

/// Where a cell's verdict came from: a door judged its text (`cell`), its exact unquoted
/// bytes are a declared token (`token`), the row has no such cell (`padded`), or a
/// workbook's formula has no cached value (`uncached`). All but `cell` are `empty`.
#[napi(string_enum)]
#[derive(Clone, Copy)]
pub enum Origin {
    #[napi(value = "cell")]
    Cell,
    #[napi(value = "token")]
    Token,
    #[napi(value = "padded")]
    Padded,
    #[napi(value = "uncached")]
    Uncached,
}

/// How delimited text is written. Every field left out is the core's default: comma-
/// separated (`Dialect.CSV`), `"` quoting, a header, blank lines skipped, nothing skipped,
/// short and long rows refused, UTF-8, no tokens.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct Dialect {
    /// One character: a tab, or printable ASCII other than `"`.
    pub separator: Option<String>,
    pub quoting: Option<bool>,
    pub has_header: Option<bool>,
    pub skip_blank_lines: Option<bool>,
    /// Physical lines to pass over first, after a byte-order mark; a quote in one means
    /// nothing.
    pub skip: Option<u32>,
    /// Records to discard after those, ahead of the header if there is one.
    pub header_row: Option<u32>,
    pub short_rows: Option<ShortRows>,
    pub long_rows: Option<LongRows>,
    pub encoding: Option<Encoding>,
    /// The exact unquoted cells that stand for no value, in every column without tokens of
    /// its own: such a cell is `empty`, its origin `token`. Each distinct list is kept for the
    /// life of the process.
    #[napi(ts_type = "ReadonlyArray<string>")]
    pub tokens: Option<Vec<String>>,
}

/// How a delimited reader sizes what it owns. Each left out is the core's default.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct DelimitedOptions {
    /// The most rows a batch holds; 4096 when left out. The core refuses `0`, and a batch of
    /// the plan that would ask for more memory than a batch may.
    pub batch_rows: Option<f64>,
    /// The input buffer a file is read through to begin with (it grows to hold the longest
    /// record); 256 KiB when left out. The core refuses `0`.
    pub buffer_bytes: Option<f64>,
    /// Whether a batch keeps each cell's `detail` and `origin`; `true` when left out.
    pub notes: Option<bool>,
}

/// How a sheet is read. Each left out is the core's default: a header, empty rows skipped,
/// 4096 rows a batch, nothing skipped, the whole sheet, no tokens, notes kept.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct SheetOptions {
    pub has_header: Option<bool>,
    pub skip_empty_rows: Option<bool>,
    /// The most rows a batch holds. The core refuses `0`.
    pub batch_rows: Option<f64>,
    /// Rows at the range's start to pass over, empty ones included.
    pub skip: Option<u32>,
    /// Rows read and discarded after those, ahead of the header.
    pub header_row: Option<u32>,
    /// The part of the sheet to read: a `Range`, or its text (`'A5:F200'`).
    #[napi(ts_type = "Range | string")]
    pub range: Option<String>,
    /// The exact text cells that stand for no value, in every column without tokens of its
    /// own. Each distinct list is kept for the life of the process.
    #[napi(ts_type = "ReadonlyArray<string>")]
    pub tokens: Option<Vec<String>>,
    /// Whether a batch keeps each cell's `detail` and `origin`.
    pub notes: Option<bool>,
}

/// What a numeric column's notation is read from: a `NumFormat` of `@skunkwerkx/hypercast`,
/// whose `packed` is the 92-byte layout the core reads.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct PackedNumFormat {
    pub packed: Uint8Array,
}

/// What a column's date doors read beyond their own shapes: a `DateOptions` of
/// `@skunkwerkx/hypercast`, its month names included.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct DeclaredDateOptions {
    pub flags: u32,
    pub pivot: u32,
    #[napi(ts_type = "{ readonly packed: Uint8Array } | null")]
    pub months: Option<bool>,
}

/// What a suggestion is asked with. Each left out is the core's default: a sample of 1,000
/// rows (an explicit `0` is one row), no notation and no date order declared.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct SuggestOptions {
    /// The most rows the sample holds. It stops at 4096 rows of delimited text and at a
    /// sheet's own batch size.
    pub sample_rows: Option<f64>,
    /// Every column's notation: a HyperCast `NumFormat`.
    pub format: Option<PackedNumFormat>,
    /// A column's own notation, ahead of `format`: `[ordinal, NumFormat]` pairs, the last for
    /// an ordinal winning.
    #[napi(ts_type = "ReadonlyArray<readonly [number, PackedNumFormat]>")]
    pub column_formats: Option<Vec<bool>>,
    /// Every column's date order where the sample leaves it open: a HyperCast `DateOrder`.
    pub date_order: Option<u32>,
    /// A column's own date order, ahead of `dateOrder`: `[ordinal, DateOrder]` pairs.
    #[napi(ts_type = "ReadonlyArray<readonly [number, number]>")]
    pub column_date_orders: Option<Vec<bool>>,
}

/// A notation a suggestion names: the 92 bytes the core reads, which a `Column` factory
/// takes as its `format` and which compare with a HyperCast `NumFormat`'s own `packed`. A
/// copy, the caller's own.
#[napi(object, object_from_js = false)]
pub struct SuggestedFormat {
    pub packed: Uint8Array,
}

/// How far a sample settles a column.
#[napi(string_enum)]
pub enum SuggestStatus {
    /// One door and one declaration take every cell, and the plan reads the column through
    /// them.
    #[napi(value = "resolved")]
    Resolved,
    /// Every cell is read, but the sample cannot say which declaration it is written in:
    /// `candidates` are the choices, and the plan reads the column as text.
    #[napi(value = "ambiguous")]
    Ambiguous,
    /// Nothing to go on, or cells no door took beside cells one did: the plan reads text.
    #[napi(value = "unresolved")]
    Unresolved,
}

/// What a candidate declares.
#[napi(string_enum)]
pub enum CandidateKind {
    #[napi(value = "dateOrder")]
    DateOrder,
    #[napi(value = "numFormat")]
    NumFormat,
}

/// A declaration that would settle an ambiguous column: a date order (a HyperCast
/// `DateOrder`, in `order`) or a notation (in `format`).
#[napi(object, object_from_js = false, use_nullable = true)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub order: Option<u32>,
    pub format: Option<SuggestedFormat>,
}

/// What a sample says of one source column.
#[napi(object, object_from_js = false, use_nullable = true)]
pub struct SuggestedColumn {
    /// The zero-based source column, as a `Column` names it.
    pub ordinal: u32,
    /// The header's name for it, or `null` with no header.
    pub name: Option<String>,
    /// The door found: text when unresolved, and under the first candidate when ambiguous.
    pub door: Door,
    /// What the door declares beside itself (a `UnixPrecision`, `DateOrder` or
    /// `ExcelEpoch`), or `null`.
    pub declared: Option<u32>,
    /// The notation the plan reads a resolved numeric column in, or `null`.
    pub format: Option<SuggestedFormat>,
    /// What to choose between: empty unless ambiguous.
    pub candidates: Vec<Candidate>,
    pub status: SuggestStatus,
    /// The cells `door` took.
    pub matched: u32,
    /// The cells that are not blank.
    pub sampled: u32,
}

/// What a suggestion found: a plan to inspect, change and pass to a read, a report for each
/// source column, and whether the source has rows past the sample. Never applied.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct Suggestion {
    #[napi(ts_type = "Array<Column>")]
    pub plan: Vec<bool>,
    pub report: Vec<SuggestedColumn>,
    pub truncated: bool,
}

/// What failed in a cell, as HyperCast names it: the names `@skunkwerkx/hypercast`'s
/// `Detail` uses, and `unknown` for one a later core adds that this binding does not know.
#[napi(string_enum)]
pub enum Detail {
    #[napi(value = "blank")]
    Blank,
    #[napi(value = "unexpectedCharacter")]
    UnexpectedCharacter,
    #[napi(value = "digitExpected")]
    DigitExpected,
    #[napi(value = "delimiterExpected")]
    DelimiterExpected,
    #[napi(value = "trailingText")]
    TrailingText,
    #[napi(value = "noDigits")]
    NoDigits,
    #[napi(value = "signMisplaced")]
    SignMisplaced,
    #[napi(value = "parenthesesUnbalanced")]
    ParenthesesUnbalanced,
    #[napi(value = "markRepeated")]
    MarkRepeated,
    #[napi(value = "groupMisplaced")]
    GroupMisplaced,
    #[napi(value = "groupSize")]
    GroupSize,
    #[napi(value = "groupInFraction")]
    GroupInFraction,
    #[napi(value = "ambiguousSeparator")]
    AmbiguousSeparator,
    #[napi(value = "fractionNotAllowed")]
    FractionNotAllowed,
    #[napi(value = "fieldWidth")]
    FieldWidth,
    #[napi(value = "wrongLength")]
    WrongLength,
    #[napi(value = "componentMissing")]
    ComponentMissing,
    #[napi(value = "notRecognized")]
    NotRecognized,
    #[napi(value = "unknownUnit")]
    UnknownUnit,
    #[napi(value = "invalidUtf8")]
    InvalidUtf8,
    #[napi(value = "tooLong")]
    TooLong,
    #[napi(value = "notFinite")]
    NotFinite,
    #[napi(value = "notWhole")]
    NotWhole,
    #[napi(value = "negativeNotAllowed")]
    NegativeNotAllowed,
    #[napi(value = "tooLarge")]
    TooLarge,
    #[napi(value = "tooSmall")]
    TooSmall,
    #[napi(value = "tooPrecise")]
    TooPrecise,
    #[napi(value = "dateFieldOutOfRange")]
    DateFieldOutOfRange,
    #[napi(value = "timeFieldOutOfRange")]
    TimeFieldOutOfRange,
    #[napi(value = "outsideWindow")]
    OutsideWindow,
    #[napi(value = "nonexistentDate")]
    NonexistentDate,
    #[napi(value = "notAScalar")]
    NotAScalar,
    #[napi(value = "unknown")]
    Unknown,
}

/// One thing that went wrong in a batch (`Batch.faults`): a cell at `malformed` or
/// `outOfRange`, a formula with no cached value, a cell that broke its column's constraints, or
/// a row longer than the width under `longRows: 'report'`. Blanks, tokens and padding are not
/// problems.
#[napi(object, object_from_js = false, use_nullable = true)]
pub struct Problem {
    /// The row in the batch, from zero.
    pub row: u32,
    /// The line (delimited text) or sheet row (a workbook) it came from.
    pub line: u32,
    /// The plan column, or `null` for an entry about the row as a whole.
    pub plan_column: Option<u32>,
    /// The source column, from zero (a sheet's own, `A` being 0), or `null` for a row.
    pub source_column: Option<u32>,
    /// The verdict's reason, or `null` for a row and for a violation, which `keyword` names.
    pub reason: Option<CastFailure>,
    /// What failed, by HyperCast's `Detail` name, or `null`.
    pub detail: Option<Detail>,
    /// Where the verdict came from; `cell` for a row.
    pub origin: Origin,
    /// The span in the cell's raw text; 0 where there is none.
    pub offset: u32,
    pub length: u32,
    /// For a long row, the width the first record fixed and the cells it had; otherwise 0.
    pub expected: u32,
    pub found: u32,
    /// For a cell that broke its column's constraints, the keyword it broke, in the OpenAPI
    /// spelling (a `Violation`'s); otherwise `null`.
    #[napi(
        ts_type = "'minimum' | 'exclusiveMinimum' | 'maximum' | 'exclusiveMaximum' | 'multipleOf' | 'minLength' | 'maxLength' | 'enum' | (string & {}) | null"
    )]
    pub keyword: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Building verdicts and outcomes.
// ---------------------------------------------------------------------------------------

/// Clears an exception a caller's getter or a Proxy trap left pending, so the call answers
/// an outcome rather than rethrowing it.
fn clear_pending_exception(env: &Env) {
    let mut pending = false;
    // SAFETY: `env` is the live environment of the call in progress.
    unsafe {
        if sys::napi_is_exception_pending(env.raw(), &mut pending) == sys::Status::napi_ok
            && pending
        {
            let mut exception = std::ptr::null_mut();
            sys::napi_get_and_clear_last_exception(env.raw(), &mut exception);
        }
    }
}

/// A plain object holding exactly these own properties, defined rather than assigned, so a
/// setter on `Object.prototype` neither runs nor keeps a field off the object.
fn record<'env>(env: &'env Env, fields: &[(&CStr, sys::napi_value)]) -> Result<Object<'env>> {
    let properties: Vec<sys::napi_property_descriptor> = fields
        .iter()
        .map(|(name, value)| sys::napi_property_descriptor {
            utf8name: name.as_ptr(),
            name: std::ptr::null_mut(),
            method: None,
            getter: None,
            setter: None,
            value: *value,
            attributes: sys::PropertyAttributes::writable
                | sys::PropertyAttributes::enumerable
                | sys::PropertyAttributes::configurable,
            data: std::ptr::null_mut(),
        })
        .collect();
    // SAFETY: `env` is the live environment of the call in progress, and each descriptor's
    // name is a static C string and its value a handle made in it; `raw` is the object made.
    unsafe {
        let raw = napi::bindgen_prelude::create_object_with_properties(env.raw(), &properties)?;
        napi::bindgen_prelude::FromNapiValue::from_napi_value(env.raw(), raw)
    }
}

fn js<V: ToNapiValue>(env: &Env, value: V) -> Result<sys::napi_value> {
    // SAFETY: `env` is the live environment of the call in progress.
    unsafe { V::to_napi_value(env.raw(), value) }
}

fn success<'env, V: ToNapiValue>(env: &'env Env, value: V) -> Result<Object<'env>> {
    let value = js(env, value)?;
    record(env, &[(c"ok", js(env, true)?), (c"value", value)])
}

fn failure(env: &Env, fault: hc::Fault) -> Result<Object<'_>> {
    let fault = js(
        env,
        Fault {
            reason: cast_failure(fault.reason),
            offset: fault.offset,
            length: fault.len,
        },
    )?;
    record(env, &[(c"ok", js(env, false)?), (c"fault", fault)])
}

fn cast_failure(reason: hc::Reason) -> CastFailure {
    match reason {
        hc::Reason::Empty => CastFailure::Empty,
        hc::Reason::Malformed => CastFailure::Malformed,
        hc::Reason::OutOfRange => CastFailure::OutOfRange,
    }
}

/// A cell asked for at a column or a row the batch does not have, or with an index that is
/// not one: HyperCast's contract violation, `malformed` with an empty span.
const CONTRACT: hc::Fault = hc::Fault {
    reason: hc::Reason::Malformed,
    offset: 0,
    len: 0,
};

/// Why a call answers an error: the Rust binding's own, or one this binding raises before
/// the Rust binding is asked anything (an argument of the wrong type, a closed or busy
/// reader).
struct Refusal {
    kind: ErrorKind,
    message: String,
    column: Option<u32>,
    ambiguous: Option<(u32, u32)>,
    failure: Option<ht::Failure>,
}

type Refused<T> = std::result::Result<T, Refusal>;

impl Refusal {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Refusal {
        Refusal {
            kind,
            message: message.into(),
            column: None,
            ambiguous: None,
            failure: None,
        }
    }
}

fn argument(message: impl Into<String>) -> Refusal {
    Refusal::new(ErrorKind::Argument, message)
}

fn closed(what: &str) -> Refusal {
    Refusal::new(ErrorKind::Closed, format!("the {what} was closed"))
}

fn busy(what: &str) -> Refusal {
    Refusal::new(
        ErrorKind::Busy,
        format!(
            "the {what} is in the middle of a call; nothing that call runs can call back into it"
        ),
    )
}

/// What a getter does from inside one of its object's own calls, where it has no outcome to
/// answer in: throws the `busy` error, an `Error` whose `code` is `ERR_HYPERTABULAR_BUSY`, whose
/// `kind` is `busy` and whose message is the one every other call answers it with.
fn thrown_busy<T>(env: &Env, what: &str) -> Result<T> {
    let refusal = busy(what);
    let kind = js(env, ErrorKind::Busy)?;
    // SAFETY: `env` is the live environment of the call in progress; each handle is made in it,
    // and the descriptors' names are NUL-terminated literals.
    unsafe {
        let mut code = std::ptr::null_mut();
        let mut message = std::ptr::null_mut();
        let mut error = std::ptr::null_mut();
        let own = |name: &'static std::ffi::CStr, value| sys::napi_property_descriptor {
            utf8name: name.as_ptr(),
            name: std::ptr::null_mut(),
            method: None,
            getter: None,
            setter: None,
            value,
            attributes: sys::PropertyAttributes::writable
                | sys::PropertyAttributes::enumerable
                | sys::PropertyAttributes::configurable,
            data: std::ptr::null_mut(),
        };
        // `code` and `kind` are defined as own properties rather than assigned (as
        // `napi_create_error` would assign `code`), so no setter on the prototype chain runs.
        let made = sys::napi_create_string_utf8(
            env.raw(),
            c"ERR_HYPERTABULAR_BUSY".as_ptr(),
            "ERR_HYPERTABULAR_BUSY".len() as isize,
            &mut code,
        ) == sys::Status::napi_ok
            && sys::napi_create_string_utf8(
                env.raw(),
                refusal.message.as_ptr().cast(),
                refusal.message.len() as isize,
                &mut message,
            ) == sys::Status::napi_ok
            && sys::napi_create_error(env.raw(), std::ptr::null_mut(), message, &mut error)
                == sys::Status::napi_ok
            && {
                let properties = [own(c"code", code), own(c"kind", kind)];
                sys::napi_define_properties(env.raw(), error, properties.len(), properties.as_ptr())
                    == sys::Status::napi_ok
            }
            && sys::napi_throw(env.raw(), error) == sys::Status::napi_ok;
        if made {
            // Thrown already: napi-rs leaves a pending exception as it is.
            return Err(napi::Error::new(
                napi::Status::PendingException,
                refusal.message,
            ));
        }
    }
    Err(napi::Error::new(
        napi::Status::GenericFailure,
        refusal.message,
    ))
}

fn narrow(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn failure_kind(kind: ht::FailureKind) -> FailureKind {
    match kind {
        ht::FailureKind::UnclosedQuote => FailureKind::UnclosedQuote,
        ht::FailureKind::ColumnCount => FailureKind::ColumnCount,
        ht::FailureKind::RowTooLong => FailureKind::RowTooLong,
        ht::FailureKind::NotAZip => FailureKind::NotAZip,
        ht::FailureKind::Encrypted => FailureKind::Encrypted,
        ht::FailureKind::Method => FailureKind::Method,
        ht::FailureKind::MissingPart => FailureKind::MissingPart,
        ht::FailureKind::Xml => FailureKind::Xml,
        ht::FailureKind::Deflate => FailureKind::Deflate,
        ht::FailureKind::NotAWorkbook => FailureKind::NotAWorkbook,
        ht::FailureKind::SharedString => FailureKind::SharedString,
        ht::FailureKind::TooLarge => FailureKind::TooLarge,
        ht::FailureKind::Iwa => FailureKind::Iwa,
        ht::FailureKind::Encoding => FailureKind::Encoding,
        // `Container`, and the reading of any kind the Rust binding gains later.
        _ => FailureKind::Container,
    }
}

impl From<&ht::Error> for Refusal {
    /// The Rust binding's error, in its own words.
    fn from(error: &ht::Error) -> Refusal {
        let mut refusal = Refusal::new(ErrorKind::Io, error.to_string());
        refusal.kind = match error {
            ht::Error::Io(_) => ErrorKind::Io,
            ht::Error::Separator(_) => ErrorKind::Separator,
            ht::Error::Plan { column } => {
                refusal.column = Some(narrow(*column));
                ErrorKind::Plan
            }
            ht::Error::NoSheet(_) => ErrorKind::NoSheet,
            ht::Error::NoColumn(_) => ErrorKind::NoColumn,
            ht::Error::AmbiguousColumn { first, second, .. } => {
                refusal.ambiguous = Some((narrow(*first), narrow(*second)));
                ErrorKind::AmbiguousColumn
            }
            ht::Error::Range(_) => ErrorKind::Range,
            ht::Error::Unbound => ErrorKind::Unbound,
            ht::Error::AlreadyBound => ErrorKind::AlreadyBound,
            ht::Error::Structure(failure) => {
                refusal.failure = Some(*failure);
                ErrorKind::Structure
            }
            // The sizes the core refuses are the caller's arguments, as a Rust caller's are.
            ht::Error::ZeroBatchRows
            | ht::Error::ZeroBufferBytes
            | ht::Error::BatchTooLarge { .. }
            | ht::Error::RowTooWide { .. }
            | ht::Error::NoMemory { .. } => ErrorKind::Argument,
            // A kind the Rust binding gains later reads as the most general one.
            _ => ErrorKind::Io,
        };
        refusal
    }
}

impl From<ht::Error> for Refusal {
    fn from(error: ht::Error) -> Refusal {
        Refusal::from(&error)
    }
}

fn refused(env: &Env, refusal: Refusal) -> Result<Object<'_>> {
    clear_pending_exception(env);
    let failure = refusal.failure.map(|failure| StructuralFailure {
        kind: failure_kind(failure.kind),
        record: failure.record as f64,
        line: failure.line,
        byte: failure.byte as f64,
        expected: failure.expected,
        found: failure.found,
    });
    let error = js(
        env,
        TabularError {
            kind: refusal.kind,
            message: refusal.message,
            failure,
            column: refusal.column,
            first: refusal.ambiguous.map(|(first, _)| first),
            second: refusal.ambiguous.map(|(_, second)| second),
        },
    )?;
    record(env, &[(c"ok", js(env, false)?), (c"error", error)])
}

/// An outcome: the value `made` builds, or the refusal.
fn outcome<'env>(env: &'env Env, answer: Refused<impl ToNapiValue>) -> Result<Object<'env>> {
    match answer {
        Ok(value) => success(env, value),
        Err(refusal) => refused(env, refusal),
    }
}

// ---------------------------------------------------------------------------------------
// Reading arguments.
// ---------------------------------------------------------------------------------------

fn is_absent(value: &Unknown<'_>) -> bool {
    matches!(value.get_type(), Ok(ValueType::Undefined | ValueType::Null))
}

/// A whole number from 0 to `max`, the way an index, a count or a size crosses.
fn whole(value: &Unknown<'_>, max: f64) -> Option<f64> {
    match value.get_type() {
        Ok(ValueType::Number) => unsafe { value.cast::<f64>() }
            .ok()
            .filter(|number| number.fract() == 0.0 && (0.0..=max).contains(number)),
        _ => None,
    }
}

/// A whole number, as a `usize`, however large: one past the host's address space is the
/// largest, which the core then refuses in its own words as a size it cannot have (2^53 and
/// beyond included, which a JavaScript number holds only rounded).
fn whole_usize(value: &Unknown<'_>) -> Option<usize> {
    whole(value, f64::MAX).map(|number| usize::try_from(number as u64).unwrap_or(usize::MAX))
}

fn index(value: &Unknown<'_>) -> Option<usize> {
    whole_usize(value)
}

fn code(value: &Unknown<'_>) -> Option<u32> {
    whole(value, f64::from(u32::MAX)).map(|number| number as u32)
}

/// The bytes a reader reads: a string as its UTF-8, or a `Uint8Array`'s bytes (a `Buffer`
/// is one), copied, since the reader outlives the call.
fn bytes_of(value: &Unknown<'_>) -> Option<Vec<u8>> {
    match value.get_type() {
        Ok(ValueType::String) => unsafe { value.cast::<String>() }
            .ok()
            .map(String::into_bytes),
        Ok(ValueType::Object) => unsafe { value.cast::<Uint8Array>() }
            .ok()
            .map(|bytes| bytes.to_vec()),
        _ => None,
    }
}

fn string_of(value: &Unknown<'_>) -> Option<String> {
    match value.get_type() {
        Ok(ValueType::String) => unsafe { value.cast::<String>() }.ok(),
        _ => None,
    }
}

/// An options object: `undefined` or `null` is every default.
fn options_object<'env>(
    env: &Env,
    value: &Unknown<'env>,
    what: &str,
) -> Refused<Option<Object<'env>>> {
    match value.get_type() {
        Ok(ValueType::Undefined | ValueType::Null) => Ok(None),
        Ok(ValueType::Object) => match unsafe { value.cast::<Object>() } {
            Ok(object) => Ok(Some(object)),
            Err(_) => {
                clear_pending_exception(env);
                Err(argument(format!("{what} must be an object")))
            }
        },
        _ => Err(argument(format!("{what} must be an object"))),
    }
}

/// One field of an object, read the one way that can run the caller's JavaScript (a getter,
/// a Proxy trap); an exception it throws is cleared and refused. Absent is `None`.
fn field<'env>(
    env: &Env,
    object: &Object<'env>,
    what: &str,
    name: &str,
) -> Refused<Option<Unknown<'env>>> {
    match object.get::<Unknown>(name) {
        Ok(value) => Ok(value.filter(|value| !is_absent(value))),
        Err(_) => {
            clear_pending_exception(env);
            Err(argument(format!("{what}.{name} could not be read")))
        }
    }
}

fn flag(env: &Env, object: &Object<'_>, what: &str, name: &str) -> Refused<Option<bool>> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(None);
    };
    match value.get_type() {
        Ok(ValueType::Boolean) => unsafe { value.cast::<bool>() }
            .map(Some)
            .map_err(|_| argument(format!("{what}.{name} must be a boolean"))),
        _ => Err(argument(format!("{what}.{name} must be a boolean"))),
    }
}

/// A size: any whole number, which the core judges (`0` and too large included).
fn size(env: &Env, object: &Object<'_>, what: &str, name: &str) -> Refused<Option<usize>> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(None);
    };
    whole_usize(&value)
        .map(Some)
        .ok_or_else(|| argument(format!("{what}.{name} must be a whole number, 0 or more")))
}

/// A count of lines, records or rows: a whole number that fits in 32 bits.
fn count(env: &Env, object: &Object<'_>, what: &str, name: &str) -> Refused<Option<u32>> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(None);
    };
    code(&value).map(Some).ok_or_else(|| {
        argument(format!(
            "{what}.{name} must be a whole number from 0 to 4294967295"
        ))
    })
}

/// One of a closed set of names.
fn named<T: Copy>(
    env: &Env,
    object: &Object<'_>,
    what: &str,
    name: &str,
    names: &[(&str, T)],
) -> Refused<Option<T>> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(None);
    };
    let text = string_of(&value);
    names
        .iter()
        .find(|(known, _)| Some(*known) == text.as_deref())
        .map(|(_, member)| Some(*member))
        .ok_or_else(|| {
            let known: Vec<String> = names
                .iter()
                .map(|(known, _)| format!("'{known}'"))
                .collect();
            argument(format!("{what}.{name} must be one of {}", known.join(", ")))
        })
}

/// An array, each element read by `item` (`None` refusing it).
fn list<'env, T>(
    env: &Env,
    value: &Unknown<'env>,
    what: &str,
    mut item: impl FnMut(usize, Unknown<'env>) -> Refused<T>,
) -> Refused<Vec<T>> {
    let array = match value.is_array() {
        Ok(true) => unsafe { value.cast::<Array>() }.ok(),
        _ => None,
    };
    let Some(array) = array else {
        clear_pending_exception(env);
        return Err(argument(format!("{what} must be an array")));
    };
    let mut items = Vec::new();
    for at in 0..array.len() {
        let element = match array.get::<Unknown>(at) {
            Ok(Some(element)) => element,
            _ => {
                clear_pending_exception(env);
                return Err(argument(format!("{what}[{at}] could not be read")));
            }
        };
        items.push(item(at as usize, element)?);
    }
    Ok(items)
}

/// A list of tokens: strings, matched as their UTF-8.
fn tokens_of(env: &Env, value: &Unknown<'_>, what: &str) -> Refused<Vec<String>> {
    list(env, value, what, |at, element| {
        string_of(&element).ok_or_else(|| argument(format!("{what}[{at}] must be a string")))
    })
}

fn as_strs(tokens: &[String]) -> Vec<&str> {
    tokens.iter().map(String::as_str).collect()
}

fn dialect_of(env: &Env, value: &Unknown<'_>) -> Refused<ht::Dialect> {
    let Some(object) = options_object(env, value, "the dialect")? else {
        return Ok(ht::Dialect::CSV);
    };
    let what = "dialect";
    let mut dialect = match field(env, &object, what, "separator")? {
        None => ht::Dialect::CSV,
        Some(value) => {
            let text = string_of(&value)
                .ok_or_else(|| argument("dialect.separator must be a one-character string"))?;
            match text.as_bytes() {
                [byte] => ht::Dialect::new(*byte),
                _ => {
                    return Err(Refusal::new(
                        ErrorKind::Separator,
                        format!(
                            "separator {text:?} is not one tab or printable ASCII character other than '\"'"
                        ),
                    ));
                }
            }
        }
    };
    if let Some(quoting) = flag(env, &object, what, "quoting")? {
        dialect = dialect.with_quoting(quoting);
    }
    if let Some(header) = flag(env, &object, what, "hasHeader")? {
        dialect = dialect.with_header(header);
    }
    if let Some(skip) = flag(env, &object, what, "skipBlankLines")? {
        dialect = dialect.with_blank_lines_skipped(skip);
    }
    if let Some(skip) = count(env, &object, what, "skip")? {
        dialect = dialect.with_skip(skip);
    }
    if let Some(row) = count(env, &object, what, "headerRow")? {
        dialect = dialect.with_header_row(row);
    }
    let short = [
        ("reject", ht::ShortRows::Reject),
        ("pad", ht::ShortRows::Pad),
    ];
    if let Some(short) = named(env, &object, what, "shortRows", &short)? {
        dialect = dialect.with_short_rows(short);
    }
    let long = [
        ("reject", ht::LongRows::Reject),
        ("report", ht::LongRows::Report),
        ("truncate", ht::LongRows::Truncate),
    ];
    if let Some(long) = named(env, &object, what, "longRows", &long)? {
        dialect = dialect.with_long_rows(long);
    }
    let encodings = [
        ("utf8", ht::Encoding::Utf8),
        ("windows1252", ht::Encoding::Windows1252),
        ("latin1", ht::Encoding::Latin1),
        ("utf16le", ht::Encoding::Utf16Le),
    ];
    if let Some(encoding) = named(env, &object, what, "encoding", &encodings)? {
        dialect = dialect.with_encoding(encoding);
    }
    if let Some(tokens) = field(env, &object, what, "tokens")? {
        let tokens = tokens_of(env, &tokens, "dialect.tokens")?;
        dialect = dialect.with_tokens(&as_strs(&tokens));
    }
    Ok(dialect)
}

fn delimited_options(env: &Env, value: &Unknown<'_>) -> Refused<ht::DelimitedOptions> {
    let mut options = ht::DelimitedReader::options();
    let Some(object) = options_object(env, value, "the options")? else {
        return Ok(options);
    };
    let what = "options";
    if let Some(rows) = size(env, &object, what, "batchRows")? {
        options = options.batch_rows(rows);
    }
    if let Some(bytes) = size(env, &object, what, "bufferBytes")? {
        options = options.buffer_bytes(bytes);
    }
    if let Some(notes) = flag(env, &object, what, "notes")? {
        options = options.notes(notes);
    }
    Ok(options)
}

fn sheet_options(env: &Env, value: &Unknown<'_>) -> Refused<ht::SheetOptions> {
    let mut options = ht::SheetOptions::new();
    let Some(object) = options_object(env, value, "the options")? else {
        return Ok(options);
    };
    let what = "options";
    if let Some(header) = flag(env, &object, what, "hasHeader")? {
        options = options.with_header(header);
    }
    if let Some(skip) = flag(env, &object, what, "skipEmptyRows")? {
        options = options.with_empty_rows_skipped(skip);
    }
    if let Some(rows) = size(env, &object, what, "batchRows")? {
        options = options.with_batch_rows(rows);
    }
    if let Some(skip) = count(env, &object, what, "skip")? {
        options = options.with_skip(skip);
    }
    if let Some(row) = count(env, &object, what, "headerRow")? {
        options = options.with_header_row(row);
    }
    if let Some(range) = field(env, &object, what, "range")? {
        options = options.with_range(range_of(env, &range)?);
    }
    if let Some(tokens) = field(env, &object, what, "tokens")? {
        let tokens = tokens_of(env, &tokens, "options.tokens")?;
        options = options.with_tokens(&as_strs(&tokens));
    }
    if let Some(notes) = flag(env, &object, what, "notes")? {
        options = options.with_notes(notes);
    }
    Ok(options)
}

/// A `Range`, or text the core parses as one.
fn range_of(env: &Env, value: &Unknown<'_>) -> Refused<ht::Range> {
    if let Some(text) = string_of(value) {
        return Ok(ht::Range::parse(&text)?);
    }
    let range = match value.get_type() {
        Ok(ValueType::Object) => unsafe { value.cast::<ClassInstance<Range>>() }.ok(),
        _ => None,
    };
    match range {
        Some(range) => Ok(range.inner),
        None => {
            clear_pending_exception(env);
            Err(argument("options.range must be a Range or its text"))
        }
    }
}

/// A plan: `undefined` or `null` is none (the reader waits for `bind`); otherwise an array of
/// `Column`s. A column the binding could not make (an argument of the wrong type) is the
/// core's own `plan` error at its place.
fn plan_of(env: &Env, value: &Unknown<'_>) -> Refused<Option<Vec<ht::Column>>> {
    if is_absent(value) {
        return Ok(None);
    }
    let list = match value.is_array() {
        Ok(true) => unsafe { value.cast::<Array>() }.ok(),
        _ => None,
    };
    let Some(list) = list else {
        clear_pending_exception(env);
        return Err(argument("the plan must be an array of Columns"));
    };
    let mut plan = Vec::with_capacity(list.len() as usize);
    for at in 0..list.len() {
        let column = list
            .get::<Unknown>(at)
            .ok()
            .flatten()
            .and_then(|item| unsafe { item.cast::<ClassInstance<Column>>() }.ok());
        let Some(column) = column else {
            clear_pending_exception(env);
            let mut refusal = Refusal::from(ht::Error::Plan {
                column: at as usize,
            });
            refusal.message = format!("plan column {at} is not a Column");
            return Err(refusal);
        };
        match column.resolved {
            Some(resolved) => plan.push(resolved),
            None => {
                return Err(ht::Error::Plan {
                    column: at as usize,
                }
                .into());
            }
        }
    }
    Ok(Some(plan))
}

/// A `format` argument: absent is none, which reads as the core's default; otherwise its
/// `packed` bytes, the 92-byte layout `@skunkwerkx/hypercast`'s `NumFormat` hands out, as
/// HyperCast resolves them. `Err` for anything else, a retired grouping word included.
fn format_of(env: &Env, value: &Unknown<'_>) -> std::result::Result<Option<hc::NumFormat>, ()> {
    if is_absent(value) {
        return Ok(None);
    }
    let object = match value.get_type() {
        Ok(ValueType::Object | ValueType::Function) => unsafe { value.cast::<Object>() }.ok(),
        _ => None,
    }
    .ok_or(())?;
    let packed = match object.get::<Unknown>("packed") {
        Ok(Some(packed)) => match packed.get_type() {
            Ok(ValueType::Object) => unsafe { packed.cast::<Uint8Array>() }.ok(),
            _ => None,
        },
        _ => None,
    };
    let Some(packed) = packed else {
        clear_pending_exception(env);
        return Err(());
    };
    let bytes: [u8; 92] = packed.as_ref().try_into().map_err(|_| ())?;
    hc::RawNumFormat::from_le_bytes(bytes)
        .resolve()
        .copied()
        .map(Some)
        .ok_or(())
}

fn date_order_of(value: &Unknown<'_>) -> Option<hc::DateOrder> {
    code(value).and_then(hc::DateOrder::from_code)
}

/// An `[ordinal, value]` pair.
fn pair<'env>(env: &Env, element: &Unknown<'env>, what: &str) -> Refused<(usize, Unknown<'env>)> {
    let parts = list(env, element, what, |_, part| Ok(part))?;
    match <[Unknown; 2]>::try_from(parts) {
        Ok([ordinal, value]) => index(&ordinal)
            .map(|ordinal| (ordinal, value))
            .ok_or_else(|| argument(format!("{what}[0] must be a whole-number ordinal"))),
        Err(_) => Err(argument(format!("{what} must be an [ordinal, value] pair"))),
    }
}

fn suggest_options(env: &Env, value: &Unknown<'_>) -> Refused<ht::SuggestOptions> {
    let mut options = ht::SuggestOptions::new();
    let Some(object) = options_object(env, value, "the options")? else {
        return Ok(options);
    };
    let what = "options";
    if let Some(rows) = size(env, &object, what, "sampleRows")? {
        options = options.sample_rows(rows);
    }
    // A format HyperCast won't resolve is the core's `plan` error at its ordinal (`0` for every
    // column's), as the Rust binding's own check of a suggestion's formats answers.
    let stale = |ordinal: usize| Refusal::from(ht::Error::Plan { column: ordinal });
    if let Some(format) = field(env, &object, what, "format")?
        && let Some(format) = format_of(env, &format).map_err(|()| stale(0))?
    {
        options = options.format(format);
    }
    if let Some(entries) = field(env, &object, what, "columnFormats")? {
        let entries = list(env, &entries, "options.columnFormats", |at, element| {
            let (ordinal, format) = pair(env, &element, &format!("options.columnFormats[{at}]"))?;
            match format_of(env, &format) {
                Ok(Some(format)) => Ok((ordinal, format)),
                _ => Err(stale(ordinal)),
            }
        })?;
        for (ordinal, format) in entries {
            options = options.column_format(ordinal, format);
        }
    }
    if let Some(order) = field(env, &object, what, "dateOrder")? {
        let order = date_order_of(&order)
            .ok_or_else(|| argument("options.dateOrder must be a DateOrder"))?;
        options = options.date_order(order);
    }
    if let Some(entries) = field(env, &object, what, "columnDateOrders")? {
        let entries = list(env, &entries, "options.columnDateOrders", |at, element| {
            let what = format!("options.columnDateOrders[{at}]");
            let (ordinal, order) = pair(env, &element, &what)?;
            date_order_of(&order)
                .map(|order| (ordinal, order))
                .ok_or_else(|| argument(format!("{what}[1] must be a DateOrder")))
        })?;
        for (ordinal, order) in entries {
            options = options.column_date_order(ordinal, order);
        }
    }
    Ok(options)
}

/// The state a reader or a sheet keeps, for this call alone: `None` (and the caller answered
/// `busy`) when a call already holds it, as one does while JavaScript it runs calls back in.
fn exclusive<T, R>(cell: &RefCell<T>, call: impl FnOnce(&mut T) -> R) -> Option<R> {
    let mut held = cell.try_borrow_mut().ok()?;
    Some(call(&mut held))
}

/// The state, to look at: `None` while a call that changes it holds it.
fn shared<T, R>(cell: &RefCell<T>, call: impl FnOnce(&T) -> R) -> Option<R> {
    let held = cell.try_borrow().ok()?;
    Some(call(&held))
}

// ---------------------------------------------------------------------------------------
// Columns.
// ---------------------------------------------------------------------------------------

/// One output column: the source column it reads, the door its cells go through, and what it
/// declares beside them. Made by a factory (`Column.i32(0)`), never by `new`. A factory never
/// refuses: a column whose arguments are not of the declared types is refused, with its
/// place in the plan, when the plan is bound, as is one the core cannot honour (an ordinal
/// past the core's limit, a format or date options HyperCast refuses).
#[napi]
pub struct Column {
    /// `None` only for `Column.checked` given no declaration, whose door is the declaration's.
    door: Option<Door>,
    ordinal: f64,
    /// Whether it was made by `Column.checked`.
    checked: bool,
    resolved: Option<ht::Column>,
}

impl Column {
    fn make(
        env: &Env,
        door: Door,
        ordinal: &Unknown<'_>,
        declared: Option<ht::Door>,
        format: std::result::Result<Option<hc::NumFormat>, ()>,
    ) -> Column {
        let given = match ordinal.get_type() {
            Ok(ValueType::Number) => unsafe { ordinal.cast::<f64>() }.unwrap_or(f64::NAN),
            _ => f64::NAN,
        };
        clear_pending_exception(env);
        let resolved = match (index(ordinal), declared, format) {
            (Some(at), Some(door), Ok(format)) => {
                let column = ht::Column::new(at, door);
                Some(match format {
                    Some(format) => column.format(format),
                    None => column,
                })
            }
            _ => None,
        };
        Column {
            door: Some(door),
            ordinal: given,
            checked: false,
            resolved,
        }
    }

    fn of(column: ht::Column) -> Column {
        Column {
            door: Some(door_name(column.door)),
            ordinal: column.ordinal as f64,
            checked: column.is_checked(),
            resolved: Some(column),
        }
    }

    fn with(&self, change: impl FnOnce(ht::Column) -> Option<ht::Column>) -> Column {
        Column {
            door: self.door,
            ordinal: self.ordinal,
            checked: self.checked,
            resolved: self.resolved.and_then(change),
        }
    }
}

// One `#[napi] impl` block a factory: napi-rs reads the items of an impl block before macros
// in it expand, so a macro can make whole blocks but not the items inside one. Each doc is a
// literal, not a `///` comment: a comment passed through a macro reaches napi's type generator
// as a raw string token, and binding.d.ts would print it with its `r"` quotes.
macro_rules! plain_columns {
    ($($doc:literal $name:ident => $door:ident;)+) => {$(
        #[napi]
        impl Column {
            #[doc = $doc]
            #[napi(catch_unwind, ts_args_type = "ordinal: number")]
            pub fn $name(env: &Env, ordinal: Unknown<'_>) -> Column {
                Column::make(env, Door::$door, &ordinal, Some(ht::Door::$door), Ok(None))
            }
        }
    )+};
}

macro_rules! numeric_columns {
    ($($doc:literal $name:ident => $door:ident;)+) => {$(
        #[napi]
        impl Column {
            #[doc = $doc]
            #[napi(
                catch_unwind,
                ts_args_type = "ordinal: number, format?: PackedNumFormat | undefined | null"
            )]
            pub fn $name(env: &Env, ordinal: Unknown<'_>, format: Unknown<'_>) -> Column {
                let format = format_of(env, &format);
                Column::make(env, Door::$door, &ordinal, Some(ht::Door::$door), format)
            }
        }
    )+};
}

plain_columns! {
    "A boolean, read under HyperCast's natural-language lexicon."
    bool => Bool;
    "A UUID, as its lowercase hyphenated text."
    uuid => Uuid;
    "An RFC 3339 instant."
    timestamp => Timestamp;
    "A strict ISO `yyyy-MM-dd` date."
    date => Date;
    "An ISO 24-hour time of day."
    time => Time;
    "An ISO 8601 duration."
    duration => Duration;
    "The cell's text itself."
    text => Text;
}

numeric_columns! {
    "A signed 8-bit integer."
    i8 => I8;
    "A signed 16-bit integer."
    i16 => I16;
    "A signed 32-bit integer."
    i32 => I32;
    "A signed 64-bit integer, as a `bigint`."
    i64 => I64;
    "An unsigned 8-bit integer."
    u8 => U8;
    "An unsigned 16-bit integer."
    u16 => U16;
    "An unsigned 32-bit integer."
    u32 => U32;
    "An unsigned 64-bit integer, as a `bigint`."
    u64 => U64;
    "An IEEE single, widened exactly to a number."
    f32 => F32;
    "An IEEE double."
    f64 => F64;
    "An exact decimal, as its canonical text."
    decimal => Decimal;
}

#[napi]
impl Column {
    /// A count of seconds, milliseconds, microseconds or nanoseconds since the Unix epoch, in
    /// the declared `UnixPrecision`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, precision: number")]
    pub fn unix(env: &Env, ordinal: Unknown<'_>, precision: Unknown<'_>) -> Column {
        let door = code(&precision)
            .and_then(hc::UnixPrecision::from_code)
            .map(ht::Door::Unix);
        Column::make(env, Door::Unix, &ordinal, door, Ok(None))
    }

    /// An Excel date serial in the declared `ExcelEpoch`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, epoch: number")]
    pub fn excel_serial(env: &Env, ordinal: Unknown<'_>, epoch: Unknown<'_>) -> Column {
        let door = code(&epoch)
            .and_then(hc::ExcelEpoch::from_code)
            .map(ht::Door::ExcelSerial);
        Column::make(env, Door::ExcelSerial, &ordinal, door, Ok(None))
    }

    /// A separated calendar date whose fields come in the declared `DateOrder`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, order: number")]
    pub fn date_ordered(env: &Env, ordinal: Unknown<'_>, order: Unknown<'_>) -> Column {
        let door = date_order_of(&order).map(ht::Door::DateOrdered);
        Column::make(env, Door::DateOrdered, &ordinal, door, Ok(None))
    }

    /// A zone-less civil date-time whose date fields come in the declared `DateOrder`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, order: number")]
    pub fn datetime(env: &Env, ordinal: Unknown<'_>, order: Unknown<'_>) -> Column {
        let door = date_order_of(&order).map(ht::Door::DateTime);
        Column::make(env, Door::Datetime, &ordinal, door, Ok(None))
    }

    /// Source column `ordinal` through the door a HyperCast `Constraints` is bound to (an
    /// integer, float or decimal door, or the text door), whose cells must also meet it: read
    /// through `getChecked`, a cell that casts and breaks a declared rule is a violation carrying
    /// the value as cast, one that casts and meets them all passes, flagged `adjusted` when
    /// `multipleOf`'s rounding changed it, and one that doesn't cast is its fault. A numeric
    /// door reads its numbers in `format`, as the plain factories do. As with every factory, an
    /// argument that is not what it says (no `Constraints`, a failed `Constraints.declare`'s
    /// missing `.value` among them) makes a column that is not `valid`, refused when its plan is
    /// bound: never a column with no rules. Each distinct declaration is kept for the life of
    /// the process.
    #[napi(
        catch_unwind,
        ts_args_type = "ordinal: number, constraints: DeclaredConstraints, format?: PackedNumFormat | undefined | null"
    )]
    pub fn checked(
        env: &Env,
        ordinal: Unknown<'_>,
        constraints: Unknown<'_>,
        format: Unknown<'_>,
    ) -> Column {
        let declared = declaration_of(env, &constraints);
        let format = format_of(env, &format);
        let given = match ordinal.get_type() {
            Ok(ValueType::Number) => unsafe { ordinal.cast::<f64>() }.unwrap_or(f64::NAN),
            _ => f64::NAN,
        };
        clear_pending_exception(env);
        let door = declared.as_ref().and_then(|(door, ..)| checked_door(*door));
        let resolved = match (index(&ordinal), declared, format) {
            (Some(at), Some((door, packed, entries)), Ok(format)) => {
                checked_column(at, door, &packed, &entries).map(|column| match format {
                    Some(format) => column.format(format),
                    None => column,
                })
            }
            _ => None,
        };
        Column {
            door: door.map(door_name),
            ordinal: given,
            checked: true,
            resolved,
        }
    }

    /// The same column answering to these tokens rather than its dialect's or sheet's: a
    /// cell whose exact unquoted text is one of them is `empty`, its origin `token`. An empty
    /// list means none, so a column can opt out of its dialect's. Each distinct list is kept
    /// for the life of the process: a program building lists from request data grows without
    /// bound.
    #[napi(catch_unwind, ts_args_type = "tokens: ReadonlyArray<string>")]
    pub fn with_tokens(&self, env: &Env, tokens: Unknown<'_>) -> Column {
        let tokens = tokens_of(env, &tokens, "tokens").ok();
        clear_pending_exception(env);
        self.with(|column| tokens.map(|tokens| column.tokens(&as_strs(&tokens))))
    }

    /// The same column reading dates under a HyperCast `DateOptions`, its month names
    /// included: the `timestamp`, `date`, `dateOrdered` and `datetime` doors read them. On a
    /// sheet they reach only a text cell. Each distinct month list is kept for the life of the
    /// process.
    #[napi(catch_unwind, ts_args_type = "options: DeclaredDateOptions")]
    pub fn with_date_options(&self, env: &Env, options: Unknown<'_>) -> Column {
        let declared = date_options_of(env, &options);
        clear_pending_exception(env);
        self.with(|column| {
            let (options, months) = declared?;
            let months = match &months {
                Some(packed) => Some(hc::MonthNames::from_packed(packed).ok()?),
                None => None,
            };
            Some(column.date_options(options, months))
        })
    }

    /// The door its cells go through: for `Column.checked`, its declaration's, so `null` there
    /// for a declaration that is none.
    #[napi(getter, catch_unwind)]
    pub fn door(&self) -> Option<Door> {
        self.door
    }

    /// Whether it declares HyperCast's constraints: made by `Column.checked`, and read through
    /// `getChecked`.
    #[napi(getter, catch_unwind, js_name = "checked")]
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// The source column it reads, as it was given (`NaN` for an ordinal that is no number).
    #[napi(getter, catch_unwind)]
    pub fn ordinal(&self) -> f64 {
        self.ordinal
    }

    /// The notation a number door reads in, as its 92 bytes (other doors carry one too, and
    /// read nothing of it); `null` for a column that is not valid.
    #[napi(getter, catch_unwind)]
    pub fn format(&self) -> Option<SuggestedFormat> {
        self.resolved.map(|column| format_out(column.format))
    }

    /// Whether its arguments were of the declared types. A plan holding a column that is not
    /// valid is refused when it is bound, with the column's place in the plan; so is one the
    /// core cannot honour, valid or not.
    #[napi(getter, catch_unwind)]
    pub fn valid(&self) -> bool {
        self.resolved.is_some()
    }
}

/// A HyperCast `DateOptions`: its flags, its pivot and its month names' packed bytes.
fn date_options_of(env: &Env, value: &Unknown<'_>) -> Option<(hc::DateOptions, Option<Vec<u8>>)> {
    let object = match value.get_type() {
        Ok(ValueType::Object) => unsafe { value.cast::<Object>() }.ok(),
        _ => None,
    }?;
    let what = "options";
    let flags = field(env, &object, what, "flags").ok()?;
    let pivot = field(env, &object, what, "pivot").ok()?;
    let flags = flags.as_ref().map_or(Some(0), code)?;
    let pivot = u16::try_from(pivot.as_ref().map_or(Some(0), code)?).ok()?;
    let months = match field(env, &object, what, "months").ok()? {
        None => None,
        Some(months) => {
            let names = match months.get_type() {
                Ok(ValueType::Object) => unsafe { months.cast::<Object>() }.ok(),
                _ => None,
            }?;
            let packed = names.get::<Unknown>("packed").ok().flatten()?;
            let packed = match packed.get_type() {
                Ok(ValueType::Object) => unsafe { packed.cast::<Uint8Array>() }.ok(),
                _ => None,
            }?;
            Some(packed.to_vec())
        }
    };
    Some((hc::DateOptions::from_flags(flags).with_pivot(pivot), months))
}

/// A HyperCast `Constraints`: the code of the door it is bound to, its 104 `packed` bytes and
/// its `packedEnum`. `None` for anything else, a failed declaration's missing `.value` included.
fn declaration_of(env: &Env, value: &Unknown<'_>) -> Option<(u32, Vec<u8>, Vec<u8>)> {
    let object = match value.get_type() {
        Ok(ValueType::Object) => unsafe { value.cast::<Object>() }.ok(),
        _ => None,
    }?;
    let what = "constraints";
    let door = field(env, &object, what, "door").ok()??;
    let door = code(&door)?;
    let bytes = |name: &str| -> Option<Vec<u8>> {
        let field = field(env, &object, what, name).ok()??;
        match field.get_type() {
            Ok(ValueType::Object) => unsafe { field.cast::<Uint8Array>() }
                .ok()
                .map(|bytes| bytes.to_vec()),
            _ => None,
        }
    };
    let packed = bytes("packed")?;
    let entries = bytes("packedEnum")?;
    Some((door, packed, entries))
}

/// The doors a HyperCast `Constraints` can be bound to, by HyperCast's `Door` code.
fn checked_door(code: u32) -> Option<ht::Door> {
    [
        (hc::Door::I8, ht::Door::I8),
        (hc::Door::I16, ht::Door::I16),
        (hc::Door::I32, ht::Door::I32),
        (hc::Door::I64, ht::Door::I64),
        (hc::Door::U8, ht::Door::U8),
        (hc::Door::U16, ht::Door::U16),
        (hc::Door::U32, ht::Door::U32),
        (hc::Door::U64, ht::Door::U64),
        (hc::Door::F32, ht::Door::F32),
        (hc::Door::F64, ht::Door::F64),
        (hc::Door::Decimal, ht::Door::Decimal),
        (hc::Door::Text, ht::Door::Text),
    ]
    .into_iter()
    .find(|(declared, _)| declared.code() == code)
    .map(|(_, door)| door)
}

/// A column reading `ordinal` that declares the constraints a HyperCast `Constraints` packed
/// for the door whose code is `door`; `None` for bytes that aren't its layout and for a
/// declaration HyperCast refuses.
fn checked_column(ordinal: usize, door: u32, packed: &[u8], entries: &[u8]) -> Option<ht::Column> {
    let raw = raw_constraints(packed)?;
    macro_rules! typed {
        ($ty:ty) => {
            ht::Column::checked(
                ordinal,
                hc::Constraints::<$ty>::from_raw(raw, entries).ok()?,
            )
        };
    }
    Some(match checked_door(door)? {
        ht::Door::I8 => typed!(i8),
        ht::Door::I16 => typed!(i16),
        ht::Door::I32 => typed!(i32),
        ht::Door::I64 => typed!(i64),
        ht::Door::U8 => typed!(u8),
        ht::Door::U16 => typed!(u16),
        ht::Door::U32 => typed!(u32),
        ht::Door::U64 => typed!(u64),
        ht::Door::F32 => typed!(f32),
        ht::Door::F64 => typed!(f64),
        ht::Door::Decimal => typed!(hc::Decimal),
        _ => typed!(hc::Text),
    })
}

/// A `Constraints`' `packed` bytes as HyperCast lays them out, in the host's byte order: six
/// words, then the five decimals, each its magnitude's low 64 bits and high 32, its scale and
/// its sign. `None` for bytes of another length.
fn raw_constraints(packed: &[u8]) -> Option<hc::RawConstraints> {
    if packed.len() != hc::RawConstraints::SIZE as usize {
        return None;
    }
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(packed.get(at..at + 4)?.try_into().ok()?))
    };
    let decimal = |at: usize| -> Option<hc::Decimal> {
        Some(hc::Decimal {
            lo: u64::from_ne_bytes(packed.get(at..at + 8)?.try_into().ok()?),
            hi: word(at + 8)?,
            scale: *packed.get(at + 12)?,
            negative: *packed.get(at + 13)? != 0,
        })
    };
    Some(hc::RawConstraints {
        size: word(0)?,
        flags: word(4)?,
        rounding: word(8)?,
        min_length: word(12)?,
        max_length: word(16)?,
        _reserved: word(20)?,
        minimum: decimal(24)?,
        maximum: decimal(40)?,
        multiple_of: decimal(56)?,
        exclusive_minimum: decimal(72)?,
        exclusive_maximum: decimal(88)?,
    })
}

// ---------------------------------------------------------------------------------------
// The header and ranges.
// ---------------------------------------------------------------------------------------

/// The names a source's header declared, in column order.
#[napi]
pub struct Header {
    inner: ht::Header,
}

#[napi]
impl Header {
    /// Every name, in column order (bytes that are not UTF-8 replaced by U+FFFD).
    #[napi(getter, catch_unwind)]
    pub fn names(&self) -> Vec<String> {
        self.inner
            .names()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect()
    }

    /// How many columns the header names.
    #[napi(getter, catch_unwind)]
    pub fn length(&self) -> u32 {
        narrow(self.inner.len())
    }

    /// The first column with exactly this name (byte for byte, case and spaces included; a
    /// `Uint8Array` is matched against the bytes the file holds), or `null` if none has it.
    /// Two columns with the name answer the first: `require` refuses that instead.
    #[napi(catch_unwind, ts_args_type = "name: string | Uint8Array")]
    pub fn ordinal(&self, name: Unknown<'_>) -> Option<u32> {
        let wanted = bytes_of(&name)?;
        self.inner.ordinal(wanted).map(narrow)
    }

    /// The column with exactly this name: the `noColumn` error naming it if none has it, and
    /// the `ambiguousColumn` error naming both if two have, since a plan built from names must
    /// not choose for its caller.
    #[napi(
        catch_unwind,
        ts_args_type = "name: string | Uint8Array",
        ts_return_type = "Outcome<number>"
    )]
    pub fn require<'env>(&self, env: &'env Env, name: Unknown<'env>) -> Result<Object<'env>> {
        let answer = bytes_of(&name)
            .ok_or_else(|| argument("the name must be a string or a Uint8Array"))
            .and_then(|wanted| Ok(narrow(self.inner.require(wanted)?)));
        outcome(env, answer)
    }
}

/// A part of a sheet: rows and columns, each 1-based, `0` meaning no limit on that side.
/// Made by `Range.parse` or `Range.of`, never by `new`.
#[napi]
pub struct Range {
    inner: ht::Range,
}

#[napi]
impl Range {
    /// A range from its text, `A5:F200`: capital column letters, both corners, the last not
    /// before the first. Anything else is the `range` error.
    #[napi(
        catch_unwind,
        ts_args_type = "text: string",
        ts_return_type = "Outcome<Range>"
    )]
    pub fn parse<'env>(env: &'env Env, text: Unknown<'env>) -> Result<Object<'env>> {
        let made = string_of(&text)
            .ok_or_else(|| argument("the range must be a string"))
            .and_then(|text| Ok(ht::Range::parse(&text)?));
        match made {
            Ok(inner) => success(env, Range { inner }.into_instance(env)?),
            Err(refusal) => refused(env, refusal),
        }
    }

    /// A range from its four numbers, each 1-based, `0` for no limit on that side: what text
    /// cannot say, such as every row from the fifth. A last before its first is the `range`
    /// error.
    #[napi(
        catch_unwind,
        ts_args_type = "firstRow: number, firstColumn: number, lastRow: number, lastColumn: number",
        ts_return_type = "Outcome<Range>"
    )]
    pub fn of<'env>(
        env: &'env Env,
        first_row: Unknown<'env>,
        first_column: Unknown<'env>,
        last_row: Unknown<'env>,
        last_column: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let made = match (
            code(&first_row),
            code(&first_column),
            code(&last_row),
            code(&last_column),
        ) {
            (Some(a), Some(b), Some(c), Some(d)) => {
                ht::Range::new(a, b, c, d).map_err(Refusal::from)
            }
            _ => Err(argument(
                "each side of a range must be a whole number from 0 to 4294967295",
            )),
        };
        match made {
            Ok(inner) => success(env, Range { inner }.into_instance(env)?),
            Err(refusal) => refused(env, refusal),
        }
    }

    #[napi(getter, catch_unwind)]
    pub fn first_row(&self) -> u32 {
        self.inner.first_row()
    }

    #[napi(getter, catch_unwind)]
    pub fn first_column(&self) -> u32 {
        self.inner.first_column()
    }

    #[napi(getter, catch_unwind)]
    pub fn last_row(&self) -> u32 {
        self.inner.last_row()
    }

    #[napi(getter, catch_unwind)]
    pub fn last_column(&self) -> u32 {
        self.inner.last_column()
    }
}

// ---------------------------------------------------------------------------------------
// Batches: what a read copied out of the reader.
// ---------------------------------------------------------------------------------------
//
// A read copies its batch into one store of bytes. Under Node, Bun and Deno that store is an
// `ArrayBuffer` V8 allocates, held by the batch object as a property; the native object
// behind the batch keeps only its row count and its doors. So a batch the caller drops is
// reclaimed by V8's own collection, inside a synchronous loop too, where a finalizer the
// engine defers to the next turn of the event loop would hold every batch until the loop
// returned. Under WebAssembly the store is a buffer of the module's own memory, held by the
// native object, since emnapi would copy a JavaScript buffer into that memory on every look.
//
// Nothing here runs JavaScript while it borrows the store: a call copies what it needs out
// (a value, a span's bytes, the faults list) and only then builds what it hands back, since
// JavaScript that ran could detach the buffer. A store that is not one, or was detached, is
// a batch of no rows and no columns.
//
// The store's layout, every word a `u64` in the host's byte order:
//
// - a head: rows, columns, where the lines are, where the widths are (0 for none), the width a
//   read that reports long rows expected;
// - each column's 11 words: how its values are kept (`KIND_*`), its source column, how many of
//   its cells did not cast, where its verdicts (0 for a column whose every cell passed), notes
//   (0 for none), values, raw spans and heap are, the heap's length, whether the column is wide
//   (a heap past what a `u32` reaches), and where its checks are (0 for a column that declares
//   no constraints);
// - then the sections, each starting on 8 bytes: the lines and the widths (`u32` a row), and
//   for each column its verdicts (`CellVerdict`, 12 bytes a row), kept only for a column with a
//   cell that did not pass (a fault or a violation), its notes (`CellNote`, 4 bytes a row),
//   kept only for a column with a cell that did not cast, its values (a `Shape` a row: one
//   word, two, or a text's place in the heap), its raw spans (a place in the heap a row: two
//   `u32`s, or two `u64`s in a wide column), its heap (each cell's raw text, back to back, then
//   each text value that is not its raw text) and its checks (`RawCheck`, 12 bytes a row), kept
//   only for a column that declares constraints. A constrained column's value is the checked
//   door's: on a violation, the value as cast; at the text door, a matched `enum` entry.
//
// The faults list is worked out from the verdicts, notes and checks when it is asked for,
// through the Rust binding's `KeptBatch`.

const HEAD: usize = 5;
const COLUMN_WORDS: usize = 11;

/// How a column's values are kept: the value of each `Value` variant.
const KIND_BOOL: u64 = 0;
const KIND_INT: u64 = 1;
const KIND_UINT: u64 = 2;
const KIND_BIG: u64 = 3;
const KIND_UBIG: u64 = 4;
const KIND_REAL: u64 = 5;
const KIND_DECIMAL: u64 = 6;
const KIND_UUID: u64 = 7;
const KIND_INSTANT: u64 = 8;
const KIND_DATE: u64 = 9;
const KIND_CIVIL: u64 = 10;
const KIND_TIME: u64 = 11;
const KIND_SPAN: u64 = 12;
const KIND_TEXT: u64 = 13;
/// A door the Rust binding gains later: nothing here knows its type, so no value.
const KIND_NONE: u64 = 14;

/// A cell's value, as the door made it.
enum Value {
    Bool(bool),
    Int(i32),
    Uint(u32),
    Big(i64),
    UBig(u64),
    Real(f64),
    Decimal(hc::Decimal),
    Uuid([u8; 16]),
    Instant(hc::Timestamp),
    Date(hc::Date),
    Civil(hc::CivilDateTime),
    Time(u64),
    Span(hc::Duration),
    Text(String),
}

fn door_name(door: ht::Door) -> Door {
    match door {
        ht::Door::Bool => Door::Bool,
        ht::Door::I8 => Door::I8,
        ht::Door::I16 => Door::I16,
        ht::Door::I32 => Door::I32,
        ht::Door::I64 => Door::I64,
        ht::Door::U8 => Door::U8,
        ht::Door::U16 => Door::U16,
        ht::Door::U32 => Door::U32,
        ht::Door::U64 => Door::U64,
        ht::Door::F32 => Door::F32,
        ht::Door::F64 => Door::F64,
        ht::Door::Decimal => Door::Decimal,
        ht::Door::Uuid => Door::Uuid,
        ht::Door::Timestamp => Door::Timestamp,
        ht::Door::Unix(_) => Door::Unix,
        ht::Door::ExcelSerial(_) => Door::ExcelSerial,
        ht::Door::Date => Door::Date,
        ht::Door::DateOrdered(_) => Door::DateOrdered,
        ht::Door::DateTime(_) => Door::Datetime,
        ht::Door::Time => Door::Time,
        ht::Door::Duration => Door::Duration,
        ht::Door::Text => Door::Text,
        // A door the Rust binding gains later is read as its text until this binding knows it.
        _ => Door::Text,
    }
}

/// What a door declares beside itself, as HyperCast numbers it.
fn door_param(door: ht::Door) -> Option<u32> {
    match door {
        ht::Door::Unix(precision) => Some(precision as u32),
        ht::Door::DateOrdered(order) | ht::Door::DateTime(order) => Some(order as u32),
        ht::Door::ExcelSerial(epoch) => Some(epoch as u32),
        _ => None,
    }
}

fn origin_name(origin: ht::Origin) -> Origin {
    match origin {
        ht::Origin::Token => Origin::Token,
        ht::Origin::Padded => Origin::Padded,
        ht::Origin::Uncached => Origin::Uncached,
        // `Cell`, and an origin the Rust binding gains later.
        _ => Origin::Cell,
    }
}

/// A detail by HyperCast's own name for it.
fn detail_name(detail: hc::Detail) -> Detail {
    match detail {
        hc::Detail::Blank => Detail::Blank,
        hc::Detail::UnexpectedCharacter => Detail::UnexpectedCharacter,
        hc::Detail::DigitExpected => Detail::DigitExpected,
        hc::Detail::DelimiterExpected => Detail::DelimiterExpected,
        hc::Detail::TrailingText => Detail::TrailingText,
        hc::Detail::NoDigits => Detail::NoDigits,
        hc::Detail::SignMisplaced => Detail::SignMisplaced,
        hc::Detail::ParenthesesUnbalanced => Detail::ParenthesesUnbalanced,
        hc::Detail::MarkRepeated => Detail::MarkRepeated,
        hc::Detail::GroupMisplaced => Detail::GroupMisplaced,
        hc::Detail::GroupSize => Detail::GroupSize,
        hc::Detail::GroupInFraction => Detail::GroupInFraction,
        hc::Detail::AmbiguousSeparator => Detail::AmbiguousSeparator,
        hc::Detail::FractionNotAllowed => Detail::FractionNotAllowed,
        hc::Detail::FieldWidth => Detail::FieldWidth,
        hc::Detail::WrongLength => Detail::WrongLength,
        hc::Detail::ComponentMissing => Detail::ComponentMissing,
        hc::Detail::NotRecognized => Detail::NotRecognized,
        hc::Detail::UnknownUnit => Detail::UnknownUnit,
        hc::Detail::InvalidUtf8 => Detail::InvalidUtf8,
        hc::Detail::TooLong => Detail::TooLong,
        hc::Detail::NotFinite => Detail::NotFinite,
        hc::Detail::NotWhole => Detail::NotWhole,
        hc::Detail::NegativeNotAllowed => Detail::NegativeNotAllowed,
        hc::Detail::TooLarge => Detail::TooLarge,
        hc::Detail::TooSmall => Detail::TooSmall,
        hc::Detail::TooPrecise => Detail::TooPrecise,
        hc::Detail::DateFieldOutOfRange => Detail::DateFieldOutOfRange,
        hc::Detail::TimeFieldOutOfRange => Detail::TimeFieldOutOfRange,
        hc::Detail::OutsideWindow => Detail::OutsideWindow,
        hc::Detail::NonexistentDate => Detail::NonexistentDate,
        hc::Detail::NotAScalar => Detail::NotAScalar,
        // `hc::Detail` is non-exhaustive: a detail a later core adds is named when this binding
        // learns it, and `unknown` until then, as `@skunkwerkx/hypercast` says it.
        _ => Detail::Unknown,
    }
}

/// How a door's values are kept.
fn kind(door: ht::Door) -> u64 {
    match door {
        ht::Door::Bool => KIND_BOOL,
        ht::Door::I8 | ht::Door::I16 | ht::Door::I32 => KIND_INT,
        ht::Door::U8 | ht::Door::U16 | ht::Door::U32 => KIND_UINT,
        ht::Door::I64 => KIND_BIG,
        ht::Door::U64 => KIND_UBIG,
        ht::Door::F32 | ht::Door::F64 => KIND_REAL,
        ht::Door::Decimal => KIND_DECIMAL,
        ht::Door::Uuid => KIND_UUID,
        ht::Door::Timestamp | ht::Door::Unix(_) | ht::Door::ExcelSerial(_) => KIND_INSTANT,
        ht::Door::Date | ht::Door::DateOrdered(_) => KIND_DATE,
        ht::Door::DateTime(_) => KIND_CIVIL,
        ht::Door::Time => KIND_TIME,
        ht::Door::Duration => KIND_SPAN,
        ht::Door::Text => KIND_TEXT,
        _ => KIND_NONE,
    }
}

fn pack_date(date: hc::Date) -> u64 {
    u64::from(date.year) | u64::from(date.month) << 16 | u64::from(date.day) << 24
}

fn unpack_date(word: u64) -> hc::Date {
    hc::Date {
        year: word as u16,
        month: (word >> 16) as u8,
        day: (word >> 24) as u8,
    }
}

/// One cell's value as two words, read through the accessor its door's type is read with
/// (the only one the Rust binding answers for that door), its checked one for a column that
/// declares constraints, where a violation's value is the value as cast; a text or a decimal
/// written to `heap`, unless it is the cell's raw text, already there at `raw`.
fn cell(
    batch: &ht::Batch<'_>,
    kind: u64,
    column: usize,
    row: usize,
    raw: (usize, usize),
    heap: &mut Vec<u8>,
) -> std::result::Result<[u64; 2], hc::Fault> {
    let checked = batch.columns()[column].is_checked();
    macro_rules! get {
        ($ty:ty) => {
            if checked {
                match batch.get_checked::<$ty>(column, row) {
                    Ok(passed) => passed.value,
                    Err(ht::Rejection::Violation(broken)) => broken.value,
                    Err(ht::Rejection::Fault(fault)) => return Err(fault),
                }
            } else {
                batch.get::<$ty>(column, row)?
            }
        };
    }
    let mut text = |text: &[u8]| -> [u64; 2] {
        if heap.get(raw.0..raw.0 + raw.1) == Some(text) {
            return [raw.0 as u64, raw.1 as u64];
        }
        let at = heap.len();
        heap.extend_from_slice(text);
        [at as u64, text.len() as u64]
    };
    let one = |word: u64| [word, 0];
    Ok(match kind {
        KIND_BOOL => one(u64::from(get!(bool))),
        KIND_INT => one(match batch.columns()[column].door {
            ht::Door::I8 => i64::from(get!(i8)),
            ht::Door::I16 => i64::from(get!(i16)),
            _ => i64::from(get!(i32)),
        } as u64),
        KIND_UINT => one(match batch.columns()[column].door {
            ht::Door::U8 => u64::from(get!(u8)),
            ht::Door::U16 => u64::from(get!(u16)),
            _ => u64::from(get!(u32)),
        }),
        KIND_BIG => one(get!(i64) as u64),
        KIND_UBIG | KIND_TIME => one(get!(u64)),
        KIND_REAL => one(match batch.columns()[column].door {
            ht::Door::F32 => f64::from(get!(f32)),
            _ => get!(f64),
        }
        .to_bits()),
        KIND_DECIMAL => {
            let value = get!(hc::Decimal);
            [
                value.lo,
                u64::from(value.hi)
                    | u64::from(value.scale) << 32
                    | u64::from(value.negative) << 40,
            ]
        }
        KIND_UUID => {
            let value = get!([u8; 16]);
            let (high, low) = value.split_at(8);
            [
                u64::from_ne_bytes(high.try_into().unwrap_or_default()),
                u64::from_ne_bytes(low.try_into().unwrap_or_default()),
            ]
        }
        KIND_INSTANT => {
            let value = get!(hc::Timestamp);
            [value.seconds as u64, i64::from(value.nanos) as u64]
        }
        KIND_DATE => one(pack_date(get!(hc::Date))),
        KIND_CIVIL => {
            let value = get!(hc::CivilDateTime);
            [pack_date(value.date), value.nanos_of_day]
        }
        KIND_SPAN => {
            let value = get!(hc::Duration);
            [value.seconds as u64, i64::from(value.nanos) as u64]
        }
        KIND_TEXT if checked => match batch.text_checked(column, row) {
            Ok(passed) => text(passed.value.as_bytes()),
            Err(ht::Rejection::Violation(broken)) => text(broken.value.as_bytes()),
            Err(ht::Rejection::Fault(fault)) => return Err(fault),
        },
        KIND_TEXT => text(batch.text_str(column, row)?.as_bytes()),
        _ => return Err(CONTRACT),
    })
}

/// One column of a batch as the read copied it, before it is laid out in the store.
struct ColumnCopy {
    kind: u64,
    source: u64,
    faults: u64,
    verdicts: Vec<ht::CellVerdict>,
    notes: Vec<ht::CellNote>,
    values: Vec<[u64; 2]>,
    spans: Vec<[u64; 2]>,
    heap: Vec<u8>,
    /// The checks of a column that declares constraints.
    checks: Option<Vec<ht::RawCheck>>,
}

/// A batch as the read copied it.
struct BatchCopy {
    rows: usize,
    doors: Vec<Door>,
    lines: Vec<u32>,
    widths: Option<(u32, Vec<u32>)>,
    columns: Vec<ColumnCopy>,
}

/// `at` rounded up to a multiple of 8.
fn eight(at: usize) -> usize {
    at.div_ceil(8) * 8
}

impl BatchCopy {
    fn of(batch: &ht::Batch<'_>) -> BatchCopy {
        let rows = batch.rows();
        let plan = batch.columns();
        let columns = plan
            .iter()
            .enumerate()
            .map(|(column, spec)| {
                let kind = kind(spec.door);
                let mut copied = ColumnCopy {
                    kind,
                    source: batch.source_column(column) as u64,
                    faults: 0,
                    verdicts: batch.verdicts(column).to_vec(),
                    notes: Vec::new(),
                    values: Vec::with_capacity(rows),
                    spans: Vec::with_capacity(rows),
                    heap: Vec::new(),
                    checks: spec.is_checked().then(|| batch.checks(column).to_vec()),
                };
                for row in 0..rows {
                    let raw = batch.raw(column, row);
                    let span = (copied.heap.len(), raw.len());
                    copied.heap.extend_from_slice(&raw);
                    copied.spans.push([span.0 as u64, span.1 as u64]);
                    // A violation cast: its value is kept too.
                    let verdict = copied.verdicts[row];
                    let ok = verdict.is_ok() || verdict.reason == ht::CellVerdict::VIOLATION;
                    let value = if ok {
                        cell(batch, kind, column, row, span, &mut copied.heap)
                    } else {
                        Ok([0; 2])
                    };
                    match value {
                        Ok(value) => copied.values.push(value),
                        Err(fault) => {
                            // A cell the core passed that its accessor did not: the accessor's
                            // verdict is the one handed out.
                            copied.verdicts[row] = ht::CellVerdict {
                                offset: fault.offset,
                                len: fault.len,
                                reason: fault.reason as u32,
                            };
                            copied.values.push([0; 2]);
                        }
                    }
                    copied.faults += u64::from(copied.verdicts[row].fault().is_some());
                }
                if copied.faults > 0 {
                    copied.notes = batch.notes(column).to_vec();
                }
                copied
            })
            .collect();
        BatchCopy {
            rows,
            doors: plan.iter().map(|column| door_name(column.door)).collect(),
            lines: (0..rows).map(|row| batch.line(row)).collect(),
            widths: batch
                .reported_widths()
                .map(|(width, widths)| (width, widths[..rows.min(widths.len())].to_vec())),
            columns,
        }
    }

    /// The store's length in bytes, and where each section starts: the lines, the widths and
    /// each column's verdicts, notes, values, spans and heap.
    fn layout(&self) -> (usize, Vec<usize>) {
        let mut at = (HEAD + COLUMN_WORDS * self.columns.len()) * 8;
        let mut starts = Vec::with_capacity(2 + 6 * self.columns.len());
        let mut section = |at: &mut usize, len: usize| {
            *at = eight(*at);
            starts.push(*at);
            *at += len;
        };
        section(&mut at, self.lines.len() * 4);
        section(
            &mut at,
            self.widths
                .as_ref()
                .map_or(0, |(_, widths)| widths.len() * 4),
        );
        for column in &self.columns {
            let rows = column.values.len();
            let verdicts = if column.passed() { 0 } else { rows };
            section(&mut at, verdicts * ht::CellVerdict::SIZE as usize);
            section(&mut at, column.notes.len() * size_of::<ht::CellNote>());
            section(
                &mut at,
                rows * Shape::of(column.kind, column.wide()).width(),
            );
            section(
                &mut at,
                rows * Shape::Pair {
                    wide: column.wide(),
                }
                .width(),
            );
            section(&mut at, column.heap.len());
            section(
                &mut at,
                column
                    .checks
                    .as_ref()
                    .map_or(0, |checks| checks.len() * size_of::<ht::RawCheck>()),
            );
        }
        (eight(at), starts)
    }

    /// The store, written into `out`, which is `layout`'s length and zeroed.
    fn write(&self, out: &mut [u8], starts: &[usize]) {
        let mut put = |at: usize, bytes: &[u8]| {
            if let Some(to) = out.get_mut(at..at + bytes.len()) {
                to.copy_from_slice(bytes);
            }
        };
        let word = |index: usize| index * 8;
        put(word(0), &(self.rows as u64).to_ne_bytes());
        put(word(1), &(self.columns.len() as u64).to_ne_bytes());
        put(word(2), &(starts[0] as u64).to_ne_bytes());
        if let Some((width, _)) = &self.widths {
            put(word(3), &(starts[1] as u64).to_ne_bytes());
            put(word(4), &u64::from(*width).to_ne_bytes());
        }
        put(starts[0], as_bytes(&self.lines));
        if let Some((_, widths)) = &self.widths {
            put(starts[1], as_bytes(widths));
        }
        for (index, column) in self.columns.iter().enumerate() {
            let head = HEAD + COLUMN_WORDS * index;
            let at = &starts[2 + 6 * index..2 + 6 * index + 6];
            let wide = column.wide();
            let words = [
                column.kind,
                column.source,
                column.faults,
                if column.passed() { 0 } else { at[0] as u64 },
                if column.notes.is_empty() {
                    0
                } else {
                    at[1] as u64
                },
                at[2] as u64,
                at[3] as u64,
                at[4] as u64,
                column.heap.len() as u64,
                u64::from(wide),
                if column.checks.is_some() {
                    at[5] as u64
                } else {
                    0
                },
            ];
            for (offset, value) in words.iter().enumerate() {
                put(word(head + offset), &value.to_ne_bytes());
            }
            if !column.passed() {
                put(at[0], as_bytes(&column.verdicts));
            }
            put(at[1], as_bytes(&column.notes));
            for (pairs, start, shape) in [
                (&column.values, at[2], Shape::of(column.kind, wide)),
                (&column.spans, at[3], Shape::Pair { wide }),
            ] {
                for (row, value) in pairs.iter().enumerate() {
                    let (bytes, width) = shape.bytes(*value);
                    put(start + row * width, &bytes[..width]);
                }
            }
            put(at[4], &column.heap);
            if let Some(checks) = &column.checks {
                put(at[5], as_bytes(checks));
            }
        }
    }
}

impl ColumnCopy {
    /// Whether every cell passed: cast, and met the column's constraints.
    fn passed(&self) -> bool {
        self.verdicts.iter().all(ht::CellVerdict::is_ok)
    }

    /// Whether the column's heap is past what a `u32` offset reaches, so that its spans and
    /// texts are kept in two `u64`s rather than two `u32`s.
    fn wide(&self) -> bool {
        u32::try_from(self.heap.len()).is_err()
    }
}

/// How a value or a span is kept.
#[derive(Clone, Copy)]
enum Shape {
    /// One `u64`.
    Word,
    /// Two `u64`s: a decimal, a UUID, an instant, a date-time, a duration.
    TwoWords,
    /// A place in the heap (a span, a text): two `u32`s, or two `u64`s in a wide column.
    Pair { wide: bool },
}

impl Shape {
    fn of(kind: u64, wide: bool) -> Shape {
        match kind {
            KIND_TEXT => Shape::Pair { wide },
            KIND_DECIMAL | KIND_UUID | KIND_INSTANT | KIND_CIVIL | KIND_SPAN => Shape::TwoWords,
            _ => Shape::Word,
        }
    }

    fn width(self) -> usize {
        match self {
            Shape::Word | Shape::Pair { wide: false } => 8,
            Shape::TwoWords | Shape::Pair { wide: true } => 16,
        }
    }

    /// `value` as its bytes, `width` of them.
    fn bytes(self, value: [u64; 2]) -> ([u8; 16], usize) {
        let mut out = [0; 16];
        match self {
            Shape::Word => out[..8].copy_from_slice(&value[0].to_ne_bytes()),
            Shape::Pair { wide: false } => {
                out[..4].copy_from_slice(&(value[0] as u32).to_ne_bytes());
                out[4..8].copy_from_slice(&(value[1] as u32).to_ne_bytes());
            }
            Shape::TwoWords | Shape::Pair { wide: true } => {
                out[..8].copy_from_slice(&value[0].to_ne_bytes());
                out[8..].copy_from_slice(&value[1].to_ne_bytes());
            }
        }
        (out, self.width())
    }

    /// The value at `row` of `bytes`.
    fn read(self, bytes: &[u8], row: usize) -> Option<[u64; 2]> {
        let width = self.width();
        let at = row.checked_mul(width)?;
        let bytes = bytes.get(at..at.checked_add(width)?)?;
        let word = |from: usize| {
            Some(u64::from_ne_bytes(
                bytes.get(from..from + 8)?.try_into().ok()?,
            ))
        };
        let half = |from: usize| {
            Some(u64::from(u32::from_ne_bytes(
                bytes.get(from..from + 4)?.try_into().ok()?,
            )))
        };
        Some(match self {
            Shape::Word => [word(0)?, 0],
            Shape::Pair { wide: false } => [half(0)?, half(4)?],
            Shape::TwoWords | Shape::Pair { wide: true } => [word(0)?, word(8)?],
        })
    }
}

/// A batch's store, read back: every look is bounds-checked, so a store that is not whole
/// reads as no rows and no columns.
#[derive(Clone, Copy)]
struct Store<'a> {
    bytes: &'a [u8],
}

/// One column of a store.
struct StoredColumn<'a> {
    kind: u64,
    source: usize,
    faults: u64,
    /// The verdicts, none kept for a column whose every cell passed.
    verdicts: Option<&'a [ht::CellVerdict]>,
    notes: &'a [ht::CellNote],
    values: &'a [u8],
    spans: &'a [u8],
    heap: &'a [u8],
    wide: bool,
    rows: usize,
    /// The checks of a column that declares constraints.
    checks: Option<&'a [ht::RawCheck]>,
}

impl<'a> Store<'a> {
    fn word(&self, index: usize) -> Option<u64> {
        let at = index.checked_mul(8)?;
        let bytes = self.bytes.get(at..at.checked_add(8)?)?;
        Some(u64::from_ne_bytes(bytes.try_into().ok()?))
    }

    fn section(&self, at: u64, len: usize) -> Option<&'a [u8]> {
        let at = usize::try_from(at).ok()?;
        self.bytes.get(at..at.checked_add(len)?)
    }

    fn rows(&self) -> usize {
        self.word(0)
            .and_then(|rows| usize::try_from(rows).ok())
            .unwrap_or(0)
    }

    fn columns(&self) -> usize {
        self.word(1)
            .and_then(|columns| usize::try_from(columns).ok())
            .unwrap_or(0)
    }

    fn lines(&self) -> &'a [u32] {
        let rows = self.rows();
        self.word(2)
            .and_then(|at| self.section(at, rows.checked_mul(4)?))
            .and_then(as_slice)
            .unwrap_or_default()
    }

    fn line(&self, row: usize) -> u32 {
        self.lines().get(row).copied().unwrap_or(0)
    }

    fn column(&self, column: usize) -> Option<StoredColumn<'a>> {
        if column >= self.columns() {
            return None;
        }
        let rows = self.rows();
        let head = HEAD + COLUMN_WORDS * column;
        let word = |offset: usize| self.word(head + offset);
        let kind = word(0)?;
        let wide = word(9)? != 0;
        let verdicts = match word(3)? {
            0 => None,
            at => Some(as_slice(
                self.section(at, rows.checked_mul(ht::CellVerdict::SIZE as usize)?)?,
            )?),
        };
        let notes = match word(4)? {
            0 => &[][..],
            at => self.section(at, rows.checked_mul(size_of::<ht::CellNote>())?)?,
        };
        let checks = match word(10)? {
            0 => None,
            at => Some(as_slice(
                self.section(at, rows.checked_mul(size_of::<ht::RawCheck>())?)?,
            )?),
        };
        Some(StoredColumn {
            kind,
            source: usize::try_from(word(1)?).ok()?,
            faults: word(2)?,
            verdicts,
            notes: as_slice(notes)?,
            values: self.section(word(5)?, rows.checked_mul(Shape::of(kind, wide).width())?)?,
            spans: self.section(word(6)?, rows.checked_mul(Shape::Pair { wide }.width())?)?,
            heap: self.section(word(7)?, usize::try_from(word(8)?).ok()?)?,
            wide,
            rows,
            checks,
        })
    }

    /// The batch's faults list, worked out now.
    fn faults(&self) -> Vec<Problem> {
        let rows = self.rows();
        let Some(stored) = (0..self.columns())
            .map(|column| self.column(column))
            .collect::<Option<Vec<_>>>()
        else {
            return Vec::new();
        };
        // A column whose every cell passed keeps no verdicts: its own are all `OK`.
        let cast = if stored.iter().any(|column| column.verdicts.is_none()) {
            vec![ht::CellVerdict::OK; rows]
        } else {
            Vec::new()
        };
        let columns: Vec<ht::KeptColumn<'_>> = stored
            .iter()
            .map(|column| {
                ht::KeptColumn::new(
                    column.verdicts.unwrap_or(&cast),
                    column.notes,
                    column.source,
                )
                .with_checks(column.checks.unwrap_or_default())
            })
            .collect();
        let widths = match self.word(3) {
            Some(0) | None => None,
            Some(at) => rows
                .checked_mul(4)
                .and_then(|len| self.section(at, len))
                .and_then(as_slice)
                .map(|widths| (self.word(4).unwrap_or(0) as u32, widths)),
        };
        ht::KeptBatch::new(self.lines(), &columns, widths)
            .map(|kept| kept.faults().map(|found| problem(&found)).collect())
            .unwrap_or_default()
    }
}

impl StoredColumn<'_> {
    /// The column as the Rust binding reads a kept one, for one look at a cell.
    fn kept(&self) -> ht::KeptColumn<'_> {
        ht::KeptColumn::new(self.verdicts.unwrap_or_default(), self.notes, self.source)
            .with_checks(self.checks.unwrap_or_default())
    }

    /// The bytes of the heap at a place a pair names.
    fn heap_at(&self, place: [u64; 2]) -> Option<&[u8]> {
        let at = usize::try_from(place[0]).ok()?;
        self.heap
            .get(at..at.checked_add(usize::try_from(place[1]).ok()?)?)
    }

    fn raw(&self, row: usize) -> &[u8] {
        Shape::Pair { wide: self.wide }
            .read(self.spans, row)
            .and_then(|place| self.heap_at(place))
            .unwrap_or_default()
    }

    fn detail(&self, row: usize) -> Option<hc::Detail> {
        self.kept().detail(row)
    }

    fn origin(&self, row: usize) -> ht::Origin {
        self.kept().origin(row)
    }

    /// The cell at `row` as the plain getters read it: HyperCast's contract violation for a
    /// column that declares constraints, whose verdicts only `getChecked` can say.
    fn value(&self, row: usize) -> std::result::Result<Value, hc::Fault> {
        if row >= self.rows || self.checks.is_some() {
            return Err(CONTRACT);
        }
        if self.verdicts.is_some() {
            match self.kept().verdict(row) {
                Some(Ok(())) => {}
                Some(Err(fault)) => return Err(fault),
                None => return Err(CONTRACT),
            }
        }
        self.stored(row)
    }

    /// The cell at `row` as HyperCast's checked door judged it. A column that declares no
    /// constraints reads as `value` does, never adjusted.
    fn checked(&self, row: usize) -> CheckedCell {
        if row >= self.rows {
            return CheckedCell::Fault(CONTRACT);
        }
        let verdict = self.verdicts.map_or(Some(ht::CellVerdict::OK), |verdicts| {
            verdicts.get(row).copied()
        });
        let check = self.checks.and_then(|checks| checks.get(row).copied());
        let Some(verdict) = verdict else {
            return CheckedCell::Fault(CONTRACT);
        };
        if verdict.reason == ht::CellVerdict::VIOLATION {
            return match (self.stored(row), check) {
                (Ok(value), Some(check)) => CheckedCell::Broke {
                    keyword: check.keyword,
                    value,
                    offset: verdict.offset,
                    length: verdict.len,
                },
                _ => CheckedCell::Fault(CONTRACT),
            };
        }
        if let Some(fault) = verdict.fault() {
            return CheckedCell::Fault(fault);
        }
        if !verdict.is_ok() {
            return CheckedCell::Fault(CONTRACT);
        }
        match self.stored(row) {
            Ok(value) => CheckedCell::Passed {
                value,
                adjusted: check.is_some_and(|check| check.adjusted != 0),
                enum_index: check
                    .map(|check| check.enum_index)
                    .filter(|index| *index != u32::MAX),
            },
            Err(fault) => CheckedCell::Fault(fault),
        }
    }

    /// The value kept at `row`, whatever its verdict.
    fn stored(&self, row: usize) -> std::result::Result<Value, hc::Fault> {
        let [first, second] = Shape::of(self.kind, self.wide)
            .read(self.values, row)
            .ok_or(CONTRACT)?;
        Ok(match self.kind {
            KIND_BOOL => Value::Bool(first != 0),
            KIND_INT => Value::Int(first as i32),
            KIND_UINT => Value::Uint(first as u32),
            KIND_BIG => Value::Big(first as i64),
            KIND_UBIG => Value::UBig(first),
            KIND_REAL => Value::Real(f64::from_bits(first)),
            KIND_DECIMAL => Value::Decimal(hc::Decimal {
                lo: first,
                hi: second as u32,
                scale: (second >> 32) as u8,
                negative: (second >> 40) & 1 == 1,
            }),
            KIND_UUID => {
                let mut bytes = [0; 16];
                bytes[..8].copy_from_slice(&first.to_ne_bytes());
                bytes[8..].copy_from_slice(&second.to_ne_bytes());
                Value::Uuid(bytes)
            }
            KIND_INSTANT => Value::Instant(hc::Timestamp {
                seconds: first as i64,
                nanos: second as i32,
            }),
            KIND_DATE => Value::Date(unpack_date(first)),
            KIND_CIVIL => Value::Civil(hc::CivilDateTime {
                date: unpack_date(first),
                nanos_of_day: second,
            }),
            KIND_TIME => Value::Time(first),
            KIND_SPAN => Value::Span(hc::Duration {
                seconds: first as i64,
                nanos: second as i32,
            }),
            KIND_TEXT => {
                let text = self.heap_at([first, second]).ok_or(CONTRACT)?;
                Value::Text(String::from_utf8_lossy(text).into_owned())
            }
            _ => return Err(CONTRACT),
        })
    }
}

/// A cell of a store as HyperCast's checked door judged it.
enum CheckedCell {
    Passed {
        value: Value,
        adjusted: bool,
        enum_index: Option<u32>,
    },
    Broke {
        keyword: u32,
        value: Value,
        offset: u32,
        length: u32,
    },
    Fault(hc::Fault),
}

/// A plain-integer ABI type a store keeps as bytes: `CellVerdict`, `CellNote`, `RawCheck` and
/// `u32` (a line, a width). Each is `repr(C)` integers with no padding, so every byte pattern
/// is a value.
trait Plain: Copy {}
impl Plain for ht::CellVerdict {}
impl Plain for ht::CellNote {}
impl Plain for ht::RawCheck {}
impl Plain for u32 {}

const _: () = assert!(size_of::<ht::CellVerdict>() == 12 && align_of::<ht::CellVerdict>() == 4);
const _: () = assert!(size_of::<ht::CellNote>() == 4 && align_of::<ht::CellNote>() == 2);
const _: () = assert!(size_of::<ht::RawCheck>() == 12 && align_of::<ht::RawCheck>() == 4);

/// `bytes` as the elements they hold, in this host's byte order; `None` when `bytes` isn't a
/// whole number of them or isn't aligned for them. No bytes are no elements, wherever they are.
fn as_slice<T: Plain>(bytes: &[u8]) -> Option<&[T]> {
    if bytes.is_empty() {
        return Some(&[]);
    }
    let size = size_of::<T>();
    if !bytes.len().is_multiple_of(size) || bytes.as_ptr().align_offset(align_of::<T>()) != 0 {
        return None;
    }
    // SAFETY: the pointer is aligned for `T` and the length a whole number of `T`s, all within
    // `bytes`, which the result borrows; any bytes are a valid `T` (see `Plain`).
    Some(unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<T>(), bytes.len() / size) })
}

/// `items` as bytes, in this host's byte order: what `as_slice` reads back.
fn as_bytes<T: Plain>(items: &[T]) -> &[u8] {
    // SAFETY: the bytes of `items`, which the result borrows; a `Plain` type has no padding, so
    // every byte is initialized.
    unsafe { std::slice::from_raw_parts(items.as_ptr().cast::<u8>(), size_of_val(items)) }
}

/// A store in the module's own memory: what the native `Batch` and `Row` hold, and, under
/// WebAssembly, what a JavaScript batch holds through a `BatchStore`.
#[derive(Clone)]
struct Held(Arc<[u64]>);

impl Held {
    fn of(copy: &BatchCopy) -> Held {
        let (len, starts) = copy.layout();
        let mut words = vec![0u64; len / 8];
        // SAFETY: the bytes of `words`, `len` of them (a multiple of 8), borrowed for the write.
        let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), len) };
        copy.write(bytes, &starts);
        Held(Arc::from(words))
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the bytes of the words, borrowed with them.
        unsafe { std::slice::from_raw_parts(self.0.as_ptr().cast::<u8>(), self.0.len() * 8) }
    }
}

/// A batch's store under WebAssembly, as `index.js`'s batch holds it: opaque, and of no use to
/// anything but this package's own `batch*` functions. (Under Node, Bun and Deno the store is an
/// `ArrayBuffer` instead.)
#[napi]
pub struct BatchStore {
    held: Held,
}

/// Where a call finds a batch's store: what a native object holds, or what `index.js`'s batch
/// passed in.
enum StoreAt<'a, 'env> {
    Held(&'a Held),
    Given(&'a Env, &'a Unknown<'env>),
}

/// The store `source` names, looked at by `look`, which runs no JavaScript. A value that is no
/// store, or a buffer that was detached, is a store of no rows and no columns.
fn with_store<R>(source: &StoreAt<'_, '_>, look: impl FnOnce(Store<'_>) -> R) -> R {
    let (env, given) = match source {
        StoreAt::Held(held) => {
            return look(Store {
                bytes: held.bytes(),
            });
        }
        StoreAt::Given(env, given) => (*env, *given),
    };
    let mut is_buffer = false;
    let mut data = std::ptr::null_mut();
    let mut len = 0;
    // SAFETY: `env` is the live environment of the call in progress and `given` a value in it;
    // the buffer's bytes are V8's, held by the caller's argument for the whole call, and `look`
    // runs no JavaScript that could detach them.
    unsafe {
        if sys::napi_is_arraybuffer(env.raw(), given.raw(), &mut is_buffer) == sys::Status::napi_ok
            && is_buffer
        {
            let bytes: &[u8] =
                if sys::napi_get_arraybuffer_info(env.raw(), given.raw(), &mut data, &mut len)
                    == sys::Status::napi_ok
                    && !data.is_null()
                {
                    std::slice::from_raw_parts(data.cast::<u8>(), len)
                } else {
                    &[]
                };
            return look(Store { bytes });
        }
        // `ClassInstance` unwraps and checks the class it was wrapped as; `&BatchStore` would
        // ask napi-rs to register a borrow, which only its generated argument conversion may.
        match <ClassInstance<'_, BatchStore> as napi::bindgen_prelude::FromNapiValue>::from_napi_value(
            env.raw(),
            given.raw(),
        ) {
            Ok(store) => look(Store {
                bytes: store.held.bytes(),
            }),
            Err(_) => {
                clear_pending_exception(env);
                look(Store { bytes: &[] })
            }
        }
    }
}

/// The store a JavaScript batch is made with: under Node, Bun and Deno an `ArrayBuffer` V8
/// allocates, which V8 reclaims with the batch; under WebAssembly a `BatchStore`.
fn js_store(env: &Env, copy: &BatchCopy) -> Result<sys::napi_value> {
    #[cfg(not(target_family = "wasm"))]
    {
        let (len, starts) = copy.layout();
        let mut data = std::ptr::null_mut();
        let mut buffer = std::ptr::null_mut();
        // SAFETY: `env` is the live environment of the call in progress; V8 allocates the buffer
        // zeroed and `data` is its `len` bytes, written here before any JavaScript can see it.
        unsafe {
            let status = sys::napi_create_arraybuffer(env.raw(), len, &mut data, &mut buffer);
            if status != sys::Status::napi_ok || (data.is_null() && len > 0) {
                return Err(napi::Error::from_reason(
                    "could not allocate a batch's store",
                ));
            }
            if len > 0 {
                copy.write(
                    std::slice::from_raw_parts_mut(data.cast::<u8>(), len),
                    &starts,
                );
            }
        }
        Ok(buffer)
    }
    #[cfg(target_family = "wasm")]
    {
        js(
            env,
            BatchStore {
                held: Held::of(copy),
            }
            .into_instance(env)?,
        )
    }
}

/// What `index.js` registered to make its batches with: a function taking the store, the row
/// count and the doors.
struct BatchMaker(sys::napi_ref);

/// Makes every batch this environment reads with `make(store, rows, doors)`, `index.js`'s own,
/// rather than as the native `Batch`, which a native finalizer frees only once the event loop
/// turns. Internal to the package: what `make` is given is of no use to anything else.
#[napi(
    catch_unwind,
    js_name = "__makeBatchesWith",
    ts_args_type = "make: (store: unknown, rows: number, doors: Array<Door>) => Batch"
)]
pub fn make_batches_with(env: &Env, make: Unknown<'_>) -> Result<()> {
    if make.get_type()? != ValueType::Function {
        return Err(napi::Error::from_reason("make must be a function"));
    }
    let mut reference = std::ptr::null_mut();
    // SAFETY: `env` is the live environment of the call in progress and `make` a function in
    // it; the strong reference lives as long as the environment's instance data.
    unsafe {
        if sys::napi_create_reference(env.raw(), make.raw(), 1, &mut reference)
            != sys::Status::napi_ok
        {
            return Err(napi::Error::from_reason("could not hold make"));
        }
    }
    env.set_instance_data(BatchMaker(reference), (), |context| {
        // SAFETY: the reference was made in this environment, which is tearing down; it is
        // deleted once, here.
        unsafe { sys::napi_delete_reference(context.env.raw(), context.value.0) };
    })
}

/// The function `index.js` registered, if it did.
fn batch_maker(env: &Env) -> Option<sys::napi_value> {
    let maker = env.get_instance_data::<BatchMaker>().ok()??;
    let mut make = std::ptr::null_mut();
    // SAFETY: the reference was made in this environment and is alive while its data is.
    let status = unsafe { sys::napi_get_reference_value(env.raw(), maker.0, &mut make) };
    (status == sys::Status::napi_ok && !make.is_null()).then_some(make)
}

/// A value as JavaScript holds it.
fn value_js(env: &Env, value: Value) -> Result<sys::napi_value> {
    match value {
        Value::Bool(value) => js(env, value),
        Value::Int(value) => js(env, value),
        Value::Uint(value) => js(env, value),
        Value::Big(value) => js(env, BigInt::from(value)),
        Value::UBig(value) => js(env, BigInt::from(value)),
        Value::Real(value) => js(env, value),
        Value::Decimal(value) => js(env, value.to_string()),
        Value::Uuid(value) => js(env, uuid_text(value)),
        Value::Instant(value) => js(env, timestamp(value)),
        Value::Date(value) => js(env, calendar(value)),
        Value::Civil(value) => js(env, civil(value)),
        Value::Time(value) => js(env, time_of_day(value)),
        Value::Span(value) => js(env, duration(value)),
        Value::Text(value) => js(env, value),
    }
}

/// A value as JavaScript holds it, in a success.
fn verdict(env: &Env, value: std::result::Result<Value, hc::Fault>) -> Result<Object<'_>> {
    match value {
        Err(fault) => failure(env, fault),
        Ok(value) => {
            let value = value_js(env, value)?;
            record(env, &[(c"ok", js(env, true)?), (c"value", value)])
        }
    }
}

/// A checked cell as JavaScript holds it: HyperCast's `Checked`, `Failure` or `Violated`.
fn checked_verdict(env: &Env, cell: CheckedCell) -> Result<Object<'_>> {
    match cell {
        CheckedCell::Fault(fault) => failure(env, fault),
        CheckedCell::Passed {
            value,
            adjusted,
            enum_index,
        } => {
            let value = value_js(env, value)?;
            let enum_index = match enum_index {
                Some(index) => js(env, index)?,
                None => js(env, Null)?,
            };
            record(
                env,
                &[
                    (c"ok", js(env, true)?),
                    (c"value", value),
                    (c"adjusted", js(env, adjusted)?),
                    (c"enumIndex", enum_index),
                ],
            )
        }
        CheckedCell::Broke {
            keyword,
            value,
            offset,
            length,
        } => {
            let value = value_js(env, value)?;
            let violation = record(
                env,
                &[
                    (c"keyword", js(env, keyword_name(keyword))?),
                    (c"value", value),
                    (c"offset", js(env, offset)?),
                    (c"length", js(env, length)?),
                ],
            )?;
            record(
                env,
                &[(c"ok", js(env, false)?), (c"violation", violation.raw())],
            )
        }
    }
}

/// A keyword's OpenAPI spelling, `"keyword N"` for a code this package does not name yet.
fn keyword_name(code: u32) -> String {
    [
        hc::Keyword::Minimum,
        hc::Keyword::ExclusiveMinimum,
        hc::Keyword::Maximum,
        hc::Keyword::ExclusiveMaximum,
        hc::Keyword::MultipleOf,
        hc::Keyword::MinLength,
        hc::Keyword::MaxLength,
        hc::Keyword::Enum,
    ]
    .into_iter()
    .find(|keyword| keyword.code() == code)
    .map_or_else(
        || format!("keyword {code}"),
        |keyword| keyword.name().to_owned(),
    )
}

/// Bytes as a `Uint8Array` the caller owns: under Node, Bun and Deno over a buffer V8
/// allocates, so it is reclaimed like any other.
fn bytes_out<'env>(env: &'env Env, bytes: &[u8]) -> Result<Unknown<'env>> {
    #[cfg(not(target_family = "wasm"))]
    {
        let buffer = napi::bindgen_prelude::ArrayBuffer::copy_from(env, bytes)?;
        let mut array = std::ptr::null_mut();
        // SAFETY: `env` is the live environment of the call in progress and `buffer` a buffer
        // made in it, `bytes.len()` long.
        unsafe {
            let status = sys::napi_create_typedarray(
                env.raw(),
                sys::TypedarrayType::uint8_array,
                bytes.len(),
                buffer.raw(),
                0,
                &mut array,
            );
            if status != sys::Status::napi_ok {
                return Err(napi::Error::from_reason("could not make a Uint8Array"));
            }
            napi::bindgen_prelude::FromNapiValue::from_napi_value(env.raw(), array)
        }
    }
    #[cfg(target_family = "wasm")]
    {
        let array = js(env, Uint8Array::from(bytes.to_vec()))?;
        // SAFETY: `array` was made in `env`, the live environment of the call in progress.
        unsafe { napi::bindgen_prelude::FromNapiValue::from_napi_value(env.raw(), array) }
    }
}

fn problem(problem: &ht::Problem) -> Problem {
    Problem {
        row: narrow(problem.row),
        line: problem.line,
        plan_column: problem.plan_column.map(narrow),
        source_column: problem.source_column.map(narrow),
        reason: problem.reason.map(cast_failure),
        detail: problem.detail.map(detail_name),
        origin: origin_name(problem.origin),
        offset: problem.offset,
        length: problem.len,
        expected: problem.expected,
        found: problem.found,
        keyword: problem.keyword.map(|keyword| keyword.name().to_owned()),
    }
}

fn uuid_text(bytes: [u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    out
}

fn timestamp(value: hc::Timestamp) -> Timestamp {
    Timestamp {
        seconds: value.seconds,
        nanos: value.nanos,
    }
}

fn calendar(date: hc::Date) -> CalendarDate {
    CalendarDate {
        year: u32::from(date.year),
        month: u32::from(date.month),
        day: u32::from(date.day),
    }
}

fn time_of_day(nanos_of_day: u64) -> TimeOfDay {
    let seconds = nanos_of_day / 1_000_000_000;
    TimeOfDay {
        hour: (seconds / 3600) as u32,
        minute: (seconds / 60 % 60) as u32,
        second: (seconds % 60) as u32,
        nanos: (nanos_of_day % 1_000_000_000) as u32,
    }
}

fn civil(value: hc::CivilDateTime) -> CivilDateTime {
    let time = time_of_day(value.nanos_of_day);
    CivilDateTime {
        year: u32::from(value.date.year),
        month: u32::from(value.date.month),
        day: u32::from(value.date.day),
        hour: time.hour,
        minute: time.minute,
        second: time.second,
        nanos: time.nanos,
    }
}

fn duration(value: hc::Duration) -> Duration {
    Duration {
        seconds: value.seconds,
        nanos: value.nanos,
    }
}

// The cells a call asks for: a column and a row as whole numbers, judged against the store.

/// The cell at `column` and `row` of `source`'s store: HyperCast's contract violation for one
/// it does not have.
fn value_at(
    source: &StoreAt<'_, '_>,
    column: Option<usize>,
    row: Option<usize>,
) -> std::result::Result<Value, hc::Fault> {
    let (Some(column), Some(row)) = (column, row) else {
        return Err(CONTRACT);
    };
    with_store(source, |store| {
        store
            .column(column)
            .map_or(Err(CONTRACT), |stored| stored.value(row))
    })
}

/// The cell at `column` and `row` of `source`'s store as HyperCast's checked door judged it:
/// HyperCast's contract violation for one it does not have.
fn checked_at(source: &StoreAt<'_, '_>, column: Option<usize>, row: Option<usize>) -> CheckedCell {
    let (Some(column), Some(row)) = (column, row) else {
        return CheckedCell::Fault(CONTRACT);
    };
    with_store(source, |store| {
        store
            .column(column)
            .map_or(CheckedCell::Fault(CONTRACT), |stored| stored.checked(row))
    })
}

fn detail_at(
    source: &StoreAt<'_, '_>,
    column: Option<usize>,
    row: Option<usize>,
) -> Option<Detail> {
    let (column, row) = (column?, row?);
    with_store(source, |store| store.column(column)?.detail(row)).map(detail_name)
}

fn origin_at(
    source: &StoreAt<'_, '_>,
    column: Option<usize>,
    row: Option<usize>,
) -> Option<Origin> {
    let (column, row) = (column?, row?);
    with_store(source, |store| {
        (row < store.rows())
            .then(|| store.column(column).map(|stored| stored.origin(row)))
            .flatten()
    })
    .map(origin_name)
}

fn raw_at(source: &StoreAt<'_, '_>, column: Option<usize>, row: Option<usize>) -> Vec<u8> {
    let (Some(column), Some(row)) = (column, row) else {
        return Vec::new();
    };
    with_store(source, |store| {
        store
            .column(column)
            .map(|stored| stored.raw(row).to_vec())
            .unwrap_or_default()
    })
}

fn line_at(source: &StoreAt<'_, '_>, row: Option<usize>) -> u32 {
    row.map_or(0, |row| with_store(source, |store| store.line(row)))
}

fn faults_of(source: &StoreAt<'_, '_>) -> Vec<Problem> {
    with_store(source, |store| store.faults())
}

fn fault_count_at(source: &StoreAt<'_, '_>, column: Option<usize>) -> u32 {
    column.map_or(0, |column| {
        with_store(source, |store| {
            store
                .column(column)
                .map_or(0, |stored| u32::try_from(stored.faults).unwrap_or(u32::MAX))
        })
    })
}

fn column_at<'env>(
    env: &'env Env,
    source: &StoreAt<'_, '_>,
    column: Option<usize>,
) -> Result<Array<'env>> {
    let rows = column.map_or(0, |column| {
        with_store(source, |store| {
            if column < store.columns() {
                store.rows()
            } else {
                0
            }
        })
    });
    let mut list = Array::from_vec(env, Vec::<bool>::new())?;
    for row in 0..rows {
        // The store is looked at afresh for each row: making a verdict can run JavaScript.
        list.insert(verdict(env, value_at(source, column, Some(row)))?)?;
    }
    Ok(list)
}

fn column_checked_at<'env>(
    env: &'env Env,
    source: &StoreAt<'_, '_>,
    column: Option<usize>,
) -> Result<Array<'env>> {
    let rows = column.map_or(0, |column| {
        with_store(source, |store| {
            if column < store.columns() {
                store.rows()
            } else {
                0
            }
        })
    });
    let mut list = Array::from_vec(env, Vec::<bool>::new())?;
    for row in 0..rows {
        // The store is looked at afresh for each row: making a verdict can run JavaScript.
        list.insert(checked_verdict(env, checked_at(source, column, Some(row)))?)?;
    }
    Ok(list)
}

// What `index.js`'s batch calls, its store in hand. Internal to the package: what they take is
// what `__makeBatchesWith`'s function is given.

/// `Batch.line`, over a store.
#[napi(catch_unwind, ts_args_type = "store: unknown, row: number")]
pub fn batch_line(env: &Env, store: Unknown<'_>, row: Unknown<'_>) -> u32 {
    line_at(&StoreAt::Given(env, &store), index(&row))
}

/// `Batch.get`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number",
    ts_return_type = "Verdict<Value>"
)]
pub fn batch_get<'env>(
    env: &'env Env,
    store: Unknown<'env>,
    column: Unknown<'env>,
    row: Unknown<'env>,
) -> Result<Object<'env>> {
    let value = value_at(&StoreAt::Given(env, &store), index(&column), index(&row));
    verdict(env, value)
}

/// `Batch.getChecked`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number",
    ts_return_type = "CheckedVerdict<Value>"
)]
pub fn batch_get_checked<'env>(
    env: &'env Env,
    store: Unknown<'env>,
    column: Unknown<'env>,
    row: Unknown<'env>,
) -> Result<Object<'env>> {
    let cell = checked_at(&StoreAt::Given(env, &store), index(&column), index(&row));
    checked_verdict(env, cell)
}

/// `Batch.detail`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number"
)]
pub fn batch_detail(
    env: &Env,
    store: Unknown<'_>,
    column: Unknown<'_>,
    row: Unknown<'_>,
) -> Option<Detail> {
    detail_at(&StoreAt::Given(env, &store), index(&column), index(&row))
}

/// `Batch.origin`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number"
)]
pub fn batch_origin(
    env: &Env,
    store: Unknown<'_>,
    column: Unknown<'_>,
    row: Unknown<'_>,
) -> Option<Origin> {
    origin_at(&StoreAt::Given(env, &store), index(&column), index(&row))
}

/// `Batch.faults`, over a store.
#[napi(catch_unwind, ts_args_type = "store: unknown")]
pub fn batch_faults(env: &Env, store: Unknown<'_>) -> Vec<Problem> {
    faults_of(&StoreAt::Given(env, &store))
}

/// `Batch.raw`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number",
    ts_return_type = "Uint8Array"
)]
pub fn batch_raw<'env>(
    env: &'env Env,
    store: Unknown<'env>,
    column: Unknown<'env>,
    row: Unknown<'env>,
) -> Result<Unknown<'env>> {
    let bytes = raw_at(&StoreAt::Given(env, &store), index(&column), index(&row));
    bytes_out(env, &bytes)
}

/// `Batch.rawString`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number, row: number"
)]
pub fn batch_raw_string(
    env: &Env,
    store: Unknown<'_>,
    column: Unknown<'_>,
    row: Unknown<'_>,
) -> String {
    let bytes = raw_at(&StoreAt::Given(env, &store), index(&column), index(&row));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `Batch.column`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number",
    ts_return_type = "Array<Verdict<Value>>"
)]
pub fn batch_column<'env>(
    env: &'env Env,
    store: Unknown<'env>,
    column: Unknown<'env>,
) -> Result<Array<'env>> {
    column_at(env, &StoreAt::Given(env, &store), index(&column))
}

/// `Batch.columnChecked`, over a store.
#[napi(
    catch_unwind,
    ts_args_type = "store: unknown, column: number",
    ts_return_type = "Array<CheckedVerdict<Value>>"
)]
pub fn batch_column_checked<'env>(
    env: &'env Env,
    store: Unknown<'env>,
    column: Unknown<'env>,
) -> Result<Array<'env>> {
    column_checked_at(env, &StoreAt::Given(env, &store), index(&column))
}

/// `Batch.faultCount`, over a store.
#[napi(catch_unwind, ts_args_type = "store: unknown, column: number")]
pub fn batch_fault_count(env: &Env, store: Unknown<'_>, column: Unknown<'_>) -> u32 {
    fault_count_at(&StoreAt::Given(env, &store), index(&column))
}

/// The rows of one read, column-major, copied out of the reader: a batch stays valid after
/// the reader has moved on, and after it is closed.
#[napi]
pub struct Batch {
    rows: usize,
    doors: Vec<Door>,
    held: Held,
}

#[napi]
impl Batch {
    /// How many rows the batch holds.
    #[napi(getter, catch_unwind)]
    pub fn rows(&self) -> u32 {
        narrow(self.rows)
    }

    /// How many columns: the plan's length.
    #[napi(getter, catch_unwind)]
    pub fn column_count(&self) -> u32 {
        narrow(self.doors.len())
    }

    /// The door each column was read through, in plan order.
    #[napi(getter, catch_unwind)]
    pub fn doors(&self) -> Vec<Door> {
        self.doors.clone()
    }

    /// The line (delimited text) or the sheet row (a workbook) the row came from; 0 for a
    /// row the batch does not have.
    #[napi(catch_unwind, ts_args_type = "row: number")]
    pub fn line(&self, row: Unknown<'_>) -> u32 {
        line_at(&StoreAt::Held(&self.held), index(&row))
    }

    /// The cell at `row` of `column` as HyperCast judged it. A column or row the batch does
    /// not have is HyperCast's contract violation: `malformed`, offset 0, length 0; so is every
    /// cell of a column that declares constraints, which only `getChecked` can say.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number, row: number",
        ts_return_type = "Verdict<T>"
    )]
    pub fn get<'env>(
        &self,
        env: &'env Env,
        column: Unknown<'env>,
        row: Unknown<'env>,
    ) -> Result<Object<'env>> {
        verdict(
            env,
            value_at(&StoreAt::Held(&self.held), index(&column), index(&row)),
        )
    }

    /// The cell at `row` of `column` as HyperCast's checked door judged it: the value, flagged
    /// `adjusted` when `multipleOf`'s rounding changed it (at the text door, a matched `enum`
    /// entry as declared, `enumIndex` naming it); a violation of a constraint the column
    /// declares, carrying the value as cast and the judged text's span; or the cell's fault. A
    /// column that declares none reads as `get` does, never adjusted. A column or row the batch
    /// does not have is HyperCast's contract violation.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number, row: number",
        ts_return_type = "CheckedVerdict<T>"
    )]
    pub fn get_checked<'env>(
        &self,
        env: &'env Env,
        column: Unknown<'env>,
        row: Unknown<'env>,
    ) -> Result<Object<'env>> {
        checked_verdict(
            env,
            checked_at(&StoreAt::Held(&self.held), index(&column), index(&row)),
        )
    }

    /// What failed in the cell, by HyperCast's `Detail` name: `null` for a cell that cast,
    /// one no door was asked about (a token, padding, a formula with no cached value), a read
    /// with notes off, and a cell the batch does not have.
    #[napi(catch_unwind, ts_args_type = "column: number, row: number")]
    pub fn detail(&self, column: Unknown<'_>, row: Unknown<'_>) -> Option<Detail> {
        detail_at(&StoreAt::Held(&self.held), index(&column), index(&row))
    }

    /// Where the cell's verdict came from (`cell` for every cell of a read with notes off), or
    /// `null` for a cell the batch does not have.
    #[napi(catch_unwind, ts_args_type = "column: number, row: number")]
    pub fn origin(&self, column: Unknown<'_>, row: Unknown<'_>) -> Option<Origin> {
        origin_at(&StoreAt::Held(&self.held), index(&column), index(&row))
    }

    /// What went wrong in the batch, by row and then column, a row's own entry first: worked
    /// out when asked for, a new array each call, the caller's own.
    #[napi(catch_unwind)]
    pub fn faults(&self) -> Vec<Problem> {
        faults_of(&StoreAt::Held(&self.held))
    }

    /// The text of the cell, whatever its door and verdict: what a fault's span is a span of.
    /// Empty for an empty cell, a typed workbook cell that cast, and a cell the batch does
    /// not have.
    #[napi(
        catch_unwind,
        ts_args_type = "column: number, row: number",
        ts_return_type = "Uint8Array"
    )]
    pub fn raw<'env>(
        &self,
        env: &'env Env,
        column: Unknown<'env>,
        row: Unknown<'env>,
    ) -> Result<Unknown<'env>> {
        bytes_out(
            env,
            &raw_at(&StoreAt::Held(&self.held), index(&column), index(&row)),
        )
    }

    /// `raw` as a string, each invalid UTF-8 sequence replaced by U+FFFD: for display, since
    /// a fault's span counts bytes of `raw`, not UTF-16 code units of this.
    #[napi(catch_unwind, ts_args_type = "column: number, row: number")]
    pub fn raw_string(&self, column: Unknown<'_>, row: Unknown<'_>) -> String {
        let bytes = raw_at(&StoreAt::Held(&self.held), index(&column), index(&row));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The whole column, a verdict a row, as `get` reads each cell; empty for a column the batch
    /// does not have. Every cell of a column that declares constraints is HyperCast's contract
    /// violation: `columnChecked` reads it.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number",
        ts_return_type = "Array<Verdict<T>>"
    )]
    pub fn column<'env>(&self, env: &'env Env, column: Unknown<'env>) -> Result<Array<'env>> {
        column_at(env, &StoreAt::Held(&self.held), index(&column))
    }

    /// The whole column, a verdict a row, as `getChecked` reads each cell: a constrained column
    /// and one that declares nothing alike. Empty for a column the batch does not have.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number",
        ts_return_type = "Array<CheckedVerdict<T>>"
    )]
    pub fn column_checked<'env>(
        &self,
        env: &'env Env,
        column: Unknown<'env>,
    ) -> Result<Array<'env>> {
        column_checked_at(env, &StoreAt::Held(&self.held), index(&column))
    }

    /// How many cells of the column did not cast; 0 for a column the batch does not have.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn fault_count(&self, column: Unknown<'_>) -> u32 {
        fault_count_at(&StoreAt::Held(&self.held), index(&column))
    }

    /// The row at `row`, read across; `null` for a row the batch does not have.
    #[napi(catch_unwind, ts_args_type = "row: number")]
    pub fn row(&self, row: Unknown<'_>) -> Option<Row> {
        index(&row).filter(|row| *row < self.rows).map(|index| Row {
            index,
            held: self.held.clone(),
        })
    }
}

/// One row of a batch, read across: the batch's store and an index, nothing copied.
#[napi]
pub struct Row {
    index: usize,
    held: Held,
}

#[napi]
impl Row {
    /// The row's place in its batch.
    #[napi(getter, catch_unwind)]
    pub fn index(&self) -> u32 {
        narrow(self.index)
    }

    /// The line or the sheet row it came from.
    #[napi(getter, catch_unwind)]
    pub fn line(&self) -> u32 {
        line_at(&StoreAt::Held(&self.held), Some(self.index))
    }

    /// The cell in `column` as HyperCast judged it; a column the row does not have is the
    /// contract violation.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number",
        ts_return_type = "Verdict<T>"
    )]
    pub fn get<'env>(&self, env: &'env Env, column: Unknown<'env>) -> Result<Object<'env>> {
        verdict(
            env,
            value_at(&StoreAt::Held(&self.held), index(&column), Some(self.index)),
        )
    }

    /// The cell in `column` as HyperCast's checked door judged it, as `Batch.getChecked`.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number",
        ts_return_type = "CheckedVerdict<T>"
    )]
    pub fn get_checked<'env>(&self, env: &'env Env, column: Unknown<'env>) -> Result<Object<'env>> {
        checked_verdict(
            env,
            checked_at(&StoreAt::Held(&self.held), index(&column), Some(self.index)),
        )
    }

    /// What failed in the cell in `column`, as `Batch.detail`.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn detail(&self, column: Unknown<'_>) -> Option<Detail> {
        detail_at(&StoreAt::Held(&self.held), index(&column), Some(self.index))
    }

    /// Where the verdict of the cell in `column` came from, as `Batch.origin`.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn origin(&self, column: Unknown<'_>) -> Option<Origin> {
        origin_at(&StoreAt::Held(&self.held), index(&column), Some(self.index))
    }

    /// The raw text of the cell in `column`, as `Batch.raw`.
    #[napi(
        catch_unwind,
        ts_args_type = "column: number",
        ts_return_type = "Uint8Array"
    )]
    pub fn raw<'env>(&self, env: &'env Env, column: Unknown<'env>) -> Result<Unknown<'env>> {
        bytes_out(
            env,
            &raw_at(&StoreAt::Held(&self.held), index(&column), Some(self.index)),
        )
    }

    /// `raw` as a string, as `Batch.rawString`.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn raw_string(&self, column: Unknown<'_>) -> String {
        let bytes = raw_at(&StoreAt::Held(&self.held), index(&column), Some(self.index));
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A read's outcome: the batch, or `null` past the end, or the error that stopped it. Under
/// `index.js` a batch is its own JavaScript object over the store; read through the module on
/// its own (the browser entry) it is the native `Batch`.
fn read_outcome<'env>(
    env: &'env Env,
    read: std::result::Result<Option<ht::Batch<'_>>, ht::Error>,
) -> Result<Object<'env>> {
    match read {
        Ok(Some(batch)) => {
            let copy = BatchCopy::of(&batch);
            let Some(make) = batch_maker(env) else {
                let held = Held::of(&copy);
                let BatchCopy { rows, doors, .. } = copy;
                return success(env, Batch { rows, doors, held }.into_instance(env)?);
            };
            let store = js_store(env, &copy)?;
            let BatchCopy { rows, doors, .. } = copy;
            let args = [store, js(env, narrow(rows))?, js(env, doors)?];
            let mut undefined = std::ptr::null_mut();
            let mut made = std::ptr::null_mut();
            // SAFETY: `env` is the live environment of the call in progress; `make` and every
            // argument are values in it.
            let status = unsafe {
                sys::napi_get_undefined(env.raw(), &mut undefined);
                sys::napi_call_function(
                    env.raw(),
                    undefined,
                    make,
                    args.len(),
                    args.as_ptr(),
                    &mut made,
                )
            };
            if status != sys::Status::napi_ok {
                // `index.js`'s own function throws nothing; an engine out of memory does.
                clear_pending_exception(env);
                return Err(napi::Error::from_reason("could not make a batch"));
            }
            // SAFETY: `made` was made in `env`.
            success(env, unsafe { Unknown::from_raw_unchecked(env.raw(), made) })
        }
        Ok(None) => success(env, Null),
        Err(error) => refused(env, error.into()),
    }
}

fn format_out(format: hc::NumFormat) -> SuggestedFormat {
    SuggestedFormat {
        packed: Uint8Array::from(hc::RawNumFormat::from(format).to_le_bytes().to_vec()),
    }
}

/// A suggestion as JavaScript holds it: its plan as `Column`s, ready to bind.
fn suggestion_outcome<'env>(
    env: &'env Env,
    found: std::result::Result<ht::Suggestion, ht::Error>,
) -> Result<Object<'env>> {
    let found = match found {
        Ok(found) => found,
        Err(error) => return refused(env, error.into()),
    };
    let mut plan = Array::from_vec(env, Vec::<bool>::new())?;
    for column in &found.plan {
        plan.insert(Column::of(*column).into_instance(env)?)?;
    }
    let report: Vec<SuggestedColumn> = found
        .report
        .iter()
        .map(|column| SuggestedColumn {
            ordinal: narrow(column.ordinal),
            name: column
                .name
                .as_deref()
                .map(|name| String::from_utf8_lossy(name).into_owned()),
            door: door_name(column.door),
            declared: door_param(column.door),
            format: column.format.map(format_out),
            candidates: column
                .candidates
                .iter()
                .filter_map(|candidate| match candidate {
                    ht::Candidate::DateOrder(order) => Some(Candidate {
                        kind: CandidateKind::DateOrder,
                        order: Some(*order as u32),
                        format: None,
                    }),
                    ht::Candidate::NumFormat(format) => Some(Candidate {
                        kind: CandidateKind::NumFormat,
                        order: None,
                        format: Some(format_out(*format)),
                    }),
                    // A candidate the Rust binding gains later, which nothing here can name.
                    _ => None,
                })
                .collect(),
            status: match column.status {
                ht::SuggestStatus::Resolved => SuggestStatus::Resolved,
                ht::SuggestStatus::Ambiguous => SuggestStatus::Ambiguous,
                // `Unresolved`, and a status the Rust binding gains later: the plan reads text.
                _ => SuggestStatus::Unresolved,
            },
            matched: column.matched,
            sampled: column.sampled,
        })
        .collect();
    let value = record(
        env,
        &[
            (c"plan", js(env, plan)?),
            (c"report", js(env, report)?),
            (c"truncated", js(env, found.truncated)?),
        ],
    )?;
    success(env, value)
}

// ---------------------------------------------------------------------------------------
// Delimited text.
// ---------------------------------------------------------------------------------------

/// A forward-only read of delimited text — CSV, TSV, any single-byte ASCII separator —
/// through a plan, a batch at a time. Made by `fromBytes` or `open`, never by `new`.
#[napi]
pub struct DelimitedReader {
    // `None` once closed.
    state: RefCell<Option<ht::DelimitedReader<'static>>>,
}

/// Where a reader's bytes come from.
enum Source {
    Bytes(Vec<u8>),
    Path(String),
}

fn delimited<'env>(
    env: &'env Env,
    source: Refused<Source>,
    dialect: &Unknown<'_>,
    plan: &Unknown<'_>,
    options: &Unknown<'_>,
) -> Result<Object<'env>> {
    let opened = (|| {
        let source = source?;
        let dialect = dialect_of(env, dialect)?;
        let plan = plan_of(env, plan)?;
        let options = delimited_options(env, options)?;
        let reader = match (source, plan) {
            (Source::Bytes(bytes), Some(plan)) => {
                options.from_reader(std::io::Cursor::new(bytes), dialect, &plan)
            }
            (Source::Bytes(bytes), None) => {
                options.from_reader_unbound(std::io::Cursor::new(bytes), dialect)
            }
            (Source::Path(path), Some(plan)) => options.open(path, dialect, &plan),
            (Source::Path(path), None) => options.open_unbound(path, dialect),
        }?;
        Ok(DelimitedReader {
            state: RefCell::new(Some(reader)),
        })
    })();
    match opened {
        Ok(reader) => success(env, reader.into_instance(env)?),
        Err(refusal) => refused(env, refusal),
    }
}

const READER: &str = "reader";

#[napi]
impl DelimitedReader {
    /// Reads delimited text held in memory: a `Uint8Array` (a `Buffer` is one) or a string,
    /// read as its UTF-8. With a plan it is bound now; without one (`undefined` or `null`)
    /// the header is read and the reader waits for `bind`. The dialect defaults to
    /// `Dialect.CSV`. Never throws: anything that stops it is the outcome's `error`.
    #[napi(
        catch_unwind,
        ts_args_type = "bytes: Uint8Array | string, dialect?: Dialect | null, plan?: ReadonlyArray<Column> | null, options?: DelimitedOptions | null",
        ts_return_type = "Outcome<DelimitedReader>"
    )]
    pub fn from_bytes<'env>(
        env: &'env Env,
        bytes: Unknown<'env>,
        dialect: Unknown<'env>,
        plan: Unknown<'env>,
        options: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let source = bytes_of(&bytes)
            .map(Source::Bytes)
            .ok_or_else(|| argument("the bytes must be a Uint8Array or a string"));
        delimited(env, source, &dialect, &plan, &options)
    }

    /// Opens the file at `path` and reads it a buffer at a time, as `fromBytes` reads memory.
    /// The core judges the options' sizes before the file is opened.
    #[napi(
        catch_unwind,
        ts_args_type = "path: string, dialect?: Dialect | null, plan?: ReadonlyArray<Column> | null, options?: DelimitedOptions | null",
        ts_return_type = "Outcome<DelimitedReader>"
    )]
    pub fn open<'env>(
        env: &'env Env,
        path: Unknown<'env>,
        dialect: Unknown<'env>,
        plan: Unknown<'env>,
        options: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let source = string_of(&path)
            .map(Source::Path)
            .ok_or_else(|| argument("the path must be a string"));
        delimited(env, source, &dialect, &plan, &options)
    }

    /// Samples the file at `path` and suggests a plan for it, never applying it: each column
    /// tried against the doors, with a report saying how far the sample settles it. A
    /// starting point to inspect; declaring the number format and date order settles what a
    /// sample cannot.
    #[napi(
        catch_unwind,
        ts_args_type = "path: string, dialect?: Dialect | null, options?: SuggestOptions | null",
        ts_return_type = "Outcome<Suggestion>"
    )]
    pub fn suggest<'env>(
        env: &'env Env,
        path: Unknown<'env>,
        dialect: Unknown<'env>,
        options: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let asked = (|| {
            let options = suggest_options(env, &options)?;
            let path = string_of(&path).ok_or_else(|| argument("the path must be a string"))?;
            Ok((options, path, dialect_of(env, &dialect)?))
        })();
        match asked {
            Ok((options, path, dialect)) => {
                suggestion_outcome(env, ht::DelimitedReader::suggest(path, dialect, &options))
            }
            Err(refusal) => refused(env, refusal),
        }
    }

    /// `suggest`, over delimited text held in memory: a `Uint8Array` or a string.
    #[napi(
        catch_unwind,
        ts_args_type = "bytes: Uint8Array | string, dialect?: Dialect | null, options?: SuggestOptions | null",
        ts_return_type = "Outcome<Suggestion>"
    )]
    pub fn suggest_bytes<'env>(
        env: &'env Env,
        bytes: Unknown<'env>,
        dialect: Unknown<'env>,
        options: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let asked = (|| {
            let options = suggest_options(env, &options)?;
            let bytes = bytes_of(&bytes)
                .ok_or_else(|| argument("the bytes must be a Uint8Array or a string"))?;
            Ok((options, bytes, dialect_of(env, &dialect)?))
        })();
        match asked {
            Ok((options, bytes, dialect)) => suggestion_outcome(
                env,
                ht::DelimitedReader::suggest_slice(&bytes, dialect, &options),
            ),
            Err(refusal) => refused(env, refusal),
        }
    }

    /// Declares the plan a reader opened without one reads through: once, before the first
    /// read.
    #[napi(
        catch_unwind,
        ts_args_type = "plan: ReadonlyArray<Column>",
        ts_return_type = "Outcome<null>"
    )]
    pub fn bind<'env>(&self, env: &'env Env, plan: Unknown<'env>) -> Result<Object<'env>> {
        let answer = exclusive(&self.state, |state| {
            // The plan is read with the reader held: a getter or a Proxy trap in it that calls
            // back into this reader is answered `busy`.
            let plan = plan_of(env, &plan)?
                .ok_or_else(|| argument("the plan must be an array of Columns"))?;
            let inner = state.as_mut().ok_or_else(|| closed(READER))?;
            inner.bind(&plan)?;
            Ok(Null)
        })
        .unwrap_or_else(|| Err(busy(READER)));
        outcome(env, answer)
    }

    /// Reads the next batch: the batch, `null` once the text is exhausted, or the error that
    /// stopped it — a structural failure only after every intact row before it was
    /// delivered, and again on every read after.
    #[napi(catch_unwind, ts_return_type = "Outcome<Batch | null>")]
    pub fn read<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        exclusive(&self.state, |state| match state.as_mut() {
            Some(inner) => read_outcome(env, inner.read()),
            None => refused(env, closed(READER)),
        })
        .unwrap_or_else(|| refused(env, busy(READER)))
    }

    /// Releases the reader's buffers and its file now, rather than when it is collected.
    /// Batches already read stay valid; a read or bind after it is a `closed` error. Closing
    /// twice is no error.
    #[napi(catch_unwind, ts_return_type = "Outcome<null>")]
    pub fn close<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let answer = exclusive(&self.state, |state| {
            *state = None;
            Ok(Null)
        })
        .unwrap_or_else(|| Err(busy(READER)));
        outcome(env, answer)
    }

    /// Whether a plan has been bound; `false` once closed. Throws the `busy` error from
    /// inside one of the reader's own calls.
    #[napi(getter, catch_unwind)]
    pub fn is_bound(&self, env: &Env) -> Result<bool> {
        shared(&self.state, |state| {
            state.as_ref().is_some_and(|inner| inner.is_bound())
        })
        .map_or_else(|| thrown_busy(env, READER), Ok)
    }

    /// The header record's names, or `null` if the dialect declares no header, and once
    /// closed. Throws the `busy` error from inside one of the reader's own calls.
    #[napi(getter, catch_unwind)]
    pub fn header(&self, env: &Env) -> Result<Option<Header>> {
        shared(&self.state, |state| {
            state.as_ref()?.header().map(|header| Header {
                inner: header.clone(),
            })
        })
        .map_or_else(|| thrown_busy(env, READER), Ok)
    }

    /// How many cells a record has: the header's count, or the first record's once it has
    /// been read; `null` before either, and once closed. Throws the `busy` error from inside
    /// one of the reader's own calls.
    #[napi(getter, catch_unwind)]
    pub fn column_count(&self, env: &Env) -> Result<Option<u32>> {
        shared(&self.state, |state| {
            state.as_ref()?.column_count().map(narrow)
        })
        .map_or_else(|| thrown_busy(env, READER), Ok)
    }

    /// How many records have been read, the header among them; `null` once closed. Throws
    /// the `busy` error from inside one of the reader's own calls.
    #[napi(getter, catch_unwind)]
    pub fn records(&self, env: &Env) -> Result<Option<f64>> {
        shared(&self.state, |state| {
            state.as_ref().map(|inner| inner.records() as f64)
        })
        .map_or_else(|| thrown_busy(env, READER), Ok)
    }
}

// ---------------------------------------------------------------------------------------
// Workbooks.
// ---------------------------------------------------------------------------------------

/// An open XLSX, ODS or Numbers workbook, read whole into memory it owns (a zip is read from
/// its directory at the end back). Made by `fromBytes` or `open`, never by `new`.
#[napi]
pub struct Workbook {
    inner: Arc<ht::Workbook<'static>>,
}

fn workbook_outcome<'env>(
    env: &'env Env,
    opened: std::result::Result<ht::Workbook<'static>, ht::Error>,
) -> Result<Object<'env>> {
    match opened {
        Ok(book) => success(
            env,
            Workbook {
                inner: Arc::new(book),
            }
            .into_instance(env)?,
        ),
        Err(error) => refused(env, error.into()),
    }
}

/// Which sheet: a whole index, or a name.
enum Which {
    Index(usize),
    Name(String),
}

#[napi]
impl Workbook {
    /// Opens a workbook held in memory, its format told by what is in it.
    #[napi(
        catch_unwind,
        ts_args_type = "bytes: Uint8Array",
        ts_return_type = "Outcome<Workbook>"
    )]
    pub fn from_bytes<'env>(env: &'env Env, bytes: Unknown<'env>) -> Result<Object<'env>> {
        let bytes = match bytes.get_type() {
            Ok(ValueType::Object) => unsafe { bytes.cast::<Uint8Array>() }
                .ok()
                .map(|bytes| bytes.to_vec()),
            _ => None,
        };
        match bytes {
            Some(bytes) => workbook_outcome(env, ht::Workbook::from_vec(bytes)),
            None => refused(env, argument("the bytes must be a Uint8Array")),
        }
    }

    /// Reads the file at `path` into memory and opens it.
    #[napi(
        catch_unwind,
        ts_args_type = "path: string",
        ts_return_type = "Outcome<Workbook>"
    )]
    pub fn open<'env>(env: &'env Env, path: Unknown<'env>) -> Result<Object<'env>> {
        match string_of(&path) {
            Some(path) => workbook_outcome(env, ht::Workbook::open(path)),
            None => refused(env, argument("the path must be a string")),
        }
    }

    /// The kind of workbook it is.
    #[napi(getter, catch_unwind)]
    pub fn format(&self) -> WorkbookFormat {
        match self.inner.format() {
            ht::Format::Ods => WorkbookFormat::Ods,
            ht::Format::Numbers => WorkbookFormat::Numbers,
            _ => WorkbookFormat::Xlsx,
        }
    }

    /// The date system its serials are in, as HyperCast's `ExcelEpoch` numbers it (`1` 1900,
    /// `2` 1904).
    #[napi(getter, catch_unwind)]
    pub fn date_system(&self) -> u32 {
        self.inner.date_system() as u32
    }

    /// Its sheets, in its own order.
    #[napi(getter, catch_unwind)]
    pub fn sheets(&self) -> Vec<SheetInfo> {
        self.inner
            .sheets()
            .iter()
            .map(|sheet| SheetInfo {
                name: sheet.name.clone(),
                hidden: sheet.hidden,
            })
            .collect()
    }

    /// Starts a read of one sheet, by index or by name. With a plan it is bound now; without
    /// one the header (if the options declare one) is read and the sheet waits for `bind`.
    #[napi(
        catch_unwind,
        ts_args_type = "which: number | string, options?: SheetOptions | null, plan?: ReadonlyArray<Column> | null",
        ts_return_type = "Outcome<Sheet>"
    )]
    pub fn sheet<'env>(
        &self,
        env: &'env Env,
        which: Unknown<'env>,
        options: Unknown<'env>,
        plan: Unknown<'env>,
    ) -> Result<Object<'env>> {
        let opened = (|| {
            let which = match (index(&which), string_of(&which)) {
                (Some(at), _) => Which::Index(at),
                (None, Some(name)) => Which::Name(name),
                _ => {
                    return Err(argument("the sheet must be a whole-number index or a name"));
                }
            };
            let options = sheet_options(env, &options)?;
            let plan = plan_of(env, &plan)?;
            let book = Arc::clone(&self.inner);
            // SAFETY: the sheet borrows the workbook behind the `Arc` it is stored beside,
            // which keeps that workbook at one address, unmoved and unchanged (a sheet only
            // reads it), for as long as the sheet lives: `Sheet` declares `state` before
            // `book`, so the sheet is dropped first.
            let borrowed: &'static ht::Workbook<'static> = unsafe { &*Arc::as_ptr(&book) };
            let sheet = match (&which, plan) {
                (Which::Index(at), Some(plan)) => borrowed.sheet(*at, options, &plan),
                (Which::Index(at), None) => borrowed.sheet_unbound(*at, options),
                (Which::Name(name), Some(plan)) => borrowed.sheet(name.as_str(), options, &plan),
                (Which::Name(name), None) => borrowed.sheet_unbound(name.as_str(), options),
            }?;
            Ok(Sheet {
                state: RefCell::new(Some(sheet)),
                book,
            })
        })();
        match opened {
            Ok(sheet) => success(env, sheet.into_instance(env)?),
            Err(refusal) => refused(env, refusal),
        }
    }
}

/// A forward-only read of one sheet through a plan, a batch at a time. Made by
/// `Workbook.sheet`, never by `new`.
#[napi]
pub struct Sheet {
    // Declared first, so dropped first: it borrows `book` (see `Workbook.sheet`).
    // `None` once closed.
    state: RefCell<Option<ht::Sheet<'static>>>,
    book: Arc<ht::Workbook<'static>>,
}

const SHEET: &str = "sheet";

#[napi]
impl Sheet {
    /// Declares the plan a sheet opened without one reads through: once, before the first
    /// read.
    #[napi(
        catch_unwind,
        ts_args_type = "plan: ReadonlyArray<Column>",
        ts_return_type = "Outcome<null>"
    )]
    pub fn bind<'env>(&self, env: &'env Env, plan: Unknown<'env>) -> Result<Object<'env>> {
        let answer = exclusive(&self.state, |state| {
            let plan = plan_of(env, &plan)?
                .ok_or_else(|| argument("the plan must be an array of Columns"))?;
            let sheet = state.as_mut().ok_or_else(|| closed(SHEET))?;
            sheet.bind(&plan)?;
            Ok(Null)
        })
        .unwrap_or_else(|| Err(busy(SHEET)));
        outcome(env, answer)
    }

    /// Reads the next batch, as `DelimitedReader.read` does.
    #[napi(catch_unwind, ts_return_type = "Outcome<Batch | null>")]
    pub fn read<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        exclusive(&self.state, |state| match state.as_mut() {
            Some(sheet) => read_outcome(env, sheet.read()),
            None => refused(env, closed(SHEET)),
        })
        .unwrap_or_else(|| refused(env, busy(SHEET)))
    }

    /// Samples the sheet from its first row, under the options it was opened with, and
    /// suggests a plan for it, never applying it: as `DelimitedReader.suggest`. A sheet's
    /// date column is text, and its typed cells count for every notation. Reads nothing of
    /// this sheet's own read.
    #[napi(
        catch_unwind,
        ts_args_type = "options?: SuggestOptions | null",
        ts_return_type = "Outcome<Suggestion>"
    )]
    pub fn suggest<'env>(&self, env: &'env Env, options: Unknown<'env>) -> Result<Object<'env>> {
        shared(&self.state, |state| {
            // The options are read with the sheet held: a getter that calls back into it to
            // close or read it is answered `busy`.
            let found = suggest_options(env, &options).and_then(|options| {
                let sheet = state.as_ref().ok_or_else(|| closed(SHEET))?;
                Ok(sheet.suggest(&options))
            });
            match found {
                Ok(found) => suggestion_outcome(env, found),
                Err(refusal) => refused(env, refusal),
            }
        })
        .unwrap_or_else(|| refused(env, busy(SHEET)))
    }

    /// Releases the sheet's buffers now, rather than when it is collected (the workbook's
    /// bytes stay until the workbook and its sheets are). Batches already read stay valid; a
    /// read or bind after it is a `closed` error. Closing twice is no error.
    #[napi(catch_unwind, ts_return_type = "Outcome<null>")]
    pub fn close<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let answer = exclusive(&self.state, |state| {
            *state = None;
            Ok(Null)
        })
        .unwrap_or_else(|| Err(busy(SHEET)));
        outcome(env, answer)
    }

    /// Whether a plan has been bound; `false` once closed. Throws the `busy` error from
    /// inside one of the sheet's own calls.
    #[napi(getter, catch_unwind)]
    pub fn is_bound(&self, env: &Env) -> Result<bool> {
        shared(&self.state, |state| {
            state.as_ref().is_some_and(|sheet| sheet.is_bound())
        })
        .map_or_else(|| thrown_busy(env, SHEET), Ok)
    }

    /// The header row's names, or `null` if the sheet was opened without a header, and once
    /// closed. Throws the `busy` error from inside one of the sheet's own calls.
    #[napi(getter, catch_unwind)]
    pub fn header(&self, env: &Env) -> Result<Option<Header>> {
        shared(&self.state, |state| {
            state.as_ref()?.header().map(|header| Header {
                inner: header.clone(),
            })
        })
        .map_or_else(|| thrown_busy(env, SHEET), Ok)
    }

    /// The workbook's date system, as `Workbook.dateSystem`.
    #[napi(getter, catch_unwind)]
    pub fn date_system(&self) -> u32 {
        self.book.date_system() as u32
    }
}

// ---------------------------------------------------------------------------------------
// The version.
// ---------------------------------------------------------------------------------------

/// The core's version, `major.minor.patch`, decoded from the same packed
/// `hypertabular_version` every other binding probes.
#[napi(catch_unwind)]
pub fn native_version() -> String {
    // SAFETY: the export has no contract to uphold; it is `unsafe` only because every export
    // is declared one way.
    let packed = unsafe { ht::kernel::exports::hypertabular_version() };
    format!(
        "{}.{}.{}",
        packed >> 16,
        (packed >> 8) & 0xff,
        packed & 0xff
    )
}
