//! The caller-declared dialect. The same philosophy as HyperCast's `NumFormat`: no
//! sniffing, no guessing — the separator is stated, quoting is stated, the header is
//! stated.

use crate::delimited::error::Error;

/// How the text is delimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dialect {
    /// The single-byte separator: `\t` or any printable ASCII byte except `"`.
    pub separator: u8,
    /// Whether `"` quotes cells (RFC 4180 style, `""` for a literal quote). Off, a `"`
    /// is an ordinary byte — for files that never quote, it is also faster.
    pub quoting: bool,
    /// Whether the first record is a header, exposed through [`crate::delimited::Reader::header`]
    /// and never delivered as a row.
    pub has_header: bool,
    /// Whether a completely empty line is skipped rather than delivered as a one-cell row.
    pub skip_blank_lines: bool,
}

impl Dialect {
    /// Comma-separated, quoted, with a header, blank lines skipped.
    pub const CSV: Dialect = Dialect {
        separator: b',',
        quoting: true,
        has_header: true,
        skip_blank_lines: true,
    };
    /// Tab-separated, otherwise as [`Dialect::CSV`].
    pub const TSV: Dialect = Dialect {
        separator: b'\t',
        ..Dialect::CSV
    };
    /// Pipe-separated, otherwise as [`Dialect::CSV`].
    pub const PSV: Dialect = Dialect {
        separator: b'|',
        ..Dialect::CSV
    };

    /// A dialect over `separator`, otherwise as [`Dialect::CSV`].
    pub const fn new(separator: u8) -> Dialect {
        Dialect {
            separator,
            ..Dialect::CSV
        }
    }

    /// The same dialect with quoting on or off.
    pub const fn with_quoting(self, quoting: bool) -> Dialect {
        Dialect { quoting, ..self }
    }

    /// The same dialect with or without a header record.
    pub const fn with_header(self, has_header: bool) -> Dialect {
        Dialect { has_header, ..self }
    }

    /// The same dialect, skipping or delivering blank lines.
    pub const fn with_blank_lines_skipped(self, skip_blank_lines: bool) -> Dialect {
        Dialect {
            skip_blank_lines,
            ..self
        }
    }

    /// True for a separator the scanner can honour: tab, or printable ASCII other than
    /// the quote. (`\r`, `\n`, NUL and other control bytes are structural or padding.)
    pub const fn is_valid_separator(separator: u8) -> bool {
        crate::kernel::delimited::fill::RawDialect::is_valid_separator(separator)
    }

    /// Rejects an unusable separator as a contract violation.
    pub fn validate(&self) -> Result<(), Error> {
        if Dialect::is_valid_separator(self.separator) {
            Ok(())
        } else {
            Err(Error::Separator(self.separator))
        }
    }
}
