//! Structural failures — the provider's own concern, never a cell verdict. A reader that
//! has failed keeps returning the same error: forward-only means there is no recovery
//! point.

use std::fmt;
use std::io;
use std::sync::Arc;

/// Why the reader stopped.
#[derive(Clone, Debug)]
pub enum Error {
    /// The declared separator cannot be honoured (see [`crate::delimited::Dialect::is_valid_separator`]).
    /// A caller bug, not a data verdict.
    Separator(u8),
    /// The underlying read failed.
    Io(Arc<io::Error>),
    /// The input ended inside a quoted cell.
    UnclosedQuote {
        /// Zero-based index of the record (header included) the quote opened in.
        record: u64,
        /// One-based line the record started on.
        line: u32,
        /// Absolute byte offset of the record's start.
        byte: u64,
    },
    /// A record's cell count disagrees with the first record's.
    ColumnCount {
        /// Cells in the first record.
        expected: usize,
        /// Cells in this record.
        found: usize,
        /// Zero-based index of the offending record (header included).
        record: u64,
        /// One-based line the record started on.
        line: u32,
        /// Absolute byte offset of the record's start.
        byte: u64,
    },
    /// One record is larger than the buffer ceiling.
    RowTooLong {
        /// The ceiling, in bytes.
        limit: usize,
        /// Zero-based index of the offending record (header included).
        record: u64,
        /// One-based line the record started on.
        line: u32,
        /// Absolute byte offset of the record's start.
        byte: u64,
    },
}

impl Error {
    /// The ABI code for this error's kind: `1` unclosed quote, `2` column count,
    /// `3` row too long, `4` I/O, `5` separator.
    pub const fn code(&self) -> i32 {
        match self {
            Error::UnclosedQuote { .. } => 1,
            Error::ColumnCount { .. } => 2,
            Error::RowTooLong { .. } => 3,
            Error::Io(_) => 4,
            Error::Separator(_) => 5,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(Arc::new(error))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Separator(byte) => write!(
                f,
                "separator byte 0x{byte:02X} is not tab or printable ASCII other than '\"'"
            ),
            Error::Io(error) => write!(f, "read failed: {error}"),
            Error::UnclosedQuote { record, line, byte } => {
                write!(
                    f,
                    "input ended inside a quoted cell in record {record} (line {line}, byte {byte})"
                )
            }
            Error::ColumnCount {
                expected,
                found,
                record,
                line,
                byte,
            } => write!(
                f,
                "record {record} (line {line}, byte {byte}) has {found} cells; the first record had {expected}"
            ),
            Error::RowTooLong {
                limit,
                record,
                line,
                byte,
            } => {
                write!(
                    f,
                    "record {record} (line {line}, byte {byte}) exceeds the {limit}-byte row ceiling"
                )
            }
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
