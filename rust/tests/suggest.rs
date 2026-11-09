//! The Rust surface of the suggester: a path, bytes or a sheet in, a plan and a report out, and
//! nothing a read ever sees. What the doors make of a value is `kernel_delimited`'s `suggesting`.

use hypertabular::{
    Candidate, Column, CurrencySymbol, DateOrder, DelimitedReader, Dialect, Door, Error,
    FailureKind, Grouping, LongRows, NativeDigits, NumFormat, SheetOptions, ShortRows,
    SuggestOptions, SuggestStatus, Suggestion, Workbook,
};
use std::path::{Path, PathBuf};

#[path = "support/packages.rs"]
mod packages;

const WORKBOOKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook");

fn suggest(bytes: &[u8]) -> Suggestion {
    DelimitedReader::suggest_slice(bytes, Dialect::CSV, &SuggestOptions::new()).unwrap()
}

fn doors(suggestion: &Suggestion) -> Vec<Door> {
    suggestion.report.iter().map(|column| column.door).collect()
}

fn plan_doors(suggestion: &Suggestion) -> Vec<Door> {
    suggestion.plan.iter().map(|column| column.door).collect()
}

fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_report_says_what_the_plan_does_and_why() {
    let found = suggest(
        b"flag,id,big,score,day,at,key,name,mixed,sep,order,none\n\
          true,1,5000000000,2.5,2026-01-07,2026-01-07T00:00:00Z,6ba7b810-9dad-11d1-80b4-00c04fd430c8,abc,1,\"1,234\",01/02/2026,\n\
          false,2,6000000000,3,2026-01-08,2026-01-08T00:00:00Z,6ba7b811-9dad-11d1-80b4-00c04fd430c8,def,x,\"2,345\",03/04/2026,\n",
    );
    assert_eq!(
        doors(&found),
        [
            Door::Bool,
            Door::I32,
            Door::I64,
            Door::F64,
            Door::Date,
            Door::Timestamp,
            Door::Uuid,
            Door::Text,
            Door::Text,
            Door::I32,
            Door::DateOrdered(DateOrder::MonthDayYear),
            Door::Text,
        ]
    );
    // Only what's resolved is read through a door: the last four read as text.
    assert_eq!(
        plan_doors(&found),
        [
            Door::Bool,
            Door::I32,
            Door::I64,
            Door::F64,
            Door::Date,
            Door::Timestamp,
            Door::Uuid,
            Door::Text,
            Door::Text,
            Door::Text,
            Door::Text,
            Door::Text,
        ]
    );
    let statuses: Vec<SuggestStatus> = found.report.iter().map(|column| column.status).collect();
    use SuggestStatus::{Ambiguous, Resolved, Unresolved};
    assert_eq!(
        statuses,
        [
            Resolved, Resolved, Resolved, Resolved, Resolved, Resolved, Resolved, Resolved,
            Unresolved, Ambiguous, Ambiguous, Unresolved,
        ]
    );
    for (ordinal, column) in found.report.iter().enumerate() {
        assert_eq!(column.ordinal, ordinal);
        assert_eq!(found.plan[ordinal].ordinal, ordinal);
    }
    assert_eq!(found.report[0].name.as_deref(), Some(&b"flag"[..]));
    assert_eq!(found.report[11].name.as_deref(), Some(&b"none"[..]));
    assert_eq!((found.report[8].matched, found.report[8].sampled), (1, 2));
    assert_eq!((found.report[11].matched, found.report[11].sampled), (0, 0));
    assert_eq!((found.report[9].matched, found.report[9].sampled), (2, 2));
    assert!(found.report[0].candidates.is_empty());
    assert_eq!(
        found.report[10].candidates,
        [
            Candidate::DateOrder(DateOrder::MonthDayYear),
            Candidate::DateOrder(DateOrder::DayMonthYear),
        ]
    );
    let [Candidate::NumFormat(first), Candidate::NumFormat(second)] =
        found.report[9].candidates[..]
    else {
        panic!("two notations: {:?}", found.report[9].candidates);
    };
    assert_eq!(first, NumFormat::INVARIANT);
    assert_eq!(
        (first.decimal_sep.as_str(), first.group_sep.as_str()),
        (".", ",")
    );
    assert_eq!(
        (second.decimal_sep.as_str(), second.group_sep.as_str()),
        (",", ".")
    );
    assert!(!found.truncated);
}

/// Every door a plan names reads its sample back with no fault, and with the values it was shown.
#[test]
fn a_plan_reads_back_what_was_sampled() {
    let bytes = b"flag,id,score,day,order,name\n\
        1,2,2.5,2026-01-07,13/02/2026,abc\n\
        0,3,3,2026-01-08,01/02/2026,def\n";
    let found = suggest(bytes);
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.rows(), 2);
    assert_eq!(batch.faults().count(), 0);
    assert_eq!(batch.values::<bool>(0), [true, false]);
    assert_eq!(batch.values::<i32>(1), [2, 3]);
    assert_eq!(batch.values::<f64>(2), [2.5, 3.0]);
    let day = batch.values::<hypertabular::Date>(4)[0];
    assert_eq!((day.year, day.month, day.day), (2026, 2, 13));
    assert_eq!(batch.text(5, 1).unwrap(), b"def");
}

