//! A batch's verdicts and notes as a caller keeps them after the reader has moved on, and the
//! batch's faults list worked out from them when it's asked for.
//!
//! A [`Batch`](crate::Batch) is a view the next read overwrites, so a binding that hands a batch to
//! code outliving the read copies it. The faults list is the core's to work out, from each column's
//! verdicts and notes, the batch's lines, and the widths of a read that reports long rows: copy
//! those ([`Batch::verdicts`](crate::Batch::verdicts), [`Batch::notes`](crate::Batch::notes),
//! [`Batch::line`](crate::Batch::line), [`Batch::reported_widths`](crate::Batch::reported_widths),
//! [`Batch::source_column`](crate::Batch::source_column)), and [`KeptBatch::faults`] lists the
//! problems from the copies whenever it's called, the same list [`Batch::faults`](crate::Batch::faults)
//! gave, at no cost to the read until then.
//!
//! ```
//! # use hypertabular::{CellNote, CellVerdict, Column, DelimitedReader, Dialect, KeptBatch, KeptColumn};
//! let plan = [Column::text(0), Column::i32(1)];
//! let mut reader = DelimitedReader::from_slice(b"name,n\na,1\nb,x\n", Dialect::CSV, &plan)?;
//! let batch = reader.read()?.unwrap();
//! // What a binding keeps: owned copies, which outlive the read.
//! let verdicts: Vec<Vec<CellVerdict>> = (0..2).map(|c| batch.verdicts(c).to_vec()).collect();
//! let notes: Vec<Vec<CellNote>> = (0..2).map(|c| batch.notes(c).to_vec()).collect();
//! let lines: Vec<u32> = (0..batch.rows()).map(|row| batch.line(row)).collect();
//! let sources: Vec<usize> = (0..2).map(|c| batch.source_column(c)).collect();
//! let widths = batch.reported_widths().map(|(width, rows)| (width, rows.to_vec()));
//! let expected: Vec<_> = batch.faults().collect();
//! drop(reader);
//!
//! let columns: Vec<KeptColumn> = (0..2)
//!     .map(|c| KeptColumn::new(&verdicts[c], &notes[c], sources[c]))
//!     .collect();
//! let long_rows = widths.as_ref().map(|(width, rows)| (*width, rows.as_slice()));
//! let kept = KeptBatch::new(&lines, &columns, long_rows).unwrap();
//! assert_eq!(kept.faults().collect::<Vec<_>>(), expected);
//! # Ok::<(), hypertabular::Error>(())
//! ```

use crate::batch::{Origin, Problem};
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBufferWith, OK, RawDelimitedOptions, RawProblem,
};
use crate::kernel::faults::faults;
use hypercast::{Detail, Fault, RawCheck};

/// One plan column of a batch as a caller kept it: its verdicts, one a row, its notes, one a row or
/// none for a read that takes none, and the source column it reads.
#[derive(Clone, Copy, Debug)]
pub struct KeptColumn<'a> {
    verdicts: &'a [CellVerdict],
    notes: &'a [CellNote],
    checks: &'a [RawCheck],
    source_column: usize,
}

impl<'a> KeptColumn<'a> {
    /// A column from what [`Batch::verdicts`](crate::Batch::verdicts),
    /// [`Batch::notes`](crate::Batch::notes) and
    /// [`Batch::source_column`](crate::Batch::source_column) gave for it.
    pub fn new(verdicts: &'a [CellVerdict], notes: &'a [CellNote], source_column: usize) -> Self {
        KeptColumn {
            verdicts,
            notes,
            checks: &[],
            source_column,
        }
    }

    /// The same column with the checks [`Batch::checks`](crate::Batch::checks) gave for it, so its
    /// faults list names the keyword each violation broke. Without them a violation isn't listed,
    /// as [`Batch::faults`](crate::Batch::faults) lists none for a column with no checks.
    pub fn with_checks(self, checks: &'a [RawCheck]) -> Self {
        KeptColumn { checks, ..self }
    }

    /// The verdict at `row`: `Ok` for a cell that cast, or its fault. `None` for a row the column
    /// doesn't have. A cell that cast and broke its column's constraints is `Ok`, as
    /// [`CellVerdict::fault`](crate::kernel::abi::CellVerdict::fault) reads it: its checks say what
    /// it broke. A reason code HyperCast doesn't define, which no read writes, is `None` too.
    pub fn verdict(&self, row: usize) -> Option<Result<(), Fault>> {
        let verdict = self.verdicts.get(row)?;
        if verdict.is_ok() || verdict.reason == CellVerdict::VIOLATION {
            return Some(Ok(()));
        }
        verdict.fault().map(Err)
    }

    /// What failed in the cell at `row`, as [`Batch::detail`](crate::Batch::detail) says it: `None`
    /// for a cell that cast, an `Empty` no door gave, a read with notes off, and a row the column
    /// doesn't have.
    pub fn detail(&self, row: usize) -> Option<Detail> {
        Detail::from_code(u32::from(self.notes.get(row)?.detail))
    }

    /// Where the verdict at `row` came from, as [`Batch::origin`](crate::Batch::origin) says it:
    /// [`Origin::Cell`] for a read with notes off, and for a row the column doesn't have.
    pub fn origin(&self, row: usize) -> Origin {
        self.notes
            .get(row)
            .map_or(Origin::Cell, |note| Origin::from_code(note.origin))
    }

