//! The shapes of `corpus/delimited.json`, shared by the example that writes it and the
//! test that replays it. A cell is written the way HyperCast's own corpus writes a
//! verdict: `{"expect": "ok", …the value…}` or `{"expect": "empty" | "malformed" |
//! "out_of_range", "fault": [offset, len]}` — plus `"raw"`, the cell's text, on a cell
//! that did not cast, which a binding has to be able to give back: `"raw_hex"` in its place
//! when those bytes aren't UTF-8, as `"input_hex"` stands for `"input"`.

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
        _ => unreachable!("the fixture declares a known door"),
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
        "decimal_sep": format.decimal_sep.as_str(),
        "group_sep": format.group_sep.as_str(),
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
    let mut fault = json!({
        "expect": reason_name(reason),
        "fault": [offset, len],
    });
    match std::str::from_utf8(raw) {
        Ok(text) => fault["raw"] = json!(text),
        Err(_) => fault["raw_hex"] = json!(hex(raw)),
    }
    fault
}

/// An origin's name in the corpus: an ordinary cell, the default, has no key.
pub fn origin_name(origin: hypertabular::Origin) -> Option<&'static str> {
    match origin {
        hypertabular::Origin::Cell => None,
        hypertabular::Origin::Token => Some("token"),
        hypertabular::Origin::Padded => Some("padded"),
        hypertabular::Origin::Uncached => Some("uncached"),
        other => panic!("origin {other:?} has no corpus name"),
    }
}

/// Bytes in the corpus's `input_hex` and `raw_hex` shape: two lowercase digits a byte.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The bytes a case's `input_hex` or `raw_hex` names.
pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("two hex digits"))
        .collect()
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
    json!({"expect": "ok", "value": hex(&value)})
}

pub fn text_json(value: &[u8]) -> Value {
    json!({"expect": "ok", "text": String::from_utf8_lossy(value)})
}

/// The keys of a workbook case, in the order `corpus/workbook.json` writes them.
pub const WORKBOOK_KEYS: [&str; 15] = [
    "name", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header", "numbers",
    "rows", "problems", "failure", "note", "suggest",
];

/// `corpus/workbook.json` as text: one case an object, one key a line, and one row of
/// `rows` or `problems` a line — the file is thousands of cells, and a line a row is what keeps a
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
            match case[key]
                .as_array()
                .filter(|_| matches!(*key, "rows" | "problems"))
            {
                Some(rows) if !rows.is_empty() => {
                    text.push_str(&format!("    \"{key}\": [\n"));
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
        let mut cell = fault_json(fault.reason, fault.offset, fault.len, &raw);
        if let Some(detail) = batch.detail(column, row) {
            cell["detail"] = json!(detail.name());
        }
        if let Some(origin) = origin_name(batch.origin(column, row)) {
            cell["origin"] = json!(origin);
        }
        return cell;
    }
    if batch.columns()[column].is_checked() {
        return checked_cell_json(batch, column, row);
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
        _ => unreachable!("the fixture declares a known door"),
    }
}

/// A plan entry's `constraints`, in HyperCast's corpus shape, as the C ABI takes them for `door`:
/// the layout and the packed `enum`.
pub fn declaration_of(door: Door, c: &Value) -> (hypercast::RawConstraints, Vec<u8>) {
    use hypercast::{Constraints, NumericRules, Rounding, TextRules, decimal_for_declaration};
    let decimal = |key: &str| {
        c.get(key).map(|text| {
            decimal_for_declaration(text.as_str().expect("a declared decimal"))
                .unwrap_or_else(|refusal| panic!("{c}: {key}: {refusal:?}"))
        })
    };
    let strings: Option<Vec<&str>> = c.get("enum").map(strings_of);
    let declared = if door == Door::Text {
        let length = |key: &str| c.get(key).map(|n| n.as_u64().expect("a length") as u32);
        let flag = |key: &str| c.get(key).is_some_and(|f| f.as_bool().expect("a flag"));
        let rules = TextRules::new()
            .maybe_min_length(length("minLength"))
            .maybe_max_length(length("maxLength"))
            .trim(flag("trim"))
            .case_sensitive(flag("caseSensitive"));
        Constraints::text(rules, strings.as_deref()).map(|declared| {
            (
                declared.raw(),
                declared.pack_enum(&mut []).map(|len| {
                    let mut packed = vec![0; len];
                    declared.pack_enum(&mut packed);
                    packed
                }),
            )
        })
    } else {
        let rounding = match c.get("rounding").map(|r| r.as_str().expect("rounding")) {
            None | Some("strict") => Rounding::Strict,
            Some("midpoint") => Rounding::Midpoint,
            Some("floor") => Rounding::Floor,
            Some("ceiling") => Rounding::Ceiling,
            Some(other) => panic!("rounding {other}"),
        };
        let rules = NumericRules::new()
            .maybe_minimum(decimal("minimum"))
            .maybe_exclusive_minimum(decimal("exclusiveMinimum"))
            .maybe_maximum(decimal("maximum"))
            .maybe_exclusive_maximum(decimal("exclusiveMaximum"))
            .maybe_multiple_of(decimal("multipleOf").map(|step| (step, rounding)));
        let entries: Option<Vec<hypercast::Decimal>> = strings.map(|list| {
            list.iter()
                .map(|entry| decimal_for_declaration(entry).expect("an enum decimal"))
                .collect()
        });
        Constraints::<hypercast::Decimal>::numeric(rules, entries.as_deref()).map(|declared| {
            (
                declared.raw(),
                declared.pack_enum(&mut []).map(|len| {
                    let mut packed = vec![0; len];
                    declared.pack_enum(&mut packed);
                    packed
                }),
            )
        })
    };
    let (raw, packed) = declared.unwrap_or_else(|refusal| panic!("{c}: {refusal:?}"));
    (raw, packed.expect("an enum that packs"))
}

