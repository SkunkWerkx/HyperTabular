//! Replays `corpus/delimited.json` — the contract every binding replays — through the
//! Rust binding: from a slice, from a stream read through buffers too small for a row,
//! and from a file, in batches of one row, of two, and of more than any case has.
//! Also holds the committed file to the example that writes it: a case edited in one and
//! not the other fails here.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypertabular::kernel::abi::{
    Buffers, BuffersWith, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith,
    ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Filled, OK, RawDelimitedOptions, RawProblem,
    RawSuggestion, Span,
};
use hypertabular::kernel::delimited::fill::{RawDialect, State};
use hypertabular::kernel::exports::{
    hypertabular_delimited_fill, hypertabular_delimited_fill_with, hypertabular_delimited_header,
    hypertabular_delimited_init, hypertabular_faults, hypertabular_suggest,
};
use hypertabular::{
    Column, DelimitedReader, Dialect, Door, Encoding, Error, LongRows, ShortRows, SuggestOptions,
    SuggestStatus,
};
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

/// What a reader makes of a case: `(header, rows, problems, failure)`, in the corpus's own shapes.
fn read(reader: Result<DelimitedReader<'_>, Error>) -> (Value, Vec<Value>, Vec<Value>, Value) {
    let mut reader = match reader {
        Ok(reader) => reader,
        // A failure in the header record itself.
        Err(error) => return (Value::Null, Vec::new(), Vec::new(), failure_json(&error)),
    };
    let header = header_json(reader.header());
    let (mut rows, mut problems) = (Vec::new(), Vec::new());
    loop {
        match reader.read() {
            Ok(Some(batch)) => {
                problems_json(&batch, rows.len(), &mut problems);
                rows_json(&batch, &mut rows);
            }
            Ok(None) => return (header, rows, problems, Value::Null),
            Err(error) => {
                // A reader that has failed says so again, and says the same.
                let again = reader.read().expect_err("a failed reader stays failed");
                assert_eq!(failure_json(&again), failure_json(&error));
                return (header, rows, problems, failure_json(&error));
            }
        }
    }
}

/// A case's dialect, the options it may declare beside the 1.0 four.
fn dialect_of(entry: &Value) -> Dialect {
    let mut dialect = Dialect::new(entry["separator"].as_str().expect("separator").as_bytes()[0])
        .with_quoting(entry["quoting"].as_bool().expect("quoting"))
        .with_header(entry["has_header"].as_bool().expect("has_header"))
        .with_blank_lines_skipped(entry["skip_blank_lines"].as_bool().expect("skip"));
    if let Some(skip) = entry.get("skip") {
        dialect = dialect.with_skip(skip.as_u64().expect("skip") as u32);
    }
    if let Some(row) = entry.get("header_row") {
        dialect = dialect.with_header_row(row.as_u64().expect("header_row") as u32);
    }
    if let Some(policy) = entry.get("short_rows") {
        dialect = dialect.with_short_rows(match policy.as_str().expect("short_rows") {
            "reject" => ShortRows::Reject,
            "pad" => ShortRows::Pad,
            other => panic!("short_rows {other}"),
        });
    }
    if let Some(policy) = entry.get("long_rows") {
        dialect = dialect.with_long_rows(match policy.as_str().expect("long_rows") {
            "reject" => LongRows::Reject,
            "report" => LongRows::Report,
            "truncate" => LongRows::Truncate,
            other => panic!("long_rows {other}"),
        });
    }
    if let Some(tokens) = entry.get("tokens") {
        dialect = dialect.with_tokens(&strings_of(tokens));
    }
    if let Some(encoding) = entry.get("encoding") {
        dialect = dialect.with_encoding(match encoding.as_str().expect("encoding") {
            "windows-1252" => Encoding::Windows1252,
            "latin-1" => Encoding::Latin1,
            "utf-16le" => Encoding::Utf16Le,
            other => panic!("encoding {other}"),
        });
    }
    dialect
}

