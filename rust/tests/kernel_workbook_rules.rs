//! The workbook core's rules, stated by hand: what a number format means, what a serial
//! is, which ISO texts are a wall clock, and how a typed cell is said as text. These are
//! the expectations the std workbook reader's own unit tests held it to, carried over to
//! the core that replaced it — written down, not computed, so they do not move when the
//! code does.

#[path = "support/corpus.rs"]
#[allow(unreachable_patterns)] // Its wildcards are for outside the crate.
mod corpus;
use super::{packages, support};

use hypertabular::kernel::abi::{
    CellNote, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith,
    ERR_CONTRACT, Filled, OK, RawRange, RawSheetOptions, Slot,
};
use hypertabular::kernel::exports::{
    hypertabular_range_parse, hypertabular_workbook_fill, hypertabular_workbook_fill_with,
    hypertabular_workbook_sheet_with,
};
use hypertabular::kernel::workbook::cell::{Cell, render, serial, wall_from_iso};
use hypertabular::kernel::workbook::styles::{
    DATE_TIME, ELAPSED, NUMBER, TEXT, TIME, builtin, classify,
};
use hypertabular::{
    Column, Date, DateOptions, Detail, Door, Duration, Error, ExcelEpoch, FailureKind, NumFormat,
    Origin, Range, Reason, SheetOptions, Workbook,
};
use packages::{ods, ranges_numbers, ranges_ods, ranges_xlsx, uncached_ods, uncached_xlsx, xlsx};
use std::ptr::{null, null_mut};
use support::{Harness, Sizing};

fn said(cell: Cell<'_>) -> String {
    String::from_utf8(render(&cell).as_bytes().to_vec()).unwrap()
}

#[test]
fn builtin_number_formats() {
    assert_eq!(builtin(0), NUMBER);
    assert_eq!(builtin(4), NUMBER);
    assert_eq!(builtin(14), DATE_TIME);
    assert_eq!(builtin(18), TIME);
    assert_eq!(builtin(21), TIME);
    assert_eq!(builtin(22), DATE_TIME);
    assert_eq!(builtin(45), TIME);
    assert_eq!(builtin(46), ELAPSED);
    assert_eq!(builtin(47), TIME);
    // The locale ids: a date or a time in every locale the spec lists, or — 34, 35, 52, 53,
    // 55, 56 — a time in Chinese and a date in Japanese or Korean, read as the time.
    assert_eq!(builtin(31), DATE_TIME);
    assert_eq!(builtin(32), TIME);
    assert_eq!(builtin(34), TIME);
    assert_eq!(builtin(54), DATE_TIME);
    assert_eq!(builtin(56), TIME);
    assert_eq!(builtin(76), TIME);
    assert_eq!(builtin(77), DATE_TIME);
    assert_eq!(builtin(79), ELAPSED);
    assert_eq!(builtin(81), DATE_TIME);
    assert_eq!(builtin(49), TEXT);
    assert_eq!(builtin(164), NUMBER);
}

#[test]
fn custom_number_formats() {
    assert_eq!(classify(b"yyyy-mm-dd"), DATE_TIME);
    assert_eq!(classify(b"m/d/yyyy h:mm"), DATE_TIME);
    assert_eq!(classify(b"h:mm AM/PM"), TIME);
    // Month versus minutes: `m` after an `h` or before an `s` is minutes, else the month.
    assert_eq!(classify(b"h:mm"), TIME);
    assert_eq!(classify(b"mm:ss"), TIME);
    assert_eq!(classify(b"m:ss.0"), TIME);
    assert_eq!(classify(b"hh\"h\" mm\"m\""), TIME);
    assert_eq!(classify(b"mmm"), DATE_TIME);
    assert_eq!(classify(b"mm"), DATE_TIME);
    assert_eq!(classify(b"mmmm h"), DATE_TIME);
    assert_eq!(classify(b"h:mm mm"), DATE_TIME);
    assert_eq!(classify(b"[$-F400]h:mm:ss AM/PM"), TIME);
    assert_eq!(classify(b"[$-F800]dddd, mmmm dd, yyyy"), DATE_TIME);
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
    let y1904 = ExcelEpoch::Y1904;
    assert_eq!(serial(0.0, y1904, TIME), Some(Cell::Clock(0)));
    // A date format in the 1900 system: its serial 0 is the `1900-01-00` that never was,
    // so what is under a day is still only a time.
    assert_eq!(
        serial(0.5, y1900, DATE_TIME),
        Some(Cell::Clock(43_200_000_000_000))
    );
    assert_eq!(
        serial(0.5, y1900, TIME),
        Some(Cell::Clock(43_200_000_000_000))
    );
    // 1 - 2^-53 days rounds up to exactly one day and carries: a date, at midnight.
    assert_eq!(
        serial(1.0 - f64::EPSILON / 2.0, y1900, TIME),
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
        serial(short, y1900, TIME),
        Some(Cell::Clock(86_399_999_999_999))
    );
    assert_eq!(serial(-1.0, y1900, TIME), None);
    assert_eq!(serial(f64::NAN, y1900, TIME), None);
    assert_eq!(serial(1e16, y1900, TIME), None);
}

#[test]
fn a_1904_date_format_has_a_date_from_serial_zero() {
    let y1904 = ExcelEpoch::Y1904;
    let first = Date {
        year: 1904,
        month: 1,
        day: 1,
    };
    assert_eq!(
        serial(0.0, y1904, DATE_TIME),
        Some(Cell::Wall {
            date: first,
            nanos: 0
        })
    );
    assert_eq!(
        serial(0.5, y1904, DATE_TIME),
        Some(Cell::Wall {
            date: first,
            nanos: 43_200_000_000_000
        })
    );
    assert_eq!(serial(-1.0, y1904, DATE_TIME), None);
    assert_eq!(serial(f64::NAN, y1904, DATE_TIME), None);
}

