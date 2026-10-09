//! Replays `corpus/delimited.json` — the contract every binding replays — through the
//! Rust binding: from a slice, from a stream read through buffers too small for a row,
//! and from a file, in batches of one row, of two, and of more than any case has.
//! Also holds the committed file to the example that writes it: a case edited in one and
//! not the other fails here.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::{Column, DelimitedReader, Dialect, Error};
use serde_json::Value;
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

/// What a reader makes of a case: `(header, rows, failure)`, in the corpus's own shapes.
fn read(reader: Result<DelimitedReader<'_>, Error>) -> (Value, Vec<Value>, Value) {
    let mut reader = match reader {
        Ok(reader) => reader,
        // A failure in the header record itself.
        Err(error) => return (Value::Null, Vec::new(), failure_json(&error)),
    };
    let header = header_json(reader.header());
    let mut rows = Vec::new();
    loop {
        match reader.read() {
            Ok(Some(batch)) => rows_json(&batch, &mut rows),
            Ok(None) => return (header, rows, Value::Null),
            Err(error) => {
                // A reader that has failed says so again, and says the same.
                let again = reader.read().expect_err("a failed reader stays failed");
                assert_eq!(failure_json(&again), failure_json(&error));
                return (header, rows, failure_json(&error));
            }
        }
    }
}

#[test]
fn every_case_reads_as_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert!(cases.len() >= 30);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus_delimited.txt");
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
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(column_of)
            .collect();
        let expected = (
            case["header"].clone(),
            case["rows"].as_array().expect("rows").clone(),
            case.get("failure").cloned().unwrap_or(Value::Null),
        );
        std::fs::write(&path, input).expect("writing the case to a file");
        for batch_rows in [1, 2, 1024] {
            let options = DelimitedReader::options().batch_rows(batch_rows);
            assert_eq!(
                read(options.from_slice(input, dialect, &plan)),
                expected,
                "{name}: a slice, {batch_rows} rows a batch"
            );
            for buffer_bytes in [1, 5, 64] {
                let stream = options.buffer_bytes(buffer_bytes).from_reader(
                    Cursor::new(input),
                    dialect,
                    &plan,
                );
                assert_eq!(
                    read(stream),
                    expected,
                    "{name}: a stream through {buffer_bytes} bytes, {batch_rows} rows a batch"
                );
            }
            assert_eq!(
                read(options.buffer_bytes(7).open(&path, dialect, &plan)),
                expected,
                "{name}: a file, {batch_rows} rows a batch"
            );
            // Header first, the plan bound after: the same reads, whatever the source.
            fn bound<'a>(
                reader: Result<DelimitedReader<'a>, Error>,
                plan: &[Column],
            ) -> Result<DelimitedReader<'a>, Error> {
                let mut reader = reader?;
                reader.bind(plan)?;
                Ok(reader)
            }
            assert_eq!(
                read(bound(options.from_slice_unbound(input, dialect), &plan)),
                expected,
                "{name}: a slice bound after its header, {batch_rows} rows a batch"
            );
            assert_eq!(
                read(bound(
                    options
                        .buffer_bytes(5)
                        .from_reader_unbound(Cursor::new(input), dialect),
                    &plan
                )),
                expected,
                "{name}: a stream bound after its header, {batch_rows} rows a batch"
            );
            assert_eq!(
                read(bound(
                    options.buffer_bytes(7).open_unbound(&path, dialect),
                    &plan
                )),
                expected,
                "{name}: a file bound after its header, {batch_rows} rows a batch"
            );
        }
    }
}
