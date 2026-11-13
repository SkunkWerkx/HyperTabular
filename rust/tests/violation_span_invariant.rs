//! Every span a verdict carries lies inside its cell's text: for each constrained case of the
//! corpus, `offset + len <= raw.len()` for every verdict that isn't `OK`, a violation's
//! included. Delimited cases are read at every truncation of their input, so a span is proven
//! for every cell a cut can leave; workbook cases are read whole, a package having no cuts.
//! Bindings slice a cell's text with these spans.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::kernel::abi::CellVerdict;
use hypertabular::{Batch, Column, DelimitedReader, Workbook};
use serde_json::Value;

fn cases(file: &str) -> Vec<Value> {
    let path = format!("{}/../corpus/{file}", env!("CARGO_MANIFEST_DIR"));
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the corpus")).expect("JSON");
    cases
        .into_iter()
        .filter(|case| {
            case.get("plan")
                .and_then(Value::as_array)
                .is_some_and(|plan| plan.iter().any(|entry| entry.get("constraints").is_some()))
        })
        .collect()
}

/// Checks every span of `batch`, and counts the violations it holds.
fn spans_inside(batch: &Batch<'_>, name: &str) -> usize {
    let mut violations = 0;
    for column in 0..batch.columns().len() {
        for (row, verdict) in batch.verdicts(column).iter().enumerate() {
            if verdict.reason == CellVerdict::OK.reason {
                continue;
            }
            assert!(
                (1..=CellVerdict::VIOLATION).contains(&verdict.reason),
                "{name}: column {column}, row {row}: reason {}",
                verdict.reason
            );
            violations += usize::from(verdict.reason == CellVerdict::VIOLATION);
            let raw = batch.raw(column, row);
            assert!(
                verdict.offset as usize + verdict.len as usize <= raw.len(),
                "{name}: column {column}, row {row}: [{}, {}) over {} bytes",
                verdict.offset,
                verdict.offset as usize + verdict.len as usize,
                raw.len()
            );
        }
    }
    violations
}

#[test]
fn every_delimited_span_lies_inside_its_cell_at_every_cut() {
    let cases = cases("delimited.json");
    assert!(cases.len() >= 10, "{} constrained cases", cases.len());
    let mut violations = 0;
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let input = match case.get("input_hex") {
            Some(hex) => unhex(hex.as_str().expect("input_hex")),
            None => case["input"].as_str().expect("input").as_bytes().to_vec(),
        };
        let dialect = dialect_of(&case["dialect"]);
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(column_of)
            .collect();
        for cut in 0..=input.len() {
            let Ok(mut reader) =
                DelimitedReader::options().from_slice(&input[..cut], dialect, &plan)
            else {
                continue;
            };
            while let Ok(Some(batch)) = reader.read() {
                violations += spans_inside(&batch, &format!("{name}, cut at {cut}"));
            }
        }
    }
    assert!(violations > 0, "the cuts left no violation to check");
}

#[test]
fn every_workbook_span_lies_inside_its_cell() {
    let cases = cases("workbook.json");
    assert!(!cases.is_empty());
    let mut violations = 0;
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let file = case["file"].as_str().expect("file");
        let file = file.strip_prefix("workbook/").unwrap_or(file);
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(column_of)
            .collect();
        let Ok(book) = Workbook::open(format!("{}/{file}", generator::WORKBOOK_DIR)) else {
            continue;
        };
        let Ok(mut sheet) = book.sheet(
            case["sheet"].as_u64().unwrap_or(0) as usize,
            sheet_options_of(&case["options"]),
            &plan,
        ) else {
            continue;
        };
        while let Ok(Some(batch)) = sheet.read() {
            violations += spans_inside(&batch, name);
        }
    }
    assert!(violations > 0, "the cases held no violation to check");
}
