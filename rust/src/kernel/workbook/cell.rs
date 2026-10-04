//! A workbook cell as it was read, and the cast matrix: every pair of a cell and a door
//! with one defined outcome.
//!
//! A text cell goes through the HyperCast door verbatim, and its fault span is HyperCast's
//! own, into the cell's bytes. A typed cell is converted directly — a workbook's `42.0`
//! never passes through text to become an `i32` — and a typed cell that fails its door
//! is said back as text ([`super::number::Text`]) with the fault spanning all of it.

use super::number::{self, Text, is_integral, magnitude, round_signed, round_unsigned};
use crate::kernel::abi::{CellVerdict, ColumnBuffer, ColumnSpec, Slot, Span};
use crate::kernel::door::Door;
use hypercast::{
    CivilDateTime, Date, Decimal, Duration, ExcelEpoch, Fault, MAX_DURATION_SECONDS,
    MAX_TIMESTAMP_SECONDS, MIN_TIMESTAMP_SECONDS, NumFormat, Reason, Timestamp, UnixPrecision,
    excel_serial,
};

/// Nanoseconds in one day.
pub const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// One cell as its part stored it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell<'a> {
    /// No value: a missing cell, a gap in a sparse row.
    Empty,
    /// UTF-8 bytes, untrimmed — the doors do their own trimming.
    Text(&'a [u8]),
    /// The double a numeric cell with a non-temporal format stores.
    Number(f64),
    /// A boolean cell.
    Bool(bool),
    /// A zoneless wall-clock reading: a date, and nanoseconds since its midnight.
    Wall { date: Date, nanos: u64 },
    /// A time of day with no date.
    Clock(u64),
    /// A signed duration.
    Span(Duration),
    /// A formula error, by its BIFF code.
    Error(u8),
}

/// The spreadsheet error values, as XLSX and ODS spell them, with the BIFF code of each.
const ERRORS: [(&[u8], u8); 8] = [
    (b"#NULL!", 0x00),
    (b"#DIV/0!", 0x07),
    (b"#VALUE!", 0x0F),
    (b"#REF!", 0x17),
    (b"#NAME?", 0x1D),
    (b"#NUM!", 0x24),
    (b"#N/A", 0x2A),
    (b"#GETTING_DATA", 0x2B),
];

/// The code of an error cell's spelling.
pub fn error_code(text: &[u8]) -> Option<u8> {
    ERRORS
        .iter()
        .find(|(spelling, _)| *spelling == text)
        .map(|&(_, code)| code)
}

fn error_text(code: u8) -> &'static [u8] {
    ERRORS
        .iter()
        .find(|&&(_, known)| known == code)
        .map_or(&b""[..], |&(spelling, _)| spelling)
}

impl Slot {
    /// No cell.
    pub const EMPTY: Slot = Slot {
        tag: 0,
        aux: 0,
        bits: 0,
    };
    const TEXT: u32 = 1;
    const NUMBER: u32 = 2;
    const BOOL: u32 = 3;
    const WALL: u32 = 4;
    const CLOCK: u32 = 5;
    const SPAN: u32 = 6;
    const ERROR: u32 = 7;

    /// A text cell, by where its bytes are.
    pub const fn text(span: Span) -> Slot {
        Slot {
            tag: Slot::TEXT,
            aux: 0,
            bits: span.offset as u64 | (span.len as u64) << 32,
        }
    }

    /// A typed cell. (A text cell is stored by its span, not its bytes: [`Slot::text`].)
    pub fn of(cell: &Cell<'_>) -> Slot {
        let (tag, aux, bits) = match *cell {
            Cell::Empty | Cell::Text(_) => (0, 0, 0),
            Cell::Number(value) => (Slot::NUMBER, 0, value.to_bits()),
            Cell::Bool(value) => (Slot::BOOL, 0, u64::from(value)),
            Cell::Wall { date, nanos } => (
                Slot::WALL,
                u32::from(date.year) << 16 | u32::from(date.month) << 8 | u32::from(date.day),
                nanos,
            ),
            Cell::Clock(nanos) => (Slot::CLOCK, 0, nanos),
            Cell::Span(span) => (Slot::SPAN, span.nanos as u32, span.seconds as u64),
            Cell::Error(code) => (Slot::ERROR, 0, u64::from(code)),
        };
        Slot { tag, aux, bits }
    }

