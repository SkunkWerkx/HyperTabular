//! The Python binding: the core linked straight into a CPython extension module
//! (`hypertabular._native`, PyO3: abi3-py311 with the GIL, abi3t-py315 or a 3.14t build
//! without it) — the whole Python backend, as HyperCast's extension is the whole of its
//! own.
//!
//! It reads through this crate's own [`crate::DelimitedReader`], [`crate::Workbook`] and
//! [`crate::Sheet`], the Rust binding, which own every buffer the core writes into and make one
//! call into the core per batch, with the interpreter released here. Each batch they lend is copied
//! into a Python [`Batch`], the shared strings a sheet's indexes held once per workbook as a `bytes`
//! every batch shares, and the text of delimited `bytes` being those `bytes` themselves.
//!
//! What Python is handed is a [`Batch`] that owns what it shows. A Rust batch is valid until the
//! next read, a rule Python has no way to hold a caller to — `list(reader)`
//! is the first thing anyone writes — so each array the core filled is copied, once and
//! whole, into a `bytes` of exactly the batch's size, and a column is a read-only
//! `memoryview` over that: the door's own item type for the primitive doors, so anything
//! that takes a buffer takes a column, with no Python call per cell. The doors whose value is not
//! a primitive become a `list` of the Python values HyperCast's package gives them, built in one
//! loop here on first use. A cell's note is kept only for a column with a cell that didn't cast,
//! and a batch's faults list is taken when the batch is copied, so that it costs nothing to a read
//! that has none, and stays right for as long as the batch is held.
//!
//! A caller's stream is the Rust reader's asynchronous source, polled here with a waker nothing
//! wakes: `Pending` says only that the stream has nothing to give this step, and the Rust reader's
//! cancel-safety is what lets the step be dropped and taken again. A file object is read in the
//! poll, and so is a file `DelimitedReader.open` opened, as CPython reads one: a read a signal cuts
//! short runs the signal's handler. A raised exception is `Pending` too, so that it reaches the
//! caller and the reader isn't spoiled; what a stream hands over that can't be read is final, since
//! it's gone from the stream. An asynchronous stream (`asyncio.StreamReader`, or anything with an `async read(size)`)
//! is never called here: the core never waits, and neither does this module. A step of such a read
//! ends by saying how much input it has room for; the coroutines in `hypertabular/_aio.py` await
//! that much from the stream, feed it in and take the next step, so all the core's work happens
//! between awaits, the interpreter released as ever.
//!
//! HyperCast is the judge of every cell, and its Python package is the judge of how a
//! verdict looks in Python: a cell is `hypercast.Success` or `hypercast.Fault` — that
//! package's own classes, called here, not copies of them — carrying its own `CastFailure`
//! members, and a plan column's notation is its own `NumFormat`, crossing as the bytes it
//! packs itself into. A cast value becomes a Python value (`decimal.Decimal`, `uuid.UUID`,
//! the `datetime` types, microsecond truncation) through HyperCast's own conversions
//! (`hypercast::python::Values`), so it is the object `hypercast.cast_*` makes of the same
//! text.
//!
//! Free-threading: PyO3 0.30 declares the module needs no GIL, so on 3.14t/3.15t calls from
//! different threads really run at once, and nothing here leans on the GIL for exclusion:
//! - What is shared is immutable. `Batch`, `ColumnData`, `Row` and `Workbook` are `frozen`,
//!   their bytes are `bytes` objects or vectors never written after the batch is made, and
//!   a `Workbook`'s core is behind an `Arc` that every sheet only reads. A column's lazily
//!   built `values` and `verdicts` are `PyOnceLock`s, which run the build once however many
//!   threads ask, and `COMPANIONS` is one, set once by `_bind`.
//! - What is mutable is a reader's or a sheet's, borrowed `&mut` for each call. PyO3's
//!   borrow flag is atomic on a free-threaded build, so a second thread calling into one
//!   that another is inside of gets `RuntimeError` ("Already borrowed"), and that holds
//!   across the `py.detach` the core runs under, too: the buffers the core writes are
//!   reached only through that borrow.
//! - The only Python storage read in place is a `bytes` (the caller's input), which never changes
//!   or moves while the reference to it is held. A `bytearray`
//!   or other mutable buffer is refused, never read through a raw pointer.

use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use pyo3::exceptions::{
    PyIndexError, PyKeyError, PyMemoryError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBytes, PyDict, PyList, PyMemoryView, PyString, PyTuple};

use crate::column;
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBufferWith, ColumnSpec, OK, RawCheck, RawDelimitedOptions,
    RawProblem, Span,
};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use crate::kernel::suggest::{COMMA_DECIMAL, COMMA_GROUPS};
use crate::{Candidate, Encoding, LongRows, Origin, Problem, ShortRows, SuggestStatus};
use hypercast::python::Values;
use hypercast::{
    CivilDateTime, Date, DateOptions, DateOrder, Decimal, Duration, MonthNames, NumFormat,
    RawNumFormat, Reason, Timestamp,
};

/// Rows per batch unless told otherwise.
const DEFAULT_BATCH_ROWS: usize = 4096;
/// The read buffer a stream starts with; it doubles when a record does not fit.
const DEFAULT_BUFFER_BYTES: usize = 256 * 1024;
const CONTRACT: &str =
    "the hypertabular core reported a contract violation — a binding bug, please report it";

/// The Python objects this module presents its results with. HyperCast's are imported
/// from its package; the binding's own (the plan column, the dialect, the structural
/// failure) are defined in `hypertabular/__init__.py` and handed over by `_bind`.
struct Companions {
    success: Py<PyAny>,
    fault: Py<PyAny>,
    /// HyperCast's `Checked`, `Violation` and `Keyword`: a checked cell's verdict.
    checked: Py<PyAny>,
    violation: Py<PyAny>,
    keyword: Py<PyAny>,
    /// HyperCast's `Constraints`, which a plan column's `constraints` must be.
    constraints: Py<PyAny>,
    /// `CastFailure.EMPTY`, `MALFORMED`, `OUT_OF_RANGE` — indexed by reason code less one.
    reasons: [Py<PyAny>; 3],
    num_format: Py<PyAny>,
    column: Py<PyAny>,
    dialect: Py<PyAny>,
    error: Py<PyAny>,
    /// `TabularFailure`, called with the core's code for its member.
    failure: Py<PyAny>,
    /// `WorkbookFormat`, called with the core's code for its member.
    workbook_format: Py<PyAny>,
    /// HyperCast's `ExcelEpoch`, likewise.
    excel_epoch: Py<PyAny>,
    /// `Header`, called with the names as `str`s and as the bytes they were read from.
    header: Py<PyAny>,
    /// `Problem`, called with an entry's fields.
    problem: Py<PyAny>,
    /// `Origin`, called with the core's code for its member.
    origin: Py<PyAny>,
    /// `SuggestOptions`, which `suggest` is handed.
    suggest_options: Py<PyAny>,
    /// `_suggestion`, which makes a `Suggestion` of what the crate found.
    suggestion: Py<PyAny>,
    /// HyperCast's `Detail`, called with the core's code for its member.
    detail: Py<PyAny>,
    /// What every cast value is built with: HyperCast's own conversions.
    values: Values,
}

static COMPANIONS: PyOnceLock<Companions> = PyOnceLock::new();

fn companions(py: Python<'_>) -> PyResult<&'static Companions> {
    COMPANIONS
        .get(py)
        .ok_or_else(|| PyRuntimeError::new_err("hypertabular._native used before _bind"))
}

/// The `struct` format of a door whose value is a primitive — the doors whose value array
/// is handed to Python as it is. `None` for the doors whose values are Python objects.
const fn primitive_format(door: Door) -> Option<&'static str> {
    Some(match door {
        Door::Bool => "?",
        Door::I8 => "b",
        Door::I16 => "h",
        Door::I32 => "i",
        Door::I64 => "q",
        Door::U8 => "B",
        Door::U16 => "H",
        Door::U32 => "I",
        Door::U64 => "Q",
        Door::F32 => "f",
        Door::F64 => "d",
        _ => return None,
    })
}

/// A read-only `memoryview` over `bytes`, as `format` items — and as `shape` when the
/// view has more than one dimension.
fn view(
    py: Python<'_>,
    bytes: &Py<PyBytes>,
    format: &str,
    shape: Option<(usize, usize)>,
) -> PyResult<Py<PyAny>> {
    let whole = PyMemoryView::from(bytes.bind(py).as_any())?;
    let typed = match shape {
        None => whole.call_method1("cast", (format,))?,
        Some(shape) => whole.call_method1("cast", (format, shape))?,
    };
    Ok(typed.unbind())
}

/// A cell's or a name's text as a `str`. What a read delivers as text is UTF-8 already, a cell that
/// isn't being a fault and a name that isn't a failure ahead of here, so nothing's replaced.
fn text<'py>(py: Python<'py>, bytes: &[u8]) -> Bound<'py, PyString> {
    PyString::new(py, &String::from_utf8_lossy(bytes))
}

/// The package's `Header` for these names: a tuple of their `str`s, which also keeps the
/// bytes each was read from, for a lookup by bytes to match exactly.
fn header_of<'a>(py: Python<'_>, names: impl IntoIterator<Item = &'a [u8]>) -> PyResult<Py<PyAny>> {
    let companions = companions(py)?;
    let mut said = Vec::new();
    let mut raw = Vec::new();
    for name in names {
        said.push(text(py, name));
        raw.push(PyBytes::new(py, name));
    }
    let made = companions
        .header
        .bind(py)
        .call1((PyTuple::new(py, said)?, PyTuple::new(py, raw)?))?;
    Ok(made.unbind())
}

/// The exception for a reader or sheet used out of order: read before a plan is bound, or
/// bound twice. Python's own word for an object in the wrong state for a call.
fn out_of_order(message: &str) -> PyErr {
    PyRuntimeError::new_err(message.to_owned())
}

/// What `read()` on a reader whose plan is still to be bound raises.
const UNBOUND: &str = "the reader has no plan yet: bind(plan) before the first read";
/// What a second `bind` raises.
const ALREADY_BOUND: &str = "the reader already has a plan: a reader is bound once";

fn fault<'py>(
    py: Python<'py>,
    companions: &Companions,
    verdict: CellVerdict,
    note: CellNote,
) -> PyResult<Bound<'py, PyAny>> {
    let fault = verdict
        .fault()
        .ok_or_else(|| PyRuntimeError::new_err(CONTRACT))?;
    let reason = match fault.reason {
        Reason::Empty => &companions.reasons[0],
        Reason::Malformed => &companions.reasons[1],
        Reason::OutOfRange => &companions.reasons[2],
    };
    let args = (reason.bind(py), fault.offset, fault.len);
    if note.detail == 0 {
        return companions.fault.bind(py).call1(args);
    }
    let details = PyDict::new(py);
    details.set_item("detail_code", u32::from(note.detail))?;
    companions.fault.bind(py).call(args, Some(&details))
}

/// One plan column of a batch: the batch's copies of the two arrays the core filled, and
/// the views of them handed out so far.
struct Slot {
    door: Door,
    /// Where its raw cells are in the cell table: the source column it reads, or — for a
    /// workbook, whose table is laid out by plan column — its place in the plan.
    ordinal: usize,
    /// The source column a problem in this column names.
    source: usize,
    values: Py<PyBytes>,
    verdicts: Py<PyBytes>,
    /// What the core noted of each row, for a column with a cell that did not cast and a read
    /// that takes notes. Otherwise none, and every row's is the default.
    notes: Vec<CellNote>,
    /// Cells that did not cast — empty ones included. A violation cast, so it isn't one.
    faults: usize,
    /// Whether the plan column declares constraints, and so whether the plain getters refuse it.
    checked: bool,
    /// A constrained column's checks, one a row, as the core wrote them beside its verdicts; none
    /// for a column that declares none.
    checks: Vec<RawCheck>,
    values_view: PyOnceLock<Py<PyAny>>,
    verdicts_view: PyOnceLock<Py<PyAny>>,
}

impl Slot {
    /// The value at `row`, exactly as the core wrote it.
    ///
    /// # Safety
    /// `T` is the type this slot's door writes.
    unsafe fn value<T: Copy>(&self, py: Python<'_>, row: usize) -> PyResult<T> {
        let at = row * size_of::<T>();
        let bytes = self
            .values
            .as_bytes(py)
            .get(at..at + size_of::<T>())
            .ok_or_else(|| PyIndexError::new_err("row out of range"))?;
        // SAFETY: the slice holds one `T` the core wrote — a batch holds only the rows
        // it filled, and a cell that did not cast is written as the type's zero. It need
        // not be aligned.
        Ok(unsafe { bytes.as_ptr().cast::<T>().read_unaligned() })
    }

    fn verdict(&self, py: Python<'_>, row: usize) -> PyResult<CellVerdict> {
        verdict_at(self.verdicts.as_bytes(py), row)
            .ok_or_else(|| PyIndexError::new_err("row out of range"))
    }

    fn note(&self, row: usize) -> CellNote {
        self.notes.get(row).copied().unwrap_or_default()
    }

    fn fault<'py>(
        &self,
        py: Python<'py>,
        companions: &Companions,
        verdict: CellVerdict,
        row: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        fault(py, companions, verdict, self.note(row))
    }
}

/// The verdict at `row` of a verdict array held as bytes.
fn verdict_at(verdicts: &[u8], row: usize) -> Option<CellVerdict> {
    let at = row.checked_mul(size_of::<CellVerdict>())?;
    let bytes = verdicts.get(at..at.checked_add(size_of::<CellVerdict>())?)?;
    // SAFETY: twelve bytes the core wrote as one `CellVerdict`: three plain integers,
    // which any twelve bytes are. They need not be aligned.
    Some(unsafe { bytes.as_ptr().cast::<CellVerdict>().read_unaligned() })
}

/// The input a batch's cell table indexes.
enum Input {
    /// The caller's own `bytes`, shared: `len` bytes from `start`.
    Shared {
        bytes: Py<PyBytes>,
        start: usize,
        len: usize,
    },
    /// A stream's bytes for these rows, copied out of the read buffer before it moves on.
    Owned(Vec<u8>),
    /// A workbook's shared strings, whole, shared by every batch of every sheet.
    Strings(Py<PyBytes>),
}

