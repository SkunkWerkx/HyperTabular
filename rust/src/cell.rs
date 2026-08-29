//! The format-neutral cell: what a provider hands the cast engine. Delimited text only
//! ever has bytes; a workbook stores typed values. One enum covers both so the cast
//! matrix (`cast.rs`, and the table in `docs/design.md`) is written exactly once.

use hypercast::{Date, Duration};

/// One cell as the provider read it. Borrowed variants point into the provider's own
/// buffer and are valid until the next row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell<'a> {
    /// No value: a missing cell, a blank delimited field, a gap in a sparse workbook row.
    Empty,
    /// UTF-8 bytes, untrimmed — the HyperCast doors do their own ASCII trimming.
    Text(&'a [u8]),
    /// The IEEE double a workbook stores for a numeric cell with a non-temporal format.
    Number(f64),
    /// A workbook boolean cell.
    Bool(bool),
    /// A zoneless wall-clock instant: calendar date plus nanoseconds since midnight.
    /// Date-formatted Excel serials ≥ 1, ODS `office:date-value`, XLSX `t="d"`.
    Wall {
        /// The calendar date, proleptic Gregorian, years `1..=9999`.
        date: Date,
        /// Nanoseconds since midnight, `0..86_400_000_000_000`.
        nanos: u64,
    },
    /// A time of day with no date: a time-formatted Excel serial in `[0, 1)`.
    Clock(u64),
    /// A signed duration: Excel elapsed formats (`[h]:mm:ss`), ODS `office:time-value`.
    Span(Duration),
    /// A formula error cell (`#N/A`, `#DIV/0!`, …).
    Error(CellError),
}

impl Cell<'_> {
    /// True for [`Cell::Empty`].
    pub const fn is_empty(&self) -> bool {
        matches!(self, Cell::Empty)
    }
}

/// The closed set of spreadsheet error values. Discriminants are the BIFF error codes,
/// which XLSB and XLS store directly; XLSX and ODS spell them out.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellError {
    /// `#NULL!`
    Null = 0x00,
    /// `#DIV/0!`
    DivideByZero = 0x07,
    /// `#VALUE!`
    Value = 0x0F,
    /// `#REF!`
    Reference = 0x17,
    /// `#NAME?`
    Name = 0x1D,
    /// `#NUM!`
    Number = 0x24,
    /// `#N/A`
    NotAvailable = 0x2A,
    /// `#GETTING_DATA`
    GettingData = 0x2B,
}

impl CellError {
    /// The spreadsheet spelling of the error.
    pub const fn text(self) -> &'static str {
        match self {
            CellError::Null => "#NULL!",
            CellError::DivideByZero => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Reference => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Number => "#NUM!",
            CellError::NotAvailable => "#N/A",
            CellError::GettingData => "#GETTING_DATA",
        }
    }

    /// Parses the spreadsheet spelling (`#N/A`, …), as XLSX `t="e"` cells carry it.
    pub fn from_text(text: &[u8]) -> Option<CellError> {
        Some(match text {
            b"#NULL!" => CellError::Null,
            b"#DIV/0!" => CellError::DivideByZero,
            b"#VALUE!" => CellError::Value,
            b"#REF!" => CellError::Reference,
            b"#NAME?" => CellError::Name,
            b"#NUM!" => CellError::Number,
            b"#N/A" => CellError::NotAvailable,
            b"#GETTING_DATA" => CellError::GettingData,
            _ => return None,
        })
    }

    /// Decodes the BIFF error byte XLS and XLSB store.
    pub const fn from_biff(code: u8) -> Option<CellError> {
        Some(match code {
            0x00 => CellError::Null,
            0x07 => CellError::DivideByZero,
            0x0F => CellError::Value,
            0x17 => CellError::Reference,
            0x1D => CellError::Name,
            0x24 => CellError::Number,
            0x2A => CellError::NotAvailable,
            0x2B => CellError::GettingData,
            _ => return None,
        })
    }
}
