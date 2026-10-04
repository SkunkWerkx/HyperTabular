//! The shapes of `corpus/delimited.json`, shared by the example that writes it and the
//! test that replays it. A cell is written the way HyperCast's own corpus writes a
//! verdict: `{"expect": "ok", …the value…}` or `{"expect": "empty" | "malformed" |
//! "out_of_range", "fault": [offset, len]}` — plus `"raw"`, the cell's text, on a cell
//! that did not cast, which a binding has to be able to give back.

#![allow(dead_code)]

use hypertabular::{Door, NumFormat, Reason};
use serde_json::{Value, json};

/// A door's name in the corpus, and what it declares beside it.
pub fn door_json(door: Door) -> Value {
    match door {
        Door::Bool => json!({"door": "bool"}),
        Door::I8 => json!({"door": "i8"}),
        Door::I16 => json!({"door": "i16"}),
        Door::I32 => json!({"door": "i32"}),
        Door::I64 => json!({"door": "i64"}),
        Door::U8 => json!({"door": "u8"}),
        Door::U16 => json!({"door": "u16"}),
        Door::U32 => json!({"door": "u32"}),
        Door::U64 => json!({"door": "u64"}),
        Door::F32 => json!({"door": "f32"}),
        Door::F64 => json!({"door": "f64"}),
        Door::Decimal => json!({"door": "decimal"}),
        Door::Uuid => json!({"door": "uuid"}),
        Door::Timestamp => json!({"door": "timestamp"}),
        Door::Unix(precision) => json!({"door": "unix", "precision": precision as u32}),
        Door::ExcelSerial(epoch) => json!({"door": "excel_serial", "epoch": epoch as u32}),
        Door::Date => json!({"door": "date"}),
        Door::DateOrdered(order) => json!({"door": "date_ordered", "order": order as u32}),
        Door::DateTime(order) => json!({"door": "datetime", "order": order as u32}),
        Door::Time => json!({"door": "time"}),
        Door::Duration => json!({"door": "duration"}),
        Door::Text => json!({"door": "text"}),
    }
}

/// The door a plan entry names.
pub fn door_of(entry: &Value) -> Door {
    let param = |key: &str| {
        entry[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{entry}: {key}")) as u32
    };
    let code = match entry["door"].as_str().expect("door") {
        "bool" => (1, 0),
        "i8" => (2, 0),
        "i16" => (3, 0),
        "i32" => (4, 0),
        "i64" => (5, 0),
        "u8" => (6, 0),
        "u16" => (7, 0),
        "u32" => (8, 0),
        "u64" => (9, 0),
        "f32" => (10, 0),
        "f64" => (11, 0),
        "uuid" => (12, 0),
        "timestamp" => (13, 0),
        "unix" => (14, param("precision")),
        "date" => (15, 0),
        "time" => (16, 0),
        "duration" => (17, 0),
        "text" => (18, 0),
        "decimal" => (19, 0),
        "date_ordered" => (20, param("order")),
        "datetime" => (21, param("order")),
        "excel_serial" => (22, param("epoch")),
        other => panic!("unknown door {other}"),
    };
    Door::from_code(code.0, code.1).unwrap_or_else(|| panic!("{entry}"))
}

pub fn format_json(format: &NumFormat) -> Value {
    let mut value = json!({
        "decimal_sep": format.decimal_sep.to_string(),
        "group_sep": format.group_sep.to_string(),
        "flags": format.flags,
    });
    if !format.currency.is_empty() {
        value["currency"] = json!(format.currency.as_str());
    }
    value
}

pub fn format_of(entry: &Value) -> NumFormat {
    let Some(format) = entry.get("format") else {
        return NumFormat::INVARIANT;
    };
    let one = |key: &str| {
        format[key]
            .as_str()
            .and_then(|s| s.chars().next())
            .expect("separator")
    };
    let mut parsed = NumFormat::new(
        one("decimal_sep"),
        one("group_sep"),
        format["flags"].as_u64().expect("flags") as u32,
    );
    if let Some(symbol) = format.get("currency").and_then(Value::as_str) {
        parsed = parsed.with_currency(hypercast::CurrencySymbol::new(symbol).expect("currency"));
    }
    parsed
}

pub fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::Empty => "empty",
        Reason::Malformed => "malformed",
        Reason::OutOfRange => "out_of_range",
    }
}

/// A cell that did not cast.
pub fn fault_json(reason: Reason, offset: u32, len: u32, raw: &[u8]) -> Value {
    if reason == Reason::Empty {
        return json!({"expect": "empty"});
    }
    json!({
        "expect": reason_name(reason),
        "fault": [offset, len],
        "raw": String::from_utf8_lossy(raw),
    })
}

pub fn timestamp_json(value: hypercast::Timestamp) -> Value {
    json!({"expect": "ok", "seconds": value.seconds, "nanos": value.nanos})
}

pub fn date_json(value: hypercast::Date) -> Value {
    json!({"expect": "ok", "year": value.year, "month": value.month, "day": value.day})
}

pub fn datetime_json(value: hypercast::CivilDateTime) -> Value {
    json!({
        "expect": "ok",
        "year": value.date.year,
        "month": value.date.month,
        "day": value.date.day,
        "nanos_of_day": value.nanos_of_day,
    })
}

pub fn duration_json(value: hypercast::Duration) -> Value {
    json!({"expect": "ok", "seconds": value.seconds, "nanos": value.nanos})
}

pub fn decimal_json(value: hypercast::Decimal) -> Value {
    json!({
        "expect": "ok",
        "magnitude": value.magnitude().to_string(),
        "scale": value.scale,
        "negative": value.negative,
        "value": value.to_string(),
    })
}

