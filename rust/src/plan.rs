//! What the caller declares: the door each output column casts through, the numeric
//! notation its numeric doors accept, and which source ordinal it reads. A plan is a
//! projection — nothing about the source's width is inferred from it.

use hypercast::{NumFormat, UnixPrecision};

/// One HyperCast door, plus [`Door::Text`] for the bytes themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Uuid,
    Timestamp,
    /// A Unix-epoch integer at the declared precision (never guessed from magnitude).
    Unix(UnixPrecision),
    Date,
    Time,
    Duration,
    /// The cell's bytes (or a typed cell's canonical rendering) — no cast.
    Text,
}

impl Door {
    /// The ABI discriminant. `0` is never a door; [`Door::Unix`] carries its precision
    /// beside the code (see [`crate::ffi::RawColumnSpec`]).
    pub const fn code(self) -> u32 {
        match self {
            Door::Bool => 1,
            Door::I8 => 2,
            Door::I16 => 3,
            Door::I32 => 4,
            Door::I64 => 5,
            Door::U8 => 6,
            Door::U16 => 7,
            Door::U32 => 8,
            Door::U64 => 9,
            Door::F32 => 10,
            Door::F64 => 11,
            Door::Uuid => 12,
            Door::Timestamp => 13,
            Door::Unix(_) => 14,
            Door::Date => 15,
            Door::Time => 16,
            Door::Duration => 17,
            Door::Text => 18,
        }
    }

    /// Decodes an ABI discriminant; `precision` is read only for the Unix door
    /// (`1` seconds … `4` nanoseconds, as HyperCast's `cast_unix` export numbers them).
    pub const fn from_code(code: u32, precision: u32) -> Option<Door> {
        Some(match code {
            1 => Door::Bool,
            2 => Door::I8,
            3 => Door::I16,
            4 => Door::I32,
            5 => Door::I64,
            6 => Door::U8,
            7 => Door::U16,
            8 => Door::U32,
            9 => Door::U64,
            10 => Door::F32,
            11 => Door::F64,
            12 => Door::Uuid,
            13 => Door::Timestamp,
            14 => Door::Unix(match precision {
                1 => UnixPrecision::Seconds,
                2 => UnixPrecision::Millis,
                3 => UnixPrecision::Micros,
                4 => UnixPrecision::Nanos,
                _ => return None,
            }),
            15 => Door::Date,
            16 => Door::Time,
            17 => Door::Duration,
            18 => Door::Text,
            _ => return None,
        })
    }

    /// True for the doors that read a [`NumFormat`] (integers and reals).
    pub const fn is_numeric(self) -> bool {
        matches!(
            self,
            Door::I8
                | Door::I16
                | Door::I32
                | Door::I64
                | Door::U8
                | Door::U16
                | Door::U32
                | Door::U64
                | Door::F32
                | Door::F64
        )
    }
}

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