/// A checked column of `door` at `ordinal`, declaring `c`.
pub fn checked_column(ordinal: usize, door: Door, c: &Value) -> hypertabular::Column {
    use hypercast::Constraints;
    use hypertabular::Column;
    let (raw, packed) = declaration_of(door, c);
    macro_rules! typed {
        ($ty:ty) => {
            Column::checked(
                ordinal,
                Constraints::<$ty>::from_raw(raw, &packed).expect("a declaration for its door"),
            )
        };
    }
    match door {
        Door::I8 => typed!(i8),
        Door::I16 => typed!(i16),
        Door::I32 => typed!(i32),
        Door::I64 => typed!(i64),
        Door::U8 => typed!(u8),
        Door::U16 => typed!(u16),
        Door::U32 => typed!(u32),
        Door::U64 => typed!(u64),
        Door::F32 => typed!(f32),
        Door::F64 => typed!(f64),
        Door::Decimal => typed!(hypercast::Decimal),
        Door::Text => typed!(hypercast::Text),
        other => panic!("the {other:?} door takes no constraints"),
    }
}

/// A float's bits, as the corpus writes them beside a constrained float's value: what tells `-0`
/// from `0`.
pub fn bits_json(door: Door, value: f64) -> Value {
    match door {
        Door::F32 => json!(format!("0x{:08x}", (value as f32).to_bits())),
        _ => json!(format!("0x{:016x}", value.to_bits())),
    }
}

/// What a checked door said beside the value: `Ok((adjusted, enum_index))` when it passed, or
/// the violation's `(keyword, offset, len)`.
pub type Judged = Result<(bool, Option<u32>), (hypercast::Keyword, u32, u32)>;

/// A checked door's value that passed, or that broke `keyword` at `fault`, in the corpus's shape:
/// `value` is the door's ok cell, to which the check is added. `raw` is the cell's text.
pub fn judged_json(door: Door, mut value: Value, passed: Judged, raw: &[u8]) -> Value {
    if let Some(number) = value.get("value").and_then(Value::as_f64)
        && matches!(door, Door::F32 | Door::F64)
    {
        value["bits"] = bits_json(door, number);
    }
    match passed {
        Ok((adjusted, enum_index)) => {
            value["adjusted"] = json!(adjusted);
            if let Some(index) = enum_index {
                value["enum_index"] = json!(index);
            }
        }
        Err((keyword, offset, len)) => {
            value["expect"] = json!("violation");
            value["keyword"] = json!(keyword.name());
            value["fault"] = json!([offset, len]);
            match std::str::from_utf8(raw) {
                Ok(text) => value["raw"] = json!(text),
                Err(_) => value["raw_hex"] = json!(hex(raw)),
            }
        }
    }
    value
}

