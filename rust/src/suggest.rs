//! Asking what a source's columns look like: a sample read through the text door, judged by
//! [`crate::kernel::suggest`], and handed back as a plan and a report for the caller to look at.
//! Nothing is applied: a read takes only the plan its caller passes it.
//!
//! A sheet's sample binds a second, UUID, door to every column, for what it says of a cell: no door
//! but a text door takes a typed cell's kind, so there it's `Malformed` with no detail, where a text
//! cell that isn't a UUID has one. That's the read's word for which cells were typed, and it's what
//! keeps a typed number out of the question of what `,` and `.` mean.

use crate::column::{Column, MAX_ORDINAL};
use crate::error::Error;
use crate::kernel::abi::{OK, RawSuggestion, Span};
use crate::kernel::door::Door;
use crate::kernel::suggest::{COMMA_DECIMAL, COMMA_GROUPS, suggest as judge};
use crate::{Batch, Header};
use hypercast::{DateOrder, NumFormat, RawNumFormat, Reason};
use std::borrow::Cow;

/// How a sample is taken and read.
///
/// ```
/// use hypertabular::{DelimitedReader, Dialect, NumFormat, SuggestOptions, SuggestStatus};
/// let bytes = b"id,total\n1,\"1,234\"\n";
/// let suggestion =
///     DelimitedReader::suggest_slice(bytes, Dialect::CSV, &SuggestOptions::new())?;
/// assert_eq!(suggestion.report[1].status, SuggestStatus::Ambiguous);
/// let settled = DelimitedReader::suggest_slice(
///     bytes,
///     Dialect::CSV,
///     &SuggestOptions::new().format(NumFormat::INVARIANT),
/// )?;
/// assert_eq!(settled.report[1].status, SuggestStatus::Resolved);
/// # Ok::<(), hypertabular::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestOptions {
    sample_rows: usize,
    format: Option<NumFormat>,
    column_formats: Vec<(usize, NumFormat)>,
    date_order: Option<DateOrder>,
    column_date_orders: Vec<(usize, DateOrder)>,
}

impl Default for SuggestOptions {
    fn default() -> SuggestOptions {
        SuggestOptions::new()
    }
}

impl SuggestOptions {
    /// 1,000 rows, with no notation declared.
    pub const fn new() -> SuggestOptions {
        SuggestOptions {
            sample_rows: 1000,
            format: None,
            column_formats: Vec::new(),
            date_order: None,
            column_date_orders: Vec::new(),
        }
    }

    /// The most rows the sample holds (at least one). It stops at 4096 rows of delimited text, and
    /// at a sheet's own batch size. The core reads them in as many batches as fit: a sample is
    /// never refused for the size of a batch it chose.
    pub const fn sample_rows(mut self, rows: usize) -> SuggestOptions {
        self.sample_rows = if rows == 0 { 1 } else { rows };
        self
    }

    /// Reads every column's numbers in `format`, which settles what `,` and `.` mean in them. A
    /// declared format is the door's whole rule: Indian grouping declared makes `1,23,456` a number
    /// and `1,234,567` text. One [`Column::format`] would refuse is [`Error::Plan`], before the
    /// source is opened.
    pub const fn format(mut self, format: NumFormat) -> SuggestOptions {
        self.format = Some(format);
        self
    }

    /// Reads the numbers of source column `ordinal` in `format`, ahead of [`SuggestOptions::format`],
    /// so one sample can ask of an Indian column beside a Western one. Every format given is judged,
    /// before the source is opened: one [`Column::format`] would refuse is [`Error::Plan`], though a
    /// later one replaces it or the sample never reaches `ordinal`.
    pub fn column_format(mut self, ordinal: usize, format: NumFormat) -> SuggestOptions {
        self.column_formats.push((ordinal, format));
        self
    }

    /// Reads every column's dates in `order` where the sample leaves the order open, as
    /// `01/02/2026` does. A column whose cells rule that order out keeps the order they leave: it
    /// answers what the sample can't, and doesn't overrule what it can.
    pub const fn date_order(mut self, order: DateOrder) -> SuggestOptions {
        self.date_order = Some(order);
        self
    }

