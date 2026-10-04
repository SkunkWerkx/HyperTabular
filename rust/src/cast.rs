//! The cast matrix: one function per door, every `(Cell, Door)` pair with one defined
//! outcome. Text cells go through the HyperCast door verbatim — the fault span is
//! HyperCast's own, indexed into the cell's bytes. Typed cells are converted directly
//! (a workbook's `42.0` never round-trips through text to become an `i32`), and their
//! faults carry a zero span until the batch renders the cell (`batch.rs`).
//!
//! The full table is in `docs/design.md` under "The cast matrix".

use crate::cell::Cell;
use crate::plan::{Column, Door};
use crate::render::render;
use crate::serial::{self, NANOS_PER_DAY};
use hypercast::{
    CivilDateTime, Date, DateOrder, Decimal, Duration, ExcelEpoch, Fault, MAX_TIMESTAMP_SECONDS,
    MIN_TIMESTAMP_SECONDS, NumFormat, Reason, Timestamp, UnixPrecision, excel_serial,
};
use std::borrow::Cow;

const EMPTY: Fault = Fault {
    reason: Reason::Empty,
    offset: 0,
    len: 0,
};
const MALFORMED: Fault = Fault {
    reason: Reason::Malformed,
    offset: 0,
    len: 0,
};
const OUT_OF_RANGE: Fault = Fault {
    reason: Reason::OutOfRange,
    offset: 0,
    len: 0,
};

fn spanless(reason: Reason) -> Fault {
    match reason {
        Reason::Empty => EMPTY,
        Reason::Malformed => MALFORMED,
        Reason::OutOfRange => OUT_OF_RANGE,
    }
}

/// A finite, integral double as any integer type: non-integral is `Malformed`, integral
/// but unrepresentable is `OutOfRange`.
fn number_to_int<T: TryFrom<i128>>(value: f64) -> Result<T, Fault> {
    if !value.is_finite() || value.fract() != 0.0 {
        return Err(MALFORMED);
    }
    // Every integral double below 2^127 in magnitude converts to i128 exactly.
    if value.abs() >= 1.701_411_834_604_692_3e38 {
        return Err(OUT_OF_RANGE);
    }
    T::try_from(value as i128).map_err(|_| OUT_OF_RANGE)
}

macro_rules! integer_doors {
    ($($door:ident => $ty:ty),+ $(,)?) => {$(
        /// Casts a cell through the same-named HyperCast integer door: text verbatim, a
        /// workbook number if it is integral and in range, everything else `Malformed`.
        pub fn $door(cell: &Cell<'_>, format: &NumFormat) -> Result<$ty, Fault> {
            match *cell {
                Cell::Empty => Err(EMPTY),
                Cell::Text(text) => hypercast::$door(text, format),
                Cell::Number(value) => number_to_int(value),
                _ => Err(MALFORMED),
            }
        }
    )+};
}

integer_doors! {
    cast_i8 => i8,
    cast_i16 => i16,
    cast_i32 => i32,
    cast_i64 => i64,
    cast_u8 => u8,
    cast_u16 => u16,
    cast_u32 => u32,
    cast_u64 => u64,
}

