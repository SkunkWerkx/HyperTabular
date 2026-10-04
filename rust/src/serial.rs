//! What a workbook's serial means once its number format has said what it is for. The
//! date rules themselves — the two date systems, the 1900 system's phantom February 29th —
//! are HyperCast's ([`hypercast::excel_serial`], the same code its text door runs); what
//! is tabular is the split a format declares: a date/time serial with a date in it is a
//! wall clock, one under `1` is a time of day with no date, and an elapsed serial is a
//! span of days. The fraction is kept at nanosecond resolution rather than rounded to
//! milliseconds — rounding is a presentation choice that belongs to the bindings.

use crate::cell::Cell;
use hypercast::{Duration, ExcelEpoch, MAX_DURATION_SECONDS, Reason, excel_serial};

/// What a temporal number format asked the serial to mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerialKind {
    /// A date and/or time of day (`yyyy-mm-dd`, `h:mm`, …).
    DateTime,
    /// An elapsed duration (`[h]:mm:ss`, `[mm]:ss`, …).
    Elapsed,
}

/// Nanoseconds in one day.
pub const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// Splits a non-negative serial into whole days and the nanoseconds of the fractional
/// day, rounding the fraction to the nearest nanosecond (and carrying into the day if
/// that rounds up to exactly one day).
pub fn split(value: f64) -> Result<(i64, u64), Reason> {
    if !value.is_finite() || value < 0.0 {
        // Excel renders a negative date serial as `####`; NaN/∞ never come from a file.
        return Err(Reason::Malformed);
    }
    if value >= 9.0e15 {
        // Past any calendar; also keeps the `as i64` below exact.
        return Err(Reason::OutOfRange);
    }
    let whole = value.floor();
    let mut days = whole as i64;
    let mut nanos = ((value - whole) * NANOS_PER_DAY as f64).round() as u64;
    if nanos >= NANOS_PER_DAY {
        days += 1;
        nanos -= NANOS_PER_DAY;
    }
    Ok((days, nanos))
}

/// Reads a serial the way its number format declared: a date/time serial becomes a
/// [`Cell::Wall`] (or a [`Cell::Clock`] when it is under 1 and so carries no date); an
/// elapsed serial becomes a [`Cell::Span`] of `value` days, sign and all.
pub fn to_cell(value: f64, system: ExcelEpoch, kind: SerialKind) -> Result<Cell<'static>, Reason> {
    match kind {
        SerialKind::DateTime => {
            let (days, nanos) = split(value)?;
            if days == 0 {
                return Ok(Cell::Clock(nanos));
            }
            let wall = excel_serial(value, system)?;
            Ok(Cell::Wall {
                date: wall.date,
                nanos: wall.nanos_of_day,
            })
        }
        SerialKind::Elapsed => Ok(Cell::Span(days_to_duration(value)?)),
    }
}

/// `value` days as a protobuf-shaped duration, same-signed nanos, bounded to HyperCast's
/// ±10,000-year window.
pub fn days_to_duration(value: f64) -> Result<Duration, Reason> {
    if !value.is_finite() {
        return Err(Reason::Malformed);
    }
    let seconds = value * 86_400.0;
    if seconds.abs() > MAX_DURATION_SECONDS as f64 {
        return Err(Reason::OutOfRange);
    }
    let total_nanos = (seconds * 1e9).round() as i128;
    Ok(Duration {
        seconds: (total_nanos / 1_000_000_000) as i64,
        nanos: (total_nanos % 1_000_000_000) as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercast::Date;

    #[test]
    fn split_rounds_the_fraction_to_nanoseconds() {
        assert_eq!(split(45_000.5), Ok((45_000, 43_200_000_000_000)));
        assert_eq!(split(0.0), Ok((0, 0)));
        // 1 - 2^-53 days rounds up to exactly one day and carries.
        assert_eq!(split(1.0 - f64::EPSILON / 2.0), Ok((1, 0)));
        assert_eq!(split(-1.0), Err(Reason::Malformed));
        assert_eq!(split(f64::NAN), Err(Reason::Malformed));
        assert_eq!(split(1e16), Err(Reason::OutOfRange));
    }

    #[test]
    fn to_cell_follows_the_declared_kind() {
        assert_eq!(
            to_cell(0.75, ExcelEpoch::Y1900, SerialKind::DateTime),
            Ok(Cell::Clock(64_800_000_000_000))
        );
        assert_eq!(
            to_cell(45_000.25, ExcelEpoch::Y1900, SerialKind::DateTime),
            Ok(Cell::Wall {
                date: Date {
                    year: 2023,
                    month: 3,
                    day: 15
                },
                nanos: 21_600_000_000_000
            })
        );
        assert_eq!(
            to_cell(1.5, ExcelEpoch::Y1900, SerialKind::Elapsed),
            Ok(Cell::Span(Duration {
                seconds: 129_600,
                nanos: 0
            }))
        );
        assert_eq!(
            to_cell(-0.000_000_5, ExcelEpoch::Y1904, SerialKind::Elapsed),
            Ok(Cell::Span(Duration {
                seconds: 0,
                nanos: -43_200_000
            }))
        );
        assert_eq!(
            to_cell(1e10, ExcelEpoch::Y1900, SerialKind::Elapsed),
            Err(Reason::OutOfRange)
        );
        // The date rules are HyperCast's: the phantom serial and a serial past 9999-12-31
        // are out of range, and the 1904 system has no phantom.
        assert_eq!(
            to_cell(60.0, ExcelEpoch::Y1900, SerialKind::DateTime),
            Err(Reason::OutOfRange)
        );
        assert_eq!(
            to_cell(2_958_466.0, ExcelEpoch::Y1900, SerialKind::DateTime),
            Err(Reason::OutOfRange)
        );
        assert_eq!(
            to_cell(60.0, ExcelEpoch::Y1904, SerialKind::DateTime),
            Ok(Cell::Wall {
                date: Date {
                    year: 1904,
                    month: 3,
                    day: 1
                },
                nanos: 0
            })
        );
    }
}
