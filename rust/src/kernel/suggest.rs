//! Trying a sample's columns against the doors, for a caller who wants to know what to declare.
//! It reads the cells it's handed and writes only the caller's answer, so it runs outside any read
//! and nothing a read does can come to depend on it: nothing is sniffed in a read.
//!
//! Every door is asked of every cell, once, so the work is the size of the sample times the number
//! of doors, with no input able to make it more. Each door is linear in its cell.

use crate::kernel::abi::{ERR_CONTRACT, OK, RawSuggestion, Span, no_format};
use crate::kernel::door::Door;
use hypercast::{
    DateOrder, Fault, NumFormat, RawNumFormat, Reason, cast_bool, cast_date, cast_date_ordered,
    cast_f64, cast_i32, cast_i64, cast_text, cast_timestamp, cast_uuid, i32_from_f64, i64_from_f64,
};

/// What `,` is first taken to mean where no format is declared: it groups digits in threes and `.`
/// is the decimal separator. [`RawSuggestion::NOTATION_COMMA_GROUPS`] names it.
pub const COMMA_GROUPS: NumFormat = NumFormat::INVARIANT;

/// What else `,` can mean where no format is declared: it's the decimal separator and `.` groups
/// digits in threes. [`RawSuggestion::NOTATION_COMMA_DECIMAL`] names it.
pub const COMMA_DECIMAL: NumFormat = NumFormat::new(',', '.', NumFormat::ALL);

// What the header says of the two candidates' flags, a literal there.
const _: () = {
    assert!(COMMA_GROUPS.flags == RawSuggestion::NOTATION_FLAGS);
    assert!(COMMA_DECIMAL.flags == RawSuggestion::NOTATION_FLAGS);
};

/// The set of the two date orders that can't be told apart: month-day-year and day-month-year.
const EITHER_ORDER: u32 =
    (1 << DateOrder::MonthDayYear as u32) | (1 << DateOrder::DayMonthYear as u32);

