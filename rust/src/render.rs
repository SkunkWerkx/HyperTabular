//! Canonical text for typed cells — what the text door returns for a workbook value, and
//! what the batch records as the "raw" of a typed cell that faulted. Every rendering is
//! something a HyperCast door would read back where one exists: integral numbers as
//! plain digits, wall clocks and times in the ISO profiles `cast_date`/`cast_time`
//! accept, spans in the ISO 8601 form `cast_duration` accepts.

use crate::cell::Cell;
use hypercast::{Date, Duration};
use std::io::Write;

/// Appends the canonical text of `cell` to `out`. [`Cell::Empty`] renders as nothing.
pub fn render(cell: &Cell<'_>, out: &mut Vec<u8>) {
    match *cell {
        Cell::Empty => {}
        Cell::Text(text) => out.extend_from_slice(text),
        Cell::Number(value) => render_number(value, out),
        Cell::Bool(true) => out.extend_from_slice(b"true"),
        Cell::Bool(false) => out.extend_from_slice(b"false"),
        Cell::Wall { date, nanos } => {
            render_date(date, out);
            out.push(b'T');
            render_time(nanos, out);
        }
        Cell::Clock(nanos) => render_time(nanos, out),
        Cell::Span(span) => render_duration(span, out),
        Cell::Error(error) => out.extend_from_slice(error.text().as_bytes()),
    }
}

fn render_number(value: f64, out: &mut Vec<u8>) {
    // Integral values print as integers; everything else as the shortest round-trip
    // representation (`{:?}` switches to exponent form for very large/small magnitudes,
    // which `{}` never does — `1e300` is `1e300`, not 301 characters).
    if value.fract() == 0.0 && value.abs() < 1e16 {
        let _ = write!(out, "{}", value as i64);
    } else {
        let _ = write!(out, "{value:?}");
    }
}

fn push_padded(value: u64, width: usize, out: &mut Vec<u8>) {
    let mut digits = [b'0'; 20];
    let mut cursor = digits.len();
    let mut remaining = value;
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    while digits.len() - cursor < width {
        cursor -= 1;
    }
    out.extend_from_slice(&digits[cursor..]);
}

fn render_date(date: Date, out: &mut Vec<u8>) {
    push_padded(u64::from(date.year), 4, out);
    out.push(b'-');
    push_padded(u64::from(date.month), 2, out);
    out.push(b'-');
    push_padded(u64::from(date.day), 2, out);
}

/// `HH:mm:ss`, with a fraction only when there is one, trimmed of trailing zeros.
fn render_time(nanos: u64, out: &mut Vec<u8>) {
    let seconds = nanos / 1_000_000_000;
    push_padded(seconds / 3_600, 2, out);
    out.push(b':');
    push_padded(seconds % 3_600 / 60, 2, out);
    out.push(b':');
    push_padded(seconds % 60, 2, out);
    render_fraction(nanos % 1_000_000_000, out);
}

fn render_fraction(nanos: u64, out: &mut Vec<u8>) {
    if nanos == 0 {
        return;
    }
    out.push(b'.');
    let start = out.len();
    push_padded(nanos, 9, out);
    while out.last() == Some(&b'0') {
        out.pop();
    }
    debug_assert!(out.len() > start);
}

/// ISO 8601 with fixed components only — `-P1DT6H30M15.5S` — the same profile
/// HyperCast's duration door reads; zero components are omitted, and a zero span is `PT0S`.
fn render_duration(span: Duration, out: &mut Vec<u8>) {
    let negative = span.seconds < 0 || span.nanos < 0;
    if negative {
        out.push(b'-');
    }
    let seconds = span.seconds.unsigned_abs();
    let nanos = span.nanos.unsigned_abs() as u64;
    out.push(b'P');
    let days = seconds / 86_400;
    if days > 0 {
        push_padded(days, 1, out);
        out.push(b'D');
    }
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    let whole_seconds = seconds % 60;
    if hours == 0 && minutes == 0 && whole_seconds == 0 && nanos == 0 {
        if days == 0 {
            out.extend_from_slice(b"T0S");
        }
        return;
    }
    out.push(b'T');
    if hours > 0 {
        push_padded(hours, 1, out);
        out.push(b'H');
    }
    if minutes > 0 {
        push_padded(minutes, 1, out);
        out.push(b'M');
    }
    if whole_seconds > 0 || nanos > 0 {
        push_padded(whole_seconds, 1, out);
        render_fraction(nanos, out);
        out.push(b'S');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(cell: Cell<'_>) -> String {
        let mut out = Vec::new();
        render(&cell, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn numbers_render_shortest_and_integral_as_integers() {
        assert_eq!(text(Cell::Number(42.0)), "42");
        assert_eq!(text(Cell::Number(-7.0)), "-7");
        assert_eq!(text(Cell::Number(0.1)), "0.1");
        assert_eq!(text(Cell::Number(2.5)), "2.5");
        assert_eq!(text(Cell::Number(1e16)), "1e16");
        assert_eq!(text(Cell::Number(1e300)), "1e300");
        assert_eq!(text(Cell::Number(-0.0)), "0");
    }

    #[test]
    fn temporals_render_in_the_iso_profiles_the_doors_read() {
        let date = Date {
            year: 2026,
            month: 1,
            day: 2,
        };
        assert_eq!(text(Cell::Wall { date, nanos: 0 }), "2026-01-02T00:00:00");
        assert_eq!(
            text(Cell::Wall {
                date,
                nanos: 54_245_123_456_789
            }),
            "2026-01-02T15:04:05.123456789"
        );
        assert_eq!(text(Cell::Clock(86_399_000_000_001)), "23:59:59.000000001");
        assert_eq!(text(Cell::Clock(0)), "00:00:00");
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: 108_000,
                nanos: 0
            })),
            "P1DT6H"
        );
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: 86_400,
                nanos: 0
            })),
            "P1D"
        );
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: -1,
                nanos: -500_000_000
            })),
            "-PT1.5S"
        );
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: 0,
                nanos: 1
            })),
            "PT0.000000001S"
        );
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: 0,
                nanos: 0
            })),
            "PT0S"
        );
        assert_eq!(
            text(Cell::Span(Duration {
                seconds: 90,
                nanos: 0
            })),
            "PT1M30S"
        );
        // Round trip through HyperCast's own doors.
        assert_eq!(
            hypercast::cast_duration("P1DT6H"),
            Ok(Duration {
                seconds: 108_000,
                nanos: 0
            })
        );
        assert_eq!(
            hypercast::cast_time("15:04:05.123456789"),
            Ok(54_245_123_456_789)
        );
        assert_eq!(hypercast::cast_date("2026-01-02"), Ok(date));
    }
}
