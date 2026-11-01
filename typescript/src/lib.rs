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
//! a result) and `new` on a class, which has no constructor.
//!
//! A batch is copied out of the reader as it is read, as Python's, Ruby's and PHP's are: a
//! garbage-collected caller cannot be held to "valid until the next read". A fault's span is
//! in bytes of the cell's raw text, which `raw` hands back as a `Uint8Array`.

use hypercast as hc;
use hypertabular as ht;
use napi::bindgen_prelude::{
    Array, BigInt, ClassInstance, Either, Either9, JavaScriptClassExt, JsValue, Null, Object,
    ObjectFinalize, ToNapiValue, Uint8Array, Unknown,
};
use napi::{Env, Result, ValueType};
use napi_derive::napi;
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
    /// A reader opened without a plan was read before one was bound.
    #[napi(value = "unbound")]
    Unbound,
    /// A plan was bound to a reader that already has one.
    #[napi(value = "alreadyBound")]
    AlreadyBound,
    /// The data is structurally broken; `failure` says what and where.
    #[napi(value = "structure")]
    Structure,
    /// An argument of a type the declarations rule out.
    #[napi(value = "argument")]
    Argument,
    /// The reader, sheet or workbook was closed.
    #[napi(value = "closed")]
    Closed,
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
    /// Delimited text: the record's byte offset. A workbook: how far into the part's
    /// inflated bytes the read had got.
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

/// How delimited text is written: the separator, and whether `"` quotes, the first record
/// is the header and blank lines are skipped. Each flag left out is `true`, as in
/// `Dialect.CSV`.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct Dialect {
    /// One character: a tab, or printable ASCII other than `"`.
    pub separator: String,
    pub quoting: Option<bool>,
    pub has_header: Option<bool>,
    pub skip_blank_lines: Option<bool>,
}

/// How a delimited reader sizes what it owns.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct DelimitedOptions {
    /// The most rows a batch holds; 4096 when left out.
    pub batch_rows: Option<u32>,
    /// The input buffer a file is read through to begin with (it grows to hold the longest
    /// record); 256 KiB when left out.
    pub buffer_bytes: Option<u32>,
}

/// How a sheet is read. Left out: a header, empty rows skipped, 4096 rows a batch.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct SheetOptions {
    pub has_header: Option<bool>,
    pub skip_empty_rows: Option<bool>,
    pub batch_rows: Option<u32>,
}

/// What a numeric column's notation is read from: a `NumFormat` of `@skunkwerkx/hypercast`,
/// whose `packed` is the 92-byte layout the core reads.
#[napi(object, object_to_js = false, object_from_js = false)]
pub struct PackedNumFormat {
    pub packed: Uint8Array,
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
        if napi::sys::napi_is_exception_pending(env.raw(), &mut pending)
            == napi::sys::Status::napi_ok
            && pending
        {
            let mut exception = std::ptr::null_mut();
            napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut exception);
        }
    }
}

fn success<'env, V: ToNapiValue>(env: &'env Env, value: V) -> Result<Object<'env>> {
    let mut outcome = Object::new(env)?;
    outcome.set("ok", true)?;
    outcome.set("value", value)?;
    Ok(outcome)
}

fn failure(env: &Env, fault: hc::Fault) -> Result<Object<'_>> {
    let reason = match fault.reason {
        hc::Reason::Empty => CastFailure::Empty,
        hc::Reason::Malformed => CastFailure::Malformed,
        hc::Reason::OutOfRange => CastFailure::OutOfRange,
    };
    let mut verdict = Object::new(env)?;
    verdict.set("ok", false)?;
    verdict.set(
        "fault",
        Fault {
            reason,
            offset: fault.offset,
            length: fault.len,
        },
    )?;
    Ok(verdict)
}

/// A cell asked for at a column or a row the batch does not have, or with an index that is
/// not one: HyperCast's contract violation, `malformed` with an empty span.
const CONTRACT: hc::Fault = hc::Fault {
    reason: hc::Reason::Malformed,
    offset: 0,
    len: 0,
};

/// An error this binding raises itself, before the core is asked anything.
struct Refusal {
    kind: ErrorKind,
    message: String,
    column: Option<u32>,
}

fn argument(message: impl Into<String>) -> Refusal {
    Refusal {
        kind: ErrorKind::Argument,
        message: message.into(),
        column: None,
    }
}

fn plan_refusal(column: usize, message: impl Into<String>) -> Refusal {
    Refusal {
        kind: ErrorKind::Plan,
        message: message.into(),
        column: Some(column as u32),
    }
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
        // `Container`, and the reading of any kind the Rust binding gains later.
        _ => FailureKind::Container,
    }
}

