//! The Python binding: the core linked straight into a CPython extension module
//! (`hypertabular._native`, PyO3, abi3-py311) — the whole Python backend, as HyperCast's
//! extension is the whole of its own.
//!
//! Delimited text is read here, over the kernel directly. A workbook is read through this
//! crate's own [`crate::Workbook`] and [`crate::Sheet`], which drive the kernel's workbook
//! calls; each batch they produce is copied into the same Python [`Batch`] delimited text
//! is, the shared strings it indexes held once per workbook as a `bytes` every batch
//! shares.
//!
//! It is a binding like the others, written in Rust: the core
//! ([`crate::kernel::delimited::fill`]) owns no memory and reads no files, so everything
//! it writes into is allocated here — the read buffer, one value array and one verdict
//! array per plan column, the table that locates each cell, the arena — once, and reused
//! for every batch, as the C# binding does. The core fills them in one call per batch,
//! made with the interpreter released.
//!
//! What Python is handed is a [`Batch`] that owns what it shows. C#'s spans are "valid
//! until the next `Read`", a rule Python has no way to hold a caller to — `list(reader)`
//! is the first thing anyone writes — so each array the core filled is copied, once and
//! whole, into a `bytes` of exactly the batch's size, and a column is a read-only
//! `memoryview` over that: the door's own item type for the primitive doors, so anything
//! that takes a buffer takes a column, with no Python call per cell. (Having the core
//! write into a fresh `bytes` directly would save that one `memcpy` and cost a zero-fill
//! of `batch_rows` rows per batch however few rows arrive — and a stream's batch is as
//! long as its buffer happens to hold.) The doors whose value is not a primitive become a
//! `list` of the Python values HyperCast's package gives them, built in one loop here on
//! first use.
//!
//! HyperCast is the judge of every cell, and its Python package is the judge of how a
//! verdict looks in Python: a cell is `hypercast.Success` or `hypercast.Fault` — that
//! package's own classes, called here, not copies of them — carrying its own `CastFailure`
//! members, and a plan column's notation is its own `NumFormat`, crossing as the bytes it
//! packs itself into. A cast value becomes a Python value (`decimal.Decimal`, `uuid.UUID`,
//! the `datetime` types, microsecond truncation) through HyperCast's own conversions
//! (`hypercast::python::Values`), so it is the object `hypercast.cast_*` makes of the same
//! text.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::{
    PyIndexError, PyKeyError, PyMemoryError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBytes, PyList, PyMemoryView, PyString, PyTuple};

use crate::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Failure, Filled,
    OK, Span,
};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use hypercast::python::Values;
use hypercast::{CivilDateTime, Date, Decimal, Duration, RawNumFormat, Reason, Timestamp};

/// Rows per batch unless told otherwise.
const DEFAULT_BATCH_ROWS: usize = 4096;
/// The read buffer a stream starts with; it doubles when a record does not fit.
const DEFAULT_BUFFER_BYTES: usize = 256 * 1024;
/// The row ceiling for a stream: a single record larger than this is a structural failure
/// rather than a buffer that grows without end.
const MAX_ROW_BYTES: usize = 1 << 30;
/// The most input the core takes in one call: its offsets are 31 bits.
const WINDOW: usize = i32::MAX as usize;
/// The arena every reader starts with; it grows when one row's escaped cells need more.
const ARENA_BYTES: usize = 4096;

const CONTRACT: &str =
    "the hypertabular core reported a contract violation — a binding bug, please report it";

/// The Python objects this module presents its results with. HyperCast's are imported
/// from its package; the binding's own (the plan column, the dialect, the structural
/// failure) are defined in `hypertabular/__init__.py` and handed over by `_bind`.
struct Companions {
    success: Py<PyAny>,
    fault: Py<PyAny>,
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
    /// What every cast value is built with: HyperCast's own conversions.
    values: Values,
}

static COMPANIONS: PyOnceLock<Companions> = PyOnceLock::new();