#[test]
fn every_corpus_key_is_read() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert_eq!(unknown_delimited_keys(&cases), Vec::<String>::new());
    // The check notices a key added anywhere an object is, in every case.
    for case in &cases {
        for place in key_places(case) {
            let mut edited = case.clone();
            edited
                .pointer_mut(&place)
                .and_then(Value::as_object_mut)
                .expect("an object")
                .insert("junk".to_owned(), Value::Bool(true));
            let unknown = unknown_delimited_keys(std::slice::from_ref(&edited));
            assert_eq!(unknown.len(), 1, "{place}: {unknown:?}");
            assert!(unknown[0].ends_with(".junk"), "{place}: {unknown:?}");
        }
    }
}

#[test]
fn every_case_reads_as_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    assert!(cases.len() >= 30);
    assert_eq!(
        unknown_delimited_keys(&cases),
        Vec::<String>::new(),
        "keys this replay would pass over"
    );
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus_delimited.txt");
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let input = match case.get("input_hex") {
            Some(hex) => unhex(hex.as_str().expect("input_hex")),
            None => case["input"].as_str().expect("input").as_bytes().to_vec(),
        };
        let input = input.as_slice();
        let dialect = dialect_of(&case["dialect"]);
        let plan: Vec<Column> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(column_of)
            .collect();
        let expected = (
            case["header"].clone(),
            case["rows"].as_array().expect("rows").clone(),
            case["problems"].as_array().expect("problems").clone(),
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

/// The corpus's name for a door the suggester can name.
fn door_code(name: &str) -> u32 {
    door_of(&serde_json::json!({"door": name, "order": 1})).code()
}

/// Every case the corpus has, whether it says what to suggest or not: a suggestion fails where a
/// read of the case fails, and nowhere else.
#[test]
fn suggest_fails_only_where_a_read_of_the_case_fails() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    let (mut planned, mut failed) = (0, 0);
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let input = match case.get("input_hex") {
            Some(hex) => unhex(hex.as_str().expect("input_hex")),
            None => case["input"].as_str().expect("input").as_bytes().to_vec(),
        };
        let dialect = dialect_of(&case["dialect"]);
        match DelimitedReader::suggest_slice(&input, dialect, &SuggestOptions::new()) {
            Ok(_) => planned += 1,
            Err(error) => {
                failed += 1;
                assert!(case.get("failure").is_some(), "{name}: {error:?}");
            }
        }
    }
    assert!(planned > 80 && failed > 0, "{planned}, {failed}");
}

