//! Writes `corpus/delimited.json`, the contract every binding replays.
//!
//! A case says two things, by hand: the input, and the text that must reach each cell —
//! which rows there are, where each cell begins and ends, what quoting leaves behind.
//! That grid is the tabular layer's whole job and is never computed. What each plan
//! column then makes of its cell's text is not this layer's to decide: HyperCast is the
//! judge, so the verdict written for a cell is whatever HyperCast's own door says of that
//! text, asked directly, with no reader in between.
//!
//!   cargo run --example corpus
//!
//! `tests/corpus_delimited.rs` replays the file through the reader, and fails if this
//! example would write anything other than what is committed.

#[path = "../tests/support/corpus.rs"]
pub mod corpus;

use corpus::*;
use hypercast::{DateOrder, ExcelEpoch, Fault, UnixPrecision};
use hypertabular::{Door, NumFormat};
use serde_json::{Value, json};

struct Failure {
    kind: &'static str,
    record: u64,
    line: u32,
    byte: u64,
    /// `(expected, found)` cells, for a record of the wrong width.
    width: Option<(u32, u32)>,
}

struct Case {
    name: &'static str,
    input: String,
    separator: char,
    quoting: bool,
    skip_blank_lines: bool,
    has_header: bool,
    plan: Vec<(u32, Door, NumFormat)>,
    /// The header's names, when the dialect declares one.
    header: Option<Vec<&'static str>>,
    /// The text of every cell of every row that is delivered, in file order.
    cells: Vec<Vec<&'static str>>,
    failure: Option<Failure>,
}

impl Case {
    fn csv(name: &'static str, input: &str) -> Case {
        Case {
            name,
            input: input.to_string(),
            separator: ',',
            quoting: true,
            skip_blank_lines: true,
            has_header: true,
            plan: Vec::new(),
            header: None,
            cells: Vec::new(),
            failure: None,
        }
    }

    fn plan(mut self, plan: &[(u32, Door)]) -> Case {
        self.plan = plan
            .iter()
            .map(|&(ordinal, door)| (ordinal, door, NumFormat::INVARIANT))
            .collect();
        self
    }

    fn column(mut self, ordinal: u32, door: Door, format: NumFormat) -> Case {
        self.plan.push((ordinal, door, format));
        self
    }