fn refusal_of(error: &ht::Error) -> Refusal {
    let message = error.to_string();
    let (kind, column) = match error {
        ht::Error::Io(_) => (ErrorKind::Io, None),
        ht::Error::Separator(_) => (ErrorKind::Separator, None),
        ht::Error::Plan { column } => (ErrorKind::Plan, Some(*column as u32)),
        ht::Error::NoSheet(_) => (ErrorKind::NoSheet, None),
        ht::Error::NoColumn(_) => (ErrorKind::NoColumn, None),
        ht::Error::Unbound => (ErrorKind::Unbound, None),
        ht::Error::AlreadyBound => (ErrorKind::AlreadyBound, None),
        ht::Error::Structure(_) => (ErrorKind::Structure, None),
        // A kind the Rust binding gains later reads as the most general one.
        _ => (ErrorKind::Io, None),
    };
    Refusal {
        kind,
        message,
        column,
    }
}

fn broken<'env>(
    env: &'env Env,
    refusal: Refusal,
    error: Option<&ht::Error>,
) -> Result<Object<'env>> {
    clear_pending_exception(env);
    let failure = match error {
        Some(ht::Error::Structure(failure)) => Some(StructuralFailure {
            kind: failure_kind(failure.kind),
            record: failure.record as f64,
            line: failure.line,
            byte: failure.byte as f64,
            expected: failure.expected,
            found: failure.found,
        }),
        _ => None,
    };
    let mut outcome = Object::new(env)?;
    outcome.set("ok", false)?;
    outcome.set(
        "error",
        TabularError {
            kind: refusal.kind,
            message: refusal.message,
            failure,
            column: refusal.column,
        },
    )?;
    Ok(outcome)
}

fn closed(what: &str) -> Refusal {
    Refusal {
        kind: ErrorKind::Closed,
        message: format!("the {what} was closed"),
        column: None,
    }
}

fn refused(env: &Env, refusal: Refusal) -> Result<Object<'_>> {
    broken(env, refusal, None)
}

fn errored<'env>(env: &'env Env, error: &ht::Error) -> Result<Object<'env>> {
    broken(env, refusal_of(error), Some(error))
}

// ---------------------------------------------------------------------------------------
// Reading arguments.
// ---------------------------------------------------------------------------------------

fn is_absent(value: &Unknown<'_>) -> bool {
    matches!(value.get_type(), Ok(ValueType::Undefined | ValueType::Null))
}

/// A whole number from 0 to `max`, the way an index or a size crosses.
fn whole(value: &Unknown<'_>, max: f64) -> Option<f64> {
    match value.get_type() {
        Ok(ValueType::Number) => unsafe { value.cast::<f64>() }
            .ok()
            .filter(|number| number.fract() == 0.0 && (0.0..=max).contains(number)),
        _ => None,
    }
}

fn index(value: &Unknown<'_>) -> Option<usize> {
    whole(value, f64::from(u32::MAX)).map(|number| number as usize)
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
) -> std::result::Result<Option<Object<'env>>, Refusal> {
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

/// One field of an options object, read the one way that can run the caller's JavaScript (a
/// getter, a Proxy trap); an exception it throws is cleared and refused.
fn field<'env>(
    env: &Env,
    object: &Object<'env>,
    what: &str,
    name: &str,
) -> std::result::Result<Option<Unknown<'env>>, Refusal> {
    match object.get::<Unknown>(name) {
        Ok(value) => Ok(value.filter(|value| !is_absent(value))),
        Err(_) => {
            clear_pending_exception(env);
            Err(argument(format!("{what}.{name} could not be read")))
        }
    }
}

fn flag(
    env: &Env,
    object: &Object<'_>,
    what: &str,
    name: &str,
    default: bool,
) -> std::result::Result<bool, Refusal> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(default);
    };
    match value.get_type() {
        Ok(ValueType::Boolean) => unsafe { value.cast::<bool>() }
            .map_err(|_| argument(format!("{what}.{name} must be a boolean"))),
        _ => Err(argument(format!("{what}.{name} must be a boolean"))),
    }
}

fn size(
    env: &Env,
    object: &Object<'_>,
    what: &str,
    name: &str,
    default: usize,
) -> std::result::Result<usize, Refusal> {
    let Some(value) = field(env, object, what, name)? else {
        return Ok(default);
    };
    match whole(&value, f64::from(u32::MAX)) {
        Some(number) if number >= 1.0 => Ok(number as usize),
        _ => Err(argument(format!(
            "{what}.{name} must be a whole number from 1 to 4294967295"
        ))),
    }
}

fn dialect_of(env: &Env, value: &Unknown<'_>) -> std::result::Result<ht::Dialect, Refusal> {
    let Some(object) = options_object(env, value, "the dialect")? else {
        return Ok(ht::Dialect::CSV);
    };
    let what = "dialect";
    let separator = match field(env, &object, what, "separator")? {
        None => b',',
        Some(value) => {
            let text = string_of(&value)
                .ok_or_else(|| argument("dialect.separator must be a one-character string"))?;
            match text.as_bytes() {
                [byte] => *byte,
                _ => {
                    return Err(Refusal {
                        kind: ErrorKind::Separator,
                        message: format!(
                            "separator {text:?} is not one tab or printable ASCII character other than '\"'"
                        ),
                        column: None,
                    });
                }
            }
        }
    };
    Ok(ht::Dialect::new(separator)
        .with_quoting(flag(env, &object, what, "quoting", true)?)
        .with_header(flag(env, &object, what, "hasHeader", true)?)
        .with_blank_lines_skipped(flag(env, &object, what, "skipBlankLines", true)?))
}