#[test]
fn every_suggestion_is_what_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus_suggest.txt");
    let mut seen = 0;
    for case in &cases {
        let Some(expected) = case.get("suggest") else {
            continue;
        };
        seen += 1;
        let name = case["name"].as_str().expect("name");
        let input = match case.get("input_hex") {
            Some(hex) => unhex(hex.as_str().expect("input_hex")),
            None => case["input"].as_str().expect("input").as_bytes().to_vec(),
        };
        let dialect = dialect_of(&case["dialect"]);
        let mut options = SuggestOptions::new();
        if let Some(rows) = expected.get("sample_rows") {
            options = options.sample_rows(rows.as_u64().expect("sample_rows") as usize);
        }
        if expected.get("format").is_some() {
            options = options.format(format_of(expected));
        }
        let columns = &expected["columns"];
        let truncated = expected["truncated"].as_bool().expect("truncated");

        // A report says what a resolved number's `param` said in the format it names, the one the
        // plan uses, and the export below is held to the `param`.
        let reported: Vec<Value> = columns
            .as_array()
            .expect("columns")
            .iter()
            .map(|column| {
                let mut column = column.clone();
                let door = column["door"].as_str().expect("door");
                if column["status"] == "resolved" && !matches!(door, "date_ordered" | "datetime") {
                    column["param"] = serde_json::json!(0);
                }
                column
            })
            .collect();
        // Bytes, and a file.
        let found = DelimitedReader::suggest_slice(&input, dialect, &options).expect(name);
        assert_eq!(
            suggestion_json(&found),
            Value::Array(reported),
            "{name}: bytes"
        );
        let declared = expected.get("format").map(|_| format_of(expected));
        let wanted = columns.as_array().expect("columns");
        for ((column, planned), want) in found.report.iter().zip(&found.plan).zip(wanted) {
            if column.status == SuggestStatus::Resolved && column.door.is_numeric() {
                let notation = if want["param"] == u64::from(RawSuggestion::NOTATION_COMMA_DECIMAL)
                {
                    hypertabular::NumFormat::new(',', '.', hypertabular::NumFormat::ALL)
                } else {
                    hypertabular::NumFormat::INVARIANT
                };
                let format = declared.unwrap_or(notation);
                assert_eq!(column.format, Some(format), "{name}");
                assert_eq!(planned.format, format, "{name}");
            } else {
                assert_eq!(column.format, None, "{name}");
            }
        }
        assert_eq!(found.truncated, truncated, "{name}: bytes");
        std::fs::write(&path, &input).expect("writing the case to a file");
        let from_file = DelimitedReader::suggest(&path, dialect, &options).expect(name);
        assert_eq!(from_file, found, "{name}: a file");

        // The export, over the text cells the case's own rows say, as a binding would pass them.
        // A cell with no text in the case is blank here, as a read's empty verdict is.
        let rows = case["rows"].as_array().expect("rows");
        let sample = expected
            .get("sample_rows")
            .map_or(rows.len(), |rows| rows.as_u64().expect("rows") as usize);
        let width = case["plan"].as_array().expect("plan").len();
        let mut text = Vec::new();
        let mut cells = Vec::new();
        for column in 0..width {
            for row in &rows[..sample] {
                let cell = &row[column];
                let bytes = if let Some(hex) = cell.get("raw_hex") {
                    unhex(hex.as_str().expect("raw_hex"))
                } else {
                    cell.get("text")
                        .map_or(&b""[..], |text| text.as_str().expect("text").as_bytes())
                        .to_vec()
                };
                cells.push(Span {
                    offset: text.len() as u32,
                    len: bytes.len() as u32,
                });
                text.extend_from_slice(&bytes);
            }
        }
        let format = hypercast::RawNumFormat::from(format_of(expected));
        let formats = vec![format; width];
        let declared = expected.get("format").is_some();
        let mut out = vec![RawSuggestion::default(); width];
        // SAFETY: each pointer is the vector's, of the length beside it.
        let code = unsafe {
            hypertabular_suggest(
                text.as_ptr(),
                text.len(),
                std::ptr::null(),
                0,
                cells.as_ptr(),
                core::ptr::null(),
                sample,
                width,
                if declared {
                    formats.as_ptr()
                } else {
                    std::ptr::null()
                },
                out.as_mut_ptr(),
            )
        };
        assert_eq!(code, OK, "{name}: the export");
        for (place, (got, want)) in out
            .iter()
            .zip(columns.as_array().expect("columns"))
            .enumerate()
        {
            let status = match got.status {
                RawSuggestion::RESOLVED => "resolved",
                RawSuggestion::AMBIGUOUS => "ambiguous",
                RawSuggestion::UNRESOLVED => "unresolved",
                other => panic!("status {other}"),
            };
            assert_eq!(
                (
                    got.door,
                    u64::from(got.param),
                    status,
                    u64::from(got.matched),
                    u64::from(got.sampled)
                ),
                (
                    door_code(want["door"].as_str().expect("door")),
                    want["param"].as_u64().expect("param"),
                    want["status"].as_str().expect("status"),
                    want["matched"].as_u64().expect("matched"),
                    want["sampled"].as_u64().expect("sampled"),
                ),
                "{name}: the export, column {place}"
            );
        }

        // A plan that reads back: what's resolved takes every cell it saw, and refuses none.
        let mut reader = DelimitedReader::from_slice(&input, dialect, &found.plan).expect(name);
        let mut read = 0;
        while read < sample {
            let Some(batch) = reader.read().expect(name) else {
                break;
            };
            let refused = batch.faults().filter(|problem| {
                let place = problem.plan_column.expect("a cell");
                found.report[place].status == SuggestStatus::Resolved
                    && found.report[place].door != Door::Text
                    && problem.row + read < sample
            });
            assert_eq!(refused.count(), 0, "{name}: read back");
            read += batch.rows();
        }
    }
    assert!(seen >= 12, "{seen} cases say what to suggest");
}