#[test]
fn a_serial_is_what_its_format_declared() {
    let (y1900, y1904) = (ExcelEpoch::Y1900, ExcelEpoch::Y1904);
    assert_eq!(
        serial(0.75, y1900, TIME),
        Some(Cell::Clock(64_800_000_000_000))
    );
    assert_eq!(
        serial(45_000.25, y1900, DATE_TIME),
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
        serial(1.5, y1900, ELAPSED),
        Some(Cell::Span(Duration {
            seconds: 129_600,
            nanos: 0
        }))
    );
    assert_eq!(
        serial(-0.000_000_5, y1904, ELAPSED),
        Some(Cell::Span(Duration {
            seconds: 0,
            nanos: -43_200_000
        }))
    );
    assert_eq!(serial(1e10, y1900, ELAPSED), None);
    assert_eq!(serial(f64::INFINITY, y1900, ELAPSED), None);
    // The date rules are HyperCast's: the phantom serial and a serial past 9999-12-31
    // are refused, and the 1904 system has no phantom.
    assert_eq!(serial(60.0, y1900, DATE_TIME), None);
    assert_eq!(serial(2_958_466.0, y1900, DATE_TIME), None);
    assert_eq!(
        serial(60.0, y1904, DATE_TIME),
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

// ---------------------------------------------------------------------------------------
// Ranges, skip and header row. The table `ranges_xlsx` draws, in all three formats.

fn tables() -> [(&'static str, Vec<u8>); 3] {
    [
        ("xlsx", ranges_xlsx()),
        ("ods", ranges_ods()),
        ("numbers", ranges_numbers()),
    ]
}

fn range(text: &str) -> Range {
    Range::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn no_header() -> SheetOptions {
    SheetOptions::new().with_header(false)
}

fn every_row() -> SheetOptions {
    no_header().with_empty_rows_skipped(false)
}

#[derive(Debug)]
struct Read {
    header: Option<Vec<String>>,
    rows: Vec<(u32, Vec<String>)>,
    failure: Option<(FailureKind, u32)>,
}

/// Every row of a sheet, through the text door over `width` columns: `-` for an `Empty`.
fn read(package: &[u8], sheet: usize, options: SheetOptions, width: usize) -> Read {
    let book = Workbook::from_slice(package).expect("a workbook");
    let plan: Vec<Column> = (0..width).map(Column::text).collect();
    let mut opened = book.sheet(sheet, options, &plan).expect("a sheet");
    let header = opened.header().map(|header| {
        header
            .names()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect()
    });
    let (mut rows, mut failure) = (Vec::new(), None);
    loop {
        match opened.read() {
            Ok(Some(batch)) => {
                for row in 0..batch.rows() {
                    let cells = (0..width)
                        .map(|column| match batch.text(column, row) {
                            Ok(text) => String::from_utf8_lossy(text).into_owned(),
                            Err(_) => "-".to_string(),
                        })
                        .collect();
                    rows.push((batch.line(row), cells));
                }
            }
            Ok(None) => break,
            Err(Error::Structure(found)) => {
                failure = Some((found.kind, found.line));
                break;
            }
            Err(error) => panic!("{error}"),
        }
    }
    Read {
        header,
        rows,
        failure,
    }
}

/// `rows`, written the way the tests below write them.
fn table(rows: &[(u32, &[&str])]) -> Vec<(u32, Vec<String>)> {
    rows.iter()
        .map(|&(number, cells)| (number, cells.iter().map(|cell| cell.to_string()).collect()))
        .collect()
}

fn names(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| name.to_string()).collect())
}

#[test]
fn range_parse() {
    let parse = |text: &str| {
        let mut out = RawRange {
            first_row: 7,
            first_column: 7,
            last_row: 7,
            last_column: 7,
        };
        // SAFETY: the text and `out` are live.
        let code = unsafe { hypertabular_range_parse(text.as_ptr(), text.len(), &mut out) };
        (code, out)
    };
    let four = |out: RawRange| {
        (
            out.first_row,
            out.first_column,
            out.last_row,
            out.last_column,
        )
    };
    assert_eq!(parse("A5:F200").0, OK);
    assert_eq!(four(parse("A5:F200").1), (5, 1, 200, 6));
    assert_eq!(four(parse("A1:A1").1), (1, 1, 1, 1));
    assert_eq!(four(parse("AB12:AC13").1), (12, 28, 13, 29));
    assert_eq!(
        four(parse("XFD1048576:XFD1048576").1),
        (1_048_576, 16_384, 1_048_576, 16_384)
    );
    assert_eq!(
        four(parse("A1:A4294967295").1),
        (1, 1, u32::MAX, 1),
        "the core reads rows a u32 can number"
    );
    for refused in [
        "",
        ":",
        "A5",
        "A5:",
        ":F200",
        "a5:f200",
        "A5:f200",
        "F200:A5",
        "F5:A200",
        "A200:F5",
        "A:F",
        "5:6",
        "A0:B1",
        "A1:B0",
        "A01:B2",
        "$A$1:B2",
        "A5:F200 ",
        " A5:F200",
        "A5 :F200",
        "A5:F200:G300",
        "XFE1:XFE1",
        "AAAA1:AAAA1",
        "A1:A4294967296",
        "A1:A99999999999999999999",
        "A-1:B2",
        "A+1:B2",
        "é1:é2",
    ] {
        let (code, out) = parse(refused);
        assert_eq!(code, ERR_CONTRACT, "{refused:?}");
        assert_eq!(four(out), (7, 7, 7, 7), "{refused:?} wrote");
        assert!(Range::parse(refused).is_err(), "{refused:?}");
    }
    let mut out = RawRange::default();
    // SAFETY: nothing here is read past what its length says.
    unsafe {
        assert_eq!(hypertabular_range_parse(null(), 0, &mut out), ERR_CONTRACT);
        assert_eq!(hypertabular_range_parse(null(), 5, &mut out), ERR_CONTRACT);
        let text = "A1:A1";
        assert_eq!(
            hypertabular_range_parse(text.as_ptr(), text.len(), null_mut()),
            ERR_CONTRACT
        );
    }
    assert_eq!(range("A5:F200").first_row(), 5);
    assert_eq!(range("A5:F200").first_column(), 1);
    assert_eq!(range("A5:F200").last_row(), 200);
    assert_eq!(range("A5:F200").last_column(), 6);
    assert_eq!(
        Range::new(0, 2, 0, 0)
            .expect("no limit on three sides")
            .first_column(),
        2
    );
    assert!(
        Range::parse("A4294967296:A4294967296").is_err(),
        "past a u32"
    );
    assert!(
        Range::parse("A4294967295:A4294967295").is_ok(),
        "the last a u32 holds"
    );
    assert!(Range::new(5, 0, 4, 0).is_err(), "inverted rows");
    assert!(Range::new(0, 5, 0, 4).is_err(), "inverted columns");
}

#[test]
fn a_range_reads_inside_it() {
    for (format, package) in tables() {
        let options = SheetOptions::new().with_range(range("B4:E12"));
        let read = read(&package, 0, options, 3);
        assert_eq!(
            read.header,
            names(&["id", "name", "amt", "day"]),
            "{format}"
        );
        assert_eq!(
            read.rows,
            table(&[
                (5, &["1", "alpha", "1.5"]),
                (6, &["2", "NA", "NA"]),
                (8, &["3", "rep", "12a"]),
                (9, &["3", "rep", "12a"]),
                (10, &["3", "rep", "12a"]),
                (11, &["4", "-", "-"]),
                (12, &["merged", "-", "7.25"]),
            ]),
            "{format}"
        );
        assert_eq!(read.failure, None, "{format}");
    }
}

#[test]
fn a_range_delivers_the_empty_rows_inside_it() {
    for (format, package) in tables() {
        let options = every_row().with_range(range("B6:C8"));
        assert_eq!(
            read(&package, 0, options, 2).rows,
            table(&[(6, &["2", "NA"]), (7, &["-", "-"]), (8, &["3", "rep"])]),
            "{format}"
        );
        // A gap in the row numbers on either side of the range's edge.
        let options = every_row().with_range(range("A2:A3"));
        assert_eq!(
            read(&package, 0, options, 1).rows,
            table(&[(2, &["-"]), (3, &["as of"])]),
            "{format}"
        );
        let options = every_row().with_range(range("A12:A14"));
        assert_eq!(
            read(&package, 0, options, 1).rows,
            table(&[(12, &["-"]), (13, &["-"]), (14, &["footer"])]),
            "{format}"
        );
    }
}

#[test]
fn a_range_cuts_repeated_rows() {
    for (format, package) in tables() {
        let cut = |text: &str| read(&package, 0, every_row().with_range(range(text)), 2).rows;
        assert_eq!(cut("B9:C9"), table(&[(9, &["3", "rep"])]), "{format}");
        assert_eq!(
            cut("B7:C8"),
            table(&[(7, &["-", "-"]), (8, &["3", "rep"])]),
            "{format}"
        );
        assert_eq!(
            cut("B10:C11"),
            table(&[(10, &["3", "rep"]), (11, &["4", "-"])]),
            "{format}"
        );
        assert_eq!(
            cut("B8:C10"),
            table(&[(8, &["3", "rep"]), (9, &["3", "rep"]), (10, &["3", "rep"])]),
            "{format}"
        );
        // And a repeated cell, or the same four cells, across a column's edge.
        assert_eq!(
            read(&package, 0, every_row().with_range(range("C15:E15")), 3).rows,
            table(&[(15, &["wide", "wide", "-"])]),
            "{format}"
        );
        assert_eq!(
            read(
                &package,
                0,
                SheetOptions::new().with_range(range("C15:E15")),
                1
            )
            .header,
            names(&["wide", "wide"]),
            "{format}"
        );
    }
}

#[test]
fn a_sparse_xlsx_row_across_each_edge() {
    let sheet = "<row r=\"2\"><c r=\"B2\" t=\"str\"><v>two</v></c></row>\
<row r=\"5\"><c r=\"D5\" t=\"str\"><v>five</v></c></row>\
<row r=\"9\"><c r=\"B9\" t=\"str\"><v>nine</v></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    let cut = |text: &str| read(&package, 0, every_row().with_range(range(text)), 3).rows;
    assert_eq!(
        cut("B3:D7"),
        table(&[
            (3, &["-", "-", "-"]),
            (4, &["-", "-", "-"]),
            (5, &["-", "-", "five"]),
            (6, &["-", "-", "-"]),
            (7, &["-", "-", "-"]),
        ])
    );
    assert_eq!(
        cut("C3:D5"),
        table(&[
            (3, &["-", "-", "-"]),
            (4, &["-", "-", "-"]),
            (5, &["-", "five", "-"])
        ])
    );
    // With the filter on, a row whose cells all lie outside the columns is an empty row.
    let options = no_header().with_range(range("B1:C9"));
    assert_eq!(
        read(&package, 0, options, 1).rows,
        table(&[(2, &["two"]), (9, &["nine"])])
    );
}

#[test]
fn an_ods_repeat_count_is_never_walked() {
    let table_xml = "<table:table table:name=\"Data\">\
<table:table-row table:number-rows-repeated=\"4000000000\">\
<table:table-cell office:value-type=\"string\"><text:p>many</text:p></table:table-cell></table:table-row>\
<table:table-row table:number-rows-repeated=\"200000000\"><table:table-cell/></table:table-row>\
<table:table-row><table:table-cell office:value-type=\"string\"><text:p>last</text:p></table:table-cell></table:table-row>\
</table:table>";
    let package = ods(table_xml, false);
    let started = std::time::Instant::now();
    let many = |number: u32| (number, vec!["many".to_string()]);
    let cut = |options: SheetOptions| read(&package, 0, options, 1);

    let options = no_header().with_range(range("A3999999998:A3999999999"));
    assert_eq!(
        cut(options).rows,
        vec![many(3_999_999_998), many(3_999_999_999)]
    );
    // Across the end of the repeat, into the empty rows after it.
    let options = every_row().with_range(range("A4000000000:A4000000001"));
    assert_eq!(
        cut(options).rows,
        vec![many(4_000_000_000), (4_000_000_001, vec!["-".to_string()])]
    );
    // Skip and header row through most of it, and the filter through what's left.
    let options = no_header()
        .with_range(range("A1:A4294967295"))
        .with_skip(3_000_000_000)
        .with_header_row(999_999_999);
    assert_eq!(
        cut(options).rows,
        vec![
            many(4_000_000_000),
            (4_200_000_001, vec!["last".to_string()])
        ]
    );
    // A range starting past a repeat that is all of the sheet that could be read.
    let options = no_header().with_range(range("A4294967295:A4294967295"));
    assert_eq!(cut(options).rows, vec![]);
    // A header past nearly all of it.
    let options = SheetOptions::new().with_header_row(3_999_999_999);
    let read = cut(options);
    assert_eq!(read.header, names(&["many"]));
    assert_eq!(read.rows, table(&[(4_200_000_001, &["last"])]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "something walked a repeat, one row at a time: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_range_past_the_end_reads_what_exists() {
    for (format, package) in tables() {
        let cut = |options: SheetOptions| read(&package, 0, options, 2);
        let options = every_row().with_range(range("B14:C1000"));
        assert_eq!(
            cut(options).rows,
            table(&[(14, &["-", "-"]), (15, &["wide", "wide"])]),
            "{format}: no row is made up past the sheet's last"
        );
        let options = no_header().with_range(range("B14:C1000"));
        assert_eq!(
            cut(options).rows,
            table(&[(15, &["wide", "wide"])]),
            "{format}"
        );
        let options = every_row().with_range(range("A100:B200"));
        assert_eq!(cut(options).rows, vec![], "{format}");
        let read_past = cut(SheetOptions::new().with_range(range("A100:B200")));
        assert_eq!(read_past.header, Some(vec![]), "{format}");
        assert_eq!(read_past.rows, vec![], "{format}");
        // Columns past the sheet's widest.
        let options = no_header().with_range(range("XFA1:XFD15"));
        assert_eq!(cut(options).rows, vec![], "{format}");
        // Every last row there is: the whole sheet, and no more.
        let options = no_header().with_range(Range::new(14, 0, u32::MAX, 0).expect("a range"));
        assert_eq!(
            cut(options).rows,
            table(&[(14, &["footer", "-"]), (15, &["wide", "wide"])]),
            "{format}"
        );
        // A last row of 0 is no limit, as is a first row of 0.
        let options = no_header().with_range(Range::new(14, 0, 0, 0).expect("a range"));
        assert_eq!(cut(options).rows.len(), 2, "{format}");
        let options = no_header().with_range(Range::new(0, 0, 1, 0).expect("a range"));
        assert_eq!(
            cut(options).rows,
            table(&[(1, &["Quarterly report", "-"])]),
            "{format}"
        );
        let options = no_header().with_range(Range::new(u32::MAX, 0, u32::MAX, 0).expect("one"));
        assert_eq!(cut(options).rows, vec![], "{format}");
    }
}

#[test]
fn sheet_skip_and_header_row_count_inside_the_range() {
    let header = ["", "id", "name", "amt", "day"];
    for (format, package) in tables() {
        let cut = |options: SheetOptions| read(&package, 0, options, 2);

        // Skip counts the sheet's rows, the empty ones too, whatever the filter says.
        let whole = range("A1:F15");
        let options = SheetOptions::new().with_range(whole).with_skip(3);
        assert_eq!(cut(options).header, names(&header), "{format}");
        let options = SheetOptions::new().with_range(whole).with_skip(2);
        assert_eq!(
            cut(options).header,
            names(&["as of", "", "stray"]),
            "{format}"
        );
        let options = SheetOptions::new().with_skip(3);
        assert_eq!(cut(options).header, names(&header), "{format}: no range");

        // Inside the range: its first row is the first skipped.
        let options = SheetOptions::new().with_range(range("B3:E15")).with_skip(1);
        let read_it = cut(options);
        assert_eq!(read_it.header, names(&header[1..]), "{format}");
        assert_eq!(
            read_it.rows[0],
            (5, vec!["1".into(), "alpha".into()]),
            "{format}"
        );

        // Header row counts what the filter leaves.
        let options = SheetOptions::new()
            .with_range(range("A2:F15"))
            .with_header_row(1);
        assert_eq!(cut(options).header, names(&header), "{format}");
        let options = SheetOptions::new().with_skip(2).with_header_row(1);
        assert_eq!(cut(options).header, names(&header), "{format}");

        // With the filter off it counts them all.
        let delivered = SheetOptions::new().with_empty_rows_skipped(false);
        let options = delivered.with_range(range("A2:F15")).with_header_row(2);
        assert_eq!(cut(options).header, names(&header), "{format}");
        let options = delivered.with_range(range("A2:F15")).with_header_row(1);
        assert_eq!(
            cut(options).header,
            names(&["as of", "", "stray"]),
            "{format}"
        );
        let options = delivered.with_range(range("A2:F15"));
        assert_eq!(
            cut(options).header,
            Some(vec![]),
            "{format}: an empty first row"
        );

        // With no header declared, the rows are only discarded.
        let options = no_header().with_range(range("B4:C6")).with_header_row(1);
        assert_eq!(
            cut(options).rows,
            table(&[(5, &["1", "alpha"]), (6, &["2", "NA"])]),
            "{format}"
        );

        // Skip and header row past the range's end.
        let options = SheetOptions::new().with_range(range("B4:C6")).with_skip(3);
        let read_it = cut(options);
        assert_eq!(
            (read_it.header, read_it.rows),
            (Some(vec![]), vec![]),
            "{format}"
        );
        let options = SheetOptions::new()
            .with_range(range("B4:C6"))
            .with_header_row(3);
        let read_it = cut(options);
        assert_eq!(
            (read_it.header, read_it.rows),
            (Some(vec![]), vec![]),
            "{format}"
        );
        let options = no_header().with_skip(u32::MAX);
        assert_eq!(cut(options).rows, vec![], "{format}");
        let options = no_header().with_header_row(u32::MAX);
        assert_eq!(cut(options).rows, vec![], "{format}");

        // Part of a repeat.
        let options = SheetOptions::new().with_range(range("B8:C11")).with_skip(1);
        let read_it = cut(options);
        assert_eq!(read_it.header, names(&["3", "rep"]), "{format}");
        assert_eq!(
            read_it.rows,
            table(&[(10, &["3", "rep"]), (11, &["4", "-"])]),
            "{format}"
        );
        let options = no_header().with_range(range("B8:C11")).with_header_row(2);
        assert_eq!(
            cut(options).rows,
            table(&[(10, &["3", "rep"]), (11, &["4", "-"])]),
            "{format}"
        );
    }
}

#[test]
fn coordinates_stay_absolute() {
    for (format, package) in tables() {
        let options = no_header().with_range(range("C5:D6"));
        assert_eq!(
            read(&package, 0, options, 2).rows,
            table(&[(5, &["alpha", "1.5"]), (6, &["NA", "NA"])]),
            "{format}: ordinal 0 is the range's first column, and the line the sheet's row"
        );
        // A batch boundary inside a repeat keeps counting.
        let book = Workbook::from_slice(&package).expect("a workbook");
        let options = no_header().with_range(range("B9:C10")).with_batch_rows(1);
        let mut sheet = book.sheet(0, options, &[Column::text(1)]).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.line(0), 9, "{format}");
        assert_eq!(batch.text(0, 0), Ok(&b"rep"[..]), "{format}");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.line(0), 10, "{format}");
        assert_eq!(batch.text(0, 0), Ok(&b"rep"[..]), "{format}");
        assert!(sheet.read().expect("a read").is_none(), "{format}");
    }
    // And a failure's row.
    let found = read(&ranges_xlsx(), 1, no_header().with_range(range("A2:A3")), 1);
    assert_eq!(found.rows, table(&[(2, &["NA"])]));
    assert_eq!(found.failure, Some((FailureKind::SharedString, 3)));
}

#[test]
fn a_merged_or_missing_cell_inside_a_range_is_blank() {
    for (format, package) in tables() {
        let book = Workbook::from_slice(&package).expect("a workbook");
        // Row 12: B holds the value, C is merged into it, D holds a number. Row 11: C is missing.
        let options = no_header().with_range(range("C11:D12"));
        let mut sheet = book
            .sheet(0, options, &[Column::text(0), Column::text(1)])
            .expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.rows(), 2, "{format}");
        for row in 0..2 {
            let fault = batch.verdicts(0)[row].fault().expect("no value");
            assert_eq!(fault.reason, Reason::Empty, "{format}");
            assert_eq!(batch.origin(0, row), Origin::Cell, "{format}: a blank");
            assert_eq!(batch.detail(0, row), Some(Detail::Blank), "{format}");
        }
    }
}

#[test]
fn a_range_stops_at_its_end_and_a_later_failure_is_not_met() {
    let package = ranges_xlsx();
    let shown = |options: SheetOptions| read(&package, 1, options, 1);
    let found = shown(no_header());
    assert_eq!(found.failure, Some((FailureKind::SharedString, 3)));
    assert_eq!(found.rows.len(), 2);

    let found = shown(no_header().with_range(range("A1:A2")));
    assert_eq!(found.failure, None, "the broken row lies past the range");
    assert_eq!(
        found.rows,
        table(&[(1, &["Quarterly report"]), (2, &["NA"])])
    );

    let found = shown(no_header().with_range(range("A1:A3")));
    assert_eq!(found.failure, Some((FailureKind::SharedString, 3)));
    assert_eq!(found.rows.len(), 2, "every intact row before it");

    // Before the range, or skipped: not read. Discarded: read, as a filter has to.
    let found = shown(no_header().with_range(range("A4:A4")));
    assert_eq!(
        (found.failure, found.rows),
        (None, table(&[(4, &["footer"])]))
    );
    let found = shown(no_header().with_skip(3));
    assert_eq!(
        (found.failure, found.rows),
        (None, table(&[(4, &["footer"])]))
    );
    let found = shown(no_header().with_header_row(3));
    assert_eq!(found.failure, Some((FailureKind::SharedString, 3)));
    // Outside the columns: not read either.
    let sheet = "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>99</v></c><c r=\"B1\" t=\"str\"><v>b</v></c>\
<c r=\"C1\" t=\"s\"><v>99</v></c></row>";
    let broken = xlsx(&[("Data", sheet)], "", false, false);
    let found = read(&broken, 0, no_header().with_range(range("B1:B1")), 1);
    assert_eq!((found.failure, found.rows), (None, table(&[(1, &["b"])])));

    // The row past the range comes with no row on the last one to stop at: seeing its number is
    // enough, and the empty rows of the gap before it that are inside the range still come.
    let sheet = "<row r=\"1\"><c t=\"str\"><v>one</v></c></row>\
<row r=\"5\"><c t=\"s\"><v>99</v></c></row>";
    let gapped = xlsx(&[("Data", sheet)], "", false, false);
    let found = read(&gapped, 0, no_header().with_range(range("A1:A3")), 1);
    assert_eq!((found.failure, found.rows), (None, table(&[(1, &["one"])])));
    let delivered = no_header()
        .with_range(range("A1:A3"))
        .with_empty_rows_skipped(false);
    let found = read(&gapped, 0, delivered, 1);
    assert_eq!(found.failure, None);
    assert_eq!(
        found.rows,
        table(&[(1, &["one"]), (2, &["-"]), (3, &["-"])])
    );

    // A sheet whose XML breaks after the range's last row, inside the row after it. (An ODS is
    // read through to list its tables when it opens, so a break in one is met then, whatever the
    // range.)
    let sheet = "<row r=\"1\"><c t=\"str\"><v>one</v></c></row>\
<row r=\"2\"><c t=\"str\"><v>two</v></c></row>\
<row r=\"3\"><c t=\"str\"><v>three</v></c></row><!-- never closed";
    let cut = xlsx(&[("Data", sheet)], "", false, false);
    let found = read(&cut, 0, no_header().with_range(range("A1:A2")), 1);
    // And when `skip` takes the range's last row: the read ends there too, with nothing to deliver.
    let sheet = "<row r=\"1\"/><row r=\"2\"/><!-- never closed";
    let skipped = xlsx(&[("Data", sheet)], "", false, false);
    let all = no_header().with_range(range("A1:A2")).with_skip(2);
    let nothing = read(&skipped, 0, all, 1);
    assert_eq!((nothing.failure, nothing.rows), (None, table(&[])));
    assert_eq!(found.failure, None);
    assert_eq!(found.rows, table(&[(1, &["one"]), (2, &["two"])]));
    let found = read(&cut, 0, no_header(), 1);
    assert_eq!(
        found.failure.map(|failure| failure.0),
        Some(FailureKind::Xml)
    );
    assert_eq!(found.rows.len(), 3);
}

#[test]
fn a_cell_outside_the_columns_asks_for_no_room() {
    // A value of 2 MB in an ODS attribute, in a column the range leaves out.
    let big = "x".repeat(2_000_000);
    let table = format!(
        "<table:table table:name=\"Data\"><table:table-row>\
<table:table-cell office:value-type=\"string\" office:string-value=\"{big}\"/>\
<table:table-cell/>\
<table:table-cell office:value-type=\"string\"><text:p>kept</text:p></table:table-cell>\
</table:table-row></table:table>"
    );
    let package = ods(&table, false);
    let mut harness = Harness::new(&package, Sizing::One);
    harness.open().expect("it opens");
    let listed = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    let specs = [ColumnSpec::new(0, Door::Text.code(), 0)];
    let options = no_header().with_range(range("C1:C1"));
    let read = harness.read(&listed[0], options, &specs, 4);
    assert_eq!(read.rows.len(), 1);
    assert!(read.rows[0].1[0].starts_with("ok "), "{:?}", read.rows[0]);
    assert!(
        harness.arena.len() < 1 << 16,
        "{} bytes",
        harness.arena.len()
    );
}

#[test]
fn a_row_past_the_last_number_says_the_last_number() {
    let row = |repeat: u32, text: &str| {
        format!(
            "<table:table-row table:number-rows-repeated=\"{repeat}\">\
<table:table-cell office:value-type=\"string\"><text:p>{text}</text:p></table:table-cell>\
</table:table-row>"
        )
    };
    let table = format!(
        "<table:table table:name=\"Data\">{}{}{}</table:table>",
        row(u32::MAX, "a"),
        row(u32::MAX, "b"),
        row(1, "c"),
    );
    let package = ods(&table, false);
    let book = Workbook::from_slice(&package).expect("a workbook");
    // Skip every `a`, discard two `b`s: what's left starts past 2^32.
    let options = no_header()
        .with_skip(u32::MAX)
        .with_header_row(2)
        .with_batch_rows(4);
    let mut sheet = book.sheet(0, options, &[Column::text(0)]).expect("a sheet");
    let batch = sheet.read().expect("a read").expect("a batch");
    assert_eq!(batch.rows(), 4);
    for at in 0..4 {
        assert_eq!(batch.line(at), u32::MAX, "row {at} of the batch");
    }
    assert_eq!(batch.text(0, 0), Ok(&b"b"[..]));
}

/// Where a Numbers failure is met, with and without a range around it.
#[test]
fn a_numbers_failure_past_a_range() {
    use packages::{NCell, NSheet, NTable, NumbersShape, numbers};
    let sheets = [NSheet {
        name: "Data".to_string(),
        hidden: false,
        tables: vec![NTable {
            name: "T".to_string(),
            header_rows: 0,
            rows: vec![
                vec![NCell::Number(1.0)],
                vec![NCell::Number(2.0)],
                vec![NCell::Text("three".to_string())],
                vec![NCell::Number(4.0)],
            ],
        }],
    }];
    let lost = numbers(
        &sheets,
        &NumbersShape {
            lose_strings: true,
            tile_size: 2,
            ..NumbersShape::default()
        },
    );
    let found = read(&lost, 0, no_header(), 1);
    assert_eq!(
        found.rows.len(),
        2,
        "a text cell naming no string fails its row"
    );
    assert!(found.failure.is_some());
    let found = read(&lost, 0, no_header().with_range(range("A1:A2")), 1);
    assert_eq!(
        (found.failure, found.rows.len()),
        (None, 2),
        "and isn't met past a range"
    );

    // A chunk of the second tile's part that isn't one.
    let mut cut = numbers(
        &sheets,
        &NumbersShape {
            tile_size: 2,
            deflate: false,
            ..NumbersShape::default()
        },
    );
    let name = b"Index/Tables/Tile-";
    let mut locals: Vec<usize> = Vec::new();
    for at in 0..cut.len() - name.len() {
        if &cut[at..at + name.len()] == name && at >= 30 && cut[at - 30..at - 26] == *b"PK\x03\x04"
        {
            locals.push(at - 30);
        }
    }
    assert_eq!(locals.len(), 2);
    let at = *locals.iter().max().expect("a tile");
    let word =
        |offset: usize| usize::from(u16::from_le_bytes([cut[at + offset], cut[at + offset + 1]]));
    let data = at + 30 + word(26) + word(28);
    assert_eq!(cut[data], 0, "a chunk begins");
    cut[data] = 0xFF;
    // Opening walks every part's chunks to index their objects: it's met before any range.
    let error = Workbook::from_slice(&cut).err().expect("no workbook");
    assert!(
        matches!(&error, Error::Structure(failure) if failure.kind == FailureKind::Iwa),
        "{error:?}"
    );
}

#[test]
fn a_discarded_row_keeps_no_room() {
    // Rows `header_row` reads and discards don't add up in the arena, however many there are.
    let mut sheet = String::new();
    for number in 1..=2000 {
        sheet.push_str(&format!(
            "<row r=\"{number}\"><c t=\"inlineStr\"><is><t>row {number}</t></is></c></row>"
        ));
    }
    let package = xlsx(&[("Data", &sheet)], "", false, false);
    let mut harness = Harness::new(&package, Sizing::One);
    harness.open().expect("it opens");
    let listed = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    let specs = [ColumnSpec::new(0, Door::Text.code(), 0)];
    let read = harness.read(&listed[0], no_header().with_header_row(1999), &specs, 16);
    assert_eq!(read.rows.len(), 1);
    assert_eq!(read.rows[0].0, 2000);
    assert!(harness.arena.len() < 1024, "{} bytes", harness.arena.len());
}

#[test]
fn a_cell_that_is_not_finite_says_so() {
    use packages::{NCell, NSheet, NTable, NumbersShape, numbers};
    // A Numbers double holds whatever bits it holds. XLSX and ODS say a number in text, and text
    // too large for one goes to the door as text.
    let package = numbers(
        &[NSheet {
            name: "Data".to_string(),
            hidden: false,
            tables: vec![NTable {
                name: "T".to_string(),
                header_rows: 0,
                rows: vec![vec![
                    NCell::Currency(f64::NAN),
                    NCell::Currency(f64::INFINITY),
                ]],
            }],
        }],
        &NumbersShape::default(),
    );
    let book = Workbook::from_slice(&package).expect("a workbook");
    let plan = [Column::new(0, Door::F64), Column::new(1, Door::F64)];
    let mut sheet = book.sheet(0, no_header(), &plan).expect("a sheet");
    let batch = sheet.read().expect("a read").expect("a batch");
    let first = batch.verdicts(0)[0].fault().expect("no value");
    assert_eq!(first.reason, Reason::Malformed);
    let second = batch.verdicts(1)[0].fault().expect("no value");
    assert_eq!(second.reason, Reason::OutOfRange);
    assert_eq!(batch.detail(0, 0), Some(Detail::NotFinite));
    assert_eq!(batch.detail(1, 0), Some(Detail::NotFinite));
}

#[test]
fn a_numbers_cell_outside_a_range_is_not_read() {
    use packages::{NCell, NSheet, NTable, NumbersShape, cell_storage, numbers};
    // Two columns whose cell is one with a storage version the core doesn't read.
    let bad = cell_storage(4, 3, None, None, None, &[]);
    let package = numbers(
        &[NSheet {
            name: "Data".to_string(),
            hidden: false,
            tables: vec![NTable {
                name: "T".to_string(),
                header_rows: 0,
                rows: vec![vec![NCell::Text("one".to_string())]],
            }],
        }],
        &NumbersShape {
            raw_row: Some((bad, vec![0u8, 0, 0, 0])),
            ..NumbersShape::default()
        },
    );
    let found = read(&package, 0, no_header(), 1);
    assert!(found.failure.is_some(), "the cell is one the core refuses");
    let beside = Range::new(0, 3, 0, 0).expect("a range");
    let found = read(&package, 0, no_header().with_range(beside), 1);
    assert_eq!(found.failure, None, "and it isn't asked to read it here");
}

#[test]
fn a_numbers_row_outside_a_range_is_not_read() {
    use packages::{NCell, NSheet, NTable, NumbersShape, cell_storage, numbers};
    // The first row's only cell has a storage version the core doesn't read.
    let bad = cell_storage(4, 3, None, None, None, &[]);
    let offsets = [0u8, 0].to_vec();
    let package = numbers(
        &[NSheet {
            name: "Data".to_string(),
            hidden: false,
            tables: vec![NTable {
                name: "T".to_string(),
                header_rows: 0,
                rows: vec![
                    vec![NCell::Text("one".to_string())],
                    vec![NCell::Text("two".to_string())],
                ],
            }],
        }],
        &NumbersShape {
            raw_row: Some((bad, offsets)),
            ..NumbersShape::default()
        },
    );
    let found = read(&package, 0, no_header(), 1);
    assert_eq!(
        found.failure.map(|failure| failure.0),
        Some(FailureKind::Iwa)
    );
    let found = read(&package, 0, no_header().with_range(range("A2:A2")), 1);
    assert_eq!((found.failure, found.rows), (None, table(&[(2, &["two"])])));
    let found = read(&package, 0, no_header().with_skip(1), 1);
    assert_eq!((found.failure, found.rows), (None, table(&[(2, &["two"])])));
}

#[test]
fn tokens_match_text_cells_only() {
    for (format, package) in tables() {
        let book = Workbook::from_slice(&package).expect("a workbook");
        // Every text and every said-as-text value of the rows below, as a token.
        let options = no_header().with_range(range("B5:E11")).with_tokens(&[
            "NA",
            "-",
            "1",
            "1.5",
            "true",
            "#N/A",
            "2025-12-09",
        ]);
        let plan = [
            Column::i32(0),
            Column::text(1),
            Column::text(2),
            Column::text(3),
        ];
        let mut sheet = book.sheet(0, options, &plan).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.line(0), 5, "{format}");
        // Row 5: a number whose text would be a token, through a numeric door and the text door.
        assert!(batch.verdicts(0)[0].is_ok(), "{format}: 1 is a number");
        assert_eq!(batch.i32(0)[0], 1, "{format}");
        assert_eq!(batch.origin(0, 0), Origin::Cell, "{format}");
        assert_eq!(
            batch.text(2, 0),
            Ok(&b"1.5"[..]),
            "{format}: 1.5 is a number"
        );
        assert_eq!(batch.origin(2, 0), Origin::Cell, "{format}");
        assert_ne!(batch.origin(3, 0), Origin::Token, "{format}: a date");
        // Row 6: text.
        assert_eq!(batch.line(1), 6, "{format}");
        for column in [1, 2] {
            let fault = batch.verdicts(column)[1].fault().expect("a token");
            assert_eq!(fault.reason, Reason::Empty, "{format}");
            assert_eq!(batch.origin(column, 1), Origin::Token, "{format}");
            assert_eq!(batch.detail(column, 1), None, "{format}");
        }
        // Rows 8 to 10: a boolean whose text would be a token.
        assert_eq!(batch.line(2), 8, "{format}");
        assert_eq!(batch.text(3, 2), Ok(&b"true"[..]), "{format}");
        // Row 11: `-`, and an error cell, and a text door's rendering of it isn't asked.
        assert_eq!(batch.line(5), 11, "{format}");
        assert_eq!(batch.origin(2, 5), Origin::Token, "{format}");
        assert_ne!(batch.origin(3, 5), Origin::Token, "{format}: an error cell");

        // A column's own list replaces the sheet's, and an empty one means none.
        let plan = [Column::text(1).tokens(&[]), Column::text(2).tokens(&["2"])];
        let options = no_header().with_range(range("B6:D6")).with_tokens(&["NA"]);
        let mut sheet = book.sheet(0, options, &plan).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.text(0, 0), Ok(&b"NA"[..]), "{format}");
        assert_eq!(batch.text(1, 0), Ok(&b"NA"[..]), "{format}");
    }
    // A blank never meets the empty token; an empty text cell does.
    let sheet = "<row r=\"1\"><c r=\"A1\" t=\"str\"><v></v></c><c r=\"B1\"/><c r=\"D1\" t=\"str\"><v>x</v></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    let book = Workbook::from_slice(&package).expect("a workbook");
    let plan = [
        Column::text(0),
        Column::text(1),
        Column::text(3),
        Column::text(4),
    ];
    let mut opened = book
        .sheet(0, no_header().with_tokens(&[""]), &plan)
        .expect("a sheet");
    let batch = opened.read().expect("a read").expect("a batch");
    assert_eq!(batch.origin(0, 0), Origin::Token, "an empty text cell");
    assert_eq!(batch.origin(1, 0), Origin::Cell, "a blank");
    assert_eq!(batch.detail(1, 0), Some(Detail::Blank));
    assert_eq!(batch.origin(2, 0), Origin::Cell);
    assert_eq!(batch.text(2, 0), Ok(&b"x"[..]));
    assert_eq!(batch.origin(3, 0), Origin::Padded, "past the row's end");
    assert_eq!(batch.detail(3, 0), None);
}