    fn header(mut self, names: &[&'static str]) -> Case {
        self.header = Some(names.to_vec());
        self
    }

    fn no_header(mut self) -> Case {
        self.has_header = false;
        self
    }

    fn rows(mut self, cells: &[&[&'static str]]) -> Case {
        self.cells = cells.iter().map(|row| row.to_vec()).collect();
        self
    }

    fn fails(mut self, kind: &'static str, record: u64, line: u32, byte: u64) -> Case {
        self.failure = Some(Failure {
            kind,
            record,
            line,
            byte,
            width: None,
        });
        self
    }

    fn width(mut self, expected: u32, found: u32) -> Case {
        if let Some(failure) = &mut self.failure {
            failure.width = Some((expected, found));
        }
        self
    }
}

/// HyperCast's verdict on one cell's text.
fn verdict(door: Door, format: &NumFormat, text: &[u8]) -> Value {
    fn judged<T>(verdict: Result<T, Fault>, text: &[u8], ok: impl Fn(T) -> Value) -> Value {
        match verdict {
            Ok(value) => ok(value),
            Err(fault) => fault_json(fault.reason, fault.offset, fault.len, text),
        }
    }
    let number = |value: Value| json!({"expect": "ok", "value": value});
    match door {
        Door::Bool => judged(hypercast::cast_bool(text), text, |v| number(json!(v))),
        Door::I8 => judged(hypercast::cast_i8(text, format), text, |v| number(json!(v))),
        Door::I16 => judged(hypercast::cast_i16(text, format), text, |v| {
            number(json!(v))
        }),
        Door::I32 => judged(hypercast::cast_i32(text, format), text, |v| {
            number(json!(v))
        }),
        Door::I64 => judged(hypercast::cast_i64(text, format), text, |v| {
            number(json!(v))
        }),
        Door::U8 => judged(hypercast::cast_u8(text, format), text, |v| number(json!(v))),
        Door::U16 => judged(hypercast::cast_u16(text, format), text, |v| {
            number(json!(v))
        }),
        Door::U32 => judged(hypercast::cast_u32(text, format), text, |v| {
            number(json!(v))
        }),
        Door::U64 => judged(hypercast::cast_u64(text, format), text, |v| {
            number(json!(v))
        }),
        Door::F32 => judged(hypercast::cast_f32(text, format), text, |v| {
            number(json!(f64::from(v)))
        }),
        Door::F64 => judged(hypercast::cast_f64(text, format), text, |v| {
            number(json!(v))
        }),
        Door::Decimal => judged(hypercast::cast_decimal(text, format), text, decimal_json),
        Door::Uuid => judged(hypercast::cast_uuid(text), text, uuid_json),
        Door::Timestamp => judged(hypercast::cast_timestamp(text), text, timestamp_json),
        Door::Unix(precision) => {
            judged(hypercast::cast_unix(text, precision), text, timestamp_json)
        }
        Door::ExcelSerial(epoch) => judged(
            hypercast::cast_excel_serial(text, epoch),
            text,
            timestamp_json,
        ),
        Door::Date => judged(hypercast::cast_date(text), text, date_json),
        Door::DateOrdered(order) => {
            judged(hypercast::cast_date_ordered(text, order), text, date_json)
        }
        Door::DateTime(order) => judged(hypercast::cast_datetime(text, order), text, datetime_json),
        Door::Time => judged(
            hypercast::cast_time(text),
            text,
            |v| json!({"expect": "ok", "nanos": v}),
        ),
        Door::Duration => judged(hypercast::cast_duration(text), text, duration_json),
        // The bytes themselves, untrimmed; no bytes at all is the one way to fail.
        Door::Text if text.is_empty() => json!({"expect": "empty"}),
        Door::Text => text_json(text),
    }
}

/// One cell for each door, and the text each is given.
fn every_door() -> Vec<(Door, &'static str)> {
    vec![
        (Door::Bool, "yes"),
        (Door::I8, "-128"),
        (Door::I16, "32767"),
        (Door::I32, "-2147483648"),
        (Door::I64, "9223372036854775807"),
        (Door::U8, "255"),
        (Door::U16, "65535"),
        (Door::U32, "4294967295"),
        (Door::U64, "18446744073709551615"),
        (Door::F32, "2.5"),
        (Door::F64, "-0.125"),
        (Door::Decimal, "1234.50"),
        (Door::Uuid, "6ba7b810-9dad-11d1-80b4-00c04fd430c8"),
        (Door::Timestamp, "2024-01-31T10:30:00.5Z"),
        (Door::Unix(UnixPrecision::Seconds), "1700000000"),
        (Door::Unix(UnixPrecision::Millis), "1700000000123"),
        (Door::Unix(UnixPrecision::Micros), "1700000000123456"),
        (Door::Unix(UnixPrecision::Nanos), "1700000000123456789"),
        (Door::ExcelSerial(ExcelEpoch::Y1900), "45292.75"),
        (Door::ExcelSerial(ExcelEpoch::Y1904), "45292.75"),
        (Door::Date, "2024-01-31"),
        (Door::DateOrdered(DateOrder::YearMonthDay), "2026/1/7"),
        (Door::DateOrdered(DateOrder::MonthDayYear), "1/7/2026"),
        (Door::DateOrdered(DateOrder::DayMonthYear), "1/7/2026"),
        (Door::DateTime(DateOrder::MonthDayYear), "1/7/2026 3:04 PM"),
        (Door::Time, "15:04:05.5"),
        (Door::Duration, "PT1H30M"),
        (Door::Text, "plain"),
    ]
}

fn cases() -> Vec<Case> {
    let doors = every_door();
    let door_plan: Vec<(u32, Door)> = doors
        .iter()
        .enumerate()
        .map(|(ordinal, &(door, _))| (ordinal as u32, door))
        .collect();
    let door_texts: Vec<&'static str> = doors.iter().map(|&(_, text)| text).collect();
    let door_blanks: Vec<&'static str> = doors.iter().map(|_| "").collect();
    let eurozone = NumFormat::new(',', '.', NumFormat::ALL);
    let dollars = NumFormat::new('.', ',', NumFormat::ALL)
        .with_currency(hypercast::CurrencySymbol::new("$").expect("a currency symbol"));

    vec![
        Case::csv("a plain file", "id,name,score\n1,alice,2.5\n2,bob,3\n")
            .plan(&[(0, Door::I32), (1, Door::Text), (2, Door::F64)])
            .header(&["id", "name", "score"])
            .rows(&[&["1", "alice", "2.5"], &["2", "bob", "3"]]),
        Case::csv(
            "quoted cells keep their separators and line breaks",
            "id,note\n1,\"a, b\"\n2,\"line one\nline two\"\n3,\"cr\r\nlf\"\n",
        )
        .plan(&[(0, Door::I32), (1, Door::Text)])
        .header(&["id", "note"])
        .rows(&[
            &["1", "a, b"],
            &["2", "line one\nline two"],
            &["3", "cr\r\nlf"],
        ]),
        Case::csv(
            "a doubled quote is one quote",
            "q\n\"say \"\"hi\"\"\"\n\"\"\"\"\n\"a\"\"b\"\n",
        )
        .plan(&[(0, Door::Text)])
        .header(&["q"])
        .rows(&[&["say \"hi\""], &["\""], &["a\"b"]]),
        Case::csv(
            "a quoted empty cell is as empty as an unquoted one",
            "a,b\n\"\",x\n,y\n",
        )
        .plan(&[(0, Door::Text), (1, Door::Text), (0, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["", "x"], &["", "y"]]),
        Case::csv(
            "a lone quote inside a quoted cell is kept",
            "a\n\"x\"y\"z\"\n",
        )
        .plan(&[(0, Door::Text)])
        .header(&["a"])
        .rows(&[&["x\"y\"z"]]),
        Case::csv(
            "quotes in a cell that does not start with one are ordinary bytes",
            "a,b\nx\"\"y,5 \"inch\"\n",
        )
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .header(&["a", "b"])
        .rows(&[&["x\"\"y", "5 \"inch\""]]),
        Case::csv(
            "a quoted header name",
            "id,\"full name\",\"say \"\"hi\"\"\"\n1,a,b\n",
        )
        .plan(&[(0, Door::I32), (2, Door::Text)])
        .header(&["id", "full name", "say \"hi\""])
        .rows(&[&["1", "a", "b"]]),
        Case::csv(
            "every line ending, and none at the end",
            "a,b\n1,2\r\n3,4\r5,6",
        )
        .plan(&[(0, Door::I32), (1, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"], &["3", "4"], &["5", "6"]]),
        Case::csv("blank lines are skipped", "a,b\n\n1,2\n\r\n\n3,4\n\n")
            .plan(&[(0, Door::I32), (1, Door::I32)])
            .header(&["a", "b"])
            .rows(&[&["1", "2"], &["3", "4"]]),
        {
            let mut case = Case::csv(
                "blank lines are rows when the dialect keeps them",
                "1\n\n3\n",
            )
            .no_header()
            .plan(&[(0, Door::I32)])
            .rows(&[&["1"], &[""], &["3"]]);
            case.skip_blank_lines = false;
            case
        },
        Case::csv("a byte-order mark is not data", "\u{FEFF}id,name\n1,a\n")
            .plan(&[(0, Door::I32), (1, Door::Text)])
            .header(&["id", "name"])
            .rows(&[&["1", "a"]]),
        Case::csv("no header", "1,a\n2,b\n")
            .no_header()
            .plan(&[(0, Door::I32), (1, Door::Text)])
            .rows(&[&["1", "a"], &["2", "b"]]),
        Case::csv("a header and nothing after it", "id,name\n")
            .plan(&[(0, Door::I32)])
            .header(&["id", "name"])
            .rows(&[]),
        Case::csv("a header with no line ending", "id,name")
            .plan(&[(0, Door::I32)])
            .header(&["id", "name"])
            .rows(&[]),
        Case::csv("no input at all", "")
            .plan(&[(0, Door::I32)])
            .header(&[])
            .rows(&[]),
        Case::csv(
            "a plan is a projection, in its own order",
            "a,b,c,d\n1,2,3,4\n",
        )
        .plan(&[
            (3, Door::I32),
            (0, Door::I32),
            (3, Door::Text),
            (9, Door::I32),
        ])
        .header(&["a", "b", "c", "d"])
        .rows(&[&["1", "2", "3", "4"]]),
        Case::csv(
            "the doors trim; text is the bytes as they are",
            "n,t\n 7 ,  padded  \n",
        )
        .plan(&[(0, Door::I32), (0, Door::Text), (1, Door::Text)])
        .header(&["n", "t"])
        .rows(&[&[" 7 ", "  padded  "]]),
        Case::csv(
            "a cell that does not cast says where",
            "a,b,c\n12x4,256,x\n -1,7,no\n",
        )
        .plan(&[
            (0, Door::I32),
            (1, Door::U8),
            (2, Door::Bool),
            (0, Door::U8),
        ])
        .header(&["a", "b", "c"])
        .rows(&[&["12x4", "256", "x"], &[" -1", "7", "no"]]),
        Case::csv(
            "an escaped cell is cast as its unescaped text",
            "a\n\"1\"\"2\"\n\"12\"\n",
        )
        .plan(&[(0, Door::I32), (0, Door::Text)])
        .header(&["a"])
        .rows(&[&["1\"2"], &["12"]]),
        {
            let mut case = Case::csv("with quoting off, a quote is a byte", "a,b\n\"x,y\"\n")
                .plan(&[(0, Door::Text), (1, Door::Text)])
                .header(&["a", "b"])
                .rows(&[&["\"x", "y\""]]);
            case.quoting = false;
            case
        },
        {
            let mut case = Case::csv("tab separated", "a\tb\n1\tx, y\n")
                .plan(&[(0, Door::I32), (1, Door::Text)])
                .header(&["a", "b"])
                .rows(&[&["1", "x, y"]]);
            case.separator = '\t';
            case
        },
        {
            let mut case = Case::csv("pipe separated", "a|b\n1,5|\"x|y\"\n")
                .plan(&[(0, Door::Text), (1, Door::Text)])
                .header(&["a", "b"])
                .rows(&[&["1,5", "x|y"]]);
            case.separator = '|';
            case
        },
        Case::csv("every door", &format!("{}\n", door_texts.join(",")))
            .no_header()
            .plan(&door_plan)
            .rows(&[&door_texts]),
        Case::csv(
            "every door, on nothing",
            &format!("{}\n", door_blanks.join(",")),
        )
        .no_header()
        .plan(&door_plan)
        .rows(&[&door_blanks]),
        {
            let mut case = Case::csv("a declared notation", "amount\n1.234,50\n(7)\n1,234.50\n")
                .column(0, Door::F64, eurozone)
                .column(0, Door::F64, NumFormat::INVARIANT)
                .column(0, Door::Decimal, eurozone)
                .header(&["amount"])
                .rows(&[&["1.234,50"], &["(7)"], &["1,234.50"]]);
            case.separator = ';';
            case
        },
        Case::csv(
            "a declared currency symbol",
            "price\n\"$1,234.50\"\n\"($5.00)\"\n9\n",
        )
        .column(0, Door::Decimal, dollars)
        .column(0, Door::Decimal, NumFormat::INVARIANT)
        .header(&["price"])
        .rows(&[&["$1,234.50"], &["($5.00)"], &["9"]]),
        Case::csv(
            "text beyond ASCII, and spans in bytes",
            "名前,n\n日本語 é 🙂,é12\n",
        )
        .plan(&[(0, Door::Text), (1, Door::I32), (1, Door::Text)])
        .header(&["名前", "n"])
        .rows(&[&["日本語 é 🙂", "é12"]]),
        Case::csv(
            "a record of the wrong width ends the input",
            "a,b\n1,2\n3\n4,5\n",
        )
        .plan(&[(0, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"]])
        .fails("column_count", 2, 3, 8)
        .width(2, 1),
        Case::csv(
            "a wider record is as wrong as a narrower one",
            "a,b\n1,2,3\n",
        )
        .plan(&[(0, Door::I32)])
        .header(&["a", "b"])
        .rows(&[])
        .fails("column_count", 1, 2, 4)
        .width(2, 3),
        Case::csv("a trailing separator is one more cell", "a,b\n1,2\n3,4,\n")
            .plan(&[(1, Door::I32)])
            .header(&["a", "b"])
            .rows(&[&["1", "2"]])
            .fails("column_count", 2, 3, 8)
            .width(2, 3),
        Case::csv("input that ends inside a quoted cell", "a\n1\n\"open\n")
            .plan(&[(0, Door::I32)])
            .header(&["a"])
            .rows(&[&["1"]])
            .fails("unclosed_quote", 2, 3, 4),
        Case::csv(
            "positions count the byte-order mark and the blank lines",
            "\u{FEFF}a,b\n\n1,2\n\n3\n",
        )
        .plan(&[(0, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"]])
        .fails("column_count", 4, 5, 13)
        .width(2, 1),
        Case::csv(
            "a quoted line break is not a row but is a line",
            "a,b\n\"x\ny\",1\n2\n",
        )
        .plan(&[(0, Door::Text), (1, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["x\ny", "1"]])
        .fails("column_count", 2, 4, 12)
        .width(2, 1),
    ]
}

pub fn corpus_json() -> String {
    let cases: Vec<Value> = cases()
        .iter()
        .map(|case| {
            let plan: Vec<Value> = case
                .plan
                .iter()
                .map(|(ordinal, door, format)| {
                    let mut entry = door_json(*door);
                    entry["ordinal"] = json!(ordinal);
                    if *format != NumFormat::INVARIANT {
                        entry["format"] = format_json(format);
                    }
                    entry
                })
                .collect();
            let rows: Vec<Value> = case
                .cells
                .iter()
                .map(|row| {
                    let cells: Vec<Value> = case
                        .plan
                        .iter()
                        .map(|(ordinal, door, format)| {
                            let text = row.get(*ordinal as usize).copied().unwrap_or("");
                            verdict(*door, format, text.as_bytes())
                        })
                        .collect();
                    json!(cells)
                })
                .collect();
            let mut value = json!({
                "name": case.name,
                "input": case.input,
                "dialect": {
                    "separator": case.separator.to_string(),
                    "quoting": case.quoting,
                    "skip_blank_lines": case.skip_blank_lines,
                    "has_header": case.has_header,
                },
                "plan": plan,
                "header": case.header,
                "rows": rows,
            });
            if let Some(failure) = &case.failure {
                let mut entry = json!({
                    "kind": failure.kind,
                    "record": failure.record,
                    "line": failure.line,
                    "byte": failure.byte,
                });
                if let Some((expected, found)) = failure.width {
                    entry["expected"] = json!(expected);
                    entry["found"] = json!(found);
                }
                value["failure"] = entry;
            }
            value
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&cases).expect("the corpus is JSON");
    text.push('\n');
    text
}

#[allow(dead_code)]
fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/delimited.json");
    std::fs::write(path, corpus_json()).expect("writing the corpus");
    println!("wrote {path}");
}