/// Says in `out` what each column of the sample looks like, one entry a column, with `rows` cells
/// to a column in `cells`, column after column. A flagged span's bytes are in `arena`, and the
/// others in `input`.
///
/// `typed`, where it's given, is a byte for each of `cells`, and one that isn't `0` says the cell
/// was a workbook's typed cell and `cells` holds its canonical text, which the sheet's read said
/// and not a person. A typed number has no notation: the shortest digits of a double, always with
/// `.`, whichever the file's locale. So it counts for every notation, declared or candidate, as
/// what the read will give it, its value through the door's own `f64` twin, and it raises no
/// question: `1.234` as a typed number is `f64`, and `1.234` as text is `1.234`. A column of text
/// and typed numbers still asks what its text asks.
///
/// A blank cell has nothing but white space in it. It doesn't count, and neither `matched` nor
/// `sampled` holds it. A column with nothing else is [`RawSuggestion::UNRESOLVED`], read as text.
///
/// Otherwise the first door in this order that takes every other cell is
/// [`RawSuggestion::RESOLVED`]: bool, `i32`, `i64`, `f64`, an ISO date, a date under a declared
/// order, a timestamp, a UUID. Each is HyperCast's own door, whose leniences are followed, so `0`
/// and `1` are bool, `in` and `out` too, and `1e3` and `+1` are `i32`. Two things that door would
/// take aren't taken here, because the value it gives has less in it than the cell does:
///
/// - A leading zero on a digit, as `007` or `-01.5` has: it's an identifier, and no numeric door
///   takes it.
/// - An integer past `i64`, with no exponent: the `f64` door doesn't take it, since it would lose
///   digits.
///
/// If no door takes them all, and no door takes any, and every cell is valid UTF-8, it's a text
/// column, resolved. Any other way, it's unresolved, read as text.
///
/// Where the cell has a choice of declarations, it's [`RawSuggestion::AMBIGUOUS`] unless the column's
/// other cells settle it:
///
/// - The date orders: a cell whose first field isn't a year fits month-day-year and day-month-year
///   alike, until a field over 12 rules one out. A column whose cells all start with a year is
///   year-month-day.
/// - The notations, only with no format declared. Each of two is a candidate that has to read the
///   whole column by HyperCast's own door: `,` groups digits in threes and `.` is the decimal
///   separator, [`RawSuggestion::NOTATION_COMMA_GROUPS`], or `,` is the decimal separator and `.`
///   groups, [`RawSuggestion::NOTATION_COMMA_DECIMAL`]. Both reading a column with a separator in
///   it is ambiguous, since they read different values: `1,234` and `1.234` are. Both reading one
///   with none is resolved, `param` `0`, since they read the same value: `5` is. Only the first
///   reading it is resolved, with `param` [`RawSuggestion::NOTATION_COMMA_GROUPS`] if a `,` was
///   seen and `0` if not: `1,234,567` and `2.5`. Only the second reading it is resolved, with
///   `param` [`RawSuggestion::NOTATION_COMMA_DECIMAL`]: `2,5`, `0,125`, `1.234,5` and `1.234.567`.
///   Neither reading it, as with `2,5` beside `2.5`, is no door. A binding's plan reads a resolved
///   number under [`RawSuggestion::NOTATION_FLAGS`], in the second notation if `param` names it
///   and otherwise in the first. Both candidates are Western, so `1,23,456` is no door until the
///   column declares Indian grouping: nothing in it says its culture, and a declared format is
///   the door's whole rule.
///
/// [`ERR_CONTRACT`], with nothing written, for `rows` past `u32::MAX`, a `cells` that isn't `rows`
/// for each entry in `out`, a `formats` that isn't one for each, a format that an entry declares
/// and the doors would refuse, a `typed` that isn't one for each of `cells`, or a span reaching past
/// what it points into. An entry of `formats` with a `size` of `0` declares nothing, as `None` does
/// for them all; one that holds a grouping policy all the same is refused.
pub fn suggest(
    input: &[u8],
    arena: &[u8],
    cells: &[Span],
    typed: Option<&[u8]>,
    rows: usize,
    formats: Option<&[RawNumFormat]>,
    out: &mut [RawSuggestion],
) -> i32 {
    if u32::try_from(rows).is_err()
        || rows.checked_mul(out.len()) != Some(cells.len())
        || typed.is_some_and(|typed| typed.len() != cells.len())
        || formats.is_some_and(|formats| formats.len() != out.len())
    {
        return ERR_CONTRACT;
    }
    if formats.is_some_and(|formats| {
        formats.iter().any(|format| match format.size {
            0 => !no_format(format),
            _ => format.resolve().is_none(),
        })
    }) || cells
        .iter()
        .any(|&span| cell_of(input, arena, span).is_none())
    {
        return ERR_CONTRACT;
    }

    let mut rest = cells;
    let mut marks = typed.unwrap_or_default();
    for (at, entry) in out.iter_mut().enumerate() {
        let Some((column, after)) = rest.split_at_checked(rows) else {
            return ERR_CONTRACT;
        };
        rest = after;
        let (kinds, others) = marks.split_at_checked(rows).unwrap_or_default();
        marks = others;
        // A `size` of `0` isn't one `resolve` reads: it declares nothing.
        let declared = formats
            .and_then(|formats| formats.get(at))
            .and_then(|format| format.resolve());
        let mut tally = Tally::default();
        for (at, &span) in column.iter().enumerate() {
            if let Some(cell) = cell_of(input, arena, span) {
                let number = kinds.get(at).is_some_and(|&kind| kind != 0);
                tally.judge(cell, number, declared);
            }
        }
        *entry = tally.finding();
    }
    OK
}

/// The cell a span names, or `None` if it reaches past the end of what it points into.
fn cell_of<'a>(input: &'a [u8], arena: &'a [u8], span: Span) -> Option<&'a [u8]> {
    let from = if span.flagged() { arena } else { input };
    let start = span.offset as usize;
    from.get(start..start.checked_add(span.len())?)
}

/// How many of a column's cells each numeric door takes, under one notation.
#[derive(Default)]
struct Numbers {
    i32: u32,
    i64: u32,
    f64: u32,
}

impl Numbers {
    /// Reads one cell, in `format`.
    fn judge(&mut self, cell: &[u8], format: &NumFormat) {
        if padded(cell, format) {
            return;
        }
        self.i32 += u32::from(cast_i32(cell, format).is_ok());
        let whole = cast_i64(cell, format);
        self.i64 += u32::from(whole.is_ok());
        if !loses_digits(cell, whole, format) {
            self.f64 += u32::from(cast_f64(cell, format).is_ok());
        }
    }

    /// Reads one typed number, as the doors read one: by its value, with no notation to be in.
    fn number(&mut self, value: f64) {
        self.i32 += u32::from(i32_from_f64(value).is_ok());
        self.i64 += u32::from(i64_from_f64(value).is_ok());
        self.f64 += 1;
    }

    /// The first door that takes `sampled` cells.
    fn door(&self, sampled: u32) -> Option<Door> {
        if self.i32 == sampled {
            Some(Door::I32)
        } else if self.i64 == sampled {
            Some(Door::I64)
        } else if self.f64 == sampled {
            Some(Door::F64)
        } else {
            None
        }
    }

