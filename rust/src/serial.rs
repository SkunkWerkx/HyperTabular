//! Excel's serial dates: days since an epoch, with the time of day in the fraction, under
//! one of two date systems. The 1900 system inherits Lotus 1-2-3's belief that 1900 was
//! a leap year, so serial 60 is the nonexistent 1900-02-29 and every earlier serial is
//! off by one relative to the 1899-12-30 epoch the rest of the system implies. The rules
//! here are the intersection of what Sylvan.Data.Excel, calamine, xlrd, and Apache POI
//! agree on (see `docs/prior-art.md`), with one deliberate difference: the fraction is
//! kept at nanosecond resolution rather than rounded to milliseconds — rounding is a
//! presentation choice that belongs to the bindings.

use crate::cell::Cell;
use hypercast::{Date, Duration, MAX_DURATION_SECONDS, Reason};

/// Which epoch a workbook's serials count from.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateSystem {
    /// Epoch 1899-12-30, with the 1900 leap-year lie. Excel's default.
    #[default]
    Excel1900 = 0,
    /// Epoch 1904-01-01, no lie. The classic Macintosh default (`workbookPr@date1904`).
    Excel1904 = 1,
}

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

/// The calendar date `days` serial days after the system's epoch. `days` must be ≥ 1 —
/// a serial under 1 has no date (see [`to_cell`]).
pub fn date(days: i64, system: DateSystem) -> Result<Date, Reason> {
    let civil_days = match system {
        DateSystem::Excel1900 => {
            if days == 60 {
                // 1900-02-29 does not exist; Excel displays it anyway. We don't.
                return Err(Reason::Malformed);
            }
            // Serials 1..=59 sit one day earlier than the 1899-12-30 epoch implies.
            days_from_civil(1899, 12, 30) + days + i64::from(days < 60)
        }
        DateSystem::Excel1904 => days_from_civil(1904, 1, 1) + days,
    };
    let (year, month, day) = civil_from_days(civil_days);
    if year > 9999 {
        return Err(Reason::OutOfRange);
    }
    Ok(Date {
        year: year as u16,
        month: month as u8,
        day: day as u8,
    })
}

/// Reads a serial the way its number format declared: a date/time serial becomes a
/// [`Cell::Wall`] (or a [`Cell::Clock`] when it is under 1 and so carries no date); an
/// elapsed serial becomes a [`Cell::Span`] of `value` days, sign and all.
pub fn to_cell(value: f64, system: DateSystem, kind: SerialKind) -> Result<Cell<'static>, Reason> {
    match kind {
        SerialKind::DateTime => {
            let (days, nanos) = split(value)?;
            if days == 0 {
                return Ok(Cell::Clock(nanos));
            }
            Ok(Cell::Wall {
                date: date(days, system)?,
                nanos,
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

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm, the
/// same one HyperCast's temporal doors use).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let month_shift: i64 = if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * (month as i64 + month_shift) + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`]: `(year, month, day)` for days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_1900_system_keeps_lotus_honest() {
        assert_eq!(
            date(1, DateSystem::Excel1900),
            Ok(Date {
                year: 1900,
                month: 1,
                day: 1
            })
        );
        assert_eq!(
            date(59, DateSystem::Excel1900),
            Ok(Date {
                year: 1900,
                month: 2,
                day: 28
            })
        );
        assert_eq!(date(60, DateSystem::Excel1900), Err(Reason::Malformed));
        assert_eq!(
            date(61, DateSystem::Excel1900),
            Ok(Date {
                year: 1900,
                month: 3,
                day: 1
            })
        );
        assert_eq!(
            date(45_000, DateSystem::Excel1900),
            Ok(Date {
                year: 2023,
                month: 3,
                day: 15
            })
        );
        assert_eq!(
            date(2_958_465, DateSystem::Excel1900),
            Ok(Date {
                year: 9999,
                month: 12,
                day: 31
            })
        );
        assert_eq!(
            date(2_958_466, DateSystem::Excel1900),
            Err(Reason::OutOfRange)
        );
    }

    #[test]
    fn the_1904_system_is_plain_arithmetic() {
        assert_eq!(
            date(1, DateSystem::Excel1904),
            Ok(Date {
                year: 1904,
                month: 1,
                day: 2
            })
        );
        assert_eq!(
            date(60, DateSystem::Excel1904),
            Ok(Date {
                year: 1904,
                month: 3,
                day: 1
            })
        );
        assert_eq!(
            date(2_957_003, DateSystem::Excel1904),
            Ok(Date {
                year: 9999,
                month: 12,
                day: 31
            })
        );
        assert_eq!(
            date(2_957_004, DateSystem::Excel1904),
            Err(Reason::OutOfRange)
        );
    }

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
            to_cell(0.75, DateSystem::Excel1900, SerialKind::DateTime),
            Ok(Cell::Clock(64_800_000_000_000))
        );
        assert_eq!(
            to_cell(45_000.25, DateSystem::Excel1900, SerialKind::DateTime),
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
            to_cell(1.5, DateSystem::Excel1900, SerialKind::Elapsed),
            Ok(Cell::Span(Duration {
                seconds: 129_600,
                nanos: 0
            }))
        );
        assert_eq!(
            to_cell(-0.000_000_5, DateSystem::Excel1904, SerialKind::Elapsed),
            Ok(Cell::Span(Duration {
                seconds: 0,
                nanos: -43_200_000
            }))
        );
        assert_eq!(
            to_cell(1e10, DateSystem::Excel1900, SerialKind::Elapsed),
            Err(Reason::OutOfRange)
        );
    }

    #[test]
    fn civil_round_trips() {
        for days in [-719_468, -1, 0, 1, 10_957, 19_796, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_796), (2024, 3, 14));
    }
}