    /// True for no cell at all. (A text cell with no bytes is still a cell.)
    pub const fn is_empty(&self) -> bool {
        self.tag == 0 || self.tag > Slot::ERROR
    }

    /// Where a text cell's bytes are.
    pub const fn span(&self) -> Option<Span> {
        if self.tag != Slot::TEXT {
            return None;
        }
        Some(Span {
            offset: self.bits as u32,
            len: (self.bits >> 32) as u32,
        })
    }

    /// The cell, its text read from whichever of the two stores its span names.
    pub fn cell<'a>(&self, arena: &'a [u8], strings: &'a [u8]) -> Cell<'a> {
        match self.tag {
            Slot::TEXT => Cell::Text(resolve(Span::default(), self.span(), arena, strings)),
            Slot::NUMBER => Cell::Number(f64::from_bits(self.bits)),
            Slot::BOOL => Cell::Bool(self.bits != 0),
            Slot::WALL => Cell::Wall {
                date: Date {
                    year: (self.aux >> 16) as u16,
                    month: (self.aux >> 8) as u8,
                    day: self.aux as u8,
                },
                nanos: self.bits,
            },
            Slot::CLOCK => Cell::Clock(self.bits),
            Slot::SPAN => Cell::Span(Duration {
                seconds: self.bits as i64,
                nanos: self.aux as i32,
            }),
            Slot::ERROR => Cell::Error(self.bits as u8),
            _ => Cell::Empty,
        }
    }
}

/// The bytes a span names: in the batch arena when flagged, in the shared strings when not.
pub fn resolve<'a>(
    fallback: Span,
    span: Option<Span>,
    arena: &'a [u8],
    strings: &'a [u8],
) -> &'a [u8] {
    let span = span.unwrap_or(fallback);
    let store = if span.flagged() { arena } else { strings };
    let from = span.offset as usize;
    store
        .get(from..from.saturating_add(span.len()))
        .unwrap_or_default()
}

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

/// Splits a non-negative serial into whole days and the nanoseconds of the fractional
/// day, rounding the fraction to the nearest nanosecond (and carrying into the day if
/// that rounds up to exactly one day).
pub fn split(value: f64) -> Result<(i64, u64), Reason> {
    if !value.is_finite() || value < 0.0 {
        // Excel shows a negative date serial as `####`; NaN and ∞ never come from a file.
        return Err(Reason::Malformed);
    }
    if value >= 9.0e15 {
        // Past any calendar; also keeps the conversion below exact.
        return Err(Reason::OutOfRange);
    }
    let whole = value as u64;
    let mut days = whole as i64;
    let mut nanos = round_unsigned((value - whole as f64) * NANOS_PER_DAY as f64);
    if nanos >= NANOS_PER_DAY {
        days += 1;
        nanos -= NANOS_PER_DAY;
    }
    Ok((days, nanos))
}

/// `value` days as a duration, nanos signed as the seconds are, within HyperCast's
/// ±10,000-year window.
pub fn days_to_duration(value: f64) -> Result<Duration, Reason> {
    if !value.is_finite() {
        return Err(Reason::Malformed);
    }
    let seconds = value * 86_400.0;
    if magnitude(seconds) > MAX_DURATION_SECONDS as f64 {
        return Err(Reason::OutOfRange);
    }
    let total = round_signed(seconds * 1e9);
    Ok(Duration {
        seconds: (total / 1_000_000_000) as i64,
        nanos: (total % 1_000_000_000) as i32,
    })
}