/// A problem's reason in a cell's terms.
fn json_of_reason(problem: &Value) -> Value {
    serde_json::json!({"expect": problem["reason"]})
}

/// One cell of a case in the 1.0 exports' terms: its fault's reason and span, or `None`.
fn expected_verdict(cell: &Value) -> CellVerdict {
    let fault = cell.get("fault").and_then(Value::as_array);
    let at = |index: usize| fault.map_or(0, |fault| fault[index].as_u64().expect("span") as u32);
    CellVerdict {
        offset: at(0),
        len: at(1),
        reason: match cell["expect"].as_str().expect("expect") {
            "ok" => 0,
            "empty" => 1,
            "malformed" => 2,
            "out_of_range" => 3,
            other => panic!("expect {other}"),
        },
    }
}

/// One value as a door's type says it, so padding bytes aren't part of what's compared.
///
/// # Safety
/// `values` holds `row + 1` values of the door's type.
unsafe fn said(door: u32, values: *const u8, row: usize) -> String {
    /// # Safety
    /// As [`said`], with `T` the door's type.
    unsafe fn at<T: Copy + std::fmt::Debug>(values: *const u8, row: usize) -> String {
        // SAFETY: per the function contract.
        format!("{:?}", unsafe {
            values.cast::<T>().add(row).read_unaligned()
        })
    }
    // SAFETY: per the function contract: each door's type, as `ColumnBuffer` lists them.
    unsafe {
        match door {
            1 | 2 | 6 => at::<u8>(values, row),
            3 | 7 => at::<u16>(values, row),
            4 | 8 | 10 => at::<u32>(values, row),
            5 | 9 | 11 | 16 => at::<u64>(values, row),
            12 => at::<[u8; 16]>(values, row),
            13 | 14 | 22 => at::<hypercast::Timestamp>(values, row),
            15 | 20 => at::<hypercast::Date>(values, row),
            17 => at::<hypercast::Duration>(values, row),
            18 => at::<Span>(values, row),
            19 => at::<hypercast::Decimal>(values, row),
            21 => at::<hypercast::CivilDateTime>(values, row),
            other => panic!("door {other}"),
        }
    }
}

/// What [`said`] says of a corpus's number or boolean through `door`, `None` for any other door.
fn value_said(door: u32, value: &Value) -> Option<String> {
    let whole = || value.as_i64().expect("an integer");
    let float = || value.as_f64().expect("a number");
    Some(match door {
        1 => u8::from(value.as_bool().expect("a boolean")).to_string(),
        2 => (whole() as i8 as u8).to_string(),
        3 => (whole() as i16 as u16).to_string(),
        4 => (whole() as i32 as u32).to_string(),
        5 => (whole() as u64).to_string(),
        6..=8 => whole().to_string(),
        9 => value.as_u64().expect("an integer").to_string(),
        10 => (float() as f32).to_bits().to_string(),
        11 => float().to_bits().to_string(),
        _ => return None,
    })
}