fn companions(py: Python<'_>) -> PyResult<&'static Companions> {
    COMPANIONS
        .get(py)
        .ok_or_else(|| PyRuntimeError::new_err("hypertabular._native used before _bind"))
}

/// Bytes one value of a door takes in a column's value array (`kernel::abi::ColumnBuffer`).
const fn value_size(door: Door) -> usize {
    match door {
        Door::Bool | Door::I8 | Door::U8 => 1,
        Door::I16 | Door::U16 => 2,
        Door::I32 | Door::U32 | Door::F32 => 4,
        Door::I64 | Door::U64 | Door::F64 | Door::Time => 8,
        Door::Decimal => size_of::<Decimal>(),
        Door::Uuid => 16,
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => size_of::<Timestamp>(),
        Door::Date | Door::DateOrdered(_) => size_of::<Date>(),
        Door::DateTime(_) => size_of::<CivilDateTime>(),
        Door::Duration => size_of::<Duration>(),
        Door::Text => size_of::<Span>(),
    }
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

/// `len` zero bytes for the core to write into — or `MemoryError`, where a vector would
/// abort the interpreter.
fn zeroed(len: Option<usize>) -> PyResult<Vec<u8>> {
    let exhausted = || PyMemoryError::new_err("batch_rows is too large for this plan");
    let len = len.ok_or_else(exhausted)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| exhausted())?;
    bytes.resize(len, 0);
    Ok(bytes)
}

/// The column buffers of one call into the core: raw pointers into the reader's own
/// arrays, made for the call and dropped with it.
struct Columns(Vec<ColumnBuffer>);

// SAFETY: the pointers are into vectors the reader owns and holds exclusively (`&mut
// self`) for as long as this value lives; nothing else reads or writes them meanwhile.
unsafe impl Send for Columns {}
// SAFETY: as above — a shared reference to this is only ever used by the one call.
unsafe impl Sync for Columns {}

impl Columns {
    fn as_slice(&self) -> &[ColumnBuffer] {
        &self.0
    }
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

/// A cell's text as a `str`. The input is UTF-8 by contract; bytes that are not are
/// replaced (U+FFFD), as the C# binding's `GetString` does, and `raw` still has them.
fn text<'py>(py: Python<'py>, bytes: &[u8]) -> Bound<'py, PyString> {
    PyString::new(py, &String::from_utf8_lossy(bytes))
}

fn fault<'py>(
    py: Python<'py>,
    companions: &Companions,
    verdict: CellVerdict,
) -> PyResult<Bound<'py, PyAny>> {
    let fault = verdict
        .fault()
        .ok_or_else(|| PyRuntimeError::new_err(CONTRACT))?;
    let reason = match fault.reason {
        Reason::Empty => &companions.reasons[0],
        Reason::Malformed => &companions.reasons[1],
        Reason::OutOfRange => &companions.reasons[2],
    };
    companions
        .fault
        .bind(py)
        .call1((reason.bind(py), fault.offset, fault.len))
}

/// One plan column of a batch: the batch's copies of the two arrays the core filled, and
/// the views of them handed out so far.
struct Slot {
    door: Door,
    /// Where its raw cells are in the cell table: the source column it reads, or — for a
    /// workbook, whose table is laid out by plan column — its place in the plan.
    ordinal: usize,
    values: Py<PyBytes>,
    verdicts: Py<PyBytes>,
    /// Cells that did not cast — empty ones included.
    faults: usize,
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
}

impl Batch {
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
    fn row(&self, row: isize) -> PyResult<usize> {
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
            fault(py, companions, verdict)
        }
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
        let row = self.row(row)?;
        let entry = self
            .cells
            .get(row * self.per_row + self.per_row - 1)
            .copied()
            .unwrap_or_default();
        Ok(if self.workbook {
            entry.offset
        } else {
            entry.len
        })
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
        Ok(self.raw_text(py, slot, self.row(row)?))
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
                    if slot.verdict(py, row)?.is_ok() {
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
    /// that cast, otherwise HyperCast's ``CastFailure`` code.
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

    /// The cells that did not cast, in row order: ``(row, Fault)`` for each.
    fn faults<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let (batch, slot) = self.parts()?;
        let companions = companions(py)?;
        let list = PyList::empty(py);
        if slot.faults == 0 {
            return Ok(list);
        }
        for row in 0..batch.rows {
            let verdict = slot.verdict(py, row)?;
            if !verdict.is_ok() {
                list.append((row, fault(py, companions, verdict)?))?;
            }
        }
        Ok(list)
    }

    /// The text the cell at ``row`` was cast from — see ``Batch.raw``.
    fn raw<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyBytes>> {
        let (batch, slot) = self.parts()?;
        Ok(batch.raw_text(py, slot, batch.row(row)?))
    }

    fn __getitem__<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
        batch.cell(py, companions(py)?, slot, batch.row(row)?)
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
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

/// What a stream is read from.
enum Reads {
    /// A file this reader opened, read without the interpreter in the way.
    File(File),
    /// The caller's binary file object: anything with ``read(size)``.
    Object(Py<PyAny>),
}

enum Source {
    /// The caller's `bytes`, read in place from `at`, at most `window` bytes a call.
    /// Nothing is copied.
    Bytes {
        bytes: Py<PyBytes>,
        at: usize,
        window: usize,
    },
    /// A stream, read into `buf`; `buf[at..]` is what the core has not finished with.
    /// `limit` is how full the buffer is allowed to get before it has to grow, and
    /// `ceiling` is where it stops growing.
    Stream {
        buf: Vec<u8>,
        at: usize,
        limit: usize,
        ceiling: usize,
        eof: bool,
        reads: Reads,
    },
    Closed,
}

impl Source {
    /// What the core is to read next, and whether nothing follows it.
    fn window<'a>(&'a self, py: Python<'_>) -> (&'a [u8], bool) {
        match self {
            Source::Bytes { bytes, at, window } => {
                let rest = bytes.as_bytes(py).get(*at..).unwrap_or_default();
                match rest.get(..*window) {
                    Some(part) if rest.len() > *window => (part, false),
                    _ => (rest, true),
                }
            }
            Source::Stream { buf, at, eof, .. } => (buf.get(*at..).unwrap_or_default(), *eof),
            Source::Closed => (&[], true),
        }
    }