#[test]
fn a_declared_format_reaches_the_plan() {
    let bytes = b"a,b\n\"1,234\",\"1,234\"\n";
    let options = SuggestOptions::new()
        .format(NumFormat::INVARIANT)
        .column_format(1, NumFormat::new(',', '.', NumFormat::ALL));
    let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &options).unwrap();
    assert_eq!(doors(&found), [Door::I32, Door::F64]);
    assert_eq!(found.plan[1].format.decimal_sep.as_str(), ",");
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.values::<i32>(0), [1234]);
    assert_eq!(batch.values::<f64>(1), [1.234]);
}

#[test]
fn the_last_format_declared_for_a_column_is_its_format() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    let options = SuggestOptions::new()
        .column_format(0, NumFormat::INVARIANT)
        .column_format(0, comma);
    let found = DelimitedReader::suggest_slice(b"a\n\"1,234\"\n", Dialect::CSV, &options).unwrap();
    assert_eq!(doors(&found), [Door::F64]);
}

#[test]
fn a_format_the_core_refuses_is_a_plan_error() {
    let options = SuggestOptions::new().column_format(1, NumFormat::new('.', '.', 0));
    let error = DelimitedReader::suggest_slice(b"a,b\n1,2\n", Dialect::CSV, &options).unwrap_err();
    assert!(matches!(error, Error::Plan { column: 1 }), "{error:?}");
}

/// The contract at the top of `CLAUDE.md`: nothing is sniffed in a read.
#[test]
fn suggest_applies_nothing() {
    let bytes = b"id,score\n1,2.5\n2,3.5\n";
    let path = scratch("suggest_applies_nothing.csv", bytes);
    let found = DelimitedReader::suggest(&path, Dialect::CSV, &SuggestOptions::new()).unwrap();
    assert_eq!(doors(&found), [Door::I32, Door::F64]);

    // A reader is opened from the same file with a plan of its own, and has that plan.
    let declared = [Column::text(0), Column::text(1)];
    let mut reader = DelimitedReader::open(&path, Dialect::CSV, &declared).unwrap();
    assert_eq!(reader.plan(), declared);
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.text(0, 0).unwrap(), b"1");
    assert_eq!(batch.text(1, 0).unwrap(), b"2.5");

    // One opened without a plan still has none, and says so when read.
    let mut unbound = DelimitedReader::from_slice_unbound(bytes, Dialect::CSV).unwrap();
    let _ = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &SuggestOptions::new()).unwrap();
    assert!(!unbound.is_bound());
    assert!(unbound.plan().is_empty());
    assert!(matches!(unbound.read(), Err(Error::Unbound)));

    // And the same of a sheet.
    let book = Workbook::open(Path::new(WORKBOOKS).join("basic.xlsx")).unwrap();
    let mut sheet = book.sheet_unbound(0, SheetOptions::new()).unwrap();
    let _ = sheet.suggest(&SuggestOptions::new()).unwrap();
    assert!(!sheet.is_bound());
    assert!(sheet.plan().is_empty());
    assert!(matches!(sheet.read(), Err(Error::Unbound)));
}

/// A sheet answers from its first row whatever it has been up to, and carries on where it was.
#[test]
fn a_bound_or_part_read_sheet_answers_the_same_and_stays_where_it_is() {
    let book = Workbook::open(Path::new(WORKBOOKS).join("libreoffice-data.xlsx")).unwrap();
    let options = SheetOptions::new().with_batch_rows(1);
    let fresh = book
        .sheet_unbound(0, options)
        .unwrap()
        .suggest(&SuggestOptions::new().sample_rows(1))
        .unwrap();
    let width = fresh.plan.len();
    assert!(width > 0);

    let plan: Vec<Column> = (0..width).map(Column::text).collect();
    let mut whole = book.sheet(0, options, &plan).unwrap();
    let mut rows = Vec::new();
    while let Some(batch) = whole.read().unwrap() {
        rows.push(batch.text(0, 0).map(<[u8]>::to_vec));
    }
    assert!(
        rows.len() > 1,
        "the fixture has a second row to come back to"
    );

    let mut sheet = book.sheet(0, options, &plan).unwrap();
    let asked = SuggestOptions::new().sample_rows(1);
    assert_eq!(sheet.suggest(&asked).unwrap(), fresh, "bound, unread");
    let first = sheet
        .read()
        .unwrap()
        .unwrap()
        .text(0, 0)
        .map(<[u8]>::to_vec);
    assert_eq!(first, rows[0]);
    assert_eq!(
        sheet.suggest(&asked).unwrap(),
        fresh,
        "part of the way through"
    );
    assert_eq!(sheet.plan(), plan);
    let second = sheet
        .read()
        .unwrap()
        .unwrap()
        .text(0, 0)
        .map(<[u8]>::to_vec);
    assert_eq!(second, rows[1], "it carries on from where it was");
}

#[test]
fn a_named_sheet_is_the_sheet_sampled() {
    let book = Workbook::open(Path::new(WORKBOOKS).join("multisheet.xlsx")).unwrap();
    assert!(book.sheets().len() > 1);
    for at in 0..book.sheets().len() {
        let name = book.sheets()[at].name.clone();
        let by_index = book.sheet_unbound(at, SheetOptions::new());
        let by_name = book.sheet_unbound(name.as_str(), SheetOptions::new());
        match (by_index, by_name) {
            (Ok(index), Ok(name)) => {
                let asked = SuggestOptions::new();
                assert_eq!(
                    index.suggest(&asked).unwrap(),
                    name.suggest(&asked).unwrap()
                );
            }
            (Err(_), Err(_)) => {}
            (a, b) => panic!("{}: {:?} and {:?}", at, a.is_ok(), b.is_ok()),
        }
    }
}