#[test]
fn notes_carry_detail_for_text_and_typed_cells() {
    let format = NumFormat::INVARIANT.validate().expect("the invariant");
    let text = hypercast::detailed::cast_i32("12a", &format)
        .expect_err("not a number")
        .detail;
    let typed = hypercast::detailed::i32_from_f64(1.5)
        .expect_err("not whole")
        .detail;
    assert_eq!(typed, Detail::NotWhole);
    for (name, package) in tables() {
        let book = Workbook::from_slice(&package).expect("a workbook");
        let options = every_row().with_range(range("B5:F12"));
        let plan = [
            Column::i32(2),
            Column::i32(3),
            Column::i32(1),
            Column::i32(4),
        ];
        let mut sheet = book.sheet(0, options, &plan).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        let line = |number: u32| {
            (0..batch.rows())
                .find(|&row| batch.line(row) == number)
                .expect("a row")
        };
        let fault = |column: usize, row: usize| batch.verdicts(column)[row].fault();
        // The typed twin: 1.5 in row 5.
        assert_eq!(
            fault(0, line(5)).expect("a fault").reason,
            Reason::Malformed
        );
        assert_eq!(batch.detail(0, line(5)), Some(Detail::NotWhole), "{name}");
        assert_eq!(batch.origin(0, line(5)), Origin::Cell, "{name}");
        // The text door: 12a in row 8.
        assert_eq!(batch.detail(0, line(8)), Some(text), "{name}");
        // A kind no door is asked about: a boolean through an integer door.
        assert_eq!(
            fault(1, line(8)).expect("a fault").reason,
            Reason::Malformed
        );
        assert_eq!(batch.detail(1, line(8)), None, "{name}");
        // A cell that cast has no note.
        assert!(fault(2, line(6)).is_some(), "{name}: NA isn't an integer");
        assert_eq!(
            batch.detail(2, line(6)),
            hypercast::detailed::cast_i32("NA", &format)
                .err()
                .map(|failure| failure.detail),
            "{name}"
        );
        // A blank inside the row's cells, past them, and a whole empty row.
        assert_eq!(batch.detail(2, line(11)), Some(Detail::Blank), "{name}");
        assert_eq!(batch.origin(2, line(11)), Origin::Cell, "{name}");
        assert_eq!(
            batch.origin(3, line(6)),
            Origin::Padded,
            "{name}: past row 6's end"
        );
        assert_eq!(batch.detail(3, line(6)), None, "{name}");
        assert_eq!(
            batch.origin(0, line(7)),
            Origin::Padded,
            "{name}: an empty row"
        );
        assert_eq!(batch.detail(0, line(7)), None, "{name}");
    }
}

