//! The workbook core's rules, stated by hand: what a number format means, what a serial
//! is, which ISO texts are a wall clock, and how a typed cell is said as text. These are
//! the expectations the std workbook reader's own unit tests held it to, carried over to
//! the core that replaced it — written down, not computed, so they do not move when the
//! code does.

use hypertabular::kernel::workbook::cell::{Cell, render, serial, wall_from_iso};
use hypertabular::kernel::workbook::styles::{DATE_TIME, ELAPSED, NUMBER, TEXT, builtin, classify};
use hypertabular::{Date, Duration, ExcelEpoch};

fn said(cell: Cell<'_>) -> String {
    String::from_utf8(render(&cell).as_bytes().to_vec()).unwrap()
}

#[test]
fn builtin_number_formats() {
    assert_eq!(builtin(0), NUMBER);
    assert_eq!(builtin(4), NUMBER);
    assert_eq!(builtin(14), DATE_TIME);
    assert_eq!(builtin(22), DATE_TIME);
    assert_eq!(builtin(45), DATE_TIME);
    assert_eq!(builtin(46), ELAPSED);
    assert_eq!(builtin(47), DATE_TIME);
    assert_eq!(builtin(49), TEXT);
    assert_eq!(builtin(164), NUMBER);
}

#[test]
fn custom_number_formats() {
    assert_eq!(classify(b"yyyy-mm-dd"), DATE_TIME);
    assert_eq!(classify(b"m/d/yyyy h:mm"), DATE_TIME);
    assert_eq!(classify(b"h:mm AM/PM"), DATE_TIME);
    assert_eq!(classify(b"[h]:mm:ss"), ELAPSED);
    assert_eq!(classify(b"[mm]:ss"), ELAPSED);
    assert_eq!(classify(b"0.00"), NUMBER);
    assert_eq!(classify(b"#,##0.00;[Red]-#,##0.00"), NUMBER);
    assert_eq!(classify(b"General"), NUMBER);
    assert_eq!(classify(b"0.00E+00"), NUMBER);
    assert_eq!(classify(b"\"Total: \"0.00"), NUMBER);
    assert_eq!(classify(b"\"Days: \"d"), DATE_TIME);
    assert_eq!(classify(b"0\\d"), NUMBER);
    assert_eq!(classify(b"[$-409]d-mmm-yy"), DATE_TIME);
    assert_eq!(classify(b"[$USD] #,##0"), NUMBER);
    assert_eq!(classify(b"@"), TEXT);
    assert_eq!(classify(b"0.0;yyyy"), NUMBER);
    assert_eq!(classify(b"AM/PM"), NUMBER);
    // And what must not trip it: an escape or a bracket the code ends inside.
    assert_eq!(classify(b"0\\"), NUMBER);
    assert_eq!(classify(b"[h"), ELAPSED);
    assert_eq!(classify(b"\"unclosed d"), NUMBER);
    assert_eq!(classify(b""), NUMBER);
}

#[test]
fn a_serial_under_one_day_has_no_date() {
    let y1900 = ExcelEpoch::Y1900;
    assert_eq!(serial(0.0, ExcelEpoch::Y1904, false), Some(Cell::Clock(0)));
    assert_eq!(
        serial(0.5, y1900, false),
        Some(Cell::Clock(43_200_000_000_000))
    );
    // 1 - 2^-53 days rounds up to exactly one day and carries: a date, at midnight.
    assert_eq!(
        serial(1.0 - f64::EPSILON / 2.0, y1900, false),
        Some(Cell::Wall {
            date: Date {
                year: 1900,
                month: 1,
                day: 1
            },
            nanos: 0
        })
    );
    // Half a nanosecond short of a day is still the day before, rounded down.
    let short = 1.0 - 0.6 / 86_400_000_000_000.0;
    assert_eq!(
        serial(short, y1900, false),
        Some(Cell::Clock(86_399_999_999_999))
    );
    assert_eq!(serial(-1.0, y1900, false), None);
    assert_eq!(serial(f64::NAN, y1900, false), None);
    assert_eq!(serial(1e16, y1900, false), None);
}