fn delimited_options(
    env: &Env,
    value: &Unknown<'_>,
) -> std::result::Result<ht::DelimitedOptions, Refusal> {
    let defaults = ht::DelimitedOptions::new();
    let Some(object) = options_object(env, value, "the options")? else {
        return Ok(defaults);
    };
    let what = "options";
    Ok(defaults
        .batch_rows(size(env, &object, what, "batchRows", 4096)?)
        .buffer_bytes(size(env, &object, what, "bufferBytes", 256 * 1024)?))
}

fn sheet_options(env: &Env, value: &Unknown<'_>) -> std::result::Result<ht::SheetOptions, Refusal> {
    let Some(object) = options_object(env, value, "the options")? else {
        return Ok(ht::SheetOptions::new());
    };
    let what = "options";
    Ok(ht::SheetOptions::new()
        .with_header(flag(env, &object, what, "hasHeader", true)?)
        .with_empty_rows_skipped(flag(env, &object, what, "skipEmptyRows", true)?)
        .with_batch_rows(size(env, &object, what, "batchRows", 4096)?))
}

/// A plan: `undefined` or `null` is none (the reader waits for `bind`); otherwise an array of
/// `Column`s, each of which must have been declared with values the core can honour.
fn plan_of(
    env: &Env,
    value: &Unknown<'_>,
) -> std::result::Result<Option<Vec<ht::Column>>, Refusal> {
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
            return Err(plan_refusal(
                at as usize,
                format!("plan column {at} is not a Column"),
            ));
        };
        match column.resolved {
            Some(resolved) => plan.push(resolved),
            None => {
                return Err(plan_refusal(
                    at as usize,
                    format!(
                        "plan column {at} declares an ordinal, a format or a {} the core cannot honour",
                        "precision, order or epoch"
                    ),
                ));
            }
        }
    }
    Ok(Some(plan))
}

// ---------------------------------------------------------------------------------------
// Columns.
// ---------------------------------------------------------------------------------------

/// One output column: the source column it reads, the door its cells go through, and, for a
/// numeric door, the notation. Made by a factory (`Column.i32(0)`), never by `new`. A factory
/// never refuses: a column it cannot honour (an ordinal that is not a whole number below
/// 2^20, a format or a declared precision, order or epoch that is not one) is refused, with
/// its place in the plan, when the plan is bound.
#[napi]
pub struct Column {
    door: Door,
    ordinal: f64,
    resolved: Option<ht::Column>,
}

/// A `format` argument: absent is the invariant format; otherwise its `packed` bytes, the
/// 92-byte layout `@skunkwerkx/hypercast`'s `NumFormat` hands out, as the core reads them.
fn format_of(env: &Env, value: &Unknown<'_>) -> Option<hc::NumFormat> {
    if is_absent(value) {
        return Some(hc::NumFormat::INVARIANT);
    }
    let object = match value.get_type() {
        Ok(ValueType::Object | ValueType::Function) => unsafe { value.cast::<Object>() }.ok(),
        _ => None,
    }?;
    let packed = match object.get::<Unknown>("packed") {
        Ok(Some(packed)) => match packed.get_type() {
            Ok(ValueType::Object) => unsafe { packed.cast::<Uint8Array>() }.ok(),
            _ => None,
        },
        _ => None,
    };
    let Some(packed) = packed else {
        clear_pending_exception(env);
        return None;
    };
    let bytes: [u8; 92] = packed.as_ref().try_into().ok()?;
    hc::RawNumFormat::from_le_bytes(bytes).resolve().copied()
}

/// An enum member, the way a HyperCast enum crosses: a whole number.
fn code(value: &Unknown<'_>) -> Option<u32> {
    whole(value, f64::from(u32::MAX)).map(|number| number as u32)
}

