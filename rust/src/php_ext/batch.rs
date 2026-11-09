//! `Batch`, `Row` and `RowIterator`: the rows one read delivered, as typed columns, the same
//! classes for delimited text and for a sheet of a workbook.
//!
//! A batch owns what it shows: the arrays the Rust reader's batch holds are copied out as it is
//! made, so it stays good after the reader has moved on. A column is lifted out of its arrays the
//! first time it is asked for, into PHP values HyperCast's own interop surface builds
//! (`hypercast.rs`), and kept: the values as a list, null where a cell did not cast, and the
//! faults keyed by row.

use std::cell::{OnceCell, RefCell, RefMut};
use std::rc::Rc;
use std::sync::Arc;

use ext_php_rs::convert::{FromZval, IntoZval};
use ext_php_rs::error::Result as ZvalResult;
use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::flags::DataType;
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, ZendHashTable, Zval};
use ext_php_rs::zend::ce;

use super::args::{Int, OptInt};
use super::classes::{array_zval, bool_zval, bytes_zval, call, float_zval, int_zval, list_zval};
use super::enums::{Door, Origin};
use super::errors::{contract_violation, logic, out_of_range, pending};
use super::guard;
use super::hypercast::{Judge, detailed_fault};
use super::plan::Plan;
use super::results::problem;
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBufferWith, OK, RawDelimitedOptions, RawProblem, Span,
};
use crate::kernel::delimited::unescape::unescape_into;

/// The low 31 bits of a span's length; the top bit is its flag.
const SPAN_LENGTH: u32 = !Span::FLAG;

