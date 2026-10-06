//! Writes the two corpora every binding replays: `corpus/delimited.json` and
//! `corpus/workbook.json`.
//!
//!   cargo run --example corpus
//!
//! **Delimited.** A case says two things, by hand: the input, and the text that must
//! reach each cell — which rows there are, where each cell begins and ends, what quoting
//! leaves behind. That grid is the tabular layer's whole job and is never computed. What
//! each plan column then makes of its cell's text is not this layer's to decide: HyperCast
//! is the judge, so the verdict written for a cell is whatever HyperCast's own door says
//! of that text, asked directly, with no reader in between.
//!
//! **Workbook.** A case is one sheet of one of the packages in `corpus/workbook/`, read
//! with one set of options through one plan. Nothing here is by hand: the file was first
//! written by the std workbook reader this crate had before its core could read a
//! workbook — a reader that owed the core nothing — with the core required to agree
//! before it was written; that reader is gone, and the file is what is left of it. This
//! example now writes the file from the core, through the Rust binding, so running it
//! says whether the core still reads what that reader read: a diff in `workbook.json` is
//! a change in behaviour, to be explained before it is committed.
//!
//!   cargo run --example corpus -- --packages
//!
//! also rewrites the generated packages themselves (the fixtures beside them are
//! `make_fixtures.py`'s). Their deflated bytes come from whatever deflate the build
//! links, so they are committed rather than rebuilt, and rewritten only on purpose.
//!
//! `tests/corpus_delimited.rs` and `tests/corpus_workbook.rs` replay the files through the
//! readers, and fail if this example would write anything other than what is committed.

#[path = "../tests/support/corpus.rs"]
pub mod corpus;
#[path = "../tests/support/packages.rs"]
pub mod packages;

use corpus::*;
use hypercast::{DateOrder, ExcelEpoch, Fault, UnixPrecision};
use hypertabular::{Column, Door, Format, NumFormat, SheetOptions, Workbook};
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

/// Where the workbook packages are.
pub const WORKBOOK_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook");

/// Every door over each of `width` source columns, what a door declares varied by column.
fn every_door_over(width: usize) -> Vec<Column> {
    let mut plan = Vec::new();
    for ordinal in 0..width {
        for code in 1..=22u32 {
            let by = ordinal as u32;
            let param = match code {
                14 => 1 + by % 4,
                20 | 21 => 1 + by % 3,
                22 => 1 + by % 2,
                _ => 0,
            };
            let door = Door::from_code(code, param).expect("a door");
            plan.push(Column::new(ordinal, door));
        }
    }
    plan
}

/// The most rows a reading with empty rows delivered may have and still be a case.
const DELIVERED_ROWS: usize = 100;

/// How many leading columns of a package's sheets the plan covers, and one more it reads
/// as text beyond them. The generated packages are four wide and the library-written
/// fixtures within ten. A file a real application wrote uses the dataset in
/// `corpus/README.md`: seventeen columns of values, and one cell far to the right, in
/// column Z, which is there to make a row sparse — so the columns between are not read,
/// and that one is read once, not through every door.
fn columns_of(file: &str) -> (usize, Option<usize>) {
    const FIXTURES: [&str; 6] = [
        "basic.xlsx",
        "basic-1904.xlsx",
        "basic.ods",
        "multisheet.xlsx",
        "rich.xlsx",
        "broken.xlsx",
    ];
    if file.starts_with("generated") {
        (4, None)
    } else if FIXTURES.contains(&file) {
        (10, None)
    } else {
        (17, Some(25))
    }
}

