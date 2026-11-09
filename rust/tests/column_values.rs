//! Every typed door lends its whole column through the same checked generic accessor.

use hypertabular::{
    CivilDateTime, Column, Date, DateOrder, Decimal, DelimitedReader, Detail, Dialect, Duration,
    ExcelEpoch, LongRows, Origin, Problem, Range, Reason, SheetOptions, ShortRows, Timestamp,
    UnixPrecision, Value, Workbook,
};

fn check<T: Value + PartialEq + std::fmt::Debug>(column: Column, text: &str, expected: T) {
    let input = format!("value\n{text}\nnot-a-value\n\"\"\n");
    let mut reader =
        DelimitedReader::from_slice(input.as_bytes(), Dialect::CSV, &[column]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let values: &[T] = batch.values(0);
    assert_eq!(values.len(), 3);
    assert_eq!(values[0], expected);
    assert_eq!(batch.get::<T>(0, 0), Ok(expected));
    assert!(batch.verdicts(0)[0].is_ok());
    assert_eq!(
        batch.verdicts(0)[1].fault().unwrap().reason,
        Reason::Malformed
    );
    assert_eq!(batch.verdicts(0)[2].fault().unwrap().reason, Reason::Empty);
}

#[test]
fn every_value_type_has_a_borrowed_column_with_verdicts() {
    check(Column::bool(0), "true", true);
    check(Column::i8(0), "-12", -12i8);
    check(Column::i16(0), "-1234", -1234i16);
    check(Column::i32(0), "-123456", -123456i32);
    check(Column::i64(0), "-12345678901", -12345678901i64);
    check(Column::u8(0), "12", 12u8);
    check(Column::u16(0), "1234", 1234u16);
    check(Column::u32(0), "123456", 123456u32);
    check(Column::u64(0), "12345678901", 12345678901u64);
    check(Column::f32(0), "1.25", 1.25f32);
    check(Column::f64(0), "1.25", 1.25f64);
    check(
        Column::decimal(0),
        "12.5",
        Decimal {
            lo: 125,
            hi: 0,
            scale: 1,
            negative: false,
        },
    );
    check(
        Column::uuid(0),
        "01020304-0506-0708-090a-0b0c0d0e0f10",
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    );
    let instant = Timestamp {
        seconds: 1_704_067_200,
        nanos: 0,
    };
    check(Column::timestamp(0), "2024-01-01T00:00:00Z", instant);
    check(
        Column::unix(0, UnixPrecision::Seconds),
        "1704067200",
        instant,
    );
    check(Column::excel_serial(0, ExcelEpoch::Y1900), "45292", instant);
    let date = Date {
        year: 2024,
        month: 1,
        day: 1,
    };
    check(Column::date(0), "2024-01-01", date);
    check(
        Column::date_ordered(0, DateOrder::DayMonthYear),
        "01/01/2024",
        date,
    );
    check(
        Column::datetime(0, DateOrder::YearMonthDay),
        "2024-01-01 12:30:00",
        CivilDateTime {
            date,
            nanos_of_day: 45_000_000_000_000,
        },
    );
    check(Column::time(0), "00:00:01", 1_000_000_000u64);
    check(
        Column::duration(0),
        "PT1S",
        Duration {
            seconds: 1,
            nanos: 0,
        },
    );
}

#[test]
#[should_panic(expected = "which does not make a f64")]
fn a_generic_column_checks_its_type() {
    let mut reader =
        DelimitedReader::from_slice(b"n\n1\n", Dialect::CSV, &[Column::i32(0)]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let _ = batch.values::<f64>(0);
}

#[test]
#[should_panic(expected = "column 1")]
fn a_generic_column_checks_its_index() {
    let mut reader =
        DelimitedReader::from_slice(b"n\n1\n", Dialect::CSV, &[Column::i32(0)]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let _ = batch.values::<i32>(1);
}

/// A problem's row, line, plan column, source column and reason.
type Listed = (usize, u32, Option<usize>, Option<usize>, Option<Reason>);

/// Everything a read lists, batch by batch, with each row counted across the read.
fn faults_of(mut reader: DelimitedReader<'_>) -> Vec<Listed> {
    let (mut all, mut base) = (Vec::new(), 0);
    while let Some(batch) = reader.read().unwrap() {
        all.extend(batch.faults().map(|problem| {
            (
                base + problem.row,
                problem.line,
                problem.plan_column,
                problem.source_column,
                problem.reason,
            )
        }));
        base += batch.rows();
    }
    all
}

#[test]
fn faults_say_line_and_source_column() {
    // A quoted line break makes a record's line differ from its row; the plan reads out of order.
    let input = b"a,b,c\n1,x,\"two\nlines\"\n1,x,3,4\ny,2,3\n";
    let plan = [Column::i32(1), Column::text(2), Column::i32(0)];
    let dialect = Dialect::CSV.with_long_rows(LongRows::Report);
    for batch_rows in [1, 2, 1024] {
        let reader = DelimitedReader::options()
            .batch_rows(batch_rows)
            .from_slice(input, dialect, &plan)
            .unwrap();
        assert_eq!(
            faults_of(reader),
            [
                (0, 2, Some(0), Some(1), Some(Reason::Malformed)),
                (1, 4, None, None, None),
                (1, 4, Some(0), Some(1), Some(Reason::Malformed)),
                (2, 5, Some(2), Some(0), Some(Reason::Malformed)),
            ],
            "{batch_rows} rows a batch"
        );
    }
}

#[test]
fn a_problem_carries_the_whole_answer() {
    let plan = [Column::u8(0), Column::i32(1)];
    let dialect = Dialect::CSV
        .with_header(false)
        .with_long_rows(LongRows::Report);
    let mut reader = DelimitedReader::from_slice(b"300,12a\n1,2,3\n", dialect, &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let problems: Vec<Problem> = batch.faults().collect();
    assert_eq!(problems.len(), 3);
    let first = problems[0];
    assert_eq!(first.reason, Some(Reason::OutOfRange));
    assert_eq!(first.detail, batch.detail(0, 0));
    assert_eq!(first.detail, Some(Detail::TooLarge));
    assert_eq!(first.origin, Origin::Cell);
    assert_eq!((first.offset, first.len), (0, 3));
    assert_eq!((first.expected, first.found), (0, 0));
    let second = problems[1];
    assert_eq!((second.offset, second.len), (2, 1));
    assert_eq!(second.detail, Some(Detail::UnexpectedCharacter));
    let row = problems[2];
    assert_eq!((row.row, row.line), (1, 2));
    assert_eq!((row.plan_column, row.source_column), (None, None));
    assert_eq!((row.reason, row.detail), (None, None));
    assert_eq!((row.expected, row.found), (2, 3));
}

#[test]
fn faults_leave_out_what_the_read_declared() {
    let plan = [Column::i32(0), Column::i32(1)];
    let dialect = Dialect::CSV
        .with_header(false)
        .with_short_rows(ShortRows::Pad)
        .with_long_rows(LongRows::Truncate)
        .with_tokens(&["NA"]);
    let mut reader = DelimitedReader::from_slice(b"1,2\nNA,\n3\n4,5,6\n", dialect, &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.rows(), 4);
    assert_eq!(batch.faults().count(), 0);
}

#[test]
fn more_faults_than_a_call_holds_all_come() {
    // More than the iterator holds at a time, and exactly as many, and one past that.
    for count in [15, 16, 17, 33, 48, 300] {
        let input = "x\n".repeat(count + 1);
        let plan = [Column::i32(0)];
        let mut reader =
            DelimitedReader::from_slice(input.as_bytes(), Dialect::CSV, &plan).unwrap();
        let batch = reader.read().unwrap().unwrap();
        let rows: Vec<usize> = batch.faults().map(|problem| problem.row).collect();
        assert_eq!(rows, (0..count).collect::<Vec<_>>());
        let mut faults = batch.faults();
        assert!(faults.by_ref().count() == count && faults.next().is_none());
    }
}

#[test]
fn notes_off_still_lists_refused_cells_and_long_rows() {
    let plan = [Column::i32(0)];
    let dialect = Dialect::CSV
        .with_header(false)
        .with_long_rows(LongRows::Report);
    let mut reader = DelimitedReader::options()
        .notes(false)
        .from_slice(b"x\n1,2\n", dialect, &plan)
        .unwrap();
    let batch = reader.read().unwrap().unwrap();
    let problems: Vec<Problem> = batch.faults().collect();
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[0].reason, Some(Reason::Malformed));
    assert_eq!(problems[0].detail, None);
    assert_eq!(problems[0].origin, Origin::Cell);
    assert_eq!((problems[1].expected, problems[1].found), (1, 2));
}

fn workbook(file: &str) -> Workbook<'static> {
    Workbook::open(format!(
        "{}/../corpus/workbook/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn a_sheets_faults_say_its_own_rows_and_columns() {
    // A range: ordinal 0 of the plan is the range's first column, and a problem's line and source
    // column stay the sheet's own.
    let plan = [Column::u8(0), Column::u8(1)];
    for file in ["ranges.xlsx", "ranges.ods", "ranges.numbers"] {
        let book = workbook(file);
        for (range, base) in [(None, 0), (Some(Range::parse("B1:D30").unwrap()), 1usize)] {
            let mut options = SheetOptions::default().with_header(false);
            if let Some(range) = range {
                options = options.with_range(range);
            }
            let mut sheet = book.sheet(0, options, &plan).unwrap();
            let mut seen = 0;
            while let Some(batch) = sheet.read().unwrap() {
                for problem in batch.faults() {
                    let column = problem.plan_column.unwrap();
                    assert_eq!(problem.source_column, Some(base + column), "{file}");
                    assert_eq!(problem.line, batch.line(problem.row), "{file}");
                    assert!(batch.verdicts(column)[problem.row].fault().is_some());
                    seen += 1;
                }
            }
            assert!(seen > 0, "{file}: the sheet's text fails a number door");
        }
    }
}

#[test]
fn a_formula_with_no_result_is_listed_unless_the_read_kept_no_notes() {
    let plan: Vec<Column> = (0..8).map(Column::text).collect();
    for file in ["uncached.xlsx", "uncached.ods"] {
        let book = workbook(file);
        let delivered = SheetOptions::default()
            .with_header(false)
            .with_empty_rows_skipped(false);
        let mut uncached = 0;
        let mut sheet = book.sheet(0, delivered, &plan).unwrap();
        while let Some(batch) = sheet.read().unwrap() {
            let mut told = 0;
            for column in 0..plan.len() {
                for row in 0..batch.rows() {
                    told += usize::from(batch.origin(column, row) == Origin::Uncached);
                }
            }
            let listed: Vec<Problem> = batch
                .faults()
                .filter(|problem| problem.origin == Origin::Uncached)
                .collect();
            assert_eq!(listed.len(), told, "{file}");
            assert!(listed.iter().all(|p| p.reason == Some(Reason::Empty)));
            assert!(listed.iter().all(|p| p.detail.is_none()));
            uncached += told;
        }
        assert!(uncached > 0, "{file}");

        let mut sheet = book.sheet(0, delivered.with_notes(false), &plan).unwrap();
        while let Some(batch) = sheet.read().unwrap() {
            assert!(
                batch.faults().all(|p| p.origin != Origin::Uncached),
                "{file}"
            );
            assert!(
                batch.faults().all(|p| p.reason != Some(Reason::Empty)),
                "{file}"
            );
        }
    }
}