/// A constrained column's cell that cast, through its checked getter: passed, or a violation.
fn checked_cell_json(batch: &hypertabular::Batch<'_>, column: usize, row: usize) -> Value {
    use hypertabular::{Checked, Decimal, Rejection};
    let door = batch.columns()[column].door;
    let raw = batch.raw(column, row);
    fn split<T>(judged: Result<Checked<T>, Rejection<T>>) -> (T, Judged) {
        match judged {
            Ok(passed) => (passed.value, Ok((passed.adjusted, passed.enum_index))),
            Err(Rejection::Violation(broken)) => (
                broken.value,
                Err((broken.keyword, broken.offset, broken.len)),
            ),
            Err(Rejection::Fault(fault)) => panic!("a fault the verdict didn't say: {fault:?}"),
        }
    }
    let number = |value: Value| json!({"expect": "ok", "value": value});
    macro_rules! numeric {
        ($ty:ty, $json:expr) => {{
            let (value, passed) = split(batch.get_checked::<$ty>(column, row));
            judged_json(door, $json(value), passed, &raw)
        }};
    }
    match door {
        Door::I8 => numeric!(i8, |v| number(json!(v))),
        Door::I16 => numeric!(i16, |v| number(json!(v))),
        Door::I32 => numeric!(i32, |v| number(json!(v))),
        Door::I64 => numeric!(i64, |v| number(json!(v))),
        Door::U8 => numeric!(u8, |v| number(json!(v))),
        Door::U16 => numeric!(u16, |v| number(json!(v))),
        Door::U32 => numeric!(u32, |v| number(json!(v))),
        Door::U64 => numeric!(u64, |v| number(json!(v))),
        Door::F32 => numeric!(f32, |v: f32| number(json!(f64::from(v)))),
        Door::F64 => numeric!(f64, |v| number(json!(v))),
        Door::Decimal => numeric!(Decimal, decimal_json),
        Door::Text => {
            let (value, passed) = split(batch.text_checked(column, row));
            judged_json(door, text_json(value.as_bytes()), passed, &raw)
        }
        other => panic!("the {other:?} door takes no constraints"),
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

/// What a case's `problems` should say, worked out from what its `rows` say and owing the faults
/// list nothing: a cell a door refused and a formula with no cached result, by row and then by
/// column, after the long rows `long` says by hand as `(row, expected, found)`. `lines` is where
/// each row came from and `sources` each plan column's source column.
pub fn problems_of(
    rows: &[Value],
    lines: &[u32],
    sources: &[usize],
    long: &[(usize, u32, u32)],
) -> Vec<Value> {
    let mut problems = Vec::new();
    for (at, cells) in rows.iter().enumerate() {
        if let Some((_, expected, found)) = long.iter().find(|entry| entry.0 == at) {
            problems.push(json!({
                "row": at,
                "line": lines[at],
                "expected": expected,
                "found": found,
            }));
        }
        for (column, cell) in cells.as_array().expect("a row").iter().enumerate() {
            let reason = cell["expect"].as_str().expect("expect");
            let refused = matches!(reason, "malformed" | "out_of_range" | "violation");
            if !refused && cell.get("origin") != Some(&json!("uncached")) {
                continue;
            }
            let mut problem = json!({
                "row": at,
                "line": lines[at],
                "plan_column": column,
                "source_column": sources[column],
                "reason": reason,
            });
            for key in ["fault", "detail", "origin", "keyword"] {
                if let Some(value) = cell.get(key) {
                    problem[key] = value.clone();
                }
            }
            problems.push(problem);
        }
    }
    problems
}

/// A batch's [`Batch::faults`](hypertabular::Batch::faults) in the corpus's shape, its rows
/// counted from `base`.
pub fn problems_json(batch: &hypertabular::Batch<'_>, base: usize, problems: &mut Vec<Value>) {
    for problem in batch.faults() {
        let mut entry = json!({"row": base + problem.row, "line": problem.line});
        match (problem.plan_column, problem.source_column, problem.reason) {
            (Some(column), Some(source), Some(reason)) => {
                entry["plan_column"] = json!(column);
                entry["source_column"] = json!(source);
                entry["reason"] = json!(reason_name(reason));
                if reason != Reason::Empty {
                    entry["fault"] = json!([problem.offset, problem.len]);
                } else {
                    assert_eq!((problem.offset, problem.len), (0, 0));
                }
                assert_eq!((problem.expected, problem.found), (0, 0));
            }
            (Some(column), Some(source), None) => {
                let keyword = problem
                    .keyword
                    .expect("a cell's entry with no reason is a violation");
                entry["plan_column"] = json!(column);
                entry["source_column"] = json!(source);
                entry["reason"] = json!("violation");
                entry["fault"] = json!([problem.offset, problem.len]);
                entry["keyword"] = json!(keyword.name());
            }
            (None, None, None) => {
                entry["expected"] = json!(problem.expected);
                entry["found"] = json!(problem.found);
                assert_eq!((problem.offset, problem.len), (0, 0));
            }
            other => panic!("a problem that's neither a cell's nor a row's: {other:?}"),
        }
        if let Some(detail) = problem.detail {
            entry["detail"] = json!(detail.name());
        }
        if let Some(origin) = origin_name(problem.origin) {
            entry["origin"] = json!(origin);
        }
        problems.push(entry);
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

/// A JSON list of strings.
pub fn strings_of(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|token| token.as_str().expect("a string"))
        .collect()
}

/// The bits a case's date-option names stand for: HyperCast's corpus spelling.
pub fn date_flag(name: &str) -> u32 {
    match name {
        "compact_date" => hypercast::DateOptions::COMPACT_DATE,
        "space_separator" => hypercast::DateOptions::SPACE_SEPARATOR,
        "hour_offset" => hypercast::DateOptions::HOUR_OFFSET,
        other => panic!("date flag {other}"),
    }
}

/// A workbook case's `options`: the two every case has, and the ones only some do.
pub fn sheet_options_of(options: &Value) -> hypertabular::SheetOptions {
    let number = |key: &str| {
        options
            .get(key)
            .map_or(0, |value| value.as_u64().expect("a count") as u32)
    };
    let mut read = hypertabular::SheetOptions::new()
        .with_header(options["has_header"].as_bool().expect("has_header"))
        .with_empty_rows_skipped(options["skip_empty_rows"].as_bool().expect("skip"))
        .with_skip(number("skip"))
        .with_header_row(number("header_row"));
    if let Some(range) = options.get("range") {
        read = read.with_range(
            hypertabular::Range::parse(range.as_str().expect("a range")).expect("a range"),
        );
    }
    if let Some(tokens) = options.get("tokens") {
        read = read.with_tokens(&strings_of(tokens));
    }
    read
}

/// A case's dialect, the options it may declare beside the 1.0 four.
pub fn dialect_of(entry: &Value) -> hypertabular::Dialect {
    let mut dialect =
        hypertabular::Dialect::new(entry["separator"].as_str().expect("separator").as_bytes()[0])
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
            "reject" => hypertabular::ShortRows::Reject,
            "pad" => hypertabular::ShortRows::Pad,
            other => panic!("short_rows {other}"),
        });
    }
    if let Some(policy) = entry.get("long_rows") {
        dialect = dialect.with_long_rows(match policy.as_str().expect("long_rows") {
            "reject" => hypertabular::LongRows::Reject,
            "report" => hypertabular::LongRows::Report,
            "truncate" => hypertabular::LongRows::Truncate,
            other => panic!("long_rows {other}"),
        });
    }
    if let Some(tokens) = entry.get("tokens") {
        dialect = dialect.with_tokens(&strings_of(tokens));
    }
    if let Some(encoding) = entry.get("encoding") {
        dialect = dialect.with_encoding(match encoding.as_str().expect("encoding") {
            "windows-1252" => hypertabular::Encoding::Windows1252,
            "latin-1" => hypertabular::Encoding::Latin1,
            "utf-16le" => hypertabular::Encoding::Utf16Le,
            other => panic!("encoding {other}"),
        });
    }
    dialect
}

