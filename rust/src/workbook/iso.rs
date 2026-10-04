//! ISO 8601 wall-clock readings as XLSX `t="d"` cells and ODS `office:date-value`
//! attributes carry them: `yyyy-MM-dd`, `yyyy-MM-ddTHH:mm[:ss[.f…]]`, and the same with a
//! zone (which makes it an instant, presented as its UTC wall clock). Built from
//! HyperCast's own date/time/timestamp doors so the accepted profiles are exactly theirs.

use crate::Cell;
use hypercast::{cast_date, cast_time, cast_timestamp};

/// A wall-clock cell from ISO text, or `None` if the text is not one of the profiles.
pub fn wall_from_iso(text: &[u8]) -> Option<Cell<'static>> {
    let text = text.trim_ascii();
    if let Ok(date) = cast_date(text) {
        return Some(Cell::Wall { date, nanos: 0 });
    }
    if text.len() < 11 || (text[10] != b'T' && text[10] != b't' && text[10] != b' ') {
        return None;
    }
    let date = cast_date(&text[..10]).ok()?;
    let clock = &text[11..];
    let zoned = clock.ends_with(b"Z")
        || clock.ends_with(b"z")
        || clock.iter().skip(1).any(|&b| b == b'+' || b == b'-');
    if zoned {
        // An instant. HyperCast's RFC 3339 door demands the `T` separator and seconds;
        // ISO 29500's own example (`1976-11-22T08:30Z`) has neither guarantee.
        let zone_at = if clock.ends_with(b"Z") || clock.ends_with(b"z") {
            clock.len() - 1
        } else {
            clock.iter().skip(1).position(|&b| b == b'+' || b == b'-')? + 1
        };
        let mut canonical = Vec::with_capacity(text.len() + 3);
        canonical.extend_from_slice(&text[..10]);
        canonical.push(b'T');
        canonical.extend_from_slice(&clock[..zone_at]);
        if zone_at == 5 {
            canonical.extend_from_slice(b":00");
        }
        canonical.extend_from_slice(&clock[zone_at..]);
        let wall = cast_timestamp(&canonical).ok()?.utc_civil()?;
        return Some(Cell::Wall {
            date: wall.date,
            nanos: wall.nanos_of_day,
        });
    }
    let nanos = cast_time(clock).ok()?;
    Some(Cell::Wall { date, nanos })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercast::Date;

    #[test]
    fn reads_the_three_profiles() {
        let date = Date {
            year: 2024,
            month: 1,
            day: 31,
        };
        assert_eq!(
            wall_from_iso(b"2024-01-31"),
            Some(Cell::Wall { date, nanos: 0 })
        );
        assert_eq!(
            wall_from_iso(b"2024-01-31T10:00:00"),
            Some(Cell::Wall {
                date,
                nanos: 36_000_000_000_000
            })
        );
        assert_eq!(
            wall_from_iso(b"2024-01-31T10:00:00.5"),
            Some(Cell::Wall {
                date,
                nanos: 36_000_500_000_000
            })
        );
        assert_eq!(
            wall_from_iso(b"2024-01-31T10:00"),
            Some(Cell::Wall {
                date,
                nanos: 36_000_000_000_000
            })
        );
        // Zoned readings are instants, shown at UTC.
        assert_eq!(
            wall_from_iso(b"1976-11-22T08:30Z"),
            Some(Cell::Wall {
                date: Date {
                    year: 1976,
                    month: 11,
                    day: 22
                },
                nanos: 30_600_000_000_000
            })
        );
        assert_eq!(
            wall_from_iso(b"2024-01-01T00:30:00+01:00"),
            Some(Cell::Wall {
                date: Date {
                    year: 2023,
                    month: 12,
                    day: 31
                },
                nanos: 84_600_000_000_000
            })
        );
        assert_eq!(
            wall_from_iso(b"2024-01-31 10:00:00"),
            Some(Cell::Wall {
                date,
                nanos: 36_000_000_000_000
            })
        );
        assert_eq!(wall_from_iso(b"31/01/2024"), None);
        assert_eq!(wall_from_iso(b"2024-01-31Tnoon"), None);
    }
}
