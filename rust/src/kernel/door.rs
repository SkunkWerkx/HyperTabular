//! The doors a column can be cast through: HyperCast's, plus [`Door::Text`] for the
//! bytes themselves.

use hypercast::{DateOrder, ExcelEpoch, UnixPrecision};

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
    /// An exact decimal: sign, 96-bit magnitude, base-10 scale. Never a float.
    Decimal,
    /// A separated calendar date under the declared field order.
    DateOrdered(DateOrder),
    /// A zone-less civil date-time under the declared field order.
    DateTime(DateOrder),
    /// An Excel date serial under the declared date system.
    ExcelSerial(ExcelEpoch),
}

impl Door {
    /// The ABI discriminant. `0` is never a door; a door that declares something — the
    /// Unix precision, a date order, an Excel date system — carries it beside the code
    /// (see [`crate::abi::ColumnSpec`]), numbered as HyperCast's own exports number it.
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
            Door::Decimal => 19,
            Door::DateOrdered(_) => 20,
            Door::DateTime(_) => 21,
            Door::ExcelSerial(_) => 22,
        }
    }

    /// Decodes an ABI discriminant; `param` is read only by the doors that declare
    /// something: the Unix precision (`1` seconds … `4` nanoseconds), a date order
    /// (`1` year-month-day, `2` month-day-year, `3` day-month-year), or an Excel date
    /// system (`1` 1900, `2` 1904).
    pub const fn from_code(code: u32, param: u32) -> Option<Door> {
        const fn order(param: u32) -> Option<DateOrder> {
            Some(match param {
                1 => DateOrder::YearMonthDay,
                2 => DateOrder::MonthDayYear,
                3 => DateOrder::DayMonthYear,
                _ => return None,
            })
        }
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
            14 => Door::Unix(match param {
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
            19 => Door::Decimal,
            20 => match order(param) {
                Some(order) => Door::DateOrdered(order),
                None => return None,
            },
            21 => match order(param) {
                Some(order) => Door::DateTime(order),
                None => return None,
            },
            22 => Door::ExcelSerial(match param {
                1 => ExcelEpoch::Y1900,
                2 => ExcelEpoch::Y1904,
                _ => return None,
            }),
            _ => return None,
        })
    }

    /// True for the doors that read a [`NumFormat`] (integers, reals and decimals).
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
                | Door::Decimal
        )
    }
}