#[test]
fn date_options_reach_text_cells_and_leave_typed_ones() {
    let compact = DateOptions::from_flags(DateOptions::COMPACT_DATE);
    for (name, package) in tables() {
        let book = Workbook::from_slice(&package).expect("a workbook");
        let options = no_header().with_range(range("E5:E6"));
        let plain = [Column::date(0)];
        let mut sheet = book.sheet(0, options, &plain).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        let december = Date {
            year: 2025,
            month: 12,
            day: 9,
        };
        assert_eq!(batch.get::<Date>(0, 0), Ok(december), "{name}: typed");
        let fault = batch.verdicts(0)[1]
            .fault()
            .expect("no compact date without its flag");
        assert_eq!(fault.reason, Reason::Malformed, "{name}");

        let declared = [Column::date(0).date_options(compact, None)];
        let mut sheet = book.sheet(0, options, &declared).expect("a sheet");
        let batch = sheet.read().expect("a read").expect("a batch");
        assert_eq!(batch.get::<Date>(0, 0), Ok(december), "{name}: typed");
        assert_eq!(
            batch.get::<Date>(0, 1),
            Ok(Date {
                year: 2026,
                month: 1,
                day: 7
            }),
            "{name}: text"
        );
    }
}

/// The same read with buffers that start at one element and with room to spare, both through
/// `fill_with`, and `fill`'s answer beside them.
fn replay(package: &[u8], options: SheetOptions, width: u32, max_rows: usize) -> usize {
    let specs = support::every_door(width);
    let open = |sizing: Sizing| {
        let mut harness = Harness::new(package, sizing);
        harness.open().expect("it opens");
        let sheets = harness.sheets().expect("its sheets");
        harness.load().expect("its tables");
        (harness, sheets)
    };
    let mut rows = 0;
    let (mut one, sheets) = open(Sizing::One);
    let (mut roomy, _) = open(Sizing::Roomy);
    let (mut plain, _) = open(Sizing::Stingy);
    one.through_with = Some(Vec::new());
    roomy.through_with = Some(Vec::new());
    for listed in &sheets {
        let ours = one.read(listed, options, &specs, max_rows);
        let theirs = roomy.read(listed, options, &specs, max_rows * 3 + 1);
        let bare = plain.read(listed, options, &specs, max_rows);
        assert_eq!(ours, theirs, "{options:?}");
        assert_eq!(
            (&ours.header, &ours.rows, ours.failed),
            (&bare.header, &bare.rows, bare.failed),
            "fill_with and fill, {options:?}"
        );
        assert!(
            ours.notes
                .iter()
                .flatten()
                .all(|note| note.origin != 0xAAAA)
        );
        assert!(ours.widths.iter().all(|&width| width != 0xAAAA_AAAA));
        rows += ours.rows.len();
    }
    rows
}

