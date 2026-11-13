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
//! of that text, asked directly, with no reader in between. A case also says by hand which
//! cells are its tokens; a cell its row doesn't have is padding, and neither goes to a door.
//! A case's `problems` follow from those verdicts, with the line each row starts on and each long
//! row's widths said by hand (`ROW_LINES`, `LONG_ROWS`), so no reader's list has a say in them.
//!
//! **Workbook.** A case is one sheet of one of the packages in `corpus/workbook/`, read
//! with one set of options through one plan. Nothing here is by hand: the file was first
//! written by the std workbook reader this crate had before its core could read a
//! workbook — a reader that owed the core nothing — with the core required to agree
//! before it was written; that reader is gone, and the file is what is left of it. This
//! example now writes the file from the core, through the Rust binding, so running it
//! says whether the core still reads what that reader read: a diff in `workbook.json` is
//! a change in behaviour, to be explained before it is committed. A case's `problems` are worked
//! out from its own `rows` and `numbers`, not from the core's list, which has to agree.
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
use hypercast::{DateOptions, DateOrder, ExcelEpoch, MonthNames, UnixPrecision, ValidDateOptions};
use hypertabular::{Column, Door, Format, Grouping, NumFormat, Range, SheetOptions, Workbook};
use serde_json::{Value, json};

/// `NumFormat::INVARIANT`'s separators and leniences, declaring Indian grouping.
const INDIAN: NumFormat = NumFormat::INVARIANT.with_grouping(Grouping::Indian);

/// Where each row of a case starts, for every case with a cell in its `problems`: by hand.
const ROW_LINES: &[(&str, &[u32])] = &[
    (
        "constraints: a violation keeps the value as cast",
        &[2, 3, 4],
    ),
    (
        "constraints: every numeric keyword over the integers",
        &[2, 3, 4],
    ),
    (
        "constraints: every numeric keyword over the floats and the decimal",
        &[2, 3, 4],
    ),
    ("constraints: multipleOf rounds, or refuses", &[2, 3, 4]),
    ("constraints: a rounding past the type is multipleOf", &[2]),
    ("constraints: an empty enum matches nothing", &[2]),
    (
        "constraints: a numeric enum matches the value, not the text",
        &[2, 3, 4],
    ),
    ("constraints: a trimmed text spans the judged text", &[2]),
    ("constraints: a quoted cell is judged unescaped", &[2]),
    ("constraints: tokens and padding stay empty", &[2, 3]),
    ("constraints: a number in a text enum is text", &[2, 3]),
    (
        "constraints: an enum folds case unless it is case sensitive",
        &[2, 3],
    ),
    (
        "constraints: of duplicate enum entries the first matches",
        &[2, 3],
    ),
    (
        "constraints: text that isn't UTF-8 is the door's fault",
        &[2, 3],
    ),
    ("a cell that does not cast says where", &[2, 3]),
    ("an escaped cell is cast as its unescaped text", &[2, 3]),
    ("a declared notation", &[2, 3, 4]),
    ("a declared currency symbol", &[2, 3, 4]),
    ("text beyond ASCII, and spans in bytes", &[2]),
    (
        "text that isn't UTF-8 is malformed, and the rows go on",
        &[1, 2],
    ),
    ("a sequence cut off by its cell's end or its line's", &[1]),
    ("a sequence cut off by the end of the input", &[1]),
    (
        "an encoded surrogate, an overlong form and a code point past U+10FFFF",
        &[1, 2, 3],
    ),
    (
        "the span of invalid text counts in the unescaped cell",
        &[1],
    ),
    ("the same bytes through the numeric and date doors", &[1]),
    ("long rows are reported", &[1, 2]),
    (
        "what went wrong is listed, and nothing else",
        &[4, 6, 7, 8, 9],
    ),
    ("a token comes before any door", &[2, 3, 4]),
    ("a token is matched on bytes no door would take", &[1, 2]),
    ("a verdict says what failed", &[2, 3, 4, 5]),
    ("date options reach the door", &[2, 3]),
    ("a pivot and month names reach the door", &[2, 3]),
    (
        "Western grouping refuses a last group that isn't three digits",
        &[2, 3],
    ),
    ("Western grouping refuses an Indian group", &[2, 3]),
    ("no format is Western grouping", &[2, 3]),
    ("an Indian column beside a Western one", &[2, 3]),
    ("no format beside an Indian column", &[2, 3]),
    (
        "neither grouping reads these shapes",
        &[2, 3, 4, 5, 6, 7, 8, 9],
    ),
    ("one policy reads each of these shapes", &[2, 3, 4]),
    ("space groups: neither reads a misplaced one", &[2, 3, 4]),
    ("a fault's span is the decoded text's", &[2]),
    (
        "suggest: a cell that isn't text leaves its column unresolved",
        &[2, 3],
    ),
];

/// A long row a case reports: `(row, expected, found)`.
type LongRow = (usize, u32, u32);

/// The long rows a case reports: by hand.
const LONG_ROWS: &[(&str, &[LongRow])] = &[
    ("long rows are reported", &[(1, 1, 2)]),
    (
        "what went wrong is listed, and nothing else",
        &[(3, 3, 4), (4, 3, 5)],
    ),
];

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
    input: Vec<u8>,
    separator: char,
    quoting: bool,
    skip_blank_lines: bool,
    has_header: bool,
    plan: Vec<(u32, Door, NumFormat)>,
    /// The header's names, when the dialect declares one.
    header: Option<Vec<&'static str>>,
    /// The text of every cell of every row that is delivered, in file order.
    cells: Vec<Vec<&'static [u8]>>,
    failure: Option<Failure>,
    /// Whether the plan says its formats even where they're the invariant one.
    states_format: bool,
    /// Physical lines to pass over.
    skip: u32,
    /// Records to discard after them.
    header_row: u32,
    /// `pad`, or the default.
    short_rows: Option<&'static str>,
    /// `report` or `truncate`, or the default.
    long_rows: Option<&'static str>,
    /// The dialect's tokens.
    tokens: Option<&'static [&'static str]>,
    /// The encoding the dialect declares; `input` is then the original bytes, and everything
    /// the case says of its cells is about the decoded text.
    encoding: Option<&'static str>,
    /// Plan columns' own tokens, by place in the plan.
    column_tokens: Vec<(usize, &'static [&'static str])>,
    /// Plan columns' date options, by place in the plan.
    column_dates: Vec<(usize, ColumnDate)>,
    /// The `(row, plan column)` cells that are tokens: by hand, as the rest of the grid is.
    token_cells: Vec<(usize, usize)>,
    /// Plan columns' constraints, by place in the plan, in HyperCast's corpus shape.
    column_constraints: Vec<(usize, Value)>,
    /// What `hypertabular_suggest` says of the input: by hand, worked out from the rule in
    /// `docs/design.md` and nothing that ran.
    suggest: Option<Suggest>,
}

/// One column as the suggester has to see it: door, `param`, status, matched, sampled.
type Suggested = (&'static str, u32, &'static str, u32, u32);

/// A case's `suggest`.
#[derive(Clone, Copy)]
struct Suggest {
    sample_rows: Option<u32>,
    format: Option<NumFormat>,
    truncated: bool,
    columns: &'static [Suggested],
}

impl Suggest {
    fn of(columns: &'static [Suggested]) -> Suggest {
        Suggest {
            sample_rows: None,
            format: None,
            truncated: false,
            columns,
        }
    }

    fn json(&self) -> Value {
        let columns: Vec<Value> = self
            .columns
            .iter()
            .map(|(door, param, status, matched, sampled)| {
                json!({
                    "door": door,
                    "param": param,
                    "status": status,
                    "matched": matched,
                    "sampled": sampled,
                })
            })
            .collect();
        let mut value = json!({"truncated": self.truncated, "columns": columns});
        if let Some(rows) = self.sample_rows {
            value["sample_rows"] = json!(rows);
        }
        if let Some(format) = &self.format {
            value["format"] = format_json(format);
        }
        value
    }
}

/// What a plan column declares to the date doors.
#[derive(Clone, Copy)]
struct ColumnDate {
    flags: &'static [&'static str],
    pivot: u16,
    months: Option<&'static [(u8, &'static str)]>,
}

impl ColumnDate {
    fn json(&self) -> Value {
        let mut value = json!({"flags": self.flags});
        if self.pivot != 0 {
            value["pivot"] = json!(self.pivot);
        }
        if let Some(months) = self.months {
            value["months"] = json!(months);
        }
        value
    }

    /// The door's options, judged by HyperCast.
    fn valid(&self) -> ValidDateOptions<'static> {
        let flags = self
            .flags
            .iter()
            .fold(0, |flags, name| flags | date_flag(name));
        DateOptions::from_flags(flags)
            .with_pivot(self.pivot)
            .validate(self.months.map(MonthNames::from_entries))
            .expect("the fixture declares options HyperCast takes")
    }
}

impl Case {
    fn csv(name: &'static str, input: &str) -> Case {
        Case::bytes(name, input.as_bytes())
    }

    /// A case over bytes that needn't be text: `corpus_json` writes them as `input` if they
    /// are UTF-8 and no encoding is declared, and as `input_hex` otherwise.
    fn bytes(name: &'static str, input: &[u8]) -> Case {
        Case {
            name,
            input: input.to_vec(),
            separator: ',',
            quoting: true,
            skip_blank_lines: true,
            has_header: true,
            plan: Vec::new(),
            header: None,
            cells: Vec::new(),
            failure: None,
            skip: 0,
            header_row: 0,
            short_rows: None,
            long_rows: None,
            tokens: None,
            encoding: None,
            column_tokens: Vec::new(),
            column_dates: Vec::new(),
            token_cells: Vec::new(),
            column_constraints: Vec::new(),
            suggest: None,
            states_format: false,
        }
    }

    /// A plan that reads every cell's text, which is what a sample is.
    fn all_text(self, width: u32) -> Case {
        let plan: Vec<(u32, Door)> = (0..width).map(|ordinal| (ordinal, Door::Text)).collect();
        self.plan(&plan)
    }

    fn suggest(mut self, suggest: Suggest) -> Case {
        self.suggest = Some(suggest);
        self
    }

    fn skip(mut self, lines: u32) -> Case {
        self.skip = lines;
        self
    }

    fn header_row(mut self, records: u32) -> Case {
        self.header_row = records;
        self
    }

    fn short_rows(mut self, policy: &'static str) -> Case {
        self.short_rows = Some(policy);
        self
    }

    fn long_rows(mut self, policy: &'static str) -> Case {
        self.long_rows = Some(policy);
        self
    }

    fn encoding(mut self, encoding: &'static str) -> Case {
        self.encoding = Some(encoding);
        self
    }