impl Column {
    fn make(
        env: &Env,
        door: Door,
        ordinal: &Unknown<'_>,
        declared: Option<ht::Door>,
        format: Option<hc::NumFormat>,
    ) -> Column {
        let given = match ordinal.get_type() {
            Ok(ValueType::Number) => unsafe { ordinal.cast::<f64>() }.unwrap_or(f64::NAN),
            _ => f64::NAN,
        };
        clear_pending_exception(env);
        // Below 2^20, the Rust binding's own ceiling, so `valid` says what `bind` will.
        let ordinal_index = index(ordinal).filter(|at| *at < 1 << 20);
        let resolved = match (ordinal_index, declared, format) {
            (Some(at), Some(door), Some(format)) => Some(ht::Column::new(at, door).format(format)),
            _ => None,
        };
        Column {
            door,
            ordinal: given,
            resolved,
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
                Column::make(env, Door::$door, &ordinal, Some(ht::Door::$door), Some(hc::NumFormat::INVARIANT))
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
        Column::make(
            env,
            Door::Unix,
            &ordinal,
            door,
            Some(hc::NumFormat::INVARIANT),
        )
    }

    /// An Excel date serial in the declared `ExcelEpoch`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, epoch: number")]
    pub fn excel_serial(env: &Env, ordinal: Unknown<'_>, epoch: Unknown<'_>) -> Column {
        let door = code(&epoch)
            .and_then(hc::ExcelEpoch::from_code)
            .map(ht::Door::ExcelSerial);
        Column::make(
            env,
            Door::ExcelSerial,
            &ordinal,
            door,
            Some(hc::NumFormat::INVARIANT),
        )
    }

    /// A separated calendar date whose fields come in the declared `DateOrder`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, order: number")]
    pub fn date_ordered(env: &Env, ordinal: Unknown<'_>, order: Unknown<'_>) -> Column {
        let door = code(&order)
            .and_then(hc::DateOrder::from_code)
            .map(ht::Door::DateOrdered);
        Column::make(
            env,
            Door::DateOrdered,
            &ordinal,
            door,
            Some(hc::NumFormat::INVARIANT),
        )
    }

    /// A zone-less civil date-time whose date fields come in the declared `DateOrder`.
    #[napi(catch_unwind, ts_args_type = "ordinal: number, order: number")]
    pub fn datetime(env: &Env, ordinal: Unknown<'_>, order: Unknown<'_>) -> Column {
        let door = code(&order)
            .and_then(hc::DateOrder::from_code)
            .map(ht::Door::DateTime);
        Column::make(
            env,
            Door::Datetime,
            &ordinal,
            door,
            Some(hc::NumFormat::INVARIANT),
        )
    }

    /// The door its cells go through.
    #[napi(getter, catch_unwind)]
    pub fn door(&self) -> Door {
        self.door
    }

    /// The source column it reads, as it was given (`NaN` for an ordinal that is no number).
    #[napi(getter, catch_unwind)]
    pub fn ordinal(&self) -> f64 {
        self.ordinal
    }

    /// Whether the core can honour it: a plan holding a column that is not valid is refused
    /// when it is bound, with the column's place in the plan.
    #[napi(getter, catch_unwind)]
    pub fn valid(&self) -> bool {
        self.resolved.is_some()
    }
}

// ---------------------------------------------------------------------------------------
// The header.
// ---------------------------------------------------------------------------------------

/// The names a source's header declared, in column order.
#[napi]
pub struct Header {
    names: Vec<Vec<u8>>,
}

impl Header {
    fn from(header: &ht::Header) -> Header {
        Header {
            names: header.names().map(<[u8]>::to_vec).collect(),
        }
    }

    fn find(&self, name: &Unknown<'_>) -> Option<Option<usize>> {
        let wanted = bytes_of(name)?;
        Some(self.names.iter().position(|candidate| *candidate == wanted))
    }
}

#[napi]
impl Header {
    /// Every name, in column order (bytes that are not UTF-8 replaced by U+FFFD).
    #[napi(getter, catch_unwind)]
    pub fn names(&self) -> Vec<String> {
        self.names
            .iter()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect()
    }

    /// How many columns the header names.
    #[napi(getter, catch_unwind)]
    pub fn length(&self) -> u32 {
        self.names.len() as u32
    }

    /// The first column with exactly this name (byte for byte, case and spaces included; a
    /// `Uint8Array` is matched against the bytes the file holds), or `null` if none has it.
    #[napi(catch_unwind, ts_args_type = "name: string | Uint8Array")]
    pub fn ordinal(&self, name: Unknown<'_>) -> Option<u32> {
        self.find(&name).flatten().map(|at| at as u32)
    }