/// A serial read the way its number format declared: a date/time serial is a wall clock,
/// or a time of day when it is under one day and so carries no date; an elapsed serial is
/// a span of that many days, sign and all. `None` where the rules refuse the serial.
pub fn serial(value: f64, system: ExcelEpoch, elapsed: bool) -> Option<Cell<'static>> {
    if elapsed {
        return days_to_duration(value).ok().map(Cell::Span);
    }
    let (days, nanos) = split(value).ok()?;
    if days == 0 {
        return Some(Cell::Clock(nanos));
    }
    let wall = excel_serial(value, system).ok()?;
    Some(Cell::Wall {
        date: wall.date,
        nanos: wall.nanos_of_day,
    })
}

/// A wall-clock cell from ISO 8601 text as XLSX `t="d"` cells and ODS `office:date-value`
/// carry it: `yyyy-MM-dd`, `yyyy-MM-ddTHH:mm[:ss[.f…]]`, and the same with a zone (an
/// instant, read as its UTC wall clock). `None` if the text is none of them.
pub fn wall_from_iso(text: &[u8]) -> Option<Cell<'static>> {
    let text = text.trim_ascii();
    if let Ok(date) = hypercast::cast_date(text) {
        return Some(Cell::Wall { date, nanos: 0 });
    }
    let (day, rest) = text.split_at_checked(10)?;
    let (&separator, clock) = rest.split_first()?;
    if !matches!(separator, b'T' | b't' | b' ') {
        return None;
    }
    let date = hypercast::cast_date(day).ok()?;
    let zulu = matches!(clock.last(), Some(b'Z' | b'z'));
    let offset = clock
        .iter()
        .skip(1)
        .position(|&b| b == b'+' || b == b'-')
        .map(|at| at + 1);
    if !zulu && offset.is_none() {
        let nanos = hypercast::cast_time(clock).ok()?;
        return Some(Cell::Wall { date, nanos });
    }
    // An instant. HyperCast's RFC 3339 door wants the `T` and the seconds; ISO 29500's own
    // example (`1976-11-22T08:30Z`) has neither guarantee. No timestamp is this long.
    let zone_at = if zulu { clock.len() - 1 } else { offset? };
    let (time, zone) = clock.split_at_checked(zone_at)?;
    let mut canonical = [0u8; 64];
    let mut len = 0usize;
    let pieces: [&[u8]; 5] = [
        day,
        b"T",
        time,
        if zone_at == 5 { b":00" } else { b"" },
        zone,
    ];
    for piece in pieces {
        canonical
            .get_mut(len..len + piece.len())?
            .copy_from_slice(piece);
        len += piece.len();
    }
    let wall = hypercast::cast_timestamp(canonical.get(..len)?)
        .ok()?
        .utc_civil()?;
    Some(Cell::Wall {
        date: wall.date,
        nanos: wall.nanos_of_day,
    })
}

/// A finite, integral double as any integer type: non-integral is `Malformed`, integral
/// but unrepresentable is `OutOfRange`.
fn number_to_int<T: TryFrom<i128>>(value: f64) -> Result<T, Fault> {
    if !is_integral(value) {
        return Err(MALFORMED);
    }
    // Every integral double below 2^127 in magnitude converts to i128 exactly.
    if magnitude(value) >= 1.701_411_834_604_692_3e38 {
        return Err(OUT_OF_RANGE);
    }
    T::try_from(number::integral(value)).map_err(|_| OUT_OF_RANGE)
}

fn integer<T: TryFrom<i128>>(
    cell: &Cell<'_>,
    door: impl FnOnce(&[u8]) -> Result<T, Fault>,
) -> Result<T, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => door(text),
        Cell::Number(value) => number_to_int(value),
        _ => Err(MALFORMED),
    }
}

fn wall_to_timestamp(date: Date, nanos: u64) -> Timestamp {
    CivilDateTime {
        date,
        nanos_of_day: nanos,
    }
    .assume_utc()
}

fn timestamp(cell: &Cell<'_>, system: ExcelEpoch) -> Result<Timestamp, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_timestamp(text),
        Cell::Number(value) => excel_serial(value, system)
            .map(CivilDateTime::assume_utc)
            .map_err(spanless),
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
    }
}

