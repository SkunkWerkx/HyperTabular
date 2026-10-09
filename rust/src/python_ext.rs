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
//! A reader over an asynchronous stream (`asyncio.StreamReader`, or anything with an `async
//! read(size)`) never calls it: the core never waits, and neither does this module. A step
//! of such a read ends by saying how much input it has room for; the coroutines in
//! `hypertabular/_aio.py` await that much from the stream, feed it in and take the next
//! step, so all the core's work happens between awaits, the interpreter released as ever.
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
    /// `Header`, called with the names as `str`s and as the bytes they were read from.
    header: Py<PyAny>,
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
        let slot = self.slot(column)?;
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
        let row = self.row_index(row)?;
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
        Ok(self.raw_text(py, slot, self.row_index(row)?))
    }

    /// The cell at (``column``, ``row``) as HyperCast's verdict — ``Success(value)`` or
    /// ``Fault(reason, offset, length)`` — as ``batch.column(column)[row]`` gives it.
    fn get<'py>(&self, py: Python<'py>, column: usize, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let slot = self.slot(column)?;
        self.cell(py, companions(py)?, slot, self.row_index(row)?)
    }

    /// The text cell at (``column``, ``row``) as a ``str``, or ``None`` for one that did
    /// not cast (a text cell fails only by being empty) — the same ``str`` ``values``
    /// holds. A column read through any other door raises ``TypeError``.
    fn text<'py>(
        &self,
        py: Python<'py>,
        column: usize,
        row: isize,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.text_cell(py, column, self.row_index(row)?)
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
        Ok(batch.raw_text(py, slot, batch.row_index(row)?))
    }

    fn __getitem__<'py>(&self, py: Python<'py>, row: isize) -> PyResult<Bound<'py, PyAny>> {
        let (batch, slot) = self.parts()?;
        batch.cell(py, companions(py)?, slot, batch.row_index(row)?)
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
        batch.cell(py, companions(py)?, batch.slot(column)?, self.index)
    }

    /// The text cell in column ``column`` as a ``str``, or ``None``: ``Batch.text``.
    fn text<'py>(&self, py: Python<'py>, column: usize) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.batch.get().text_cell(py, column, self.index)
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
                    reader.bind(py).try_borrow_mut()?.next_batch(py)?
                }
                Some(Feeds::Sheet(sheet)) => sheet.bind(py).try_borrow_mut()?.next_batch(py)?,
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

