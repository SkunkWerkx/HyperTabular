//! Structural failures — the container's or a part's, never a cell verdict. A sheet that
//! has failed keeps returning the same error: forward-only means there is no recovery
//! point.

use std::fmt;
use std::io;
use std::sync::Arc;

/// Why the workbook or sheet stopped.
#[derive(Clone, Debug)]
pub enum Error {
    /// The underlying read failed.
    Io(Arc<io::Error>),
    /// The zip container is broken or uses something unsupported (encryption, a
    /// compression method other than stored/deflate).
    Container(String),
    /// A required part is missing or its XML/values are not what the format promises.
    Malformed {
        /// The part name (`xl/worksheets/sheet1.xml`, `content.xml`, …).
        part: String,
        /// What was wrong.
        detail: String,
    },
    /// The container is a zip but neither an XLSX nor an ODS workbook.
    NotAWorkbook,
    /// No sheet by that index or name.
    SheetNotFound(String),
}

impl Error {
    /// The ABI code for this error's kind: `1` container, `2` malformed, `3` not a
    /// workbook, `4` sheet not found, `5` I/O.
    pub const fn code(&self) -> i32 {
        match self {
            Error::Container(_) => 1,
            Error::Malformed { .. } => 2,
            Error::NotAWorkbook => 3,
            Error::SheetNotFound(_) => 4,
            Error::Io(_) => 5,
        }
    }

    pub(crate) fn malformed(part: &str, detail: impl Into<String>) -> Error {
        Error::Malformed {
            part: part.to_string(),
            detail: detail.into(),
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
            Error::Io(error) => write!(f, "read failed: {error}"),
            Error::Container(detail) => write!(f, "zip container: {detail}"),
            Error::Malformed { part, detail } => write!(f, "{part}: {detail}"),
            Error::NotAWorkbook => {
                f.write_str("the container is neither an XLSX nor an ODS workbook")
            }
            Error::SheetNotFound(name) => write!(f, "no sheet {name}"),
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