fn rows_of(count: usize) -> Vec<u8> {
    let mut bytes = b"a\n".to_vec();
    for row in 1..=count {
        bytes.extend_from_slice(format!("{}\n", row + 1).as_bytes());
    }
    bytes
}

#[test]
fn a_sample_of_exactly_the_source_is_not_truncated() {
    let exact = SuggestOptions::new().sample_rows(3);
    for (rows, truncated) in [(2, false), (3, false), (4, true), (5, true), (9000, true)] {
        let bytes = rows_of(rows);
        let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &exact).unwrap();
        assert_eq!(found.truncated, truncated, "{rows} rows from a slice");
        assert_eq!(found.report[0].sampled, rows.min(3) as u32, "{rows} rows");
        let path = scratch("suggest_exact.csv", &bytes);
        let found = DelimitedReader::suggest(&path, Dialect::CSV, &exact).unwrap();
        assert_eq!(found.truncated, truncated, "{rows} rows from a file");
    }
}

#[test]
fn a_sample_is_a_thousand_rows_and_at_most_a_batch() {
    let bytes = rows_of(9000);
    let default = suggest(&bytes);
    assert_eq!(default.report[0].sampled, 1000);
    assert!(default.truncated);
    let most = SuggestOptions::new().sample_rows(1_000_000);
    let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &most).unwrap();
    assert_eq!(found.report[0].sampled, 4096);
    assert!(found.truncated);
    let bytes = rows_of(4096);
    let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &most).unwrap();
    assert_eq!(found.report[0].sampled, 4096);
    assert!(!found.truncated, "one batch exactly");
    // Zero is one, as a batch size is.
    let one = SuggestOptions::new().sample_rows(0);
    let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &one).unwrap();
    assert_eq!(found.report[0].sampled, 1);
}

/// A later row can't change a suggestion that didn't see it: that's what `truncated` warns of.
#[test]
fn what_the_sample_did_not_reach_has_no_say() {
    let mut bytes = rows_of(3);
    bytes.extend_from_slice(b"oops\n");
    let two = SuggestOptions::new().sample_rows(2);
    let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &two).unwrap();
    assert_eq!(doors(&found), [Door::I32]);
    assert_eq!(found.report[0].status, SuggestStatus::Resolved);
    assert!(found.truncated);
    let all = SuggestOptions::new();
    let found = DelimitedReader::suggest_slice(&bytes, Dialect::CSV, &all).unwrap();
    assert_eq!(found.report[0].status, SuggestStatus::Unresolved);
}

#[test]
fn a_sheet_s_sample_is_at_most_its_batch() {
    let book = Workbook::open(Path::new(WORKBOOKS).join("libreoffice-data.xlsx")).unwrap();
    let sheet = book
        .sheet_unbound(0, SheetOptions::new().with_batch_rows(1))
        .unwrap();
    let found = sheet.suggest(&SuggestOptions::new()).unwrap();
    assert!(found.truncated);
    assert!(found.report.iter().all(|column| column.sampled <= 1));
}

#[test]
fn a_failure_among_the_sampled_rows_is_said_and_one_past_them_is_not() {
    let bytes = b"a,b\n1,2\n3,4\n5\n7,8\n";
    let error =
        DelimitedReader::suggest_slice(bytes, Dialect::CSV, &SuggestOptions::new()).unwrap_err();
    let Error::Structure(failure) = error else {
        panic!("{error:?}");
    };
    assert_eq!(failure.kind, FailureKind::ColumnCount);
    // The row right after the sample says there's more, and no more than that.
    let two = SuggestOptions::new().sample_rows(2);
    let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &two).unwrap();
    assert!(found.truncated);
    assert_eq!(found.report[0].sampled, 2);
    // Declared, a ragged row is a read like any other, and what a row hasn't got is blank.
    let padded = Dialect::CSV.with_short_rows(ShortRows::Pad);
    let found = DelimitedReader::suggest_slice(bytes, padded, &SuggestOptions::new()).unwrap();
    assert_eq!(found.report[1].sampled, 3);
    let cut = Dialect::CSV.with_long_rows(LongRows::Truncate);
    let found =
        DelimitedReader::suggest_slice(b"a,b\n1,2,3\n", cut, &SuggestOptions::new()).unwrap();
    assert_eq!(found.plan.len(), 2);
}

#[test]
fn a_column_of_tokens_is_a_column_of_blanks() {
    let dialect = Dialect::CSV.with_tokens(&["NA", "-"]);
    let bytes = b"a,b,c\nNA,1,\"NA\"\n-,NA,\"NA\"\nNA,3,\"NA\"\n";
    let found = DelimitedReader::suggest_slice(bytes, dialect, &SuggestOptions::new()).unwrap();
    // Nothing but tokens: nothing to go on, as a blank column has.
    assert_eq!(found.report[0].status, SuggestStatus::Unresolved);
    assert_eq!((found.report[0].matched, found.report[0].sampled), (0, 0));
    assert_eq!(found.plan[0].door, Door::Text);
    // A token among numbers doesn't stop them being numbers.
    assert_eq!(found.report[1].door, Door::I32);
    assert_eq!(found.report[1].status, SuggestStatus::Resolved);
    assert_eq!((found.report[1].matched, found.report[1].sampled), (2, 2));
    // A quoted cell is never a token, so it's the text NA.
    assert_eq!(found.report[2].door, Door::Text);
    assert_eq!(found.report[2].status, SuggestStatus::Resolved);
    assert_eq!(found.report[2].sampled, 3);
}