    fn tokens(mut self, tokens: &'static [&'static str]) -> Case {
        self.tokens = Some(tokens);
        self
    }

    fn column_tokens(mut self, column: usize, tokens: &'static [&'static str]) -> Case {
        self.column_tokens.push((column, tokens));
        self
    }

    fn column_date(
        mut self,
        column: usize,
        flags: &'static [&'static str],
        pivot: u16,
        months: Option<&'static [(u8, &'static str)]>,
    ) -> Case {
        self.column_dates.push((
            column,
            ColumnDate {
                flags,
                pivot,
                months,
            },
        ));
        self
    }

    /// Plan column `column` declaring these constraints, HyperCast's corpus shape as JSON text.
    fn constrain(mut self, column: usize, constraints: &str) -> Case {
        let declared = serde_json::from_str(constraints).expect("a declaration is JSON");
        self.column_constraints.push((column, declared));
        self
    }

    /// The `(row, plan column)` cells that are the declared tokens.
    fn token_cells(mut self, cells: &[(usize, usize)]) -> Case {
        self.token_cells = cells.to_vec();
        self
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

    /// Writes the plan's formats even where they're the invariant one, which a case otherwise leaves
    /// out, for a binding to have the flags to declare.
    fn states_format(mut self) -> Case {
        self.states_format = true;
        self
    }

    fn header(mut self, names: &[&'static str]) -> Case {
        self.header = Some(names.to_vec());
        self
    }

    fn with_blank_lines(mut self) -> Case {
        self.skip_blank_lines = false;
        self
    }

    fn no_quoting(mut self) -> Case {
        self.quoting = false;
        self
    }

    fn no_header(mut self) -> Case {
        self.has_header = false;
        self
    }

    fn rows(mut self, cells: &[&[&'static str]]) -> Case {
        self.cells = cells
            .iter()
            .map(|row| row.iter().map(|cell| cell.as_bytes()).collect())
            .collect();
        self
    }

    fn byte_rows(mut self, cells: &[&[&'static [u8]]]) -> Case {
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
fn verdict(
    door: Door,
    format: &NumFormat,
    date: Option<&ValidDateOptions<'_>>,
    text: &[u8],
) -> Value {
    use hypercast::detailed;
    fn judged<T>(
        verdict: Result<T, hypercast::Failure>,
        text: &[u8],
        ok: impl Fn(T) -> Value,
    ) -> Value {
        match verdict {
            Ok(value) => ok(value),
            Err(failure) => {
                let mut cell = fault_json(failure.reason, failure.offset, failure.len, text);
                cell["detail"] = json!(failure.detail.name());
                cell
            }
        }
    }
    let format = &format
        .validate()
        .expect("the fixture declares a format HyperCast takes");
    let number = |value: Value| json!({"expect": "ok", "value": value});
    match door {
        Door::Bool => judged(detailed::cast_bool(text), text, |v| number(json!(v))),
        Door::I8 => judged(detailed::cast_i8(text, format), text, |v| number(json!(v))),
        Door::I16 => judged(detailed::cast_i16(text, format), text, |v| number(json!(v))),
        Door::I32 => judged(detailed::cast_i32(text, format), text, |v| number(json!(v))),
        Door::I64 => judged(detailed::cast_i64(text, format), text, |v| number(json!(v))),
        Door::U8 => judged(detailed::cast_u8(text, format), text, |v| number(json!(v))),
        Door::U16 => judged(detailed::cast_u16(text, format), text, |v| number(json!(v))),
        Door::U32 => judged(detailed::cast_u32(text, format), text, |v| number(json!(v))),
        Door::U64 => judged(detailed::cast_u64(text, format), text, |v| number(json!(v))),
        Door::F32 => judged(detailed::cast_f32(text, format), text, |v| {
            number(json!(f64::from(v)))
        }),
        Door::F64 => judged(detailed::cast_f64(text, format), text, |v| number(json!(v))),
        Door::Decimal => judged(detailed::cast_decimal(text, format), text, decimal_json),
        Door::Uuid => judged(detailed::cast_uuid(text), text, uuid_json),
        Door::Timestamp => judged(detailed::cast_timestamp(text, date), text, timestamp_json),
        Door::Unix(precision) => judged(detailed::cast_unix(text, precision), text, timestamp_json),
        Door::ExcelSerial(epoch) => judged(
            detailed::cast_excel_serial(text, epoch),
            text,
            timestamp_json,
        ),
        Door::Date => judged(detailed::cast_date(text, date), text, date_json),
        Door::DateOrdered(order) => judged(
            detailed::cast_date_ordered(text, order, date),
            text,
            date_json,
        ),
        Door::DateTime(order) => judged(
            detailed::cast_datetime(text, order, date),
            text,
            datetime_json,
        ),
        Door::Time => judged(
            detailed::cast_time(text),
            text,
            |v| json!({"expect": "ok", "nanos": v}),
        ),
        Door::Duration => judged(detailed::cast_duration(text), text, duration_json),
        Door::Text => judged(detailed::cast_text(text), text, |v| text_json(v.as_bytes())),
        _ => unreachable!("the fixture declares a known door"),
    }
}

/// HyperCast's checked verdict on one cell's text, under the column's declaration: the door's
/// own fault, a value that passed, or one that broke a declared constraint.
fn checked_verdict(door: Door, format: &NumFormat, declared: &Value, text: &[u8]) -> Value {
    use hypercast::detailed::{self, Rejection};
    use hypercast::{Checked, Constraints, Text};
    fn shaped<T>(
        judged: Result<Checked<T>, Rejection<T>>,
        door: Door,
        text: &[u8],
        ok: impl Fn(T) -> Value,
    ) -> Value {
        match judged {
            Ok(passed) => judged_json(
                door,
                ok(passed.value),
                Ok((passed.adjusted, passed.enum_index)),
                text,
            ),
            Err(Rejection::Violation(broken)) => judged_json(
                door,
                ok(broken.value),
                Err((broken.keyword, broken.offset, broken.len)),
                text,
            ),
            Err(Rejection::Failure(failure)) => {
                let mut cell = fault_json(failure.reason, failure.offset, failure.len, text);
                cell["detail"] = json!(failure.detail.name());
                cell
            }
        }
    }
    let (raw, packed) = declaration_of(door, declared);
    let format = &format
        .validate()
        .expect("the fixture declares a format HyperCast takes");
    let number = |value: Value| json!({"expect": "ok", "value": value});
    macro_rules! numeric {
        ($ty:ty, $ok:expr) => {{
            let constraints =
                Constraints::<$ty>::from_raw(raw, &packed).expect("a declaration for its door");
            shaped(
                detailed::cast_checked(text, format, &constraints),
                door,
                text,
                $ok,
            )
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
        Door::Decimal => numeric!(hypercast::Decimal, decimal_json),
        Door::Text => {
            let constraints =
                Constraints::<Text>::from_raw(raw, &packed).expect("a declaration for its door");
            shaped(
                detailed::cast_text_checked(text, &constraints),
                door,
                text,
                |v: &str| text_json(v.as_bytes()),
            )
        }
        other => panic!("the {other:?} door takes no constraints"),
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
        // Without a header, whose width would refuse ordinal 9 at bind: past the record's
        // last cell, the column is empty.
        Case::csv("a plan is a projection, in its own order", "1,2,3,4\n")
            .no_header()
            .plan(&[
                (3, Door::I32),
                (0, Door::I32),
                (3, Door::Text),
                (9, Door::I32),
            ])
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
        // Quotes decide boundaries only, and only a cell's first byte opens one.
        Case::csv(
            "stray quotes after the close keep the quoted cell whole",
            "\"x\"y\"z\"\n",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .rows(&[&["x\"y\"z"]]),
        Case::csv(
            "a stray quote after the close can't hide a separator",
            "\"x\"y\"z,w\"\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .rows(&[&["x\"y\"z", "w\""]]),
        Case::csv(
            "a doubled quote after the close is unescaped like any other",
            "\"x\"y\"\"z\"\n",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .rows(&[&["x\"y\"z"]]),
        Case::csv(
            "a quote that isn't a cell's first byte can't merge records",
            "12\" pipe,1\n3,4\n6\" pipe,2\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .rows(&[&["12\" pipe", "1"], &["3", "4"], &["6\" pipe", "2"]]),
        Case::csv(
            "a record after a stray quote is still a record, wrong width and all",
            "12\" pipe,1\n3,4\n6\" pipe\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .rows(&[&["12\" pipe", "1"], &["3", "4"]])
        .fails("column_count", 2, 3, 15)
        .width(2, 1),
        Case::csv(
            "one stray quote at the end of the input is not an open quote",
            "12\" pipe,1\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .rows(&[&["12\" pipe", "1"]]),
        Case::csv(
            "a quote at a cell's first byte still opens after a stray one",
            "\"x\"y,\"a,b\nc\"\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .rows(&[&["x\"y", "a,b\nc"]]),
        Case::csv(
            "a stray quote then a quote at a cell's first byte, never closed",
            "a\"\n\"b\n",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .rows(&[&["a\""]])
        .fails("unclosed_quote", 1, 2, 3),
        Case::bytes(
            "text that isn't UTF-8 is malformed, and the rows go on",
            b"\xe9t\xe9,x\ncaf\xc3\xa9,y\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .byte_rows(&[&[b"\xe9t\xe9", b"x"], &[b"caf\xc3\xa9", b"y"]]),
        Case::bytes(
            "a sequence cut off by its cell's end or its line's",
            b"ab\xc3,\xe2\x82\n",
        )
        .no_header()
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .byte_rows(&[&[b"ab\xc3", b"\xe2\x82"]]),
        Case::bytes(
            "a sequence cut off by the end of the input",
            b"abc\xf0\x9f\x98",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .byte_rows(&[&[b"abc\xf0\x9f\x98"]]),
        Case::bytes(
            "an encoded surrogate, an overlong form and a code point past U+10FFFF",
            b"\xed\xa0\x80\n\xc0\x80\n\xf4\x90\x80\x80\n",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .byte_rows(&[&[b"\xed\xa0\x80"], &[b"\xc0\x80"], &[b"\xf4\x90\x80\x80"]]),
        Case::bytes(
            "the span of invalid text counts in the unescaped cell",
            b"\"a\"\"\xff\"\n",
        )
        .no_header()
        .plan(&[(0, Door::Text)])
        .byte_rows(&[&[b"a\"\xff"]]),
        Case::bytes(
            "the same bytes through the numeric and date doors",
            b"\xff,1\xff\n",
        )
        .no_header()
        .plan(&[(0, Door::I32), (1, Door::Date)])
        .byte_rows(&[&[b"\xff", b"1\xff"]]),
        Case::bytes("a cell no plan column reads isn't judged", b"ok,\xff\n")
            .no_header()
            .plan(&[(0, Door::Text)])
            .byte_rows(&[&[b"ok", b"\xff"]]),
        Case::csv("two, three and four bytes are text", "é,€,😀\n")
            .no_header()
            .plan(&[(0, Door::Text), (1, Door::Text), (2, Door::Text)])
            .rows(&[&["é", "€", "😀"]]),
        Case::csv("an embedded NUL is text", "a\0b\n")
            .no_header()
            .plan(&[(0, Door::Text)])
            .rows(&[&["a\0b"]]),
        Case::bytes(
            "a header name that isn't UTF-8 ends the input",
            b"\xe9,b\n1,2\n",
        )
        .plan(&[(0, Door::Text)])
        .fails("encoding", 0, 1, 0),
        Case::bytes(
            "the header's name is judged unescaped",
            b"a,\"x\"\"\xff\"\n1,2\n",
        )
        .plan(&[(0, Door::Text)])
        .fails("encoding", 0, 1, 0),
        Case::bytes(
            "a failing header's position counts the mark and the blank lines",
            b"\xef\xbb\xbf\n\xe9\n",
        )
        .plan(&[(0, Door::Text)])
        .fails("encoding", 1, 2, 4),
        // ---- Options: what a read passes over, what a width policy does, tokens, date options. ----
        Case::csv("skip counts physical lines", "t\"itle\na,b\n1,2\n")
            .skip(1)
            .plan(&[(0, Door::I32), (1, Door::Text)])
            .header(&["a", "b"])
            .rows(&[&["1", "2"]]),
        Case::csv(
            "skip counts a line break in quotes",
            "\"a\n\r\nb\"\rc,d\n1,2\n",
        )
        .skip(3)
        .plan(&[(1, Door::I32)])
        .header(&["c", "d"])
        .rows(&[&["1", "2"]]),
        Case::csv("skip past the end reads nothing", "a,b\n1,2\n")
            .skip(9)
            .plan(&[(0, Door::I32)])
            .header(&[])
            .rows(&[]),
        Case::csv(
            "skip past the end reads nothing, with no header",
            "a,b\n1,2",
        )
        .skip(9)
        .no_header()
        .plan(&[(0, Door::I32)])
        .rows(&[]),
        Case::csv(
            "a byte-order mark does not count as a line",
            "\u{FEFF}title\na\n1\n",
        )
        .skip(1)
        .plan(&[(0, Door::I32)])
        .header(&["a"])
        .rows(&[&["1"]]),
        Case::csv("header_row discards records", "x,y\na,b\n1,2\n")
            .header_row(1)
            .plan(&[(0, Door::I32), (1, Door::I32)])
            .header(&["a", "b"])
            .rows(&[&["1", "2"]]),
        Case::csv(
            "header_row counts records, after what is skipped, and not blank lines it skips",
            "t\"\n\n\"x\ny\",z,w\n\na,b\n1,2\n",
        )
        .skip(1)
        .header_row(1)
        .plan(&[(1, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"]]),
        Case::csv(
            "header_row with no header discards the same",
            "x,y,z\n1,2\n",
        )
        .header_row(1)
        .no_header()
        .plan(&[(1, Door::I32)])
        .rows(&[&["1", "2"]]),
        Case::csv("header_row past the end reads nothing", "a,b\n1,2\n")
            .header_row(9)
            .plan(&[(0, Door::I32)])
            .header(&[])
            .rows(&[]),
        Case::csv(
            "a failure's position counts what was discarded, not what was skipped",
            "title\nx\na,b\n1,2\n3\n",
        )
        .skip(1)
        .header_row(1)
        .plan(&[(0, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"]])
        .fails("column_count", 3, 5, 16)
        .width(2, 1),
        Case::csv("short rows are padded", "1,2\n3\n,\n")
            .no_header()
            .short_rows("pad")
            .plan(&[(0, Door::I32), (1, Door::I32), (1, Door::Text)])
            .rows(&[&["1", "2"], &["3"], &["", ""]]),
        Case::csv("short rows are refused by default", "1,2\n3\n")
            .no_header()
            .short_rows("reject")
            .plan(&[(0, Door::I32)])
            .rows(&[&["1", "2"]])
            .fails("column_count", 1, 2, 4)
            .width(2, 1),
        Case::csv("long rows are refused by default", "1\n2,3\n")
            .no_header()
            .long_rows("reject")
            .plan(&[(0, Door::I32)])
            .rows(&[&["1"]])
            .fails("column_count", 1, 2, 2)
            .width(1, 2),
        Case::csv("long rows are reported", "1\n2,3\n")
            .no_header()
            .long_rows("report")
            .plan(&[(0, Door::I32), (1, Door::I32)])
            .rows(&[&["1"], &["2"]]),
        Case::csv(
            "what went wrong is listed, and nothing else",
            "title\nskipped,record\nid,n,note\n1,NA,\"two\nlines\"\nx,300,y\n7\n8,9,z,extra\n12a,1,ok,more,cells\n",
        )
        .skip(1)
        .header_row(1)
        .short_rows("pad")
        .long_rows("report")
        .tokens(&["NA"])
        .plan(&[(1, Door::U8), (0, Door::I32), (2, Door::Text)])
        .header(&["id", "n", "note"])
        .rows(&[
            &["1", "NA", "two\nlines"],
            &["x", "300", "y"],
            &["7"],
            &["8", "9", "z"],
            &["12a", "1", "ok"],
        ])
        .token_cells(&[(0, 0)]),
        Case::bytes(
            "long rows are cut, and what is cut is nothing",
            b"1\n2,\"\xff\"\"\"\n",
        )
        .no_header()
        .long_rows("truncate")
        .plan(&[(0, Door::I32), (1, Door::Text)])
        .rows(&[&["1"], &["2"]]),
        Case::csv("tokens replace and inherit", "a,b,c\nNA,NA,NA\n-,-,-\n")
            .tokens(&["NA"])
            .plan(&[(0, Door::Text), (1, Door::Text), (2, Door::Text)])
            .column_tokens(1, &[])
            .column_tokens(2, &["-"])
            .header(&["a", "b", "c"])
            .rows(&[&["NA", "NA", "NA"], &["-", "-", "-"]])
            .token_cells(&[(0, 0), (1, 2)]),
        Case::csv("a token comes before any door", "n\nNA\n7\nx\n")
            .tokens(&["NA"])
            .plan(&[(0, Door::I32)])
            .header(&["n"])
            .rows(&[&["NA"], &["7"], &["x"]])
            .token_cells(&[(0, 0)]),
        Case::csv("a token is exact bytes", "n\nN/A\nna\n na\nNA\n")
            .tokens(&["N/A", "na"])
            .plan(&[(0, Door::Text)])
            .header(&["n"])
            .rows(&[&["N/A"], &["na"], &[" na"], &["NA"]])
            .token_cells(&[(0, 0), (1, 0)]),
        Case::csv("tokens meet blanks and padding", "a,\nb\n ,c\n")
            .no_header()
            .short_rows("pad")
            .tokens(&[""])
            .plan(&[(0, Door::Text), (1, Door::Text)])
            .rows(&[&["a", ""], &["b"], &[" ", "c"]])
            .token_cells(&[(0, 1)]),
        Case::csv(
            "a quoted token is text",
            "n\n\"NA\"\nNA\n\"N\"\"A\"\nN\"A\n\"\"\n\n",
        )
        .with_blank_lines()
        .tokens(&["NA", "", "N\"A"])
        .plan(&[(0, Door::Text)])
        .header(&["n"])
        .rows(&[&["NA"], &["NA"], &["N\"A"], &["N\"A"], &[""], &[""]])
        .token_cells(&[(1, 0), (3, 0), (5, 0)]),
        Case::csv("with quoting off, a quote is part of a token", "\"NA\"\n")
            .no_header()
            .no_quoting()
            .tokens(&["\"NA\""])
            .plan(&[(0, Door::Text)])
            .rows(&[&["\"NA\""]])
            .token_cells(&[(0, 0)]),
        Case::bytes(
            "a token is matched on bytes no door would take",
            b"\xff\nx\n",
        )
        .no_header()
        .tokens(&["x"])
        .plan(&[(0, Door::Text)])
        .byte_rows(&[&[b"\xff"], &[b"x"]])
        .token_cells(&[(1, 0)]),
        Case::bytes(
            "bad bytes in what is passed over are nothing",
            b"\xff\n\xfe,\xfd\na\n1\n",
        )
        .skip(1)
        .header_row(1)
        .plan(&[(0, Door::Text)])
        .header(&["a"])
        .rows(&[&["1"]]),
        Case::csv(
            "a verdict says what failed",
            "n\n12a\n1 2\n99999999999\n\"\"\n",
        )
        .plan(&[(0, Door::I32)])
        .header(&["n"])
        .rows(&[&["12a"], &["1 2"], &["99999999999"], &[""]]),
        Case::csv("date options reach the door", "d\n20260107\n2026-01-07\n")
            .plan(&[(0, Door::Date), (0, Door::Date)])
            .column_date(0, &["compact_date"], 0, None)
            .header(&["d"])
            .rows(&[&["20260107"], &["2026-01-07"]]),
        Case::csv(
            "a pivot and month names reach the door",
            "d\n07-Jan-26\n07-Foo-26\n",
        )
        .plan(&[(0, Door::DateOrdered(DateOrder::DayMonthYear))])
        .column_date(
            0,
            &[],
            2029,
            Some(&[
                (1, "Jan"),
                (2, "Feb"),
                (3, "Mar"),
                (4, "Apr"),
                (5, "May"),
                (6, "Jun"),
                (7, "Jul"),
                (8, "Aug"),
                (9, "Sep"),
                (10, "Oct"),
                (11, "Nov"),
                (12, "Dec"),
            ]),
        )
        .header(&["d"])
        .rows(&[&["07-Jan-26"], &["07-Foo-26"]]),
        // No policy reads a shape into a wrong value: a last group that isn't three digits is refused.
        Case::csv(
            "Western grouping refuses a last group that isn't three digits",
            "n\n\"12,34\"\n\"1,234\"\n",
        )
        .column(0, Door::I32, NumFormat::INVARIANT)
        .states_format()
        .header(&["n"])
        .rows(&[&["12,34"], &["1,234"]]),
        // What the two policies differ on. The same bytes under each, and under no format at all,
        // which means Western: an Indian shape never reads without a declared culture.
        Case::csv(
            "Western grouping refuses an Indian group",
            "n\n\"1,23,456\"\n\"1,234\"\n",
        )
        .column(0, Door::I32, NumFormat::INVARIANT)
        .states_format()
        .header(&["n"])
        .rows(&[&["1,23,456"], &["1,234"]]),
        Case::csv(
            "no format is Western grouping",
            "n\n\"1,23,456\"\n\"1,234\"\n",
        )
        .column(0, Door::I32, NumFormat::INVARIANT)
        .header(&["n"])
        .rows(&[&["1,23,456"], &["1,234"]]),
        Case::csv(
            "Indian grouping reads an Indian group",
            "n\n\"1,23,456\"\n\"1,234\"\n",
        )
        .column(0, Door::I32, INDIAN)
        .header(&["n"])
        .rows(&[&["1,23,456"], &["1,234"]]),
        // A policy is a column's own: one read, the two policies, each refusing what the other reads.
        Case::csv(
            "an Indian column beside a Western one",
            "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n",
        )
        .column(0, Door::I32, INDIAN)
        .column(1, Door::I32, NumFormat::INVARIANT)
        .states_format()
        .header(&["indian", "western"])
        .rows(&[&["1,23,456", "1,23,456"], &["1,234,567", "1,234,567"]]),
        Case::csv(
            "no format beside an Indian column",
            "none,indian\n\"1,234,567\",\"12,34,567\"\n\"12,34,567\",\"1,234,567\"\n",
        )
        .column(0, Door::I32, NumFormat::INVARIANT)
        .column(1, Door::I32, INDIAN)
        .header(&["none", "indian"])
        .rows(&[&["1,234,567", "12,34,567"], &["12,34,567", "1,234,567"]]),
        // What both policies read, and every shape neither does, in the three numeric families.
        Case::csv(
            "both groupings read these shapes",
            "w,i\n\"1,000\",\"1,000\"\n\"1,234\",\"1,234\"\n0125,0125\n1234567,1234567\n",
        )
        .column(0, Door::I64, NumFormat::INVARIANT)
        .column(1, Door::I64, INDIAN)
        .header(&["w", "i"])
        .rows(&[
            &["1,000", "1,000"],
            &["1,234", "1,234"],
            &["0125", "0125"],
            &["1234567", "1234567"],
        ]),
        Case::csv(
            "neither grouping reads these shapes",
            concat!(
                "w,i,f,d\n",
                "\"0,125\",\"0,125\",\"0,125\",\"0,125\"\n",
                "\"0,000\",\"0,000\",\"0,000\",\"0,000\"\n",
                "\"12,34\",\"12,34\",\"12,34\",\"12,34\"\n",
                "\"1,2\",\"1,2\",\"1,2\",\"1,2\"\n",
                "\"1234,567\",\"1234,567\",\"1234,567\",\"1234,567\"\n",
                "\"1,2,345\",\"1,2,345\",\"1,2,345\",\"1,2,345\"\n",
                "\"1,2345,678\",\"1,2345,678\",\"1,2345,678\",\"1,2345,678\"\n",
                "\"1234,567,890\",\"1234,567,890\",\"1234,567,890\",\"1234,567,890\"\n",
            ),
        )
        .column(0, Door::I64, NumFormat::INVARIANT)
        .column(1, Door::I64, INDIAN)
        .column(2, Door::F64, INDIAN)
        .column(3, Door::Decimal, INDIAN)
        .header(&["w", "i", "f", "d"])
        .rows(&[
            &["0,125", "0,125", "0,125", "0,125"],
            &["0,000", "0,000", "0,000", "0,000"],
            &["12,34", "12,34", "12,34", "12,34"],
            &["1,2", "1,2", "1,2", "1,2"],
            &["1234,567", "1234,567", "1234,567", "1234,567"],
            &["1,2,345", "1,2,345", "1,2,345", "1,2,345"],
            &["1,2345,678", "1,2345,678", "1,2345,678", "1,2345,678"],
            &["1234,567,890", "1234,567,890", "1234,567,890", "1234,567,890"],
        ]),
        Case::csv(
            "one policy reads each of these shapes",
            concat!(
                "w,i\n",
                "\"1,234,56,789\",\"1,234,56,789\"\n",
                "\"123,456\",\"123,456\"\n",
                "\"1,23,45,678\",\"1,23,45,678\"\n",
            ),
        )
        .column(0, Door::I64, NumFormat::INVARIANT)
        .column(1, Door::I64, INDIAN)
        .header(&["w", "i"])
        .rows(&[
            &["1,234,56,789", "1,234,56,789"],
            &["123,456", "123,456"],
            &["1,23,45,678", "1,23,45,678"],
        ]),
        Case::csv(
            "space groups: neither reads a misplaced one",
            "w,i\n1 2 345,1 2 345\n1234 567,1234 567\n1 234,1 234\n",
        )
        .column(0, Door::I64, NumFormat::new('.', ' ', NumFormat::ALL))
        .column(
            1,
            Door::I64,
            NumFormat::new('.', ' ', NumFormat::ALL).with_grouping(Grouping::Indian),
        )
        .header(&["w", "i"])
        .rows(&[
            &["1 2 345", "1 2 345"],
            &["1234 567", "1234 567"],
            &["1 234", "1 234"],
        ]),
        // ---- A declared encoding: decoded first, and what's said of a cell is of the decoded. ----
        Case::bytes(
            "windows-1252 decodes ahead of the read",
            b"h\xe9,n\n\x80,\x81\n\xff,\x9f\n",
        )
        .encoding("windows-1252")
        .plan(&[(0, Door::Text), (1, Door::Text)])
        .header(&["h\u{e9}", "n"])
        .rows(&[&["\u{20AC}", "\u{81}"], &["\u{ff}", "\u{178}"]]),
        Case::bytes("latin-1 is every byte's own code point", b"\xe9,\x80\n")
            .encoding("latin-1")
            .no_header()
            .plan(&[(0, Door::Text), (1, Door::Text)])
            .rows(&[&["\u{e9}", "\u{80}"]]),
        Case::bytes(
            "utf-16le with a mark",
            &utf16le("\u{FEFF}a,b\n1,\u{20AC}\u{1F600}\n"),
        )
        .encoding("utf-16le")
        .plan(&[(0, Door::I32), (1, Door::Text)])
        .header(&["a", "b"])
        .rows(&[&["1", "\u{20AC}\u{1F600}"]]),
        Case::bytes("utf-16le without a mark", &utf16le("1\r\n2\r3"))
            .encoding("utf-16le")
            .no_header()
            .plan(&[(0, Door::I32)])
            .rows(&[&["1"], &["2"], &["3"]]),
        Case::bytes(
            "one mark goes and a second is text",
            &utf16le("\u{FEFF}\u{FEFF}a\nb\u{FEFF}\n"),
        )
        .encoding("utf-16le")
        .no_header()
        .plan(&[(0, Door::Text)])
        .rows(&[&["\u{FEFF}a"], &["b\u{FEFF}"]]),
        Case::bytes(
            "those three bytes in windows-1252 are not a mark",
            b"\xef\xbb\xbfa\n",
        )
        .encoding("windows-1252")
        .no_header()
        .plan(&[(0, Door::Text)])
        .rows(&[&["\u{ef}\u{bb}\u{bf}a"]]),
        Case::bytes(
            "a fault's span is the decoded text's",
            b"n\n\"\x80\"\"1\"\n",
        )
        .encoding("windows-1252")
        .plan(&[(0, Door::I32)])
        .header(&["n"])
        .rows(&[&["\u{20AC}\"1"]]),
        Case::bytes(
            "what is passed over and cut is decoded text",
            &utf16le("t\"itle\nx\n\u{e9},b\n1,\u{e9}\n2\n3,\n"),
        )
        .encoding("utf-16le")
        .skip(1)
        .header_row(1)
        .short_rows("pad")
        .plan(&[(0, Door::I32), (1, Door::Text)])
        .header(&["\u{e9}", "b"])
        .rows(&[&["1", "\u{e9}"], &["2"], &["3", ""]]),
        Case::bytes(
            "a lone low surrogate ends the input after its rows",
            &[utf16le("\u{FEFF}a\n1\n\u{1F600}\n"), vec![0x00, 0xDC]].concat(),
        )
        .encoding("utf-16le")
        .plan(&[(0, Door::Text)])
        .header(&["a"])
        .rows(&[&["1"], &["\u{1F600}"]])
        // Sixteen bytes of the file, twelve of the decoded text.
        .fails("encoding", 3, 4, 16),
        Case::bytes(
            "a high surrogate with no low ends the input",
            &[utf16le("1\n"), vec![0x00, 0xD8], utf16le("2\n")].concat(),
        )
        .encoding("utf-16le")
        .no_header()
        .plan(&[(0, Door::I32)])
        .rows(&[&["1"]])
        .fails("encoding", 1, 2, 4),
        Case::bytes(
            "a final half unit ends the input",
            &[utf16le("1\n"), vec![0x32]].concat(),
        )
        .encoding("utf-16le")
        .no_header()
        .plan(&[(0, Door::I32)])
        .rows(&[&["1"]])
        .fails("encoding", 1, 2, 4),
        Case::bytes(
            "a record the failure cuts is not delivered",
            &[utf16le("a,b\n1,2\n3,"), vec![0x00, 0xDC]].concat(),
        )
        .encoding("utf-16le")
        .plan(&[(0, Door::I32), (1, Door::I32)])
        .header(&["a", "b"])
        .rows(&[&["1", "2"]])
        .fails("encoding", 2, 3, 20),
        Case::bytes(
            "a row a lone CR ended is delivered before the failure",
            &[utf16le("1\r"), vec![0x00, 0xDC]].concat(),
        )
        .encoding("utf-16le")
        .no_header()
        .plan(&[(0, Door::I32)])
        .rows(&[&["1"]])
        .fails("encoding", 1, 2, 4),
        Case::bytes(
            "a failure in the header says the file's offset",
            &[utf16le("\u{FEFF}a,"), vec![0x00, 0xD8]].concat(),
        )
        .encoding("utf-16le")
        .plan(&[(0, Door::Text)])
        .fails("encoding", 0, 1, 6),
        Case::csv(
            "suggest: each door takes its own column",
            "flag,small,big,real,day,instant,key,name\n\
             true,1,5000000000,2.5,2026-01-07,2026-01-07T00:00:00Z,6ba7b810-9dad-11d1-80b4-00c04fd430c8,abc\n\
             false,2,-5000000000,-1,2026-01-08,2026-01-08T12:30:00+01:00,6BA7B811-9DAD-11D1-80B4-00C04FD430C8,def\n",
        )
        .all_text(8)
        .header(&["flag", "small", "big", "real", "day", "instant", "key", "name"])
        .rows(&[
            &[
                "true",
                "1",
                "5000000000",
                "2.5",
                "2026-01-07",
                "2026-01-07T00:00:00Z",
                "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
                "abc",
            ],
            &[
                "false",
                "2",
                "-5000000000",
                "-1",
                "2026-01-08",
                "2026-01-08T12:30:00+01:00",
                "6BA7B811-9DAD-11D1-80B4-00C04FD430C8",
                "def",
            ],
        ])
        .suggest(Suggest::of(&[
            ("bool", 0, "resolved", 2, 2),
            ("i32", 0, "resolved", 2, 2),
            ("i64", 0, "resolved", 2, 2),
            ("f64", 0, "resolved", 2, 2),
            ("date", 0, "resolved", 2, 2),
            ("timestamp", 0, "resolved", 2, 2),
            ("uuid", 0, "resolved", 2, 2),
            ("text", 0, "resolved", 2, 2),
        ])),
        // Each door earlier in the preference that also takes the column wins it.
        Case::csv(
            "suggest: the first door that takes every cell wins",
            "ones,words,past,mixed,wide\n0,TRUE,0,1,1\n1,no,2,2.5,5000000000\n",
        )
        .all_text(5)
        .header(&["ones", "words", "past", "mixed", "wide"])
        .rows(&[
            &["0", "TRUE", "0", "1", "1"],
            &["1", "no", "2", "2.5", "5000000000"],
        ])
        .suggest(Suggest::of(&[
            ("bool", 0, "resolved", 2, 2),
            ("bool", 0, "resolved", 2, 2),
            ("i32", 0, "resolved", 2, 2),
            ("f64", 0, "resolved", 2, 2),
            ("i64", 0, "resolved", 2, 2),
        ])),
        // `param` is a set when ambiguous: bit 1 groups by comma and bit 2 is a comma decimal, so 6
        // is both; bits 2 and 3 are month-day-year and day-month-year, so 12 is both. It's an
        // order's code when resolved, or the one notation left of a number: 2, or 4.
        Case::csv(
            "suggest: a comma or a date order the sample can't settle",
            "a,b,c,d,e,f,g,h\n\
             \"1,234\",01/02/2026,\"2,5\",13/02/2026,02/13/2026,2026/01/02,\"1,234,567\",\"1,234\"\n\
             \"5,678\",03/04/2026,\"3,5\",01/02/2026,01/02/2026,2026/02/03,5,2.5\n",
        )
        .all_text(8)
        .header(&["a", "b", "c", "d", "e", "f", "g", "h"])
        .rows(&[
            &[
                "1,234",
                "01/02/2026",
                "2,5",
                "13/02/2026",
                "02/13/2026",
                "2026/01/02",
                "1,234,567",
                "1,234",
            ],
            &[
                "5,678",
                "03/04/2026",
                "3,5",
                "01/02/2026",
                "01/02/2026",
                "2026/02/03",
                "5",
                "2.5",
            ],
        ])
        .suggest(Suggest::of(&[
            ("i32", 6, "ambiguous", 2, 2),
            ("date_ordered", 12, "ambiguous", 2, 2),
            ("f64", 4, "resolved", 2, 2),
            ("date_ordered", 3, "resolved", 2, 2),
            ("date_ordered", 2, "resolved", 2, 2),
            ("date_ordered", 1, "resolved", 2, 2),
            ("i32", 2, "resolved", 2, 2),
            ("f64", 2, "resolved", 2, 2),
        ])),
        // The delimiter rule, each shape alone. Two of one delimiter groups: 2 and 4. One of each:
        // the left groups and the right is the decimal: 2 and 4. One with 1, 2 or more than 3
        // digits after it is the decimal: 4, or 0 for `.`, where no `,` proves anything. One with
        // exactly 3 after it is both, 6, unless what's left of it can't be a first group: a lone
        // zero, or more than three digits.
        Case::csv(
            "suggest: the delimiter rule, each shape alone",
            "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r\n\
             \"1,234,567\",1.234.567,\"1,234.5\",\"1.234,5\",\"2,5\",2.5,\"2,50\",2.50,\"2,5000\",\
             2.5000,\"1,234\",1.234,\"0,125\",0.125,\"1234,567\",1234.567,5,\"1,2345\"\n",
        )
        .all_text(18)
        .header(&[
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        ])
        .rows(&[&[
            "1,234,567",
            "1.234.567",
            "1,234.5",
            "1.234,5",
            "2,5",
            "2.5",
            "2,50",
            "2.50",
            "2,5000",
            "2.5000",
            "1,234",
            "1.234",
            "0,125",
            "0.125",
            "1234,567",
            "1234.567",
            "5",
            "1,2345",
        ]])
        .suggest(Suggest::of(&[
            ("i32", 2, "resolved", 1, 1),
            ("i32", 4, "resolved", 1, 1),
            ("f64", 2, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 0, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 0, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 0, "resolved", 1, 1),
            ("i32", 6, "ambiguous", 1, 1),
            ("f64", 6, "ambiguous", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 0, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
            ("f64", 0, "resolved", 1, 1),
            ("i32", 0, "resolved", 1, 1),
            ("f64", 4, "resolved", 1, 1),
        ])),
        // Each pair: what one cell leaves open, the other settles or contradicts. A column is
        // resolved when exactly one notation reads it, ambiguous when both do and they differ, and
        // unresolved when neither does.
        Case::csv(
            "suggest: the delimiter rule, cells that settle or conflict",
            "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q\n\
             \"1,234\",\"1,234\",1.234,1.234,\"2,5\",\"1,234\",\"1,234\",1.234,1.234,\"1,234\",\
             \"1,234\",1.234,\"1,234,567\",\"1,234,567\",1.234.567,\"1.234,5\",5\n\
             \"2,5\",2.5,2.5,\"2,5\",2.5,1.234,5,5,0.125,\"0,125\",0.125,\"0,125\",1.234.567,\
             \"1,234\",1.234,\"1,234.5\",7\n",
        )
        .all_text(17)
        .header(&[
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q",
        ])
        .rows(&[
            &[
                "1,234",
                "1,234",
                "1.234",
                "1.234",
                "2,5",
                "1,234",
                "1,234",
                "1.234",
                "1.234",
                "1,234",
                "1,234",
                "1.234",
                "1,234,567",
                "1,234,567",
                "1.234.567",
                "1.234,5",
                "5",
            ],
            &[
                "2,5",
                "2.5",
                "2.5",
                "2,5",
                "2.5",
                "1.234",
                "5",
                "5",
                "0.125",
                "0,125",
                "0.125",
                "0,125",
                "1.234.567",
                "1,234",
                "1.234",
                "1,234.5",
                "7",
            ],
        ])
        .suggest(Suggest::of(&[
            ("f64", 4, "resolved", 2, 2),
            ("f64", 2, "resolved", 2, 2),
            ("f64", 0, "resolved", 2, 2),
            ("f64", 4, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
            ("f64", 6, "ambiguous", 2, 2),
            ("i32", 6, "ambiguous", 2, 2),
            ("f64", 6, "ambiguous", 2, 2),
            ("f64", 0, "resolved", 2, 2),
            ("f64", 4, "resolved", 2, 2),
            ("f64", 2, "resolved", 2, 2),
            ("f64", 4, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
            ("i32", 2, "resolved", 2, 2),
            ("i32", 4, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
            ("i32", 0, "resolved", 2, 2),
        ])),
        // Every form a `.` or a `,` reads as a decimal or a group, in one column: still a question.
        Case::csv(
            "suggest: three digits after a lone separator, with the first group's every width",
            "dots,commas\n1.234,\"1,234\"\n12.345,\"12,345\"\n123.456,\"123,456\"\n-1.234,\"-1,234\"\n",
        )
        .all_text(2)
        .header(&["dots", "commas"])
        .rows(&[
            &["1.234", "1,234"],
            &["12.345", "12,345"],
            &["123.456", "123,456"],
            &["-1.234", "-1,234"],
        ])
        .suggest(Suggest::of(&[
            ("f64", 6, "ambiguous", 4, 4),
            ("i32", 6, "ambiguous", 4, 4),
        ])),
        // A declared format asks nothing, and what it refuses is a cell no door took: the sample
        // and a read of it agree on a lone zero's group.
        Case::csv(
            "suggest: a declared format asks nothing, Western",
            "a,b,c,d\n\"0,125\",0.125,1.234,\"1,234\"\n",
        )
        .all_text(4)
        .header(&["a", "b", "c", "d"])
        .rows(&[&["0,125", "0.125", "1.234", "1,234"]])
        .suggest(Suggest {
            format: Some(NumFormat::INVARIANT),
            ..Suggest::of(&[
                ("text", 0, "resolved", 1, 1),
                ("f64", 0, "resolved", 1, 1),
                ("f64", 0, "resolved", 1, 1),
                ("i32", 0, "resolved", 1, 1),
            ])
        }),
        Case::csv(
            "suggest: a declared format asks nothing, a comma decimal",
            "a,b,c,d\n\"0,125\",0.125,1.234,\"2,5\"\n",
        )
        .all_text(4)
        .header(&["a", "b", "c", "d"])
        .rows(&[&["0,125", "0.125", "1.234", "2,5"]])
        .suggest(Suggest {
            format: Some(NumFormat::new(
                ',',
                '.',
                NumFormat::ALL,
            )),
            ..Suggest::of(&[
                ("f64", 0, "resolved", 1, 1),
                ("text", 0, "resolved", 1, 1),
                ("i32", 0, "resolved", 1, 1),
                ("f64", 0, "resolved", 1, 1),
            ])
        }),
        Case::csv(
            "suggest: a column with a cell no door takes, and blanks",
            "failed,words,blank,spaces\n1,abc,,\"  \"\nabc,,,7\n",
        )
        .all_text(4)
        .header(&["failed", "words", "blank", "spaces"])
        .rows(&[&["1", "abc", "", "  "], &["abc", "", "", "7"]])
        .suggest(Suggest::of(&[
            ("text", 0, "unresolved", 1, 2),
            ("text", 0, "resolved", 1, 1),
            ("text", 0, "unresolved", 0, 0),
            ("i32", 0, "resolved", 1, 1),
        ])),
        Case::csv(
            "suggest: a value the number would lose something of",
            "padded,zip,huge,float,signs,nan\n\
             007,07030,99999999999999999999,1e30,-0,NaN\n\
             0012,10001,99999999999999999999,1.5e300,+1,2.5\n",
        )
        .all_text(6)
        .header(&["padded", "zip", "huge", "float", "signs", "nan"])
        .rows(&[
            &["007", "07030", "99999999999999999999", "1e30", "-0", "NaN"],
            &[
                "0012",
                "10001",
                "99999999999999999999",
                "1.5e300",
                "+1",
                "2.5",
            ],
        ])
        .suggest(Suggest::of(&[
            ("text", 0, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
            ("text", 0, "resolved", 2, 2),
            ("f64", 0, "resolved", 2, 2),
            ("i32", 0, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
        ])),
        Case::csv(
            "suggest: a declared format settles it, Western grouping",
            "a,b\n\"1,234\",\"12,34\"\n\"5,678\",\"56,78\"\n",
        )
        .all_text(2)
        .header(&["a", "b"])
        .rows(&[&["1,234", "12,34"], &["5,678", "56,78"]])
        .suggest(Suggest {
            format: Some(NumFormat::INVARIANT),
            ..Suggest::of(&[("i32", 0, "resolved", 2, 2), ("text", 0, "resolved", 2, 2)])
        }),
        // An Indian shape has no culture until one is declared. Then it's a number, and a Western
        // shape isn't.
        Case::csv(
            "suggest: an Indian column with nothing declared",
            "a,b\n\"1,23,456\",\"1,23,456\"\n\"12,34,567\",5\n",
        )
        .all_text(2)
        .header(&["a", "b"])
        .rows(&[&["1,23,456", "1,23,456"], &["12,34,567", "5"]])
        .suggest(Suggest::of(&[
            ("text", 0, "resolved", 2, 2),
            ("text", 0, "unresolved", 1, 2),
        ])),
        Case::csv(
            "suggest: a declared format settles it, Indian grouping",
            "a,b,c\n\"1,23,456\",\"1,234,567\",\"1,23,456\"\n\"12,34,567\",\"1,234,567\",5\n",
        )
        .all_text(3)
        .header(&["a", "b", "c"])
        .rows(&[
            &["1,23,456", "1,234,567", "1,23,456"],
            &["12,34,567", "1,234,567", "5"],
        ])
        .suggest(Suggest {
            format: Some(INDIAN),
            ..Suggest::of(&[
                ("i32", 0, "resolved", 2, 2),
                ("text", 0, "resolved", 2, 2),
                ("i32", 0, "resolved", 2, 2),
            ])
        }),
        Case::csv(
            "suggest: a declared format settles it, a comma decimal",
            "a,b\n\"1,234\",1.234\n",
        )
        .all_text(2)
        .header(&["a", "b"])
        .rows(&[&["1,234", "1.234"]])
        .suggest(Suggest {
            format: Some(NumFormat::new(
                ',',
                '.',
                NumFormat::ALL,
            )),
            ..Suggest::of(&[("f64", 0, "resolved", 1, 1), ("i32", 0, "resolved", 1, 1)])
        }),
        Case::csv(
            "suggest: a sample stops at its rows, and says there are more",
            "a\n2\n3\nabc\n",
        )
        .all_text(1)
        .header(&["a"])
        .rows(&[&["2"], &["3"], &["abc"]])
        .suggest(Suggest {
            sample_rows: Some(2),
            truncated: true,
            ..Suggest::of(&[("i32", 0, "resolved", 2, 2)])
        }),
        Case::csv(
            "suggest: a sample of exactly the source's rows isn't cut short",
            "a\n2\n3\n",
        )
        .all_text(1)
        .header(&["a"])
        .rows(&[&["2"], &["3"]])
        .suggest(Suggest {
            sample_rows: Some(2),
            ..Suggest::of(&[("i32", 0, "resolved", 2, 2)])
        }),
        Case::csv(
            "suggest: a token is a blank, and a quoted one isn't a token",
            "only,some,quoted\nNA,NA,\"NA\"\nNA,7,\"NA\"\n",
        )
        .tokens(&["NA"])
        .all_text(3)
        .header(&["only", "some", "quoted"])
        .rows(&[&["NA", "NA", "NA"], &["NA", "7", "NA"]])
        .token_cells(&[(0, 0), (0, 1), (1, 0)])
        .suggest(Suggest::of(&[
            ("text", 0, "unresolved", 0, 0),
            ("i32", 0, "resolved", 1, 1),
            ("text", 0, "resolved", 2, 2),
        ])),
        Case::bytes(
            "suggest: a cell that isn't text leaves its column unresolved",
            b"a,b\n\xe9t\xe9,7\nabc,\xff\n",
        )
        .all_text(2)
        .header(&["a", "b"])
        .byte_rows(&[&[b"\xe9t\xe9", b"7"], &[b"abc", b"\xff"]])
        .suggest(Suggest::of(&[
            ("text", 0, "unresolved", 0, 2),
            ("text", 0, "unresolved", 1, 2),
        ])),
        // The first record is a row here. `7` isn't on the boolean door's list, and neither is
        // `ab`, where a lone `y` would be.
        Case::csv("suggest: with no header, the first record's width", "7,ab\n8,cd\n")
            .no_header()
            .all_text(2)
            .rows(&[&["7", "ab"], &["8", "cd"]])
            .suggest(Suggest::of(&[
                ("i32", 0, "resolved", 2, 2),
                ("text", 0, "resolved", 2, 2),
            ])),
        // Constraints: HyperCast's checked doors judge each cell, and a value that casts and
        // breaks a declared rule is a violation carrying it as cast.
        Case::csv("constraints: a violation keeps the value as cast", "n,m\n11,7\n7,12\nx,5\n")
            .plan(&[(0, Door::I32), (1, Door::I32)])
            .constrain(0, r#"{"maximum": "10"}"#)
            .constrain(1, r#"{"minimum": "6", "exclusiveMaximum": "12"}"#)
            .header(&["n", "m"])
            .rows(&[&["11", "7"], &["7", "12"], &["x", "5"]]),
        Case::csv("constraints: every numeric keyword over the integers", "v\n0\n5\n-3\n")
            .plan(&[
                (0, Door::I8),
                (0, Door::I16),
                (0, Door::I64),
                (0, Door::U8),
                (0, Door::U16),
                (0, Door::U32),
                (0, Door::U64),
            ])
            .constrain(0, r#"{"minimum": "0"}"#)
            .constrain(1, r#"{"exclusiveMinimum": "0"}"#)
            .constrain(2, r#"{"maximum": "-1"}"#)
            .constrain(3, r#"{"exclusiveMaximum": "5"}"#)
            .constrain(4, r#"{"multipleOf": "5"}"#)
            .constrain(5, r#"{"enum": ["0", "5"]}"#)
            .constrain(6, r#"{"minimum": "5"}"#)
            .header(&["v"])
            .rows(&[&["0"], &["5"], &["-3"]]),
        Case::csv("constraints: every numeric keyword over the floats and the decimal", "v\n-0\n0.3\n1.25\n")
            .plan(&[(0, Door::F64), (0, Door::F32), (0, Door::Decimal), (0, Door::F64), (0, Door::Decimal), (0, Door::F32)])
            .constrain(0, r#"{"minimum": "0", "maximum": "1"}"#)
            .constrain(1, r#"{"exclusiveMinimum": "0", "exclusiveMaximum": "1"}"#)
            .constrain(2, r#"{"multipleOf": "0.25"}"#)
            .constrain(3, r#"{"enum": ["0", "0.3"]}"#)
            .constrain(4, r#"{"minimum": "0.3", "exclusiveMaximum": "1.25"}"#)
            .constrain(5, r#"{"multipleOf": "0.1", "rounding": "ceiling", "maximum": "1.2"}"#)
            .header(&["v"])
            .rows(&[&["-0"], &["0.3"], &["1.25"]]),
        Case::csv("constraints: multipleOf rounds, or refuses", "v\n1.3\n1.2\n2.5\n")
            .plan(&[(0, Door::F64), (0, Door::F64), (0, Door::Decimal), (0, Door::Decimal)])
            .constrain(0, r#"{"multipleOf": "0.5", "rounding": "midpoint"}"#)
            .constrain(1, r#"{"multipleOf": "0.5"}"#)
            .constrain(2, r#"{"multipleOf": "0.5", "rounding": "floor"}"#)
            .constrain(3, r#"{"multipleOf": "0.5", "rounding": "ceiling", "maximum": "2.5"}"#)
            .header(&["v"])
            .rows(&[&["1.3"], &["1.2"], &["2.5"]]),
        // A rounding past what the type holds is `multipleOf`, and the value is as cast.
        Case::csv("constraints: a rounding past the type is multipleOf", "v\n127\n")
            .plan(&[(0, Door::I8)])
            .constrain(0, r#"{"multipleOf": "5", "rounding": "ceiling"}"#)
            .header(&["v"])
            .rows(&[&["127"]]),
        Case::csv("constraints: an empty enum matches nothing", "v\n7\n")
            .plan(&[(0, Door::I32)])
            .constrain(0, r#"{"enum": []}"#)
            .header(&["v"])
            .rows(&[&["7"]]),
        Case::csv("constraints: a numeric enum matches the value, not the text", "v\n10\n10.0\n15\n")
            .plan(&[(0, Door::I32), (0, Door::Decimal)])
            .constrain(0, r#"{"enum": ["10", "20"]}"#)
            .constrain(1, r#"{"enum": ["10"]}"#)
            .header(&["v"])
            .rows(&[&["10"], &["10.0"], &["15"]]),
        Case::csv("constraints: a trimmed text spans the judged text", "a,b\nx,  ab \n")
            .plan(&[(1, Door::Text), (1, Door::Text), (0, Door::Text)])
            .constrain(0, r#"{"trim": true, "maxLength": 1}"#)
            .constrain(1, r#"{"trim": true, "minLength": 2}"#)
            .constrain(2, r#"{"minLength": 2}"#)
            .header(&["a", "b"])
            .rows(&[&["x", "  ab "]]),
        Case::csv("constraints: a quoted cell is judged unescaped", "a\n\"  a\"\"b  \"\n")
            .plan(&[(0, Door::Text), (0, Door::Text)])
            .constrain(0, r#"{"trim": true, "enum": ["a\"b"]}"#)
            .constrain(1, r#"{"trim": true, "maxLength": 1}"#)
            .header(&["a"])
            .rows(&[&["  a\"b  "]]),
        Case::csv("constraints: tokens and padding stay empty", "a,b\nNA,abc\nabc\n")
            .short_rows("pad")
            .tokens(&["NA"])
            .plan(&[(0, Door::Text), (1, Door::Text)])
            .constrain(0, r#"{"minLength": 3}"#)
            .constrain(1, r#"{"minLength": 3}"#)
            .header(&["a", "b"])
            .rows(&[&["NA", "abc"], &["abc"]])
            .token_cells(&[(0, 0)]),
        Case::csv("constraints: a number in a text enum is text", "v\n1\n1.0\n")
            .plan(&[(0, Door::Text)])
            .constrain(0, r#"{"enum": ["01", "1.0"]}"#)
            .header(&["v"])
            .rows(&[&["1"], &["1.0"]]),
        Case::csv("constraints: an enum folds case unless it is case sensitive", "v\nyES\nYes\n")
            .plan(&[(0, Door::Text), (0, Door::Text)])
            .constrain(0, r#"{"enum": ["Yes"]}"#)
            .constrain(1, r#"{"enum": ["Yes"], "caseSensitive": true}"#)
            .header(&["v"])
            .rows(&[&["yES"], &["Yes"]]),
        Case::csv("constraints: of duplicate enum entries the first matches", "v\na\nA\n")
            .plan(&[(0, Door::Text)])
            .constrain(0, r#"{"enum": ["b", "A", "a"]}"#)
            .header(&["v"])
            .rows(&[&["a"], &["A"]]),
        Case::bytes("constraints: text that isn't UTF-8 is the door's fault", b"v\nok\n\xff\n")
            .plan(&[(0, Door::Text)])
            .constrain(0, r#"{"maxLength": 9}"#)
            .header(&["v"])
            .byte_rows(&[&[b"ok"], &[b"\xff"]]),
    ]
}

/// The file a case in UTF-16LE holds.
fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

pub fn corpus_json() -> String {
    let cases: Vec<Value> = cases()
        .iter()
        .map(|case| {
            let plan: Vec<Value> = case
                .plan
                .iter()
                .enumerate()
                .map(|(place, (ordinal, door, format))| {
                    let mut entry = door_json(*door);
                    entry["ordinal"] = json!(ordinal);
                    if *format != NumFormat::INVARIANT || case.states_format {
                        entry["format"] = format_json(format);
                    }
                    if let Some((_, tokens)) = case.column_tokens.iter().find(|c| c.0 == place) {
                        entry["tokens"] = json!(tokens);
                    }
                    if let Some((_, date)) = case.column_dates.iter().find(|c| c.0 == place) {
                        entry["options"] = date.json();
                    }
                    if let Some((_, declared)) =
                        case.column_constraints.iter().find(|c| c.0 == place)
                    {
                        entry["constraints"] = declared.clone();
                    }
                    entry
                })
                .collect();
            let rows: Vec<Value> = case
                .cells
                .iter()
                .enumerate()
                .map(|(at, row)| {
                    let cells: Vec<Value> = case
                        .plan
                        .iter()
                        .enumerate()
                        .map(|(place, (ordinal, door, format))| {
                            // A cell the row doesn't have, or a token: no door is asked.
                            let Some(text) = row.get(*ordinal as usize).copied() else {
                                return json!({"expect": "empty", "origin": "padded"});
                            };
                            if case.token_cells.contains(&(at, place)) {
                                return json!({"expect": "empty", "origin": "token"});
                            }
                            if let Some((_, declared)) =
                                case.column_constraints.iter().find(|c| c.0 == place)
                            {
                                return checked_verdict(*door, format, declared, text);
                            }
                            let date = case
                                .column_dates
                                .iter()
                                .find(|c| c.0 == place)
                                .map(|c| c.1.valid());
                            verdict(*door, format, date.as_ref(), text)
                        })
                        .collect();
                    json!(cells)
                })
                .collect();
            let long = LONG_ROWS
                .iter()
                .find(|entry| entry.0 == case.name)
                .map_or(&[][..], |entry| entry.1);
            let sources: Vec<usize> = case.plan.iter().map(|column| column.0 as usize).collect();
            let lines = ROW_LINES
                .iter()
                .find(|entry| entry.0 == case.name)
                .map_or(&[][..], |entry| entry.1);
            let refused = rows
                .iter()
                .flat_map(|row| row.as_array().expect("a row"))
                .any(|cell| {
                    matches!(
                        cell["expect"].as_str(),
                        Some("malformed" | "out_of_range" | "violation")
                    )
                });
            assert!(
                (!refused && long.is_empty()) || lines.len() == rows.len(),
                "{}: its problems need ROW_LINES to say where each of its rows starts",
                case.name
            );
            let problems = problems_of(&rows, lines, &sources, long);
            let mut value = json!({
                "name": case.name,
                "dialect": {
                    "separator": case.separator.to_string(),
                    "quoting": case.quoting,
                    "skip_blank_lines": case.skip_blank_lines,
                    "has_header": case.has_header,
                },
                "plan": plan,
                "header": case.header,
                "rows": rows,
                "problems": problems,
            });
            if case.skip != 0 {
                value["dialect"]["skip"] = json!(case.skip);
            }
            if case.header_row != 0 {
                value["dialect"]["header_row"] = json!(case.header_row);
            }
            if let Some(policy) = case.short_rows {
                value["dialect"]["short_rows"] = json!(policy);
            }
            if let Some(policy) = case.long_rows {
                value["dialect"]["long_rows"] = json!(policy);
            }
            if let Some(tokens) = case.tokens {
                value["dialect"]["tokens"] = json!(tokens);
            }
            if let Some(encoding) = case.encoding {
                value["dialect"]["encoding"] = json!(encoding);
            }
            // A declared encoding's file is bytes, even where they'd pass for text.
            match std::str::from_utf8(&case.input) {
                Ok(input) if case.encoding.is_none() => value["input"] = json!(input),
                _ => value["input_hex"] = json!(hex(&case.input)),
            }
            if let Some(suggest) = &case.suggest {
                value["suggest"] = suggest.json();
            }
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
    } else if file.starts_with("ranges") {
        (6, None)
    } else if file.starts_with("uncached")
        || file.starts_with("dates")
        || file.starts_with("constraints")
    {
        (3, None)
    } else if file.starts_with("suggest") {
        (7, None)
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
            [".xlsx", ".xlsm", ".ods", ".numbers"]
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
                        Format::Numbers => "numbers",
                        other => panic!("format {other:?} has no corpus name"),
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
        if file.starts_with("ranges") {
            cases.extend(range_cases(file, &book, &sheets));
        } else if file.starts_with("uncached") {
            cases.extend(uncached_cases(file, &book, &sheets));
        } else if file.starts_with("dates") {
            cases.extend(date_cases(file, &book, &sheets));
        } else if file.starts_with("grouping") {
            cases.extend(grouping_cases(file, &book, &sheets));
        } else if file.starts_with("suggest") {
            cases.extend(suggest_cases(file, &book, &sheets));
        } else if file.starts_with("constraints") {
            cases.extend(constraint_cases(file, &book, &sheets));
        }
    }
    for case in &mut cases {
        with_problems(case);
    }
    cases
}

/// Adds a read case's `problems`, from the rows and numbers it already holds.
fn with_problems(case: &mut Value) {
    let Some(rows) = case["rows"].as_array() else {
        return;
    };
    let first = case["options"]["range"].as_str().map_or(0, |range| {
        let range = Range::parse(range).expect("a range");
        range.first_column() as usize - 1
    });
    let sources: Vec<usize> = case["plan"]
        .as_array()
        .expect("plan")
        .iter()
        .map(|column| first + column["ordinal"].as_u64().expect("ordinal") as usize)
        .collect();
    let lines: Vec<u32> = case["numbers"]
        .as_array()
        .expect("numbers")
        .iter()
        .map(|number| number.as_u64().expect("a number") as u32)
        .collect();
    case["problems"] = json!(problems_of(rows, &lines, &sources, &[]));
}

/// A plan entry for `door` at `ordinal`.
fn entry(door: Door, ordinal: usize) -> Value {
    let mut entry = door_json(door);
    entry["ordinal"] = json!(ordinal);
    entry
}

/// What a corpus says a plan is: a text door over each of `width` columns.
fn text_plan(width: usize) -> Vec<Value> {
    (0..width)
        .map(|ordinal| entry(Door::Text, ordinal))
        .collect()
}

/// The cases written by hand over `ranges.*`, the one table in all three formats: where a range,
/// a skip, a header row, tokens and date options meet. Each names what it is reading, and only the
/// options it sets are in its `options`.
fn range_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    let compact = json!({"flags": ["compact_date"]});
    let mut own_tokens = entry(Door::Text, 2);
    own_tokens["tokens"] = json!(["12a"]);
    let mut compact_date = entry(Door::Date, 0);
    compact_date["options"] = compact;
    let header = |range: &str| json!({"has_header": true, "skip_empty_rows": true, "range": range});
    let plain = |range: &str, skip_empty_rows: bool| json!({"has_header": false, "skip_empty_rows": skip_empty_rows, "range": range});
    let whole = |skip: u32, header_row: u32, skip_empty_rows: bool| {
        let mut options = json!({"has_header": true, "skip_empty_rows": skip_empty_rows});
        if skip != 0 {
            options["skip"] = json!(skip);
        }
        if header_row != 0 {
            options["header_row"] = json!(header_row);
        }
        options["range"] = json!("A1:F15");
        options
    };
    let mut tokens = header("B4:E12");
    tokens["tokens"] = json!(["NA", "-"]);
    let mut no_range = whole(3, 0, true);
    no_range.as_object_mut().expect("options").remove("range");
    let mut listed: Vec<(&str, usize, Value, Vec<Value>)> = vec![
        ("a range and a header", 0, header("B4:E12"), text_plan(4)),
        (
            "a range's last column ends the table",
            0,
            header("B4:C12"),
            text_plan(3),
        ),
        (
            "empty rows inside a range",
            0,
            plain("B4:E12", false),
            text_plan(4),
        ),
        (
            "a range inside a repeated row",
            0,
            plain("B9:E9", true),
            text_plan(4),
        ),
        (
            "a range ending at a repeat's end",
            0,
            plain("B9:E11", true),
            text_plan(4),
        ),
        (
            "a range running past the sheet",
            0,
            plain("B14:E1000", false),
            text_plan(4),
        ),
        (
            "a range wholly past the sheet",
            0,
            plain("A100:B200", false),
            text_plan(2),
        ),
        ("skip, then a header", 0, whole(3, 0, true), text_plan(6)),
        (
            "skip, a header row, then a header",
            0,
            whole(2, 1, true),
            text_plan(6),
        ),
        (
            "a header row counting empty rows",
            0,
            whole(0, 3, false),
            text_plan(6),
        ),
        ("skip with no range", 0, no_range, text_plan(6)),
        (
            "tokens over text cells, typed cells and a column's own",
            0,
            tokens,
            vec![
                entry(Door::I32, 0),
                entry(Door::Text, 1),
                own_tokens,
                entry(Door::Text, 3),
            ],
        ),
        (
            "details of text and typed cells",
            0,
            plain("B5:E12", true),
            vec![
                entry(Door::I32, 2),
                entry(Door::I32, 3),
                entry(Door::Bool, 3),
                entry(Door::F64, 2),
                entry(Door::Text, 9),
            ],
        ),
        (
            "date options reaching text cells",
            0,
            plain("E5:E6", true),
            vec![compact_date, entry(Door::Date, 0)],
        ),
        (
            "columns cutting a repeated cell",
            0,
            plain("C15:E15", true),
            text_plan(4),
        ),
        (
            "columns past a repeated cell",
            0,
            plain("B15:F15", true),
            text_plan(5),
        ),
    ];
    if file.ends_with(".xlsx") {
        listed.push((
            "a range stopping before a break",
            1,
            plain("A1:B2", true),
            text_plan(2),
        ));
        listed.push((
            "a range reaching a break",
            1,
            plain("A1:B3", true),
            text_plan(2),
        ));
    }
    listed_cases(file, book, sheets, listed)
}

/// The cases written by hand over `uncached.*`: where a formula with no result meets a token, a
/// range and a door that's no text door.
fn uncached_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    let plain =
        |skip_empty_rows: bool| json!({"has_header": false, "skip_empty_rows": skip_empty_rows});
    let mut tokens = plain(false);
    tokens["tokens"] = json!([""]);
    let mut range = plain(false);
    range["range"] = json!("B1:C30");
    let listed = vec![
        ("a token of no text", 0, tokens, text_plan(3)),
        ("a range starting at the formulas", 0, range, text_plan(2)),
        (
            "doors that aren't the text door",
            0,
            plain(true),
            vec![
                entry(Door::I32, 1),
                entry(Door::Bool, 1),
                entry(Door::Date, 1),
                entry(Door::F64, 1),
            ],
        ),
    ];
    listed_cases(file, book, sheets, listed)
}

/// A plan entry for `door` at `ordinal` declaring `constraints`, HyperCast's corpus shape as JSON
/// text.
fn constrained(door: Door, ordinal: usize, constraints: &str) -> Value {
    let mut entry = entry(door, ordinal);
    entry["constraints"] = serde_json::from_str(constraints).expect("a declaration is JSON");
    entry
}

/// The cases written by hand over `constraints.xlsx`: a typed number judged through its typed twin
/// (`-0` with no sign, a sum on its shortest digits), at the text door through its canonical text, a
/// cell the door has no twin for, a formula with no result and a token beside them, and text cells
/// judged with their spans.
fn constraint_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    let mut options = json!({"has_header": true, "skip_empty_rows": false});
    options["tokens"] = json!(["NA"]);
    let listed = vec![
        (
            "typed numbers judged through their typed twins",
            0,
            options.clone(),
            vec![
                constrained(Door::F64, 0, r#"{"minimum": "0", "maximum": "0.3"}"#),
                constrained(Door::I32, 0, r#"{"maximum": "10"}"#),
                constrained(Door::Text, 0, r#"{"enum": ["42", "11", "0"]}"#),
                constrained(
                    Door::Decimal,
                    0,
                    r#"{"multipleOf": "0.5", "rounding": "midpoint"}"#,
                ),
                constrained(Door::I32, 2, r#"{"maximum": "5"}"#),
                constrained(Door::F32, 0, r#"{"exclusiveMaximum": "42"}"#),
            ],
        ),
        (
            "text judged with its span",
            0,
            options,
            vec![
                constrained(Door::Text, 1, r#"{"trim": true, "maxLength": 3}"#),
                constrained(Door::Text, 1, r#"{"enum": ["Female", "Male"]}"#),
                constrained(Door::I32, 1, r#"{"maximum": "10"}"#),
                constrained(Door::Text, 2, r#"{"minLength": 2}"#),
            ],
        ),
    ];
    listed_cases(file, book, sheets, listed)
}

/// The cases written by hand over `dates.*`: where a field order decides a text cell, and declared
/// month names reach one. `corpus_workbook.rs` holds what they read to a table worked out by hand.
fn date_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    let options = json!({"has_header": true, "skip_empty_rows": true});
    let ordered = |order: DateOrder| {
        vec![
            entry(Door::DateOrdered(order), 0),
            entry(Door::DateTime(order), 1),
        ]
    };
    let german = json!({"flags": [], "months": [
        [1, "Jan"], [2, "Feb"], [3, "M\u{e4}r"], [4, "Apr"], [5, "Mai"], [6, "Jun"],
        [7, "Jul"], [8, "Aug"], [9, "Sep"], [10, "Okt"], [11, "Nov"], [12, "Dez"],
    ]});
    let named = |order: DateOrder| {
        let mut column = entry(Door::DateOrdered(order), 2);
        column["options"] = german.clone();
        column
    };
    let listed = vec![
        (
            "year first",
            0,
            options.clone(),
            ordered(DateOrder::YearMonthDay),
        ),
        (
            "month first",
            0,
            options.clone(),
            ordered(DateOrder::MonthDayYear),
        ),
        (
            "day first",
            0,
            options.clone(),
            ordered(DateOrder::DayMonthYear),
        ),
        (
            "month names, day first and month first, and none declared",
            0,
            options.clone(),
            vec![
                named(DateOrder::DayMonthYear),
                named(DateOrder::MonthDayYear),
                entry(Door::DateOrdered(DateOrder::DayMonthYear), 2),
            ],
        ),
    ];
    listed_cases(file, book, sheets, listed)
}

/// The cases written by hand over `grouping.*`: a column's policy reaching a text cell, and leaving a
/// typed one alone. `corpus_workbook.rs` holds what they read to a table worked out by hand.
fn grouping_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    let in_format = |ordinal: usize, format: &NumFormat| {
        let mut column = entry(Door::I32, ordinal);
        column["format"] = format_json(format);
        column
    };
    let options = json!({"has_header": true, "skip_empty_rows": true});
    let listed = vec![
        (
            "an Indian column beside a Western one",
            0,
            options.clone(),
            vec![
                in_format(0, &INDIAN),
                in_format(1, &NumFormat::INVARIANT),
                in_format(2, &INDIAN),
                in_format(2, &NumFormat::INVARIANT),
            ],
        ),
        (
            "no format is Western grouping",
            0,
            options.clone(),
            vec![
                entry(Door::I32, 0),
                entry(Door::I32, 1),
                entry(Door::I32, 2),
            ],
        ),
        (
            "the policies crossed",
            0,
            options.clone(),
            vec![in_format(0, &NumFormat::INVARIANT), in_format(1, &INDIAN)],
        ),
    ];
    let mut cases = listed_cases(file, book, sheets, listed);
    let mut suggested = |what: &str, format: Option<NumFormat>, columns: &'static [Suggested]| {
        let mut plan = Vec::new();
        for (ordinal, column) in columns.iter().enumerate() {
            let door = if column.0 == "i32" {
                Door::I32
            } else {
                Door::Text
            };
            plan.push(if door == Door::I32 {
                in_format(ordinal, &format.unwrap_or(NumFormat::INVARIANT))
            } else {
                entry(door, ordinal)
            });
        }
        let mut case = listed_cases(file, book, sheets, vec![(what, 0, options.clone(), plan)]);
        let said = Suggest {
            format,
            ..Suggest::of(columns)
        };
        case[0]["suggest"] = said.json();
        cases.extend(case);
    };
    // Nothing declared, column A has no culture and B is `,` groups. Column C's shapes are each one
    // policy's.
    suggested(
        "suggest, nothing declared",
        None,
        &[
            ("text", 0, "unresolved", 1, 3),
            ("i32", 2, "resolved", 3, 3),
            ("text", 0, "unresolved", 2, 3),
        ],
    );
    suggested(
        "suggest, Indian declared",
        Some(INDIAN),
        &[
            ("i32", 0, "resolved", 3, 3),
            ("text", 0, "unresolved", 2, 3),
            ("text", 0, "unresolved", 2, 3),
        ],
    );
    cases
}

/// What the suggester says of `suggest.*`, by hand from the table in `packages.rs` and the rule in
/// `docs/design.md`: a typed number has no notation, and a text cell asks what text asks. Each entry
/// is the declaration, what's said of the seven columns, and a plan of what that says, whose rows
/// are read as any case's are and whose values `corpus_workbook.rs` holds to a table of its own.
fn suggest_cases(file: &str, book: &Workbook<'_>, sheets: &[Value]) -> Vec<Value> {
    const NONE: &[Suggested] = &[
        ("f64", 0, "resolved", 3, 3),
        ("f64", 6, "ambiguous", 3, 3),
        ("f64", 6, "ambiguous", 3, 3),
        ("i32", 0, "resolved", 3, 3),
        ("f64", 4, "resolved", 3, 3),
        ("i64", 0, "resolved", 3, 3),
        ("bool", 0, "resolved", 3, 3),
    ];
    const COMMA: &[Suggested] = &[
        ("f64", 0, "resolved", 3, 3),
        ("i32", 0, "resolved", 3, 3),
        ("f64", 0, "resolved", 3, 3),
        ("i32", 0, "resolved", 3, 3),
        ("f64", 0, "resolved", 3, 3),
        ("i64", 0, "resolved", 3, 3),
        ("bool", 0, "resolved", 3, 3),
    ];
    const WESTERN: &[Suggested] = &[
        ("f64", 0, "resolved", 3, 3),
        ("f64", 0, "resolved", 3, 3),
        ("f64", 0, "resolved", 3, 3),
        ("i32", 0, "resolved", 3, 3),
        ("text", 0, "unresolved", 2, 3),
        ("i64", 0, "resolved", 3, 3),
        ("bool", 0, "resolved", 3, 3),
    ];
    let comma_decimal = NumFormat::new(',', '.', NumFormat::ALL);
    let in_format = |door: Door, ordinal: usize, format: &NumFormat| {
        let mut column = entry(door, ordinal);
        column["format"] = format_json(format);
        column
    };
    let doors = |doors: [Door; 6], format: &NumFormat| -> Vec<Value> {
        let mut plan: Vec<Value> = doors
            .into_iter()
            .enumerate()
            .map(|(ordinal, door)| in_format(door, ordinal, format))
            .collect();
        plan.push(entry(Door::Bool, 6));
        plan
    };
    let options = json!({"has_header": true, "skip_empty_rows": true});
    let listed = vec![
        ("nothing declared", 0, options.clone(), {
            let mut plan = doors(
                [
                    Door::F64,
                    Door::Text,
                    Door::Text,
                    Door::I32,
                    Door::F64,
                    Door::I64,
                ],
                &NumFormat::INVARIANT,
            );
            plan[4] = in_format(Door::F64, 4, &comma_decimal);
            plan
        }),
        (
            "a comma decimal declared",
            0,
            options.clone(),
            doors(
                [
                    Door::F64,
                    Door::I32,
                    Door::F64,
                    Door::I32,
                    Door::F64,
                    Door::I64,
                ],
                &comma_decimal,
            ),
        ),
        (
            "Western declared",
            0,
            options,
            doors(
                [
                    Door::F64,
                    Door::F64,
                    Door::F64,
                    Door::I32,
                    Door::Text,
                    Door::I64,
                ],
                &NumFormat::INVARIANT,
            ),
        ),
    ];
    let mut cases = listed_cases(file, book, sheets, listed);
    for (case, (columns, format)) in cases.iter_mut().zip([
        (NONE, None),
        (COMMA, Some(comma_decimal)),
        (WESTERN, Some(NumFormat::INVARIANT)),
    ]) {
        let said = Suggest {
            format,
            ..Suggest::of(columns)
        };
        case["suggest"] = said.json();
    }
    cases
}

/// A case for each entry of a hand-written list: what it's reading, the sheet, its options and plan.
fn listed_cases(
    file: &str,
    book: &Workbook<'_>,
    sheets: &[Value],
    listed: Vec<(&str, usize, Value, Vec<Value>)>,
) -> Vec<Value> {
    listed
        .into_iter()
        .map(|(what, index, options, plan)| {
            let columns: Vec<Column> = plan.iter().map(column_of).collect();
            let mut sheet = book
                .sheet(index, sheet_options_of(&options), &columns)
                .unwrap_or_else(|error| panic!("{file}, {what}: {error}"));
            let header = header_json(sheet.header());
            let (mut numbers, mut rows) = (Vec::new(), Vec::new());
            let failure = loop {
                match sheet.read() {
                    Ok(Some(batch)) => {
                        numbers.extend((0..batch.rows()).map(|row| batch.line(row)));
                        rows_json(&batch, &mut rows);
                    }
                    Ok(None) => break None,
                    Err(error) => break Some(error),
                }
            };
            let mut case = json!({
                "name": format!("{file}, sheet {index}, {what}"),
                "file": format!("workbook/{file}"),
                "format": match book.format() {
                    Format::Xlsx => "xlsx",
                    Format::Ods => "ods",
                    Format::Numbers => "numbers",
                    other => panic!("format {other:?} has no corpus name"),
                },
                "epoch": book.date_system() as u32,
                "sheets": sheets,
                "sheet": index,
                "options": options,
                "plan": plan,
                "header": header,
                "numbers": numbers,
                "rows": rows,
            });
            if let Some(error) = failure {
                case["failure"] = failure_json(&error);
            }
            case
        })
        .collect()
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