/// What a stream is read from.
enum Reads {
    /// A file this reader opened, read without the interpreter in the way.
    File(File),
    /// The caller's binary file object: anything with ``read(size)``.
    Object(Py<PyAny>),
    /// The caller's asynchronous stream — anything with ``async read(size)``, an
    /// ``asyncio.StreamReader`` — which this reader never calls: a refill says how much it
    /// wants instead, and ``read_async`` awaits that much from the stream and feeds it in.
    Fed(Py<PyAny>),
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
    /// `ceiling` is where it stops growing. `wanted` is what a fed stream was last asked
    /// for and has not yet been given.
    Stream {
        buf: Vec<u8>,
        at: usize,
        limit: usize,
        ceiling: usize,
        eof: bool,
        wanted: usize,
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
/// ``DelimitedReader(source, dialect, plan=None, *, batch_rows=4096, buffer_bytes=262144)``
/// reads ``source``: ``bytes``, read in place and never copied, or a binary file object —
/// anything with ``read(size)`` — read forward only. ``DelimitedReader.open(path, …)``
/// opens a file, and ``await DelimitedReader.open_async(stream, …)`` reads an asynchronous
/// stream. ``dialect`` is a :class:`Dialect` and ``plan`` an iterable of :class:`Column`:
/// nothing is sniffed and nothing is inferred.
///
/// The header, when the dialect declares one, is read as the reader is made. A reader made
/// without a plan reads it all the same and waits for one: ``bind(plan)``, once, with
/// ordinals the :class:`Header` has looked up by name.
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
    /// The plan, as the caller's own `Column`s: empty until one is bound.
    plan: Py<PyTuple>,
    bound: bool,
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
    /// The package's `Header`, once read.
    header: Option<Py<PyAny>>,
    /// Whether the dialect declares a header that has not been read yet: only ever true of
    /// a reader over an asynchronous stream, whose header `open_async` reads as it is fed.
    header_pending: bool,
    /// The structural failure that ended the input: final, and raised again on every read.
    failure: Option<PyErr>,
    done: bool,
}

/// What one step of a read came to.
enum Step {
    Batch(Batch),
    /// The input is exhausted.
    End,
    /// A fed stream has to be given up to this many bytes before the read can go on.
    Wants(usize),
}

impl DelimitedReader {
    /// A reader over `source`, bound to `plan` when there is one — before the header is
    /// read, so that a plan that cannot be honoured is refused first. The header is read
    /// here unless `defer_header` (a fed stream, which has nothing to read it from yet).
    fn build(
        py: Python<'_>,
        source: Source,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        defer_header: bool,
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

        let mut reader = DelimitedReader {
            dialect: dialect.clone().unbind(),
            plan: PyTuple::empty(py).unbind(),
            bound: false,
            doors: Vec::new(),
            specs: Vec::new(),
            per_row: 0,
            batch_rows,
            state,
            source,
            values: Vec::new(),
            verdicts: Vec::new(),
            cells: Vec::new(),
            arena: vec![0; ARENA_BYTES],
            cramped: false,
            header: None,
            header_pending: has_header,
            failure: None,
            done: false,
        };
        if let Some(plan) = plan {
            reader.bind_plan(py, plan)?;
        }
        if !defer_header && reader.settle_header(py)?.is_some() {
            // Only a fed stream asks for input, and its header is deferred.
            return Err(PyRuntimeError::new_err(CONTRACT));
        }
        Ok(reader)
    }

    /// Checks `plan` and allocates everything the core writes into for it, once, to be
    /// reused for every batch. A plan refused leaves the reader as it was: unbound.
    fn bind_plan(&mut self, py: Python<'_>, plan: &Bound<'_, PyAny>) -> PyResult<()> {
        let companions = companions(py)?;
        let batch_rows = self.batch_rows;
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

        self.plan = PyTuple::new(py, columns)?.unbind();
        self.doors = doors;
        self.specs = specs;
        self.per_row = per_row;
        self.values = values;
        self.verdicts = verdicts;
        self.cells = cells;
        self.bound = true;
        Ok(())
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

    /// Makes room behind the unfinished record and reads more into it — or, for a fed
    /// stream, says how much it has room for, to be fed with `_feed` before the next step.
    fn refill(&mut self, py: Python<'_>) -> PyResult<Option<usize>> {
        let position = (self.state.records, self.state.line, self.state.offset);
        let too_long = match &mut self.source {
            Source::Closed => return Ok(None),
            // Read in place, so there is nothing to make room in: one record is larger
            // than the core takes in a call.
            Source::Bytes { .. } => true,
            Source::Stream {
                buf,
                at,
                limit,
                ceiling,
                eof,
                wanted,
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
                        // Nothing is read here: the step ends, asking for this much. Room
                        // made and a buffer grown are kept, so a step taken again — after a
                        // cancelled await, say — asks for the same.
                        Reads::Fed(_) => {
                            *wanted = room;
                            return Ok(Some(room));
                        }
                    }
                }
                room == 0
            }
        };
        if too_long {
            return Err(self.failed(py, 3, position, (0, 0)));
        }
        Ok(None)
    }

    /// Reads the header, if the dialect declares one and it has not been read yet. `Some`
    /// is how much a fed stream wants before the header can be finished.
    fn settle_header(&mut self, py: Python<'_>) -> PyResult<Option<usize>> {
        if self.header_pending {
            if let Some(wanted) = self.read_header(py)? {
                return Ok(Some(wanted));
            }
            self.header_pending = false;
        }
        Ok(None)
    }