/// The workbook cases: every sheet of every package in `corpus/workbook/`, each way of
/// reading it.
pub fn workbook_cases() -> Vec<Value> {
    let mut files: Vec<String> = std::fs::read_dir(WORKBOOK_DIR)
        .expect("corpus/workbook")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .into_string()
                .expect("a name")
        })
        .filter(|name| {
            [".xlsx", ".xlsm", ".ods"]
                .iter()
                .any(|ext| name.ends_with(ext))
        })
        .collect();
    files.sort();
    let mut cases = Vec::new();
    for file in &files {
        // A package the core refuses to open — an encrypted one, say — is a case of its
        // own: the file, and the failure every way of opening it must give.
        let book = match Workbook::open(format!("{WORKBOOK_DIR}/{file}")) {
            Ok(book) => book,
            Err(error) => {
                cases.push(json!({
                    "name": format!("{file}, refused"),
                    "file": format!("workbook/{file}"),
                    "failure": failure_json(&error),
                }));
                continue;
            }
        };
        let (width, beyond) = columns_of(file);
        let sheets: Vec<Value> = book
            .sheets()
            .iter()
            .map(|sheet| json!({"name": sheet.name, "hidden": sheet.hidden}))
            .collect();
        let base = SheetOptions::default();
        for (index, info) in book.sheets().iter().enumerate() {
            for (at, options) in [
                base,
                base.with_header(false),
                base.with_empty_rows_skipped(false),
                base.with_header(false).with_empty_rows_skipped(false),
            ]
            .into_iter()
            .enumerate()
            {
                // Every door over every column once; the other options change which rows
                // there are, which the text door shows.
                let mut plan: Vec<Column> = if at == 0 {
                    every_door_over(width)
                } else {
                    (0..width).map(Column::text).collect()
                };
                plan.extend(beyond.map(Column::text));
                let mut sheet = book.sheet(index, options, &plan).expect("a sheet");
                let header = header_json(sheet.header());
                let (mut numbers, mut rows) = (Vec::new(), Vec::new());
                let mut padded = false;
                let failure = loop {
                    match sheet.read() {
                        Ok(Some(batch)) => {
                            numbers.extend((0..batch.rows()).map(|row| batch.line(row)));
                            rows_json(&batch, &mut rows);
                            // A sheet LibreOffice wrote ends in a million empty rows, as
                            // one element. Delivered, they would be most of this file and
                            // say nothing a few do not: such a reading is not a case.
                            if !options.skip_empty_rows && numbers.len() > DELIVERED_ROWS {
                                padded = true;
                                break None;
                            }
                        }
                        Ok(None) => break None,
                        Err(error) => break Some(error),
                    }
                };
                if padded {
                    continue;
                }
                let plan: Vec<Value> = plan
                    .iter()
                    .map(|column| {
                        let mut entry = door_json(column.door);
                        entry["ordinal"] = json!(column.ordinal);
                        entry
                    })
                    .collect();
                let mut case = json!({
                    "name": format!(
                        "{file}, sheet {index} ({:?}), {}, {}",
                        info.name,
                        if options.has_header { "a header" } else { "no header" },
                        if options.skip_empty_rows {
                            "empty rows skipped"
                        } else {
                            "empty rows delivered"
                        },
                    ),
                    "file": format!("workbook/{file}"),
                    "format": match book.format() {
                        Format::Xlsx => "xlsx",
                        Format::Ods => "ods",
                    },
                    "epoch": book.date_system() as u32,
                    "sheets": sheets,
                    "sheet": index,
                    "options": {
                        "has_header": options.has_header,
                        "skip_empty_rows": options.skip_empty_rows,
                    },
                    "plan": plan,
                    "header": header,
                    "numbers": numbers,
                    "rows": rows,
                });
                if let Some(error) = failure {
                    case["failure"] = failure_json(&error);
                    if file == "broken.xlsx" {
                        case["note"] = json!(
                            "the rows are the oracle's; the failure's fields are the core's own"
                        );
                    }
                }
                cases.push(case);
            }
        }
    }
    cases
}

/// `corpus/workbook.json`, as it should be.
pub fn workbook_json() -> String {
    workbook_text(&workbook_cases())
}

#[allow(dead_code)]
fn main() {
    if std::env::args().any(|argument| argument == "--packages") {
        for (name, bytes) in packages::corpus_packages() {
            let path = format!("{WORKBOOK_DIR}/{name}");
            std::fs::write(&path, bytes).expect("writing a package");
            println!("wrote {path}");
        }
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/delimited.json");
    std::fs::write(path, corpus_json()).expect("writing the corpus");
    println!("wrote {path}");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook.json");
    std::fs::write(path, workbook_json()).expect("writing the corpus");
    println!("wrote {path}");
}