fn unix(cell: &Cell<'_>, precision: UnixPrecision) -> Result<Timestamp, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_unix(text, precision),
        Cell::Number(value) => {
            let ticks: i128 = number_to_int(value)?;
            // Nanos count forward from `seconds`, even before the epoch (protobuf).
            let ((seconds, nanos), nanos_per_tick) = match precision {
                UnixPrecision::Seconds => ((ticks, 0), 1),
                UnixPrecision::Millis => (floor_div::<1_000>(ticks), 1_000_000),
                UnixPrecision::Micros => (floor_div::<1_000_000>(ticks), 1_000),
                UnixPrecision::Nanos => (floor_div::<1_000_000_000>(ticks), 1),
            };
            if seconds < i128::from(MIN_TIMESTAMP_SECONDS)
                || seconds > i128::from(MAX_TIMESTAMP_SECONDS)
            {
                return Err(OUT_OF_RANGE);
            }
            Ok(Timestamp {
                seconds: seconds as i64,
                nanos: (nanos * nanos_per_tick) as i32,
            })
        }
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
    }
}

/// Floored quotient and non-negative remainder by a positive constant.
#[inline(always)]
fn floor_div<const BY: i128>(value: i128) -> (i128, i128) {
    let (quotient, remainder) = (value / BY, value % BY);
    if remainder < 0 {
        (quotient - 1, remainder + BY)
    } else {
        (quotient, remainder)
    }
}

fn date(cell: &Cell<'_>, system: ExcelEpoch) -> Result<Date, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_date(text),
        Cell::Number(value) => excel_serial(value, system)
            .map(|wall| wall.date)
            .map_err(spanless),
        Cell::Wall { date, .. } => Ok(date),
        _ => Err(MALFORMED),
    }
}

fn time(cell: &Cell<'_>) -> Result<u64, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_time(text),
        Cell::Number(value) => split(value).map(|(_, nanos)| nanos).map_err(spanless),
        Cell::Wall { nanos, .. } | Cell::Clock(nanos) => Ok(nanos),
        Cell::Span(span) => {
            if span.seconds < 0 || span.nanos < 0 {
                return Err(OUT_OF_RANGE);
            }
            let total = (span.seconds as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(span.nanos as u64);
            if total >= NANOS_PER_DAY {
                Err(OUT_OF_RANGE)
            } else {
                Ok(total)
            }
        }
        _ => Err(MALFORMED),
    }
}

fn duration(cell: &Cell<'_>) -> Result<Duration, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => hypercast::cast_duration(text),
        Cell::Number(value) => days_to_duration(value).map_err(spanless),
        Cell::Clock(nanos) => Ok(Duration {
            seconds: (nanos / 1_000_000_000) as i64,
            nanos: (nanos % 1_000_000_000) as i32,
        }),
        Cell::Span(span) => Ok(span),
        _ => Err(MALFORMED),
    }
}

/// The canonical text of a typed cell; nothing for an empty or a text cell.
pub fn render(cell: &Cell<'_>) -> Text {
    let mut out = Text::new();
    match *cell {
        Cell::Empty | Cell::Text(_) => {}
        Cell::Number(value) => out.number(value),
        Cell::Bool(value) => out.boolean(value),
        Cell::Wall { date, nanos } => out.wall(date, nanos),
        Cell::Clock(nanos) => out.clock(nanos),
        Cell::Span(span) => out.span(span),
        Cell::Error(code) => out.literal(error_text(code)),
    }
    out
}

/// Appends `bytes` to the arena and says where, or how large the arena would have to be
/// (`u64::MAX` when no size would do).
pub fn append(arena: &mut [u8], used: &mut usize, bytes: &[u8]) -> Result<Span, u64> {
    // More than a span can address, and no arena is large enough.
    let end = match used.checked_add(bytes.len()) {
        Some(end) if end as u64 <= u64::from(u32::MAX) => end,
        _ => return Err(u64::MAX),
    };
    let Some(room) = arena.get_mut(*used..*used + bytes.len()) else {
        return Err(end as u64);
    };
    room.copy_from_slice(bytes);
    let span = Span {
        offset: *used as u32,
        len: bytes.len() as u32 | Span::FLAG,
    };
    *used = end;
    Ok(span)
}