#[test]
fn invalid_text_is_a_failed_cell_and_not_a_blank() {
    let found = suggest(b"a,b\n\xe9t\xe9,1\nabc,\xff\n");
    assert_eq!(found.report[0].status, SuggestStatus::Unresolved);
    assert_eq!((found.report[0].matched, found.report[0].sampled), (0, 2));
    assert_eq!((found.report[1].matched, found.report[1].sampled), (1, 2));
}

#[test]
fn a_quote_s_doubled_text_is_what_is_judged() {
    let found = suggest(b"a\n\"1\"\"\"\n");
    assert_eq!(found.report[0].door, Door::Text);
    let found = suggest(b"a\n\"1\"\n\"2\"\n");
    assert_eq!(found.report[0].door, Door::I32);
}

#[test]
fn no_header_has_no_names_and_the_first_record_s_width() {
    let dialect = Dialect::CSV.with_header(false);
    let found =
        DelimitedReader::suggest_slice(b"1,x\n2,y\n", dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(doors(&found), [Door::I32, Door::Text]);
    assert_eq!(found.report[0].name, None);
    assert_eq!(found.report[0].sampled, 2, "the first record is a row too");
}

#[test]
fn nothing_in_the_source_is_nothing_to_plan() {
    for dialect in [Dialect::CSV, Dialect::CSV.with_header(false)] {
        let found = DelimitedReader::suggest_slice(b"", dialect, &SuggestOptions::new()).unwrap();
        assert!(found.plan.is_empty());
        assert!(found.report.is_empty());
        assert!(!found.truncated);
    }
    // A header and no rows: columns with nothing to go on.
    let found = suggest(b"a,b\n");
    assert_eq!(doors(&found), [Door::Text, Door::Text]);
    assert_eq!(found.report[0].status, SuggestStatus::Unresolved);
    assert_eq!(found.report[0].sampled, 0);
}

#[test]
fn a_dialect_s_declarations_are_the_sample_s() {
    let dialect = Dialect::TSV.with_skip(1).with_header_row(1);
    let bytes = b"a title, with a comma\nignored\tcolumns\nid\tscore\n7\t2.5\n";
    let found = DelimitedReader::suggest_slice(bytes, dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(doors(&found), [Door::I32, Door::F64]);
    assert_eq!(found.report[1].name.as_deref(), Some(&b"score"[..]));
    // Latin-1, decoded before anything's judged, and bytes and a file say the same.
    let dialect = Dialect::CSV.with_encoding(hypertabular::Encoding::Latin1);
    let bytes = b"caf\xe9\n7\n8\n";
    let found = DelimitedReader::suggest_slice(bytes, dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(found.report[0].name.as_deref(), Some("café".as_bytes()));
    assert_eq!(found.report[0].door, Door::I32);
    let path = scratch("suggest_latin1.csv", bytes);
    let from_file = DelimitedReader::suggest(&path, dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(from_file, found);
}

#[test]
fn a_path_that_is_not_there_is_an_io_error() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("suggest_missing.csv");
    let error =
        DelimitedReader::suggest(&missing, Dialect::CSV, &SuggestOptions::new()).unwrap_err();
    assert!(matches!(error, Error::Io(_)), "{error:?}");
}

#[test]
fn a_sheet_with_no_header_has_no_names_and_its_first_row_s_width() {
    let book = Workbook::open(Path::new(WORKBOOKS).join("basic.xlsx")).unwrap();
    let options = SheetOptions::new().with_header(false);
    let found = book
        .sheet_unbound(0, options)
        .unwrap()
        .suggest(&SuggestOptions::new())
        .unwrap();
    assert_eq!(found.plan.len(), 9);
    assert_eq!(found.report[0].name, None);
    // The header row is a row here: `id` and its four numbers, one door taking four of five.
    assert_eq!(
        (found.report[0].door, found.report[0].matched),
        (Door::Text, 4)
    );
    assert_eq!(found.report[0].sampled, 5);
}

/// What each writer's date, number and boolean cells come to, through the text door, in all three
/// formats. A date through it is `2023-03-15T00:00:00`, which has no zone, so a typed date column
/// suggests text, and the text it reads is what's in the sheet.
#[test]
fn a_typed_cell_is_judged_by_what_the_text_door_says_of_it() {
    use SuggestStatus::{Resolved, Unresolved};
    let suggested = |file: &str| {
        let book = Workbook::open(Path::new(WORKBOOKS).join(file)).unwrap();
        let sheet = book.sheet_unbound(0, SheetOptions::new()).unwrap();
        sheet.suggest(&SuggestOptions::new()).unwrap()
    };
    let column = |found: &Suggestion, name: &str| {
        let column = found
            .report
            .iter()
            .find(|column| column.name.as_deref() == Some(name.as_bytes()))
            .unwrap_or_else(|| panic!("{name}"));
        (column.door, column.status)
    };
    for file in [
        "basic.xlsx",
        "libreoffice-data.xlsx",
        "libreoffice-data.ods",
        "numbers-data.numbers",
    ] {
        let found = suggested(file);
        assert_eq!(column(&found, "id"), (Door::I32, Resolved), "{file}");
        assert_eq!(column(&found, "flag"), (Door::Bool, Resolved), "{file}");
    }
    // A date, a date-time and a time of day, each as its writer's canonical text: no door here
    // takes them, so each is a text column, read as what the text door says.
    for (file, day) in [
        ("basic.xlsx", "day"),
        ("libreoffice-data.ods", "day"),
        ("numbers-data.numbers", "day"),
        ("basic.xlsx", "when"),
        ("libreoffice-data.ods", "when"),
        ("numbers-data.numbers", "when"),
        ("basic.xlsx", "at"),
        ("libreoffice-data.ods", "at"),
        ("numbers-data.numbers", "at"),
        ("libreoffice-data.ods", "elapsed"),
    ] {
        let found = suggested(file);
        assert_eq!(column(&found, day), (Door::Text, Resolved), "{file}: {day}");
    }
    // A fraction, a percentage's value and a whole number held as a float.
    assert_eq!(
        column(&suggested("basic.xlsx"), "amount"),
        (Door::F64, Resolved)
    );
    assert_eq!(
        column(&suggested("numbers-data.numbers"), "pct"),
        (Door::F64, Resolved)
    );
    assert_eq!(
        column(&suggested("libreoffice-data.ods"), "pct"),
        (Door::F64, Resolved)
    );
    assert_eq!(
        column(&suggested("basic.ods"), "money"),
        (Door::I32, Resolved)
    );
    // A column that mixes kinds says so, with a formula's cell an empty one.
    assert_eq!(
        column(&suggested("numbers-data.numbers"), "calc"),
        (Door::Text, Unresolved)
    );
    assert_eq!(
        column(&suggested("basic.ods"), "stamp"),
        (Door::Text, Unresolved)
    );
}

/// Whether a read of every row of a sheet, through its first column, meets a failure.
fn read_fails(book: &Workbook<'_>, at: usize, options: SheetOptions) -> bool {
    let Ok(mut sheet) = book.sheet(at, options, &[Column::text(0)]) else {
        return true;
    };
    loop {
        match sheet.read() {
            Ok(Some(_)) => {}
            Ok(None) => return false,
            Err(_) => return true,
        }
    }
}

/// What was resolved reads back through its door over every sampled row of every sheet of every
/// package the corpus names, under each thing a caller can declare and under nothing: no typed cell
/// is refused by the door the sample suggested, whichever format that is in.
#[test]
fn no_sheet_s_plan_refuses_a_cell_it_sampled() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    let declarations = [
        ("nothing", None),
        ("Western", Some(NumFormat::INVARIANT)),
        ("a comma decimal", Some(comma)),
        (
            "Indian",
            Some(NumFormat::INVARIANT.with_grouping(Grouping::Indian)),
        ),
    ];
    let mut planned = 0;
    let mut typed = 0;
    let mut refused_by_suggest = 0;
    for entry in std::fs::read_dir(WORKBOOKS).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "py") {
            continue;
        }
        let Ok(book) = Workbook::open(&path) else {
            continue;
        };
        for at in 0..book.sheets().len() {
            for (declared, format) in declarations {
                let Ok(sheet) = book.sheet_unbound(at, SheetOptions::new()) else {
                    continue;
                };
                let mut asked = SuggestOptions::new();
                if let Some(format) = format {
                    asked = asked.format(format);
                }
                // A package that's broken among its sampled rows fails, and only if its read does.
                let found = match sheet.suggest(&asked) {
                    Ok(found) => found,
                    Err(error) => {
                        assert!(
                            read_fails(&book, at, SheetOptions::new()),
                            "{path:?} sheet {at}: {error:?}, where a read has no failure"
                        );
                        refused_by_suggest += 1;
                        continue;
                    }
                };
                if found.plan.is_empty() {
                    continue;
                }
                planned += 1;
                typed += found
                    .plan
                    .iter()
                    .filter(|column| column.door != Door::Text)
                    .count();
                let mut read = book.sheet(at, SheetOptions::new(), &found.plan).unwrap();
                let mut rows = 0;
                while rows < 1000 {
                    let Some(batch) = read.read().unwrap() else {
                        break;
                    };
                    let refused: Vec<_> = batch
                        .faults()
                        .filter(|problem| {
                            problem.row + rows < 1000
                                && matches!(
                                    problem.reason,
                                    Some(
                                        hypertabular::Reason::Malformed
                                            | hypertabular::Reason::OutOfRange
                                    )
                                )
                                && found.plan[problem.plan_column.unwrap()].door != Door::Text
                        })
                        .collect();
                    assert!(
                        refused.is_empty(),
                        "{path:?} sheet {at}, {declared} declared: {refused:?}"
                    );
                    rows += batch.rows();
                }
            }
        }
    }
    assert!(planned >= 80, "{planned} plans");
    assert!(typed >= 350, "{typed} columns planned through a door");
    assert!(refused_by_suggest >= 1, "broken.xlsx is among them");
}