/// A plan entry as a column.
pub fn column_of(entry: &Value) -> hypertabular::Column {
    let ordinal = entry["ordinal"].as_u64().expect("ordinal") as usize;
    // An entry with no format is a column given none, so the vector says what `Column::new` reads.
    let mut column = match entry.get("constraints") {
        Some(declared) => checked_column(ordinal, door_of(entry), declared),
        None => hypertabular::Column::new(ordinal, door_of(entry)),
    };
    if entry.get("format").is_some() {
        column = column.format(format_of(entry));
    }
    if let Some(tokens) = entry.get("tokens") {
        column = column.tokens(&strings_of(tokens));
    }
    if let Some(options) = entry.get("options") {
        let flags = options["flags"]
            .as_array()
            .expect("flags")
            .iter()
            .fold(0, |flags, name| {
                flags | date_flag(name.as_str().expect("a flag"))
            });
        let pivot = options
            .get("pivot")
            .map_or(0, |pivot| pivot.as_u64().expect("pivot") as u16);
        let declared = hypercast::DateOptions::from_flags(flags).with_pivot(pivot);
        let months: Option<Vec<(u8, &str)>> = options.get("months").map(|months| {
            months
                .as_array()
                .expect("months")
                .iter()
                .map(|entry| {
                    (
                        entry[0].as_u64().expect("a month") as u8,
                        entry[1].as_str().expect("a name"),
                    )
                })
                .collect()
        });
        column = column.date_options(
            declared,
            months.as_deref().map(hypercast::MonthNames::from_entries),
        );
    }
    column
}