pub fn uuid_json(value: [u8; 16]) -> Value {
    let hex: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
    json!({"expect": "ok", "value": hex})
}

pub fn text_json(value: &[u8]) -> Value {
    json!({"expect": "ok", "text": String::from_utf8_lossy(value)})
}

/// The keys of a workbook case, in the order `corpus/workbook.json` writes them.
pub const WORKBOOK_KEYS: [&str; 13] = [
    "name", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header", "numbers",
    "rows", "failure", "note",
];

/// `corpus/workbook.json` as text: one case an object, one key a line, and one row of
/// `rows` a line — the file is thousands of cells, and a line a row is what keeps a
/// change to it readable.
pub fn workbook_text(cases: &[Value]) -> String {
    let mut text = String::from("[\n");
    for (index, case) in cases.iter().enumerate() {
        text.push_str("  {\n");
        let keys: Vec<&str> = WORKBOOK_KEYS
            .into_iter()
            .filter(|key| case.get(key).is_some())
            .collect();
        assert_eq!(
            keys.len(),
            case.as_object().expect("a case is an object").len(),
            "a case has a key the writer does not know"
        );
        for (at, key) in keys.iter().enumerate() {
            let last = if at + 1 == keys.len() { "" } else { "," };
            match case[key].as_array().filter(|_| *key == "rows") {
                Some(rows) if !rows.is_empty() => {
                    text.push_str("    \"rows\": [\n");
                    for (row, cells) in rows.iter().enumerate() {
                        let comma = if row + 1 == rows.len() { "" } else { "," };
                        text.push_str(&format!("      {cells}{comma}\n"));
                    }
                    text.push_str(&format!("    ]{last}\n"));
                }
                _ => text.push_str(&format!("    \"{key}\": {}{last}\n", case[key])),
            }
        }
        text.push_str(if index + 1 == cases.len() {
            "  }\n"
        } else {
            "  },\n"
        });
    }
    text.push_str("]\n");
    text
}

/// A cell of a batch in the corpus's shape: its value through its column's door, or its
/// fault with the text the fault is a span of.
pub fn cell_json(batch: &hypertabular::Batch<'_>, column: usize, row: usize) -> Value {
    use hypertabular::{CivilDateTime, Date, Decimal, Duration, Timestamp};
    if let Some(fault) = batch.verdicts(column)[row].fault() {
        let raw = batch.raw(column, row);
        return fault_json(fault.reason, fault.offset, fault.len, &raw);
    }
    let number = |value: Value| json!({"expect": "ok", "value": value});
    let read = "a cell whose verdict is ok has a value";
    match batch.columns()[column].door {
        Door::Bool => number(json!(batch.bool(column)[row])),
        Door::I8 => number(json!(batch.i8(column)[row])),
        Door::I16 => number(json!(batch.i16(column)[row])),
        Door::I32 => number(json!(batch.i32(column)[row])),
        Door::I64 => number(json!(batch.i64(column)[row])),
        Door::U8 => number(json!(batch.u8(column)[row])),
        Door::U16 => number(json!(batch.u16(column)[row])),
        Door::U32 => number(json!(batch.u32(column)[row])),
        Door::U64 => number(json!(batch.u64(column)[row])),
        Door::F32 => number(json!(f64::from(batch.f32(column)[row]))),
        Door::F64 => number(json!(batch.f64(column)[row])),
        Door::Decimal => decimal_json(batch.get::<Decimal>(column, row).expect(read)),
        Door::Uuid => uuid_json(batch.get::<[u8; 16]>(column, row).expect(read)),
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => {
            timestamp_json(batch.get::<Timestamp>(column, row).expect(read))
        }
        Door::Date | Door::DateOrdered(_) => date_json(batch.get::<Date>(column, row).expect(read)),
        Door::DateTime(_) => datetime_json(batch.get::<CivilDateTime>(column, row).expect(read)),
        Door::Time => json!({"expect": "ok", "nanos": batch.get::<u64>(column, row).expect(read)}),
        Door::Duration => duration_json(batch.get::<Duration>(column, row).expect(read)),
        Door::Text => text_json(batch.text(column, row).expect(read)),
    }
}

/// Every row of a batch, in the corpus's shape.
pub fn rows_json(batch: &hypertabular::Batch<'_>, rows: &mut Vec<Value>) {
    for row in 0..batch.rows() {
        let cells: Vec<Value> = (0..batch.columns().len())
            .map(|column| cell_json(batch, column, row))
            .collect();
        rows.push(json!(cells));
    }
}

/// A header in the corpus's shape: its names, or `null` for none declared.
pub fn header_json(header: Option<&hypertabular::Header>) -> Value {
    match header {
        Some(header) => json!(
            header
                .names()
                .map(|name| String::from_utf8_lossy(name))
                .collect::<Vec<_>>()
        ),
        None => Value::Null,
    }
}

/// A plan entry as a column.
pub fn column_of(entry: &Value) -> hypertabular::Column {
    let ordinal = entry["ordinal"].as_u64().expect("ordinal") as usize;
    hypertabular::Column::new(ordinal, door_of(entry)).format(format_of(entry))
}

/// A structural failure in the corpus's shape. Only a column-count failure of delimited
/// text and the failures of a workbook say what was expected and found.
pub fn failure_json(error: &hypertabular::Error) -> Value {
    use hypertabular::{Error, FailureKind};
    let Error::Structure(failure) = error else {
        panic!("not a failure the corpus describes: {error:?}");
    };
    let mut entry = json!({
        "kind": failure.kind.name(),
        "record": failure.record,
        "line": failure.line,
        "byte": failure.byte,
    });
    if failure.kind != FailureKind::UnclosedQuote {
        entry["expected"] = json!(failure.expected);
        entry["found"] = json!(failure.found);
    }
    entry
}