/// One batch of rows, column by column: for each plan column, the values the core cast
/// and a verdict for every cell. A batch owns everything it shows — it stays valid after
/// the reader has moved on, for as long as it is referenced.
#[pyclass(frozen, module = "hypertabular")]
struct Batch {
    rows: usize,
    /// Cell-table entries one row takes.
    per_row: usize,
    plan: Py<PyTuple>,
    slots: Vec<Slot>,
    /// Where each cell is in `input`: row `r`, source column `c` at `r * per_row + c`.
    cells: Vec<Span>,
    input: Input,
    /// The unescaped bytes of the text values that had an escaped quote in them — or, for
    /// a workbook, the text the core said typed cells as.
    arena: Vec<u8>,
    /// Whether the rows are a sheet's: its cell table is one raw cell per plan column (in
    /// the shared strings, or flagged, in the arena) and its row numbers are offsets.
    workbook: bool,
    /// The width and each row's, for a read that reports long rows. With the verdicts and notes the
    /// slots hold, they're all the core needs to list a batch's faults when it's asked to.
    widths: Option<(u32, Vec<u32>)>,
}

/// What a batch's unflagged spans index, and how a copy holds it.
enum Holds {
    /// A workbook's shared strings.
    Strings(Py<PyBytes>),
    /// The caller's `bytes`, which the text the batch was read from is part of.
    Shared(Py<PyBytes>),
    /// The reader's own buffer, so the batch takes what its cells reach.
    Stream,
}

/// A batch the crate lent, as one that owns what it shows.
fn batch_of(
    py: Python<'_>,
    batch: &crate::Batch<'_>,
    plan: &Py<PyTuple>,
    doors: &[Door],
    holds: Holds,
) -> PyResult<Batch> {
    let (cells, per_row, base, arena) = batch.tables();
    let workbook = matches!(holds, Holds::Strings(_));
    // The arena as far as any span of this batch reaches into it: text with an escaped quote in
    // it, or for a sheet the text typed cells were said as, and the raw text of those that failed.
    let mut reach = 0;
    let mut slots = Vec::with_capacity(doors.len());
    for (index, door) in doors.iter().enumerate() {
        let (values, verdicts) = batch.column_bytes(index);
        let source = batch.source_column(index);
        if *door == Door::Text {
            for at in (0..values.len()).step_by(size_of::<Span>()) {
                // SAFETY: a text column's values are spans, eight bytes each; they
                // need not be aligned.
                let span = unsafe { values.as_ptr().add(at).cast::<Span>().read_unaligned() };
                if span.flagged() {
                    reach = reach.max(span.offset as usize + span.len());
                }
            }
        }
        // SAFETY: a verdict array is plain integers, viewed as its bytes.
        let verdict_bytes = unsafe {
            std::slice::from_raw_parts(verdicts.as_ptr().cast::<u8>(), size_of_val(verdicts))
        };
        let faults = verdicts
            .iter()
            .filter(|verdict| verdict.fault().is_some())
            .count();
        let checked = batch
            .columns()
            .get(index)
            .is_some_and(crate::Column::is_checked);
        slots.push(Slot {
            door: *door,
            ordinal: if workbook {
                index
            } else {
                batch
                    .columns()
                    .get(index)
                    .map_or(0, |column| column.ordinal)
            },
            source,
            values: PyBytes::new(py, values).unbind(),
            verdicts: PyBytes::new(py, verdict_bytes).unbind(),
            notes: if faults == 0 {
                Vec::new()
            } else {
                batch.notes(index).to_vec()
            },
            faults,
            checked,
            checks: batch.checks(index).to_vec(),
            values_view: PyOnceLock::new(),
            verdicts_view: PyOnceLock::new(),
        });
    }
    if workbook {
        for cell in cells.iter().filter(|cell| cell.flagged()) {
            reach = reach.max(cell.offset as usize + cell.len());
        }
    }
    let input = match holds {
        Holds::Strings(bytes) => Input::Strings(bytes),
        Holds::Shared(bytes) => Input::Shared {
            start: (base.as_ptr() as usize).saturating_sub(bytes.as_bytes(py).as_ptr() as usize),
            len: base.len(),
            bytes,
        },
        Holds::Stream => {
            // Every row's last entry says where it came from, and isn't in the text.
            let end = cells
                .chunks_exact(per_row.max(1))
                .flat_map(|row| row.get(..per_row - 1).unwrap_or_default())
                .map(|cell| cell.offset as usize + cell.len())
                .max()
                .unwrap_or(0);
            Input::Owned(base.get(..end).unwrap_or(base).to_vec())
        }
    };
    Ok(Batch {
        rows: batch.rows(),
        per_row,
        plan: plan.clone_ref(py),
        slots,
        cells: cells.to_vec(),
        input,
        arena: arena.get(..reach).unwrap_or(arena).to_vec(),
        workbook,
        widths: batch
            .reported_widths()
            .map(|(width, widths)| (width, widths.to_vec())),
    })
}

impl Batch {
    /// The batch's faults list: the core's answer over this batch's own copies, so the same whenever
    /// it's asked, and costing a read nothing until it is.
    fn problems(&self, py: Python<'_>) -> PyResult<Vec<Problem>> {
        let columns: Vec<ColumnBufferWith> = self
            .slots
            .iter()
            .map(|slot| ColumnBufferWith {
                size: ColumnBufferWith::SIZE,
                values: std::ptr::null_mut(),
                verdicts: slot.verdicts.as_bytes(py).as_ptr().cast_mut().cast(),
                notes: if slot.notes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    slot.notes.as_ptr().cast_mut()
                },
                checks: if slot.checks.is_empty() {
                    std::ptr::null_mut()
                } else {
                    slot.checks.as_ptr().cast_mut()
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
            // SAFETY: each column's verdicts are this batch's `rows` of them, at any alignment,
            // and its notes either as many or null; the core only reads them, and nothing here
            // changes them, a `bytes` being immutable and a batch's slots fixed once made.
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
                return Err(PyRuntimeError::new_err(CONTRACT));
            }
            for raw in held.get(..written).unwrap_or_default() {
                let source = self
                    .slots
                    .get(raw.plan_column as usize)
                    .map(|slot| slot.source);
                problems.push(Problem::from_raw(
                    *raw,
                    self.line_of(raw.row as usize),
                    source,
                ));
            }
            if written < held.len() {
                return Ok(problems);
            }
        }
    }

    /// The line row `row` came from, or for a sheet its row number.
    fn line_of(&self, row: usize) -> u32 {
        let entry = self
            .cells
            .get(row * self.per_row + self.per_row - 1)
            .copied()
            .unwrap_or_default();
        if self.workbook {
            entry.offset
        } else {
            entry.len
        }
    }

    fn input<'a>(&'a self, py: Python<'_>) -> &'a [u8] {
        match &self.input {
            Input::Shared { bytes, start, len } => bytes
                .as_bytes(py)
                .get(*start..*start + *len)
                .unwrap_or_default(),
            Input::Owned(bytes) => bytes,
            Input::Strings(bytes) => bytes.as_bytes(py),
        }
    }

    fn slot(&self, column: usize) -> PyResult<&Slot> {
        self.slots
            .get(column)
            .ok_or_else(|| PyIndexError::new_err("column out of range"))
    }

    /// A row index as Python gives it — negative counts from the end — checked.
    fn row_index(&self, row: isize) -> PyResult<usize> {
        let index = if row < 0 {
            row + self.rows as isize
        } else {
            row
        };
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.rows)
            .ok_or_else(|| PyIndexError::new_err("row out of range"))
    }

    /// The text a cell was cast from, with its quoting resolved, as the core read it.
    fn raw_text<'py>(&self, py: Python<'py>, slot: &Slot, row: usize) -> Bound<'py, PyBytes> {
        let cell = self
            .cells
            .get(row * self.per_row + slot.ordinal)
            .copied()
            .unwrap_or_default();
        let start = cell.offset as usize;
        if self.workbook {
            // A sheet's raw text is in the shared strings, or — flagged — in the arena.
            let from: &[u8] = if cell.flagged() {
                &self.arena
            } else {
                self.input(py)
            };
            return PyBytes::new(py, from.get(start..start + cell.len()).unwrap_or_default());
        }
        let raw = self
            .input(py)
            .get(start..start + cell.len())
            .unwrap_or_default();
        if !cell.flagged() {
            return PyBytes::new(py, raw);
        }
        // A cell with an escaped quote in it: unescaped, as the core cast it.
        let mut unescaped = vec![0u8; raw.len()];
        let written = unescape_into(raw, &mut unescaped);
        PyBytes::new(py, unescaped.get(..written).unwrap_or_default())
    }

    /// The value of a cell that cast, as the Python type HyperCast's package gives the
    /// same door.
    fn object<'py>(
        &self,
        py: Python<'py>,
        companions: &Companions,
        slot: &Slot,
        row: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY (every `value` below): the type read is the one the door writes, per
        // `kernel::abi::ColumnBuffer`.
        Ok(match slot.door {
            Door::Bool => {
                let value = unsafe { slot.value::<u8>(py, row)? } != 0;
                value.into_pyobject(py)?.to_owned().into_any()
            }
            Door::I8 => unsafe { slot.value::<i8>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::I16 => unsafe { slot.value::<i16>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::I32 => unsafe { slot.value::<i32>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::I64 => unsafe { slot.value::<i64>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::U8 => unsafe { slot.value::<u8>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::U16 => unsafe { slot.value::<u16>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::U32 => unsafe { slot.value::<u32>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::U64 => unsafe { slot.value::<u64>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            // Widened losslessly, as HyperCast's `cast_f32` presents it.
            Door::F32 => f64::from(unsafe { slot.value::<f32>(py, row)? })
                .into_pyobject(py)?
                .into_any(),
            Door::F64 => unsafe { slot.value::<f64>(py, row)? }
                .into_pyobject(py)?
                .into_any(),
            Door::Decimal => companions
                .values
                .decimal(py, unsafe { slot.value::<Decimal>(py, row)? })?,
            Door::Uuid => companions
                .values
                .uuid(py, unsafe { slot.value::<[u8; 16]>(py, row)? })?,
            Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => companions
                .values
                .instant(py, unsafe { slot.value::<Timestamp>(py, row)? })?,
            Door::Date | Door::DateOrdered(_) => companions
                .values
                .date(py, unsafe { slot.value::<Date>(py, row)? })?,
            Door::DateTime(_) => companions
                .values
                .civil(py, unsafe { slot.value::<CivilDateTime>(py, row)? })?,
            Door::Time => companions
                .values
                .time(py, unsafe { slot.value::<u64>(py, row)? })?,
            Door::Duration => companions
                .values
                .duration(py, unsafe { slot.value::<Duration>(py, row)? })?,
            Door::Text => {
                let value = unsafe { slot.value::<Span>(py, row)? };
                let from: &[u8] = if value.flagged() {
                    &self.arena
                } else {
                    self.input(py)
                };
                let start = value.offset as usize;
                text(py, from.get(start..start + value.len()).unwrap_or_default()).into_any()
            }
        })
    }

    /// A text cell's value as a `str`, or `None` for one that did not cast.
    fn text_cell<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: usize,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let slot = self.unchecked(column)?;
        if slot.door != Door::Text {
            return Err(PyTypeError::new_err(format!(
                "column {column} is not read through the text door"
            )));
        }
        if !slot.verdict(py, row)?.is_ok() {
            return Ok(None);
        }
        Ok(Some(self.object(py, companions(py)?, slot, row)?))
    }

    /// A cell as HyperCast's verdict: `Success(value)` or `Fault(reason, offset, length)`.
    fn cell<'py>(
        &self,
        py: Python<'py>,
        companions: &Companions,
        slot: &Slot,
        row: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let verdict = slot.verdict(py, row)?;
        if verdict.is_ok() {
            let value = self.object(py, companions, slot, row)?;
            companions.success.bind(py).call1((value,))
        } else {
            slot.fault(py, companions, verdict, row)
        }
    }

    /// Column `column`'s slot, refused when the column declares constraints: a plain getter would
    /// read a violation as a value that passed.
    fn unchecked(&self, column: usize) -> PyResult<&Slot> {
        let slot = self.slot(column)?;
        if slot.checked {
            return Err(PyTypeError::new_err(format!(
                "column {column} declares constraints: read it with get_checked"
            )));
        }
        Ok(slot)
    }

    /// A cell as HyperCast's checked verdict: `Checked(value, adjusted, enum_index)`,
    /// `Violation(keyword, value, offset, length)` carrying the value as cast, or the cell's
    /// `Fault`. A text `enum` match is the entry as the plan's `Constraints` declare it.
    fn checked_cell<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let slot = self.slot(column)?;
        let companions = companions(py)?;
        let verdict = slot.verdict(py, row)?;
        // A column that declares none has no checks: nothing was adjusted or matched.
        let check = slot.checks.get(row).copied().unwrap_or(RawCheck {
            keyword: 0,
            adjusted: 0,
            enum_index: u32::MAX,
        });
        if verdict.reason == CellVerdict::VIOLATION {
            let keyword = companions.keyword.bind(py).call1((check.keyword,))?;
            let value = self.object(py, companions, slot, row)?;
            return companions.violation.bind(py).call1((
                keyword,
                value,
                verdict.offset,
                verdict.len,
            ));
        }
        if verdict.fault().is_some() {
            return slot.fault(py, companions, verdict, row);
        }
        let enum_index = (check.enum_index != u32::MAX).then_some(check.enum_index);
        let value = match enum_index {
            Some(index) if slot.door == Door::Text => self
                .plan
                .bind(py)
                .get_item(column)?
                .getattr("constraints")?
                .getattr("enum")?
                .get_item(index)?,
            _ => self.object(py, companions, slot, row)?,
        };
        companions
            .checked
            .bind(py)
            .call1((value, check.adjusted != 0, enum_index))
    }

    /// Where the verdict of a cell came from, as the package's `Origin`.
    fn origin_of<'py>(
        &self,
        py: Python<'py>,
        slot: &Slot,
        row: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        companions(py)?
            .origin
            .bind(py)
            .call1((u32::from(slot.note(row).origin),))
    }
}

/// The package's name for an origin.
const fn origin_code(origin: Origin) -> u32 {
    match origin {
        Origin::Token => 1,
        Origin::Padded => 2,
        Origin::Uncached => 3,
        Origin::Cell => 0,
    }
}

#[pymethods]
impl Batch {
    /// Rows in this batch — never zero.
    #[getter]
    fn rows(&self) -> usize {
        self.rows
    }

    fn __len__(&self) -> usize {
        self.rows
    }

    /// The batch's columns, one per plan column, in plan order.
    #[getter]
    fn columns<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let columns = (0..slf.get().slots.len()).map(|index| ColumnData {
            batch: slf.clone().unbind(),
            index,
        });
        PyTuple::new(slf.py(), columns)
    }

    /// The column at plan index ``index``.
    fn column(slf: &Bound<'_, Self>, index: isize) -> PyResult<ColumnData> {
        let count = slf.get().slots.len();
        let index = if index < 0 {
            index + count as isize
        } else {
            index
        };
        let index = usize::try_from(index)
            .ok()
            .filter(|index| *index < count)
            .ok_or_else(|| PyIndexError::new_err("column out of range"))?;
        Ok(ColumnData {
            batch: slf.clone().unbind(),
            index,
        })
    }

    /// Where row ``row`` came from: for delimited text the 1-based line its record starts
    /// on, for a sheet its 1-based row number.
    fn line(&self, row: isize) -> PyResult<u32> {
        Ok(self.line_of(self.row_index(row)?))
    }

    /// The text the cell at (``column``, ``row``) was cast from, whatever its door and
    /// whatever its verdict — what a fault's span indexes, and what to show for a value
    /// that did not cast. Quoting is resolved; nothing is trimmed. For a sheet, a typed
    /// cell that cast has none.
    fn raw<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: isize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let slot = self.slot(column)?;
        Ok(self.raw_text(py, slot, self.row_index(row)?))
    }

    /// The cell at (``column``, ``row``) as HyperCast's verdict — ``Success(value)`` or
    /// ``Fault(reason, offset, length)`` — as ``batch.column(column)[row]`` gives it.
    ///
    /// A column that declares constraints raises ``TypeError``: read it with :meth:`get_checked`.
    fn get<'py>(&self, py: Python<'py>, column: usize, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let slot = self.unchecked(column)?;
        self.cell(py, companions(py)?, slot, self.row_index(row)?)
    }

    /// The cell at (``column``, ``row``) as HyperCast's checked door judged it: ``Checked(value,
    /// adjusted, enum_index)``, flagged when ``multiple_of``'s rounding adjusted the value, and at
    /// the text door with an ``enum`` the entry it matched as the plan's ``Constraints`` declare it;
    /// ``Violation(keyword, value, offset, length)`` of a constraint the column declares, carrying
    /// the value as cast and the judged text's span; or the cell's ``Fault``. A column that declares
    /// none reads as :meth:`get` does, never adjusted.
    fn get_checked<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: isize,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.checked_cell(py, column, self.row_index(row)?)
    }

    /// The text cell at (``column``, ``row``) as a ``str``, or ``None`` for one that did
    /// not cast (a text cell fails only by being empty) — the same ``str`` ``values``
    /// holds. A column read through any other door, or one that declares constraints, raises
    /// ``TypeError``.
    fn text<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: isize,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.text_cell(py, column, self.row_index(row)?)
    }

    /// Where the verdict of the cell at (``column``, ``row``) came from: an :class:`Origin`, a door
    /// (``Origin.CELL``), a declared token, padding, or a workbook's formula with no cached value.
    /// With notes off it's ``Origin.CELL``, whatever the cell's ``Empty`` came from.
    fn origin<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: isize,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.origin_of(py, self.slot(column)?, self.row_index(row)?)
    }

    /// What went wrong in this batch, by row and then by column, a row's own entry first: a list of
    /// :class:`Problem`. Cells at ``MALFORMED`` or ``OUT_OF_RANGE``, formulas with no cached value
    /// and rows longer than the width under ``LongRows.REPORT``. Blanks, declared tokens, padding
    /// and a row cut under ``LongRows.TRUNCATE`` aren't problems.
    ///
    /// A new list each call, and the batch's own: it stays right after the reader has moved on,
    /// since a batch is a copy, so there is no such thing here as a stale one. With notes off the
    /// list still holds every refused cell and long row, with no ``detail``, but no formula with no
    /// cached value. A row of a sheet the empty-row filter left out is in no batch, so it can't be
    /// named: set ``SheetOptions.skip_empty_rows`` to ``False`` to have those rows listed.
    fn faults<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let companions = companions(py)?;
        let list = PyList::empty(py);
        for problem in self.problems(py)? {
            let reason = problem.reason.map(|reason| match reason {
                Reason::Empty => companions.reasons[0].bind(py),
                Reason::Malformed => companions.reasons[1].bind(py),
                Reason::OutOfRange => companions.reasons[2].bind(py),
            });
            let code = problem.detail.map(hypercast::Detail::code);
            let keyword = problem
                .keyword
                .map(|keyword| companions.keyword.bind(py).call1((keyword.code(),)))
                .transpose()?;
            let detail = code
                .map(|code| companions.detail.bind(py).call1((code,)))
                .transpose()?;
            // Past the twelve a call takes positionally.
            let kwargs = PyDict::new(py);
            kwargs.set_item("keyword", keyword)?;
            list.append(
                companions.problem.bind(py).call(
                    (
                        problem.row,
                        problem.line,
                        problem.plan_column,
                        problem.source_column,
                        reason,
                        detail,
                        code,
                        companions
                            .origin
                            .bind(py)
                            .call1((origin_code(problem.origin),))?,
                        problem.offset,
                        problem.len,
                        problem.expected,
                        problem.found,
                    ),
                    Some(&kwargs),
                )?,
            )?;
        }
        Ok(list)
    }

    /// Row ``row`` of the batch — negative counts from the end — as a :class:`Row`.
    fn row(slf: &Bound<'_, Self>, row: isize) -> PyResult<Row> {
        let index = slf.get().row_index(row)?;
        Ok(Row {
            batch: slf.clone().unbind(),
            index,
        })
    }

    /// The batch's rows, in order, as :class:`Row` views — what ``for row in batch``
    /// iterates. Each is the batch and an index, nothing copied.
    fn iter_rows(slf: &Bound<'_, Self>) -> Rows {
        Rows::over(slf.clone().unbind())
    }

    fn __iter__(slf: &Bound<'_, Self>) -> Rows {
        Rows::over(slf.clone().unbind())
    }

    fn __repr__(&self) -> String {
        format!(
            "<hypertabular.Batch rows={} columns={}>",
            self.rows,
            self.slots.len()
        )
    }
}

