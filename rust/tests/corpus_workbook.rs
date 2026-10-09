//! Replays `corpus/workbook.json` — the contract every binding replays — through the Rust
//! binding: each package opened from a slice, from bytes handed over, and from its path,
//! read in batches of one row, of two, and of more than any sheet has.
//!
//! The file is the frozen word of the std workbook reader this crate had before its core
//! could read a workbook; that reader has been deleted, and nothing independent of the
//! core reads a workbook here any more. So this replay is what holds the core to what
//! that reader read — and `the_committed_corpus_is_what_the_example_writes` is what says
//! so when it stops.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::{Column, Error, ExcelEpoch, Format, SheetOptions, Workbook};
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
fn every_case_reads_as_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert!(cases.len() >= 80);
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
                let options = SheetOptions {
                    has_header: case["options"]["has_header"].as_bool().expect("has_header"),
                    skip_empty_rows: case["options"]["skip_empty_rows"].as_bool().expect("skip"),
                    batch_rows,
                };
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
                    let (mut numbers, mut rows) = (Vec::new(), Vec::new());
                    let failure = loop {
                        match sheet.read() {
                            Ok(Some(batch)) => {
                                assert!(batch.rows() >= 1 && batch.rows() <= batch_rows);
                                numbers.extend((0..batch.rows()).map(|row| batch.line(row)));
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
                        (header, json!(numbers), rows, failure),
                        expected,
                        "{name}: {source}, {batch_rows} rows a batch, named {named}, unbound {unbound}"
                    );
                }
            }
        }
    }
    assert!(cells >= 12_000, "{cells} cells replayed");
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