    /// How many of the column's cells didn't cast. A cell that cast and broke its column's
    /// constraints isn't one.
    pub fn fault_count(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|verdict| verdict.fault().is_some())
            .count()
    }
}

/// A batch as a caller kept it, from which its faults list is worked out: [`KeptBatch::faults`].
#[derive(Clone, Copy, Debug)]
pub struct KeptBatch<'a> {
    rows: usize,
    lines: &'a [u32],
    columns: &'a [KeptColumn<'a>],
    long_rows: Option<(u32, &'a [u32])>,
}

impl<'a> KeptBatch<'a> {
    /// A batch from each row's [`Batch::line`](crate::Batch::line), one a row, its plan columns,
    /// and what [`Batch::reported_widths`](crate::Batch::reported_widths) gave.
    ///
    /// `None` unless each column's verdicts, and its notes and checks when it has any, hold as many elements
    /// as `lines`, the widths at least as many, and the row and column counts fit a `u32`.
    pub fn new(
        lines: &'a [u32],
        columns: &'a [KeptColumn<'a>],
        long_rows: Option<(u32, &'a [u32])>,
    ) -> Option<Self> {
        let rows = lines.len();
        let fits = u32::try_from(rows).is_ok() && u32::try_from(columns.len()).is_ok();
        let whole = columns.iter().all(|column| {
            column.verdicts.len() == rows
                && (column.notes.is_empty() || column.notes.len() == rows)
                && (column.checks.is_empty() || column.checks.len() == rows)
        }) && long_rows.is_none_or(|(_, widths)| widths.len() >= rows);
        (fits && whole).then_some(KeptBatch {
            rows,
            lines,
            columns,
            long_rows,
        })
    }

    /// How many rows the batch holds.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The batch's problems, in the order and on the terms of
    /// [`Batch::faults`](crate::Batch::faults), worked out from the kept copies as they're read.
    pub fn faults(&self) -> KeptFaults<'a> {
        KeptFaults {
            batch: *self,
            buffers: self
                .columns
                .iter()
                .map(|column| ColumnBufferWith {
                    size: ColumnBufferWith::SIZE,
                    values: std::ptr::null_mut(),
                    verdicts: column.verdicts.as_ptr().cast_mut(),
                    notes: if column.notes.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        column.notes.as_ptr().cast_mut()
                    },
                    checks: if column.checks.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        column.checks.as_ptr().cast_mut()
                    },
                })
                .collect(),
            cursor: 0,
            held: [RawProblem::default(); FAULTS_HELD],
            at: 0,
            len: 0,
            done: false,
        }
    }
}

/// Entries of the list a [`KeptFaults`] holds at a time.
const FAULTS_HELD: usize = 16;

/// The problems of a [`KeptBatch`], in order: [`KeptBatch::faults`].
pub struct KeptFaults<'a> {
    batch: KeptBatch<'a>,
    /// The columns as the core takes them, pointing into `batch`'s borrowed slices.
    buffers: Vec<ColumnBufferWith>,
    cursor: u64,
    held: [RawProblem; FAULTS_HELD],
    at: usize,
    len: usize,
    done: bool,
}

impl KeptFaults<'_> {
    fn fill(&mut self) {
        let batch = self.batch;
        let (long_rows, expected, widths) = match batch.long_rows {
            Some((width, widths)) => (RawDelimitedOptions::LONG_ROWS_REPORT, width, widths),
            None => (RawDelimitedOptions::LONG_ROWS_REJECT, 0, &[][..]),
        };
        let mut written = 0;
        // SAFETY: each buffer points into a column's verdicts, and its notes and checks or null, which
        // `KeptBatch::new` checked hold `rows` elements and which this iterator borrows for its life;
        // the core only reads them.
        let code = unsafe {
            faults(
                &self.buffers,
                batch.rows,
                widths,
                expected,
                long_rows,
                &mut self.cursor,
                &mut self.held,
                &mut written,
            )
        };
        (self.at, self.len) = (0, if code == OK { written } else { 0 });
        self.done = code != OK || written < FAULTS_HELD;
    }
}

impl Iterator for KeptFaults<'_> {
    type Item = Problem;

    fn next(&mut self) -> Option<Problem> {
        if self.at == self.len {
            if self.done {
                return None;
            }
            self.fill();
            if self.len == 0 {
                return None;
            }
        }
        let raw = *self.held.get(self.at)?;
        self.at += 1;
        let batch = self.batch;
        // A row-level entry (a long row) has no column, so no source column.
        let source = if raw.plan_column == RawProblem::ROW_LEVEL {
            None
        } else {
            batch
                .columns
                .get(raw.plan_column as usize)
                .map(|column| column.source_column)
        };
        let line = batch.lines.get(raw.row as usize).copied().unwrap_or(0);
        Some(Problem::from_raw(raw, line, source))
    }
}

impl std::iter::FusedIterator for KeptFaults<'_> {}

// SAFETY: the raw pointers in `buffers` point into the `'a` slices of `CellVerdict` and `CellNote`
// that `batch` borrows, which are plain integers and `Sync`, and the core only reads through them:
// the iterator is as safe to send or share as those `&'a` slices, which are `Send` and `Sync`.
unsafe impl Send for KeptFaults<'_> {}
// SAFETY: as for `Send`; `&KeptFaults` gives no access to the pointers at all.
unsafe impl Sync for KeptFaults<'_> {}

impl std::fmt::Debug for KeptFaults<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptFaults")
            .field("cursor", &self.cursor)
            .finish()
    }
}