/// One column of a batch.
///
/// ``values`` and ``verdicts`` are the whole column at once, with no call per cell;
/// ``column[row]`` is one cell as a HyperCast verdict, to ``match`` on.
#[pyclass(frozen, module = "hypertabular")]
struct ColumnData {
    batch: Py<Batch>,
    index: usize,
}

impl ColumnData {
    fn parts(&self) -> PyResult<(&Batch, &Slot)> {
        let batch = self.batch.get();
        Ok((batch, batch.slot(self.index)?))
    }
}

#[pymethods]
impl ColumnData {
    /// The plan column this is the data of.
    #[getter]
    fn column<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.batch.get().plan.bind(py).get_item(self.index)
    }

    fn __len__(&self) -> usize {
        self.batch.get().rows
    }

    /// The column's values, one per row.
    ///
    /// For the doors whose value is a primitive — ``bool``, the integers, the floats —
    /// a read-only ``memoryview`` of the array the core wrote, in the door's own item
    /// type (``'?'``, ``'b'`` … ``'Q'``, ``'f'``, ``'d'``). It views the batch's memory
    /// rather than copying it, and anything that takes a buffer takes it. A row that
    /// did not cast holds zero; ``verdicts`` says which those are.
    ///
    /// For every other door, a ``list`` of the Python values HyperCast's package gives
    /// that door — ``decimal.Decimal``, ``uuid.UUID``, ``datetime``, ``date``, ``time``,
    /// ``timedelta``, and ``str`` for text — with ``None`` in a row that did not cast.
    ///
    /// Built on first use and the same object after that.
    #[getter]
    fn values<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
        let made = slot
            .values_view
            .get_or_try_init(py, || -> PyResult<Py<PyAny>> {
                if let Some(format) = primitive_format(slot.door) {
                    return view(py, &slot.values, format, None);
                }
                let companions = companions(py)?;
                let list = PyList::empty(py);
                for row in 0..batch.rows {
                    // A violation cast: its value is there, exactly as cast.
                    if slot.verdict(py, row)?.fault().is_none() {
                        list.append(batch.object(py, companions, slot, row)?)?;
                    } else {
                        list.append(py.None())?;
                    }
                }
                Ok(list.into_any().unbind())
            })?;
        Ok(made.bind(py).clone())
    }

    /// The column's verdicts as the core wrote them: a read-only ``memoryview`` of
    /// unsigned 32-bit integers, shape ``(rows, 3)`` — per row, the offending span's byte
    /// offset and byte length within the cell's text, then the reason: ``0`` for a cell
    /// that cast, ``4`` for one that cast and broke a constraint the column declares (its span
    /// the judged text's), otherwise HyperCast's ``CastFailure`` code.
    #[getter]
    fn verdicts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
        let made = slot
            .verdicts_view
            .get_or_try_init(py, || view(py, &slot.verdicts, "I", Some((batch.rows, 3))))?;
        Ok(made.bind(py).clone())
    }

    /// How many of the column's cells did not cast — empty ones included. Zero means
    /// ``values`` can be taken whole.
    #[getter]
    fn fault_count(&self) -> PyResult<usize> {
        Ok(self.parts()?.1.faults)
    }

    /// The cells that did not cast, in row order: ``(row, Fault)`` for each. A violation cast, so
    /// it isn't one; :meth:`Batch.faults` lists it.
    fn faults<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let (batch, slot) = self.parts()?;
        let companions = companions(py)?;
        let list = PyList::empty(py);
        if slot.faults == 0 {
            return Ok(list);
        }
        for row in 0..batch.rows {
            let verdict = slot.verdict(py, row)?;
            if verdict.fault().is_some() {
                list.append((row, slot.fault(py, companions, verdict, row)?))?;
            }
        }
        Ok(list)
    }

    /// Where the verdict of the cell at ``row`` came from — see ``Batch.origin``.
    fn origin<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
        batch.origin_of(py, slot, batch.row_index(row)?)
    }

    /// The text the cell at ``row`` was cast from — see ``Batch.raw``.
    fn raw<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyBytes>> {
        let (batch, slot) = self.parts()?;
        Ok(batch.raw_text(py, slot, batch.row_index(row)?))
    }

    fn __getitem__<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let batch = self.batch.get();
        let slot = batch.unchecked(self.index)?;
        batch.cell(py, companions(py)?, slot, batch.row_index(row)?)
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let batch = self.batch.get();
        let slot = batch.unchecked(self.index)?;
        let companions = companions(py)?;
        let list = PyList::empty(py);
        for row in 0..batch.rows {
            list.append(batch.cell(py, companions, slot, row)?)?;
        }
        Ok(list.try_iter()?.into_any())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let (batch, slot) = self.parts()?;
        Ok(format!(
            "<hypertabular.ColumnData {} rows={} faults={}>",
            self.column(py)?.repr()?,
            batch.rows,
            slot.faults
        ))
    }
}

/// One row of a :class:`Batch`: the batch read across rather than down, for code that
/// builds a value a row at a time. A view — the batch and an index, nothing copied — whose
/// every method is the batch's own with this row's index. A batch owns what it shows, so a
/// row stays valid for as long as it is referenced, after the reader has moved on.
#[pyclass(frozen, module = "hypertabular")]
struct Row {
    batch: Py<Batch>,
    index: usize,
}

#[pymethods]
impl Row {
    /// The row's place in its batch, from zero.
    #[getter]
    fn index(&self) -> usize {
        self.index
    }

    /// Where the row came from: ``Batch.line``.
    #[getter]
    fn line(&self) -> PyResult<u32> {
        self.batch.get().line(self.index as isize)
    }

    /// The batch the row is a row of.
    #[getter]
    fn batch(&self, py: Python<'_>) -> Py<Batch> {
        self.batch.clone_ref(py)
    }

    /// The cell in column ``column`` as HyperCast's verdict: ``Batch.get``.
    fn get<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Bound<'py, PyAny>> {
        let batch = self.batch.get();
        batch.cell(py, companions(py)?, batch.unchecked(column)?, self.index)
    }

    /// The cell in column ``column`` as HyperCast's checked verdict: ``Batch.get_checked``.
    fn get_checked<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Bound<'py, PyAny>> {
        self.batch.get().checked_cell(py, column, self.index)
    }

    /// The text cell in column ``column`` as a ``str``, or ``None``: ``Batch.text``.
    fn text<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.batch.get().text_cell(py, column, self.index)
    }

    /// Where the verdict of the cell in column ``column`` came from: ``Batch.origin``.
    fn origin<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Bound<'py, PyAny>> {
        let batch = self.batch.get();
        batch.origin_of(py, batch.slot(column)?, self.index)
    }

    /// The text the cell in column ``column`` was cast from: ``Batch.raw``.
    fn raw<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Bound<'py, PyBytes>> {
        let batch = self.batch.get();
        Ok(batch.raw_text(py, batch.slot(column)?, self.index))
    }

    fn __len__(&self) -> usize {
        self.batch.get().slots.len()
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!(
            "<hypertabular.Row index={} line={}>",
            self.index,
            self.line()?
        ))
    }
}

/// Where a :class:`Rows` takes its next batch from once the one in hand is done.
enum Feeds {
    Delimited(Py<DelimitedReader>),
    Sheet(Py<Sheet>),
}

/// An iterator of :class:`Row` views: over one batch (``iter(batch)``), or over every batch
/// a reader or sheet has left (``reader.rows()``), reading the next as the one in hand runs
/// out.
#[pyclass(module = "hypertabular")]
struct Rows {
    batch: Option<Py<Batch>>,
    next: usize,
    feeds: Option<Feeds>,
}

impl Rows {
    fn over(batch: Py<Batch>) -> Rows {
        Rows {
            batch: Some(batch),
            next: 0,
            feeds: None,
        }
    }

    fn reading(feeds: Feeds) -> Rows {
        Rows {
            batch: None,
            next: 0,
            feeds: Some(feeds),
        }
    }
}

#[pymethods]
impl Rows {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Row>> {
        loop {
            if let Some(batch) = &self.batch
                && self.next < batch.get().rows
            {
                let row = Row {
                    batch: batch.clone_ref(py),
                    index: self.next,
                };
                self.next += 1;
                return Ok(Some(row));
            }
            let read = match &self.feeds {
                None => None,
                Some(Feeds::Delimited(reader)) => {
                    exclusive(reader.bind(py), "reader")?.next_batch(py)?
                }
                Some(Feeds::Sheet(sheet)) => exclusive(sheet.bind(py), "sheet")?.next_batch(py)?,
            };
            let Some(batch) = read else {
                self.batch = None;
                self.feeds = None;
                return Ok(None);
            };
            self.batch = Some(Py::new(py, batch)?);
            self.next = 0;
        }
    }
}

/// The extension's own reader or future, held for `&mut` use only: nothing reaches it through a
/// shared reference, which is what makes it `Sync` whatever a boxed source is.
struct Exclusive<T>(T);

