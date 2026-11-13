//! A workbook cell as it was read, and the cast matrix: every pair of a cell and a door
//! with one defined outcome.
//!
//! A text cell goes through the HyperCast door verbatim, and its fault span is HyperCast's
//! own, into the cell's bytes. A typed cell is converted directly — a workbook's `42.0`
//! never passes through text to become an `i32` — by HyperCast's typed twin of the same
//! door (`i32_from_f64`, `decimal_from_f64`, `excel_time`, …), and a typed cell that fails
//! its door is said back as text ([`super::number::Text`]) with the fault spanning all of it.

use super::number::{self, Text};
use super::styles;
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith, Slot, Span,
};
use crate::kernel::checked::{Checks, resolved_constraints, text_checked};
use crate::kernel::delimited::fill::{is_token, resolved_date, resolved_format, resolved_tokens};
use crate::kernel::door::Door;
use hypercast::detailed::Rejection;
use hypercast::{
    Checked, CivilDateTime, Constraints, Date, Decimal, Detail, Duration, ExcelEpoch, Fault,
    NumFormat, Numeric, RawCheck, Reason, Timestamp, UnixPrecision, ValidDateOptions,
    ValidNumFormat, detailed, excel_duration, excel_serial, excel_time,
};

/// Nanoseconds in one day.
pub const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// One cell as its part stored it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell<'a> {
    /// No value: a missing cell, a gap in a sparse row.
    Empty,
    /// The part's bytes, untrimmed — the doors do their own trimming, and judge whether
    /// they are UTF-8.
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
    /// A formula with no cached result: read as [`Cell::Empty`], with
    /// [`CellNote::ORIGIN_UNCACHED`] where a note can say so.
    Uncached,
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
    const UNCACHED: u32 = 8;
    /// `aux` of a text cell of no bytes that's really a formula with no result.
    const NO_RESULT: u32 = 1;
    /// `aux` of a text cell whose bytes are a shared string that was found to be UTF-8 when the
    /// strings were loaded, so the text door has nothing left to ask of them but whether there
    /// are any.
    const VOUCHED: u32 = 2;

    /// A text cell, by where its bytes are.
    pub const fn text(span: Span) -> Slot {
        Slot {
            tag: Slot::TEXT,
            aux: 0,
            bits: span.offset as u64 | (span.len as u64) << 32,
        }
    }

    /// A text cell whose bytes are a shared string, by where they are and whether the load
    /// vouched for them ([`book::strings`](super::book::strings)).
    pub(crate) const fn shared(span: Span, vouched: bool) -> Slot {
        Slot {
            aux: if vouched { Slot::VOUCHED } else { 0 },
            ..Slot::text(span)
        }
    }

    /// True for a text cell the strings' load vouched for.
    pub(crate) const fn is_vouched(&self) -> bool {
        self.tag == Slot::TEXT && self.aux == Slot::VOUCHED
    }

    /// This text cell with its bytes moved to `span`, whatever else it is.
    pub(crate) const fn moved(self, span: Span) -> Slot {
        Slot {
            aux: self.aux,
            ..Slot::text(span)
        }
    }

    /// This slot, for a formula it turns out has no result. 1.0 read such a cell one of two ways:
    /// as no cell at all, or, where the file spells a value and it's empty, as a text cell of no
    /// bytes, which unlike the other makes its row one that holds something. So the second stays
    /// that to everything but `cast_with`, and the rows a sheet delivers don't move.
    pub(crate) const fn with_no_result(self) -> Slot {
        if self.tag == Slot::TEXT {
            Slot {
                aux: Slot::NO_RESULT,
                ..self
            }
        } else {
            Slot {
                tag: Slot::UNCACHED,
                aux: 0,
                bits: 0,
            }
        }
    }

    /// True for a formula with no result.
    pub(crate) const fn is_uncached(&self) -> bool {
        self.tag == Slot::UNCACHED || (self.tag == Slot::TEXT && self.aux == Slot::NO_RESULT)
    }

    /// A typed cell. (A text cell is stored by its span, not its bytes: [`Slot::text`].)
    pub fn of(cell: &Cell<'_>) -> Slot {
        let (tag, aux, bits) = match *cell {
            Cell::Empty | Cell::Text(_) => (0, 0, 0),
            Cell::Uncached => (Slot::UNCACHED, 0, 0),
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

    /// True for no cell at all. (A text cell with no bytes is still a cell, and a formula with no
    /// result, which holds nothing, is none.)
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
            Slot::UNCACHED => Cell::Uncached,
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

/// A serial read the way its number format declared (`kind`, a [`styles`] constant): an
/// elapsed serial is a span of that many days, sign and all; any other is a wall clock, or a
/// time of day when it carries no date. A serial under one day carries none in the 1900
/// system — its serial `0` is the `1900-01-00` that never was — nor under a time format;
/// in the 1904 system a date format's serial `0` is 1904-01-01, and a date it is. `None`
/// where the rules refuse the serial.
///
/// [`styles`]: super::styles
pub fn serial(value: f64, system: ExcelEpoch, kind: u8) -> Option<Cell<'static>> {
    if kind == styles::ELAPSED {
        return excel_duration(value).ok().map(Cell::Span);
    }
    let dated = kind == styles::DATE_TIME && matches!(system, ExcelEpoch::Y1904);
    // Under one day once snapped, as `excel_time` snaps: a candidate it picks lies inside the
    // double's midpoints, so under a day; only its nearest-nanosecond fallback carries what
    // is left short of half a nanosecond under a day into the next one, which has a date.
    // Decided before either door runs, so a serial is converted once: everything
    // `excel_time` refuses at a day or more, `excel_serial` refuses too (NaN included, which
    // fails the comparison).
    if !dated && value * (NANOS_PER_DAY as f64) < NANOS_PER_DAY as f64 - 0.5 {
        return excel_time(value).ok().map(Cell::Clock);
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

/// A cell through one of the numeric doors: the text door for text, its typed twin for a
/// number, and nothing else.
fn numeric<T>(
    cell: &Cell<'_>,
    door: impl FnOnce(&[u8]) -> Result<T, Fault>,
    typed: impl FnOnce(f64) -> Result<T, Reason>,
) -> Result<T, Fault> {
    match *cell {
        Cell::Empty => Err(EMPTY),
        Cell::Text(text) => door(text),
        Cell::Number(value) => typed(value).map_err(spanless),
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
        Cell::Number(value) => hypercast::unix_from_f64(value, precision).map_err(spanless),
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(MALFORMED),
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
        Cell::Number(value) => excel_time(value).map_err(spanless),
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
        Cell::Number(value) => excel_duration(value).map_err(spanless),
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
        Cell::Empty | Cell::Text(_) | Cell::Uncached => {}
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
#[allow(clippy::too_many_arguments)]
pub unsafe fn cast(
    slot: &Slot,
    door: Door,
    spec: &ColumnSpec,
    read_format: fn(&ColumnSpec) -> Option<NumFormat>,
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
    let cell = match slot.cell(stores.arena, stores.strings) {
        Cell::Uncached => Cell::Empty,
        cell => cell,
    };
    let cell = &cell;
    // Only a text cell meets a declared notation, so only then is it worked out.
    let format = || read_format(spec).unwrap_or(NumFormat::INVARIANT);
    // SAFETY: the caller's contract, one value at a time.
    let fault = unsafe {
        match door {
            Door::Bool => {
                let result = match *cell {
                    Cell::Bool(value) => Ok(value),
                    _ => numeric(
                        cell,
                        |text| hypercast::cast_bool(text),
                        hypercast::bool_from_f64,
                    ),
                };
                put(buffer, row, result.map(u8::from), 0u8)
            }
            Door::I8 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_i8(text, &format()),
                    hypercast::i8_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::I16 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_i16(text, &format()),
                    hypercast::i16_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::I32 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_i32(text, &format()),
                    hypercast::i32_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::I64 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_i64(text, &format()),
                    hypercast::i64_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::U8 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_u8(text, &format()),
                    hypercast::u8_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::U16 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_u16(text, &format()),
                    hypercast::u16_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::U32 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_u32(text, &format()),
                    hypercast::u32_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::U64 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_u64(text, &format()),
                    hypercast::u64_from_f64,
                );
                put(buffer, row, result, 0)
            }
            Door::F32 => {
                let result = numeric(
                    cell,
                    |text| hypercast::cast_f32(text, &format()),
                    hypercast::f32_from_f64,
                );
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
                // A workbook number is the shortest decimal that names its double: the
                // digits the writer put in the file, whichever writer it was.
                let result = numeric(
                    cell,
                    |text| hypercast::cast_decimal(text, &format()),
                    hypercast::decimal_from_f64,
                );
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
                // The bytes themselves, untrimmed, once HyperCast's door has judged them, or
                // a typed cell said as text.
                let result = match *cell {
                    Cell::Empty => Err(EMPTY),
                    // A shared string was judged once, when the strings were loaded.
                    Cell::Text(text) if slot.is_vouched() && !text.is_empty() => Ok(raw),
                    Cell::Text(text) => hypercast::cast_text(text).map(|_| raw),
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

/// Why a cell has no value, with what a note says of it: [`cast`]'s [`Fault`], and HyperCast's
/// detail where one of its doors was asked.
#[derive(Clone, Copy)]
struct Refused {
    fault: Fault,
    detail: u16,
}

impl From<hypercast::Failure> for Refused {
    #[inline]
    fn from(failure: hypercast::Failure) -> Refused {
        Refused {
            fault: failure.fault(),
            detail: failure.detail.code() as u16,
        }
    }
}

/// What a blank cell is told, as the doors tell blank text.
const BLANK: Refused = Refused {
    fault: EMPTY,
    detail: Detail::Blank.code() as u16,
};
/// What a typed cell is told by a door with no twin for its kind. No door was asked, so there is
/// no detail to report.
const WRONG_KIND: Refused = Refused {
    fault: MALFORMED,
    detail: 0,
};

fn refused(fault: Fault, detail: Detail) -> Refused {
    Refused {
        fault,
        detail: detail.code() as u16,
    }
}

/// [`numeric`], through the detailed doors.
#[inline(always)]
fn numeric_with<T>(
    cell: &Cell<'_>,
    door: impl FnOnce(&[u8]) -> Result<T, hypercast::Failure>,
    typed: impl FnOnce(f64) -> Result<T, hypercast::Failure>,
) -> Result<T, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => door(text).map_err(Refused::from),
        Cell::Number(value) => typed(value).map_err(Refused::from),
        _ => Err(WRONG_KIND),
    }
}

/// [`timestamp`], through the detailed doors.
fn timestamp_with<'o>(
    cell: &Cell<'_>,
    system: ExcelEpoch,
    options: impl FnOnce() -> Option<ValidDateOptions<'o>>,
) -> Result<Timestamp, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => {
            detailed::cast_timestamp(text, options().as_ref()).map_err(Refused::from)
        }
        Cell::Number(value) => detailed::excel_serial(value, system)
            .map(CivilDateTime::assume_utc)
            .map_err(Refused::from),
        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
        _ => Err(WRONG_KIND),
    }
}

/// [`date`], through the detailed doors.
fn date_with<'o>(
    cell: &Cell<'_>,
    system: ExcelEpoch,
    options: impl FnOnce() -> Option<ValidDateOptions<'o>>,
) -> Result<Date, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => detailed::cast_date(text, options().as_ref()).map_err(Refused::from),
        Cell::Number(value) => detailed::excel_serial(value, system)
            .map(|wall| wall.date)
            .map_err(Refused::from),
        Cell::Wall { date, .. } => Ok(date),
        _ => Err(WRONG_KIND),
    }
}