#[test]
fn every_option_reads_the_same_however_little_room_it_has() {
    let mut seen = 0;
    for (_, package) in tables() {
        for text in [
            "B4:E12",
            "A1:F15",
            "B9:C9",
            "C15:E15",
            "A100:B200",
            "B7:D11",
        ] {
            let base = SheetOptions::new().with_range(range(text));
            for options in [
                base,
                base.with_header(false),
                base.with_empty_rows_skipped(false),
                base.with_skip(2),
                base.with_header_row(1),
                base.with_skip(1).with_header_row(1).with_header(false),
                base.with_skip(1)
                    .with_header_row(1)
                    .with_empty_rows_skipped(false),
            ] {
                for max_rows in [1, 3, 64] {
                    seen += replay(&package, options, 6, max_rows);
                }
            }
        }
    }
    assert!(seen > 500, "{seen} rows compared");
}

#[test]
fn every_corpus_package_reads_the_same_through_fill_and_fill_with() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("the corpus")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| {
            ["xlsx", "xlsm", "ods", "numbers"]
                .iter()
                .any(|extension| path.extension().is_some_and(|found| found == *extension))
        })
        .collect();
    files.sort();
    let mut compared = 0;
    for path in files {
        let bytes = std::fs::read(&path).expect("a package");
        if Workbook::from_slice(&bytes).is_err() {
            continue;
        }
        let specs = support::every_door(12);
        let open = |with: bool| {
            let mut harness = Harness::new(&bytes, Sizing::Roomy);
            harness.open().expect("it opens");
            let sheets = harness.sheets().expect("its sheets");
            harness.load().expect("its tables");
            harness.row_limit = 400;
            if with {
                harness.through_with = Some(Vec::new());
            }
            (harness, sheets)
        };
        let (mut plain, sheets) = open(false);
        let (mut with, _) = open(true);
        for listed in &sheets {
            let options = SheetOptions::new();
            let ours = with.read(listed, options, &specs, 64);
            let theirs = plain.read(listed, options, &specs, 64);
            assert_eq!(
                (&ours.header, &ours.rows, ours.failed, ours.truncated),
                (
                    &theirs.header,
                    &theirs.rows,
                    theirs.failed,
                    theirs.truncated
                ),
                "{path:?}"
            );
            compared += ours.rows.len();
        }
    }
    assert!(compared > 150, "{compared} rows compared");
}

/// The cases `examples/corpus.rs` lists by hand over `ranges.*` and writes over `uncached.*`,
/// replayed through the core with every buffer starting at one element, each grown as a binding
/// grows it, through the plan and tokens they declare: the corpus's rows, numbers, reasons, details
/// and origins, then the values of a read with room to spare.
#[test]
fn the_corpus_ranges_and_uncached_replay_from_buffers_of_one_element() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook.json");
    let text = std::fs::read_to_string(path).expect("the corpus");
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).expect("JSON");
    let mut replayed = 0;
    for case in &cases {
        let name = case["name"].as_str().expect("a name");
        if !name.starts_with("ranges") && !name.starts_with("uncached") {
            continue;
        }
        let file = format!(
            "{}/../corpus/{}",
            env!("CARGO_MANIFEST_DIR"),
            case["file"].as_str().expect("a file")
        );
        let bytes = std::fs::read(&file).expect("the package");
        let options = corpus::sheet_options_of(&case["options"]);
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("a plan")
            .iter()
            .map(corpus::column_of)
            .collect();
        let specs: Vec<ColumnSpec> = plan
            .iter()
            .enumerate()
            .map(|(index, column)| column.spec(index).expect("a spec"))
            .collect();
        let with: Vec<ColumnSpecWith> = plan
            .iter()
            .enumerate()
            .map(|(index, column)| column.spec_with(index).expect("a spec"))
            .collect();
        let tokens = case["options"].get("tokens").map_or_else(Vec::new, |list| {
            let mut packed = Vec::new();
            for token in corpus::strings_of(list) {
                packed.extend_from_slice(&(token.len() as u32).to_le_bytes());
                packed.extend_from_slice(token.as_bytes());
            }
            packed
        });
        let index = case["sheet"].as_u64().expect("a sheet") as usize;
        let reads = [Sizing::One, Sizing::Roomy].map(|sizing| {
            let mut harness = Harness::new(&bytes, sizing);
            harness.open().expect("it opens");
            let listed = harness.sheets().expect("its sheets");
            harness.load().expect("its tables");
            harness.through_with = Some(tokens.clone());
            harness.plan_with = Some(with.clone());
            let read = harness.read(&listed[index], options, &specs, 2);
            (read, harness.refusals)
        });
        let [(one, refusals), (roomy, _)] = reads;
        assert!(
            refusals > 0,
            "{name}: a buffer of one element is never enough"
        );
        assert_eq!(one, roomy, "{name}");
        let numbers: Vec<u32> = one.rows.iter().map(|row| row.0).collect();
        assert_eq!(case["numbers"], serde_json::json!(numbers), "{name}");
        assert_eq!(one.failed, case.get("failure").is_some(), "{name}");
        let header = one.header.as_ref().map(|names| {
            names
                .iter()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect::<Vec<_>>()
        });
        let expected = case["header"].as_array().map(|names| {
            names
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        });
        assert_eq!(header, expected, "{name}");
        for (at, row) in case["rows"].as_array().expect("rows").iter().enumerate() {
            for (column, cell) in row.as_array().expect("a row").iter().enumerate() {
                let said = &one.rows[at].1[column];
                let reason = match cell["expect"].as_str().expect("expect") {
                    "ok" => "ok ",
                    "empty" => "empty ",
                    "malformed" => "fault 2 ",
                    "out_of_range" => "fault 3 ",
                    other => panic!("{name}: {other}"),
                };
                assert!(
                    said.starts_with(reason),
                    "{name}, row {at}: {said} for {cell}"
                );
                let note = one.notes[at][column];
                let detail = cell
                    .get("detail")
                    .map(|name| name.as_str().expect("a name"));
                assert_eq!(
                    Detail::from_code(u32::from(note.detail)).map(|detail| detail.name()),
                    detail,
                    "{name}, row {at}, column {column}"
                );
                let origin = match cell.get("origin").and_then(|origin| origin.as_str()) {
                    None => CellNote::ORIGIN_CELL,
                    Some("token") => CellNote::ORIGIN_TOKEN,
                    Some("padded") => CellNote::ORIGIN_PADDED,
                    Some("uncached") => CellNote::ORIGIN_UNCACHED,
                    Some(other) => panic!("{name}: {other}"),
                };
                assert_eq!(note.origin, origin, "{name}, row {at}, column {column}");
            }
        }
        replayed += 1;
    }
    assert!(replayed >= 77, "{replayed} replayed");
}

#[test]
fn row_widths_are_the_rows_own_inside_the_range() {
    // One plan column: a width doesn't stop at what the plan reads.
    let specs = [ColumnSpec::new(0, Door::Text.code(), 0)];
    for (name, package) in tables() {
        let mut harness = Harness::new(&package, Sizing::One);
        harness.open().expect("it opens");
        let sheets = harness.sheets().expect("its sheets");
        harness.load().expect("its tables");
        harness.through_with = Some(Vec::new());
        let options = every_row().with_range(range("B5:E15"));
        let read = harness.read(&sheets[0], options, &specs, 4);
        let found: Vec<(u32, u32)> = read
            .rows
            .iter()
            .zip(&read.widths)
            .map(|(row, &width)| (row.0, width))
            .collect();
        assert_eq!(
            found,
            [
                (5, 4),
                (6, 4),
                (7, 0),
                (8, 4),
                (9, 4),
                (10, 4),
                (11, 4),
                (12, 3),
                (13, 0),
                (14, 0),
                (15, 3),
            ],
            "{name}"
        );
    }
}

/// A harness at the sheet's first row, for the exports' tests.
fn positioned(package: &[u8]) -> (Harness<'_>, Vec<u8>, u32) {
    let mut harness = Harness::new(package, Sizing::Roomy);
    harness.open().expect("it opens");
    let sheets = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    harness.row = vec![Default::default(); 4];
    let (part, index) = (sheets[0].part.clone(), sheets[0].index);
    (harness, part, index)
}

fn raw_options() -> RawSheetOptions {
    RawSheetOptions {
        size: RawSheetOptions::SIZE,
        has_header: 0,
        skip_empty_rows: 1,
        skip: 0,
        header_row: 0,
        first_row: 0,
        first_column: 0,
        last_row: 0,
        last_column: 0,
    }
}

