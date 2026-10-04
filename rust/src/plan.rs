//! What the caller declares: the door each output column casts through, the numeric
//! notation its numeric doors accept, and which source ordinal it reads. A plan is a
//! projection — nothing about the source's width is inferred from it.

pub use crate::kernel::door::Door;
use hypercast::NumFormat;

/// One output column: the source ordinal it reads, the door it casts through, and the
/// numeric notation (ignored by non-numeric doors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// Zero-based ordinal in the source row. Past the row's end reads as [`crate::Cell::Empty`].
    pub ordinal: usize,
    /// The door.
    pub door: Door,
    /// The declared numeric notation; [`NumFormat::INVARIANT`] unless changed.
    pub format: NumFormat,
}

impl Column {
    /// A column over `ordinal` through `door` with the invariant numeric notation.
    pub const fn new(ordinal: usize, door: Door) -> Column {
        Column {
            ordinal,
            door,
            format: NumFormat::INVARIANT,
        }
    }

    /// The same column under a declared numeric notation.
    pub const fn with_format(self, format: NumFormat) -> Column {
        Column { format, ..self }
    }
}

/// The ordered list of output columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    columns: Vec<Column>,
}

impl Plan {
    /// A plan over the given columns, in output order.
    pub fn new(columns: Vec<Column>) -> Plan {
        Plan { columns }
    }

    /// The columns, in output order.
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The number of output columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// True when the plan produces nothing.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// The highest source ordinal any column reads, if the plan is non-empty.
    pub fn max_ordinal(&self) -> Option<usize> {
        self.columns.iter().map(|column| column.ordinal).max()
    }
}