// SAFETY: the only way to the value is `&mut self` on the holder, so a shared `&Exclusive<T>` gives
// no access to `T` at all, which is what `Sync` asks. `T: Send` covers the move across threads.
unsafe impl<T: Send> Sync for Exclusive<T> {}

/// A stream the caller holds, read through the Rust reader's asynchronous source: a read of it
/// that can't go on now is `Pending`, and the reader is cancel-safe, so that the step is simply
/// taken again later from where it was. Which is what's wanted of both kinds: a raising file
/// object loses nothing and is asked again, and an asynchronous stream is awaited in Python.
struct Incoming {
    reads: Reads,
    held: Arc<Mutex<Held>>,
}

/// What a stream is read from.
enum Reads {
    /// The caller's binary file object: anything with ``read(size)``. Its ``read1(size)`` is
    /// called instead where it has one (``read1``), because a buffered reader's ``read(size)`` over
    /// a pipe or a socket waits for all ``size`` bytes, and with them the records already complete
    /// inside the first.
    Object { object: Py<PyAny>, read1: bool },
    /// A file `DelimitedReader.open` opened, read as CPython reads one: see [`read_file`].
    File(File),
    /// The caller's asynchronous stream, which this reader never calls: a step says how much it
    /// wants instead, and ``_aio`` awaits that much from the stream and feeds it in.
    Fed,
}

/// How many times to look for a borrow held only a moment: see [`fed_held`].
const FED_HELD_TRIES: usize = 100_000;

/// What a read of a caller's stream leaves between the reader and the Python call that drives it.
#[derive(Default)]
struct Held {
    /// A fed stream's last chunk, `at..` of it not yet read.
    chunk: Vec<u8>,
    at: usize,
    /// The stream has said it's over, and the reader hasn't been told yet.
    ended: bool,
    /// How much a fed stream was last asked for and hasn't yet been given.
    wanted: usize,
    /// One read of a fed stream is under way, in a step or awaiting what its step asked for:
    /// the one thing that, across an await, stands between it and a second.
    claimed: bool,
    /// What a file object's `read` raised, or a signal's handler while a file was read, for the
    /// caller of this step: the stream is where it was, and the next step asks it again.
    raised: Option<PyErr>,
    /// Why what a stream handed over can't be read: more than was asked for, or not `bytes`. What it
    /// handed over is gone, so this is final.
    broken: Option<PyErr>,
}

/// `error`, kept in `held` as what a stream broke, for the next step to make final.
fn broke(py: Python<'_>, held: &mut Held, error: PyErr) -> PyErr {
    held.broken = Some(error.clone_ref(py));
    error
}

/// The one call a reader or a sheet takes at a time: a call back into it from inside one of its own
/// calls (a stream's ``read``, a plan's iteration), or one from another thread while it runs, raises
/// ``RuntimeError`` with the sentence every other binding uses, where PyO3 would say only that it
/// is "Already borrowed".
fn exclusive<'py, T>(slf: &Bound<'py, T>, noun: &str) -> PyResult<PyRefMut<'py, T>>
where
    T: pyo3::PyClass<Frozen = pyo3::pyclass::boolean_struct::False>,
{
    slf.try_borrow_mut().map_err(|_| {
        PyRuntimeError::new_err(format!(
            "the {noun} is in the middle of a call; nothing that call runs can call back into it"
        ))
    })
}

fn locked(held: &Mutex<Held>) -> std::sync::MutexGuard<'_, Held> {
    // Nothing holding this lock calls anything that can panic.
    held.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why a read of a caller's stream came to nothing.
enum Unread {
    /// The stream raised: it is where it was, and the next read asks it again.
    Raised(PyErr),
    /// The stream handed over something that can't be read, and it's gone: final.
    Broken(PyErr),
}

/// One read of a caller's file object into `into`.
fn read_object(
    py: Python<'_>,
    object: &Py<PyAny>,
    read1: bool,
    into: &mut [u8],
) -> Result<usize, Unread> {
    let method = if read1 { "read1" } else { "read" };
    let chunk = object
        .bind(py)
        .call_method1(method, (into.len(),))
        .map_err(Unread::Raised)?;
    let chunk = chunk.cast::<PyBytes>().map_err(|_| {
        Unread::Broken(PyTypeError::new_err(
            "read() must return bytes: open the file in binary mode",
        ))
    })?;
    let chunk = chunk.as_bytes();
    let room = into.len();
    let Some(landing) = into.get_mut(..chunk.len()) else {
        return Err(Unread::Broken(PyValueError::new_err(format!(
            "{method}({room}) returned more than size bytes"
        ))));
    };
    landing.copy_from_slice(chunk);
    Ok(chunk.len())
}

/// Opens the file at `path` for reading as CPython's `open` does, so that a signal's handler runs
/// while it waits, as it does for a FIFO with no writer yet, and an exception it raises ends the
/// wait. Python's `os.open` releases the interpreter while it waits.
#[cfg(unix)]
fn open_file(py: Python<'_>, path: &Path) -> PyResult<File> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let os = py.import("os")?;
    let flags: i32 = os.getattr("O_RDONLY")?.extract()?;
    let fd: i32 = os.call_method1("open", (path, flags))?.extract()?;
    // SAFETY: `os.open` has just made the descriptor, and nothing else holds it.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Opens the file at `path` for reading, with the interpreter released.
#[cfg(not(unix))]
fn open_file(py: Python<'_>, path: &Path) -> PyResult<File> {
    Ok(py.detach(|| File::open(path))?)
}

/// One read of a file this module opened, as CPython reads one (PEP 475): a read a signal cuts
/// short runs the signal's Python handler, and is made again unless the handler raised, which is
/// the outer `Err`. Called with the interpreter released.
fn read_file(file: &File, into: &mut [u8]) -> PyResult<std::io::Result<usize>> {
    loop {
        match (&*file).read(into) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                Python::attach(|py| py.check_signals())?;
            }
            read => return Ok(read),
        }
    }
}

/// A file this module opened, read for a call that reads it through once (a suggestion's sample, a
/// workbook): an exception a signal's handler raises is kept in `raised`, for the call to raise in
/// place of the read's failure, which only says that it was cut short.
struct Checked<'a> {
    file: File,
    raised: &'a Mutex<Option<PyErr>>,
}

impl Read for Checked<'_> {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        read_file(&self.file, into).unwrap_or_else(|raised| {
            *self
                .raised
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(raised);
            Err(std::io::Error::other("a signal's handler raised"))
        })
    }
}

/// What `raised` holds once a read through [`Checked`] is over, if anything.
fn taken(raised: &Mutex<Option<PyErr>>) -> Option<PyErr> {
    raised
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

// Nothing here is pinned structurally: a read only ever goes through `&mut`.
impl Unpin for Incoming {}

impl futures_io::AsyncRead for Incoming {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        into: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match &this.reads {
            Reads::Object { object, read1 } => {
                match Python::attach(|py| read_object(py, object, *read1, into)) {
                    Ok(read) => Poll::Ready(Ok(read)),
                    Err(Unread::Raised(raised)) => {
                        locked(&this.held).raised = Some(raised);
                        Poll::Pending
                    }
                    Err(Unread::Broken(broken)) => {
                        locked(&this.held).broken = Some(broken);
                        Poll::Pending
                    }
                }
            }
            Reads::File(file) => match read_file(file, into) {
                Ok(read) => Poll::Ready(read),
                Err(raised) => {
                    locked(&this.held).raised = Some(raised);
                    Poll::Pending
                }
            },
            Reads::Fed => {
                let mut held = locked(&this.held);
                let Held {
                    chunk, at, ended, ..
                } = &mut *held;
                let rest = chunk.get(*at..).unwrap_or_default();
                if !rest.is_empty() {
                    let read = rest.len().min(into.len());
                    if let (Some(to), Some(from)) = (into.get_mut(..read), rest.get(..read)) {
                        to.copy_from_slice(from);
                    }
                    *at += read;
                    return Poll::Ready(Ok(read));
                }
                if std::mem::take(ended) {
                    return Poll::Ready(Ok(0));
                }
                held.wanted = into.len();
                Poll::Pending
            }
        }
    }
}

/// A reader over a caller's stream, its header still being read.
type Opening =
    Pin<Box<dyn Future<Output = Result<crate::DelimitedReader<'static>, crate::Error>> + Send>>;

/// A plan, checked: the caller's columns, the crate's, and their doors.
struct Planned {
    columns: Py<PyTuple>,
    planned: Vec<crate::Column>,
    doors: Vec<Door>,
}

impl Planned {
    fn of(py: Python<'_>, plan: &Bound<'_, PyAny>) -> PyResult<Self> {
        let companions = companions(py)?;
        let mut columns = Vec::new();
        let mut planned = Vec::new();
        let mut doors = Vec::new();
        for (index, column) in plan.try_iter()?.enumerate() {
            let column = column?;
            let (door, made) = column_of(py, companions, index, &column)?;
            columns.push(column);
            planned.push(made);
            doors.push(door);
        }
        Ok(Planned {
            columns: PyTuple::new(py, columns)?.unbind(),
            planned,
            doors,
        })
    }
}

/// A numeric notation as the core reads it, from the bytes HyperCast's `NumFormat` packs itself
/// into: the door a format holding a retired word is refused at.
fn num_format_of(py: Python<'_>, format: &Bound<'_, PyAny>, what: &str) -> PyResult<NumFormat> {
    let companions = companions(py)?;
    if !format.is_instance(companions.num_format.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "{what} must be a hypercast.NumFormat, not {}",
            format.get_type().name()?
        )));
    }
    let packed: [u8; 92] = format.getattr("packed")?.extract()?;
    let spec = ColumnSpec {
        ordinal: 0,
        door: Door::I32.code(),
        param: 0,
        format: RawNumFormat::from_le_bytes(packed),
    };
    spec.num_format()
        .ok_or_else(|| PyValueError::new_err(format!("{what} cannot be honoured")))
}

/// One plan column as the crate takes it, checked: the door it names and everything it declares.
fn column_of(
    py: Python<'_>,
    companions: &Companions,
    index: usize,
    column: &Bound<'_, PyAny>,
) -> PyResult<(Door, crate::Column)> {
    if !column.is_instance(companions.column.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "plan[{index}] must be a Column, not {}",
            column.get_type().name()?
        )));
    }
    let format = num_format_of(
        py,
        &column.getattr("format")?,
        &format!("plan[{index}].format"),
    )?;
    let ordinal: u32 = column.getattr("ordinal")?.extract()?;
    // `Column` refuses an ordinal past the cap itself; one that got past it is refused here.
    if ordinal as usize > column::MAX_ORDINAL {
        return Err(PyValueError::new_err(format!(
            "plan[{index}] reads source column {ordinal}; an ordinal must be below 2^20 (1,048,576)"
        )));
    }
    let declared = column.getattr("declared")?;
    let param = if declared.is_none() {
        0
    } else {
        declared.extract()?
    };
    let door = Door::from_code(column.getattr("door")?.extract()?, param).ok_or_else(|| {
        PyValueError::new_err(format!(
            "plan[{index}] names no door; build it with Column's factories"
        ))
    })?;
    let constraints = column.getattr("constraints")?;
    let mut made = if constraints.is_none() {
        crate::Column::new(ordinal as usize, door)
    } else {
        checked_of(py, companions, index, ordinal as usize, door, &constraints)?
    }
    .format(format);
    let tokens = column.getattr("tokens")?;
    if !tokens.is_none() {
        let tokens: Vec<String> = tokens.extract()?;
        made = made.tokens(&tokens.iter().map(String::as_str).collect::<Vec<_>>());
    }
    let options = column.getattr("date_options")?;
    let months = column.getattr("months")?;
    if !options.is_none() || !months.is_none() {
        let date = if options.is_none() {
            DateOptions::NONE
        } else {
            DateOptions::from_flags(options.getattr("flags")?.extract()?)
                .with_pivot(options.getattr("pivot")?.extract()?)
        };
        let packed: Option<Vec<u8>> = if months.is_none() {
            None
        } else {
            Some(months.getattr("packed")?.extract()?)
        };
        let names = packed
            .as_deref()
            .map(MonthNames::from_packed)
            .transpose()
            .map_err(|_| {
                PyValueError::new_err(format!("plan[{index}].months cannot be honoured"))
            })?;
        made = made.date_options(date, names);
    }
    made.spec_with(index).map_err(|_| {
        PyValueError::new_err(format!("plan[{index}]'s date options cannot be honoured"))
    })?;
    Ok((door, made))
}

/// A plan column that declares HyperCast's `Constraints`, refused unless they are for its own
/// door: a Python column can pair any door with any declaration, so the pairing is held here.
fn checked_of(
    py: Python<'_>,
    companions: &Companions,
    index: usize,
    ordinal: usize,
    door: Door,
    constraints: &Bound<'_, PyAny>,
) -> PyResult<crate::Column> {
    if !constraints.is_instance(companions.constraints.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "plan[{index}].constraints must be a hypercast.Constraints, not {}",
            constraints.get_type().name()?
        )));
    }
    let declared: u32 = constraints.getattr("door")?.extract()?;
    let packed: Vec<u8> = constraints.getattr("packed")?.extract()?;
    let entries: Vec<u8> = constraints.getattr("packed_enum")?.extract()?;
    let named: String = constraints.getattr("door")?.getattr("name")?.extract()?;
    column::checked_from_packed(ordinal, door, declared, &packed, &entries).map_err(|refusal| {
        match refusal {
            column::Undeclarable::NotPacked => PyRuntimeError::new_err(CONTRACT),
            column::Undeclarable::Mismatched => PyValueError::new_err(format!(
                "plan[{index}]: Constraints for Door.{named} were given to a Door.{} column",
                format!("{door:?}").to_uppercase()
            )),
            column::Undeclarable::Refused => {
                PyValueError::new_err(format!("plan[{index}].constraints cannot be honoured"))
            }
        }
    })
}