/// Writes one value where the caller pointed, or the door's zero if it did not cast.
///
/// # Safety
/// `buffer.values` has room for `row + 1` values of `T`.
#[inline(always)]
unsafe fn put<T: Copy>(
    buffer: &ColumnBuffer,
    row: usize,
    result: Result<T, Fault>,
    zero: T,
) -> Option<Fault> {
    let (value, fault) = match result {
        Ok(value) => (value, None),
        Err(fault) => (zero, Some(fault)),
    };
    // SAFETY: per the function contract; the array need not be aligned.
    unsafe { buffer.values.cast::<T>().add(row).write_unaligned(value) };
    fault
}

/// The memory a cast reads and writes besides the column itself.
pub struct Stores<'a> {
    /// The batch arena, where rendered text goes.
    pub arena: &'a mut [u8],
    /// How much of it is used.
    pub used: &'a mut usize,
    /// The workbook's shared strings.
    pub strings: &'a [u8],
}

/// Casts the cell in `slot` through `spec`'s door, writing the value and the verdict at
/// `row`. Returns the span of the cell's text — its own bytes for a text cell, its
/// rendering for a typed cell that failed its door or went through the text door, and
/// nothing otherwise — or, if rendering it did not fit the arena, the size that would.
///
/// # Safety
/// `buffer.values` has room for `row + 1` values of the door's type, and
/// `buffer.verdicts` for `row + 1` verdicts.
pub unsafe fn cast(
    slot: &Slot,
    door: Door,
    spec: &ColumnSpec,
    system: ExcelEpoch,
    buffer: &ColumnBuffer,
    row: usize,
    stores: &mut Stores<'_>,
) -> Result<Span, u64> {
    const INSTANT: Timestamp = Timestamp {
        seconds: 0,
        nanos: 0,
    };
    const DAY: Date = Date {
        year: 0,
        month: 0,
        day: 0,
    };
    let own = slot.span();
    let mut raw = own.unwrap_or_default();
    let cell = slot.cell(stores.arena, stores.strings);
    let cell = &cell;
    // Only a text cell meets a declared notation, so only then is it worked out.
    let format = || spec.num_format().unwrap_or(NumFormat::INVARIANT);
    // SAFETY: the caller's contract, one value at a time.
    let fault = unsafe {
        match door {
            Door::Bool => {
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_bool(text),
                    Cell::Bool(value) => Ok(value),
                    // A workbook number is accepted only as exactly `0` or `1`.
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
                };
                put(buffer, row, result.map(u8::from), 0u8)
            }
            Door::I8 => {
                let result = integer(cell, |text| hypercast::cast_i8(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::I16 => {
                let result = integer(cell, |text| hypercast::cast_i16(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::I32 => {
                let result = integer(cell, |text| hypercast::cast_i32(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::I64 => {
                let result = integer(cell, |text| hypercast::cast_i64(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::U8 => {
                let result = integer(cell, |text| hypercast::cast_u8(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::U16 => {
                let result = integer(cell, |text| hypercast::cast_u16(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::U32 => {
                let result = integer(cell, |text| hypercast::cast_u32(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::U64 => {
                let result = integer(cell, |text| hypercast::cast_u64(text, &format()));
                put(buffer, row, result, 0)
            }
            Door::F32 => {
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_f32(text, &format()),
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
                };
                put(buffer, row, result, 0.0)
            }
            Door::F64 => {
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_f64(text, &format()),
                    Cell::Number(value) if value.is_nan() => Err(MALFORMED),
                    Cell::Number(value) if value.is_infinite() => Err(OUT_OF_RANGE),
                    Cell::Number(value) => Ok(value),
                    _ => Err(MALFORMED),
                };
                put(buffer, row, result, 0.0)
            }
            Door::Decimal => {
                // Only text can be one: a workbook number is a binary double, and which
                // decimal it "meant" is not something the file says.
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_decimal(text, &format()),
                    _ => Err(MALFORMED),
                };
                let zero = Decimal {
                    lo: 0,
                    hi: 0,
                    scale: 0,
                    negative: false,
                };
                put(buffer, row, result, zero)
            }
            Door::Uuid => {
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_uuid(text),
                    _ => Err(MALFORMED),
                };
                put(buffer, row, result, [0u8; 16])
            }
            Door::Timestamp => put(buffer, row, timestamp(cell, system), INSTANT),
            Door::Unix(precision) => put(buffer, row, unix(cell, precision), INSTANT),
            Door::ExcelSerial(epoch) => {
                // The *declared* date system, for serials the file did not format as dates.
                let result = match *cell {
                    Cell::Text(text) => hypercast::cast_excel_serial(text, epoch),
                    _ => timestamp(cell, epoch),
                };
                put(buffer, row, result, INSTANT)
            }
            Door::Date => put(buffer, row, date(cell, system), DAY),
            Door::DateOrdered(order) => {
                // A stored date has no field order to declare.
                let result = match *cell {
                    Cell::Text(text) => hypercast::cast_date_ordered(text, order),
                    _ => date(cell, system),
                };
                put(buffer, row, result, DAY)
            }
            Door::DateTime(order) => {
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    Cell::Text(text) => hypercast::cast_datetime(text, order),
                    Cell::Number(value) => excel_serial(value, system).map_err(spanless),
                    Cell::Wall { date, nanos } => Ok(CivilDateTime {
                        date,
                        nanos_of_day: nanos,
                    }),
                    _ => Err(MALFORMED),
                };
                let zero = CivilDateTime {
                    date: DAY,
                    nanos_of_day: 0,
                };
                put(buffer, row, result, zero)
            }
            Door::Time => put(buffer, row, time(cell), 0u64),
            Door::Duration => {
                let zero = Duration {
                    seconds: 0,
                    nanos: 0,
                };
                put(buffer, row, duration(cell), zero)
            }
            Door::Text => {
                // The bytes themselves, untrimmed, or a typed cell said as text; no bytes
                // at all is the one way to fail.
                let result = match *cell {
                    Cell::Empty | Cell::Text(&[]) => Err(EMPTY),
                    Cell::Text(_) => Ok(raw),
                    _ => {
                        raw = append(
                            stores.arena,
                            stores.used,
                            render(&slot.cell(&[], &[])).as_bytes(),
                        )?;
                        Ok(raw)
                    }
                };
                put(buffer, row, result, Span::default())
            }
        }
    };
    let verdict = match fault {
        None => CellVerdict::OK,
        Some(fault) if fault.reason == Reason::Empty || own.is_some() => {
            CellVerdict::from_fault(fault)
        }
        Some(fault) => {
            // A typed cell's fault spans its whole rendering.
            raw = append(
                stores.arena,
                stores.used,
                render(&slot.cell(&[], &[])).as_bytes(),
            )?;
            let mut verdict = CellVerdict::from_fault(fault);
            verdict.len = raw.len() as u32;
            verdict
        }
    };
    // SAFETY: per the function contract; the array need not be aligned.
    unsafe { buffer.verdicts.add(row).write_unaligned(verdict) };
    Ok(raw)
}

/// The door's zero and the verdict of no cell at all: what every column of an empty row
/// holds.
///
/// # Safety
/// As [`cast`].
pub unsafe fn cast_empty(door: Door, buffer: &ColumnBuffer, row: usize) {
    let mut used = 0usize;
    let mut stores = Stores {
        arena: &mut [],
        used: &mut used,
        strings: &[],
    };
    let spec = ColumnSpec::new(0, door.code(), 0);
    // SAFETY: the caller's contract; an empty cell renders nothing, so the arena is not asked for.
    let _ = unsafe {
        cast(
            &Slot::EMPTY,
            door,
            &spec,
            ExcelEpoch::Y1900,
            buffer,
            row,
            &mut stores,
        )
    };
}

/// Re-exported for the row readers, which parse numbers and dates out of attribute text.
pub use number::parse_f64;