/// A cell of `rows`, in either file. A replay compares a cell whole, so this says what a binding
/// that reads cell by cell has to know.
const CELL_KEYS: &[&str] = &[
    "expect",
    "value",
    "text",
    "seconds",
    "nanos",
    "year",
    "month",
    "day",
    "nanos_of_day",
    "magnitude",
    "scale",
    "negative",
    "fault",
    "raw",
    "raw_hex",
    "detail",
    "origin",
    "keyword",
    "adjusted",
    "enum_index",
    "bits",
];
const PROBLEM_KEYS: &[&str] = &[
    "row",
    "line",
    "plan_column",
    "source_column",
    "reason",
    "fault",
    "detail",
    "origin",
    "expected",
    "found",
    "keyword",
];
const FAILURE_KEYS: &[&str] = &["kind", "record", "line", "byte", "expected", "found"];
const PLAN_KEYS: &[&str] = &[
    "door",
    "ordinal",
    "precision",
    "order",
    "epoch",
    "format",
    "tokens",
    "options",
    "constraints",
];
/// HyperCast's corpus names for a declaration's keywords.
const CONSTRAINT_KEYS: &[&str] = &[
    "minimum",
    "exclusiveMinimum",
    "maximum",
    "exclusiveMaximum",
    "multipleOf",
    "rounding",
    "minLength",
    "maxLength",
    "enum",
    "trim",
    "caseSensitive",
];
const FORMAT_KEYS: &[&str] = &["decimal_sep", "group_sep", "flags", "currency"];
const DATE_OPTION_KEYS: &[&str] = &["flags", "pivot", "months"];

/// Adds to `unknown` the path of every key of `value` that isn't in `known`.
fn unknown_keys(value: &Value, path: &str, known: &[&str], unknown: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for key in object.keys().filter(|key| !known.contains(&key.as_str())) {
        unknown.push(format!("{path}.{key}"));
    }
}

/// What every case of either file shares: the plan, the rows, the problems and the failure.
fn unknown_shared_keys(case: &Value, name: &str, unknown: &mut Vec<String>) {
    let items = |key: &str| {
        case.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    };
    for column in items("plan") {
        let path = format!("{name}.plan[]");
        unknown_keys(column, &path, PLAN_KEYS, unknown);
        if let Some(format) = column.get("format") {
            unknown_keys(format, &format!("{path}.format"), FORMAT_KEYS, unknown);
        }
        if let Some(declared) = column.get("constraints") {
            unknown_keys(
                declared,
                &format!("{path}.constraints"),
                CONSTRAINT_KEYS,
                unknown,
            );
        }
        if let Some(options) = column.get("options") {
            unknown_keys(
                options,
                &format!("{path}.options"),
                DATE_OPTION_KEYS,
                unknown,
            );
        }
    }
    for row in items("rows") {
        for cell in row.as_array().into_iter().flatten() {
            unknown_keys(cell, &format!("{name}.rows[][]"), CELL_KEYS, unknown);
        }
    }
    for problem in items("problems") {
        unknown_keys(
            problem,
            &format!("{name}.problems[]"),
            PROBLEM_KEYS,
            unknown,
        );
    }
    if let Some(failure) = case.get("failure") {
        unknown_keys(failure, &format!("{name}.failure"), FAILURE_KEYS, unknown);
    }
}

/// Every key of `corpus/delimited.json` that its replay doesn't know how to read, as a path. A
/// new key has to be taught to the replay, here and in `corpus/README.md`, before it can be in
/// the file: a replay that passed over one would pass whatever the key said.
pub fn unknown_delimited_keys(cases: &[Value]) -> Vec<String> {
    let mut unknown = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or("a case with no name");
        unknown_keys(
            case,
            name,
            &[
                "name",
                "input",
                "input_hex",
                "dialect",
                "plan",
                "header",
                "rows",
                "problems",
                "failure",
                "suggest",
            ],
            &mut unknown,
        );
        unknown_keys(
            &case["dialect"],
            &format!("{name}.dialect"),
            &[
                "separator",
                "quoting",
                "has_header",
                "skip_blank_lines",
                "skip",
                "header_row",
                "short_rows",
                "long_rows",
                "tokens",
                "encoding",
            ],
            &mut unknown,
        );
        unknown_shared_keys(case, name, &mut unknown);
        unknown_suggest_keys(case, name, &mut unknown);
    }
    unknown
}

