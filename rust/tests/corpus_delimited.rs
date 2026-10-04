//! Replays `corpus/delimited.json` — the contract every binding replays — through the
//! Rust binding, from a slice and from a stream read through buffers too small for a row.
//! Also holds the committed file to the example that writes it: a case edited in one and
//! not the other fails here.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::delimited::{BatchReader, Dialect, Error};
use hypertabular::{Batch, Column, Plan, Values};
use serde_json::{Value, json};
use std::io::Cursor;

fn committed() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/delimited.json");
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("reading {path}: {error}"))
}

#[test]
fn the_committed_corpus_is_what_the_example_writes() {
    assert!(
        committed() == generator::corpus_json(),
        "corpus/delimited.json is out of date: cargo run --example corpus"
    );
}

fn cell(batch: &Batch, column: usize, row: usize) -> Value {
    let data = batch.column(column);
    if let Some(fault) = data.verdicts()[row].fault() {
        let raw = batch
            .faults()
            .iter()
            .find(|entry| (entry.row as usize, entry.column as usize) == (row, column))
            .map_or(&[][..], |entry| batch.raw(entry));
        return fault_json(fault.reason, fault.offset, fault.len, raw);
    }
    let number = |value: Value| json!({"expect": "ok", "value": value});
    match data.values() {
        Values::Bool(v) => number(json!(v[row])),
        Values::I8(v) => number(json!(v[row])),
        Values::I16(v) => number(json!(v[row])),
        Values::I32(v) => number(json!(v[row])),
        Values::I64(v) => number(json!(v[row])),
        Values::U8(v) => number(json!(v[row])),
        Values::U16(v) => number(json!(v[row])),
        Values::U32(v) => number(json!(v[row])),
        Values::U64(v) => number(json!(v[row])),
        Values::F32(v) => number(json!(f64::from(v[row]))),
        Values::F64(v) => number(json!(v[row])),
        Values::Decimal(v) => decimal_json(v[row]),
        Values::Uuid(v) => uuid_json(v[row]),
        Values::Timestamp(v) => timestamp_json(v[row]),
        Values::Date(v) => date_json(v[row]),
        Values::DateTime(v) => datetime_json(v[row]),
        Values::Time(v) => json!({"expect": "ok", "nanos": v[row]}),
        Values::Duration(v) => duration_json(v[row]),
        Values::Text(_) => text_json(batch.text(column, row).expect("a text value")),
    }
}

fn failure(error: &Error) -> Value {
    match *error {
        Error::ColumnCount {
            expected,
            found,
            record,
            line,
            byte,
        } => json!({
            "kind": "column_count", "record": record, "line": line, "byte": byte,
            "expected": expected, "found": found,
        }),
        Error::UnclosedQuote { record, line, byte } => {
            json!({"kind": "unclosed_quote", "record": record, "line": line, "byte": byte})
        }
        ref other => panic!("not a failure the corpus describes: {other:?}"),
    }
}

/// What a reader makes of a case: `(header, rows, failure)`, in the corpus's own shapes.
fn read(reader: Result<BatchReader<'_>, Error>, max_rows: usize) -> (Value, Vec<Value>, Value) {
    let mut reader = match reader {
        Ok(reader) => reader,
        // A failure in the header record itself.
        Err(error) => return (Value::Null, Vec::new(), failure(&error)),
    };
    let header = match reader.header() {
        Some(header) => {
            json!(
                header
                    .names()
                    .map(|name| String::from_utf8_lossy(name))
                    .collect::<Vec<_>>()
            )
        }
        None => Value::Null,
    };
    let mut rows = Vec::new();
    let mut batch = Batch::new();
    loop {
        match reader.fill(&mut batch, max_rows) {
            Ok(0) => return (header, rows, Value::Null),
            Ok(filled) => {
                for row in 0..filled {
                    let cells: Vec<Value> = (0..batch.columns().len())
                        .map(|column| cell(&batch, column, row))
                        .collect();
                    rows.push(json!(cells));
                }
            }
            Err(error) => return (header, rows, failure(&error)),
        }
    }
}

#[test]
fn every_case_reads_as_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert!(cases.len() >= 30);
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let input = case["input"].as_str().expect("input").as_bytes();
        let dialect = Dialect {
            separator: case["dialect"]["separator"]
                .as_str()
                .expect("separator")
                .as_bytes()[0],
            quoting: case["dialect"]["quoting"].as_bool().expect("quoting"),
            has_header: case["dialect"]["has_header"].as_bool().expect("has_header"),
            skip_blank_lines: case["dialect"]["skip_blank_lines"].as_bool().expect("skip"),
        };
        let plan = Plan::new(
            case["plan"]
                .as_array()
                .expect("plan")
                .iter()
                .map(|entry| {
                    Column::new(
                        entry["ordinal"].as_u64().expect("ordinal") as usize,
                        door_of(entry),
                    )
                    .with_format(format_of(entry))
                })
                .collect(),
        );
        let expected = (
            case["header"].clone(),
            case["rows"].as_array().expect("rows").clone(),
            case.get("failure").cloned().unwrap_or(Value::Null),
        );
        for max_rows in [1, 2, 1024] {
            let slice = BatchReader::from_slice(input, dialect, plan.clone());
            assert_eq!(
                read(slice, max_rows),
                expected,
                "{name}: a slice, {max_rows} rows a batch"
            );
            for capacity in [1, 5, 64] {
                let stream = BatchReader::from_reader_with_capacity(
                    Cursor::new(input),
                    dialect,
                    plan.clone(),
                    capacity,
                );
                assert_eq!(
                    read(stream, max_rows),
                    expected,
                    "{name}: a stream through {capacity} bytes, {max_rows} rows a batch"
                );
            }
        }
    }
}