#[test]
fn sheet_with_refuses_before_it_writes() {
    let package = ranges_xlsx();
    let (mut harness, part, index) = positioned(&package);
    let mut position = |options: *const RawSheetOptions, out: &mut Filled| {
        // SAFETY: the state was opened over this container; `options` is live or null.
        unsafe {
            hypertabular_workbook_sheet_with(
                &mut *harness.state,
                package.as_ptr(),
                package.len(),
                part.as_ptr(),
                part.len(),
                index,
                options,
                out,
            )
        }
    };
    let junk = Filled {
        rows: 0xAA,
        ..Filled::default()
    };
    let refuses =
        |options: RawSheetOptions,
         position: &mut dyn FnMut(*const RawSheetOptions, &mut Filled) -> i32| {
            let mut out = junk;
            assert_eq!(position(&options, &mut out), ERR_CONTRACT, "{options:?}");
            assert_eq!(out.rows, 0xAA, "{options:?} wrote");
        };
    let good = raw_options();
    refuses(RawSheetOptions { size: 0, ..good }, &mut position);
    refuses(
        RawSheetOptions {
            size: RawSheetOptions::SIZE + 1,
            ..good
        },
        &mut position,
    );
    refuses(
        RawSheetOptions {
            size: RawSheetOptions::SIZE - 1,
            ..good
        },
        &mut position,
    );
    refuses(
        RawSheetOptions {
            first_row: 5,
            last_row: 4,
            ..good
        },
        &mut position,
    );
    refuses(
        RawSheetOptions {
            first_column: 5,
            last_column: 4,
            ..good
        },
        &mut position,
    );
    let mut out = junk;
    assert_eq!(position(null(), &mut out), ERR_CONTRACT);
    assert_eq!(out.rows, 0xAA);
    let mut out = junk;
    assert_eq!(position(&good, &mut out), OK);
    assert_eq!(out.rows, 0);
    // One side of a range alone is no limit on the other, and never inverted.
    for (first_row, last_row) in [(5, 0), (0, 5), (5, 5)] {
        let options = RawSheetOptions {
            first_row,
            last_row,
            ..good
        };
        let mut out = junk;
        assert_eq!(position(&options, &mut out), OK);
    }
}

#[test]
fn fill_with_refuses_before_it_writes() {
    let package = ranges_xlsx();
    let (mut harness, part, index) = positioned(&package);
    let mut out = Filled::default();
    let options = raw_options();
    // SAFETY: as above.
    let code = unsafe {
        hypertabular_workbook_sheet_with(
            &mut *harness.state,
            package.as_ptr(),
            package.len(),
            part.as_ptr(),
            part.len(),
            index,
            &options,
            &mut out,
        )
    };
    assert_eq!(code, OK);
    let mut values = [0x55u8; 32];
    let mut verdicts = [CellVerdict {
        offset: 0x55,
        len: 0x55,
        reason: 0x55,
    }; 2];
    let mut notes = [CellNote {
        detail: 0x55,
        origin: 0x55,
    }; 2];
    let mut buffers = hypertabular::kernel::abi::BuffersWith::empty();
    buffers.window = harness.window.as_mut_ptr();
    buffers.window_cap = harness.window.len();
    buffers.arena = harness.arena.as_mut_ptr();
    buffers.arena_cap = harness.arena.len();
    buffers.cells = harness.cells.as_mut_ptr();
    buffers.cells_cap = harness.cells.len();
    buffers.row = harness.row.as_mut_ptr();
    buffers.row_cap = harness.row.len();
    buffers.strings = harness.strings.as_ptr();
    buffers.strings_len = harness.strings.len();
    buffers.table = harness.table.as_ptr();
    buffers.table_len = harness.table.len();
    buffers.kinds = harness.kinds.as_ptr();
    buffers.kinds_len = harness.kinds.len();
    let spec = ColumnSpecWith::new(0, Door::Text.code(), 0);
    let column = ColumnBufferWith {
        size: ColumnBufferWith::SIZE,
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
        notes: notes.as_mut_ptr(),
    };
    let state: *mut _ = &mut *harness.state;
    let fill = |spec: ColumnSpecWith,
                column: ColumnBufferWith,
                buffers: hypertabular::kernel::abi::BuffersWith,
                out: &mut Filled| {
        // SAFETY: every array holds two rows.
        unsafe {
            hypertabular_workbook_fill_with(
                state,
                package.as_ptr(),
                package.len(),
                &spec,
                &column,
                1,
                2,
                &buffers,
                out,
            )
        }
    };
    let mut undefined = spec;
    let mut format = NumFormat::INVARIANT;
    format.flags |= 1 << 30;
    undefined.format = hypercast::RawNumFormat::from(format);
    let short = [1u8, 0, 0];
    let mut torn = spec;
    torn.tokens = short.as_ptr();
    torn.tokens_len = short.len();
    let mut far = spec;
    far.ordinal = 99;
    let mut bad_door = spec;
    bad_door.door = 0;
    let mut bad_pivot = spec;
    bad_pivot.date = hypercast::RawDateOptions {
        size: hypercast::RawDateOptions::SIZE,
        flags: 0,
        pivot: 5,
    };
    let stale = |word: u32| {
        let mut format = NumFormat::INVARIANT;
        format.flags = word;
        ColumnSpecWith {
            format: hypercast::RawNumFormat::from(format),
            ..spec
        }
    };
    let mut unsized_stale = stale(351);
    unsized_stale.format.size = 0;
    for (name, spec) in [
        ("size", ColumnSpecWith { size: 1, ..spec }),
        ("flags", undefined),
        ("351", stale(351)),
        ("863", stale(863)),
        ("351 with no size", unsized_stale),
        ("tokens", torn),
        ("ordinal", far),
        ("door", bad_door),
        ("pivot", bad_pivot),
    ] {
        let mut out = Filled {
            rows: 0xAA,
            ..Filled::default()
        };
        assert_eq!(
            fill(spec, column, buffers, &mut out),
            ERR_CONTRACT,
            "{name}"
        );
        assert_eq!(out.rows, 0xAA, "{name} wrote");
        assert_eq!(values, [0x55; 32], "{name} wrote");
    }
    let mut out = Filled {
        rows: 0xAA,
        ..Filled::default()
    };
    let wrong = ColumnBufferWith { size: 3, ..column };
    assert_eq!(fill(spec, wrong, buffers, &mut out), ERR_CONTRACT);
    let bad = hypertabular::kernel::abi::BuffersWith { size: 3, ..buffers };
    assert_eq!(fill(spec, column, bad, &mut out), ERR_CONTRACT);
    let mut torn_list = buffers;
    torn_list.tokens = short.as_ptr();
    torn_list.tokens_len = short.len();
    assert_eq!(fill(spec, column, torn_list, &mut out), ERR_CONTRACT);
    assert_eq!(out.rows, 0xAA);
    assert_eq!(values, [0x55; 32]);
    assert_eq!(fill(spec, column, buffers, &mut out), OK);
    assert_eq!(out.rows, 2);
}

/// `1,23,456`, which only a read declaring Indian grouping takes, with these flags, through the 1.0
/// fill.
fn one_point_oh(flags: u32) -> (i32, u8, i32) {
    let sheet = "<row><c t=\"str\"><v>1,23,456</v></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    let mut harness = Harness::new(&package, Sizing::Roomy);
    harness.open().expect("it opens");
    let sheets = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    harness.row = vec![Default::default(); 1];
    let mut format = NumFormat::INVARIANT;
    format.flags = flags;
    let mut spec = ColumnSpec::new(0, Door::I32.code(), 0);
    spec.format = hypercast::RawNumFormat::from(format);
    let mut out = Filled::default();
    assert_eq!(
        hypertabular::kernel::workbook::rows::sheet(
            &mut harness.state,
            &package,
            &sheets[0].part,
            sheets[0].index,
            false,
            true,
            &mut out
        ),
        OK
    );
    let mut value = 0i32;
    let mut verdict = CellVerdict::default();
    let column = ColumnBuffer {
        values: (&raw mut value).cast(),
        verdicts: &mut verdict,
    };
    let buffers = hypertabular::kernel::abi::Buffers {
        window: harness.window.as_mut_ptr(),
        window_cap: harness.window.len(),
        arena: harness.arena.as_mut_ptr(),
        arena_cap: harness.arena.len(),
        cells: harness.cells.as_mut_ptr(),
        cells_cap: harness.cells.len(),
        row: harness.row.as_mut_ptr(),
        row_cap: harness.row.len(),
        strings: harness.strings.as_ptr(),
        strings_len: harness.strings.len(),
        table: harness.table.as_ptr(),
        table_len: harness.table.len(),
        kinds: harness.kinds.as_ptr(),
        kinds_len: harness.kinds.len(),
    };
    // SAFETY: every array holds the one row.
    let code = unsafe {
        hypertabular_workbook_fill(
            &mut *harness.state,
            package.as_ptr(),
            package.len(),
            &spec,
            &column,
            1,
            1,
            &buffers,
            &mut out,
        )
    };
    (code, verdict.reason as u8, value)
}

#[test]
fn the_1_0_fill_reads_flags_as_1_0_did() {
    let western = one_point_oh(NumFormat::ALL);
    assert_eq!(western.0, OK);
    assert_eq!(western.1, 2, "Western grouping refuses 1,23,456");
    assert_eq!(
        one_point_oh(NumFormat::ALL | (1 << 30)),
        western,
        "an undefined bit"
    );
    for (flags, name) in [
        (351, "the retired bit"),
        (863, "policy 11"),
        (607, "Indian"),
    ] {
        assert_eq!(
            one_point_oh(flags),
            western,
            "{name}: every format is Western"
        );
    }
}

#[test]
fn a_range_says_why_it_was_refused() {
    let error = Range::parse("F200:A5").expect_err("inverted");
    assert!(matches!(error, Error::Range(_)), "{error:?}");
    assert!(error.to_string().contains("F200:A5"), "{error}");
}

// Formulas with no cached result.

/// One cell as the tests below write it: its text through the text door, or what it was told.
fn told(batch: &hypertabular::Batch<'_>, column: usize, row: usize) -> String {
    match batch.text(column, row) {
        Ok(text) => format!("{:?}", String::from_utf8_lossy(text)),
        Err(fault) => {
            assert_eq!(fault.reason, Reason::Empty, "column {column}, row {row}");
            match batch.origin(column, row) {
                Origin::Uncached => "uncached".to_string(),
                Origin::Padded => "padded".to_string(),
                Origin::Token => "token".to_string(),
                Origin::Cell => "blank".to_string(),
            }
        }
    }
}

/// Every row's number and what column B (ordinal 1) of it was told.
fn column_b(package: &[u8], options: SheetOptions) -> Vec<(u32, String)> {
    let book = Workbook::from_slice(package).expect("a workbook");
    let plan = [Column::text(1)];
    let mut sheet = book.sheet(0, options, &plan).expect("a sheet");
    let mut found = Vec::new();
    while let Some(batch) = sheet.read().expect("a batch") {
        for row in 0..batch.rows() {
            found.push((batch.line(row), told(&batch, 0, row)));
        }
    }
    found
}

/// What `column_b` should find of `rows`, listed by the text of each.
fn numbered(rows: &[(u32, &str)]) -> Vec<(u32, String)> {
    rows.iter()
        .map(|&(number, text)| (number, text.to_string()))
        .collect()
}