    fn advance(&mut self, consumed: usize) {
        match self {
            Source::Bytes { at, .. } | Source::Stream { at, .. } => *at += consumed,
            Source::Closed => {}
        }
    }
}

/// Reads until `into` is full or the file ends; returns how much was read, and the
/// failure that stopped it short if one did.
fn read_full(file: &mut File, into: &mut [u8]) -> (usize, Option<std::io::Error>) {
    let mut total = 0;
    while let Some(room) = into.get_mut(total..).filter(|room| !room.is_empty()) {
        match file.read(room) {
            Ok(0) => break,
            Ok(read) => total += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return (total, Some(error)),
        }
    }
    (total, None)
}

/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time
/// into typed columns, every cell a HyperCast verdict.
///
/// ``DelimitedReader(source, dialect, plan, *, batch_rows=4096, buffer_bytes=262144)``
/// reads ``source``: ``bytes``, read in place and never copied, or a binary file object —
/// anything with ``read(size)`` — read forward only. ``DelimitedReader.open(path, …)``
/// opens a file. ``dialect`` is a :class:`Dialect` and ``plan`` an iterable of
/// :class:`Column`: nothing is sniffed and nothing is inferred.
///
/// Iterating yields :class:`Batch` objects of up to ``batch_rows`` rows. A value that does
/// not cast is that cell's verdict, and the read goes on. Input that is not rows of cells
/// at all — a record of the wrong width, a quote never closed — raises
/// :class:`TabularError`, after every intact row before it has been delivered, and again
/// on every later read.
///
/// A reader is read by one thread at a time; the batches it has handed out are immutable
/// and can go anywhere.
#[pyclass(module = "hypertabular")]
struct DelimitedReader {
    dialect: Py<PyAny>,
    plan: Py<PyTuple>,
    doors: Vec<Door>,
    specs: Vec<ColumnSpec>,
    /// Cell-table entries one row takes: the widest ordinal the plan reads, plus two.
    per_row: usize,
    batch_rows: usize,
    state: State,
    source: Source,
    /// Per plan column, `batch_rows` values of its door's type, as bytes.
    values: Vec<Vec<u8>>,
    /// Per plan column, `batch_rows` verdicts, as bytes.
    verdicts: Vec<Vec<u8>>,
    /// The cell table: `batch_rows * per_row` entries.
    cells: Vec<Span>,
    arena: Vec<u8>,
    /// Whether the last batch ended early because the arena filled: the next one starts
    /// with it doubled, so that escaped text costs a few batches, not one per row.
    cramped: bool,
    header: Option<Py<PyTuple>>,
    /// The structural failure that ended the input: final, and raised again on every read.
    failure: Option<PyErr>,
    done: bool,
}