/// `value`s and `verdict`s, column by column and row by row, as one export wrote them; with the
/// header's names, and how the input ended.
#[derive(Debug, PartialEq, Eq)]
struct Raw {
    names: u64,
    verdicts: Vec<Vec<CellVerdict>>,
    values: Vec<Vec<String>>,
    /// `_faults` over each batch, a few entries a call, each row counted across the read.
    problems: Vec<RawProblem>,
    failure: Option<(u32, u64, u32, u64)>,
}

/// `_faults` over one batch's verdicts, with no notes or widths: what either fill leaves a caller.
///
/// # Safety
/// Each of `verdicts` holds `rows` verdicts.
unsafe fn listed(
    verdicts: &mut [Vec<CellVerdict>],
    rows: usize,
    base: u32,
    all: &mut Vec<RawProblem>,
) {
    let columns: Vec<ColumnBufferWith> = verdicts
        .iter_mut()
        .map(|verdicts| ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: std::ptr::null_mut(),
            verdicts: verdicts.as_mut_ptr(),
            notes: std::ptr::null_mut(),
        })
        .collect();
    let mut cursor = 0;
    let mut out = [RawProblem::default(); 2];
    loop {
        let mut written = 0;
        // SAFETY: per the function contract, and two entries of room.
        let code = unsafe {
            hypertabular_faults(
                columns.as_ptr(),
                columns.len(),
                rows,
                std::ptr::null(),
                0,
                RawDelimitedOptions::LONG_ROWS_REJECT,
                &mut cursor,
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        assert_eq!(code, OK);
        all.extend(out[..written].iter().map(|problem| RawProblem {
            row: problem.row + base,
            ..*problem
        }));
        if written < out.len() {
            return;
        }
    }
}

/// A case through the exports alone: `_init` and `_header`, then `_fill` or `_fill_with`.
fn through_the_exports(
    input: &[u8],
    dialect: RawDialect,
    header: bool,
    specs: &[ColumnSpec],
    with: bool,
) -> Raw {
    // The 64 bytes a 1.0 caller allocates, for the 1.0 exports alone.
    let mut block = [0u64; 8];
    let mut long = vec![0u64; size_of::<State>().div_ceil(8)];
    let state: *mut State = if with {
        long.as_mut_ptr().cast()
    } else {
        block.as_mut_ptr().cast()
    };
    let mut names = vec![Span::default(); 64];
    let mut arena = vec![0u8; input.len() + 1];
    let mut raw = Raw {
        names: 0,
        verdicts: vec![Vec::new(); specs.len()],
        values: vec![Vec::new(); specs.len()],
        problems: Vec::new(),
        failure: None,
    };
    let mut at = 0usize;
    let mut out = Filled::default();
    // SAFETY: every buffer below is the size given beside it, and a row's value fits 32 bytes.
    unsafe {
        assert_eq!(hypertabular_delimited_init(state, &dialect), OK);
        if header {
            let code = hypertabular_delimited_header(
                state,
                input.as_ptr(),
                input.len(),
                1,
                names.as_mut_ptr(),
                names.len(),
                arena.as_mut_ptr(),
                arena.len(),
                &mut out,
            );
            if code == ERR_STRUCTURE {
                let failure = out.failure;
                raw.failure = Some((failure.code, failure.record, failure.line, failure.byte));
                return raw;
            }
            assert_eq!(code, OK);
            raw.names = out.rows;
            at = out.consumed as usize;
        }
        let per_row = specs.iter().map(|s| s.ordinal as usize).max().unwrap_or(0) + 2;
        const ROWS: usize = 64;
        let mut cells = vec![Span::default(); per_row * ROWS];
        let with_specs: Vec<ColumnSpecWith> = specs
            .iter()
            .map(|spec| {
                let mut wider = ColumnSpecWith::new(spec.ordinal, spec.door, spec.param);
                wider.format = spec.format;
                wider
            })
            .collect();
        loop {
            let rest = input.get(at..).unwrap_or_default();
            let mut values = vec![vec![0u8; 32 * ROWS]; specs.len()];
            let mut verdicts = vec![vec![CellVerdict::default(); ROWS]; specs.len()];
            let mut buffers = BuffersWith::empty();
            buffers.arena = arena.as_mut_ptr();
            buffers.arena_cap = arena.len();
            buffers.cells = cells.as_mut_ptr();
            buffers.cells_cap = cells.len();
            let code = if with {
                let columns: Vec<ColumnBufferWith> = values
                    .iter_mut()
                    .zip(verdicts.iter_mut())
                    .map(|(values, verdicts)| ColumnBufferWith {
                        size: ColumnBufferWith::SIZE,
                        values: values.as_mut_ptr().cast(),
                        verdicts: verdicts.as_mut_ptr(),
                        notes: std::ptr::null_mut(),
                    })
                    .collect();
                hypertabular_delimited_fill_with(
                    state,
                    rest.as_ptr(),
                    rest.len(),
                    1,
                    with_specs.as_ptr(),
                    columns.as_ptr(),
                    with_specs.len(),
                    ROWS,
                    &buffers,
                    &mut out,
                )
            } else {
                let columns: Vec<ColumnBuffer> = values
                    .iter_mut()
                    .zip(verdicts.iter_mut())
                    .map(|(values, verdicts)| ColumnBuffer {
                        values: values.as_mut_ptr().cast(),
                        verdicts: verdicts.as_mut_ptr(),
                    })
                    .collect();
                let plain: Buffers = buffers.buffers();
                hypertabular_delimited_fill(
                    state,
                    rest.as_ptr(),
                    rest.len(),
                    1,
                    specs.as_ptr(),
                    columns.as_ptr(),
                    specs.len(),
                    ROWS,
                    &plain,
                    &mut out,
                )
            };
            assert!(code != ERR_ARENA && code != ERR_CELLS);
            if code == ERR_STRUCTURE {
                let failure = out.failure;
                raw.failure = Some((failure.code, failure.record, failure.line, failure.byte));
                return raw;
            }
            assert_eq!(code, OK);
            let rows = out.rows as usize;
            let base = raw.verdicts.first().map_or(0, |column| column.len()) as u32;
            listed(&mut verdicts, rows, base, &mut raw.problems);
            for column in 0..specs.len() {
                raw.verdicts[column].extend_from_slice(&verdicts[column][..rows]);
                for row in 0..rows {
                    raw.values[column].push(said(specs[column].door, values[column].as_ptr(), row));
                }
            }
            at += out.consumed as usize;
            if rows == 0 {
                return raw;
            }
        }
    }
}

/// The 1.0 surface over the corpus, with a 1.0 caller's 64-byte state: the verdicts the corpus
/// says, the numbers and booleans it says, and `fill_with`, asked nothing new of it, writes the same
/// values and verdicts. These are the layouts 1.0 promised. A case that declares flags past 1.0's
/// isn't here, so the mask over them is `kernel_delimited`'s to hold, and what's left of a value,
/// text and dates, is `every_case_reads_as_the_corpus_says`'s. What HyperCast answers on a shape is
/// its own, and has moved under them: `1,23,456`, `12,34` and `0,125`, which 1.0 read as 123456, 1234
/// and 125, are refused now.
#[test]
fn one_point_oh_exports_read_what_the_corpus_says() {
    let cases: Vec<Value> = serde_json::from_str(&committed()).expect("the corpus is JSON");
    let mut read = 0;
    for case in &cases {
        let entry = &case["dialect"];
        // A declared encoding is decoded ahead of every export, by the caller.
        let options = [
            "skip",
            "header_row",
            "short_rows",
            "long_rows",
            "tokens",
            "encoding",
        ];
        let plain_plan = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .all(|column| column.get("tokens").is_none() && column.get("options").is_none());
        // The grouping policy, from bit 8, is nothing to `_fill`: `kernel_delimited` holds that.
        let one_point_oh_flags = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .all(|column| column["format"]["flags"].as_u64().unwrap_or(0) <= 0xFF);
        if options.iter().any(|key| entry.get(key).is_some()) || !plain_plan || !one_point_oh_flags
        {
            continue;
        }
        read += 1;
        let name = case["name"].as_str().expect("name");
        let input = match case.get("input_hex") {
            Some(hex) => unhex(hex.as_str().expect("input_hex")),
            None => case["input"].as_str().expect("input").as_bytes().to_vec(),
        };
        let dialect = RawDialect {
            separator: entry["separator"].as_str().expect("separator").as_bytes()[0],
            quoting: u8::from(entry["quoting"].as_bool().expect("quoting")),
            skip_blank_lines: u8::from(entry["skip_blank_lines"].as_bool().expect("skip")),
            engine: 0,
        };
        let header = entry["has_header"].as_bool().expect("has_header");
        let specs: Vec<ColumnSpec> = case["plan"]
            .as_array()
            .expect("plan")
            .iter()
            .map(|column| {
                let column = column_of(column);
                let param = match column.door {
                    Door::Unix(precision) => precision as u32,
                    Door::DateOrdered(order) | Door::DateTime(order) => order as u32,
                    Door::ExcelSerial(epoch) => epoch as u32,
                    _ => 0,
                };
                let mut spec = ColumnSpec::new(column.ordinal as u32, column.door.code(), param);
                spec.format = hypercast::RawNumFormat::from(column.format);
                spec
            })
            .collect();
        let old = through_the_exports(&input, dialect, header, &specs, false);
        let rows = case["rows"].as_array().expect("rows");
        // What `_faults` lists of them, a plain door's verdicts holding no notes.
        let problems: Vec<RawProblem> = case["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .map(|problem| {
                let number = |value: &Value| value.as_u64().expect("a number") as u32;
                RawProblem {
                    row: number(&problem["row"]),
                    plan_column: number(&problem["plan_column"]),
                    reason: expected_verdict(&json_of_reason(problem)).reason,
                    offset: number(&problem["fault"][0]),
                    len: number(&problem["fault"][1]),
                    ..RawProblem::default()
                }
            })
            .collect();
        assert_eq!(old.problems, problems, "{name}: what _faults lists");
        for (column, verdicts) in old.verdicts.iter().enumerate() {
            let expected: Vec<CellVerdict> = rows
                .iter()
                .map(|row| expected_verdict(&row[column]))
                .collect();
            let reasons = |all: &[CellVerdict]| -> Vec<u32> {
                all.iter().map(|verdict| verdict.reason).collect()
            };
            assert_eq!(
                reasons(verdicts),
                reasons(&expected),
                "{name}, column {column}"
            );
            for (at, (seen, want)) in verdicts.iter().zip(&expected).enumerate() {
                if want.reason > 1 {
                    assert_eq!(seen, want, "{name}, column {column}, row {at}");
                }
                if let Some(value) = rows[at][column].get("value") {
                    let door = specs[column].door;
                    if let Some(number) = value_said(door, value) {
                        assert_eq!(
                            old.values[column][at], number,
                            "{name}, column {column}, row {at}"
                        );
                    }
                }
            }
        }
        match case.get("failure") {
            Some(failure) => {
                let seen = old.failure.expect("a failure the corpus says");
                assert_eq!(seen.1, failure["record"].as_u64().unwrap(), "{name}");
                assert_eq!(
                    u64::from(seen.2),
                    failure["line"].as_u64().unwrap(),
                    "{name}"
                );
                assert_eq!(seen.3, failure["byte"].as_u64().unwrap(), "{name}");
            }
            None => assert_eq!(old.failure, None, "{name}"),
        }
        assert_eq!(
            through_the_exports(&input, dialect, header, &specs, true),
            old,
            "{name}: fill_with, asked nothing new, against fill"
        );
    }
    assert!(read >= 53, "{read} cases went through the 1.0 exports");
}