/// The package's `Dialect` as the crate takes it. Each distinct token list is kept for the life of
/// the process, as the docs of `Dialect.tokens` say.
fn dialect_of(py: Python<'_>, dialect: &Bound<'_, PyAny>) -> PyResult<crate::Dialect> {
    if !dialect.is_instance(companions(py)?.dialect.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "dialect must be a Dialect, not {}",
            dialect.get_type().name()?
        )));
    }
    let separator: String = dialect.getattr("separator")?.extract()?;
    let &[byte] = separator.as_bytes() else {
        return Err(rust_error(py, crate::Error::Separator(0)));
    };
    let short_rows = match dialect.getattr("short_rows")?.extract::<u32>()? {
        0 => ShortRows::Reject,
        1 => ShortRows::Pad,
        other => return Err(PyValueError::new_err(format!("no short_rows {other}"))),
    };
    let long_rows = match dialect.getattr("long_rows")?.extract::<u32>()? {
        0 => LongRows::Reject,
        1 => LongRows::Report,
        2 => LongRows::Truncate,
        other => return Err(PyValueError::new_err(format!("no long_rows {other}"))),
    };
    let encoding = match dialect.getattr("encoding")?.extract::<u32>()? {
        0 => Encoding::Utf8,
        1 => Encoding::Windows1252,
        2 => Encoding::Latin1,
        3 => Encoding::Utf16Le,
        other => return Err(PyValueError::new_err(format!("no encoding {other}"))),
    };
    let tokens: Vec<String> = dialect.getattr("tokens")?.extract()?;
    let mut made = crate::Dialect::new(byte)
        .with_quoting(dialect.getattr("quoting")?.is_truthy()?)
        .with_header(dialect.getattr("has_header")?.is_truthy()?)
        .with_blank_lines_skipped(dialect.getattr("skip_blank_lines")?.is_truthy()?)
        .with_skip(dialect.getattr("skip")?.extract()?)
        .with_header_row(dialect.getattr("header_row")?.extract()?)
        .with_short_rows(short_rows)
        .with_long_rows(long_rows)
        .with_encoding(encoding);
    if !tokens.is_empty() {
        made = made.with_tokens(&tokens.iter().map(String::as_str).collect::<Vec<_>>());
    }
    Ok(made)
}

/// The package's `SheetOptions` as the crate takes them.
fn sheet_options_of(options: &Bound<'_, PyAny>) -> PyResult<crate::SheetOptions> {
    let batch_rows: usize = options.getattr("batch_rows")?.extract()?;
    let tokens: Vec<String> = options.getattr("tokens")?.extract()?;
    let mut made = crate::SheetOptions::new()
        .with_header(options.getattr("has_header")?.is_truthy()?)
        .with_empty_rows_skipped(options.getattr("skip_empty_rows")?.is_truthy()?)
        .with_batch_rows(batch_rows)
        .with_skip(options.getattr("skip")?.extract()?)
        .with_header_row(options.getattr("header_row")?.extract()?)
        .with_notes(options.getattr("notes")?.is_truthy()?);
    let range = options.getattr("range")?;
    if !range.is_none() {
        made = made.with_range(range.cast::<Range>()?.get().range);
    }
    if !tokens.is_empty() {
        made = made.with_tokens(&tokens.iter().map(String::as_str).collect::<Vec<_>>());
    }
    Ok(made)
}

/// The package's `SuggestOptions` as the crate takes them: every format it holds judged, first of
/// anything, as the crate's own suggest would before it opened the source.
fn suggest_options_of(
    py: Python<'_>,
    options: Option<&Bound<'_, PyAny>>,
) -> PyResult<crate::SuggestOptions> {
    let mut made = crate::SuggestOptions::new();
    let Some(options) = options else {
        return Ok(made);
    };
    if !options.is_instance(companions(py)?.suggest_options.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "options must be a SuggestOptions, not {}",
            options.get_type().name()?
        )));
    }
    let order_of = |order: &Bound<'_, PyAny>| -> PyResult<DateOrder> {
        match Door::from_code(
            Door::DateOrdered(DateOrder::YearMonthDay).code(),
            order.extract()?,
        ) {
            Some(Door::DateOrdered(order)) => Ok(order),
            _ => Err(PyValueError::new_err("no such date order")),
        }
    };
    made = made.sample_rows(options.getattr("sample_rows")?.extract()?);
    let format = options.getattr("format")?;
    if !format.is_none() {
        made = made.format(num_format_of(py, &format, "SuggestOptions.format")?);
    }
    for entry in options.getattr("column_formats")?.try_iter()? {
        let (ordinal, format): (usize, Bound<'_, PyAny>) = entry?.extract()?;
        let what = format!("SuggestOptions.column_formats[{ordinal}]");
        made = made.column_format(ordinal, num_format_of(py, &format, &what)?);
    }
    let order = options.getattr("date_order")?;
    if !order.is_none() {
        made = made.date_order(order_of(&order)?);
    }
    for entry in options.getattr("column_date_orders")?.try_iter()? {
        let (ordinal, order): (usize, Bound<'_, PyAny>) = entry?.extract()?;
        made = made.column_date_order(ordinal, order_of(&order)?);
    }
    Ok(made)
}

/// What a door declares beside itself, as the core numbers it.
fn door_param(door: Door) -> u32 {
    match door {
        Door::Unix(precision) => precision as u32,
        Door::DateOrdered(order) | Door::DateTime(order) => order as u32,
        Door::ExcelSerial(epoch) => epoch as u32,
        _ => 0,
    }
}

/// Which of the formats a suggestion can name this is: `1` Western, `2` the comma decimal, and `0`
/// the one the caller declared, which the Python side has as the caller's own object.
fn format_kind(format: NumFormat) -> u32 {
    if format == COMMA_GROUPS {
        1
    } else if format == COMMA_DECIMAL {
        2
    } else {
        0
    }
}

/// The package's `Suggestion` for what the crate found.
fn suggestion_of<'py>(
    py: Python<'py>,
    found: &crate::Suggestion,
    options: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let columns = PyList::empty(py);
    for (column, planned) in found.report.iter().zip(&found.plan) {
        let candidates = PyList::empty(py);
        for candidate in &column.candidates {
            match candidate {
                Candidate::DateOrder(order) => candidates.append((0u32, *order as u32))?,
                Candidate::NumFormat(format) => candidates.append((1u32, format_kind(*format)))?,
            }
        }
        let status = match column.status {
            SuggestStatus::Resolved => 1u32,
            SuggestStatus::Ambiguous => 2,
            SuggestStatus::Unresolved => 3,
        };
        columns.append((
            (
                column.ordinal,
                column.name.as_deref().map(|name| text(py, name)),
                column.door.code(),
                door_param(column.door),
                column.format.map(format_kind),
                candidates,
                status,
                column.matched,
                column.sampled,
            ),
            (
                planned.door.code(),
                door_param(planned.door),
                format_kind(planned.format),
            ),
        ))?;
    }
    companions(py)?
        .suggestion
        .bind(py)
        .call1((columns, found.truncated, options))
}

/// One poll of a reader's asynchronous source, for a waker nothing wakes: `Pending` is only ever a
/// stream with nothing to give yet, and the next step is taken when the Python caller has seen to
/// that. Dropping the read here loses nothing: it's what the Rust reader's cancel-safety promises.
fn poll_read<'r>(
    reader: &'r mut crate::DelimitedReader<'static>,
) -> Poll<Result<Option<crate::Batch<'r>>, crate::Error>> {
    let mut context = Context::from_waker(Waker::noop());
    pin!(reader.read_async()).poll(&mut context)
}

/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time
/// into typed columns, every cell a HyperCast verdict.
///
/// ``DelimitedReader(source, dialect, plan=None, *, batch_rows=4096, buffer_bytes=262144,
/// notes=True)`` reads ``source``: ``bytes``, read in place and never copied unless the dialect
/// declares an encoding, or a binary file object — anything with ``read(size)``, called as
/// ``read1(size)`` where it has one — read forward only. ``DelimitedReader.open(path, …)`` opens a
/// file, and ``await DelimitedReader.open_async(stream, …)`` reads an asynchronous stream. Whatever
/// a read returns is parsed at once, so a pipe's complete records are not held back for more.
/// ``dialect`` is a :class:`Dialect` and ``plan`` an iterable of :class:`Column`: nothing is
/// sniffed and nothing is inferred.
///
/// The header, when the dialect declares one, is read as the reader is made. A reader made
/// without a plan reads it all the same and waits for one: ``bind(plan)``, once, with
/// ordinals the :class:`Header` has looked up by name.
///
/// Iterating yields :class:`Batch` objects of up to ``batch_rows`` rows. A value that does
/// not cast is that cell's verdict, and the read goes on. Input that is not rows of cells
/// at all — a record of the wrong width, a quote never closed, a unit a declared encoding has no
/// character for — raises :class:`TabularError`, after every intact row before it has been
/// delivered, and again on every later read.
///
/// ``notes=False`` drops each cell's detail and origin: a ``Fault`` then has no ``detail``,
/// ``origin`` says ``Origin.CELL`` and :meth:`Batch.faults` can't list what nothing was kept of.
///
/// A reader is read by one thread at a time — another thread calling into it meanwhile gets
/// ``RuntimeError`` — and the batches it has handed out are immutable and can go anywhere.
#[pyclass(module = "hypertabular")]
struct DelimitedReader {
    /// The Rust reader: `None` once closed, or while a fed stream's header is still to come. Ahead
    /// of `input`, which it borrows: fields drop in order.
    reader: Option<Exclusive<crate::DelimitedReader<'static>>>,
    /// A fed stream's reader, on the way.
    opening: Option<Exclusive<Opening>>,
    /// The caller's `bytes`, when that's the source: read in place, and kept alive by this.
    input: Option<Py<PyBytes>>,
    /// Whether a batch's text is `input` itself, which it isn't under a declared encoding.
    shares: bool,
    /// What's between the reader and a caller's stream, when that's the source.
    held: Option<Arc<Mutex<Held>>>,
    /// The caller's asynchronous stream, for `read_async` to await.
    stream: Option<Py<PyAny>>,
    dialect: Py<PyAny>,
    /// The plan, as the caller's own `Column`s: empty until one is bound.
    plan: Py<PyTuple>,
    /// A plan handed to `open_async`, until the header it's held to has been read.
    waiting: Option<Planned>,
    bound: bool,
    doors: Vec<Door>,
    batch_rows: usize,
    /// What the Rust reader last said of its position, which is still there when it's gone.
    records: u64,
    width: Option<usize>,
    /// The package's `Header`, once read.
    header: Option<Py<PyAny>>,
    /// The structural failure that ended the input: final, and raised again on every read.
    failure: Option<PyErr>,
}

/// What one step of a read came to.
enum Step {
    Batch(Batch),
    /// The input is exhausted.
    End,
    /// A fed stream has to be given up to this many bytes before the read can go on.
    Wants(usize),
}

/// How a reader is to start: what it reads, and what's kept for it.
struct Start {
    opened: Opened,
    input: Option<Py<PyBytes>>,
    shares: bool,
    held: Option<Arc<Mutex<Held>>>,
    stream: Option<Py<PyAny>>,
}

enum Opened {
    Reader(Box<crate::DelimitedReader<'static>>),
    Opening(Opening),
}

/// A fed reader's `Held`. A stream's bytes in hand can't be left to a borrow that's busy, since the
/// await that got them can't be undone: only a moment's `close`, `bind` or refused `read` can hold
/// one here, a second `_advance` being refused by the claim, so this waits that out, for as long
/// as a moment's could be.
fn fed_held(slf: &Bound<'_, DelimitedReader>) -> PyResult<Option<Arc<Mutex<Held>>>> {
    for _ in 0..FED_HELD_TRIES {
        if let Ok(reader) = slf.try_borrow() {
            return Ok(reader.stream.as_ref().and(reader.held.clone()));
        }
        slf.py().detach(std::thread::yield_now);
    }
    let reader = slf.try_borrow()?;
    Ok(reader.stream.as_ref().and(reader.held.clone()))
}

impl DelimitedReader {
    /// A reader from what's been opened, with the plan bound once the header has been read. The
    /// plan's refusals but one come first, ahead of anything being opened; that one has to wait to
    /// see the header's width.
    fn assemble(
        py: Python<'_>,
        start: Start,
        dialect: &Bound<'_, PyAny>,
        plan: Option<Planned>,
        batch_rows: usize,
    ) -> PyResult<Self> {
        let (reader, opening) = match start.opened {
            Opened::Reader(reader) => (Some(Exclusive(*reader)), None),
            Opened::Opening(opening) => (None, Some(Exclusive(opening))),
        };
        let mut made = DelimitedReader {
            reader,
            opening,
            input: start.input,
            shares: start.shares,
            held: start.held,
            stream: start.stream,
            dialect: dialect.clone().unbind(),
            plan: PyTuple::empty(py).unbind(),
            waiting: plan,
            bound: false,
            doors: Vec::new(),
            batch_rows,
            records: 0,
            width: None,
            header: None,
            failure: None,
        };
        if made.reader.is_some() {
            made.settled(py)?;
        }
        Ok(made)
    }

    /// The header's been read: takes it and binds the plan that was waiting for it.
    fn settled(&mut self, py: Python<'_>) -> PyResult<()> {
        let Some(Exclusive(reader)) = &self.reader else {
            return Err(PyRuntimeError::new_err(CONTRACT));
        };
        self.records = reader.records();
        self.width = reader.column_count();
        self.header = reader
            .header()
            .map(|header| header_of(py, header.names()))
            .transpose()?;
        match self.waiting.take() {
            Some(plan) => self.bind_planned(py, plan),
            None => Ok(()),
        }
    }

    /// Binds a checked plan. One refused leaves the reader as it was: unbound.
    fn bind_planned(&mut self, py: Python<'_>, plan: Planned) -> PyResult<()> {
        if let Some(width) = self.width
            && let Some((index, column)) = plan
                .planned
                .iter()
                .enumerate()
                .find(|(_, column)| column.ordinal >= width)
        {
            return Err(PyValueError::new_err(format!(
                "plan[{index}] reads source column {}, past the {width} columns the header gives every record",
                column.ordinal
            )));
        }
        let Some(Exclusive(reader)) = &mut self.reader else {
            return Err(PyValueError::new_err("bind of a closed DelimitedReader"));
        };
        reader
            .bind(&plan.planned)
            .map_err(|error| rust_error(py, error))?;
        self.plan = plan.columns;
        self.doors = plan.doors;
        self.bound = true;
        Ok(())
    }

    /// The next batch, for a caller that cannot feed a stream: a fed one's want is an
    /// error here, saying what to call instead.
    fn next_batch(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        match self.step(py)? {
            Step::Batch(batch) => Ok(Some(batch)),
            Step::End => Ok(None),
            Step::Wants(_) => Err(PyRuntimeError::new_err(
                "this reader reads an asynchronous stream: await read_async(), or async for",
            )),
        }
    }