/// A typed number has no locale. A number column of a workbook is a number whether a notation is
/// declared or not, and whichever: these are the columns a read of the file's own numbers gives,
/// from the three applications and the fixtures, and each comes out numeric under all of them.
#[test]
fn a_workbook_s_number_column_is_numeric_under_any_format() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    for (file, columns) in [
        ("basic.xlsx", &["amount", "ratio"][..]),
        ("basic.ods", &["amount", "pct"][..]),
        ("excel-web-data.xlsx", &["pct", "money"][..]),
    ] {
        let book = Workbook::open(format!("{WORKBOOKS}/{file}")).unwrap();
        for asked in [
            SuggestOptions::new(),
            SuggestOptions::new().format(NumFormat::INVARIANT),
            SuggestOptions::new().format(comma),
        ] {
            let found = book
                .sheet_unbound(0, SheetOptions::new())
                .unwrap()
                .suggest(&asked)
                .unwrap();
            for name in columns {
                let column = found
                    .report
                    .iter()
                    .find(|column| column.name.as_deref() == Some(name.as_bytes()))
                    .unwrap_or_else(|| panic!("{file} has no {name}"));
                assert_eq!(column.status, SuggestStatus::Resolved, "{file} {name}");
                assert!(column.door.is_numeric(), "{file} {name}: {:?}", column.door);
            }
        }
    }
}