impl DelimitedReader {
    fn build(
        py: Python<'_>,
        source: Source,
        dialect: &Bound<'_, PyAny>,
        plan: &Bound<'_, PyAny>,
        batch_rows: usize,
    ) -> PyResult<Self> {
        let companions = companions(py)?;
        if batch_rows == 0 {
            return Err(PyValueError::new_err("batch_rows must be at least 1"));
        }

        if !dialect.is_instance(companions.dialect.bind(py))? {
            return Err(PyTypeError::new_err(format!(
                "dialect must be a Dialect, not {}",
                dialect.get_type().name()?
            )));
        }
        let separator: String = dialect.getattr("separator")?.extract()?;
        let invalid = || {
            PyValueError::new_err(format!(
                "separator {separator:?} is not tab or a printable ASCII character other than '\"'"
            ))
        };
        let &[byte] = separator.as_bytes() else {
            return Err(invalid());
        };
        let raw = RawDialect {
            separator: byte,
            quoting: u8::from(dialect.getattr("quoting")?.is_truthy()?),
            skip_blank_lines: u8::from(dialect.getattr("skip_blank_lines")?.is_truthy()?),
            engine: 0,
        };
        let state = State::init(raw).ok_or_else(invalid)?;
        let has_header = dialect.getattr("has_header")?.is_truthy()?;

        let mut columns = Vec::new();
        let mut doors = Vec::new();
        let mut specs = Vec::new();
        let mut widest = None;
        for (index, column) in plan.try_iter()?.enumerate() {
            let column = column?;
            let (door, spec) = planned(py, companions, index, &column)?;
            widest = widest.max(Some(spec.ordinal as usize));
            columns.push(column);
            doors.push(door);
            specs.push(spec);
        }

        // Everything the core writes into, allocated once and reused for every batch.
        let per_row = widest.map_or(0, |ordinal| ordinal + 1) + 1;
        let mut values = Vec::with_capacity(doors.len());
        let mut verdicts = Vec::with_capacity(doors.len());
        for door in &doors {
            values.push(zeroed(batch_rows.checked_mul(value_size(*door)))?);
            verdicts.push(zeroed(batch_rows.checked_mul(size_of::<CellVerdict>()))?);
        }
        let entries = batch_rows.checked_mul(per_row);
        let mut cells = Vec::new();
        entries
            .and_then(|entries| cells.try_reserve_exact(entries).ok().map(|()| entries))
            .map(|entries| cells.resize(entries, Span::default()))
            .ok_or_else(|| PyMemoryError::new_err("batch_rows is too large for this plan"))?;

        let mut reader = DelimitedReader {
            dialect: dialect.clone().unbind(),
            plan: PyTuple::new(py, columns)?.unbind(),
            doors,
            specs,
            per_row,
            batch_rows,
            state,
            source,
            values,
            verdicts,
            cells,
            arena: vec![0; ARENA_BYTES],
            cramped: false,
            header: None,
            failure: None,
            done: false,
        };
        if has_header {
            reader.read_header(py)?;
        }
        Ok(reader)
    }

    /// The exception for a structural failure, kept so that every later read raises it.
    fn failed(
        &mut self,
        py: Python<'_>,
        code: u32,
        (record, line, byte): (u64, u32, u64),
        (expected, found): (u32, u32),
    ) -> PyErr {
        let error = tabular_error(py, code, (record, line, byte), (expected, found));
        self.failure = Some(error.clone_ref(py));
        error
    }

    fn structural(&mut self, py: Python<'_>, failure: Failure) -> PyErr {
        self.failed(
            py,
            failure.code,
            (failure.record, failure.line, failure.byte),
            (failure.expected, failure.found),
        )
    }

    /// Makes room behind the unfinished record and reads more into it.
    fn refill(&mut self, py: Python<'_>) -> PyResult<()> {
        let position = (self.state.records, self.state.line, self.state.offset);
        let too_long = match &mut self.source {
            Source::Closed => return Ok(()),
            // Read in place, so there is nothing to make room in: one record is larger
            // than the core takes in a call.
            Source::Bytes { .. } => true,
            Source::Stream {
                buf,
                at,
                limit,
                ceiling,
                eof,
                reads,
            } => {
                buf.drain(..*at);
                *at = 0;
                // One record fills the buffer: it needs a bigger one, while there is one.
                let full = buf.len() >= *limit;
                if full && *limit < *ceiling {
                    *limit = limit.saturating_mul(2).min(*ceiling);
                }
                let room = limit.saturating_sub(buf.len());
                if room > 0 {
                    buf.try_reserve_exact(room)
                        .map_err(|_| PyMemoryError::new_err("no memory for a record this long"))?;
                    match reads {
                        Reads::File(file) => {
                            let filled = buf.len();
                            buf.resize(*limit, 0);
                            let into = buf.get_mut(filled..).unwrap_or_default();
                            // Whatever was read before a failure is kept: the file has
                            // moved past it, and a caller who reads on must not lose it.
                            let (read, failure) = py.detach(|| read_full(file, into));
                            buf.truncate(filled + read);
                            if let Some(failure) = failure {
                                return Err(failure.into());
                            }
                            *eof = read < room;
                        }
                        Reads::Object(object) => {
                            let chunk = object.bind(py).call_method1("read", (room,))?;
                            let chunk = chunk.cast::<PyBytes>().map_err(|_| {
                                PyTypeError::new_err(
                                    "read() must return bytes: open the file in binary mode",
                                )
                            })?;
                            let chunk = chunk.as_bytes();
                            if chunk.len() > room {
                                return Err(PyValueError::new_err(
                                    "read(size) returned more than size bytes",
                                ));
                            }
                            buf.extend_from_slice(chunk);
                            *eof = chunk.is_empty();
                        }
                    }
                }
                room == 0
            }
        };
        if too_long {
            return Err(self.failed(py, 3, position, (0, 0)));
        }
        Ok(())
    }