/// The keys of a case's `suggest`, which a delimited case and a workbook case share.
fn unknown_suggest_keys(case: &Value, name: &str, unknown: &mut Vec<String>) {
    let Some(suggest) = case.get("suggest") else {
        return;
    };
    let path = format!("{name}.suggest");
    unknown_keys(
        suggest,
        &path,
        &["columns", "truncated", "sample_rows", "format"],
        unknown,
    );
    if let Some(format) = suggest.get("format") {
        unknown_keys(format, &format!("{path}.format"), FORMAT_KEYS, unknown);
    }
    for column in suggest["columns"].as_array().into_iter().flatten() {
        unknown_keys(
            column,
            &format!("{path}.columns[]"),
            &["door", "param", "status", "matched", "sampled"],
            unknown,
        );
    }
}

/// [`unknown_delimited_keys`], for `corpus/workbook.json`. `note` is the one key no replay reads: a
/// sentence for whoever reads the file.
pub fn unknown_workbook_keys(cases: &[Value]) -> Vec<String> {
    let mut unknown = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or("a case with no name");
        unknown_keys(case, name, &WORKBOOK_KEYS, &mut unknown);
        unknown_keys(
            &case["options"],
            &format!("{name}.options"),
            &[
                "has_header",
                "skip_empty_rows",
                "range",
                "skip",
                "header_row",
                "tokens",
            ],
            &mut unknown,
        );
        for sheet in case["sheets"].as_array().into_iter().flatten() {
            unknown_keys(
                sheet,
                &format!("{name}.sheets[]"),
                &["name", "hidden"],
                &mut unknown,
            );
        }
        unknown_shared_keys(case, name, &mut unknown);
        unknown_suggest_keys(case, name, &mut unknown);
    }
    unknown
}

/// Every place a key can be added to `case`, for a test that the checks above notice one.
pub fn key_places(case: &Value) -> Vec<String> {
    let mut places = Vec::new();
    let mut walk = vec![(String::new(), case)];
    while let Some((path, value)) = walk.pop() {
        match value {
            Value::Object(object) => {
                places.push(path.clone());
                for (key, inner) in object {
                    walk.push((format!("{path}/{key}"), inner));
                }
            }
            Value::Array(items) => {
                if let Some(first) = items.first() {
                    walk.push((format!("{path}/0"), first));
                }
            }
            _ => {}
        }
    }
    places
}

/// What a [`Suggestion`] says, in the corpus's shape. An ambiguous column's `param` is a set of
/// its candidates, which the Rust surface gives as a list.
pub fn suggestion_json(found: &hypertabular::Suggestion) -> Value {
    let columns: Vec<Value> = found
        .report
        .iter()
        .map(|column| {
            let status = match column.status {
                hypertabular::SuggestStatus::Resolved => "resolved",
                hypertabular::SuggestStatus::Ambiguous => "ambiguous",
                hypertabular::SuggestStatus::Unresolved => "unresolved",
                other => panic!("{other:?}"),
            };
            let code = |door: hypertabular::Door| match door {
                hypertabular::Door::DateOrdered(order) if column.candidates.is_empty() => {
                    order as u64
                }
                _ => 0,
            };
            let set = column.candidates.iter().fold(0, |set, candidate| {
                set | match candidate {
                    hypertabular::Candidate::DateOrder(order) => 1u64 << (*order as u32),
                    hypertabular::Candidate::NumFormat(format) => {
                        if format.decimal_sep.as_str() == "." {
                            hypertabular::kernel::abi::RawSuggestion::NOTATION_COMMA_GROUPS.into()
                        } else {
                            hypertabular::kernel::abi::RawSuggestion::NOTATION_COMMA_DECIMAL.into()
                        }
                    }
                    other => panic!("{other:?}"),
                }
            });
            json!({
                "door": door_json(column.door)["door"],
                "param": if column.candidates.is_empty() { code(column.door) } else { set },
                "status": status,
                "matched": column.matched,
                "sampled": column.sampled,
            })
        })
        .collect();
    json!(columns)
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
    if !matches!(
        failure.kind,
        FailureKind::UnclosedQuote | FailureKind::Encoding
    ) {
        entry["expected"] = json!(failure.expected);
        entry["found"] = json!(failure.found);
    }
    entry
}