/// The doors' own limits, on numbers a file can hold: what a typed cell suggests its door takes, in
/// every notation, the digits of its shortest form having no say.
#[test]
fn a_typed_number_at_a_door_s_limit_reads_through_the_door_it_suggests() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    let values = [
        "2147483647",
        "2147483648",
        "-2147483648",
        "-2147483649",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "18446744073709551616",
        "1e21",
        "1.5E300",
        "1.7976931348623157E+308",
        "5e-324",
        "-0",
        "0.1",
        "1234.5",
        "NaN",
        "Infinity",
        "1e999",
    ];
    for value in values {
        let data = format!("<row r=\"1\"><c><v>{value}</v></c></row>");
        let bytes = packages::xlsx(&[("Data", &data)], "", false, false);
        let book = Workbook::from_vec(bytes).unwrap();
        let options = SheetOptions::new().with_header(false);
        for asked in [
            SuggestOptions::new(),
            SuggestOptions::new().format(NumFormat::INVARIANT),
            SuggestOptions::new().format(comma),
        ] {
            let sheet = book.sheet_unbound(0, options).unwrap();
            let found = sheet.suggest(&asked).unwrap();
            let mut read = book.sheet(0, options, &found.plan).unwrap();
            let batch = read.read().unwrap().unwrap();
            let refused: Vec<_> = batch
                .faults()
                .filter(|problem| {
                    matches!(
                        problem.reason,
                        Some(hypertabular::Reason::Malformed | hypertabular::Reason::OutOfRange)
                    )
                })
                .collect();
            assert!(
                refused.is_empty(),
                "{value}: {:?} refused {refused:?}",
                found.report[0]
            );
            assert_ne!(found.report[0].status, SuggestStatus::Ambiguous, "{value}");
        }
    }
}

/// Text that looks like the number beside it keeps its question, in a sheet as in a text file.
#[test]
fn text_holding_a_number_keeps_the_owner_s_rule_in_a_sheet() {
    let book = Workbook::open(format!("{WORKBOOKS}/suggest.xlsx")).unwrap();
    let found = book
        .sheet_unbound(0, SheetOptions::new())
        .unwrap()
        .suggest(&SuggestOptions::new())
        .unwrap();
    assert_eq!(found.report[0].status, SuggestStatus::Resolved, "native");
    assert_eq!(found.report[1].status, SuggestStatus::Ambiguous, "text");
    assert_eq!(found.report[2].status, SuggestStatus::Ambiguous, "mixed");
    // The sample's notes are its own, whatever a read of the sheet was to have.
    let quiet = book
        .sheet_unbound(0, SheetOptions::new().with_notes(false))
        .unwrap()
        .suggest(&SuggestOptions::new())
        .unwrap();
    assert_eq!(quiet, found);
}

/// A declared order answers the sample's question and only that.
#[test]
fn a_declared_date_order_settles_a_date_the_sample_left_open() {
    let open = b"day\n01/02/2026\n03/04/2026\n";
    let found = suggest(open);
    assert_eq!(found.report[0].status, SuggestStatus::Ambiguous);
    assert_eq!(found.plan[0].door, Door::Text);
    let asked = SuggestOptions::new().date_order(DateOrder::DayMonthYear);
    let found = DelimitedReader::suggest_slice(open, Dialect::CSV, &asked).unwrap();
    assert_eq!(found.report[0].status, SuggestStatus::Resolved);
    assert!(found.report[0].candidates.is_empty());
    assert_eq!(
        found.plan[0].door,
        Door::DateOrdered(DateOrder::DayMonthYear)
    );
    assert_eq!(
        found.report[0].door,
        Door::DateOrdered(DateOrder::DayMonthYear)
    );
    // A column's own wins, the last one given.
    let asked = SuggestOptions::new()
        .date_order(DateOrder::DayMonthYear)
        .column_date_order(0, DateOrder::DayMonthYear)
        .column_date_order(0, DateOrder::MonthDayYear);
    let found = DelimitedReader::suggest_slice(open, Dialect::CSV, &asked).unwrap();
    assert_eq!(
        found.plan[0].door,
        Door::DateOrdered(DateOrder::MonthDayYear)
    );
    // An order the column's cells rule out doesn't overrule them.
    let settled = b"day\n13/02/2026\n";
    let asked = SuggestOptions::new().date_order(DateOrder::MonthDayYear);
    let found = DelimitedReader::suggest_slice(settled, Dialect::CSV, &asked).unwrap();
    assert_eq!(
        found.plan[0].door,
        Door::DateOrdered(DateOrder::DayMonthYear)
    );
    // An order out of the question's set, as year-first is, leaves the question open.
    let asked = SuggestOptions::new().date_order(DateOrder::YearMonthDay);
    let found = DelimitedReader::suggest_slice(open, Dialect::CSV, &asked).unwrap();
    assert_eq!(found.report[0].status, SuggestStatus::Ambiguous);
    assert_eq!(found.plan[0].door, Door::Text);
}