/// [`time`], through the detailed doors.
fn time_with(cell: &Cell<'_>) -> Result<u64, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => detailed::cast_time(text).map_err(Refused::from),
        Cell::Number(value) => detailed::excel_time(value).map_err(Refused::from),
        Cell::Wall { nanos, .. } | Cell::Clock(nanos) => Ok(nanos),
        Cell::Span(span) => {
            if span.seconds < 0 || span.nanos < 0 {
                return Err(refused(OUT_OF_RANGE, Detail::NegativeNotAllowed));
            }
            let total = (span.seconds as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(span.nanos as u64);
            if total >= NANOS_PER_DAY {
                Err(refused(OUT_OF_RANGE, Detail::TooLarge))
            } else {
                Ok(total)
            }
        }
        _ => Err(WRONG_KIND),
    }
}

/// [`duration`], through the detailed doors.
fn duration_with(cell: &Cell<'_>) -> Result<Duration, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => detailed::cast_duration(text).map_err(Refused::from),
        Cell::Number(value) => detailed::excel_duration(value).map_err(Refused::from),
        Cell::Clock(nanos) => Ok(Duration {
            seconds: (nanos / 1_000_000_000) as i64,
            nanos: (nanos % 1_000_000_000) as i32,
        }),
        Cell::Span(span) => Ok(span),
        _ => Err(WRONG_KIND),
    }
}

