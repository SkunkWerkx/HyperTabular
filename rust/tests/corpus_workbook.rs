//! Replays `corpus/workbook.json` — the contract every binding replays — through the Rust
//! binding: each package opened from a slice, from bytes handed over, and from its path,
//! read in batches of one row, of two, and of more than any sheet has.
//!
//! The file is the frozen word of the std workbook reader this crate had before its core
//! could read a workbook; that reader has been deleted, and nothing independent of the
//! core reads a workbook here. So this replay is what holds the core to what
//! that reader read — and `the_committed_corpus_is_what_the_example_writes` is what says
//! so when it stops.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::kernel::abi::RawSuggestion;
use hypertabular::kernel::suggest::{COMMA_DECIMAL, COMMA_GROUPS};
use hypertabular::{
    Column, Error, ExcelEpoch, Format, Reason, SheetOptions, SuggestOptions, SuggestStatus,
    Workbook,
};
use serde_json::{Value, json};

fn committed() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook.json");
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("reading {path}: {error}"))
}

#[test]
fn the_committed_corpus_is_what_the_example_writes() {
    assert!(
        committed() == generator::workbook_json(),
        "corpus/workbook.json is not what the core reads: cargo run --example corpus, and \
         explain the diff"
    );
}

#[test]
fn every_corpus_key_is_read() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert_eq!(unknown_workbook_keys(&cases), Vec::<String>::new());
    // The check notices a key added anywhere an object is, in every case.
    for case in &cases {
        for place in key_places(case) {
            let mut edited = case.clone();
            edited
                .pointer_mut(&place)
                .and_then(Value::as_object_mut)
                .expect("an object")
                .insert("junk".to_owned(), Value::Bool(true));
            let unknown = unknown_workbook_keys(std::slice::from_ref(&edited));
            assert_eq!(unknown.len(), 1, "{place}: {unknown:?}");
            assert!(unknown[0].ends_with(".junk"), "{place}: {unknown:?}");
        }
    }
}

#[test]
fn every_case_reads_as_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert!(cases.len() >= 80);
    assert_eq!(
        unknown_workbook_keys(&cases),
        Vec::<String>::new(),
        "keys this replay would pass over"
    );
    let mut cells = 0usize;
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let path = format!(
            "{}/../corpus/{}",
            env!("CARGO_MANIFEST_DIR"),
            case["file"].as_str().expect("file")
        );
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
        // A package the core refuses: every way of opening it gives the one failure.
        if case.get("sheet").is_none() {
            let refusals = [
                Workbook::from_slice(&bytes).err(),
                Workbook::from_vec(bytes.clone()).err(),
                Workbook::open(&path).err(),
            ];
            for refusal in refusals {
                let error = refusal.unwrap_or_else(|| panic!("{name}: it opened"));
                assert_eq!(failure_json(&error), case["failure"], "{name}");
            }
            continue;
        }
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(column_of)
            .collect();
        let sheet_index = case["sheet"].as_u64().expect("sheet") as usize;
        let sheet_name = case["sheets"][sheet_index]["name"]
            .as_str()
            .expect("a name");
        let expected = (
            case["header"].clone(),
            case["numbers"].clone(),
            case["rows"].as_array().expect("rows").clone(),
            case["problems"].as_array().expect("problems").clone(),
            case.get("failure").cloned().unwrap_or(Value::Null),
        );
        cells += expected.2.len() * plan.len();

        let books = [
            ("a slice", Workbook::from_slice(&bytes)),
            ("its own bytes", Workbook::from_vec(bytes.clone())),
            ("a path", Workbook::open(&path)),
            ("a stream", Workbook::from_reader(&bytes[..])),
        ];
        for (source, book) in books {
            let book = book.unwrap_or_else(|error| panic!("{name}: {error}"));
            let format = match book.format() {
                Format::Xlsx => "xlsx",
                Format::Ods => "ods",
                Format::Numbers => "numbers",
                other => panic!("{name}: format {other:?} has no corpus name"),
            };
            assert_eq!(case["format"], format, "{name}");
            let epoch = match book.date_system() {
                ExcelEpoch::Y1900 => 1,
                ExcelEpoch::Y1904 => 2,
            };
            assert_eq!(case["epoch"], epoch, "{name}");
            let sheets: Vec<Value> = book
                .sheets()
                .iter()
                .map(|sheet| json!({"name": sheet.name, "hidden": sheet.hidden}))
                .collect();
            assert_eq!(case["sheets"], json!(sheets), "{name}");

            for batch_rows in [1, 2, 1024] {
                let options = sheet_options_of(&case["options"]).with_batch_rows(batch_rows);
                // By index — and, where the name finds the same sheet, by name.
                let by_name = book
                    .sheets()
                    .iter()
                    .position(|sheet| sheet.name == sheet_name)
                    == Some(sheet_index);
                // Each of those with the plan given, and with it bound after the header.
                for (named, unbound) in [(false, false), (true, false), (false, true), (true, true)]
                {
                    if named && !by_name {
                        continue;
                    }
                    let mut sheet = match (named, unbound) {
                        (true, false) => book.sheet(sheet_name, options, &plan),
                        (false, false) => book.sheet(sheet_index, options, &plan),
                        (true, true) => book.sheet_unbound(sheet_name, options),
                        (false, true) => book.sheet_unbound(sheet_index, options),
                    }
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                    if unbound {
                        assert!(!sheet.is_bound());
                        sheet
                            .bind(&plan)
                            .unwrap_or_else(|error| panic!("{name}: {error}"));
                    }
                    let header = header_json(sheet.header());
                    let (mut numbers, mut rows, mut problems) =
                        (Vec::new(), Vec::new(), Vec::new());
                    let failure = loop {
                        match sheet.read() {
                            Ok(Some(batch)) => {
                                assert!(batch.rows() >= 1 && batch.rows() <= batch_rows);
                                numbers.extend((0..batch.rows()).map(|row| batch.line(row)));
                                problems_json(&batch, rows.len(), &mut problems);
                                rows_json(&batch, &mut rows);
                            }
                            Ok(None) => break Value::Null,
                            Err(error) => {
                                let again = sheet.read().expect_err("a failed sheet stays failed");
                                assert_eq!(failure_json(&again), failure_json(&error));
                                break failure_json(&error);
                            }
                        }
                    };
                    assert_eq!(
                        (header, json!(numbers), rows, problems, failure),
                        expected,
                        "{name}: {source}, {batch_rows} rows a batch, named {named}, unbound {unbound}"
                    );
                }
            }
        }
    }
    assert!(cells >= 12_000, "{cells} cells replayed");
}