    fn read_header(&mut self, py: Python<'_>) -> PyResult<()> {
        let mut names = vec![Span::default(); 64];
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window(py);
            let code = fill::header(
                &mut self.state,
                window,
                last,
                &mut names,
                &mut self.arena,
                &mut out,
            );
            let consumed = out.consumed as usize;
            match code {
                OK if out.rows > 0 => {
                    let header = names.iter().take(out.rows as usize).map(|name| {
                        let from: &[u8] = if name.flagged() { &self.arena } else { window };
                        let start = name.offset as usize;
                        text(py, from.get(start..start + name.len()).unwrap_or_default())
                    });
                    self.header = Some(PyTuple::new(py, header)?.unbind());
                    self.source.advance(consumed);
                    return Ok(());
                }
                OK => {
                    let rest = window.len();
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        // An empty input has no header and no rows; the width is unknown.
                        self.header = Some(PyTuple::empty(py).unbind());
                        return Ok(());
                    }
                    if consumed == 0 {
                        self.refill(py)?;
                    }
                }
                ERR_CELLS => names.resize(out.needed as usize, Span::default()),
                ERR_ARENA => self.arena.resize(out.needed as usize, 0),
                ERR_STRUCTURE => return Err(self.structural(py, out.failure)),
                _ => return Err(PyRuntimeError::new_err(CONTRACT)),
            }
        }
    }

    fn next_batch(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone_ref(py));
        }
        if matches!(self.source, Source::Closed) {
            return Err(PyValueError::new_err("read of a closed DelimitedReader"));
        }
        if self.done || matches!(self.source.window(py), (&[], true)) {
            self.done = true;
            return Ok(None);
        }

        // Where the core writes: this reader's own arrays, one pair per plan column.
        let max_rows = self.batch_rows;
        let columns = Columns(
            self.values
                .iter_mut()
                .zip(&mut self.verdicts)
                .map(|(values, verdicts)| ColumnBuffer {
                    values: values.as_mut_ptr().cast(),
                    verdicts: verdicts.as_mut_ptr().cast(),
                })
                .collect(),
        );

        if self.cramped {
            let doubled = self.arena.len().saturating_mul(2);
            self.arena.resize(doubled, 0);
            self.cramped = false;
        }
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window(py);
            let (state, specs) = (&mut self.state, &self.specs);
            let (cells, arena) = (&mut self.cells, &mut self.arena);
            // The one call into the core, with the interpreter released: everything it
            // reads and writes is this reader's, which nothing else can reach meanwhile.
            //
            // SAFETY: each column's two arrays were allocated in `build` with room for
            // `batch_rows` values of its door's type and `batch_rows` verdicts.
            let code = py.detach(|| unsafe {
                fill::fill(
                    state,
                    window,
                    last,
                    specs,
                    columns.as_slice(),
                    max_rows,
                    cells,
                    arena,
                    &mut out,
                )
            });
            let consumed = out.consumed as usize;
            match code {
                OK if out.rows > 0 => {
                    // The batch takes its own copy of what the core wrote — the arrays
                    // are this reader's, and the next read reuses them.
                    let rows = out.rows as usize;
                    self.cramped = rows < max_rows
                        && (out.arena_used as usize).saturating_mul(2) >= self.arena.len();
                    let input = match &self.source {
                        Source::Bytes { bytes, at, .. } => Input::Shared {
                            bytes: bytes.clone_ref(py),
                            start: *at,
                            len: consumed,
                        },
                        _ => Input::Owned(window.get(..consumed).unwrap_or_default().to_vec()),
                    };
                    let arena = self
                        .arena
                        .get(..out.arena_used as usize)
                        .unwrap_or_default()
                        .to_vec();
                    let cells = self
                        .cells
                        .get(..rows * self.per_row)
                        .unwrap_or_default()
                        .to_vec();
                    let mut slots = Vec::with_capacity(self.doors.len());
                    for (((values, verdicts), door), spec) in self
                        .values
                        .iter()
                        .zip(&self.verdicts)
                        .zip(&self.doors)
                        .zip(&self.specs)
                    {
                        let values = values.get(..rows * value_size(*door)).unwrap_or_default();
                        let verdicts = verdicts
                            .get(..rows * size_of::<CellVerdict>())
                            .unwrap_or_default();
                        let faults = (0..rows)
                            .filter(|row| verdict_at(verdicts, *row).is_some_and(|v| !v.is_ok()))
                            .count();
                        slots.push(Slot {
                            door: *door,
                            ordinal: spec.ordinal as usize,
                            values: PyBytes::new(py, values).unbind(),
                            verdicts: PyBytes::new(py, verdicts).unbind(),
                            faults,
                            values_view: PyOnceLock::new(),
                            verdicts_view: PyOnceLock::new(),
                        });
                    }
                    self.source.advance(consumed);
                    return Ok(Some(Batch {
                        rows,
                        per_row: self.per_row,
                        plan: self.plan.clone_ref(py),
                        slots,
                        cells,
                        input,
                        arena,
                        workbook: false,
                    }));
                }
                OK => {
                    let rest = window.len();
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        self.done = true;
                        return Ok(None);
                    }
                    if consumed == 0 {
                        self.refill(py)?;
                    }
                }
                // The table is sized for the plan in `build`, so this is not expected;
                // the core says what one row takes, and the table is made to hold it.
                ERR_CELLS => {
                    self.per_row = self.per_row.max(out.needed as usize);
                    let needed = self.per_row.saturating_mul(max_rows);
                    self.cells
                        .try_reserve_exact(needed.saturating_sub(self.cells.len()))
                        .map_err(|_| {
                            PyMemoryError::new_err("batch_rows is too large for this plan")
                        })?;
                    self.cells.resize(needed, Span::default());
                }
                ERR_ARENA => {
                    let needed = (out.needed as usize).max(self.arena.len() * 2);
                    self.arena.resize(needed, 0);
                }
                ERR_STRUCTURE => return Err(self.structural(py, out.failure)),
                _ => return Err(PyRuntimeError::new_err(CONTRACT)),
            }
        }
    }
}