#[test]
fn a_comma_that_proved_to_group_stays_western_in_the_plan() {
    let bytes = b"a\n\"1,234\"\n2.5\n\"1,23,456\"\n";
    let two = SuggestOptions::new().sample_rows(2);
    let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &two).unwrap();
    assert_eq!(found.report[0].door, Door::F64);
    assert_eq!(found.report[0].status, SuggestStatus::Resolved);
    assert_eq!(found.report[0].format, Some(NumFormat::INVARIANT));
    assert_eq!(found.plan[0].format, NumFormat::INVARIANT);
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.values::<f64>(0)[..2], [1234.0, 2.5]);
    let refused = batch.get::<f64>(0, 2).unwrap_err();
    assert_eq!(refused.reason, hypertabular::Reason::Malformed);

    // A month-day-year date's `param` is the same `2`, and says nothing of a notation.
    let found = suggest(b"a\n02/13/2026\n");
    assert_eq!(
        found.report[0].door,
        Door::DateOrdered(DateOrder::MonthDayYear)
    );
    assert_eq!(found.report[0].format, None);
    assert_eq!(found.plan[0].format, NumFormat::INVARIANT);

    // Declared, it's the declared format, and the report names that.
    let indian = NumFormat::INVARIANT.with_grouping(Grouping::Indian);
    let declared = SuggestOptions::new().format(indian);
    let found =
        DelimitedReader::suggest_slice(b"a\n\"1,23,456\"\n", Dialect::CSV, &declared).unwrap();
    assert_eq!(found.report[0].format, Some(indian));
    assert_eq!(found.plan[0].format, indian);
    let mut reader =
        DelimitedReader::from_slice(b"a\n\"1,23,456\"\n", Dialect::CSV, &found.plan).unwrap();
    assert_eq!(reader.read().unwrap().unwrap().get::<i32>(0, 0), Ok(123456));

    // A column that isn't a resolved number names no format: the plan's is the default it ignores.
    let found = suggest(b"a\nx\n");
    assert_eq!(found.report[0].format, None);
    let found = suggest(b"a\n\"1,234\"\n");
    assert_eq!(found.report[0].status, SuggestStatus::Ambiguous);
    assert_eq!(found.report[0].format, None);
}

/// No comma in the sample proves nothing, and the plan is Western all the same: a later `1,23,456` is
/// refused, in the same words whichever way the column was settled.
#[test]
fn a_number_with_no_format_declared_is_planned_western() {
    let two = SuggestOptions::new().sample_rows(2);
    let bytes = b"a\n7\n8\n\"1,23,456\"\n";
    let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &two).unwrap();
    assert_eq!(found.report[0].door, Door::I32);
    assert_eq!(found.report[0].status, SuggestStatus::Resolved);
    assert_eq!(found.report[0].format, Some(NumFormat::INVARIANT));
    assert_eq!(found.plan[0].format, NumFormat::INVARIANT);
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.values::<i32>(0)[..2], [7, 8]);
    let refused = batch.get::<i32>(0, 2).unwrap_err();
    assert_eq!(refused.reason, hypertabular::Reason::Malformed);
}

/// A cell that isn't text in the first record is a verdict on that cell, as a read says: only a
/// header it would be structural in, and this has none.
#[test]
fn a_headerless_first_record_is_not_judged_as_a_header() {
    let dialect = Dialect::CSV.with_header(false);
    let bytes = b"\xe9t\xe9,1\nx,2\n";
    let found = DelimitedReader::suggest_slice(bytes, dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(found.plan.len(), 2);
    assert_eq!(found.report[0].status, SuggestStatus::Unresolved);
    assert_eq!((found.report[0].matched, found.report[0].sampled), (0, 2));
    assert_eq!(found.report[1].door, Door::I32);
    let path = scratch("suggest_headerless_bad.csv", bytes);
    let from_file = DelimitedReader::suggest(&path, dialect, &SuggestOptions::new()).unwrap();
    assert_eq!(from_file, found);

    // With a header it is structural, and a read says the same.
    let header = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &SuggestOptions::new());
    assert!(matches!(header, Err(Error::Structure(_))));
    assert!(DelimitedReader::from_slice_unbound(bytes, Dialect::CSV).is_err());
}

/// What a sample says of a lone zero's group is what a read of it says, since both ask the door.
/// Undeclared, only the other notation reads it, and the plan is in that one; declared, a format
/// that refuses it is refused it by the suggestion and the read alike.
#[test]
fn a_suggestion_and_a_read_agree_on_a_lone_zero() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    for (bytes, format) in [
        (&b"a\n\"0,125\"\n"[..], NumFormat::INVARIANT),
        (&b"a\n\"0,125\"\n"[..], NumFormat::INVARIANT),
        (&b"a\n0.125\n"[..], comma),
    ] {
        let declared = SuggestOptions::new().format(format);
        let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &declared).unwrap();
        assert_eq!(found.report[0].door, Door::Text, "{format:?}");
        assert_eq!(found.report[0].status, SuggestStatus::Resolved);
        let plan = [Column::f64(0).format(format)];
        let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &plan).unwrap();
        let refused = reader
            .read()
            .unwrap()
            .unwrap()
            .get::<f64>(0, 0)
            .unwrap_err();
        assert_eq!(
            refused.reason,
            hypertabular::Reason::Malformed,
            "{format:?}"
        );
    }

    let found = suggest(b"a\n\"0,125\"\n");
    assert_eq!(found.report[0].format, Some(comma));
    let mut reader =
        DelimitedReader::from_slice(b"a\n\"0,125\"\n", Dialect::CSV, &found.plan).unwrap();
    assert_eq!(reader.read().unwrap().unwrap().values::<f64>(0), [0.125]);

    // Beside `1,234`, it's what rules out the comma decimal.
    let found = suggest(b"a\n\"1,234\"\n0.125\n");
    assert_eq!(found.report[0].door, Door::F64);
    assert_eq!(found.report[0].status, SuggestStatus::Resolved);
    assert_eq!(found.report[0].format, Some(NumFormat::INVARIANT));

    let found = suggest(b"a\n0.125\n");
    assert_eq!(found.report[0].format, Some(NumFormat::INVARIANT));
    let mut reader = DelimitedReader::from_slice(b"a\n0.125\n", Dialect::CSV, &found.plan).unwrap();
    assert_eq!(reader.read().unwrap().unwrap().values::<f64>(0), [0.125]);
}