/// A return value of a declared class type, or null when `NULLABLE`.
macro_rules! typed_return {
    ($(#[$doc:meta])* $name:ident, $class:literal, $nullable:literal) => {
        $(#[$doc])*
        pub struct $name(pub Option<Zval>);

        impl ::ext_php_rs::convert::IntoZval for $name {
            const TYPE: ::ext_php_rs::flags::DataType = ::ext_php_rs::flags::DataType::object($class);
            const NULLABLE: bool = $nullable;

            fn set_zval(
                self,
                zv: &mut ::ext_php_rs::types::Zval,
                _: bool,
            ) -> ::ext_php_rs::error::Result<()> {
                match self.0 {
                    Some(value) => *zv = value,
                    None => zv.set_null(),
                }
                Ok(())
            }
        }
    };
}
pub(crate) use typed_return;

typed_return!(
    /// `Success|Fault`.
    Verdict,
    "HyperCast\\Success|HyperCast\\Fault",
    false
);
typed_return!(
    /// `?Fault`.
    MaybeFault,
    "HyperCast\\Fault",
    true
);
typed_return!(
    /// `?DetailedFault`.
    MaybeDetailedFault,
    "HyperCast\\DetailedFault",
    true
);
typed_return!(
    /// `Batch`.
    BatchObject,
    "HyperTabular\\Batch",
    false
);
typed_return!(
    /// `?Batch`.
    MaybeBatch,
    "HyperTabular\\Batch",
    true
);
typed_return!(
    /// `\Iterator`.
    IteratorObject,
    "Iterator",
    false
);

/// An `array` return value.
pub struct Array(pub Zval);

impl IntoZval for Array {
    const TYPE: DataType = DataType::Array;
    const NULLABLE: bool = false;

    fn set_zval(self, zv: &mut Zval, _: bool) -> ZvalResult<()> {
        *zv = self.0;
        Ok(())
    }
}

/// A `?array` return value.
pub struct MaybeArray(pub Option<Zval>);

impl IntoZval for MaybeArray {
    const TYPE: DataType = DataType::Array;
    const NULLABLE: bool = true;

    fn set_zval(self, zv: &mut Zval, _: bool) -> ZvalResult<()> {
        match self.0 {
            Some(value) => *zv = value,
            None => zv.set_null(),
        }
        Ok(())
    }
}

/// A `mixed` return value.
pub struct MixedValue(pub Zval);

impl IntoZval for MixedValue {
    const TYPE: DataType = DataType::Mixed;
    const NULLABLE: bool = true;

    fn set_zval(self, zv: &mut Zval, _: bool) -> ZvalResult<()> {
        *zv = self.0;
        Ok(())
    }
}

/// A `string` return value of bytes that need not be UTF-8.
pub struct Bytes(pub Vec<u8>);

impl IntoZval for Bytes {
    const TYPE: DataType = DataType::String;
    const NULLABLE: bool = false;

    fn set_zval(self, zv: &mut Zval, _: bool) -> ZvalResult<()> {
        *zv = bytes_zval(&self.0);
        Ok(())
    }
}

/// One decoded column.
struct Decoded {
    /// The values, by row, null where a cell did not cast.
    values: Zval,
    /// The faults, keyed by row.
    faults: Zval,
    /// The same values and faults, by row, for a cell's answer.
    cells: Vec<Zval>,
    fault_of: Vec<Option<Zval>>,
}

/// Where a batch's rows were read from, which says how the batch keeps what an unflagged span
/// indexes.
pub(crate) enum Holds<'a> {
    /// The string a reader was given, shared and never copied.
    Shared(&'a Rc<Vec<u8>>),
    /// A stream, or a decoded string, whose window the next read overwrites: copied.
    Window,
    /// A workbook, whose shared strings every batch of every sheet indexes.
    Book(&'a Arc<crate::Workbook<'static>>),
}

/// What an unflagged span indexes.
enum Base {
    Shared {
        bytes: Rc<Vec<u8>>,
        start: usize,
        len: usize,
    },
    Owned(Vec<u8>),
    Book(Arc<crate::Workbook<'static>>),
}

impl Base {
    fn bytes(&self) -> &[u8] {
        match self {
            Base::Shared { bytes, start, len } => start
                .checked_add(*len)
                .and_then(|end| bytes.get(*start..end))
                .unwrap_or_default(),
            Base::Owned(bytes) => bytes,
            Base::Book(book) => book.strings(),
        }
    }
}

/// What a batch shows, shared by the batch and its rows.
pub(crate) struct BatchCore {
    plan: Zval,
    doors: Vec<Door>,
    ordinals: Vec<usize>,
    /// The source column each plan column reads, a sheet's own counted from its first.
    sources: Vec<usize>,
    rows: usize,
    values: Vec<Vec<u8>>,
    verdicts: Vec<Vec<CellVerdict>>,
    /// Each column's notes, or none for a column that has no fault or a read that takes no notes.
    notes: Vec<Vec<CellNote>>,
    cells: Vec<Span>,
    per_row: usize,
    base: Base,
    /// What a flagged span indexes, as far as any of the batch's spans reach.
    arena: Vec<u8>,
    workbook: bool,
    /// The width a read that reports long rows fixed, and each row's.
    widths: Option<(u32, Vec<u32>)>,
    decoded: RefCell<Vec<Option<Rc<Decoded>>>>,
    /// `faults()`, made the first time it is asked for.
    problems: OnceCell<Zval>,
}

/// `length` bytes of `from` at `offset`, as much of them as there is, as PHP's `substr`.
fn substr(from: &[u8], offset: u32, length: u32) -> &[u8] {
    let start = (offset as usize).min(from.len());
    let end = start.saturating_add(length as usize).min(from.len());
    &from[start..end]
}

fn word<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(slice) = bytes.get(at..at + N) {
        out.copy_from_slice(slice);
    }
    out
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(word(bytes, at))
}

fn i64_at(bytes: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(word(bytes, at))
}

impl BatchCore {
    /// A copy of the Rust reader's `batch`, read through `plan`.
    pub(crate) fn of(plan: &Plan, batch: &crate::Batch<'_>, holds: Holds<'_>) -> BatchCore {
        let (cells, per_row, base, arena) = batch.tables();
        let workbook = matches!(holds, Holds::Book(_));
        let count = plan.doors.len();
        // The arena as far as any span of this batch reaches into it: text with an escaped quote
        // in it, or for a sheet the text typed cells were said as, and the raw text of those that
        // failed.
        let mut reach = 0;
        let mut values = Vec::with_capacity(count);
        let mut verdicts = Vec::with_capacity(count);
        let mut notes = Vec::with_capacity(count);
        let mut sources = Vec::with_capacity(count);
        for (index, door) in plan.doors.iter().enumerate() {
            let (bytes, judged) = batch.column_bytes(index);
            if matches!(door, Door::Text) {
                for span in bytes.as_chunks::<{ size_of::<Span>() }>().0 {
                    let (offset, length) = (u32_at(span, 0), u32_at(span, 4));
                    if length & Span::FLAG != 0 {
                        reach = reach.max(offset as usize + (length & SPAN_LENGTH) as usize);
                    }
                }
            }
            values.push(bytes.to_vec());
            verdicts.push(judged.to_vec());
            notes.push(if judged.iter().all(CellVerdict::is_ok) {
                Vec::new()
            } else {
                batch.notes(index).to_vec()
            });
            sources.push(batch.source_column(index));
        }
        if workbook {
            for cell in cells.iter().filter(|cell| cell.flagged()) {
                reach = reach.max(cell.offset as usize + cell.len());
            }
        }
        let base = match holds {
            Holds::Shared(bytes) => Base::Shared {
                start: (base.as_ptr() as usize).saturating_sub(bytes.as_ptr() as usize),
                len: base.len(),
                bytes: Rc::clone(bytes),
            },
            Holds::Window => {
                // Every row's last entry says where it came from, and isn't in the text.
                let end = cells
                    .chunks_exact(per_row.max(1))
                    .flat_map(|row| row.get(..per_row - 1).unwrap_or_default())
                    .map(|cell| cell.offset as usize + cell.len())
                    .max()
                    .unwrap_or(0);
                Base::Owned(base.get(..end).unwrap_or(base).to_vec())
            }
            Holds::Book(book) => Base::Book(Arc::clone(book)),
        };
        BatchCore {
            plan: plan.list.shallow_clone(),
            doors: plan.doors.clone(),
            ordinals: plan.ordinals.clone(),
            sources,
            rows: batch.rows(),
            values,
            verdicts,
            notes,
            cells: cells.to_vec(),
            per_row,
            base,
            arena: arena.get(..reach).unwrap_or(arena).to_vec(),
            workbook,
            widths: batch
                .reported_widths()
                .map(|(width, widths)| (width, widths.to_vec())),
            decoded: RefCell::new(vec![None; count]),
            problems: OnceCell::new(),
        }
    }

    /// Refuses a row the batch does not have.
    fn check_row(&self, row: i64) -> PhpResult<usize> {
        usize::try_from(row)
            .ok()
            .filter(|row| *row < self.rows)
            .ok_or_else(|| {
                out_of_range(format!(
                    "The batch has {} rows; there is no row {row}",
                    self.rows
                ))
            })
    }

    /// Refuses a column the plan does not have.
    fn check_column(&self, column: i64) -> PhpResult<usize> {
        let count = self.doors.len();
        usize::try_from(column)
            .ok()
            .filter(|column| *column < count)
            .ok_or_else(|| {
                out_of_range(format!(
                    "The plan has {count} columns; there is no column {column}"
                ))
            })
    }

    /// The cell-table entry at `at`.
    fn cell(&self, at: usize) -> Span {
        self.cells.get(at).copied().unwrap_or_default()
    }

    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    pub(crate) fn line(&self, row: i64) -> PhpResult<i64> {
        let row = self.check_row(row)?;
        Ok(i64::from(self.line_of(row)))
    }

    /// The line a row of the batch came from, or for a sheet its row number.
    fn line_of(&self, row: usize) -> u32 {
        let entry = self.cell(row * self.per_row + self.per_row - 1);
        if self.workbook {
            entry.offset
        } else {
            entry.len
        }
    }

    /// The column, decoded, unless it already has been.
    fn decode(&self, column: i64) -> PhpResult<Rc<Decoded>> {
        if let Some(Some(done)) = usize::try_from(column)
            .ok()
            .and_then(|c| self.decoded.borrow().get(c).cloned())
        {
            return Ok(done);
        }
        let column = self.check_column(column)?;
        let judge = Judge::find()?;
        let rows = self.rows;
        let verdicts = &self.verdicts[column];
        let mut fault_of: Vec<Option<Zval>> = Vec::with_capacity(rows);
        let mut faults = ZendHashTable::new();
        for (row, verdict) in verdicts.iter().enumerate().take(rows) {
            if verdict.reason != 0 {
                let fault = judge.fault(verdict.reason, verdict.offset, verdict.len)?;
                faults
                    .insert_at_index(row as i64, fault.shallow_clone())
                    .map_err(|_| pending("array insert"))?;
                fault_of.push(Some(fault));
            } else {
                fault_of.push(None);
            }
        }
        let bytes = &self.values[column];
        let door = self.doors[column];
        let mut cells = Vec::with_capacity(rows);
        for (row, fault) in fault_of.iter().enumerate() {
            if fault.is_some() {
                cells.push(Zval::new());
                continue;
            }
            cells.push(self.value(&judge, door, bytes, row)?);
        }
        let mut values = Vec::with_capacity(rows);
        for cell in &cells {
            values.push(cell.shallow_clone());
        }
        let decoded = Rc::new(Decoded {
            values: list_zval(values)?,
            faults: array_zval(faults),
            cells,
            fault_of,
        });
        self.decoded.borrow_mut()[column] = Some(Rc::clone(&decoded));
        Ok(decoded)
    }

    /// The value of one cell that cast: the carrier HyperCast's `Cast` returns for the door.
    /// The layouts are `kernel::abi`'s, little-endian, and the padding in them is masked off
    /// rather than trusted to be zero.
    fn value(&self, judge: &Judge, door: Door, bytes: &[u8], row: usize) -> PhpResult<Zval> {
        let size = door.value_size();
        let at = row * size;
        Ok(match door {
            Door::Bool => bool_zval(bytes.get(at).is_some_and(|b| *b != 0)),
            Door::I8 => int_zval(i64::from(i8::from_le_bytes(word(bytes, at)))),
            Door::I16 => int_zval(i64::from(i16::from_le_bytes(word(bytes, at)))),
            Door::I32 => int_zval(i64::from(i32::from_le_bytes(word(bytes, at)))),
            // u64 rides PHP's signed int as its two's-complement bit pattern, as HyperCast's own
            // u64 door presents it; a time of day is far inside the signed range.
            Door::I64 | Door::U64 | Door::Time => int_zval(i64_at(bytes, at)),
            Door::U8 => int_zval(i64::from(u8::from_le_bytes(word(bytes, at)))),
            Door::U16 => int_zval(i64::from(u16::from_le_bytes(word(bytes, at)))),
            Door::U32 => int_zval(i64::from(u32_at(bytes, at))),
            Door::F32 => float_zval(f64::from(f32::from_le_bytes(word(bytes, at)))),
            Door::F64 => float_zval(f64::from_le_bytes(word(bytes, at))),
            Door::Text => {
                // Offset, then length: a span into the input the batch was read from or the
                // shared strings — or, flagged, into the arena.
                let (offset, length) = (u32_at(bytes, at), u32_at(bytes, at + 4));
                bytes_zval(if length & Span::FLAG != 0 {
                    substr(&self.arena, offset, length & SPAN_LENGTH)
                } else {
                    substr(self.base.bytes(), offset, length)
                })
            }
            Door::Date | Door::DateOrdered => {
                // Year (u16), month, day: one 32-bit word.
                let w = u32_at(bytes, at);
                judge.date(
                    i64::from(w & 0xFFFF),
                    i64::from((w >> 16) & 0xFF),
                    i64::from(w >> 24),
                )?
            }
            Door::Timestamp | Door::Unix | Door::ExcelSerial => {
                // Seconds (i64), then nanoseconds (i32, never negative) and four bytes of padding.
                judge.instant(i64_at(bytes, at), i64::from(u32_at(bytes, at + 8)))?
            }
            Door::DateTime => {
                // The date's word and four bytes of padding, then nanoseconds of the day (u64).
                let w = u32_at(bytes, at);
                judge.civil(
                    i64::from(w & 0xFFFF),
                    i64::from((w >> 16) & 0xFF),
                    i64::from((w >> 24) & 0xFF),
                    i64_at(bytes, at + 8),
                )?
            }
            Door::Duration => {
                // Seconds (i64), then same-signed nanoseconds (i32) and four bytes of padding.
                judge.duration(i64_at(bytes, at), i64::from(u32_at(bytes, at + 8) as i32))?
            }
            Door::Decimal => {
                // The low 64 bits of the magnitude, then its high 32 bits, the scale, the sign
                // and two bytes of padding.
                let high = i64_at(bytes, at + 8);
                judge.decimal(
                    i64_at(bytes, at),
                    high & 0xFFFF_FFFF,
                    (high >> 32) & 0xFF,
                    (high >> 40) & 0xFF != 0,
                )?
            }
            Door::Uuid => judge.uuid(bytes.get(at..at + 16).ok_or_else(contract_violation)?)?,
        })
    }

    pub(crate) fn values(&self, column: i64) -> PhpResult<Zval> {
        Ok(self.decode(column)?.values.shallow_clone())
    }

    pub(crate) fn faults(&self, column: i64) -> PhpResult<Zval> {
        Ok(self.decode(column)?.faults.shallow_clone())
    }

    pub(crate) fn verdicts(&self, column: i64) -> PhpResult<Zval> {
        let decoded = self.decode(column)?;
        let judge = Judge::find()?;
        let mut verdicts = Vec::with_capacity(self.rows);
        for (cell, fault) in decoded.cells.iter().zip(&decoded.fault_of) {
            verdicts.push(match fault {
                Some(fault) => fault.shallow_clone(),
                None => judge.success(cell.shallow_clone())?,
            });
        }
        list_zval(verdicts)
    }

    pub(crate) fn get(&self, column: i64, row: i64) -> PhpResult<Zval> {
        let decoded = self.decode(column)?;
        let row = self.check_row(row)?;
        match &decoded.fault_of[row] {
            Some(fault) => Ok(fault.shallow_clone()),
            None => Judge::find()?.success(decoded.cells[row].shallow_clone()),
        }
    }

    /// The row's entry of `values($column)`.
    pub(crate) fn value_at(&self, column: i64, row: usize) -> PhpResult<Zval> {
        let decoded = self.decode(column)?;
        Ok(decoded
            .cells
            .get(row)
            .map(Zval::shallow_clone)
            .unwrap_or_default())
    }

    /// The row's entry of `faults($column)`, or null.
    pub(crate) fn fault_at(&self, column: i64, row: usize) -> PhpResult<Option<Zval>> {
        let decoded = self.decode(column)?;
        Ok(decoded
            .fault_of
            .get(row)
            .and_then(|fault| fault.as_ref().map(Zval::shallow_clone)))
    }

    /// The note of a cell known to be in the batch; none for a read that takes none.
    fn note(&self, column: usize, row: usize) -> CellNote {
        self.notes
            .get(column)
            .and_then(|notes| notes.get(row))
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn origin(&self, column: i64, row: i64) -> PhpResult<Origin> {
        let column = self.check_column(column)?;
        let row = self.check_row(row)?;
        Ok(Origin::from_code(self.note(column, row).origin))
    }

    pub(crate) fn detail(&self, column: i64, row: i64) -> PhpResult<Option<Zval>> {
        let column = self.check_column(column)?;
        let row = self.check_row(row)?;
        let code = u32::from(self.note(column, row).detail);
        let Some(verdict) = self.verdicts[column].get(row).filter(|_| code != 0) else {
            return Ok(None);
        };
        detailed_fault(verdict.reason, verdict.offset, verdict.len, code).map(Some)
    }

    /// The batch's list of problems: the core's answer over this batch's own copies, so the same
    /// whenever it's asked, and costing a read nothing until it is.
    pub(crate) fn problems(&self) -> PhpResult<Zval> {
        if let Some(done) = self.problems.get() {
            return Ok(done.shallow_clone());
        }
        let columns: Vec<ColumnBufferWith> = self
            .verdicts
            .iter()
            .zip(&self.notes)
            .map(|(verdicts, notes)| ColumnBufferWith {
                size: ColumnBufferWith::SIZE,
                values: std::ptr::null_mut(),
                verdicts: verdicts.as_ptr().cast_mut(),
                notes: if notes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    notes.as_ptr().cast_mut()
                },
            })
            .collect();
        let (widths, expected, long_rows) = match &self.widths {
            Some((width, widths)) => (
                widths.as_slice(),
                *width,
                RawDelimitedOptions::LONG_ROWS_REPORT,
            ),
            None => (&[][..], 0, RawDelimitedOptions::LONG_ROWS_REJECT),
        };
        let mut problems = Vec::new();
        let mut cursor = 0;
        let mut held = [RawProblem::default(); 64];
        loop {
            let mut written = 0;
            // SAFETY: each column's verdicts are this batch's `rows` of them, and its notes either
            // as many or null; the core only reads them, and a batch's copies are fixed once made.
            let code = unsafe {
                crate::kernel::faults::faults(
                    &columns,
                    self.rows,
                    widths,
                    expected,
                    long_rows,
                    &mut cursor,
                    &mut held,
                    &mut written,
                )
            };
            if code != OK {
                return Err(contract_violation());
            }
            for raw in held.get(..written).unwrap_or_default() {
                let source = self.sources.get(raw.plan_column as usize).copied();
                problems.push(problem(raw, self.line_of(raw.row as usize), source)?);
            }
            if written < held.len() {
                break;
            }
        }
        let list = list_zval(problems)?;
        Ok(self.problems.get_or_init(|| list).shallow_clone())
    }

    pub(crate) fn raw(&self, column: i64, row: i64) -> PhpResult<Vec<u8>> {
        let column = self.check_column(column)?;
        let row = self.check_row(row)?;
        let at = row * self.per_row
            + if self.workbook {
                column
            } else {
                self.ordinals[column]
            };
        let cell = self.cell(at);
        if self.workbook {
            return Ok(if cell.len & Span::FLAG != 0 {
                substr(&self.arena, cell.offset, cell.len & SPAN_LENGTH)
            } else {
                substr(self.base.bytes(), cell.offset, cell.len)
            }
            .to_vec());
        }
        let length = cell.len & SPAN_LENGTH;
        if length == 0 {
            return Ok(Vec::new());
        }
        let raw = substr(self.base.bytes(), cell.offset, length);
        if cell.len & Span::FLAG == 0 {
            return Ok(raw.to_vec());
        }
        // A cell with an escaped quote in it: unescaped by the core, as the core cast it.
        let mut out = vec![0u8; raw.len().max(1)];
        let written = unescape_into(raw, &mut out);
        out.truncate(written);
        Ok(out)
    }
}

impl Clone for Decoded {
    fn clone(&self) -> Self {
        Decoded {
            values: self.values.shallow_clone(),
            faults: self.faults.shallow_clone(),
            cells: self.cells.iter().map(Zval::shallow_clone).collect(),
            fault_of: self
                .fault_of
                .iter()
                .map(|f| f.as_ref().map(Zval::shallow_clone))
                .collect(),
        }
    }
}

/// A `$this` zval for a running method.
pub(crate) fn this_zval<T: ext_php_rs::class::RegisteredClass>(
    self_: &mut ZendClassObject<T>,
) -> Zval {
    let mut zv = Zval::new();
    zv.set_object(&mut self_.std);
    zv
}

/// The batch a zval holds.
pub(crate) fn batch_core(batch: &Zval) -> Option<Rc<BatchCore>> {
    <&Batch>::from_zval(batch).map(|batch| Rc::clone(&batch.core))
}

/// The rows one `DelimitedReader::read()` or `Sheet::read()` delivered, as typed columns: the
/// same class for delimited text and for a sheet of a workbook.
///
/// ```php
/// while (($batch = $reader->read()) !== null) {
///     $ids = $batch->values(0);                    // a column at a time
///     for ($row = 0; $row < $batch->rows(); $row++) {
///         $verdict = $batch->get(2, $row);         // or a cell at a time, as HyperCast's union
///         if ($verdict instanceof Fault) {
///             echo "line {$batch->line($row)}: {$verdict->reason->name} in {$batch->raw(2, $row)}\n";
///         }
///     }
/// }
/// ```
///
/// Or read across, a `Row` at a time: a batch iterates its rows in order. A batch owns what it
/// shows — the arrays the core wrote are copied out of the reader's as it is made — so
/// it stays good after the reader has moved on. A value is the PHP carrier HyperCast's `Cast`
/// returns for the column's door (`Door` lists them).
#[php_class]
#[php(name = "HyperTabular\\Batch")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
#[php(implements(ce = ce::aggregate, stub = "\\IteratorAggregate"))]
pub struct Batch {
    core: Rc<BatchCore>,
}

impl Batch {
    /// A batch object for `core`.
    pub(crate) fn object(core: BatchCore) -> PhpResult<Zval> {
        ZendClassObject::new(Batch {
            core: Rc::new(core),
        })
        .into_zval(false)
        .map_err(|_| pending("batch"))
    }
}

#[php_impl]
impl Batch {
    /// Made by a reader or a sheet as it reads; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Batch is made by a reader".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// How many rows the batch holds — never zero.
    pub fn rows(&self) -> PhpResult<i64> {
        guard(|| Ok(self.core.rows as i64))
    }

    /// The batch's rows, in order, each a `Row` — keyed by its index — that answers as the batch
    /// does for that row.
    pub fn get_iterator(self_: &mut ZendClassObject<Batch>) -> PhpResult<IteratorObject> {
        guard(|| {
            let core = self_
                .obj
                .as_ref()
                .map(|batch| Rc::clone(&batch.core))
                .ok_or_else(contract_violation)?;
            let batch = this_zval(self_);
            RowIterator::over_batch(batch, core).map(|it| IteratorObject(Some(it)))
        })
    }

    /// The plan the batch was read through: column `i` of the batch is `columns()[i]`.
    pub fn columns(&self) -> PhpResult<Array> {
        guard(|| Ok(Array(self.core.plan.shallow_clone())))
    }

    /// Where a row came from: for delimited text the 1-based line its record starts on, for a
    /// sheet its 1-based row number.
    pub fn line(&self, row: Int) -> PhpResult<i64> {
        guard(|| self.core.line(row.0))
    }

    /// A column, one value per row, each the PHP carrier of the column's door (`Door`) — and null
    /// where the cell did not cast, whatever the reason; `faults()` says why. Built in one piece
    /// the first time it is asked for.
    pub fn values(&self, column: Int) -> PhpResult<Array> {
        guard(|| self.core.values(column.0).map(Array))
    }

    /// With no column, everything wrong in this batch, by row and then by column, a row's own entry
    /// first: a list of `Problem`. Listed are cells at `Malformed` or `OutOfRange`, a workbook's
    /// formulas with no cached value, and rows a `LongRows::Report` dialect cut. Blanks, declared
    /// tokens, padding and a row cut under `LongRows::Truncate` aren't. With notes off a refused
    /// cell has no detail and a formula with no cached value isn't listed. A row the empty-row
    /// filter dropped isn't in the batch to be named. Made the first time it is asked for.
    ///
    /// With a column, that column's cells that did not cast: HyperCast's `Fault` for each, keyed by
    /// row, empty when every cell cast.
    pub fn faults(&self, column: OptInt) -> PhpResult<Array> {
        guard(|| {
            match column.0 {
                Some(column) => self.core.faults(column),
                None => self.core.problems(),
            }
            .map(Array)
        })
    }

    /// A column as verdicts, one per row: HyperCast's `Success` around the value, or its `Fault`.
    pub fn verdicts(&self, column: Int) -> PhpResult<Array> {
        guard(|| self.core.verdicts(column.0).map(Array))
    }

    /// The cell at (`$column`, `$row`) as HyperCast judged it: its `Success` around the value —
    /// the PHP carrier of the column's door (`Door`) — or its `Fault`, whose span indexes the
    /// cell's own text (`raw()`).
    pub fn get(&self, column: Int, row: Int) -> PhpResult<Verdict> {
        guard(|| self.core.get(column.0, row.0).map(|v| Verdict(Some(v))))
    }

    /// Where the verdict of the cell at (`$column`, `$row`) came from: a door, a declared token,
    /// padding, or a workbook's formula with no cached value. With notes off a token or a padded
    /// cell answers `Origin::Cell`: its verdict is still `Empty`, and nothing was kept of why.
    pub fn origin(&self, column: Int, row: Int) -> PhpResult<Origin> {
        guard(|| self.core.origin(column.0, row.0))
    }

    /// What failed in the cell at (`$column`, `$row`), as HyperCast's `DetailedFault`: its `Fault`'s
    /// reason and span, and the `FaultDetail`. Null where the cell cast, where nothing failed in it
    /// (a declared token, padding), and with notes off.
    pub fn detail(&self, column: Int, row: Int) -> PhpResult<MaybeDetailedFault> {
        guard(|| self.core.detail(column.0, row.0).map(MaybeDetailedFault))
    }

    /// The text a cell was cast from, whatever its door and whatever its verdict — what a fault's
    /// span indexes, and what to show for a value that did not cast. Quotes are resolved, as the
    /// core resolved them before casting. For a sheet this is a text cell's own text, and what a
    /// typed cell was said as when it failed its door (or went through the text door); a typed
    /// cell that cast has none.
    pub fn raw(&self, column: Int, row: Int) -> PhpResult<Bytes> {
        guard(|| self.core.raw(column.0, row.0).map(Bytes))
    }
}

/// One row of a `Batch`: the batch read across rather than down, for code that builds a value a
/// row at a time.
///
/// ```php
/// foreach ($reader->rows() as $row) {
///     $id = $row->get(0);                          // HyperCast's union, as Batch::get()
///     $name = $row->value(1);                      // the value, or null, as Batch::values()
///     if ($id instanceof Fault) {
///         echo "line {$row->line()}: {$id->reason->name} in {$row->raw(0)}\n";
///     }
/// }
/// ```
///
/// Each method is the batch's own with this row's index, so a row answers exactly what its batch
/// does. A batch owns what it shows, and so does a row: it stays good after the reader has moved
/// on.
#[php_class]
#[php(name = "HyperTabular\\Row")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
pub struct Row {
    batch: Zval,
    core: Rc<BatchCore>,
    index: usize,
}

impl Row {
    fn object(batch: &Zval, core: &Rc<BatchCore>, index: usize) -> PhpResult<Zval> {
        ZendClassObject::new(Row {
            batch: batch.shallow_clone(),
            core: Rc::clone(core),
            index,
        })
        .into_zval(false)
        .map_err(|_| pending("row"))
    }
}

#[php_impl]
impl Row {
    /// Made by a batch as it is iterated; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Row is made by a batch".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// The batch the row is in.
    pub fn batch(&self) -> PhpResult<BatchObject> {
        guard(|| Ok(BatchObject(Some(self.batch.shallow_clone()))))
    }

    /// The row's place in its batch, from zero.
    pub fn index(&self) -> PhpResult<i64> {
        guard(|| Ok(self.index as i64))
    }

    /// Where the row came from: `Batch::line()`.
    pub fn line(&self) -> PhpResult<i64> {
        guard(|| self.core.line(self.index as i64))
    }

    /// The cell in `$column` as HyperCast judged it: `Batch::get()`.
    pub fn get(&self, column: Int) -> PhpResult<Verdict> {
        guard(|| {
            self.core
                .get(column.0, self.index as i64)
                .map(|v| Verdict(Some(v)))
        })
    }

    /// The value of the cell in `$column`, or null where it did not cast: the row's entry of
    /// `Batch::values()`.
    pub fn value(&self, column: Int) -> PhpResult<MixedValue> {
        guard(|| self.core.value_at(column.0, self.index).map(MixedValue))
    }

    /// Why the cell in `$column` did not cast, or null when it did: the row's entry of
    /// `Batch::faults()`.
    pub fn fault(&self, column: Int) -> PhpResult<MaybeFault> {
        guard(|| self.core.fault_at(column.0, self.index).map(MaybeFault))
    }

    /// Where the verdict of the cell in `$column` came from: `Batch::origin()`.
    pub fn origin(&self, column: Int) -> PhpResult<Origin> {
        guard(|| self.core.origin(column.0, self.index as i64))
    }

    /// What failed in the cell in `$column`: `Batch::detail()`.
    pub fn detail(&self, column: Int) -> PhpResult<MaybeDetailedFault> {
        guard(|| {
            self.core
                .detail(column.0, self.index as i64)
                .map(MaybeDetailedFault)
        })
    }

    /// The text the cell in `$column` was cast from: `Batch::raw()`.
    pub fn raw(&self, column: Int) -> PhpResult<Bytes> {
        guard(|| self.core.raw(column.0, self.index as i64).map(Bytes))
    }
}

/// Where a `RowIterator`'s rows come from.
enum Source {
    /// One batch's rows, keyed by their index in it.
    Batch,
    /// Every row a reader or a sheet has left, batch by batch, keyed from zero across the read.
    Reader { reader: Zval, started: bool },
}

/// The rows of a batch (`Batch::getIterator()`), or every row a reader or a sheet has left
/// (`DelimitedReader::rows()`, `Sheet::rows()`), each a `Row`. A reader's rows are read as the
/// iteration reaches them, a batch at a time, forward only: once it has moved past its first
/// row it cannot be rewound, as a generator cannot.
#[php_class]
#[php(name = "HyperTabular\\RowIterator")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
#[php(implements(ce = ce::iterator, stub = "\\Iterator"))]
pub struct RowIterator {
    /// The iteration, borrowed for each call: reading a reader's next batch runs its stream's PHP,
    /// and a call back into this iterator from there is refused.
    state: RefCell<Iteration>,
}

/// Where an iteration is, behind its `RefCell`.
struct Iteration {
    source: Source,
    /// The batch in hand, its zval and what it shows.
    batch: Option<(Zval, Rc<BatchCore>)>,
    /// The row in hand within it.
    position: usize,
    /// The key of the row in hand.
    key: i64,
    current: Option<Zval>,
}

impl RowIterator {
    fn over_batch(batch: Zval, core: Rc<BatchCore>) -> PhpResult<Zval> {
        let mut iterator = Iteration {
            source: Source::Batch,
            batch: Some((batch, core)),
            position: 0,
            key: 0,
            current: None,
        };
        iterator.settle()?;
        ZendClassObject::new(RowIterator {
            state: RefCell::new(iterator),
        })
        .into_zval(false)
        .map_err(|_| pending("iterator"))
    }

    /// Every row `reader` has left; nothing is read until the iteration starts.
    pub(crate) fn over_reader(reader: Zval) -> PhpResult<Zval> {
        let iterator = Iteration {
            source: Source::Reader {
                reader,
                started: false,
            },
            batch: None,
            position: 0,
            key: 0,
            current: None,
        };
        ZendClassObject::new(RowIterator {
            state: RefCell::new(iterator),
        })
        .into_zval(false)
        .map_err(|_| pending("iterator"))
    }

    /// The iteration for a call; refused while another call, reading, holds it.
    fn state(&self) -> PhpResult<RefMut<'_, Iteration>> {
        self.state.try_borrow_mut().map_err(|_| {
            logic("The row iterator is in the middle of a read; its reader's stream can't call back into it")
        })
    }
}

impl Iteration {
    /// Makes the row at `position` current, reading the reader's next batch when the one in
    /// hand has run out.
    fn settle(&mut self) -> PhpResult<()> {
        loop {
            if let Some((batch, core)) = &self.batch
                && self.position < core.rows()
            {
                self.current = Some(Row::object(batch, core, self.position)?);
                return Ok(());
            }
            self.current = None;
            let Source::Reader { reader, .. } = &self.source else {
                return Ok(());
            };
            let next = call(Some(reader), "read", &mut [])?;
            if next.is_null() {
                self.batch = None;
                return Ok(());
            }
            let core = batch_core(&next).ok_or_else(contract_violation)?;
            self.batch = Some((next, core));
            self.position = 0;
        }
    }

    fn do_rewind(&mut self) -> PhpResult<()> {
        match &mut self.source {
            Source::Batch => {
                self.position = 0;
                self.key = 0;
                self.settle()
            }
            Source::Reader { started, .. } => {
                if !*started {
                    *started = true;
                    return self.settle();
                }
                if self.key > 0 {
                    return Err(PhpException::new(
                        "Cannot rewind a generator that was already run".into(),
                        0,
                        ce::exception(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn start(&mut self) -> PhpResult<()> {
        if let Source::Reader { started: false, .. } = self.source {
            return self.do_rewind();
        }
        Ok(())
    }
}

#[php_impl]
impl RowIterator {
    /// Made by a batch or a reader; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\RowIterator is made by a batch or a reader".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// The row in hand, or null past the last.
    pub fn current(&self) -> PhpResult<MixedValue> {
        guard(|| {
            let mut this = self.state()?;
            this.start()?;
            Ok(MixedValue(
                this.current
                    .as_ref()
                    .map(Zval::shallow_clone)
                    .unwrap_or_default(),
            ))
        })
    }

    /// The row's key: its index in the batch, or its place in the whole read.
    pub fn key(&self) -> PhpResult<MixedValue> {
        guard(|| {
            let mut this = self.state()?;
            this.start()?;
            Ok(MixedValue(if this.current.is_some() {
                int_zval(this.key)
            } else {
                Zval::new()
            }))
        })
    }

    /// Moves to the next row, reading the next batch when this one has run out.
    pub fn next(&self) -> PhpResult<()> {
        guard(|| {
            let mut this = self.state()?;
            this.start()?;
            if this.current.is_none() {
                return Ok(());
            }
            this.position += 1;
            this.key += 1;
            this.settle()
        })
    }

    /// Starts over: a batch's rows from the first; a reader's rows only before the iteration
    /// has moved past the first row.
    pub fn rewind(&self) -> PhpResult<()> {
        guard(|| self.state()?.do_rewind())
    }

    /// Whether there is a row in hand.
    pub fn valid(&self) -> PhpResult<bool> {
        guard(|| {
            let mut this = self.state()?;
            this.start()?;
            Ok(this.current.is_some())
        })
    }
}