    fn most(&self) -> u32 {
        self.i32.max(self.i64).max(self.f64)
    }
}

/// `true` if a digit stands after a leading zero: no number writes one, and an integer door would
/// take `007` to 7. The digits start past any sign, bracket or currency symbol, and a decimal
/// separator ahead of them makes them a fraction, whose leading zeros are its own. A group separator
/// after a lone zero, as `0,125` has, needs nothing here: the door refuses it.
fn padded(cell: &[u8], format: &NumFormat) -> bool {
    let digits = Digits::of(format);
    let mut at = 0;
    while at < cell.len() {
        if let Some((value, width)) = digits.at(cell, at) {
            let ahead = cell.get(..at).unwrap_or_default();
            return value == 0
                && digits.at(cell, at + width).is_some()
                && !ahead.ends_with(format.decimal_sep.as_bytes());
        }
        at += 1;
    }
    false
}

/// `true` if an integer past `i64` is what the cell says. An exponent says it's a float, and this
/// is what the integer door says of a cell whose shape is an integer.
fn loses_digits(cell: &[u8], whole: Result<i64, Fault>, format: &NumFormat) -> bool {
    matches!(whole, Err(fault) if fault.reason == Reason::OutOfRange) && !exponent(cell, format)
}

/// `true` if an `e` or `E` stands inside the numeral: after a digit, and before one, a sign
/// between them allowed. A letter of a declared symbol, as `EUR`'s, is none.
fn exponent(cell: &[u8], format: &NumFormat) -> bool {
    let digits = Digits::of(format);
    let mut after_digit = false;
    let mut at = 0;
    while at < cell.len() {
        if let Some((_, width)) = digits.at(cell, at) {
            after_digit = true;
            at += width;
            continue;
        }
        if after_digit
            && cell
                .get(at)
                .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'e'))
        {
            let rest = cell.get(at + 1..).unwrap_or_default();
            let sign = [
                b"-".as_slice(),
                b"+",
                format.minus_sign.as_bytes(),
                format.plus_sign.as_bytes(),
            ]
            .into_iter()
            .filter(|sign| !sign.is_empty() && rest.starts_with(sign))
            .map(<[u8]>::len)
            .max()
            .unwrap_or(0);
            if digits.at(rest, sign).is_some() {
                return true;
            }
        }
        after_digit = false;
        at += 1;
    }
    false
}

/// The digits a format reads: ASCII `0`–`9`, and its native ten if it declares them. The ten share
/// every UTF-8 byte but the last, which is the zero's plus the digit's value.
struct Digits {
    zero: [u8; 4],
    width: usize,
}

impl Digits {
    fn of(format: &NumFormat) -> Digits {
        let mut zero = [0; 4];
        let width = format
            .digits
            .zero()
            .map_or(0, |zero_char| zero_char.encode_utf8(&mut zero).len());
        Digits { zero, width }
    }

    /// The digit starting at `at`, and how many bytes it takes.
    fn at(&self, cell: &[u8], at: usize) -> Option<(u8, usize)> {
        let first = *cell.get(at)?;
        if first.is_ascii_digit() {
            return Some((first - b'0', 1));
        }
        let unit = cell.get(at..at + self.width)?;
        let (last, prefix) = unit.split_last()?;
        let (zero_last, zero_prefix) = self.zero.get(..self.width)?.split_last()?;
        let value = last.checked_sub(*zero_last)?;
        (prefix == zero_prefix && value < 10).then_some((value, self.width))
    }
}

/// What each door made of a column so far.
#[derive(Default)]
struct Tally {
    /// The cells that aren't blank.
    sampled: u32,
    /// Those that are valid UTF-8.
    text: u32,
    bool: u32,
    /// In the declared format, or [`COMMA_GROUPS`].
    first: Numbers,
    /// In [`COMMA_DECIMAL`], with no format declared: otherwise all zeros, which no column with a
    /// cell to count takes for a door.
    second: Numbers,
    /// Whether a cell had a `,`, with no format declared.
    comma: bool,
    /// Whether a cell had a `.`, with no format declared.
    point: bool,
    date: u32,
    year_first: u32,
    month_first: u32,
    day_first: u32,
    timestamp: u32,
    uuid: u32,
}