    /// Reads the dates of source column `ordinal` in `order`, ahead of
    /// [`SuggestOptions::date_order`], and as that does.
    pub fn column_date_order(mut self, ordinal: usize, order: DateOrder) -> SuggestOptions {
        self.column_date_orders.push((ordinal, order));
        self
    }

    /// The order declared for source column `ordinal`, the last one given if several were.
    fn declared_order(&self, ordinal: usize) -> Option<DateOrder> {
        self.column_date_orders
            .iter()
            .rev()
            .find(|entry| entry.0 == ordinal)
            .map(|entry| entry.1)
            .or(self.date_order)
    }

    /// The format declared for source column `ordinal`, the last one given if several were.
    fn declared(&self, ordinal: usize) -> Option<NumFormat> {
        self.column_formats
            .iter()
            .rev()
            .find(|entry| entry.0 == ordinal)
            .map(|entry| entry.1)
            .or(self.format)
    }

    /// Every format these options declare, judged as a plan's would be, whichever would win for a
    /// column and whether or not the sample reaches its ordinal. A suggest asks this before it opens
    /// or reads anything, so a stale format is [`Error::Plan`] on an empty source and on a broken one.
    pub(crate) fn check_formats(&self) -> Result<(), Error> {
        if let Some(format) = self.format {
            Column::i32(0).format(format).spec(0)?;
        }
        for &(ordinal, format) in &self.column_formats {
            Column::i32(0).format(format).spec(ordinal)?;
        }
        Ok(())
    }

    /// How many rows to take, where a batch is `batch_rows`.
    pub(crate) fn rows_within(&self, batch_rows: usize) -> usize {
        self.sample_rows.min(batch_rows.max(1))
    }
}

/// What a sample says of a column.
///
/// New statuses may be added in a minor release; matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SuggestStatus {
    /// One door and one declaration take every cell, and the plan reads the column through them.
    /// That can be a notation the sample left as the only one, which the caller didn't declare:
    /// [`SuggestedColumn::format`] names it.
    Resolved,
    /// Every cell is read, but the sample can't say which declaration it's written in, as `1,234`
    /// and `1.234` can't, a `.` or a `,` reading as a decimal or as a group:
    /// [`SuggestedColumn::candidates`] are the choices. The plan reads the column as text until
    /// the caller chooses.
    Ambiguous,
    /// There's nothing to go on, or a cell no door took beside cells one did, or a cell that
    /// isn't text at all. The plan reads the column as text.
    Unresolved,
}

/// A declaration that would settle an [`SuggestStatus::Ambiguous`] column.
///
/// New kinds may be added in a minor release; matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Candidate {
    /// The date's fields come in this order.
    DateOrder(DateOrder),
    /// The numbers are in this notation. Declare it with [`SuggestOptions::column_format`] and ask
    /// again: the door can differ under each, as `1,234` is an `i32` under one and an `f64` under
    /// the other.
    NumFormat(NumFormat),
}

/// What a sample says of one source column.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SuggestedColumn {
    /// The zero-based source column, as a [`Column`] names it.
    pub ordinal: usize,
    /// The header's name for it, if the source has a header.
    pub name: Option<Vec<u8>>,
    /// The door found. Text when unresolved, and under the first candidate when ambiguous.
    pub door: Door,
    /// The notation the plan reads a resolved numeric column in: the one declared for it, or else
    /// the one candidate the sample left, `,` as the decimal separator where only that reads the
    /// column, and otherwise [`NumFormat::INVARIANT`], whether or not the sample held a `,`. Both
    /// are Western, so a later `1,23,456` is refused. `None` for any other column.
    pub format: Option<NumFormat>,
    /// What to choose between: empty unless ambiguous.
    pub candidates: Vec<Candidate>,
    /// How far the sample settles it.
    pub status: SuggestStatus,
    /// The cells [`SuggestedColumn::door`] took. Resolved or ambiguous, that's all of `sampled`.
    /// Unresolved, it's the most that any one door but the text door took.
    pub matched: u32,
    /// The cells that aren't blank.
    pub sampled: u32,
}