/// Writes one value where the caller pointed, or the door's zero if it did not cast.
///
/// # Safety
/// `buffer.values` has room for `row + 1` values of `T`.
#[inline(always)]
unsafe fn put_with<T: Copy>(
    buffer: &ColumnBufferWith,
    row: usize,
    result: Result<T, Refused>,
    zero: T,
) -> Option<Refused> {
    let (value, fault) = match result {
        Ok(value) => (value, None),
        Err(fault) => (zero, Some(fault)),
    };
    // SAFETY: per the function contract; the array need not be aligned.
    unsafe { buffer.values.cast::<T>().add(row).write_unaligned(value) };
    fault
}

/// [`cast`], with what [`ColumnSpecWith`] declares and a note beside the verdict: the same
/// verdicts and values, from HyperCast's detailed doors and their typed twins, except that
/// a text cell whose exact bytes are one of the column's `tokens` is `Empty` at
/// [`CellNote::ORIGIN_TOKEN`], a cell the row doesn't have (`absent`) at
/// [`CellNote::ORIGIN_PADDED`], its slot being empty, and a formula with no result at
/// [`CellNote::ORIGIN_UNCACHED`], which no token can match.
///
/// The spec was judged when the fill began. It's read again here, a text cell at a time, since a
/// row-wise fill has nowhere of its own to keep what a judging found.
///
/// # Safety
/// As [`cast`], `buffer.notes` too unless it's null, and `spec`'s pointers as
/// [`ColumnSpecWith`] says.
#[allow(clippy::too_many_arguments)]
pub unsafe fn cast_with(
    slot: &Slot,
    absent: bool,
    door: Door,
    spec: &ColumnSpecWith,
    inherited: &[u8],
    system: ExcelEpoch,
    buffer: &ColumnBufferWith,
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
    let uncached = slot.is_uncached();
    let own = slot.span();
    let mut raw = own.unwrap_or_default();
    let mut cell = if uncached {
        Cell::Empty
    } else {
        slot.cell(stores.arena, stores.strings)
    };
    let mut origin = if uncached {
        CellNote::ORIGIN_UNCACHED
    } else if absent {
        CellNote::ORIGIN_PADDED
    } else {
        CellNote::ORIGIN_CELL
    };
    if let Cell::Text(text) = cell {
        // SAFETY: the caller's contract; the list was judged when the fill began.
        let tokens = unsafe { resolved_tokens(spec, inherited) }.unwrap_or_default();
        if !tokens.is_empty() && is_token(tokens, text) {
            cell = Cell::Empty;
            origin = CellNote::ORIGIN_TOKEN;
        }
    }
    let cell = &cell;
    // Only a text cell meets a declared notation or date options, so only then are they worked
    // out.
    let format = || resolved_format(spec).unwrap_or(ValidNumFormat::INVARIANT);
    // SAFETY: the caller's contract.
    let options = || unsafe { resolved_date(spec) }.flatten();
    let blank = matches!(cell, Cell::Empty);
    let checks = if spec.constraints.is_null() {
        None
    } else {
        // SAFETY: the caller's contract; the declaration was judged when the fill began.
        unsafe { resolved_constraints(spec, door) }.ok().flatten()
    };
    let mut check = None;
    let mut broken = None;
    let fault = if let Some(checks) = checks {
        // SAFETY: the caller's contract, `buffer.checks` judged when the fill began.
        match unsafe { checked_cell(slot, blank, &mut raw, checks, &format, buffer, row, stores) }?
        {
            Outcome::Passed(passed) => {
                check = Some(passed);
                None
            }
            Outcome::Broke(verdict, report) => {
                check = Some(report);
                broken = Some(verdict);
                None
            }
            Outcome::Refused(refusal) => Some(refusal),
        }
    } else {
        // SAFETY: the caller's contract, one value at a time.
        unsafe {
            match door {
                Door::Bool => {
                    let result = match *cell {
                        Cell::Bool(value) => Ok(value),
                        _ => numeric_with(
                            cell,
                            |text| detailed::cast_bool(text),
                            detailed::bool_from_f64,
                        ),
                    };
                    put_with(buffer, row, result.map(u8::from), 0u8)
                }
                Door::I8 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_i8(text, &format()),
                        detailed::i8_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::I16 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_i16(text, &format()),
                        detailed::i16_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::I32 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_i32(text, &format()),
                        detailed::i32_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::I64 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_i64(text, &format()),
                        detailed::i64_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::U8 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_u8(text, &format()),
                        detailed::u8_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::U16 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_u16(text, &format()),
                        detailed::u16_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::U32 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_u32(text, &format()),
                        detailed::u32_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::U64 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_u64(text, &format()),
                        detailed::u64_from_f64,
                    );
                    put_with(buffer, row, result, 0)
                }
                Door::F32 => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_f32(text, &format()),
                        detailed::f32_from_f64,
                    );
                    put_with(buffer, row, result, 0.0)
                }
                Door::F64 => {
                    let result = match *cell {
                        Cell::Empty => Err(BLANK),
                        Cell::Text(text) => {
                            detailed::cast_f64(text, &format()).map_err(Refused::from)
                        }
                        Cell::Number(value) if value.is_nan() => {
                            Err(refused(MALFORMED, Detail::NotFinite))
                        }
                        Cell::Number(value) if value.is_infinite() => {
                            Err(refused(OUT_OF_RANGE, Detail::NotFinite))
                        }
                        Cell::Number(value) => Ok(value),
                        _ => Err(WRONG_KIND),
                    };
                    put_with(buffer, row, result, 0.0)
                }
                Door::Decimal => {
                    let result = numeric_with(
                        cell,
                        |text| detailed::cast_decimal(text, &format()),
                        detailed::decimal_from_f64,
                    );
                    let zero = Decimal {
                        lo: 0,
                        hi: 0,
                        scale: 0,
                        negative: false,
                    };
                    put_with(buffer, row, result, zero)
                }
                Door::Uuid => {
                    let result = match *cell {
                        Cell::Empty => Err(BLANK),
                        Cell::Text(text) => detailed::cast_uuid(text).map_err(Refused::from),
                        _ => Err(WRONG_KIND),
                    };
                    put_with(buffer, row, result, [0u8; 16])
                }
                Door::Timestamp => {
                    let result = timestamp_with(cell, system, options);
                    put_with(buffer, row, result, INSTANT)
                }
                Door::Unix(precision) => {
                    let result = match *cell {
                        Cell::Empty => Err(BLANK),
                        Cell::Text(text) => {
                            detailed::cast_unix(text, precision).map_err(Refused::from)
                        }
                        Cell::Number(value) => {
                            detailed::unix_from_f64(value, precision).map_err(Refused::from)
                        }
                        Cell::Wall { date, nanos } => Ok(wall_to_timestamp(date, nanos)),
                        _ => Err(WRONG_KIND),
                    };
                    put_with(buffer, row, result, INSTANT)
                }
                Door::ExcelSerial(epoch) => {
                    // The *declared* date system, for serials the file did not format as dates.
                    let result = match *cell {
                        Cell::Text(text) => {
                            detailed::cast_excel_serial(text, epoch).map_err(Refused::from)
                        }
                        _ => timestamp_with(cell, epoch, || None),
                    };
                    put_with(buffer, row, result, INSTANT)
                }
                Door::Date => {
                    let result = date_with(cell, system, options);
                    put_with(buffer, row, result, DAY)
                }
                Door::DateOrdered(order) => {
                    // A stored date has no field order to declare.
                    let result = match *cell {
                        Cell::Text(text) => {
                            detailed::cast_date_ordered(text, order, options().as_ref())
                                .map_err(Refused::from)
                        }
                        _ => date_with(cell, system, || None),
                    };
                    put_with(buffer, row, result, DAY)
                }
                Door::DateTime(order) => {
                    let result = match *cell {
                        Cell::Empty => Err(BLANK),
                        Cell::Text(text) => {
                            detailed::cast_datetime(text, order, options().as_ref())
                                .map_err(Refused::from)
                        }
                        Cell::Number(value) => {
                            detailed::excel_serial(value, system).map_err(Refused::from)
                        }
                        Cell::Wall { date, nanos } => Ok(CivilDateTime {
                            date,
                            nanos_of_day: nanos,
                        }),
                        _ => Err(WRONG_KIND),
                    };
                    let zero = CivilDateTime {
                        date: DAY,
                        nanos_of_day: 0,
                    };
                    put_with(buffer, row, result, zero)
                }
                Door::Time => put_with(buffer, row, time_with(cell), 0u64),
                Door::Duration => {
                    let zero = Duration {
                        seconds: 0,
                        nanos: 0,
                    };
                    put_with(buffer, row, duration_with(cell), zero)
                }
                Door::Text => {
                    let result = match *cell {
                        Cell::Empty => Err(BLANK),
                        Cell::Text(text) if slot.is_vouched() && !text.is_empty() => Ok(raw),
                        Cell::Text(text) => detailed::cast_text(text)
                            .map(|_| raw)
                            .map_err(Refused::from),
                        _ => {
                            raw = append(
                                stores.arena,
                                stores.used,
                                render(&slot.cell(&[], &[])).as_bytes(),
                            )?;
                            Ok(raw)
                        }
                    };
                    put_with(buffer, row, result, Span::default())
                }
            }
        }
    };
    let (verdict, detail) = match fault {
        None => (broken.unwrap_or(CellVerdict::OK), 0),
        Some(refusal) if refusal.fault.reason == Reason::Empty || own.is_some() => {
            (CellVerdict::from_fault(refusal.fault), refusal.detail)
        }
        Some(refusal) => {
            // A typed cell's fault spans its whole rendering.
            raw = append(
                stores.arena,
                stores.used,
                render(&slot.cell(&[], &[])).as_bytes(),
            )?;
            let mut verdict = CellVerdict::from_fault(refusal.fault);
            verdict.len = raw.len() as u32;
            (verdict, refusal.detail)
        }
    };
    let note = CellNote {
        // No door was asked about a token, a cell the row doesn't have or a formula with no result.
        detail: if origin == CellNote::ORIGIN_CELL {
            detail
        } else {
            0
        },
        origin,
    };
    // SAFETY: per the function contract; neither array need be aligned.
    unsafe {
        buffer.verdicts.add(row).write_unaligned(verdict);
        if !buffer.notes.is_null() {
            buffer.notes.add(row).write_unaligned(note);
        }
        if let Some(check) = check {
            buffer.checks.add(row).write(check);
        }
    }
    Ok(raw)
}

