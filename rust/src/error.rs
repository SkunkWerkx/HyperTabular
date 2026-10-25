//! What can go wrong that is not a cell's verdict: the read itself, the caller's own
//! declarations, and data that is structurally broken. A cell that does not cast is never
//! an error — it is a verdict in the batch — and a reader that has failed keeps returning
//! the same error: forward-only means there is no recovery point.

use crate::kernel::abi;
use std::fmt;
use std::io;
use std::sync::Arc;

/// Why a reader could not be made, or stopped.
///
/// Additional failure modes may be added in a minor release; matches outside this
/// crate need a wildcard arm.
///
/// ```compile_fail,E0004
/// use hypertabular::Error;
/// fn closed(error: Error) {
///     match error {
///         Error::Io(_) | Error::Separator(_) | Error::Plan { .. } |
///         Error::NoSheet(_) | Error::NoColumn(_) | Error::Unbound |
///         Error::AlreadyBound | Error::Structure(_) => (),
///     }
/// }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Error {
    /// The underlying read failed.
    Io(Arc<io::Error>),
    /// The dialect's separator is not one the scanner honours: a tab, or printable ASCII
    /// other than `"`.
    Separator(u8),
    /// Plan column `column` cannot be honoured: its numeric format declares the same
    /// decimal and group separator (or no decimal separator at all), its ordinal is 2^20
    /// (1,048,576) or more — past any workbook format's columns — or, in delimited text, at
    /// or past the width a header has given every record, or the row slots its reach takes
    /// cannot be allocated.
    Plan {
        /// The column's place in the plan.
        column: usize,
    },
    /// The workbook has no sheet by that name, or at that index.
    NoSheet(String),
    /// The header has no column by this name ([`crate::Header::require`]).
    NoColumn(String),
    /// A reader opened without a plan was read before one was bound.
    Unbound,
    /// A plan was bound to a reader that already has one: a reader is bound once, before
    /// its first read.
    AlreadyBound,
    /// The data is structurally broken. Every intact row before the break was delivered
    /// first.
    Structure(Failure),
}

/// A structural failure: what is broken, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What is wrong.
    pub kind: FailureKind,
    /// Delimited text: the zero-based index of the offending record, the header among
    /// those counted — which is how many records were read whole before it. A workbook:
    /// the part of the package being read (`1` `_rels/.rels`, `2` the workbook, `3` its
    /// relationships, `4` the styles, `5` the shared strings, `6` a worksheet, `7`
    /// `content.xml`, `8` `mimetype`, `9` the manifest, `10` a Numbers document's `.iwa`
    /// parts), or `0` for the container itself.
    pub record: u64,
    /// Delimited text: the 1-based line the record starts on. A workbook: the sheet row
    /// being read, where there is one.
    pub line: u32,
    /// Delimited text: the byte offset of the record in the input. A workbook: how far
    /// into the part's inflated bytes the read had got.
    pub byte: u64,
    /// What was expected, for the kinds that say (the first record's cell count; the
    /// length of the shared-string table; the identifier or version an IWA failure names).
    pub expected: u32,
    /// What was found, for the kinds that say (this record's cell count; the compression
    /// method; the shared-string index; which XML construct the part ends inside; what is
    /// wrong with an IWA part).
    pub found: u32,
}

/// What a structural failure is. The numbers are the core's failure codes.
///
/// A code the core gains later is a new variant, and until this binding knows it the code
/// reads as [`Container`](FailureKind::Container), the most general refusal; a match outside
/// this crate names a wildcard arm.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailureKind {
    /// The input ended inside a quoted cell.
    UnclosedQuote = 1,
    /// A record's cell count disagrees with the first record's.
    ColumnCount = 2,
    /// One record is larger than the reader will buffer (1 GiB), or, read in place from a
    /// slice, than the core takes in one call (2 GiB).
    RowTooLong = 3,
    /// The container is not a zip file.
    NotAZip = 16,
    /// The zip's own structure is broken.
    Container = 17,
    /// The part, or the document, is encrypted.
    Encrypted = 18,
    /// A part is compressed by a method that is neither stored nor deflate.
    Method = 19,
    /// A part the workbook cannot be read without is missing.
    MissingPart = 20,
    /// A part's XML ends inside a construct.
    Xml = 21,
    /// A part's bytes are not a deflate stream, or stop before the stream does.
    Deflate = 22,
    /// The container is a zip but not an XLSX, ODS or Numbers workbook.
    NotAWorkbook = 23,
    /// A cell names a shared string that is not in the table.
    SharedString = 24,
    /// More text than can be addressed: over 4 GiB in a batch, or 2 GiB in a cell.
    TooLarge = 25,
    /// A Numbers part is not what the IWA packaging says: a broken chunk or Snappy block
    /// (`found` 1), a malformed message (2), an object a reference names that is not there
    /// (3, `expected` its identifier), cell storage of a version the core does not read
    /// (4, `expected` the version).
    Iwa = 26,
}