    /// The first column with exactly this name, or the `noColumn` error naming it: what a
    /// plan built from names reads best with.
    #[napi(
        catch_unwind,
        ts_args_type = "name: string | Uint8Array",
        ts_return_type = "Outcome<number>"
    )]
    pub fn require<'env>(&self, env: &'env Env, name: Unknown<'env>) -> Result<Object<'env>> {
        match self.find(&name) {
            None => refused(env, argument("the name must be a string or a Uint8Array")),
            Some(Some(at)) => success(env, at as u32),
            Some(None) => {
                let wanted = bytes_of(&name).unwrap_or_default();
                errored(
                    env,
                    &ht::Error::NoColumn(String::from_utf8_lossy(&wanted).into_owned()),
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Batches: what a read copied out of the reader.
// ---------------------------------------------------------------------------------------

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

/// One column of a batch: each cell's verdict, and the raw text a fault's span indexes.
struct Cells {
    cells: Vec<std::result::Result<Value, hc::Fault>>,
    /// Every cell's raw text, back to back; `spans[row]` is where a row's is.
    raw: Vec<u8>,
    spans: Vec<(usize, usize)>,
    faults: usize,
}

struct BatchData {
    rows: usize,
    doors: Vec<Door>,
    lines: Vec<u32>,
    columns: Vec<Cells>,
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

/// One cell, read through the accessor its door's type is read with (the only one the Rust
/// binding answers for that door).
fn cell(
    batch: &ht::Batch<'_>,
    door: ht::Door,
    column: usize,
    row: usize,
) -> std::result::Result<Value, hc::Fault> {
    match door {
        ht::Door::Bool => batch.get::<bool>(column, row).map(Value::Bool),
        ht::Door::I8 => batch.get::<i8>(column, row).map(|v| Value::Int(v.into())),
        ht::Door::I16 => batch.get::<i16>(column, row).map(|v| Value::Int(v.into())),
        ht::Door::I32 => batch.get::<i32>(column, row).map(Value::Int),
        ht::Door::I64 => batch.get::<i64>(column, row).map(Value::Big),
        ht::Door::U8 => batch.get::<u8>(column, row).map(|v| Value::Uint(v.into())),
        ht::Door::U16 => batch.get::<u16>(column, row).map(|v| Value::Uint(v.into())),
        ht::Door::U32 => batch.get::<u32>(column, row).map(Value::Uint),
        ht::Door::U64 => batch.get::<u64>(column, row).map(Value::UBig),
        ht::Door::F32 => batch.get::<f32>(column, row).map(|v| Value::Real(v.into())),
        ht::Door::F64 => batch.get::<f64>(column, row).map(Value::Real),
        ht::Door::Decimal => batch.get::<hc::Decimal>(column, row).map(Value::Decimal),
        ht::Door::Uuid => batch.get::<[u8; 16]>(column, row).map(Value::Uuid),
        ht::Door::Timestamp | ht::Door::Unix(_) | ht::Door::ExcelSerial(_) => {
            batch.get::<hc::Timestamp>(column, row).map(Value::Instant)
        }
        ht::Door::Date | ht::Door::DateOrdered(_) => {
            batch.get::<hc::Date>(column, row).map(Value::Date)
        }
        ht::Door::DateTime(_) => batch
            .get::<hc::CivilDateTime>(column, row)
            .map(Value::Civil),
        ht::Door::Time => batch.get::<u64>(column, row).map(Value::Time),
        ht::Door::Duration => batch.get::<hc::Duration>(column, row).map(Value::Span),
        ht::Door::Text => batch
            .text_str(column, row)
            .map(|text| Value::Text(text.into_owned())),
        // A door the Rust binding gains later: nothing here knows its type, so no value.
        _ => Err(CONTRACT),
    }
}

impl BatchData {
    fn copy(batch: &ht::Batch<'_>) -> BatchData {
        let rows = batch.rows();
        let plan = batch.columns();
        let columns = plan
            .iter()
            .enumerate()
            .map(|(column, spec)| {
                let mut cells = Cells {
                    cells: Vec::with_capacity(rows),
                    raw: Vec::new(),
                    spans: Vec::with_capacity(rows),
                    faults: 0,
                };
                for row in 0..rows {
                    let verdict = cell(batch, spec.door, column, row);
                    cells.faults += usize::from(verdict.is_err());
                    cells.cells.push(verdict);
                    let raw = batch.raw(column, row);
                    cells.spans.push((cells.raw.len(), raw.len()));
                    cells.raw.extend_from_slice(&raw);
                }
                cells
            })
            .collect();
        BatchData {
            rows,
            doors: plan.iter().map(|column| door_name(column.door)).collect(),
            lines: (0..rows).map(|row| batch.line(row)).collect(),
            columns,
        }
    }

    /// Roughly the heap it holds, for V8's sense of when to collect.
    fn size(&self) -> usize {
        let cell = std::mem::size_of::<std::result::Result<Value, hc::Fault>>();
        let span = std::mem::size_of::<(usize, usize)>();
        self.lines.len() * 4
            + self
                .columns
                .iter()
                .map(|cells| cells.cells.len() * (cell + span) + cells.raw.len())
                .sum::<usize>()
    }

    fn at(&self, column: &Unknown<'_>, row: &Unknown<'_>) -> Option<(usize, usize)> {
        let (column, row) = (index(column)?, index(row)?);
        (column < self.columns.len() && row < self.rows).then_some((column, row))
    }

    fn raw(&self, at: Option<(usize, usize)>) -> &[u8] {
        let Some((column, row)) = at else {
            return &[];
        };
        let cells = &self.columns[column];
        let (from, len) = cells.spans[row];
        &cells.raw[from..from + len]
    }

    fn verdict<'env>(&self, env: &'env Env, at: Option<(usize, usize)>) -> Result<Object<'env>> {
        let Some((column, row)) = at else {
            return failure(env, CONTRACT);
        };
        match &self.columns[column].cells[row] {
            Err(fault) => failure(env, *fault),
            Ok(value) => match value {
                Value::Bool(value) => success(env, *value),
                Value::Int(value) => success(env, *value),
                Value::Uint(value) => success(env, *value),
                Value::Big(value) => success(env, BigInt::from(*value)),
                Value::UBig(value) => success(env, BigInt::from(*value)),
                Value::Real(value) => success(env, *value),
                Value::Decimal(value) => success(env, value.to_string()),
                Value::Uuid(value) => success(env, uuid_text(*value)),
                Value::Instant(value) => success(env, timestamp(*value)),
                Value::Date(value) => success(env, calendar(*value)),
                Value::Civil(value) => success(env, civil(*value)),
                Value::Time(value) => success(env, time_of_day(*value)),
                Value::Span(value) => success(env, duration(*value)),
                Value::Text(value) => success(env, value.as_str()),
            },
        }
    }

    fn line(&self, row: &Unknown<'_>) -> u32 {
        index(row)
            .and_then(|row| self.lines.get(row).copied())
            .unwrap_or(0)
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

/// The rows of one read, column-major, copied out of the reader: a batch stays valid after
/// the reader has moved on.
#[napi(custom_finalize)]
pub struct Batch {
    data: Arc<BatchData>,
    /// The bytes it told V8 it holds, given back when it is collected.
    external: i64,
}

impl ObjectFinalize for Batch {
    fn finalize(self, env: Env) -> Result<()> {
        env.adjust_external_memory(-self.external).map(|_| ())
    }
}

#[napi]
impl Batch {
    /// How many rows the batch holds.
    #[napi(getter, catch_unwind)]
    pub fn rows(&self) -> u32 {
        self.data.rows as u32
    }

    /// How many columns: the plan's length.
    #[napi(getter, catch_unwind)]
    pub fn column_count(&self) -> u32 {
        self.data.columns.len() as u32
    }

    /// The door each column was read through, in plan order.
    #[napi(getter, catch_unwind)]
    pub fn doors(&self) -> Vec<Door> {
        self.data.doors.clone()
    }

    /// The line (delimited text) or the sheet row (a workbook) the row came from; 0 for a
    /// row the batch does not have.
    #[napi(catch_unwind, ts_args_type = "row: number")]
    pub fn line(&self, row: Unknown<'_>) -> u32 {
        self.data.line(&row)
    }

    /// The cell at `row` of `column` as HyperCast judged it. A column or row the batch does
    /// not have is HyperCast's contract violation: `malformed`, offset 0, length 0.
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
        self.data.verdict(env, self.data.at(&column, &row))
    }

    /// The text of the cell, whatever its door and verdict: what a fault's span is a span of.
    /// Empty for an empty cell, a typed workbook cell that cast, and a cell the batch does
    /// not have.
    #[napi(catch_unwind, ts_args_type = "column: number, row: number")]
    pub fn raw(&self, column: Unknown<'_>, row: Unknown<'_>) -> Uint8Array {
        Uint8Array::from(self.data.raw(self.data.at(&column, &row)).to_vec())
    }

    /// `raw` as a string, each invalid UTF-8 sequence replaced by U+FFFD: for display, since
    /// a fault's span counts bytes of `raw`, not UTF-16 code units of this.
    #[napi(catch_unwind, ts_args_type = "column: number, row: number")]
    pub fn raw_string(&self, column: Unknown<'_>, row: Unknown<'_>) -> String {
        String::from_utf8_lossy(self.data.raw(self.data.at(&column, &row))).into_owned()
    }

    /// The whole column, a verdict a row; empty for a column the batch does not have.
    #[napi(
        catch_unwind,
        ts_generic_types = "T extends Value = Value",
        ts_args_type = "column: number",
        ts_return_type = "Array<Verdict<T>>"
    )]
    pub fn column<'env>(&self, env: &'env Env, column: Unknown<'env>) -> Result<Array<'env>> {
        let column = index(&column).filter(|column| *column < self.data.columns.len());
        let rows = if column.is_some() { self.data.rows } else { 0 };
        let mut list = Array::from_vec(env, Vec::<bool>::new())?;
        for row in 0..rows {
            list.insert(self.data.verdict(env, column.map(|column| (column, row)))?)?;
        }
        Ok(list)
    }

    /// How many cells of the column did not cast; 0 for a column the batch does not have.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn fault_count(&self, column: Unknown<'_>) -> u32 {
        index(&column)
            .and_then(|column| self.data.columns.get(column))
            .map_or(0, |cells| cells.faults as u32)
    }

    /// The row at `row`, read across; `null` for a row the batch does not have.
    #[napi(catch_unwind, ts_args_type = "row: number")]
    pub fn row(&self, row: Unknown<'_>) -> Option<Row> {
        index(&row)
            .filter(|row| *row < self.data.rows)
            .map(|index| Row {
                data: Arc::clone(&self.data),
                index,
            })
    }
}

/// One row of a batch, read across: the batch and an index, nothing copied.
#[napi]
pub struct Row {
    data: Arc<BatchData>,
    index: usize,
}

#[napi]
impl Row {
    /// The row's place in its batch.
    #[napi(getter, catch_unwind)]
    pub fn index(&self) -> u32 {
        self.index as u32
    }

    /// The line or the sheet row it came from.
    #[napi(getter, catch_unwind)]
    pub fn line(&self) -> u32 {
        self.data.lines[self.index]
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
        let at = index(&column)
            .filter(|column| *column < self.data.columns.len())
            .map(|column| (column, self.index));
        self.data.verdict(env, at)
    }

    /// The raw text of the cell in `column`, as `Batch.raw`.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn raw(&self, column: Unknown<'_>) -> Uint8Array {
        let at = index(&column)
            .filter(|column| *column < self.data.columns.len())
            .map(|column| (column, self.index));
        Uint8Array::from(self.data.raw(at).to_vec())
    }

    /// `raw` as a string, as `Batch.rawString`.
    #[napi(catch_unwind, ts_args_type = "column: number")]
    pub fn raw_string(&self, column: Unknown<'_>) -> String {
        let at = index(&column)
            .filter(|column| *column < self.data.columns.len())
            .map(|column| (column, self.index));
        String::from_utf8_lossy(self.data.raw(at)).into_owned()
    }
}