/// One plan column as the core takes it, checked: the door it names and its notation.
fn planned(
    py: Python<'_>,
    companions: &Companions,
    index: usize,
    column: &Bound<'_, PyAny>,
) -> PyResult<(Door, ColumnSpec)> {
    if !column.is_instance(companions.column.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "plan[{index}] must be a Column, not {}",
            column.get_type().name()?
        )));
    }
    let declared = column.getattr("declared")?;
    let format = column.getattr("format")?;
    if !format.is_instance(companions.num_format.bind(py))? {
        return Err(PyTypeError::new_err(format!(
            "plan[{index}].format must be a hypercast.NumFormat, not {}",
            format.get_type().name()?
        )));
    }
    // HyperCast's NumFormat validated itself when it was built; its packed bytes are the
    // core's layout, which `resolve` below reads back.
    let packed: [u8; 32] = format.getattr("packed")?.extract()?;
    let spec = ColumnSpec {
        ordinal: column.getattr("ordinal")?.extract()?,
        door: column.getattr("door")?.extract()?,
        param: if declared.is_none() {
            0
        } else {
            declared.extract()?
        },
        format: RawNumFormat::from_le_bytes(packed),
    };
    let door = Door::from_code(spec.door, spec.param).ok_or_else(|| {
        PyValueError::new_err(format!(
            "plan[{index}] names no door; build it with Column's factories"
        ))
    })?;
    if spec.num_format().is_none() {
        return Err(PyValueError::new_err(format!(
            "plan[{index}].format cannot be honoured"
        )));
    }
    Ok((door, spec))
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
    const MAX_ROW_BYTES: usize = MAX_ROW_BYTES;

    #[new]
    #[pyo3(signature = (source, dialect, plan, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES))]
    fn new(
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        dialect: &Bound<'_, PyAny>,
        plan: &Bound<'_, PyAny>,
        batch_rows: usize,
        buffer_bytes: usize,
    ) -> PyResult<Self> {
        let source = if let Ok(bytes) = source.cast::<PyBytes>() {
            Source::Bytes {
                bytes: bytes.clone().unbind(),
                at: 0,
                window: WINDOW,
            }
        } else if source.is_instance_of::<PyString>() || source.hasattr("__fspath__")? {
            return Err(PyTypeError::new_err(
                "source must be bytes or a binary file object; DelimitedReader.open(path, ...) opens a file",
            ));
        } else if source.hasattr("read")? {
            stream(Reads::Object(source.clone().unbind()), buffer_bytes)?
        } else {
            return Err(PyTypeError::new_err(format!(
                "source must be bytes or a binary file object, not {}",
                source.get_type().name()?
            )));
        };
        DelimitedReader::build(py, source, dialect, plan, batch_rows)
    }

    /// Opens the file at ``path`` — a ``str`` or ``os.PathLike`` — and reads it. The
    /// reader owns the file: ``close()``, or leaving a ``with`` block, closes it.
    #[staticmethod]
    #[pyo3(signature = (path, dialect, plan, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES))]
    fn open(
        py: Python<'_>,
        path: PathBuf,
        dialect: &Bound<'_, PyAny>,
        plan: &Bound<'_, PyAny>,
        batch_rows: usize,
        buffer_bytes: usize,
    ) -> PyResult<Self> {
        let source = stream(Reads::File(File::open(path)?), buffer_bytes)?;
        DelimitedReader::build(py, source, dialect, plan, batch_rows)
    }

    /// The header's names, when the dialect declares a header: a tuple of ``str``, empty
    /// for an input with no record at all. ``None`` when the dialect declares none.
    #[getter]
    fn header<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyTuple>> {
        self.header.as_ref().map(|header| header.bind(py).clone())
    }

    /// The dialect in force.
    #[getter]
    fn dialect<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.dialect.bind(py).clone()
    }

    /// The plan every batch is filled through: a tuple of :class:`Column`.
    #[getter]
    fn plan<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
        self.plan.bind(py).clone()
    }

    /// Rows per batch, at most.
    #[getter]
    fn batch_rows(&self) -> usize {
        self.batch_rows
    }

    /// Records finished so far — the header and skipped blank lines included.
    #[getter]
    fn records(&self) -> u64 {
        self.state.records
    }

    /// Reads the next batch, or returns ``None`` once the input is exhausted.
    ///
    /// Raises :class:`TabularError` for input that is structurally broken — after every
    /// intact row before the break has been delivered, and again on every later call.
    fn read(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    /// Lets go of the source: closes the file this reader opened, or drops its reference
    /// to the caller's ``bytes`` or file object (which stays open — it is the caller's).
    /// Batches already read stay valid.
    fn close(&mut self) {
        self.source = Source::Closed;
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(&mut self, _exc: &Bound<'_, PyTuple>) {
        self.close();
    }
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

/// A workbook read's error as Python raises it.
fn workbook_error(py: Python<'_>, error: crate::Error) -> PyErr {
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
        other => PyValueError::new_err(other.to_string()),
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

/// An XLSX or ODS workbook held in memory, its sheets listed and its shared strings and
/// styles loaded: what a :class:`Sheet` reads from.
///
/// ``Workbook(data)`` opens ``bytes``; ``Workbook.open(path)`` reads a file. A workbook
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
            .map_err(|error| workbook_error(py, error))?;
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

#[pymethods]
impl Workbook {
    #[new]
    fn new(py: Python<'_>, data: &Bound<'_, PyBytes>) -> PyResult<Self> {
        Workbook::build(py, data.as_bytes().to_vec())
    }

    /// Reads the file at ``path`` — a ``str`` or ``os.PathLike`` — and opens it.
    #[staticmethod]
    fn open(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        let bytes = py.detach(|| std::fs::read(path))?;
        Workbook::build(py, bytes)
    }

    /// Which kind of workbook this is: a :class:`WorkbookFormat`.
    #[getter]
    fn format<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let code = match self.book.format() {
            crate::Format::Xlsx => 1,
            crate::Format::Ods => 2,
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
    /// declare a header it is read here. A sheet the workbook does not have raises
    /// ``IndexError`` for an index and ``KeyError`` for a name.
    fn sheet(
        &self,
        py: Python<'_>,
        which: &Bound<'_, PyAny>,
        options: &Bound<'_, PyAny>,
        plan: &Bound<'_, PyAny>,
    ) -> PyResult<Sheet> {
        let companions = companions(py)?;
        let batch_rows: usize = options.getattr("batch_rows")?.extract()?;
        if batch_rows == 0 {
            return Err(PyValueError::new_err("batch_rows must be at least 1"));
        }
        let settings = crate::SheetOptions {
            has_header: options.getattr("has_header")?.is_truthy()?,
            skip_empty_rows: options.getattr("skip_empty_rows")?.is_truthy()?,
            batch_rows,
        };
        let mut columns = Vec::new();
        let mut planned_columns = Vec::new();
        let mut doors = Vec::new();
        for (index, column) in plan.try_iter()?.enumerate() {
            let column = column?;
            let (door, spec) = planned(py, companions, index, &column)?;
            let format = spec.num_format().ok_or_else(|| {
                PyValueError::new_err(format!("plan[{index}].format cannot be honoured"))
            })?;
            planned_columns.push(crate::Column::new(spec.ordinal as usize, door).format(format));
            columns.push(column);
            doors.push(door);
        }

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
                crate::Workbook::shared_sheet(shared, index.into(), settings, &planned_columns)
            })
        } else if let Ok(name) = which.extract::<String>() {
            py.detach(|| {
                crate::Workbook::shared_sheet(
                    shared,
                    name.as_str().into(),
                    settings,
                    &planned_columns,
                )
            })
        } else {
            return Err(PyTypeError::new_err(format!(
                "which must be an int or a str, not {}",
                which.get_type().name()?
            )));
        };
        let sheet = opened.map_err(|error| workbook_error(py, error))?;
        let header = sheet.header().map(|header| {
            let names: Vec<_> = header.names().map(|name| text(py, name)).collect();
            PyTuple::new(py, names).map(Bound::unbind)
        });
        Ok(Sheet {
            sheet,
            book: Arc::clone(&self.book),
            strings: self.strings.clone_ref(py),
            options: options.clone().unbind(),
            plan: PyTuple::new(py, columns)?.unbind(),
            doors,
            header: header.transpose()?,
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
    plan: Py<PyTuple>,
    doors: Vec<Door>,
    header: Option<Py<PyTuple>>,
    /// The structural failure that ended the sheet: final, and the same exception raised
    /// again on every read.
    failure: Option<PyErr>,
}

impl Sheet {
    fn next_batch(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone_ref(py));
        }
        let batch = match self.sheet.read() {
            Ok(Some(batch)) => batch,
            Ok(None) => return Ok(None),
            Err(error) => {
                let error = workbook_error(py, error);
                self.failure = Some(error.clone_ref(py));
                return Err(error);
            }
        };
        let (cells, per_row, _, arena) = batch.tables();
        // The arena as far as any span of this batch reaches into it: the text typed cells
        // were said as, and the raw text of those that failed.
        let mut reach = 0;
        let mut slots = Vec::with_capacity(self.doors.len());
        for (index, door) in self.doors.iter().enumerate() {
            let (values, verdicts) = batch.column_bytes(index);
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
            slots.push(Slot {
                door: *door,
                ordinal: index,
                values: PyBytes::new(py, values).unbind(),
                verdicts: PyBytes::new(py, verdict_bytes).unbind(),
                faults: verdicts.iter().filter(|verdict| !verdict.is_ok()).count(),
                values_view: PyOnceLock::new(),
                verdicts_view: PyOnceLock::new(),
            });
        }
        for cell in cells.iter().filter(|cell| cell.flagged()) {
            reach = reach.max(cell.offset as usize + cell.len());
        }
        Ok(Some(Batch {
            rows: batch.rows(),
            per_row,
            plan: self.plan.clone_ref(py),
            slots,
            cells: cells.to_vec(),
            input: Input::Strings(self.strings.clone_ref(py)),
            arena: arena.get(..reach).unwrap_or(arena).to_vec(),
            workbook: true,
        }))
    }
}

#[pymethods]
impl Sheet {
    /// The options the sheet is read with.
    #[getter]
    fn options<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.options.bind(py).clone()
    }

    /// The plan every batch is filled through: a tuple of :class:`Column`.
    #[getter]
    fn plan<'py>(&self, py: Python<'py>) -> Bound<'py, PyTuple> {
        self.plan.bind(py).clone()
    }

    /// The header row's names — a typed cell said the way the text door says it — as a
    /// tuple of ``str``, or ``None`` when the options declare no header.
    #[getter]
    fn header<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyTuple>> {
        self.header.as_ref().map(|header| header.bind(py).clone())
    }

    /// Reads the next batch, or returns ``None`` once the sheet has no more rows.
    fn read(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "<hypertabular.Sheet of a {:?} workbook plan={}>",
            self.book.format(),
            self.plan.bind(py).len()
        ))
    }
}