    /// Reads the header record. `Some` is how much a fed stream wants before it can go on.
    fn read_header(&mut self, py: Python<'_>) -> PyResult<Option<usize>> {
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
                    let header = header_of(
                        py,
                        names.iter().take(out.rows as usize).map(|name| {
                            let from: &[u8] = if name.flagged() { &self.arena } else { window };
                            let start = name.offset as usize;
                            from.get(start..start + name.len()).unwrap_or_default()
                        }),
                    )?;
                    self.header = Some(header);
                    self.source.advance(consumed);
                    return Ok(None);
                }
                OK => {
                    let rest = window.len();
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        // An empty input has no header and no rows; the width is unknown.
                        self.header = Some(header_of(py, [])?);
                        return Ok(None);
                    }
                    if consumed == 0
                        && let Some(wanted) = self.refill(py)?
                    {
                        return Ok(Some(wanted));
                    }
                }
                ERR_CELLS => names.resize(out.needed as usize, Span::default()),
                ERR_ARENA => self.arena.resize(out.needed as usize, 0),
                ERR_STRUCTURE => return Err(self.structural(py, out.failure)),
                _ => return Err(PyRuntimeError::new_err(CONTRACT)),
            }
        }
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
        if matches!(self.source, Source::Closed) {
            return Err(PyValueError::new_err("read of a closed DelimitedReader"));
        }
        if let Some(wanted) = self.settle_header(py)? {
            return Ok(Step::Wants(wanted));
        }
        if self.done || matches!(self.source.window(py), (&[], true)) {
            self.done = true;
            return Ok(Step::End);
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
                    return Ok(Step::Batch(Batch {
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
                        return Ok(Step::End);
                    }
                    if consumed == 0
                        && let Some(wanted) = self.refill(py)?
                    {
                        return Ok(Step::Wants(wanted));
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
    #[pyo3(signature = (source, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES))]
    fn new(
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
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
        DelimitedReader::build(py, source, dialect, plan, batch_rows, false)
    }

    /// Opens the file at ``path`` — a ``str`` or ``os.PathLike`` — and reads it. The
    /// reader owns the file: ``close()``, or leaving a ``with`` block, closes it.
    #[staticmethod]
    #[pyo3(signature = (path, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES))]
    fn open(
        py: Python<'_>,
        path: PathBuf,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
    ) -> PyResult<Self> {
        let source = stream(Reads::File(File::open(path)?), buffer_bytes)?;
        DelimitedReader::build(py, source, dialect, plan, batch_rows, false)
    }

    /// A reader over an asynchronous stream — anything with ``async read(size)``, such as
    /// an ``asyncio.StreamReader`` — made by a coroutine that reads the header (when the
    /// dialect declares one) before it returns: ``await DelimitedReader.open_async(stream,
    /// dialect)``. Read it with ``await read_async()`` or ``async for``. ``bytes`` are taken
    /// too, and read as the constructor reads them. The stream is the caller's to close.
    #[staticmethod]
    #[pyo3(signature = (stream, dialect, plan = None, *, batch_rows = DEFAULT_BATCH_ROWS, buffer_bytes = DEFAULT_BUFFER_BYTES))]
    fn open_async<'py>(
        py: Python<'py>,
        stream: &Bound<'py, PyAny>,
        dialect: &Bound<'py, PyAny>,
        plan: Option<&Bound<'py, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        asynchronous(py, "open_reader")?.call1((stream, dialect, plan, batch_rows, buffer_bytes))
    }

    /// For ``open_async``: the reader over ``stream``, its header still to be read.
    #[staticmethod]
    #[pyo3(signature = (stream, dialect, plan, batch_rows, buffer_bytes))]
    fn _over_async(
        py: Python<'_>,
        stream: &Bound<'_, PyAny>,
        dialect: &Bound<'_, PyAny>,
        plan: Option<&Bound<'_, PyAny>>,
        batch_rows: usize,
        buffer_bytes: usize,
    ) -> PyResult<Self> {
        let source = if let Ok(bytes) = stream.cast::<PyBytes>() {
            Source::Bytes {
                bytes: bytes.clone().unbind(),
                at: 0,
                window: WINDOW,
            }
        } else if stream.hasattr("read")? {
            self::stream(Reads::Fed(stream.clone().unbind()), buffer_bytes)?
        } else {
            return Err(PyTypeError::new_err(format!(
                "stream must be bytes or have an async read(size), not {}",
                stream.get_type().name()?
            )));
        };
        DelimitedReader::build(py, source, dialect, plan, batch_rows, true)
    }

    /// One step of an asynchronous read: a :class:`Batch`, ``None`` at the end, or how
    /// many bytes the stream is to be read for and fed in (``_feed``) before the next step.
    fn _advance(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        Ok(match self.step(py)? {
            Step::Batch(batch) => Py::new(py, batch)?.into_any(),
            Step::End => py.None(),
            Step::Wants(wanted) => wanted.into_pyobject(py)?.into_any().unbind(),
        })
    }

    /// Reads the header of a reader ``_over_async`` made: ``None`` once it is read, or how
    /// many bytes to feed in first.
    fn _header(&mut self, py: Python<'_>) -> PyResult<Option<usize>> {
        self.settle_header(py)
    }

    /// Feeds a fed stream's reader what the stream's ``read(size)`` returned for the size
    /// it last asked for; ``b""`` is the end of the stream.
    fn _feed(&mut self, chunk: &Bound<'_, PyAny>) -> PyResult<()> {
        let Source::Stream {
            buf,
            eof,
            wanted,
            reads: Reads::Fed(_),
            ..
        } = &mut self.source
        else {
            return Err(PyRuntimeError::new_err(
                "only a reader over an asynchronous stream is fed",
            ));
        };
        if *wanted == 0 {
            return Err(PyRuntimeError::new_err(
                "the reader asked for nothing to be fed",
            ));
        }
        let chunk = chunk.cast::<PyBytes>().map_err(|_| {
            PyTypeError::new_err("read() must return bytes: read the stream in binary mode")
        })?;
        let chunk = chunk.as_bytes();
        if chunk.len() > *wanted {
            return Err(PyValueError::new_err(
                "read(size) returned more than size bytes",
            ));
        }
        buf.extend_from_slice(chunk);
        *eof = chunk.is_empty();
        *wanted = 0;
        Ok(())
    }

    /// Binds the plan a reader made without one reads through — once, before the first
    /// read: ``plan`` an iterable of :class:`Column`, typically with ordinals the header
    /// looked up (``header.ordinal("id")``). A second call raises ``RuntimeError``; a plan
    /// that cannot be honoured raises as the constructor would, and leaves the reader
    /// unbound.
    fn bind(&mut self, py: Python<'_>, plan: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.bound {
            return Err(out_of_order(ALREADY_BOUND));
        }
        self.bind_plan(py, plan)
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
        (self.state.expected != 0).then_some(self.state.expected as usize)
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

    /// Records finished so far — the header and skipped blank lines included.
    #[getter]
    fn records(&self) -> u64 {
        self.state.records
    }

    /// Reads the next batch, or returns ``None`` once the input is exhausted.
    ///
    /// Raises :class:`TabularError` for input that is structurally broken — after every
    /// intact row before the break has been delivered, and again on every later call — and
    /// ``RuntimeError`` before a plan is bound (which a later ``bind`` puts right).
    fn read(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    /// ``read()``, as a coroutine: ``await reader.read_async()``. A reader over an
    /// asynchronous stream awaits the stream's ``read(size)`` whenever it needs more input,
    /// with the core's own work done between awaits, the interpreter released; any other
    /// reader completes without awaiting anything.
    ///
    /// Cancellation is asyncio's: a read cancelled while it awaits the stream has fed the
    /// reader nothing, and the next read picks up where it was — the reader is not spoiled.
    /// One read at a time: a reader is not read by two coroutines at once.
    fn read_async<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let stream = match &slf.borrow().source {
            Source::Stream {
                reads: Reads::Fed(stream),
                ..
            } => stream.clone_ref(py),
            _ => py.None(),
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

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
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
        crate::Error::Unbound => out_of_order(UNBOUND),
        crate::Error::AlreadyBound => out_of_order(ALREADY_BOUND),
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
        let batch_rows: usize = options.getattr("batch_rows")?.extract()?;
        if batch_rows == 0 {
            return Err(PyValueError::new_err("batch_rows must be at least 1"));
        }
        let settings = crate::SheetOptions {
            has_header: options.getattr("has_header")?.is_truthy()?,
            skip_empty_rows: options.getattr("skip_empty_rows")?.is_truthy()?,
            batch_rows,
        };
        let sheet_plan = plan.map(|plan| SheetPlan::of(py, plan)).transpose()?;
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
        let sheet = opened.map_err(|error| workbook_error(py, error))?;
        let header = sheet
            .header()
            .map(|header| header_of(py, header.names()))
            .transpose()?;
        let (plan, doors) = match sheet_plan {
            Some(plan) => (plan.columns.unbind(), plan.doors),
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

/// A plan for a sheet, checked: the caller's columns, the crate's, and their doors.
struct SheetPlan<'py> {
    columns: Bound<'py, PyTuple>,
    planned: Vec<crate::Column>,
    doors: Vec<Door>,
}

impl<'py> SheetPlan<'py> {
    fn of(py: Python<'py>, plan: &Bound<'py, PyAny>) -> PyResult<Self> {
        let companions = companions(py)?;
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
        Ok(SheetPlan {
            columns: PyTuple::new(py, columns)?,
            planned: planned_columns,
            doors,
        })
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
    fn bind(&mut self, py: Python<'_>, plan: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.sheet.is_bound() {
            return Err(out_of_order(ALREADY_BOUND));
        }
        let plan = SheetPlan::of(py, plan)?;
        self.sheet
            .bind(&plan.planned)
            .map_err(|error| workbook_error(py, error))?;
        self.plan = plan.columns.unbind();
        self.doors = plan.doors;
        Ok(())
    }

    /// Whether a plan has been bound: always, for a sheet opened with one.
    #[getter]
    fn is_bound(&self) -> bool {
        self.sheet.is_bound()
    }

    /// Reads the next batch, or returns ``None`` once the sheet has no more rows. Raises
    /// ``RuntimeError`` before a plan is bound.
    fn read(&mut self, py: Python<'_>) -> PyResult<Option<Batch>> {
        self.next_batch(py)
    }

    /// Every row left, batch by batch, as :class:`Row` views: :meth:`DelimitedReader.rows`,
    /// for a sheet.
    fn rows(slf: &Bound<'_, Self>) -> Rows {
        Rows::reading(Feeds::Sheet(slf.clone().unbind()))
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

/// One of the coroutine functions in `hypertabular._aio`, which await a stream on a
/// reader's behalf: the core never waits, so the waiting is done in Python.
fn asynchronous<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    py.import("hypertabular._aio")?.getattr(name)
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
        wanted: 0,
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
/// structural failure and its kinds, the workbook format, the header — and has it import
/// HyperCast's.
#[pyfunction]
fn _bind(
    py: Python<'_>,
    column: Bound<'_, PyAny>,
    dialect: Bound<'_, PyAny>,
    error: Bound<'_, PyAny>,
    failure: Bound<'_, PyAny>,
    workbook_format: Bound<'_, PyAny>,
    header: Bound<'_, PyAny>,
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
        header: header.unbind(),
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
    m.add_class::<Row>()?;
    m.add_class::<Rows>()?;
    m.add_class::<Workbook>()?;
    m.add_class::<SheetInfo>()?;
    m.add_class::<Sheet>()?;
    m.add_function(wrap_pyfunction!(native_version, m)?)?;
    m.add_function(wrap_pyfunction!(_bind, m)?)?;
    m.add_function(wrap_pyfunction!(_limits, m)?)?;
    Ok(())
}