/// A read's outcome: the batch, or `null` past the end, or the error that stopped it.
fn read_outcome<'env>(
    env: &'env Env,
    read: std::result::Result<Option<ht::Batch<'_>>, ht::Error>,
) -> Result<Object<'env>> {
    match read {
        Ok(Some(batch)) => {
            let data = BatchData::copy(&batch);
            // Without it V8 sees a small object and collects too rarely for the heap behind
            // it (a no-op under WebAssembly, whose memory is the module's own).
            let external = i64::try_from(data.size()).unwrap_or(i64::MAX);
            env.adjust_external_memory(external)?;
            let batch = Batch {
                data: Arc::new(data),
                external,
            };
            success(env, batch.into_instance(env)?)
        }
        Ok(None) => success(env, Null),
        Err(error) => errored(env, &error),
    }
}

fn bound<'env>(env: &'env Env, bind: std::result::Result<(), ht::Error>) -> Result<Object<'env>> {
    match bind {
        Ok(()) => success(env, Null),
        Err(error) => errored(env, &error),
    }
}

// ---------------------------------------------------------------------------------------
// Delimited text.
// ---------------------------------------------------------------------------------------

/// A forward-only read of delimited text — CSV, TSV, any single-byte ASCII separator —
/// through a plan, a batch at a time. Made by `fromBytes` or `open`, never by `new`.
#[napi]
pub struct DelimitedReader {
    // `None` once closed.
    inner: Option<ht::DelimitedReader<'static>>,
}