    /// One step of a read: a batch, the end, or — for a fed stream — how much it wants
    /// before it can go on. A step that wants is taken again once fed, from where it was.
    fn step(&mut self, py: Python<'_>) -> PyResult<Step> {
        if !self.bound {
            return Err(out_of_order(UNBOUND));
        }
        if let Some(failure) = &self.failure {
            return Err(failure.clone_ref(py));
        }
        let Some(Exclusive(reader)) = &mut self.reader else {
            return Err(PyValueError::new_err("read of a closed DelimitedReader"));
        };
        let held = self.held.as_ref();
        let polled = held.is_some();
        // With the interpreter released: everything the reader touches is its own, which nothing
        // else can reach meanwhile — on a free-threaded build too, where `&mut self` is PyO3's
        // atomic borrow of it, held across the detach — and the caller's `bytes`, which is
        // immutable and kept alive by `input`. A file object's read attaches for itself.
        let read = py.detach(|| {
            if polled {
                poll_read(reader)
            } else {
                Poll::Ready(reader.read())
            }
        });
        let step = match read {
            Poll::Ready(Ok(Some(batch))) => {
                let holds = match (&self.input, self.shares) {
                    (Some(bytes), true) => Holds::Shared(bytes.clone_ref(py)),
                    _ => Holds::Stream,
                };
                Ok(Step::Batch(batch_of(
                    py,
                    &batch,
                    &self.plan,
                    &self.doors,
                    holds,
                )?))
            }
            Poll::Ready(Ok(None)) => Ok(Step::End),
            Poll::Ready(Err(error)) => Err(rust_error(py, error)),
            Poll::Pending => {
                let mut held = locked(held.ok_or_else(|| PyRuntimeError::new_err(CONTRACT))?);
                // A stream that handed over what can't be read: final, since that is gone.
                if let Some(broken) = held.broken.take() {
                    self.failure = Some(broken.clone_ref(py));
                    return Err(broken);
                }
                // A raising file object: its exception, and the next read asks it again.
                if let Some(raised) = held.raised.take() {
                    return Err(raised);
                }
                Ok(Step::Wants(held.wanted))
            }
        };
        self.records = reader.records();
        self.width = reader.column_count();
        step.inspect_err(|error| self.failure = Some(error.clone_ref(py)))
    }
}

#[pymethods]
impl DelimitedReader {
    /// Rows per batch unless told otherwise.
    #[classattr]
    const DEFAULT_BATCH_ROWS: usize = DEFAULT_BATCH_ROWS;
    /// The read buffer a stream starts with; it doubles when a record does not fit.
    #[classattr]
    const DEFAULT_BUFFER_BYTES: usize = DEFAULT_BUFFER_BYTES;
    /// The row ceiling for a stream: a single record larger than this is a
    /// ``TabularFailure.ROW_TOO_LONG``.
    #[classattr]
    const MAX_ROW_BYTES: usize = crate::DelimitedReader::MAX_ROW_BYTES;

    #[new]
    #[pyo3(signature = (source, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES, notes = true))]
    fn new(
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
        notes: bool,
    ) -> PyResult<Self> {
        if !source.is_instance_of::<PyBytes>() {
            if source.is_instance_of::<PyString>() || source.hasattr("__fspath__")? {
                return Err(PyTypeError::new_err(
                    "source must be bytes or a binary file object; DelimitedReader.open(path, ...) opens a file",
                ));
            }
            if !source.hasattr("read")? {
                return Err(PyTypeError::new_err(format!(
                    "source must be bytes or a binary file object, not {}",
                    source.get_type().name()?
                )));
            }
        }
        let settings = settings_of(batch_rows, buffer_bytes, notes);
        let read = dialect_of(py, dialect)?;
        let plan = plan.map(|plan| Planned::of(py, plan)).transpose()?;
        check_sizes(py, settings, read, plan.as_ref())?;
        let start = if let Ok(bytes) = source.cast::<PyBytes>() {
            over_bytes(py, bytes, settings, read)?
        } else {
            let reads = Reads::Object {
                object: source.clone().unbind(),
                read1: source.hasattr("read1")?,
            };
            opened_over(py, reads, settings, read)?
        };
        DelimitedReader::assemble(py, start, dialect, plan, batch_rows)
    }

    /// Opens the file at ``path`` — a ``str`` or ``os.PathLike`` — and reads it. The
    /// reader owns the file: ``close()``, or leaving a ``with`` block, closes it.
    #[staticmethod]
    #[pyo3(signature = (path, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES, notes = true))]
    fn open(
        py: Python<'_>,
        path: PathBuf,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
        notes: bool,
    ) -> PyResult<Self> {
        let settings = settings_of(batch_rows, buffer_bytes, notes);
        let read = dialect_of(py, dialect)?;
        let plan = plan.map(|plan| Planned::of(py, plan)).transpose()?;
        check_sizes(py, settings, read, plan.as_ref())?;
        // Read as a file object is, so that a signal that cuts a read short runs its handler as
        // CPython's own reads do, and an exception it raises leaves the reader where it was.
        let reads = Reads::File(open_file(py, &path)?);
        let start = opened_over(py, reads, settings, read)?;
        DelimitedReader::assemble(py, start, dialect, plan, batch_rows)
    }

    /// A reader over an asynchronous stream — anything with ``async read(size)``, such as
    /// an ``asyncio.StreamReader`` — made by a coroutine that reads the header (when the
    /// dialect declares one) before it returns: ``await DelimitedReader.open_async(stream,
    /// dialect)``. Read it with ``await read_async()`` or ``async for``. ``bytes`` are taken
    /// too, and read as the constructor reads them. The stream is the caller's to close.
    #[staticmethod]
    #[pyo3(signature = (stream, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES, notes = true))]
    fn open_async<'py>(
        py: Python<'py>,
        stream: &Bound<'py, PyAny>,
        dialect: &Bound<'py, PyAny>,
        plan: Option<&Bound<'py, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
        notes: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        asynchronous(py, "open_reader")?.call1((
            stream,
            dialect,
            plan,
            batch_rows,
            buffer_bytes,
            notes,
        ))
    }

    /// For ``open_async``: the reader over ``stream``, its header still to be read.
    #[staticmethod]
    #[pyo3(signature = (stream, dialect, plan, batch_rows, buffer_bytes, notes))]
    fn _over_async(
        py: Python<'_>,
        stream: &Bound<'_, PyAny>,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
        notes: bool,
    ) -> PyResult<Self> {
        if !stream.is_instance_of::<PyBytes>() && !stream.hasattr("read")? {
            return Err(PyTypeError::new_err(format!(
                "stream must be bytes or have an async read(size), not {}",
                stream.get_type().name()?
            )));
        }
        let settings = settings_of(batch_rows, buffer_bytes, notes);
        let read = dialect_of(py, dialect)?;
        let plan = plan.map(|plan| Planned::of(py, plan)).transpose()?;
        check_sizes(py, settings, read, plan.as_ref())?;
        let start = if let Ok(bytes) = stream.cast::<PyBytes>() {
            over_bytes(py, bytes, settings, read)?
        } else {
            let (opening, held) = over_stream(Reads::Fed, settings, read);
            Start {
                opened: Opened::Opening(opening),
                input: None,
                shares: false,
                held: Some(held),
                stream: Some(stream.clone().unbind()),
            }
        };
        DelimitedReader::assemble(py, start, dialect, plan, batch_rows)
    }

    /// One step of an asynchronous read: a :class:`Batch`, ``None`` at the end, or how
    /// many bytes the stream is to be read for and fed in (``_feed``) before the next step.
    ///
    /// A second read of a fed stream while one is under way raises ``RuntimeError``: a step alone
    /// can't hold the reader across the await between it and ``_feed``, so the claim does, and
    /// nothing a read that's refused does touches the read that isn't.
    fn _advance(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let claim = {
            let reader = slf.try_borrow()?;
            reader.stream.as_ref().and(reader.held.clone())
        };
        if let Some(held) = &claim {
            let mut held = locked(held);
            if held.claimed {
                return Err(PyRuntimeError::new_err(
                    "another read of this reader is waiting on its stream",
                ));
            }
            held.claimed = true;
        }
        let step = slf
            .try_borrow_mut()
            .map_err(PyErr::from)
            .and_then(|mut reader| reader.step(py));
        if let Some(held) = &claim
            && !matches!(step, Ok(Step::Wants(_)))
        {
            locked(held).claimed = false;
        }
        Ok(match step? {
            Step::Batch(batch) => Py::new(py, batch)?.into_any(),
            Step::End => py.None(),
            Step::Wants(wanted) => wanted.into_pyobject(py)?.into_any().unbind(),
        })
    }

    /// Reads the header of a reader ``_over_async`` made: ``None`` once it is read, or how
    /// many bytes to feed in first.
    fn _header(slf: &Bound<'_, Self>) -> PyResult<Option<usize>> {
        let py = slf.py();
        header_step(&mut *exclusive(slf, "reader")?, py)
    }

    /// Feeds a fed stream's reader what the stream's ``read(size)`` returned for the size
    /// it last asked for; ``b""`` is the end of the stream.
    fn _feed(slf: &Bound<'_, Self>, chunk: &Bound<'_, PyAny>) -> PyResult<()> {
        let Some(held) = fed_held(slf)? else {
            // A reader closed from inside the stream's read, or from anywhere else while the read
            // awaited it: the read can't go on.
            let closed = slf
                .try_borrow()
                .is_ok_and(|reader| reader.reader.is_none() && reader.opening.is_none());
            return Err(if closed {
                PyValueError::new_err(
                    "read of a closed DelimitedReader: it was closed while the read awaited its stream",
                )
            } else {
                PyRuntimeError::new_err("only a reader over an asynchronous stream is fed")
            });
        };
        let mut held = locked(&held);
        if held.wanted == 0 {
            return Err(PyRuntimeError::new_err(
                "the reader asked for nothing to be fed",
            ));
        }
        // What the stream handed over is gone from it, so a chunk that can't be read is final: the
        // next read raises the same.
        let chunk = match chunk.cast::<PyBytes>() {
            Ok(chunk) if chunk.as_bytes().len() <= held.wanted => chunk,
            Ok(_) => {
                return Err(broke(
                    slf.py(),
                    &mut held,
                    PyValueError::new_err("read(size) returned more than size bytes"),
                ));
            }
            Err(_) => {
                return Err(broke(
                    slf.py(),
                    &mut held,
                    PyTypeError::new_err(
                        "read() must return bytes: read the stream in binary mode",
                    ),
                ));
            }
        };
        let chunk = chunk.as_bytes();
        held.chunk.clear();
        held.chunk.extend_from_slice(chunk);
        held.at = 0;
        held.ended = chunk.is_empty();
        held.wanted = 0;
        held.claimed = false;
        Ok(())
    }

    /// Lets go of a read whose await raised, cancellation included: nothing was fed, so the next
    /// read asks the stream for the same again.
    fn _abandon(slf: &Bound<'_, Self>) {
        if let Ok(Some(held)) = fed_held(slf) {
            let mut held = locked(&held);
            held.wanted = 0;
            held.claimed = false;
        }
    }

    /// Binds the plan a reader made without one reads through — once, before the first
    /// read: ``plan`` an iterable of :class:`Column`, typically with ordinals the header
    /// looked up (``header.ordinal("id")``). A second call raises ``RuntimeError``; a plan
    /// that cannot be honoured raises as the constructor would, and leaves the reader
    /// unbound.
    fn bind(slf: &Bound<'_, Self>, plan: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let mut this = exclusive(slf, "reader")?;
        if this.bound {
            return Err(out_of_order(ALREADY_BOUND));
        }
        let plan = Planned::of(py, plan)?;
        this.bind_planned(py, plan)
    }