fn stream(reads: Reads, buffer_bytes: usize) -> PyResult<Source> {
    if buffer_bytes == 0 {
        return Err(PyValueError::new_err("buffer_bytes must be at least 1"));
    }
    Ok(Source::Stream {
        buf: Vec::new(),
        at: 0,
        limit: buffer_bytes.min(MAX_ROW_BYTES),
        ceiling: MAX_ROW_BYTES,
        eof: false,
        reads,
    })
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
/// structural failure and its kinds, the workbook format — and has it import HyperCast's.
#[pyfunction]
fn _bind(
    py: Python<'_>,
    column: Bound<'_, PyAny>,
    dialect: Bound<'_, PyAny>,
    error: Bound<'_, PyAny>,
    failure: Bound<'_, PyAny>,
    workbook_format: Bound<'_, PyAny>,
) -> PyResult<()> {
    let hypercast = py.import("hypercast")?;
    let reason = hypercast.getattr("CastFailure")?;
    let companions = Companions {
        success: hypercast.getattr("Success")?.unbind(),
        fault: hypercast.getattr("Fault")?.unbind(),
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
    match &mut reader.source {
        Source::Bytes { window, .. } => *window = window_bytes.clamp(1, WINDOW),
        Source::Stream { ceiling, .. } => *ceiling = max_row_bytes.clamp(1, MAX_ROW_BYTES),
        Source::Closed => {}
    }
}

// The name is the last segment of pyproject.toml's module-name ("hypertabular._native"):
// PyO3 makes the `PyInit__native` symbol the import machinery looks for from it.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DelimitedReader>()?;
    m.add_class::<Batch>()?;
    m.add_class::<ColumnData>()?;
    m.add_class::<Workbook>()?;
    m.add_class::<SheetInfo>()?;
    m.add_class::<Sheet>()?;
    m.add_function(wrap_pyfunction!(native_version, m)?)?;
    m.add_function(wrap_pyfunction!(_bind, m)?)?;
    m.add_function(wrap_pyfunction!(_limits, m)?)?;
    Ok(())
}