/// Where a reader's bytes come from.
enum Source {
    Bytes(Vec<u8>),
    Path(String),
}

fn delimited<'env>(
    env: &'env Env,
    source: std::result::Result<Source, Refusal>,
    dialect: &Unknown<'_>,
    plan: &Unknown<'_>,
    options: &Unknown<'_>,
) -> Result<Object<'env>> {
    let read = (|| {
        let source = source?;
        let dialect = dialect_of(env, dialect)?;
        let plan = plan_of(env, plan)?;
        let options = delimited_options(env, options)?;
        let opened = match (source, plan) {
            (Source::Bytes(bytes), Some(plan)) => {
                options.from_reader(std::io::Cursor::new(bytes), dialect, &plan)
            }
            (Source::Bytes(bytes), None) => {
                options.from_reader_unbound(std::io::Cursor::new(bytes), dialect)
            }
            (Source::Path(path), Some(plan)) => options.open(path, dialect, &plan),
            (Source::Path(path), None) => options.open_unbound(path, dialect),
        };
        Ok(opened)
    })();
    match read {
        Err(refusal) => refused(env, refusal),
        Ok(Err(error)) => errored(env, &error),
        Ok(Ok(inner)) => success(
            env,
            DelimitedReader { inner: Some(inner) }.into_instance(env)?,
        ),
    }
}

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

    /// Declares the plan a reader opened without one reads through: once, before the first
    /// read.
    #[napi(
        catch_unwind,
        ts_args_type = "plan: ReadonlyArray<Column>",
        ts_return_type = "Outcome<null>"
    )]
    pub fn bind<'env>(&mut self, env: &'env Env, plan: Unknown<'env>) -> Result<Object<'env>> {
        match plan_of(env, &plan) {
            Err(refusal) => refused(env, refusal),
            Ok(None) => refused(env, argument("the plan must be an array of Columns")),
            Ok(Some(plan)) => match self.inner.as_mut() {
                Some(inner) => bound(env, inner.bind(&plan)),
                None => refused(env, closed("reader")),
            },
        }
    }

    /// Reads the next batch: the batch, `null` once the text is exhausted, or the error that
    /// stopped it — a structural failure only after every intact row before it was
    /// delivered, and again on every read after.
    #[napi(catch_unwind, ts_return_type = "Outcome<Batch | null>")]
    pub fn read<'env>(&mut self, env: &'env Env) -> Result<Object<'env>> {
        match self.inner.as_mut() {
            Some(inner) => read_outcome(env, inner.read()),
            None => refused(env, closed("reader")),
        }
    }

    /// Releases the reader's buffers and its file now, rather than when it is collected.
    /// Batches already read stay valid; a read or bind after it is a `closed` error.
    #[napi(catch_unwind)]
    pub fn close(&mut self) {
        self.inner = None;
    }

    /// Whether a plan has been bound.
    #[napi(getter, catch_unwind)]
    pub fn is_bound(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| inner.is_bound())
    }

    /// The header record's names, or `null` if the dialect declares no header or the reader
    /// was closed.
    #[napi(getter, catch_unwind)]
    pub fn header(&self) -> Option<Header> {
        self.inner.as_ref()?.header().map(Header::from)
    }

    /// How many cells a record has: the header's count, or the first record's once it has
    /// been read; `null` before either, or once closed.
    #[napi(getter, catch_unwind)]
    pub fn column_count(&self) -> Option<u32> {
        self.inner
            .as_ref()?
            .column_count()
            .map(|count| count as u32)
    }

    /// How many records have been read, the header among them; `null` once closed.
    #[napi(getter, catch_unwind)]
    pub fn records(&self) -> Option<f64> {
        self.inner.as_ref().map(|inner| inner.records() as f64)
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
        Err(error) => errored(env, &error),
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
            // reads it), for as long as the sheet lives: `Sheet` declares `sheet` before
            // `book`, so the sheet is dropped first.
            let borrowed: &'static ht::Workbook<'static> = unsafe { &*Arc::as_ptr(&book) };
            let sheet = match (&which, plan) {
                (Which::Index(at), Some(plan)) => borrowed.sheet(*at, options, &plan),
                (Which::Index(at), None) => borrowed.sheet_unbound(*at, options),
                (Which::Name(name), Some(plan)) => borrowed.sheet(name.as_str(), options, &plan),
                (Which::Name(name), None) => borrowed.sheet_unbound(name.as_str(), options),
            };
            Ok(sheet.map(|sheet| Sheet {
                sheet: Some(sheet),
                book,
            }))
        })();
        match opened {
            Err(refusal) => refused(env, refusal),
            Ok(Err(error)) => errored(env, &error),
            Ok(Ok(sheet)) => success(env, sheet.into_instance(env)?),
        }
    }
}