/// What [`DelimitedReader::suggest`](crate::DelimitedReader::suggest),
/// [`DelimitedReader::suggest_slice`](crate::DelimitedReader::suggest_slice) and
/// [`Sheet::suggest`](crate::Sheet::suggest) found: a plan to change or to pass to a read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Suggestion {
    /// A column for each source column, in order. Only a resolved column reads through its door:
    /// the rest read as text, which refuses nothing and loses nothing.
    pub plan: Vec<Column>,
    /// A report for each source column, in the plan's order.
    pub report: Vec<SuggestedColumn>,
    /// Whether the source has rows past the sample. A source with exactly the sample's rows hasn't.
    pub truncated: bool,
}

/// A read a sample can come from.
pub(crate) trait SampleSource {
    fn next_batch(&mut self) -> Result<Option<Batch<'_>>, Error>;
}

/// The sample, in the shape the core takes it.
pub(crate) struct Sample {
    bytes: Vec<u8>,
    columns: Vec<Vec<Span>>,
    /// A byte for each span, in the same shape: not `0` for a typed cell. Empty but for a sheet.
    typed: Vec<Vec<u8>>,
    rows: usize,
    limit: usize,
    truncated: bool,
}

impl Sample {
    pub(crate) fn new(columns: usize, limit: usize) -> Sample {
        Sample {
            bytes: Vec::new(),
            columns: vec![Vec::new(); columns],
            typed: Vec::new(),
            rows: 0,
            limit,
            truncated: false,
        }
    }

    /// A sheet's sample: `columns` text columns, and after them a UUID column for each.
    pub(crate) fn of_sheet(columns: usize, limit: usize) -> Sample {
        Sample {
            typed: vec![Vec::new(); columns],
            ..Sample::new(columns, limit)
        }
    }

    /// The plan a sheet's sample is read through, for [`Sample::of_sheet`].
    pub(crate) fn sheet_plan(width: usize) -> Vec<Column> {
        (0..width)
            .map(Column::text)
            .chain((0..width).map(Column::uuid))
            .collect()
    }