#[test]
fn a_serial_is_what_its_format_declared() {
    let (y1900, y1904) = (ExcelEpoch::Y1900, ExcelEpoch::Y1904);
    assert_eq!(
        serial(0.75, y1900, false),
        Some(Cell::Clock(64_800_000_000_000))
    );
    assert_eq!(
        serial(45_000.25, y1900, false),
        Some(Cell::Wall {
            date: Date {
                year: 2023,
                month: 3,
                day: 15
            },
            nanos: 21_600_000_000_000
        })
    );
    assert_eq!(
        serial(1.5, y1900, true),
        Some(Cell::Span(Duration {
            seconds: 129_600,
            nanos: 0
        }))
    );
    assert_eq!(
        serial(-0.000_000_5, y1904, true),
        Some(Cell::Span(Duration {
            seconds: 0,
            nanos: -43_200_000
        }))
    );
    assert_eq!(serial(1e10, y1900, true), None);
    assert_eq!(serial(f64::INFINITY, y1900, true), None);
    // The date rules are HyperCast's: the phantom serial and a serial past 9999-12-31
    // are refused, and the 1904 system has no phantom.
    assert_eq!(serial(60.0, y1900, false), None);
    assert_eq!(serial(2_958_466.0, y1900, false), None);
    assert_eq!(
        serial(60.0, y1904, false),
        Some(Cell::Wall {
            date: Date {
                year: 1904,
                month: 3,
                day: 1
            },
            nanos: 0
        })
    );
}

#[test]
fn iso_text_is_a_wall_clock_in_three_profiles() {
    let date = Date {
        year: 2024,
        month: 1,
        day: 31,
    };
    let wall = |nanos| Some(Cell::Wall { date, nanos });
    assert_eq!(wall_from_iso(b"2024-01-31"), wall(0));
    assert_eq!(wall_from_iso(b" 2024-01-31 "), wall(0));
    assert_eq!(
        wall_from_iso(b"2024-01-31T10:00:00"),
        wall(36_000_000_000_000)
    );
    assert_eq!(
        wall_from_iso(b"2024-01-31T10:00:00.5"),
        wall(36_000_500_000_000)
    );
    assert_eq!(wall_from_iso(b"2024-01-31T10:00"), wall(36_000_000_000_000));
    assert_eq!(
        wall_from_iso(b"2024-01-31 10:00:00"),
        wall(36_000_000_000_000)
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
    assert_eq!(wall_from_iso(b"31/01/2024"), None);
    assert_eq!(wall_from_iso(b"2024-01-31Tnoon"), None);
    assert_eq!(wall_from_iso(b""), None);
    assert_eq!(wall_from_iso(b"2024-01-31T"), None);
    // Longer than any timestamp: refused, not overrun.
    let long = format!("2024-01-31T10:00:00.{}Z", "1".repeat(200));
    assert_eq!(wall_from_iso(long.as_bytes()), None);
}

#[test]
fn a_typed_cell_is_said_as_text_a_door_reads_back() {
    assert_eq!(said(Cell::Number(42.0)), "42");
    assert_eq!(said(Cell::Number(-7.0)), "-7");
    assert_eq!(said(Cell::Number(0.1)), "0.1");
    assert_eq!(said(Cell::Number(2.5)), "2.5");
    assert_eq!(said(Cell::Number(1e16)), "1e16");
    assert_eq!(said(Cell::Number(1e300)), "1e300");
    assert_eq!(said(Cell::Number(-0.0)), "0");
    assert_eq!(said(Cell::Bool(true)), "true");
    assert_eq!(said(Cell::Bool(false)), "false");
    assert_eq!(said(Cell::Error(0x2A)), "#N/A");
    assert_eq!(said(Cell::Error(0x07)), "#DIV/0!");
    assert_eq!(said(Cell::Empty), "");

    let date = Date {
        year: 2026,
        month: 1,
        day: 2,
    };
    assert_eq!(said(Cell::Wall { date, nanos: 0 }), "2026-01-02T00:00:00");
    assert_eq!(
        said(Cell::Wall {
            date,
            nanos: 54_245_123_456_789
        }),
        "2026-01-02T15:04:05.123456789"
    );
    assert_eq!(said(Cell::Clock(86_399_000_000_001)), "23:59:59.000000001");
    assert_eq!(said(Cell::Clock(0)), "00:00:00");
    let span = |seconds, nanos| Cell::Span(Duration { seconds, nanos });
    assert_eq!(said(span(108_000, 0)), "P1DT6H");
    assert_eq!(said(span(86_400, 0)), "P1D");
    assert_eq!(said(span(-1, -500_000_000)), "-PT1.5S");
    assert_eq!(said(span(0, 1)), "PT0.000000001S");
    assert_eq!(said(span(0, 0)), "PT0S");
    assert_eq!(said(span(90, 0)), "PT1M30S");

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