/// A forward-only read of one sheet through a plan, a batch at a time. Made by
/// `Workbook.sheet`, never by `new`.
#[napi]
pub struct Sheet {
    // Declared first, so dropped first: it borrows `book` (see `Workbook.sheet`).
    // `None` once closed.
    sheet: Option<ht::Sheet<'static>>,
    book: Arc<ht::Workbook<'static>>,
}

#[napi]
impl Sheet {
    /// Declares the plan a sheet opened without one reads through: once, before the first
    /// read.
    #[napi(
        catch_unwind,
        ts_args_type = "plan: ReadonlyArray<Column>",
        ts_return_type = "Outcome<null>"
    )]
    pub fn bind<'env>(&mut self, env: &'env Env, plan: Unknown<'env>) -> Result<Object<'env>> {
        match plan_of(env, &plan) {
            Err(refusal) => refused(env, refusal),
            Ok(None) => refused(env, argument("the plan must be an array of Columns")),
            Ok(Some(plan)) => match self.sheet.as_mut() {
                Some(sheet) => bound(env, sheet.bind(&plan)),
                None => refused(env, closed("sheet")),
            },
        }
    }

    /// Reads the next batch, as `DelimitedReader.read` does.
    #[napi(catch_unwind, ts_return_type = "Outcome<Batch | null>")]
    pub fn read<'env>(&mut self, env: &'env Env) -> Result<Object<'env>> {
        match self.sheet.as_mut() {
            Some(sheet) => read_outcome(env, sheet.read()),
            None => refused(env, closed("sheet")),
        }
    }

    /// Releases the sheet's buffers now, rather than when it is collected (the workbook's
    /// bytes stay until the workbook and its sheets are). Batches already read stay valid; a
    /// read or bind after it is a `closed` error.
    #[napi(catch_unwind)]
    pub fn close(&mut self) {
        self.sheet = None;
    }

    /// Whether a plan has been bound.
    #[napi(getter, catch_unwind)]
    pub fn is_bound(&self) -> bool {
        self.sheet.as_ref().is_some_and(|sheet| sheet.is_bound())
    }

    /// The header row's names, or `null` if the sheet was opened without a header or was
    /// closed.
    #[napi(getter, catch_unwind)]
    pub fn header(&self) -> Option<Header> {
        self.sheet.as_ref()?.header().map(Header::from)
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