/// Casts a cell through HyperCast's `f64` door; a workbook number passes through as-is
/// when finite.
pub fn cast_f64(cell: &Cell<'_>, format: &NumFormat) -> Result<f64, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_f64(text, format),
        Cell::Number(value) if value.is_nan() => Err(MALFORMED),
        Cell::Number(value) if value.is_infinite() => Err(OUT_OF_RANGE),
        Cell::Number(value) => Ok(value),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's `f32` door; a workbook number that overflows `f32`
/// is `OutOfRange`.
pub fn cast_f32(cell: &Cell<'_>, format: &NumFormat) -> Result<f32, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_f32(text, format),
        Cell::Number(value) if value.is_nan() => Err(MALFORMED),
        Cell::Number(value) => {
            let narrowed = value as f32;
            if narrowed.is_infinite() {
                Err(OUT_OF_RANGE)
            } else {
                Ok(narrowed)
            }
        }
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's boolean door; a workbook boolean is itself and a
/// workbook number is accepted only as exactly `0` or `1`.
pub fn cast_bool(cell: &Cell<'_>) -> Result<bool, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_bool(text),
        Cell::Bool(value) => Ok(value),
        Cell::Number(value) => {
            if value == 1.0 {
                Ok(true)
            } else if value == 0.0 {
                Ok(false)
            } else {
                Err(MALFORMED)
            }
        }
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's UUID door; only text can be a UUID.
pub fn cast_uuid(cell: &Cell<'_>) -> Result<[u8; 16], Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_uuid(text),
        _ => Err(MALFORMED),
    }
}

/// A wall-clock reading taken as UTC — the one interpretive act in the matrix, stated in
/// `docs/design.md`: neither Excel serials nor ODS date-values carry a zone.
pub(crate) fn wall_to_timestamp(date: Date, nanos: u64) -> Timestamp {
    CivilDateTime {
        date,
        nanos_of_day: nanos,
    }
    .assume_utc()
}

/// A workbook number read as a date serial: HyperCast's typed door, verbatim.
fn serial_to_wall(value: f64, system: ExcelEpoch) -> Result<(Date, u64), Fault> {
    excel_serial(value, system)
        .map(|wall| (wall.date, wall.nanos_of_day))
        .map_err(spanless)
}

/// Casts a cell through HyperCast's RFC 3339 timestamp door; a workbook number is read
/// as an Excel serial under `system`, and a wall-clock cell is read as UTC.
pub fn cast_timestamp(cell: &Cell<'_>, system: ExcelEpoch) -> Result<Timestamp, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_timestamp(text),
        Cell::Number(value) => {
            let (date, nanos) = serial_to_wall(value, system)?;
            Ok(wall_to_timestamp(date, nanos))
        }
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's Unix-epoch door at the declared precision; a workbook
/// number is the epoch integer itself (integral, in the timestamp window), and a
/// wall-clock cell is read as UTC.
pub fn cast_unix(cell: &Cell<'_>, precision: UnixPrecision) -> Result<Timestamp, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_unix(text, precision),
        Cell::Number(value) => {
            let ticks: i128 = number_to_int(value)?;
            let (per_second, nanos_per_tick) = match precision {
                UnixPrecision::Seconds => (1, 1_000_000_000),
                UnixPrecision::Millis => (1_000, 1_000_000),
                UnixPrecision::Micros => (1_000_000, 1_000),
                UnixPrecision::Nanos => (1_000_000_000, 1),
            };
            // Nanos count forward from `seconds`, even before the epoch (protobuf).
            let seconds = ticks.div_euclid(per_second);
            let nanos = ticks.rem_euclid(per_second) * nanos_per_tick;
            if seconds < i128::from(MIN_TIMESTAMP_SECONDS)
                || seconds > i128::from(MAX_TIMESTAMP_SECONDS)
            {
                return Err(OUT_OF_RANGE);
            }
            Ok(Timestamp {
                seconds: seconds as i64,
                nanos: nanos as i32,
            })
        }
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's `yyyy-MM-dd` door; a workbook number is read as an
/// Excel serial under `system`, by HyperCast's rules.
pub fn cast_date(cell: &Cell<'_>, system: ExcelEpoch) -> Result<Date, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_date(text),
        Cell::Number(value) => serial_to_wall(value, system).map(|(date, _)| date),
        Cell::Wall { date, .. } => Ok(date),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's ordered-date door: text under the declared field
/// order; every typed cell exactly as [`cast_date`] reads it, since a stored date has no
/// field order to declare.
pub fn cast_date_ordered(
    cell: &Cell<'_>,
    order: DateOrder,
    system: ExcelEpoch,
) -> Result<Date, Fault> {
    match *cell {
        Cell::Text(text) => hypercast::cast_date_ordered(text, order),
        _ => cast_date(cell, system),
    }
}

/// Casts a cell through HyperCast's civil date-time door: text under the declared field
/// order, a workbook number as an Excel serial under `system`, and a wall-clock cell as
/// itself — the one door that takes it without assuming a zone.
pub fn cast_datetime(
    cell: &Cell<'_>,
    order: DateOrder,
    system: ExcelEpoch,
) -> Result<CivilDateTime, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_datetime(text, order),
        Cell::Number(value) => excel_serial(value, system).map_err(spanless),
        Cell::Wall { date, nanos } => Ok(CivilDateTime {
            date,
            nanos_of_day: nanos,
        }),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's Excel-serial door under the *declared* date system —
/// for a column of serials the file did not format as dates: text and workbook numbers
/// alike, read as UTC. A wall-clock cell is already past its serial and is read as UTC.
pub fn cast_excel_serial(cell: &Cell<'_>, epoch: ExcelEpoch) -> Result<Timestamp, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_excel_serial(text, epoch),
        Cell::Number(value) => excel_serial(value, epoch)
            .map(CivilDateTime::assume_utc)
            .map_err(spanless),
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's exact decimal door. Only text can be one: a workbook
/// number is a binary double, and which decimal it "meant" is not something the file says.
pub fn cast_decimal(cell: &Cell<'_>, format: &NumFormat) -> Result<Decimal, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_decimal(text, format),
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's 24-hour time door, to nanoseconds since midnight; a
/// workbook number contributes its fractional day, and a span must lie in `[0, 24h)`.
pub fn cast_time(cell: &Cell<'_>) -> Result<u64, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_time(text),
        Cell::Number(value) => serial::split(value)
            .map(|(_, nanos)| nanos)
            .map_err(spanless),
        Cell::Wall { nanos, .. } | Cell::Clock(nanos) => Ok(nanos),
        Cell::Span(span) => {
            if span.seconds < 0 || span.nanos < 0 {
                return Err(OUT_OF_RANGE);
            }
            let total = span.seconds as u64 * 1_000_000_000 + span.nanos as u64;
            if total >= NANOS_PER_DAY {
                Err(OUT_OF_RANGE)
            } else {
                Ok(total)
            }
        }
        _ => Err(MALFORMED),
    }
}

/// Casts a cell through HyperCast's duration door; a workbook number is a count of days,
/// a time of day is its span since midnight, and a span is itself.
pub fn cast_duration(cell: &Cell<'_>) -> Result<Duration, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_duration(text),
        Cell::Number(value) => serial::days_to_duration(value).map_err(spanless),
        Cell::Clock(nanos) => Ok(Duration {
            seconds: (nanos / 1_000_000_000) as i64,
            nanos: (nanos % 1_000_000_000) as i32,
        }),
        Cell::Span(span) => Ok(span),
        _ => Err(MALFORMED),
    }
}

/// The text door: a text cell's bytes, borrowed and untrimmed (empty bytes are `Empty`);
/// a typed cell's canonical rendering (see [`render`]), owned.
pub fn cast_text<'a>(cell: &Cell<'a>) -> Result<Cow<'a, [u8]>, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(&[]) => Err(EMPTY),
        Cell::Text(text) => Ok(Cow::Borrowed(text)),
        _ => {
            let mut rendered = Vec::new();
            render(cell, &mut rendered);
            Ok(Cow::Owned(rendered))
        }
    }
}

/// A dynamically typed cast result, for callers that dispatch on a [`Column`] at run
/// time rather than calling a door directly.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Uuid([u8; 16]),
    Timestamp(Timestamp),
    Date(Date),
    /// Nanoseconds since midnight.
    Time(u64),
    Duration(Duration),
    Text(Cow<'a, [u8]>),
    Decimal(Decimal),
    DateTime(CivilDateTime),
}

impl Column {
    /// Casts `cell` through this column's door, with its declared numeric notation and
    /// the source's date system.
    pub fn cast<'a>(&self, cell: &Cell<'a>, system: ExcelEpoch) -> Result<Value<'a>, Fault> {
        Ok(match self.door {
            Door::Bool => Value::Bool(cast_bool(cell)?),
            Door::I8 => Value::I8(cast_i8(cell, &self.format)?),
            Door::I16 => Value::I16(cast_i16(cell, &self.format)?),
            Door::I32 => Value::I32(cast_i32(cell, &self.format)?),
            Door::I64 => Value::I64(cast_i64(cell, &self.format)?),
            Door::U8 => Value::U8(cast_u8(cell, &self.format)?),
            Door::U16 => Value::U16(cast_u16(cell, &self.format)?),
            Door::U32 => Value::U32(cast_u32(cell, &self.format)?),
            Door::U64 => Value::U64(cast_u64(cell, &self.format)?),
            Door::F32 => Value::F32(cast_f32(cell, &self.format)?),
            Door::F64 => Value::F64(cast_f64(cell, &self.format)?),
            Door::Uuid => Value::Uuid(cast_uuid(cell)?),
            Door::Timestamp => Value::Timestamp(cast_timestamp(cell, system)?),
            Door::Unix(precision) => Value::Timestamp(cast_unix(cell, precision)?),
            Door::Date => Value::Date(cast_date(cell, system)?),
            Door::Time => Value::Time(cast_time(cell)?),
            Door::Duration => Value::Duration(cast_duration(cell)?),
            Door::Text => Value::Text(cast_text(cell)?),
            Door::Decimal => Value::Decimal(cast_decimal(cell, &self.format)?),
            Door::DateOrdered(order) => Value::Date(cast_date_ordered(cell, order, system)?),
            Door::DateTime(order) => Value::DateTime(cast_datetime(cell, order, system)?),
            Door::ExcelSerial(epoch) => Value::Timestamp(cast_excel_serial(cell, epoch)?),
        })
    }
}