/// What a checked door said of a cell: passed, broke a constraint, or refused as 1.0 would.
enum Outcome {
    Passed(RawCheck),
    Broke(CellVerdict, RawCheck),
    Refused(Refused),
}

/// Writes a checked door's value at `row`: as it passed, as it was cast when it broke a
/// constraint, or the door's zero when it didn't cast.
///
/// # Safety
/// `buffer.values` has room for `row + 1` values of `T`.
#[inline(always)]
unsafe fn settle<T: Copy>(
    buffer: &ColumnBufferWith,
    row: usize,
    judged: Result<Result<Checked<T>, Rejection<T>>, Refused>,
    zero: T,
) -> Outcome {
    let (value, outcome) = match judged {
        Ok(Ok(passed)) => (
            passed.value,
            Outcome::Passed(RawCheck {
                keyword: 0,
                adjusted: u32::from(passed.adjusted),
                enum_index: passed.enum_index.unwrap_or(u32::MAX),
            }),
        ),
        Ok(Err(Rejection::Violation(broken))) => (
            broken.value,
            Outcome::Broke(
                CellVerdict {
                    offset: broken.offset,
                    len: broken.len,
                    reason: CellVerdict::VIOLATION,
                },
                RawCheck {
                    keyword: broken.keyword.code(),
                    adjusted: 0,
                    enum_index: u32::MAX,
                },
            ),
        ),
        Ok(Err(Rejection::Failure(failure))) => (zero, Outcome::Refused(failure.into())),
        Err(refusal) => (zero, Outcome::Refused(refusal)),
    };
    // SAFETY: per the function contract; the array need not be aligned.
    unsafe { buffer.values.cast::<T>().add(row).write_unaligned(value) };
    outcome
}