    /// Reads until the sample is whole and a row past it, or the end, says whether it's the
    /// source's last. A failure among the sampled rows is the caller's, and a failure just past
    /// them only says there's more.
    pub(crate) fn read_from(&mut self, source: &mut impl SampleSource) -> Result<(), Error> {
        while !self.truncated {
            match source.next_batch() {
                Ok(Some(batch)) => self.take(&batch),
                Ok(None) => return Ok(()),
                Err(_) if self.rows == self.limit => self.truncated = true,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Takes what the sample still wants of `batch`. A blank, a token and padding are all a
    /// blank span. A refused cell keeps its bytes, for the core's own text door to refuse.
    fn take(&mut self, batch: &Batch<'_>) {
        for row in 0..batch.rows() {
            if self.rows == self.limit {
                self.truncated = true;
                return;
            }
            let cells: Vec<Cow<'_, [u8]>> = (0..self.columns.len())
                .map(|column| match batch.text(column, row) {
                    Ok(text) => Cow::Borrowed(text),
                    Err(fault) if fault.reason == Reason::Empty => Cow::Borrowed(&[][..]),
                    Err(_) => batch.raw(column, row),
                })
                .collect();
            // A span holds a `u32` offset. A sample that far along stops, and says it did.
            let length: usize = cells.iter().map(|cell| cell.len()).sum();
            if self.bytes.len() + length > u32::MAX as usize {
                self.truncated = true;
                return;
            }
            for (column, cell) in self.columns.iter_mut().zip(&cells) {
                column.push(Span {
                    offset: self.bytes.len() as u32,
                    len: cell.len() as u32,
                });
                self.bytes.extend_from_slice(cell);
            }
            let width = self.columns.len();
            for (column, marks) in self.typed.iter_mut().enumerate() {
                let fault = batch.verdicts(width + column)[row].fault();
                let typed = fault.is_some_and(|fault| fault.reason == Reason::Malformed)
                    && batch.detail(width + column, row).is_none();
                marks.push(u8::from(typed));
            }
            self.rows += 1;
        }
    }

    /// Asks the core, and builds the plan and the report from its answer.
    pub(crate) fn judge(
        self,
        header: Option<&Header>,
        options: &SuggestOptions,
    ) -> Result<Suggestion, Error> {
        let width = self.columns.len();
        let declared: Vec<Option<NumFormat>> = (0..width)
            .map(|ordinal| options.declared(ordinal))
            .collect();
        let formats: Vec<RawNumFormat> = declared
            .iter()
            .map(|format| match format {
                Some(format) => RawNumFormat::from(*format),
                None => RawNumFormat {
                    size: 0,
                    ..RawNumFormat::ZERO
                },
            })
            .collect();
        let cells: Vec<Span> = self.columns.iter().flatten().copied().collect();
        let typed: Vec<u8> = self.typed.iter().flatten().copied().collect();
        let mut out = vec![RawSuggestion::default(); width];
        let code = judge(
            &self.bytes,
            &[],
            &cells,
            (!self.typed.is_empty()).then_some(&typed[..]),
            self.rows,
            Some(&formats),
            &mut out,
        );
        if code != OK {
            // Every format was checked and every span was made here.
            return Err(Error::Plan { column: 0 });
        }
        let mut plan = Vec::with_capacity(width);
        let mut report = Vec::with_capacity(width);
        for (ordinal, (found, format)) in out.iter().zip(&declared).enumerate() {
            let (mut status, mut candidates) = read_status(found);
            let mut door = read_door(found);
            // An order declared for a date the sample left open is the answer to its question.
            if let Some(order) = options.declared_order(ordinal)
                && candidates.contains(&Candidate::DateOrder(order))
            {
                status = SuggestStatus::Resolved;
                candidates = Vec::new();
                door = Door::DateOrdered(order);
            }
            let planned = (status == SuggestStatus::Resolved && door.is_numeric()).then(|| {
                format.unwrap_or(if found.param == RawSuggestion::NOTATION_COMMA_DECIMAL {
                    COMMA_DECIMAL
                } else {
                    COMMA_GROUPS
                })
            });
            let mut column = Column::text(ordinal);
            if status == SuggestStatus::Resolved {
                column = Column::new(ordinal, door);
                if let Some(format) = format.or(planned) {
                    column = column.format(format);
                }
            }
            plan.push(column);
            report.push(SuggestedColumn {
                ordinal,
                name: header
                    .and_then(|names| names.name(ordinal))
                    .map(<[u8]>::to_vec),
                door,
                format: planned,
                candidates,
                status,
                matched: found.matched,
                sampled: found.sampled,
            });
        }
        Ok(Suggestion {
            plan,
            report,
            truncated: self.truncated,
        })
    }
}

/// The widest source a suggestion can plan for, as a plan says it.
pub(crate) fn check_width(width: usize) -> Result<(), Error> {
    if width > MAX_ORDINAL + 1 {
        return Err(Error::Plan {
            column: MAX_ORDINAL + 1,
        });
    }
    Ok(())
}

pub(crate) fn read_status(found: &RawSuggestion) -> (SuggestStatus, Vec<Candidate>) {
    match found.status {
        RawSuggestion::RESOLVED => (SuggestStatus::Resolved, Vec::new()),
        RawSuggestion::AMBIGUOUS => (SuggestStatus::Ambiguous, read_candidates(found)),
        _ => (SuggestStatus::Unresolved, Vec::new()),
    }
}

fn read_candidates(found: &RawSuggestion) -> Vec<Candidate> {
    let has = |bit: u32| found.param & bit != 0;
    let date = [
        Door::DateOrdered(DateOrder::MonthDayYear),
        Door::DateTime(DateOrder::MonthDayYear),
    ];
    if date.iter().any(|door| door.code() == found.door) {
        [
            DateOrder::YearMonthDay,
            DateOrder::MonthDayYear,
            DateOrder::DayMonthYear,
        ]
        .into_iter()
        .filter(|order| has(1 << *order as u32))
        .map(Candidate::DateOrder)
        .collect()
    } else {
        [
            (RawSuggestion::NOTATION_COMMA_GROUPS, COMMA_GROUPS),
            (RawSuggestion::NOTATION_COMMA_DECIMAL, COMMA_DECIMAL),
        ]
        .into_iter()
        .filter(|entry| has(entry.0))
        .map(|entry| Candidate::NumFormat(entry.1))
        .collect()
    }
}

/// The door a suggestion names. An ambiguous date's `param` is a set: the door it names is the
/// first of the set's orders.
fn read_door(found: &RawSuggestion) -> Door {
    let param = if found.status == RawSuggestion::AMBIGUOUS {
        found.param.trailing_zeros()
    } else {
        found.param
    };
    Door::from_code(found.door, param).unwrap_or(Door::Text)
}