/// A sheet's rows in one column as the 1.0 `hypertabular_workbook_fill` reads them, through
/// `door`: each row's number, and its reason and text.
fn through_1_0(package: &[u8], door: Door, skip_empty: bool) -> Vec<(u32, u32, Vec<u8>)> {
    let mut harness = Harness::new(package, Sizing::Roomy);
    harness.open().expect("it opens");
    let sheets = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    harness.row = vec![Default::default(); 2];
    let spec = ColumnSpec::new(1, door.code(), 0);
    let mut out = Filled::default();
    assert_eq!(
        hypertabular::kernel::workbook::rows::sheet(
            &mut harness.state,
            package,
            &sheets[0].part,
            sheets[0].index,
            false,
            skip_empty,
            &mut out
        ),
        OK
    );
    const ROWS: usize = 64;
    let mut values = vec![0u8; ROWS * support::size_of_door(door)];
    let mut verdicts = vec![CellVerdict::default(); ROWS];
    let column = ColumnBuffer {
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
    };
    harness.cells = vec![Default::default(); ROWS * 2];
    let buffers = hypertabular::kernel::abi::Buffers {
        window: harness.window.as_mut_ptr(),
        window_cap: harness.window.len(),
        arena: harness.arena.as_mut_ptr(),
        arena_cap: harness.arena.len(),
        cells: harness.cells.as_mut_ptr(),
        cells_cap: harness.cells.len(),
        row: harness.row.as_mut_ptr(),
        row_cap: harness.row.len(),
        strings: harness.strings.as_ptr(),
        strings_len: harness.strings.len(),
        table: harness.table.as_ptr(),
        table_len: harness.table.len(),
        kinds: harness.kinds.as_ptr(),
        kinds_len: harness.kinds.len(),
    };
    // SAFETY: every array holds `ROWS` rows.
    let code = unsafe {
        hypertabular_workbook_fill(
            &mut *harness.state,
            package.as_ptr(),
            package.len(),
            &spec,
            &column,
            1,
            ROWS,
            &buffers,
            &mut out,
        )
    };
    assert_eq!(code, OK);
    assert!((out.rows as usize) < ROWS, "the sheet ended");
    (0..out.rows as usize)
        .map(|row| {
            let number = harness.cells[row * 2 + 1].offset;
            let span = harness.cells[row * 2];
            let text = hypertabular::kernel::workbook::cell::resolve(
                span,
                None,
                &harness.arena,
                &harness.strings,
            )
            .to_vec();
            (number, verdicts[row].reason, text)
        })
        .collect()
}

#[test]
fn uncached_formula_is_marked() {
    let every = every_row();
    for (body, kept) in [
        ("<f>A1+1</f>", "no value"),
        ("<f>A1+1</f><v />", "openpyxl's, over the standard library"),
        ("<f>A1+1</f><v></v>", "openpyxl's, over lxml"),
        (
            "<f t=\"shared\" si=\"0\"/>",
            "a shared formula's later cell",
        ),
        ("<v></v><f>A1+1</f>", "out of the schema's order"),
    ] {
        let sheet = format!("<row><c><v>1</v></c><c>{body}</c></row>");
        let package = xlsx(&[("Data", &sheet)], "", false, false);
        assert_eq!(
            column_b(&package, every),
            numbered(&[(1, "uncached")]),
            "{kept}"
        );
    }
    for (cell, kept) in [
        ("<table:table-cell table:formula=\"of:=1\"/>", "no value"),
        (
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"float\"/>",
            "a type, and no value of it",
        ),
        (
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"float\"><text:p>7</text:p></table:table-cell>",
            "a value shown, and not said",
        ),
    ] {
        let tables = format!(
            "<table:table table:name=\"Data\"><table:table-row>\
<table:table-cell office:value-type=\"float\" office:value=\"1\"/>{cell}</table:table-row></table:table>"
        );
        assert_eq!(
            column_b(&ods(&tables, false), every),
            numbered(&[(1, "uncached")]),
            "{kept}"
        );
    }
    // No door was asked, so there's no detail.
    let package = uncached_xlsx();
    let book = Workbook::from_slice(&package).expect("a workbook");
    let plan = [Column::i32(1)];
    let mut sheet = book.sheet(0, every, &plan).expect("a sheet");
    let batch = sheet.read().expect("a batch").expect("rows");
    assert_eq!(batch.origin(0, 1), Origin::Uncached);
    assert_eq!(batch.detail(0, 1), None);
    assert_eq!(
        batch.verdicts(0)[1].fault().map(|fault| fault.reason),
        Some(Reason::Empty)
    );
}

#[test]
fn the_xlsx_fixture_reads_every_form() {
    assert_eq!(
        column_b(&uncached_xlsx(), every_row()),
        numbered(&[
            (1, "\"value\""),
            (2, "uncached"),
            (3, "uncached"),
            (4, "uncached"),
            (5, "uncached"),
            (6, "uncached"),
            (7, "uncached"),
            (8, "uncached"),
            (9, "blank"),
            (10, "padded"),
            (11, "\"3\""),
            (12, "\"#DIV/0!\""),
            (13, "\"hi\""),
            (14, "blank"),
            (15, "blank"),
            (16, "\"true\""),
            (17, "uncached"),
            (18, "uncached"),
            (19, "padded"),
            (20, "uncached"),
        ])
    );
}

#[test]
fn the_ods_fixture_reads_every_form() {
    assert_eq!(
        column_b(&uncached_ods(), every_row()),
        numbered(&[
            (1, "\"value\""),
            (2, "uncached"),
            (3, "uncached"),
            (4, "uncached"),
            (5, "uncached"),
            (6, "uncached"),
            (7, "uncached"),
            (8, "uncached"),
            (9, "uncached"),
            (10, "uncached"),
            (11, "uncached"),
            (12, "blank"),
            (13, "padded"),
            (14, "\"3\""),
            (15, "\"#DIV/0!\""),
            (16, "\"hi\""),
            (17, "blank"),
            (18, "blank"),
            (19, "blank"),
            (20, "blank"),
            (21, "\"shown\""),
            (22, "uncached"),
            (23, "uncached"),
            (24, "padded"),
            (25, "uncached"),
            (26, "uncached"),
            (27, "uncached"),
        ])
    );
}

#[test]
fn a_blank_stays_blank() {
    let sheet = "<row><c><v>1</v></c><c/><c><v>1</v></c></row>\
<row><c><v>1</v></c><c></c><c><v>1</v></c></row>";
    assert_eq!(
        column_b(&xlsx(&[("Data", sheet)], "", false, false), every_row()),
        numbered(&[(1, "blank"), (2, "blank")])
    );
}

/// `=""` has a result, of no text, and every writer we've a file from says so one of these ways: it
/// reads as it did, `Empty` at [`Origin::Cell`], and a token matches it where it's stored as text.
#[test]
fn a_cached_empty_string_is_a_result() {
    let tokens = no_header().with_tokens(&[""]);
    for (cell, kept, token) in [
        (
            "<c t=\"str\"><f>\"\"</f><v></v></c>",
            "Google's and LibreOffice's export",
            "token",
        ),
        ("<c t=\"str\"><f>\"\"</f><v/></c>", "Excel's", "blank"),
    ] {
        let sheet = format!("<row><c><v>1</v></c>{cell}<c><v>1</v></c></row>");
        let package = xlsx(&[("Data", &sheet)], "", false, false);
        assert_eq!(
            column_b(&package, every_row()),
            numbered(&[(1, "blank")]),
            "{kept}"
        );
        assert_eq!(
            column_b(&package, tokens),
            numbered(&[(1, token)]),
            "{kept}"
        );
    }
    let row = |cell: &str| {
        format!(
            "<table:table table:name=\"Data\"><table:table-row><table:table-cell office:value-type=\"float\" office:value=\"1\"/>\
{cell}<table:table-cell office:value-type=\"float\" office:value=\"1\"/></table:table-row></table:table>"
        )
    };
    for (cell, kept, token) in [
        (
            "<table:table-cell table:formula=\"of:=&quot;&quot;\"><text:p/></table:table-cell>",
            "LibreOffice's and Google's",
            "blank",
        ),
        (
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"string\" office:string-value=\"\"/>",
            "declared",
            "token",
        ),
        (
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"string\"><text:p/></table:table-cell>",
            "a string with its paragraph",
            "token",
        ),
    ] {
        let package = ods(&row(cell), false);
        assert_eq!(
            column_b(&package, every_row()),
            numbered(&[(1, "blank")]),
            "{kept}"
        );
        assert_eq!(
            column_b(&package, tokens),
            numbered(&[(1, token)]),
            "{kept}"
        );
    }
    // A string of only spaces is trimmed to that, as it always was, and it's still a result.
    let sheet = "<row><c><v>1</v></c><c t=\"str\"><f>A1</f><v>  </v></c></row>";
    assert_eq!(
        column_b(&xlsx(&[("Data", sheet)], "", false, false), every_row()),
        numbered(&[(1, "blank")])
    );
}

/// A slot gives back what it was told, and an uncached formula holds nothing to say as text.
#[test]
fn an_uncached_cell_is_kept_by_its_slot() {
    let slot = Slot::of(&Cell::Uncached);
    assert_eq!(slot.cell(&[], &[]), Cell::Uncached);
    assert!(slot.is_uncached());
    assert!(slot.is_empty(), "it doesn't hold a row past the filter");
    assert_eq!(slot.span(), None);
    assert_eq!(said(Cell::Uncached), "");
    assert_eq!(Slot::EMPTY.with_no_result(), slot);
    assert!(!Slot::EMPTY.is_uncached());
}

/// ODS's rules at their edges. A value of spaces is text, through either fill, as it always was.
/// No value is none, and text with no paragraph is text.
#[test]
fn an_ods_formula_at_the_edges_of_having_a_value() {
    let package = |cell: &str| {
        let tables = format!(
            "<table:table table:name=\"Data\"><table:table-row>\
<table:table-cell office:value-type=\"float\" office:value=\"1\"/>{cell}</table:table-row></table:table>"
        );
        ods(&tables, false)
    };
    for (kind, attribute) in [
        ("float", "value"),
        ("boolean", "boolean-value"),
        ("date", "date-value"),
        ("time", "time-value"),
    ] {
        let spaces = package(&format!(
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"{kind}\" office:{attribute}=\"  \"/>"
        ));
        assert_eq!(
            column_b(&spaces, every_row()),
            numbered(&[(1, "\"  \"")]),
            "{kind}"
        );
        assert_eq!(
            through_1_0(&spaces, Door::Text, false),
            [(1, 0, b"  ".to_vec())],
            "{kind}"
        );
        let none = package(&format!(
            "<table:table-cell table:formula=\"of:=1\" office:value-type=\"{kind}\" office:{attribute}=\"\"/>"
        ));
        assert_eq!(
            column_b(&none, every_row()),
            numbered(&[(1, "uncached")]),
            "{kind}"
        );
    }
    assert_eq!(
        column_b(
            &package("<table:table-cell table:formula=\"of:=1\">stray</table:table-cell>"),
            every_row()
        ),
        numbered(&[(1, "\"stray\"")])
    );
}

/// A later `<v/>` doesn't take back a value already read, whichever fill reads it, and the row it's
/// alone in holds a cell.
#[test]
fn an_empty_value_doesnt_take_back_a_result() {
    let sheet = "<row><c r=\"B1\"><f>A1</f><v>1</v><v/></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    assert_eq!(column_b(&package, no_header()), numbered(&[(1, "\"1\"")]));
    assert_eq!(
        through_1_0(&package, Door::Text, true),
        [(1, 0, b"1".to_vec())]
    );
    assert_eq!(through_1_0(&package, Door::I32, true)[0].1, 0);
}

