//! End-to-end reads of the committed fixtures (`tests/fixtures/`, built by
//! `make_fixtures.py`: openpyxl for the XLSX files, the ODF 1.3 schema by hand for the
//! ODS), through the Rust API and through the typed batch.

use hypertabular::workbook::{Error, Format, SheetOptions, Workbook};
use hypertabular::{
    Batch, Cell, CellError, Column, Date, Door, Duration, ExcelEpoch, Plan, Reason, fill_batch,
};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn text(cell: Cell<'_>) -> String {
    match cell {
        Cell::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn basic_xlsx_from_openpyxl() {
    for (name, system) in [
        ("basic.xlsx", ExcelEpoch::Y1900),
        ("basic-1904.xlsx", ExcelEpoch::Y1904),
    ] {
        let workbook = Workbook::from_path(fixture(name)).unwrap();
        assert_eq!(workbook.format(), Format::Xlsx);
        assert_eq!(workbook.date_system(), system, "{name}");
        assert_eq!(workbook.sheets().len(), 1);
        assert_eq!(workbook.sheets()[0].name, "Data");

        let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
        let header = sheet.header().unwrap();
        assert_eq!(header.len(), 9);
        assert_eq!(header.ordinal(b"when"), Some(3));
        assert_eq!(header.ordinal(b"at"), Some(8));

        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(row.number(), 2);
        assert_eq!(row.cell(0), Cell::Number(1.0));
        assert_eq!(text(row.cell(1)), "alice");
        assert_eq!(row.cell(2), Cell::Number(2.5));
        assert_eq!(
            row.cell(3),
            Cell::Wall {
                date: Date {
                    year: 2024,
                    month: 1,
                    day: 31
                },
                nanos: 37_800_000_000_000
            }
        );
        assert_eq!(row.cell(4), Cell::Bool(true));
        assert_eq!(text(row.cell(5)), "plain");
        assert_eq!(row.cell(6), Cell::Number(0.25));
        assert_eq!(
            row.cell(7),
            Cell::Wall {
                date: Date {
                    year: 2023,
                    month: 3,
                    day: 15
                },
                nanos: 0
            }
        );
        assert_eq!(row.cell(8), Cell::Clock(54_245_000_000_000));

        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(text(row.cell(1)), "bob, jr");
        assert_eq!(row.cell(2), Cell::Number(-7.0));
        assert_eq!(
            row.cell(3),
            Cell::Wall {
                date: Date {
                    year: 2024,
                    month: 2,
                    day: 29
                },
                nanos: 0
            }
        );
        assert_eq!(row.cell(4), Cell::Bool(false));
        assert_eq!(row.cell(5), Cell::Empty);
        if system == ExcelEpoch::Y1904 {
            // 1900-02-28 is before the 1904 epoch: a negative serial, which the serial
            // rules reject, so the raw number is delivered and the Date door will say so.
            assert_eq!(row.cell(7), Cell::Number(-1402.0));
        } else {
            assert_eq!(
                row.cell(7),
                Cell::Wall {
                    date: Date {
                        year: 1900,
                        month: 2,
                        day: 28
                    },
                    nanos: 0
                }
            );
        }
        // A time-formatted midnight serial is 0: a clock, not a date.
        assert_eq!(row.cell(8), Cell::Clock(0));

        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(row.cell(0), Cell::Number(3.0));
        assert_eq!(
            row.cell(1),
            Cell::Empty,
            "openpyxl writes an empty string as no cell"
        );
        assert_eq!(row.cell(2), Cell::Number(1e15));
        assert_eq!(row.cell(3), Cell::Empty);
        assert_eq!(text(row.cell(5)), "x <&> \"y\"");
        assert_eq!(
            row.cell(7),
            Cell::Wall {
                date: Date {
                    year: 9999,
                    month: 12,
                    day: 31
                },
                nanos: 0
            }
        );
        assert_eq!(row.cell(8), Cell::Clock(86_399_000_000_000));

        // Row 5 is absent: skipped by default.
        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(row.number(), 6);
        assert_eq!(row.cell(0), Cell::Number(6.0));
        assert_eq!(row.len(), 26);
        assert_eq!(row.cell(13), Cell::Empty);
        assert_eq!(text(row.cell(25)), "Z6");

        // A formula with no cached value is an empty cell, and a row of nothing but
        // that is an empty row — skipped.
        assert!(sheet.next_row().unwrap().is_none());

        // With empty rows delivered, the gap and the formula row both show up.
        let mut sheet = workbook
            .sheet(0, SheetOptions::default().with_empty_rows_skipped(false))
            .unwrap();
        let numbers: Vec<(u32, usize)> = std::iter::from_fn(|| {
            sheet
                .next_row()
                .unwrap()
                .map(|row| (row.number(), row.len()))
        })
        .collect();
        assert_eq!(
            numbers,
            vec![(2, 9), (3, 9), (4, 9), (5, 0), (6, 26), (7, 0)]
        );
    }
}

#[test]
fn typed_batch_over_the_xlsx_fixture() {
    let workbook = Workbook::from_path(fixture("basic.xlsx")).unwrap();
    let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
    let plan = Plan::new(vec![
        Column::new(0, Door::I32),
        Column::new(1, Door::Text),
        Column::new(2, Door::F64),
        Column::new(3, Door::Timestamp),
        Column::new(4, Door::Bool),
        Column::new(7, Door::Date),
        Column::new(8, Door::Time),
        Column::new(2, Door::U8),
    ]);
    let mut batch = Batch::new();
    assert_eq!(fill_batch(&mut sheet, &plan, &mut batch, 100).unwrap(), 4);
    assert_eq!(batch.column(0).i32s().unwrap(), &[1, 2, 3, 6]);
    assert_eq!(batch.text(1, 0), Some(&b"alice"[..]));
    assert_eq!(batch.column(1).verdicts()[2].reason, Reason::Empty as u32);
    assert_eq!(batch.column(2).f64s().unwrap(), &[2.5, -7.0, 1e15, 0.0]);
    assert_eq!(
        batch.column(3).timestamps().unwrap()[0].seconds,
        1_706_697_000
    );
    assert_eq!(batch.column(3).verdicts()[2].reason, Reason::Empty as u32);
    assert_eq!(
        batch.column(4).bools().unwrap(),
        &[true, false, false, false]
    );
    assert_eq!(
        batch.column(5).dates().unwrap()[2],
        Date {
            year: 9999,
            month: 12,
            day: 31
        }
    );
    assert_eq!(
        batch.column(6).times().unwrap(),
        &[54_245_000_000_000, 0, 86_399_000_000_000, 0]
    );
    // 2.5 into a u8: not integral; -7: out of range; 1e15: out of range.
    let verdicts = batch.column(7).verdicts();
    assert_eq!(verdicts[0].reason, Reason::Malformed as u32);
    assert_eq!(verdicts[1].reason, Reason::OutOfRange as u32);
    assert_eq!(verdicts[2].reason, Reason::OutOfRange as u32);
    let faults = batch.faults();
    assert_eq!(batch.raw(&faults[0]), b"2.5");
    assert_eq!(batch.raw(&faults[1]), b"-7");
    assert_eq!(batch.raw(&faults[2]), b"1000000000000000");
}

#[test]
fn hidden_sheets_are_listed_and_openable_by_name() {
    let workbook = Workbook::from_path(fixture("multisheet.xlsx")).unwrap();
    let names: Vec<(&str, bool)> = workbook
        .sheets()
        .iter()
        .map(|s| (s.name.as_str(), s.hidden))
        .collect();
    assert_eq!(
        names,
        vec![
            ("First", false),
            ("Hidden", true),
            ("Very", true),
            ("Last", false)
        ]
    );
    let mut sheet = workbook
        .sheet_named("Very", SheetOptions::default())
        .unwrap();
    assert_eq!(sheet.header().unwrap().name(0), Some(&b"v"[..]));
    assert_eq!(
        sheet.next_row().unwrap().unwrap().cell(0),
        Cell::Number(3.0)
    );
    assert!(matches!(
        workbook.sheet_named("Nope", SheetOptions::default()),
        Err(Error::SheetNotFound(_))
    ));
    assert!(matches!(
        workbook.sheet(9, SheetOptions::default()),
        Err(Error::SheetNotFound(_))
    ));
    // Two sheets open at once, independently.
    let mut first = workbook.sheet(0, SheetOptions::default()).unwrap();
    let mut last = workbook.sheet(3, SheetOptions::default()).unwrap();
    assert_eq!(last.next_row().unwrap().unwrap().cell(0), Cell::Number(4.0));
    assert_eq!(
        first.next_row().unwrap().unwrap().cell(0),
        Cell::Number(1.0)
    );
}

#[test]
fn rich_text_runs_are_concatenated() {
    let workbook = Workbook::from_path(fixture("rich.xlsx")).unwrap();
    let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
    assert_eq!(
        text(sheet.next_row().unwrap().unwrap().cell(0)),
        "plain bold tail"
    );
    assert_eq!(
        text(sheet.next_row().unwrap().unwrap().cell(0)),
        "shared once"
    );
    assert_eq!(
        text(sheet.next_row().unwrap().unwrap().cell(0)),
        "shared once"
    );
    assert!(sheet.next_row().unwrap().is_none());
}

#[test]
fn basic_ods_by_the_schema() {
    let bytes = std::fs::read(fixture("basic.ods")).unwrap();
    let workbook = Workbook::from_slice(&bytes).unwrap();
    assert_eq!(workbook.format(), Format::Ods);
    let names: Vec<&str> = workbook.sheets().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Data", "Second"]);

    let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
    assert_eq!(sheet.header().unwrap().ordinal(b"stamp"), Some(5));
    let row = sheet.next_row().unwrap().unwrap();
    assert_eq!(row.number(), 2);
    assert_eq!(row.cell(0), Cell::Number(1.0));
    assert_eq!(row.cell(1), Cell::Number(2.5));
    assert_eq!(row.cell(2), Cell::Number(0.25));
    assert_eq!(row.cell(3), Cell::Number(-7.0));
    assert_eq!(
        row.cell(4),
        Cell::Wall {
            date: Date {
                year: 2023,
                month: 3,
                day: 15
            },
            nanos: 0
        }
    );
    assert_eq!(
        row.cell(5),
        Cell::Wall {
            date: Date {
                year: 2024,
                month: 1,
                day: 31
            },
            nanos: 37_800_000_000_000
        }
    );
    assert_eq!(
        row.cell(6),
        Cell::Span(Duration {
            seconds: 91_800,
            nanos: 0
        })
    );
    assert_eq!(row.cell(7), Cell::Bool(true));
    assert_eq!(text(row.cell(8)), "a   b\tc\nsecond & <line>");
    assert_eq!(row.cell(9), Cell::Error(CellError::DivideByZero));

    // The repeated row is delivered twice; repeated empties are not materialised,
    // a string-value wins over the body, and a repeated value fills its columns.
    for number in [3, 4] {
        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(row.number(), number);
        assert_eq!(row.cell(0), Cell::Number(9.0));
        assert_eq!(row.cell(1), Cell::Empty);
        assert_eq!(row.cell(3), Cell::Empty);
        assert_eq!(text(row.cell(4)), "sv");
        assert_eq!(row.cell(5), Cell::Number(4.0));
        assert_eq!(row.cell(6), Cell::Number(4.0));
        assert_eq!(row.len(), 7);
    }
    // LibreOffice's million-row padding: skipped, not walked.
    assert!(sheet.next_row().unwrap().is_none());

    let mut second = workbook
        .sheet_named("Second", SheetOptions::default())
        .unwrap();
    assert_eq!(second.header().unwrap().name(0), Some(&b"only"[..]));
    assert_eq!(
        second.next_row().unwrap().unwrap().cell(0),
        Cell::Number(42.0)
    );
    assert!(second.next_row().unwrap().is_none());
}

#[test]
fn not_a_workbook() {
    assert!(matches!(
        Workbook::from_slice(b"hello"),
        Err(Error::Container(_))
    ));
    let empty_zip = hypertabular::workbook::zip::write::build(&[("readme.txt", b"hi", false)]);
    assert!(matches!(
        Workbook::from_slice(&empty_zip),
        Err(Error::NotAWorkbook)
    ));
}
