//! Every constrained cell of `corpus/workbook.json`'s `constraints.xlsx` cases, read through the Rust
//! binding, against an oracle that owes the reader nothing: the routing worked out from the fixture's
//! own cells (`tests/support/packages.rs`, `constraints_xlsx`) and the declaration, then HyperCast's
//! checked door asked directly. The delimited cases need no such test: `examples/corpus.rs` writes
//! each of their cells from HyperCast's own verdict on the hand-written text, and
//! `corpus_delimited.rs` holds the reader to it.

#[path = "../examples/corpus.rs"]
mod generator;

use generator::corpus::*;
use hypercast::detailed::{self, Rejection};
use hypercast::{Checked, Constraints, Text, ValidNumFormat};
use hypertabular::{Column, Door, Workbook};
use serde_json::{Value, json};

/// A cell of the fixture, as its part stores it.
#[derive(Clone, Copy)]
enum Fixture {
    Number(f64),
    Text(&'static str),
    Bool,
    Uncached,
}

/// `constraints_xlsx`'s rows below its header, columns A to C.
const ROWS: [[Fixture; 3]; 5] = {
    use Fixture::*;
    [
        [Number(-0.0), Text("  ab "), Bool],
        [Number(0.300_000_000_000_000_04), Text("MALE"), Uncached],
        [Number(42.0), Text(" 11"), Text("NA")],
        [Number(11.0), Text("x"), Number(7.0)],
        [Text("1.3"), Text("Female"), Number(2.0)],
    ]
};

/// The tokens the cases declare.
const TOKENS: [&str; 1] = ["NA"];

/// A typed number's canonical text, as a workbook's typed cell is said: the integer for a whole
/// number, else its shortest round-trip digits (each of the fixture's numbers is in plain notation's
/// range).
fn rendered(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn shaped<T>(
    judged: Result<Checked<T>, Rejection<T>>,
    door: Door,
    raw: &[u8],
    ok: impl Fn(T) -> Value,
) -> Value {
    match judged {
        Ok(passed) => judged_json(
            door,
            ok(passed.value),
            Ok((passed.adjusted, passed.enum_index)),
            raw,
        ),
        Err(Rejection::Violation(broken)) => judged_json(
            door,
            ok(broken.value),
            Err((broken.keyword, broken.offset, broken.len)),
            raw,
        ),
        Err(Rejection::Failure(failure)) => {
            let mut cell = fault_json(failure.reason, failure.offset, failure.len, raw);
            if failure.reason != hypercast::Reason::Empty || raw.is_empty() {
                cell["detail"] = json!(failure.detail.name());
            }
            cell
        }
    }
}

/// What the oracle says of `cell` through `door` under `declared`.
fn oracle(cell: Fixture, door: Door, declared: &Value) -> Value {
    let (raw, packed) = declaration_of(door, declared);
    let format = &ValidNumFormat::INVARIANT;
    let number = |value: Value| json!({"expect": "ok", "value": value});
    match cell {
        // No door is asked about a formula with no result, or a token.
        Fixture::Uncached => return json!({"expect": "empty", "origin": "uncached"}),
        Fixture::Text(text) if TOKENS.contains(&text) => {
            return json!({"expect": "empty", "origin": "token"});
        }
        _ => {}
    }
    if door == Door::Text {
        let constraints = Constraints::<Text>::from_raw(raw, &packed).expect("a declaration");
        let text = match cell {
            Fixture::Text(text) => text.to_owned(),
            Fixture::Number(value) => rendered(value),
            Fixture::Bool => "true".to_owned(),
            Fixture::Uncached => unreachable!(),
        };
        let judged = detailed::cast_text_checked(text.as_bytes(), &constraints);
        return shaped(judged, door, text.as_bytes(), |v: &str| {
            text_json(v.as_bytes())
        });
    }
    macro_rules! numeric {
        ($ty:ty, $ok:expr) => {{
            let constraints = Constraints::<$ty>::from_raw(raw, &packed).expect("a declaration");
            match cell {
                Fixture::Text(text) => shaped(
                    detailed::cast_checked(text, format, &constraints),
                    door,
                    text.as_bytes(),
                    $ok,
                ),
                // A typed number has no text: its violation spans nothing, and so does its raw.
                Fixture::Number(value) => {
                    match detailed::checked_from_f64::<$ty>(value, &constraints) {
                        Err(Rejection::Failure(failure)) => {
                            // A typed cell that fails its door is said back as text.
                            let text = rendered(value);
                            let mut cell = fault_json(failure.reason, 0, text.len() as u32, text.as_bytes());
                            cell["detail"] = json!(failure.detail.name());
                            cell
                        }
                        judged => shaped(judged, door, b"", $ok),
                    }
                }
                // A door with no twin for a boolean: malformed, spanning its text, no detail.
                Fixture::Bool => json!({"expect": "malformed", "fault": [0, 4], "raw": "true"}),
                Fixture::Uncached => unreachable!(),
            }
        }};
    }
    match door {
        Door::I32 => numeric!(i32, |v| number(json!(v))),
        Door::F32 => numeric!(f32, |v: f32| number(json!(f64::from(v)))),
        Door::F64 => numeric!(f64, |v| number(json!(v))),
        Door::Decimal => numeric!(hypercast::Decimal, decimal_json),
        other => panic!("the cases declare no {other:?} column"),
    }
}

#[test]
fn every_constrained_workbook_cell_is_what_hypercast_says() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook.json");
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the corpus")).expect("JSON");
    let mut compared = 0;
    for case in cases.iter().filter(|case| {
        case["file"] == "workbook/constraints.xlsx"
            && case["plan"]
                .as_array()
                .expect("plan")
                .iter()
                .any(|entry| entry.get("constraints").is_some())
    }) {
        let name = case["name"].as_str().expect("name");
        assert_eq!(case["options"]["tokens"], json!(TOKENS), "{name}");
        let plan: Vec<Value> = case["plan"].as_array().expect("plan").clone();
        let columns: Vec<Column> = plan.iter().map(column_of).collect();
        let book = Workbook::open(format!("{}/constraints.xlsx", generator::WORKBOOK_DIR))
            .expect("it opens");
        let mut sheet = book
            .sheet(0, sheet_options_of(&case["options"]), &columns)
            .expect("a sheet");
        let mut rows = Vec::new();
        while let Some(batch) = sheet.read().expect("a batch") {
            rows_json(&batch, &mut rows);
        }
        assert_eq!(rows.len(), ROWS.len(), "{name}");
        for (at, row) in rows.iter().enumerate() {
            for (place, entry) in plan.iter().enumerate() {
                let ordinal = entry["ordinal"].as_u64().expect("ordinal") as usize;
                let declared = entry
                    .get("constraints")
                    .expect("every column declares some");
                let expected = oracle(ROWS[at][ordinal], door_of(entry), declared);
                assert_eq!(row[place], expected, "{name}, row {at}, column {place}");
                compared += 1;
            }
        }
    }
    assert!(compared >= 40, "{compared} cells compared");
}