impl FailureKind {
    /// The kind's name as the conformance corpus spells it.
    pub const fn name(self) -> &'static str {
        match self {
            FailureKind::UnclosedQuote => "unclosed_quote",
            FailureKind::ColumnCount => "column_count",
            FailureKind::RowTooLong => "row_too_long",
            FailureKind::NotAZip => "not_a_zip",
            FailureKind::Container => "container",
            FailureKind::Encrypted => "encrypted",
            FailureKind::Method => "method",
            FailureKind::MissingPart => "missing_part",
            FailureKind::Xml => "xml",
            FailureKind::Deflate => "deflate",
            FailureKind::NotAWorkbook => "not_a_workbook",
            FailureKind::SharedString => "shared_string",
            FailureKind::TooLarge => "too_large",
            FailureKind::Iwa => "iwa",
        }
    }

    const fn from_code(code: u32) -> FailureKind {
        match code {
            abi::Failure::UNCLOSED_QUOTE => FailureKind::UnclosedQuote,
            abi::Failure::COLUMN_COUNT => FailureKind::ColumnCount,
            3 => FailureKind::RowTooLong,
            abi::Failure::NOT_A_ZIP => FailureKind::NotAZip,
            abi::Failure::ENCRYPTED => FailureKind::Encrypted,
            abi::Failure::METHOD => FailureKind::Method,
            abi::Failure::MISSING_PART => FailureKind::MissingPart,
            abi::Failure::XML => FailureKind::Xml,
            abi::Failure::DEFLATE => FailureKind::Deflate,
            abi::Failure::NOT_A_WORKBOOK => FailureKind::NotAWorkbook,
            abi::Failure::SHARED_STRING => FailureKind::SharedString,
            abi::Failure::TOO_LARGE => FailureKind::TooLarge,
            abi::Failure::IWA => FailureKind::Iwa,
            // The one code left, and the reading of any the core has yet to be given.
            _ => FailureKind::Container,
        }
    }
}

impl From<abi::Failure> for Failure {
    fn from(failure: abi::Failure) -> Failure {
        Failure {
            kind: FailureKind::from_code(failure.code),
            record: failure.record,
            line: failure.line,
            byte: failure.byte,
            expected: failure.expected,
            found: failure.found,
        }
    }
}

impl From<abi::Failure> for Error {
    fn from(failure: abi::Failure) -> Error {
        Error::Structure(failure.into())
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(Arc::new(error))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Failure {
            record,
            line,
            byte,
            expected,
            found,
            ..
        } = *self;
        match self.kind {
            FailureKind::UnclosedQuote => write!(
                f,
                "record {record} (line {line}, byte {byte}): the input ends inside a quoted cell"
            ),
            FailureKind::ColumnCount => write!(
                f,
                "record {record} (line {line}, byte {byte}) has {found} cells where the first has {expected}"
            ),
            FailureKind::RowTooLong => write!(
                f,
                "record {record} (line {line}, byte {byte}) is larger than a reader will buffer"
            ),
            FailureKind::NotAZip => f.write_str("the container is not a zip file"),
            FailureKind::NotAWorkbook => {
                f.write_str("the container is not an XLSX, ODS or Numbers workbook")
            }
            FailureKind::SharedString => write!(
                f,
                "row {line} names shared string {found} of {expected} (part {record}, byte {byte})"
            ),
            kind => write!(
                f,
                "{} in part {record} at byte {byte} (row {line}, detail {found})",
                kind.name()
            ),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "read failed: {error}"),
            Error::Separator(byte) => write!(
                f,
                "separator byte 0x{byte:02X} is not tab or printable ASCII other than '\"'"
            ),
            Error::Plan { column } => write!(
                f,
                "plan column {column} declares a numeric format or an ordinal that cannot be honoured"
            ),
            Error::NoSheet(which) => write!(f, "the workbook has no sheet {which}"),
            Error::NoColumn(name) => write!(f, "the header has no column named {name:?}"),
            Error::Unbound => f.write_str("the reader has no plan yet: bind one before reading"),
            Error::AlreadyBound => {
                f.write_str("the reader already has a plan: a plan is bound once")
            }
            Error::Structure(failure) => failure.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