/// What the `dates.*` cases read, worked out by hand from the cells in `packages.rs`, not by the
/// core: the writer's word on an order is the core's, so this is what holds it to the text. Each
/// inner list is one row, a cell being `(year, month, day)`, or the reason it's refused, in the plan's
/// columns; the stamps' time is 10:30:00. Row 5 is the stored date, which every order reads.
#[test]
fn the_field_order_decides_the_dates_as_worked_out_by_hand() {
    #[derive(Clone, Copy)]
    enum Cell {
        Date(u64, u64, u64),
        Refused(&'static str),
    }
    use Cell::{Date, Refused};
    const MALFORMED: Cell = Refused("malformed");
    const RANGE: Cell = Refused("out_of_range");
    const STORED: Cell = Date(2025, 12, 9);
    let tables: [(&str, &[[Option<Cell>; 3]; 4]); 4] = [
        (
            "year first",
            &[
                [Some(MALFORMED), Some(MALFORMED), None],
                [Some(MALFORMED), Some(MALFORMED), None],
                [Some(MALFORMED), Some(MALFORMED), None],
                [Some(STORED), Some(STORED), None],
            ],
        ),
        (
            "month first",
            &[
                [Some(Date(2026, 1, 2)), Some(Date(2026, 1, 2)), None],
                [Some(RANGE), Some(RANGE), None],
                [Some(Date(2026, 2, 13)), Some(Date(2026, 2, 13)), None],
                [Some(STORED), Some(STORED), None],
            ],
        ),
        (
            "day first",
            &[
                [Some(Date(2026, 2, 1)), Some(Date(2026, 2, 1)), None],
                [Some(Date(2026, 2, 13)), Some(Date(2026, 2, 13)), None],
                [Some(RANGE), Some(RANGE), None],
                [Some(STORED), Some(STORED), None],
            ],
        ),
        (
            "month names, day first and month first, and none declared",
            &[
                [Some(Date(2026, 10, 7)), Some(MALFORMED), Some(MALFORMED)],
                [Some(MALFORMED), Some(Date(2026, 10, 7)), Some(MALFORMED)],
                [Some(MALFORMED), Some(MALFORMED), Some(MALFORMED)],
                [Some(STORED), Some(STORED), Some(STORED)],
            ],
        ),
    ];
    let corpus: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus");
    for file in ["dates.xlsx", "dates.ods", "dates.numbers"] {
        for (what, table) in &tables {
            let name = format!("{file}, sheet 0, {what}");
            let case = corpus
                .iter()
                .find(|case| case["name"] == name.as_str())
                .unwrap_or_else(|| panic!("{name}: not in the corpus"));
            assert_eq!(case["numbers"], json!([2, 3, 4, 5]), "{name}");
            let plan = case["plan"].as_array().expect("plan");
            let rows = case["rows"].as_array().expect("rows");
            assert_eq!(rows.len(), table.len(), "{name}");
            for (index, (row, want)) in rows.iter().zip(table.iter()).enumerate() {
                for (column, cell) in want.iter().flatten().enumerate() {
                    let got = &row[column];
                    let at = format!("{name}, row {index}, column {column}: {got}");
                    match *cell {
                        Date(year, month, day) => {
                            assert_eq!(got["expect"], "ok", "{at}");
                            assert_eq!(got["year"], year, "{at}");
                            assert_eq!(got["month"], month, "{at}");
                            assert_eq!(got["day"], day, "{at}");
                            if plan[column]["door"] == "datetime" {
                                let stamped = year != 2025;
                                let nanos = if stamped { 37_800_000_000_000_u64 } else { 0 };
                                assert_eq!(got["nanos_of_day"], nanos, "{at}");
                            }
                        }
                        Refused(reason) => assert_eq!(got["expect"], reason, "{at}"),
                    }
                }
            }
        }
    }
}

/// What the `suggest.*` cases read, worked out by hand from the table in `packages.rs`: a typed
/// number is what it is under any format, and only a text cell meets the one declared.
#[test]
fn a_typed_number_reads_the_same_under_any_format_as_worked_out_by_hand() {
    let native = json!([1.234, 2.345, 12.345]);
    let whole = json!([1, 2, 3_000_000]);
    let big = json!([3_000_000_000_u64, 2, 5]);
    let flags = json!([true, false, true]);
    let text = |cells: [&str; 3]| json!(cells);
    let tables = [
        (
            "nothing declared",
            [
                native.clone(),
                text(["1.234", "2.345", "12.345"]),
                text(["1.5", "1.234", "2"]),
                whole.clone(),
                json!([1.25, 2.5, 2.0]),
                big.clone(),
                flags.clone(),
            ],
        ),
        (
            "a comma decimal declared",
            [
                native.clone(),
                json!([1234, 2345, 12345]),
                json!([1.5, 1234.0, 2.0]),
                whole.clone(),
                json!([1.25, 2.5, 2.0]),
                big.clone(),
                flags.clone(),
            ],
        ),
        (
            "Western declared",
            [
                native.clone(),
                native,
                json!([1.5, 1.234, 2.0]),
                whole,
                text(["1.25", "2,5", "2"]),
                big,
                flags,
            ],
        ),
    ];
    let corpus: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus");
    for file in ["suggest.xlsx", "suggest.ods", "suggest.numbers"] {
        for (what, table) in &tables {
            let name = format!("{file}, sheet 0, {what}");
            let case = corpus
                .iter()
                .find(|case| case["name"] == name.as_str())
                .unwrap_or_else(|| panic!("{name}: not in the corpus"));
            let rows = case["rows"].as_array().expect("rows");
            assert_eq!(rows.len(), 3, "{name}");
            for (column, want) in table.iter().enumerate() {
                for (row, cells) in rows.iter().enumerate() {
                    let cell = &cells[column];
                    assert_eq!(cell["expect"], "ok", "{name}, {row}, {column}: {cell}");
                    let got = cell.get("value").or_else(|| cell.get("text"));
                    assert_eq!(got, Some(&want[row]), "{name}, {row}, {column}: {cell}");
                }
            }
        }
    }
}

/// What the `grouping.*` cases read, worked out by hand from the table in `packages.rs`: a column's
/// policy decides its text cells, and a typed number is the same under either.
#[test]
fn a_column_s_grouping_is_its_own_as_worked_out_by_hand() {
    const NO: Option<i64> = None;
    type Column = [Option<i64>; 3];
    let tables: [(&str, &[Column]); 3] = [
        (
            "an Indian column beside a Western one",
            &[
                [Some(123456), Some(1234567), Some(1234)],
                [Some(1234567), Some(1234), Some(1234)],
                [Some(123456), NO, Some(1234)],
                [NO, Some(1234567), Some(1234)],
            ],
        ),
        (
            "no format is Western grouping",
            &[
                [NO, NO, Some(1234)],
                [Some(1234567), Some(1234), Some(1234)],
                [NO, Some(1234567), Some(1234)],
            ],
        ),
        (
            "the policies crossed",
            &[[NO, NO, Some(1234)], [NO, Some(1234), Some(1234)]],
        ),
    ];
    let corpus: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus");
    for file in ["grouping.xlsx", "grouping.ods", "grouping.numbers"] {
        for (what, table) in &tables {
            let name = format!("{file}, sheet 0, {what}");
            let case = corpus
                .iter()
                .find(|case| case["name"] == name.as_str())
                .unwrap_or_else(|| panic!("{name}: not in the corpus"));
            let rows = case["rows"].as_array().expect("rows");
            assert_eq!(rows.len(), 3, "{name}");
            for (column, want) in table.iter().enumerate() {
                for (row, cells) in rows.iter().enumerate() {
                    let cell = &cells[column];
                    match want[row] {
                        Some(value) => {
                            assert_eq!(cell["expect"], "ok", "{name}, {row}, {column}: {cell}");
                            assert_eq!(cell["value"], json!(value), "{name}, {row}, {column}");
                        }
                        None => {
                            assert_eq!(cell["expect"], "malformed", "{name}, {row}, {column}");
                            assert_eq!(cell["detail"], "group_size", "{name}, {row}, {column}");
                        }
                    }
                }
            }
        }
    }
}

/// Every case that says what to suggest: through the binding, and a plan that then reads back
/// refusing nothing it resolved, with each column planned in the notation its report names.
#[test]
fn every_suggestion_is_what_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    let mut seen = 0;
    for case in &cases {
        let Some(expected) = case.get("suggest") else {
            continue;
        };
        seen += 1;
        let name = case["name"].as_str().expect("name");
        let path = format!(
            "{}/../corpus/{}",
            env!("CARGO_MANIFEST_DIR"),
            case["file"].as_str().expect("file")
        );
        let book = Workbook::open(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut asked = SuggestOptions::new();
        let declared = expected.get("format").map(|_| format_of(expected));
        if let Some(format) = declared {
            asked = asked.format(format);
        }
        let index = case["sheet"].as_u64().expect("sheet") as usize;
        let options = sheet_options_of(&case["options"]);
        let sheet = book.sheet_unbound(index, options).expect(name);
        let found = sheet.suggest(&asked).expect(name);
        let columns = expected["columns"].as_array().expect("columns");
        let reported: Vec<Value> = columns
            .iter()
            .map(|column| {
                let mut column = column.clone();
                if column["status"] == "resolved" {
                    column["param"] = json!(0);
                }
                column
            })
            .collect();
        assert_eq!(suggestion_json(&found), Value::Array(reported), "{name}");
        assert_eq!(found.truncated, expected["truncated"], "{name}");
        for ((column, planned), want) in found.report.iter().zip(&found.plan).zip(columns) {
            if column.status == SuggestStatus::Resolved && column.door.is_numeric() {
                let notation = if want["param"] == u64::from(RawSuggestion::NOTATION_COMMA_DECIMAL)
                {
                    COMMA_DECIMAL
                } else {
                    COMMA_GROUPS
                };
                let format = declared.unwrap_or(notation);
                assert_eq!(column.format, Some(format), "{name}");
                assert_eq!(planned.format, format, "{name}");
            }
        }
        let mut read = book.sheet(index, options, &found.plan).expect(name);
        while let Some(batch) = read.read().expect(name) {
            let refused: Vec<_> = batch
                .faults()
                .filter(|problem| {
                    matches!(problem.reason, Some(Reason::Malformed | Reason::OutOfRange))
                })
                .collect();
            assert!(refused.is_empty(), "{name}: {refused:?}");
        }
    }
    assert_eq!(seen, 15, "{seen} cases say what to suggest");
}

#[test]
fn what_is_not_there_is_an_error_and_not_a_panic() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook/basic.xlsx");
    let book = Workbook::open(path).expect("a workbook");
    let plan = [Column::text(0)];
    let options = SheetOptions::default();
    assert!(matches!(
        book.sheet("No such sheet", options, &plan),
        Err(Error::NoSheet(_))
    ));
    assert!(matches!(
        book.sheet(99, options, &plan),
        Err(Error::NoSheet(_))
    ));
    assert!(matches!(
        Workbook::open("/no/such/file.xlsx"),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        Workbook::from_slice(b"not a zip at all"),
        Err(Error::Structure(failure)) if failure.kind == hypertabular::FailureKind::NotAZip
    ));
    // Two sheets read at once, each with its own buffers, over one workbook.
    let mut first = book.sheet(0, options, &plan).expect("a sheet");
    let mut second = book
        .sheet(0, options.with_header(false), &plan)
        .expect("a sheet");
    let a = first.read().expect("a read").expect("a batch").rows();
    let b = second.read().expect("a read").expect("a batch").rows();
    assert_eq!(a + 1, b, "the header is one row more");
}