/// A column only the comma decimal reads is resolved in it, undeclared, and reads back through its
/// plan, each shape to the value it says. What that notation refuses past the sample is refused.
#[test]
fn a_column_only_the_comma_decimal_reads_is_planned_in_it() {
    let comma = NumFormat::new(',', '.', NumFormat::ALL);
    let bytes = b"f,i\n\"2,5\",1.234.567\n\"1.234,5\",1.000\n\"1,234\",\"1,234,567\"\n";
    let two = SuggestOptions::new().sample_rows(2);
    let found = DelimitedReader::suggest_slice(bytes, Dialect::CSV, &two).unwrap();
    assert_eq!(doors(&found), [Door::F64, Door::I32]);
    for column in &found.report {
        assert_eq!(column.status, SuggestStatus::Resolved);
        assert_eq!(column.format, Some(comma));
        assert_eq!(column.candidates, []);
    }
    assert_eq!(found.plan[0].format, comma);
    assert_eq!(found.plan[1].format, comma);
    let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.values::<f64>(0), [2.5, 1234.5, 1.234]);
    assert_eq!(batch.values::<i32>(1)[..2], [1234567, 1000]);
    assert_eq!(
        batch.get::<i32>(1, 2).unwrap_err().reason,
        hypertabular::Reason::Malformed
    );
}

/// An ambiguous column plans as text and offers both notations, each reading a different value.
#[test]
fn a_question_reads_as_text_and_each_answer_reads_its_own_value() {
    let bytes = b"a\n1.234\n";
    let found = suggest(bytes);
    assert_eq!(found.report[0].status, SuggestStatus::Ambiguous);
    assert_eq!(found.report[0].format, None);
    assert_eq!(found.plan[0].door, Door::Text);
    let answers: Vec<NumFormat> = found.report[0]
        .candidates
        .iter()
        .map(|candidate| match candidate {
            Candidate::NumFormat(format) => *format,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(answers.len(), 2);
    let read = |format: NumFormat| {
        let plan = [Column::f64(0).format(format)];
        let mut reader = DelimitedReader::from_slice(bytes, Dialect::CSV, &plan).unwrap();
        reader.read().unwrap().unwrap().values::<f64>(0)[0]
    };
    assert_eq!((read(answers[0]), read(answers[1])), (1.234, 1234.0));
}

#[test]
fn a_format_with_no_group_separator_is_refused() {
    let ungrouped = SuggestOptions::new().format(NumFormat {
        group_sep: hypertabular::Symbol::NONE,
        ..NumFormat::INVARIANT
    });
    assert!(matches!(
        DelimitedReader::suggest_slice(b"a\n0.5\n", Dialect::CSV, &ungrouped),
        Err(Error::Plan { column: 0 })
    ));
}

fn suggest_in(cell: &str, format: NumFormat) -> Vec<Door> {
    let bytes = format!("a\n{cell}\n");
    let asked = SuggestOptions::new().format(format);
    doors(&DelimitedReader::suggest_slice(bytes.as_bytes(), Dialect::CSV, &asked).unwrap())
}

/// A leading zero is one in the declared digits too: `٠٠٧` is an identifier, as `007` is, and a
/// mix of native and ASCII digits is read the same way.
#[test]
fn a_leading_zero_in_native_digits_keeps_an_identifier_text() {
    let arabic = NumFormat::INVARIANT.with_digits(NativeDigits::new('\u{660}').unwrap());
    for cell in [
        "007",
        "\u{660}\u{660}\u{667}",
        "\u{660}7",
        "0\u{667}",
        "-\u{660}\u{661}.5",
    ] {
        assert_eq!(suggest_in(cell, arabic), [Door::Text], "{cell}");
    }
    // The controls: no leading zero, a lone zero, and a fraction's own leading zeros.
    for cell in ["\u{667}", "\u{660}", "\u{667}\u{660}", "0.\u{660}\u{667}"] {
        assert!(suggest_in(cell, arabic)[0].is_numeric(), "{cell}");
    }
}

/// An exponent is an `e` inside the numeral: a declared symbol holding an `E` doesn't make an
/// integer past `i64` a float, which would round it.
#[test]
fn a_symbol_s_letter_e_is_no_exponent() {
    let euro =
        NumFormat::new('.', ',', NumFormat::ALL).with_currency(CurrencySymbol::new("EUR").unwrap());
    for cell in [
        "9223372036854775809",
        "9223372036854775809 EUR",
        "EUR 9223372036854775809",
    ] {
        assert_eq!(suggest_in(cell, euro), [Door::Text], "{cell}");
    }
    // The control: a real exponent still makes a float, beside the symbol or not.
    for cell in ["9223372036854775809e0", "9.2e18 EUR", "1E+30 EUR"] {
        assert_eq!(suggest_in(cell, euro), [Door::F64], "{cell}");
    }
}