impl Tally {
    fn judge(&mut self, cell: &[u8], typed: bool, declared: Option<&NumFormat>) {
        // The door says what's blank: nothing but white space, as every door but text's has it.
        if matches!(cast_bool(cell), Err(fault) if fault.reason == Reason::Empty) {
            return;
        }
        self.sampled += 1;
        self.text += u32::from(cast_text(cell).is_ok());
        self.bool += u32::from(cast_bool(cell).is_ok());
        if typed {
            // Canonical text, read back to the double it names. A typed cell that isn't a number,
            // or a number no door gives, as an infinity is, takes no numeric door.
            if let Ok(value) = cast_f64(cell, &COMMA_GROUPS) {
                self.first.number(value);
                if declared.is_none() {
                    self.second.number(value);
                }
            }
        } else if let Some(format) = declared {
            self.first.judge(cell, format);
        } else {
            self.comma |= cell.contains(&b',');
            self.point |= cell.contains(&b'.');
            self.first.judge(cell, &COMMA_GROUPS);
            self.second.judge(cell, &COMMA_DECIMAL);
        }
        self.date += u32::from(cast_date(cell).is_ok());
        self.year_first += ordered(cell, DateOrder::YearMonthDay);
        self.month_first += ordered(cell, DateOrder::MonthDayYear);
        self.day_first += ordered(cell, DateOrder::DayMonthYear);
        self.timestamp += u32::from(cast_timestamp(cell).is_ok());
        self.uuid += u32::from(cast_uuid(cell).is_ok());
    }

    fn finding(&self) -> RawSuggestion {
        let all = self.sampled;
        let resolved = |door: Door, param: u32| RawSuggestion {
            door: door.code(),
            param,
            status: RawSuggestion::RESOLVED,
            matched: all,
            sampled: all,
        };
        let ambiguous = |door: Door, param: u32| RawSuggestion {
            status: RawSuggestion::AMBIGUOUS,
            ..resolved(door, param)
        };
        if all != 0 {
            if self.bool == all {
                return resolved(Door::Bool, 0);
            }
            // Each notation is a candidate, and the column settles which: both reading it is a
            // question only where they'd read different values, which takes a separator.
            match (self.first.door(all), self.second.door(all)) {
                (Some(door), Some(_)) if self.comma || self.point => {
                    let either = RawSuggestion::NOTATION_COMMA_GROUPS
                        | RawSuggestion::NOTATION_COMMA_DECIMAL;
                    return ambiguous(door, either);
                }
                // The sample proved that `,` groups, and the plan keeps to it.
                (Some(door), None) if self.comma => {
                    return resolved(door, RawSuggestion::NOTATION_COMMA_GROUPS);
                }
                (Some(door), _) => return resolved(door, 0),
                (None, Some(door)) => {
                    return resolved(door, RawSuggestion::NOTATION_COMMA_DECIMAL);
                }
                (None, None) => {}
            }
            if self.date == all {
                return resolved(Door::Date, 0);
            }
            // A cell that fits year-month-day starts with a year, and reads alike in all three.
            let order = |order: DateOrder| Door::DateOrdered(order);
            if self.year_first == all {
                return resolved(
                    order(DateOrder::YearMonthDay),
                    DateOrder::YearMonthDay as u32,
                );
            }
            match (self.month_first == all, self.day_first == all) {
                (true, true) => return ambiguous(order(DateOrder::MonthDayYear), EITHER_ORDER),
                (true, false) => {
                    return resolved(
                        order(DateOrder::MonthDayYear),
                        DateOrder::MonthDayYear as u32,
                    );
                }
                (false, true) => {
                    return resolved(
                        order(DateOrder::DayMonthYear),
                        DateOrder::DayMonthYear as u32,
                    );
                }
                (false, false) => {}
            }
            if self.timestamp == all {
                return resolved(Door::Timestamp, 0);
            }
            if self.uuid == all {
                return resolved(Door::Uuid, 0);
            }
        }
        let most = self.most();
        if all != 0 && most == 0 && self.text == all {
            return resolved(Door::Text, 0);
        }
        RawSuggestion {
            door: Door::Text.code(),
            param: 0,
            status: RawSuggestion::UNRESOLVED,
            matched: most,
            sampled: all,
        }
    }

    /// The most cells any one door but the text door took.
    fn most(&self) -> u32 {
        self.bool
            .max(self.first.most())
            .max(self.second.most())
            .max(self.date)
            .max(self.year_first)
            .max(self.month_first)
            .max(self.day_first)
            .max(self.timestamp)
            .max(self.uuid)
    }
}

fn ordered(cell: &[u8], order: DateOrder) -> u32 {
    u32::from(cast_date_ordered(cell, order).is_ok())
}