    /// Reads a sample of the file at ``path`` through the text door and says what its columns
    /// look like: a :class:`Suggestion`, a plan to look at, change and pass to a read, and a
    /// report. Nothing is applied, and nothing here reaches a read: this is its own read of the
    /// file, opened and closed here, and a headerless one is opened twice. ``options`` is a
    /// :class:`SuggestOptions`; a stale or bad format in it raises ``ValueError`` first of
    /// anything. There is no form of this for a stream, which would lose the rows it sampled. In a
    /// coroutine, ``await asyncio.to_thread(DelimitedReader.suggest, ...)``: it releases the
    /// interpreter.
    ///
    /// Declaring the locale is the recommended route. See the README for what a suggestion can
    /// surprise with.
    #[staticmethod]
    #[pyo3(signature = (path, dialect, options = None))]
    fn suggest<'py>(
        py: Python<'py>,
        path: &Bound<'py, PyAny>,
        dialect: &Bound<'py, PyAny>,
        options: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if path.is_instance_of::<PyBytes>() {
            return Err(PyTypeError::new_err(
                "path must be a str or os.PathLike; DelimitedReader.suggest_bytes takes bytes",
            ));
        }
        let asked = suggest_options_of(py, options)?;
        let read = dialect_of(py, dialect)?;
        let path: PathBuf = path.extract()?;
        // Each file the sample opens is opened and read as a file object is: an exception a
        // signal's handler raises ends the sample, and is raised here.
        let raised = Mutex::new(None);
        let found = py.detach(|| {
            crate::DelimitedReader::suggest_from(read, &asked, |rows, dialect| {
                let file = Python::attach(|py| open_file(py, &path)).map_err(|error| {
                    *raised
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                    std::io::Error::other("the file couldn't be opened")
                })?;
                crate::DelimitedReader::options()
                    .notes(false)
                    .batch_rows(rows)
                    .from_reader_unbound(
                        Checked {
                            file,
                            raised: &raised,
                        },
                        dialect,
                    )
            })
        });
        if let Some(raised) = taken(&raised) {
            return Err(raised);
        }
        let found = found.map_err(|error| rust_error(py, error))?;
        suggestion_of(py, &found, options)
    }

    /// :meth:`suggest`, over ``bytes``.
    #[staticmethod]
    #[pyo3(signature = (data, dialect, options = None))]
    fn suggest_bytes<'py>(
        py: Python<'py>,
        data: &Bound<'py, PyAny>,
        dialect: &Bound<'py, PyAny>,
        options: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let data = data.cast::<PyBytes>().map_err(|_| {
            PyTypeError::new_err("data must be bytes; DelimitedReader.suggest takes a path")
        })?;
        let asked = suggest_options_of(py, options)?;
        let read = dialect_of(py, dialect)?;
        let bytes = data.as_bytes();
        let found = py
            .detach(|| crate::DelimitedReader::suggest_slice(bytes, read, &asked))
            .map_err(|error| rust_error(py, error))?;
        suggestion_of(py, &found, options)
    }

    /// Whether a plan has been bound: always, for a reader made with one.
    #[getter]
    fn is_bound(&self) -> bool {
        self.bound
    }

    /// The header's names, when the dialect declares a header: a :class:`Header` — a tuple
    /// of ``str`` that looks a name up — empty for an input with no record at all. ``None``
    /// when the dialect declares none.
    #[getter]
    fn header<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        self.header.as_ref().map(|header| header.bind(py).clone())
    }

    /// How many cells a record has: the header's count once it has been read, otherwise
    /// the first record's once it has been — ``None`` until then.
    #[getter]
    fn column_count(&self) -> Option<usize> {
        self.width
    }

    /// The dialect in force.
    #[getter]
    fn dialect<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.dialect.bind(py).clone()
    }

    /// The plan every batch is filled through: a tuple of :class:`Column`, empty until one
    /// is bound.
    #[getter]
    fn plan<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
        self.plan.bind(py).clone()
    }

    /// Rows per batch, at most.
    #[getter]
    fn batch_rows(&self) -> usize {
        self.batch_rows
    }

    /// Records finished so far — the header, any discarded and skipped blank lines included. The
    /// lines ``Dialect.skip`` passes over aren't records.
    #[getter]
    fn records(&self) -> u64 {
        self.records
    }

    /// Reads the next batch, or returns ``None`` once the input is exhausted.
    ///
    /// Raises :class:`TabularError` for input that is structurally broken — after every
    /// intact row before the break has been delivered, and again on every later call — and
    /// ``RuntimeError`` before a plan is bound (which a later ``bind`` puts right), and from
    /// inside one of this reader's own calls.
    fn read(slf: &Bound<'_, Self>) -> PyResult<Option<Batch>> {
        exclusive(slf, "reader")?.next_batch(slf.py())
    }

    /// ``read()``, as a coroutine: ``await reader.read_async()``. A reader over an
    /// asynchronous stream awaits the stream's ``read(size)`` whenever it needs more input,
    /// with the core's own work done between awaits, the interpreter released; any other
    /// reader completes without awaiting anything.
    ///
    /// Cancellation is asyncio's: a read cancelled while it awaits the stream has fed the
    /// reader nothing, and the next read picks up where it was — the reader is not spoiled.
    /// One read at a time: a second, from another coroutine or thread while one awaits the stream,
    /// raises ``RuntimeError`` and leaves the first as it was.
    fn read_async<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let stream = match &slf.try_borrow()?.stream {
            Some(stream) => stream.clone_ref(py),
            None => py.None(),
        };
        asynchronous(py, "read")?.call1((slf, stream))
    }

    /// Every row left, batch by batch, as :class:`Row` views: the next batch is read as the
    /// one in hand runs out. Each row's batch owns what it shows, so a row stays valid
    /// after the iteration has moved on.
    fn rows(slf: &Bound<'_, Self>) -> Rows {
        Rows::reading(Feeds::Delimited(slf.clone().unbind()))
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Batch>> {
        exclusive(slf, "reader")?.next_batch(slf.py())
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        asynchronous(py, "next_batch")?.call1((Self::read_async(slf)?,))
    }

    /// Lets go of the source: closes the file this reader opened, or drops its reference
    /// to the caller's ``bytes`` or file object (which stays open — it is the caller's).
    /// Batches already read stay valid.
    fn close(slf: &Bound<'_, Self>) -> PyResult<()> {
        let mut this = exclusive(slf, "reader")?;
        this.reader = None;
        this.opening = None;
        this.input = None;
        this.held = None;
        this.stream = None;
        Ok(())
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(slf: &Bound<'_, Self>, _exc: &Bound<'_, PyTuple>) -> PyResult<()> {
        Self::close(slf)
    }
}

/// One step of a header read for :meth:`DelimitedReader._header`.
fn header_step(this: &mut DelimitedReader, py: Python<'_>) -> PyResult<Option<usize>> {
    let Some(Exclusive(opening)) = &mut this.opening else {
        return Ok(None);
    };
    match py.detach(|| poll_opening(opening)) {
        Poll::Ready(reader) => {
            this.opening = None;
            let reader = reader.map_err(|error| rust_error(py, error))?;
            this.reader = Some(Exclusive(reader));
            this.settled(py)?;
            Ok(None)
        }
        Poll::Pending => {
            let held = this
                .held
                .as_ref()
                .ok_or_else(|| PyRuntimeError::new_err(CONTRACT))?;
            Ok(Some(locked(held).wanted))
        }
    }
}

fn settings_of(batch_rows: usize, buffer_bytes: usize, notes: bool) -> crate::DelimitedOptions {
    crate::DelimitedReader::options()
        .batch_rows(batch_rows)
        .buffer_bytes(buffer_bytes)
        .notes(notes)
}

/// The core's answer on these sizes, ahead of reading a thing or allocating for it.
fn check_sizes(
    py: Python<'_>,
    settings: crate::DelimitedOptions,
    dialect: crate::Dialect,
    plan: Option<&Planned>,
) -> PyResult<()> {
    settings
        .checked(dialect, plan.map_or(&[], |plan| plan.planned.as_slice()))
        .map(|_| ())
        .map_err(|error| rust_error(py, error))
}

/// A reader over the caller's `bytes`, read in place, and its header read: the slice is `'static`
/// as far as the reader knows, and `DelimitedReader::input` is what makes that so. It holds the
/// `bytes` for as long as the reader lives, and a `bytes` never changes or moves.
fn over_bytes(
    py: Python<'_>,
    bytes: &Bound<'_, PyBytes>,
    settings: crate::DelimitedOptions,
    dialect: crate::Dialect,
) -> PyResult<Start> {
    let slice: &[u8] = bytes.as_bytes();
    // SAFETY: as above; the reader is dropped ahead of `input`, in field order, and `close` drops
    // it first too.
    let slice: &'static [u8] = unsafe { std::slice::from_raw_parts(slice.as_ptr(), slice.len()) };
    let reader = py
        .detach(|| settings.from_slice_unbound(slice, dialect))
        .map_err(|error| rust_error(py, error))?;
    Ok(Start {
        opened: Opened::Reader(Box::new(reader)),
        input: Some(bytes.clone().unbind()),
        shares: dialect.encoding == Encoding::Utf8,
        held: None,
        stream: None,
    })
}

/// A reader over a stream `DelimitedReader` reads in its poll, its header read. What the stream
/// raises as the header is read is raised here, and no reader is made.
fn opened_over(
    py: Python<'_>,
    reads: Reads,
    settings: crate::DelimitedOptions,
    read: crate::Dialect,
) -> PyResult<Start> {
    let (mut opening, held) = over_stream(reads, settings, read);
    let opened = py.detach(|| poll_opening(&mut opening));
    let reader = match opened {
        Poll::Ready(reader) => reader.map_err(|error| rust_error(py, error))?,
        Poll::Pending => {
            let mut held = locked(&held);
            return Err(held
                .broken
                .take()
                .or_else(|| held.raised.take())
                .unwrap_or_else(|| PyRuntimeError::new_err(CONTRACT)));
        }
    };
    Ok(Start {
        opened: Opened::Reader(Box::new(reader)),
        input: None,
        shares: false,
        held: Some(held),
        stream: None,
    })
}

/// A reader on its way over one of the caller's streams.
fn over_stream(
    reads: Reads,
    settings: crate::DelimitedOptions,
    dialect: crate::Dialect,
) -> (Opening, Arc<Mutex<Held>>) {
    let held = Arc::new(Mutex::new(Held::default()));
    let incoming = Incoming {
        reads,
        held: Arc::clone(&held),
    };
    (
        Box::pin(settings.from_async_reader(incoming, dialect)),
        held,
    )
}

/// One poll of a reader on its way: see [`poll_read`].
fn poll_opening(
    opening: &mut Opening,
) -> Poll<Result<crate::DelimitedReader<'static>, crate::Error>> {
    opening
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

/// The package's `TabularError` for a structural failure the core reported. A code this
/// module does not know is the container's: the most general refusal.
fn tabular_error(
    py: Python<'_>,
    code: u32,
    (record, line, byte): (u64, u32, u64),
    (expected, found): (u32, u32),
) -> PyErr {
    let made = companions(py).and_then(|companions| {
        let failure = companions.failure.bind(py);
        let kind = failure.call1((code,)).or_else(|_| failure.call1((17,)))?;
        companions
            .error
            .bind(py)
            .call1((kind, record, line, byte, expected, found))
    });
    match made {
        Ok(exception) => PyErr::from_value(exception),
        Err(error) => error,
    }
}

/// A read's error as Python raises it.
fn rust_error(py: Python<'_>, error: crate::Error) -> PyErr {
    match error {
        crate::Error::Structure(failure) => tabular_error(
            py,
            failure.kind as u32,
            (failure.record, failure.line, failure.byte),
            (failure.expected, failure.found),
        ),
        crate::Error::Io(error) => std::io::Error::new(error.kind(), error.to_string()).into(),
        crate::Error::NoSheet(which) => {
            PyKeyError::new_err(format!("the workbook has no sheet {which}"))
        }
        crate::Error::Unbound => out_of_order(UNBOUND),
        crate::Error::AlreadyBound => out_of_order(ALREADY_BOUND),
        crate::Error::NoMemory { .. } => PyMemoryError::new_err(error.to_string()),
        other => PyValueError::new_err(other.to_string()),
    }
}

/// The part of a sheet a read keeps: its corners, 1-based like the ``A5:F200`` they're written as,
/// and included. ``Range.parse("A5:F200")`` reads that form: capitals, both corners, not inverted.
/// ``Range(first_row, first_column, last_row, last_column)`` says what it can't, ``0`` being no
/// limit on that side. Anything else raises ``ValueError``.
///
/// A plan's ordinal ``0`` is the range's first column, while ``line`` and ``source_column`` stay
/// the sheet's own. A range past the sheet's end invents no rows.
#[pyclass(frozen, eq, hash, module = "hypertabular")]
#[derive(PartialEq, Eq)]
struct Range {
    range: crate::Range,
}

impl std::hash::Hash for Range {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (
            self.range.first_row(),
            self.range.first_column(),
            self.range.last_row(),
            self.range.last_column(),
        )
            .hash(state);
    }
}

#[pymethods]
impl Range {
    #[new]
    #[pyo3(signature = (first_row = 0, first_column = 0, last_row = 0, last_column = 0))]
    fn new(
        py: Python<'_>,
        first_row: u32,
        first_column: u32,
        last_row: u32,
        last_column: u32,
    ) -> PyResult<Self> {
        let range = crate::Range::new(first_row, first_column, last_row, last_column)
            .map_err(|error| rust_error(py, error))?;
        Ok(Range { range })
    }

    /// The range ``text`` says, in the ``A5:F200`` form. The core reads it, so every binding agrees
    /// on what one is.
    #[staticmethod]
    fn parse(py: Python<'_>, text: &str) -> PyResult<Self> {
        let range = crate::Range::parse(text).map_err(|error| rust_error(py, error))?;
        Ok(Range { range })
    }

    /// The first row, from 1, or ``0`` for no limit.
    #[getter]
    fn first_row(&self) -> u32 {
        self.range.first_row()
    }

    /// The first column, from 1, or ``0`` for no limit.
    #[getter]
    fn first_column(&self) -> u32 {
        self.range.first_column()
    }

    /// The last row, from 1, or ``0`` for no limit.
    #[getter]
    fn last_row(&self) -> u32 {
        self.range.last_row()
    }

    /// The last column, from 1, or ``0`` for no limit.
    #[getter]
    fn last_column(&self) -> u32 {
        self.range.last_column()
    }

    fn __repr__(&self) -> String {
        format!(
            "Range({}, {}, {}, {})",
            self.range.first_row(),
            self.range.first_column(),
            self.range.last_row(),
            self.range.last_column()
        )
    }
}

/// One sheet of a workbook, as :attr:`Workbook.sheets` lists it.
#[pyclass(frozen, eq, module = "hypertabular")]
#[derive(PartialEq)]
struct SheetInfo {
    /// The sheet's name.
    #[pyo3(get)]
    name: String,
    /// Whether the workbook hides the sheet. A hidden sheet reads like any other.
    #[pyo3(get)]
    hidden: bool,
}

#[pymethods]
impl SheetInfo {
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "SheetInfo(name={}, hidden={})",
            PyString::new(py, &self.name).repr()?,
            if self.hidden { "True" } else { "False" }
        ))
    }
}

/// An XLSX, ODS or Numbers workbook held in memory, its sheets listed and its shared strings and
/// styles loaded: what a :class:`Sheet` reads from.
///
/// ``Workbook(data)`` opens ``bytes``, or reads a binary file object — anything with
/// ``read(size)`` — to its end; ``Workbook.open(path)`` reads a file, and ``await
/// Workbook.open_async(stream)`` an asynchronous stream. Which format it is, is read from
/// the bytes, whatever the source was called. A workbook
/// that cannot be read — not a zip, encrypted, a part missing or broken — raises
/// :class:`TabularError`.
#[pyclass(frozen, module = "hypertabular")]
struct Workbook {
    book: Arc<crate::Workbook<'static>>,
    /// The shared strings, once, for every batch of every sheet to index.
    strings: Py<PyBytes>,
    sheets: Py<PyTuple>,
}

impl Workbook {
    fn build(py: Python<'_>, bytes: Vec<u8>) -> PyResult<Self> {
        let book = py
            .detach(|| crate::Workbook::from_vec(bytes))
            .map_err(|error| rust_error(py, error))?;
        let sheets = book.sheets().iter().map(|sheet| SheetInfo {
            name: sheet.name.clone(),
            hidden: sheet.hidden,
        });
        Ok(Workbook {
            strings: PyBytes::new(py, book.strings()).unbind(),
            sheets: PyTuple::new(py, sheets)?.unbind(),
            book: Arc::new(book),
        })
    }
}

/// How much of a file object a workbook asks for at a time.
const WORKBOOK_CHUNK: usize = 1 << 20;

/// Appends a chunk a stream's `read` returned, which has to be `bytes`.
fn append_chunk(into: &mut Vec<u8>, chunk: &Bound<'_, PyAny>) -> PyResult<usize> {
    let chunk = chunk
        .cast::<PyBytes>()
        .map_err(|_| {
            PyTypeError::new_err("read() must return bytes: open the file in binary mode")
        })?
        .as_bytes();
    into.try_reserve(chunk.len())
        .map_err(|_| PyMemoryError::new_err("no memory for a workbook this large"))?;
    into.extend_from_slice(chunk);
    Ok(chunk.len())
}