/// [`numeric_with`], through the checked doors: text through the checked numeric door, a number
/// through its typed twin, which judges it on its shortest round-trip digits.
#[inline(always)]
fn numeric_checked<T: Numeric>(
    cell: &Cell<'_>,
    format: &ValidNumFormat,
    constraints: &Constraints<'_, T>,
) -> Result<Result<Checked<T>, Rejection<T>>, Refused> {
    match *cell {
        Cell::Empty => Err(BLANK),
        Cell::Text(text) => Ok(detailed::cast_checked(text, format, constraints)),
        Cell::Number(value) => Ok(detailed::checked_from_f64(value, constraints)),
        _ => Err(WRONG_KIND),
    }
}

/// [`cast_with`]'s cell under its column's constraints: the value written, and what the door
/// said. `blank` says the cell is `Empty` whatever its slot holds, as a token or a formula with
/// no result is. A typed cell at the text door is judged on its rendering, which `raw` then
/// spans; a text cell's `raw` is its own bytes.
///
/// # Safety
/// As [`cast_with`].
#[allow(clippy::too_many_arguments)]
#[inline(always)]
unsafe fn checked_cell(
    slot: &Slot,
    blank: bool,
    raw: &mut Span,
    checks: Checks<'_>,
    format: &impl Fn() -> ValidNumFormat,
    buffer: &ColumnBufferWith,
    row: usize,
    stores: &mut Stores<'_>,
) -> Result<Outcome, u64> {
    let cell = if blank {
        Cell::Empty
    } else {
        slot.cell(stores.arena, stores.strings)
    };
    macro_rules! numeric {
        ($constraints:expr, $zero:expr) => {
            // SAFETY: the caller's contract.
            unsafe {
                settle(
                    buffer,
                    row,
                    numeric_checked(&cell, &format(), &$constraints),
                    $zero,
                )
            }
        };
    }
    Ok(match checks {
        Checks::I8(c) => numeric!(c, 0),
        Checks::I16(c) => numeric!(c, 0),
        Checks::I32(c) => numeric!(c, 0),
        Checks::I64(c) => numeric!(c, 0),
        Checks::U8(c) => numeric!(c, 0),
        Checks::U16(c) => numeric!(c, 0),
        Checks::U32(c) => numeric!(c, 0),
        Checks::U64(c) => numeric!(c, 0),
        Checks::F32(c) => numeric!(c, 0.0),
        Checks::F64(c) => numeric!(c, 0.0),
        Checks::Decimal(c) => numeric!(
            c,
            Decimal {
                lo: 0,
                hi: 0,
                scale: 0,
                negative: false,
            }
        ),
        Checks::Text(c) => {
            let judged = match cell {
                Cell::Empty => Err(BLANK),
                Cell::Text(text) => Ok(text_checked(text, *raw, &c)),
                _ => {
                    *raw = append(
                        stores.arena,
                        stores.used,
                        render(&slot.cell(&[], &[])).as_bytes(),
                    )?;
                    let from = raw.offset as usize;
                    let text = stores
                        .arena
                        .get(from..from.saturating_add(raw.len()))
                        .unwrap_or_default();
                    Ok(text_checked(text, *raw, &c))
                }
            };
            // SAFETY: the caller's contract.
            unsafe { settle(buffer, row, judged, Span::default()) }
        }
    })
}

/// Re-exported for the row readers, which parse numbers and dates out of attribute text.
pub use number::parse_f64;