/// What `SheetOptions::skip_empty_rows` warns of. A table of nothing but formulas, from a writer that
/// has nothing to spell where a value goes: under the default read its header and no rows, with
/// nothing to say so, and with the filter off every row, its cells marked. A writer spelling an
/// empty value has its rows delivered both ways. Each format has one spelling of each.
#[test]
fn an_all_formula_table_and_the_filter() {
    let rows = |body: &str| {
        let mut sheet = String::from(
            "<row r=\"1\"><c r=\"A1\" t=\"str\"><v>price</v></c><c r=\"B1\" t=\"str\"><v>total</v></c></row>",
        );
        for number in [2, 3] {
            sheet.push_str(&format!(
                "<row r=\"{number}\"><c r=\"A{number}\">{body}</c><c r=\"B{number}\">{body}</c></row>"
            ));
        }
        xlsx(&[("Data", &sheet)], "", false, false)
    };
    let table = |cell: &str| {
        let label = |text: &str| {
            format!(
                "<table:table-cell office:value-type=\"string\"><text:p>{text}</text:p></table:table-cell>"
            )
        };
        let row = format!("<table:table-row>{cell}{cell}</table:table-row>");
        ods(
            &format!(
                "<table:table table:name=\"Data\"><table:table-row>{}{}</table:table-row>{row}{row}</table:table>",
                label("price"),
                label("total")
            ),
            false,
        )
    };
    let dropped = [
        rows("<f>1+1</f><v />"),
        rows("<f>1+1</f>"),
        table("<table:table-cell table:formula=\"of:=1+1\"/>"),
    ];
    let kept = [
        rows("<f>1+1</f><v></v>"),
        table("<table:table-cell table:formula=\"of:=1+1\" office:value-type=\"float\"/>"),
    ];
    let marked = numbered(&[(2, "uncached"), (3, "uncached")]);
    let every = SheetOptions::new().with_empty_rows_skipped(false);
    for package in &dropped {
        assert_eq!(column_b(package, SheetOptions::new()), []);
        assert_eq!(column_b(package, every), marked);
        let found = read(package, 0, SheetOptions::new(), 2);
        assert_eq!(found.header, names(&["price", "total"]));
        assert_eq!(found.failure, None, "nothing says so");
    }
    for package in &kept {
        assert_eq!(column_b(package, SheetOptions::new()), marked);
        assert_eq!(column_b(package, every), marked);
    }
}

/// An inline string is a value the cell holds, in `<is>` instead of `<v>`.
#[test]
fn an_inline_string_is_a_result() {
    for (cell, said) in [
        ("<is><t>hi</t></is>", "\"hi\""),
        // No text, and yet what the cell says it holds. This is read as a blank, as it was.
        ("<is/>", "blank"),
    ] {
        let sheet = format!(
            "<row><c><v>1</v></c><c t=\"inlineStr\"><f>A1</f>{cell}</c><c><v>1</v></c></row>"
        );
        assert_eq!(
            column_b(&xlsx(&[("Data", &sheet)], "", false, false), every_row()),
            numbered(&[(1, said)]),
        );
    }
}

#[test]
fn a_cached_error_stays_an_error() {
    let plan = [Column::i32(1)];
    for package in [uncached_xlsx(), uncached_ods()] {
        let book = Workbook::from_slice(&package).expect("a workbook");
        let mut sheet = book.sheet(0, every_row(), &plan).expect("a sheet");
        let batch = sheet.read().expect("a batch").expect("rows");
        let row = (0..batch.rows())
            .find(|&row| batch.raw(0, row).as_ref() == b"#DIV/0!")
            .expect("the error");
        assert_eq!(
            batch.verdicts(0)[row].fault().map(|fault| fault.reason),
            Some(Reason::Malformed)
        );
        assert_eq!(batch.origin(0, row), Origin::Cell);
    }
}

#[test]
fn a_cached_value_stays_a_value() {
    let plan = [Column::i32(1)];
    for package in [uncached_xlsx(), uncached_ods()] {
        let book = Workbook::from_slice(&package).expect("a workbook");
        let mut sheet = book.sheet(0, every_row(), &plan).expect("a sheet");
        let batch = sheet.read().expect("a batch").expect("rows");
        let row = (0..batch.rows())
            .find(|&row| batch.get::<i32>(0, row) == Ok(3))
            .expect("the value");
        assert_eq!(batch.origin(0, row), Origin::Cell);
    }
    // A number with nothing else in its row, and a formula.
    let sheet = "<row><c/><c><f>1+2</f><v>3</v></c></row>";
    assert_eq!(
        column_b(&xlsx(&[("Data", sheet)], "", false, false), every_row()),
        numbered(&[(1, "\"3\"")])
    );
}

/// What the 1.0 fill has always read from these: `Empty` and no text, whichever door, and a row
/// holding only a formula with nothing at all where a value goes is an empty row. A row holding
/// `<v></v>`, or a typed ODS cell with no value, isn't, since 1.0 took that for a cell of no text.
#[test]
fn one_point_oh_reads_empty() {
    let empty = 1; // HyperCast's ABI code for `Empty`.
    let by_number = |rows: &[(u32, u32, Vec<u8>)], number: u32| {
        rows.iter()
            .find(|row| row.0 == number)
            .map(|row| (row.1, row.2.clone()))
    };
    let uncached = [2, 3, 4, 5, 6, 7, 8, 9, 10, 14, 15, 17, 18, 19, 20];
    for door in [Door::Text, Door::I32, Door::Date] {
        let found = through_1_0(&uncached_xlsx(), door, false);
        assert_eq!(found.len(), 20, "{door:?}");
        for number in uncached {
            assert_eq!(
                by_number(&found, number),
                Some((empty, Vec::new())),
                "{door:?}, row {number}"
            );
        }
    }
    let filtered = through_1_0(&uncached_xlsx(), Door::Text, true);
    assert_eq!(by_number(&filtered, 17), None);
    assert_eq!(by_number(&filtered, 18), Some((empty, Vec::new())));
    assert_eq!(by_number(&filtered, 19), None);

    let uncached = [
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27,
    ];
    for door in [Door::Text, Door::I32, Door::Date] {
        let found = through_1_0(&uncached_ods(), door, false);
        assert_eq!(found.len(), 27, "{door:?}");
        for number in uncached {
            assert_eq!(
                by_number(&found, number),
                Some((empty, Vec::new())),
                "{door:?}, row {number}"
            );
        }
    }
    let filtered = through_1_0(&uncached_ods(), Door::Text, true);
    assert_eq!(by_number(&filtered, 22), None);
    assert_eq!(by_number(&filtered, 23), Some((empty, Vec::new())));
    assert_eq!(by_number(&filtered, 24), None);
}

/// The rows a sheet delivers are what they were, through either fill: a row holding only a formula
/// with nothing where a value goes is an empty row, and one with an empty value spelled isn't.
#[test]
fn a_row_of_uncached_formulas_filters_as_it_did() {
    let last_of = |found: Vec<(u32, String)>, count: usize| found[found.len() - count..].to_vec();
    assert_eq!(
        last_of(column_b(&uncached_xlsx(), no_header()), 3),
        numbered(&[(16, "\"true\""), (18, "uncached"), (20, "uncached")]),
    );
    let ods_rows = column_b(&uncached_ods(), no_header());
    assert!(!ods_rows.iter().any(|row| row.0 == 22));
    assert!(ods_rows.contains(&(23, "uncached".to_string())));
    assert!(!ods_rows.iter().any(|row| row.0 == 24));
    // Past the slots a read gave, too.
    let sheet = "<row><c r=\"Z1\"><f>1</f></c></row><row><c><v>1</v></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    assert_eq!(column_b(&package, no_header()), numbered(&[(2, "padded")]));
}

/// A header is named by what its cells hold, so a formula with no result names nothing, and a row
/// of nothing else isn't one.
#[test]
fn a_header_isnt_named_by_an_uncached_formula() {
    let sheet = "<row><c><f>1</f></c></row>\
<row><c t=\"str\"><v>a</v></c><c><f>1</f></c><c t=\"str\"><v>c</v></c><c><f>1</f></c></row>\
<row><c><v>1</v></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    let found = read(&package, 0, SheetOptions::new(), 1);
    assert_eq!(found.header, names(&["a", "", "c"]));
    assert_eq!(found.rows, table(&[(3, &["1"])]));
}

/// The cell holds nothing a token could match, in either of its forms.
#[test]
fn a_token_doesnt_claim_an_uncached_formula() {
    let tokens = SheetOptions::new().with_header(false).with_tokens(&[""]);
    assert_eq!(
        column_b(&uncached_xlsx(), tokens)
            .iter()
            .filter(|row| row.0 <= 4)
            .cloned()
            .collect::<Vec<_>>(),
        numbered(&[
            (1, "\"value\""),
            (2, "uncached"),
            (3, "uncached"),
            (4, "uncached"),
        ])
    );
    let row = |number: u32| {
        column_b(&uncached_xlsx(), tokens)
            .into_iter()
            .find(|found| found.0 == number)
            .expect("the row")
            .1
    };
    assert_eq!(row(14), "token", "a cached empty string is text");
}

/// A formula with no result is a cell of the row: the row's as wide as it, and inside a range it's
/// marked, outside it nothing.
#[test]
fn an_uncached_formula_is_as_wide_as_a_cell() {
    let sheet =
        "<row><c r=\"A1\"><v>1</v></c><c r=\"C1\"><f>1</f></c><c r=\"E1\"><f>1</f></c></row>";
    let package = xlsx(&[("Data", sheet)], "", false, false);
    let widths = |options: SheetOptions| {
        let specs = [ColumnSpec::new(0, Door::Text.code(), 0)];
        let mut harness = Harness::new(&package, Sizing::Roomy);
        harness.open().expect("it opens");
        let sheets = harness.sheets().expect("its sheets");
        harness.load().expect("its tables");
        harness.through_with = Some(Vec::new());
        harness.read(&sheets[0], options, &specs, 8).widths[0]
    };
    assert_eq!(widths(every_row()), 5);
    assert_eq!(widths(every_row().with_range(range("A1:D1"))), 3);
    assert_eq!(widths(every_row().with_range(range("A1:B1"))), 1);

    let told_in = |text: &str, ordinal: usize| {
        let book = Workbook::from_slice(&package).expect("a workbook");
        let plan = [Column::text(ordinal)];
        let mut read = book
            .sheet(0, every_row().with_range(range(text)), &plan)
            .expect("a sheet");
        let batch = read.read().expect("a batch").expect("rows");
        told(&batch, 0, 0)
    };
    assert_eq!(told_in("C1:D1", 0), "uncached");
    assert_eq!(told_in("C1:D1", 1), "padded");
    assert_eq!(told_in("A1:B1", 1), "padded");
    assert_eq!(told_in("D1:E1", 0), "blank");
    assert_eq!(told_in("D1:E1", 1), "uncached");
}

/// An ODS repeat holds the stored row over a batch's end, a formula's mark with it.
#[test]
fn an_uncached_formula_survives_a_repeat_across_batches() {
    let package = uncached_ods();
    let book = Workbook::from_slice(&package).expect("a workbook");
    let plan = [Column::text(1)];
    let mut read = book
        .sheet(0, every_row().with_batch_rows(1), &plan)
        .expect("a sheet");
    let mut found = Vec::new();
    while let Some(batch) = read.read().expect("a batch") {
        assert_eq!(batch.rows(), 1);
        found.push((batch.line(0), told(&batch, 0, 0)));
    }
    assert_eq!(
        found.split_off(found.len() - 3),
        numbered(&[(25, "uncached"), (26, "uncached"), (27, "uncached")])
    );
}