#[pymethods]
impl Workbook {
    #[new]
    fn new(py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(bytes) = data.cast::<PyBytes>() {
            return Workbook::build(py, bytes.as_bytes().to_vec());
        }
        if data.is_instance_of::<PyString>() || data.hasattr("__fspath__")? {
            return Err(PyTypeError::new_err(
                "data must be bytes or a binary file object; Workbook.open(path) opens a file",
            ));
        }
        if !data.hasattr("read")? {
            return Err(PyTypeError::new_err(format!(
                "data must be bytes or a binary file object, not {}",
                data.get_type().name()?
            )));
        }
        // To its end, into memory the workbook owns: a zip is read from its directory at
        // the end back, which a forward-only stream cannot offer.
        let mut bytes = Vec::new();
        while append_chunk(&mut bytes, &data.call_method1("read", (WORKBOOK_CHUNK,))?)? > 0 {}
        Workbook::build(py, bytes)
    }

    /// A workbook read from an asynchronous stream — anything with ``async read(size)``,
    /// such as an ``asyncio.StreamReader`` — to its end, by a coroutine: ``await
    /// Workbook.open_async(stream)``. The stream is the caller's to close.
    #[staticmethod]
    fn open_async<'py>(py: Python<'py>, stream: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        asynchronous(py, "workbook")?.call1((stream, WORKBOOK_CHUNK))
    }

    /// For ``open_async``: the workbook the chunks an asynchronous stream returned make,
    /// joined once, in memory the workbook owns.
    #[staticmethod]
    fn _from_chunks(py: Python<'_>, chunks: &Bound<'_, PyAny>) -> PyResult<Self> {
        let mut bytes = Vec::new();
        for chunk in chunks.try_iter()? {
            append_chunk(&mut bytes, &chunk?)?;
        }
        Workbook::build(py, bytes)
    }

    /// Reads the file at ``path`` — a ``str`` or ``os.PathLike`` — and opens it.
    #[staticmethod]
    fn open(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        // Read as CPython reads a file: an exception a signal's handler raises ends the read.
        let file = open_file(py, &path)?;
        let raised = Mutex::new(None);
        let bytes = py.detach(|| {
            let size = file.metadata().map_or(0, |metadata| metadata.len());
            let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            Checked {
                file,
                raised: &raised,
            }
            .read_to_end(&mut bytes)
            .map(|_| bytes)
        });
        if let Some(raised) = taken(&raised) {
            return Err(raised);
        }
        Workbook::build(py, bytes?)
    }

    /// Which kind of workbook this is: a :class:`WorkbookFormat`.
    #[getter]
    fn format<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let code = match self.book.format() {
            crate::Format::Xlsx => 1,
            crate::Format::Ods => 2,
            crate::Format::Numbers => 3,
        };
        companions(py)?.workbook_format.bind(py).call1((code,))
    }

    /// The date system the workbook's serials count in — what a date-formatted number is
    /// read by: HyperCast's ``ExcelEpoch``.
    #[getter]
    fn date_system<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        companions(py)?
            .excel_epoch
            .bind(py)
            .call1((self.book.date_system() as u32,))
    }

    /// The workbook's sheets, in its own order: a tuple of :class:`SheetInfo`. Sheets that
    /// hold no cells (chart sheets, macro sheets) are not among them.
    #[getter]
    fn sheets<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
        self.sheets.bind(py).clone()
    }

    /// Starts a read of one sheet — ``which`` is its index in :attr:`sheets` or its name —
    /// through ``plan``, as ``options`` (a :class:`SheetOptions`) says. When the options
    /// declare a header it is read here. Without a plan the header is read all the same,
    /// and the sheet waits for ``Sheet.bind(plan)``. A sheet the workbook does not have
    /// raises ``IndexError`` for an index and ``KeyError`` for a name.
    #[pyo3(signature = (which, options, plan = None))]
    fn sheet(
        &self,
        py: Python<'_>,
        which: &Bound<'_, PyAny>,
        options: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Sheet> {
        let settings = sheet_options_of(options)?;
        let sheet_plan = plan.map(|plan| Planned::of(py, plan)).transpose()?;
        let planned_columns = sheet_plan.as_ref().map(|plan| plan.planned.as_slice());

        let book: &crate::Workbook<'static> = &self.book;
        let shared = &self.book;
        let opened = if let Ok(index) = which.extract::<isize>() {
            let index = usize::try_from(index)
                .ok()
                .filter(|index| *index < book.sheets().len())
                .ok_or_else(|| {
                    PyIndexError::new_err(format!(
                        "the workbook has {} sheets, and no sheet {index}",
                        book.sheets().len()
                    ))
                })?;
            py.detach(|| {
                crate::Workbook::shared_sheet(shared, index.into(), settings, planned_columns)
            })
        } else if let Ok(name) = which.extract::<String>() {
            py.detach(|| {
                crate::Workbook::shared_sheet(
                    shared,
                    name.as_str().into(),
                    settings,
                    planned_columns,
                )
            })
        } else {
            return Err(PyTypeError::new_err(format!(
                "which must be an int or a str, not {}",
                which.get_type().name()?
            )));
        };
        let sheet = opened.map_err(|error| rust_error(py, error))?;
        let header = sheet
            .header()
            .map(|header| header_of(py, header.names()))
            .transpose()?;
        let (plan, doors) = match sheet_plan {
            Some(plan) => (plan.columns, plan.doors),
            None => (PyTuple::empty(py).unbind(), Vec::new()),
        };
        Ok(Sheet {
            sheet,
            book: Arc::clone(&self.book),
            strings: self.strings.clone_ref(py),
            options: options.clone().unbind(),
            plan,
            doors,
            header,
            failure: None,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "<hypertabular.Workbook {:?} sheets={}>",
            self.book.format(),
            self.book.sheets().len()
        )
    }
}

/// A forward-only read of one sheet of a :class:`Workbook`, a batch at a time, through a
/// plan — into the same :class:`Batch` delimited text is read into.
///
/// Iterating yields batches of up to ``options.batch_rows`` rows. A typed cell is converted
/// directly by its door — a stored ``42.0`` never passes through text to become an
/// ``int`` — and a text cell goes through the door as delimited text would. A sheet that is
/// structurally broken raises :class:`TabularError` after every intact row before the break
/// has been delivered, and again on every later read. Each sheet has buffers of its own,
/// so several can be read at once; each keeps its workbook alive.
#[pyclass(module = "hypertabular")]
struct Sheet {
    /// The read itself, holding its own clone of the workbook's `Arc`.
    sheet: crate::Sheet<'static>,
    book: Arc<crate::Workbook<'static>>,
    strings: Py<PyBytes>,
    options: Py<PyAny>,
    /// The plan, as the caller's own `Column`s: empty until one is bound.
    plan: Py<PyTuple>,
    doors: Vec<Door>,
    header: Option<Py<PyAny>>,
    /// The structural failure that ended the sheet: final, and the same exception raised
    /// again on every read.
    failure: Option<PyErr>,
}

impl Sheet {
    fn next_batch(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        // Not kept as the sheet's failure: binding a plan puts it right.
        if !self.sheet.is_bound() {
            return Err(out_of_order(UNBOUND));
        }
        if let Some(failure) = &self.failure {
            return Err(failure.clone_ref(py));
        }
        let read = py.detach(|| self.sheet.read());
        let batch = match read {
            Ok(Some(batch)) => batch,
            Ok(None) => return Ok(None),
            Err(error) => {
                let error = rust_error(py, error);
                self.failure = Some(error.clone_ref(py));
                return Err(error);
            }
        };
        let holds = Holds::Strings(self.strings.clone_ref(py));
        batch_of(py, &batch, &self.plan, &self.doors, holds).map(Some)
    }
}

#[pymethods]
impl Sheet {
    /// The options the sheet is read with.
    #[getter]
    fn options<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.options.bind(py).clone()
    }

    /// The plan every batch is filled through: a tuple of :class:`Column`, empty until one
    /// is bound.
    #[getter]
    fn plan<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
        self.plan.bind(py).clone()
    }

    /// The header row's names — a typed cell said the way the text door says it — as a
    /// :class:`Header`, or ``None`` when the options declare no header.
    #[getter]
    fn header<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        self.header.as_ref().map(|header| header.bind(py).clone())
    }

    /// Binds the plan a sheet opened without one reads through — once, before the first
    /// read: :meth:`DelimitedReader.bind`, for a sheet. A second call raises
    /// ``RuntimeError``; a plan that cannot be honoured leaves the sheet unbound.
    fn bind(slf: &Bound<'_, Self>, plan: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let mut this = exclusive(slf, "sheet")?;
        if this.sheet.is_bound() {
            return Err(out_of_order(ALREADY_BOUND));
        }
        let plan = Planned::of(py, plan)?;
        this.sheet
            .bind(&plan.planned)
            .map_err(|error| rust_error(py, error))?;
        this.plan = plan.columns;
        this.doors = plan.doors;
        Ok(())
    }

    /// :meth:`DelimitedReader.suggest`, of this sheet, with its own options: a sample read apart
    /// from this sheet's own read, which it doesn't touch. A cell typed in the workbook has no
    /// notation to ask about, so a column of number cells is a number under any format, where text
    /// holding the same digits asks. A date column is text, for a plan to change by hand.
    #[pyo3(signature = (options = None))]
    fn suggest<'py>(
        slf: &Bound<'py, Self>,
        options: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let this = exclusive(slf, "sheet")?;
        let asked = suggest_options_of(py, options)?;
        let sheet = &this.sheet;
        let found = py
            .detach(|| sheet.suggest(&asked))
            .map_err(|error| rust_error(py, error))?;
        suggestion_of(py, &found, options)
    }

    /// Whether a plan has been bound: always, for a sheet opened with one.
    #[getter]
    fn is_bound(&self) -> bool {
        self.sheet.is_bound()
    }

    /// Reads the next batch, or returns ``None`` once the sheet has no more rows. Raises
    /// ``RuntimeError`` before a plan is bound, and from inside one of this sheet's own calls.
    fn read(slf: &Bound<'_, Self>) -> PyResult<Option<Batch>> {
        exclusive(slf, "sheet")?.next_batch(slf.py())
    }

    /// Every row left, batch by batch, as :class:`Row` views: :meth:`DelimitedReader.rows`,
    /// for a sheet.
    fn rows(slf: &Bound<'_, Self>) -> Rows {
        Rows::reading(Feeds::Sheet(slf.clone().unbind()))
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Batch>> {
        exclusive(slf, "sheet")?.next_batch(slf.py())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "<hypertabular.Sheet of a {:?} workbook plan={}>",
            self.book.format(),
            self.plan.bind(py).len()
        ))
    }
}

/// One of the coroutine functions in `hypertabular._aio`, which await a stream on a
/// reader's behalf: the core never waits, so the waiting is done in Python.
fn asynchronous<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    py.import("hypertabular._aio")?.getattr(name)
}

/// The core's version as ``"major.minor.patch"``, decoded from the same packed
/// ``hypertabular_version`` every other binding probes.
#[pyfunction]
fn native_version() -> String {
    // SAFETY: the export has no contract to uphold; it is `unsafe` only because every
    // export is declared one way.
    let packed = unsafe { crate::kernel::exports::hypertabular_version() };
    format!(
        "{}.{}.{}",
        packed >> 16,
        (packed >> 8) & 0xff,
        packed & 0xff
    )
}

/// Hands this module the package's own classes — the plan column, the dialect, the
/// structural failure and its kinds, the workbook format, the header — and has it import
/// HyperCast's.
// Each is a class handed over by name, once, from the package's own `__init__`.
#[allow(clippy::too_many_arguments)]
#[pyfunction]
fn _bind(
    py: Python<'_>,
    column: Bound<'_, PyAny>,
    dialect: Bound<'_, PyAny>,
    error: Bound<'_, PyAny>,
    failure: Bound<'_, PyAny>,
    workbook_format: Bound<'_, PyAny>,
    header: Bound<'_, PyAny>,
    problem: Bound<'_, PyAny>,
    origin: Bound<'_, PyAny>,
    suggest_options: Bound<'_, PyAny>,
    suggestion: Bound<'_, PyAny>,
) -> PyResult<()> {
    let hypercast = py.import("hypercast")?;
    let reason = hypercast.getattr("CastFailure")?;
    let companions = Companions {
        success: hypercast.getattr("Success")?.unbind(),
        fault: hypercast.getattr("Fault")?.unbind(),
        checked: hypercast.getattr("Checked")?.unbind(),
        violation: hypercast.getattr("Violation")?.unbind(),
        keyword: hypercast.getattr("Keyword")?.unbind(),
        constraints: hypercast.getattr("Constraints")?.unbind(),
        reasons: [
            reason.getattr("EMPTY")?.unbind(),
            reason.getattr("MALFORMED")?.unbind(),
            reason.getattr("OUT_OF_RANGE")?.unbind(),
        ],
        num_format: hypercast.getattr("NumFormat")?.unbind(),
        column: column.unbind(),
        dialect: dialect.unbind(),
        error: error.unbind(),
        failure: failure.unbind(),
        workbook_format: workbook_format.unbind(),
        excel_epoch: hypercast.getattr("ExcelEpoch")?.unbind(),
        header: header.unbind(),
        problem: problem.unbind(),
        origin: origin.unbind(),
        suggest_options: suggest_options.unbind(),
        suggestion: suggestion.unbind(),
        detail: hypercast.getattr("Detail")?.unbind(),
        values: Values::import(py)?,
    };
    let _ = COMPANIONS.set(py, companions);
    Ok(())
}

/// For the suite, not for callers: lowers a reader's two size limits — the row ceiling a
/// stream's buffer stops growing at, and the most of an in-memory input the core is
/// handed in one call — which are a gibibyte and two, and so otherwise never reached by
/// a test.
#[pyfunction]
fn _limits(mut reader: PyRefMut<'_, DelimitedReader>, max_row_bytes: usize, window_bytes: usize) {
    if let Some(Exclusive(reader)) = &mut reader.reader {
        let ceiling = max_row_bytes.clamp(1, crate::DelimitedReader::MAX_ROW_BYTES);
        reader.limit_window(window_bytes.clamp(1, ceiling));
        reader.limit_ceiling(ceiling);
    }
}

// The name is the last segment of pyproject.toml's module-name ("hypertabular._native"):
// PyO3 makes the `PyInit__native` symbol the import machinery looks for from it.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DelimitedReader>()?;
    m.add_class::<Batch>()?;
    m.add_class::<ColumnData>()?;
    m.add_class::<Row>()?;
    m.add_class::<Rows>()?;
    m.add_class::<Workbook>()?;
    m.add_class::<SheetInfo>()?;
    m.add_class::<Range>()?;
    m.add_class::<Sheet>()?;
    m.add_function(wrap_pyfunction!(native_version, m)?)?;
    m.add_function(wrap_pyfunction!(_bind, m)?)?;
    m.add_function(wrap_pyfunction!(_limits, m)?)?;
    Ok(())
}
