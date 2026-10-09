//! A batch of rows, column-major: for each column of the plan, the values its door made
//! and a verdict beside each. The reader owns the memory and lends it — a batch is a view,
//! valid until the reader is asked for the next one — so nothing in a batch was copied to
//! make it: a column of integers is the array the core wrote, and a text cell is a slice
//! of the input (or of the workbook's shared strings) wherever the bytes could stay where
//! they were.
//!
//! One type serves delimited text and workbooks alike; the two differ only in where a
//! cell's text lies, which the batch knows and its accessors hide.

use crate::Column;
use crate::kernel::abi::{CellVerdict, ColumnBuffer, ColumnSpec, Span};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use hypercast::{CivilDateTime, Date, Decimal, Duration, Fault, Timestamp};
use std::borrow::Cow;

/// One column's room: `batch_rows` values of its door's type, and as many verdicts.
struct Store {
    /// The values, as bytes the core writes — held as `u64`s so that they are aligned for
    /// every type a door produces.
    values: Vec<u64>,
    verdicts: Vec<CellVerdict>,
}

/// The pointers handed to the core for one call.
#[derive(Default)]
struct Pointers(Vec<ColumnBuffer>);

// SAFETY: the pointers are into the stores beside them, written through only inside a
// call that holds the columns exclusively, and never read back out of this list.
unsafe impl Send for Pointers {}
// SAFETY: as above; a shared reference to the list reaches nothing.
unsafe impl Sync for Pointers {}

/// The columns a reader owns: the plan, the plan as the core takes it, and a store for
/// each column.
pub(crate) struct Columns {
    plan: Vec<Column>,
    specs: Vec<ColumnSpec>,
    stores: Vec<Store>,
    pointers: Pointers,
    batch_rows: usize,
}

/// The bytes one value of a door takes.
fn size_of_value(door: Door) -> usize {
    match door {
        Door::Bool | Door::I8 | Door::U8 => 1,
        Door::I16 | Door::U16 => 2,
        Door::I32 | Door::U32 | Door::F32 => 4,
        Door::I64 | Door::U64 | Door::F64 | Door::Time => 8,
        Door::Uuid => 16,
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => size_of::<Timestamp>(),
        Door::Date | Door::DateOrdered(_) => size_of::<Date>(),
        Door::DateTime(_) => size_of::<CivilDateTime>(),
        Door::Duration => size_of::<Duration>(),
        Door::Decimal => size_of::<Decimal>(),
        Door::Text => size_of::<Span>(),
    }
}

impl Columns {
    /// Room for `batch_rows` rows of `plan`.
    pub(crate) fn new(plan: &[Column], specs: Vec<ColumnSpec>, batch_rows: usize) -> Columns {
        let stores = plan
            .iter()
            .map(|column| Store {
                values: vec![0; (size_of_value(column.door) * batch_rows).div_ceil(8)],
                verdicts: vec![CellVerdict::OK; batch_rows],
            })
            .collect();
        Columns {
            plan: plan.to_vec(),
            specs,
            stores,
            pointers: Pointers::default(),
            batch_rows,
        }
    }

    pub(crate) fn plan(&self) -> &[Column] {
        &self.plan
    }

    pub(crate) fn batch_rows(&self) -> usize {
        self.batch_rows
    }

    /// The plan and the column buffers as the core takes them. Each buffer has room for
    /// `batch_rows` values of its column's door and as many verdicts.
    pub(crate) fn for_core(&mut self) -> (&[ColumnSpec], &[ColumnBuffer]) {
        self.pointers.0.clear();
        for store in &mut self.stores {
            self.pointers.0.push(ColumnBuffer {
                values: store.values.as_mut_ptr().cast(),
                verdicts: store.verdicts.as_mut_ptr(),
            });
        }
        (&self.specs, &self.pointers.0)
    }
}

/// Where a batch came from, which is where its cells' text lies.
#[derive(Clone, Copy)]
pub(crate) enum Origin {
    /// Delimited text: the cell table is indexed by source column, an unflagged span is
    /// in the input, and a flagged cell is quoted text still to be unescaped.
    Delimited,
    /// A workbook: the cell table is indexed by plan column, and an unflagged span is in
    /// the shared strings.
    Workbook,
}

/// A batch of rows read through a plan. See the module documentation.
#[derive(Clone, Copy)]
pub struct Batch<'r> {
    columns: &'r Columns,
    rows: usize,
    cells: &'r [Span],
    /// Entries of `cells` to a row.
    per_row: usize,
    /// Where an unflagged span's bytes are: the input, or the shared strings.
    base: &'r [u8],
    /// Where a flagged span's bytes are.
    arena: &'r [u8],
    origin: Origin,
}

mod sealed {
    pub trait Sealed {}
}

/// A type a cell can be read as: what one of the doors produces.
///
/// | Type | Doors |
/// |---|---|
/// | `bool` | `bool` |
/// | `i8` … `u64`, `f32`, `f64` | the door of the same name |
/// | [`Decimal`] | `decimal` |
/// | `[u8; 16]` | `uuid`, as its sixteen bytes in text order |
/// | [`Timestamp`] | `timestamp`, `unix`, `excel_serial` |
/// | [`Date`] | `date`, `date_ordered` |
/// | [`CivilDateTime`] | `datetime` |
/// | `u64` | `time`, as nanoseconds since midnight (and the `u64` door) |
/// | [`Duration`] | `duration` |
///
/// The text door is read with [`Batch::text`].
pub trait Value: Copy + sealed::Sealed + 'static {
    #[doc(hidden)]
    const NAME: &'static str;
    #[doc(hidden)]
    fn reads(door: Door) -> bool;
}

macro_rules! values {
    ($($ty:ty => $($door:pat_param)|+),+ $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl Value for $ty {
            const NAME: &'static str = stringify!($ty);
            fn reads(door: Door) -> bool {
                matches!(door, $($door)|+)
            }
        }
        const _: () = assert!(align_of::<$ty>() <= align_of::<u64>());
    )+};
}

values! {
    bool => Door::Bool,
    i8 => Door::I8,
    i16 => Door::I16,
    i32 => Door::I32,
    i64 => Door::I64,
    u8 => Door::U8,
    u16 => Door::U16,
    u32 => Door::U32,
    u64 => Door::U64 | Door::Time,
    f32 => Door::F32,
    f64 => Door::F64,
    Decimal => Door::Decimal,
    [u8; 16] => Door::Uuid,
    Timestamp => Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_),
    Date => Door::Date | Door::DateOrdered(_),
    CivilDateTime => Door::DateTime(_),
    Duration => Door::Duration,
}

macro_rules! whole_columns {
    ($($name:ident: $ty:ty),+ $(,)?) => {$(
        #[doc = concat!("Column `column` whole, as the `", stringify!($ty), "`s its door made: one a row, and the door's zero where the verdict beside it is not ok.")]
        ///
        /// # Panics
        /// If the column is read through another door.
        pub fn $name(&self, column: usize) -> &'r [$ty] {
            self.values::<$ty>(column)
        }
    )+};
}

impl<'r> Batch<'r> {
    pub(crate) fn new(
        columns: &'r Columns,
        rows: usize,
        cells: &'r [Span],
        per_row: usize,
        base: &'r [u8],
        arena: &'r [u8],
        origin: Origin,
    ) -> Batch<'r> {
        Batch {
            columns,
            rows,
            cells,
            per_row,
            base,
            arena,
            origin,
        }
    }

    /// How many rows the batch holds. Never zero: a reader with no more rows has no batch.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The plan the batch was read through: column `i` of the batch is `columns()[i]`.
    pub fn columns(&self) -> &'r [Column] {
        self.columns.plan()
    }

    /// Where row `row` came from: for delimited text the 1-based line its record starts
    /// on, for a sheet its 1-based row number.
    pub fn line(&self, row: usize) -> u32 {
        assert!(row < self.rows, "row {row} of a batch of {}", self.rows);
        let entry = self
            .cells
            .get(row * self.per_row + self.per_row - 1)
            .copied()
            .unwrap_or_default();
        match self.origin {
            Origin::Delimited => entry.len,
            Origin::Workbook => entry.offset,
        }
    }

    fn store(&self, column: usize) -> &'r Store {
        let columns = self.columns;
        columns.stores.get(column).unwrap_or_else(|| {
            panic!(
                "column {column} of a plan of {} columns",
                columns.plan.len()
            )
        })
    }

    /// The verdicts of column `column`, one a row. A verdict with reason `0` is a value;
    /// any other is a fault ([`CellVerdict::fault`]), with its span in the cell's text.
    pub fn verdicts(&self, column: usize) -> &'r [CellVerdict] {
        &self.store(column).verdicts[..self.rows]
    }

    /// The column's values as `T`s, the door not asked about.
    /// Column `column` as the core wrote it, as bytes: `rows` values of its door, then its
    /// verdicts — what the Python binding copies a batch out of.
    #[cfg(feature = "python")]
    pub(crate) fn column_bytes(&self, column: usize) -> (&'r [u8], &'r [CellVerdict]) {
        let store = self.store(column);
        let length =
            (size_of_value(self.store_door(column)) * self.rows).min(store.values.len() * 8);
        // SAFETY: the first `length` bytes of the store, which holds at least that many;
        // any bytes are valid `u8`s.
        let values =
            unsafe { std::slice::from_raw_parts(store.values.as_ptr().cast::<u8>(), length) };
        (values, &store.verdicts[..self.rows])
    }

    /// The batch's cell table — `per_row` entries a row — what its unflagged spans index,
    /// and what its flagged spans index: what the Python binding copies a batch out of.
    #[cfg(feature = "python")]
    pub(crate) fn tables(&self) -> (&'r [Span], usize, &'r [u8], &'r [u8]) {
        (
            self.cells
                .get(..self.rows * self.per_row)
                .unwrap_or_default(),
            self.per_row,
            self.base,
            self.arena,
        )
    }

    fn stored<T: Copy>(&self, column: usize) -> &'r [T] {
        let store = self.store(column);
        debug_assert!(size_of::<T>() * self.rows <= store.values.len() * 8);
        // SAFETY: the store is aligned for `u64`, which is enough for every `T` this is
        // called with, and holds `batch_rows` values of the column's door, of which the
        // core wrote the first `rows`; the callers ask only for the type that door writes.
        unsafe { std::slice::from_raw_parts(store.values.as_ptr().cast::<T>(), self.rows) }
    }

    fn values<T: Value>(&self, column: usize) -> &'r [T] {
        let door = self.store_door(column);
        assert!(
            T::reads(door),
            "column {column} is read through the {door:?} door, which does not make a {}",
            T::NAME
        );
        self.stored(column)
    }

    fn store_door(&self, column: usize) -> Door {
        self.store(column);
        self.columns.plan[column].door
    }

    whole_columns! {
        bool: bool,
        i8: i8,
        i16: i16,
        i32: i32,
        i64: i64,
        u8: u8,
        u16: u16,
        u32: u32,
        u64: u64,
        f32: f32,
        f64: f64,
    }

    /// The cell at `row` of column `column` as HyperCast judged it: the value, or the
    /// fault. `T` is the type the column's door makes — see [`Value`].
    ///
    /// # Panics
    /// If the column's door does not make a `T`, or the row is not in the batch.
    pub fn get<T: Value>(&self, column: usize, row: usize) -> Result<T, Fault> {
        let value = self.values::<T>(column)[row];
        match self.verdicts(column)[row].fault() {
            Some(fault) => Err(fault),
            None => Ok(value),
        }
    }

    /// The bytes a span names.
    fn bytes(&self, span: Span) -> &'r [u8] {
        let store = if span.flagged() {
            self.arena
        } else {
            self.base
        };
        let from = span.offset as usize;
        store.get(from..from + span.len()).unwrap_or_default()
    }

    /// The cell at `row` of a text column: its bytes, untrimmed — or an `Empty` fault for
    /// a cell with none. A typed workbook cell is said the canonical way: `42`, `true`,
    /// `2024-01-31T10:30:00`, `PT1H30M`.
    ///
    /// # Panics
    /// If the column is not read through the text door, or the row is not in the batch.
    pub fn text(&self, column: usize, row: usize) -> Result<&'r [u8], Fault> {
        let door = self.store_door(column);
        assert!(
            door == Door::Text,
            "column {column} is read through the {door:?} door, not the text door"
        );
        let span = self.stored::<Span>(column)[row];
        match self.verdicts(column)[row].fault() {
            Some(fault) => Err(fault),
            None => Ok(self.bytes(span)),
        }
    }

    /// [`Batch::text`] as a `str`: borrowed from the batch when the bytes are UTF-8, which
    /// they are unless the input was not, and otherwise a copy with each invalid sequence
    /// replaced by U+FFFD — the one way this allocates.
    ///
    /// # Panics
    /// As [`Batch::text`].
    pub fn text_str(&self, column: usize, row: usize) -> Result<Cow<'r, str>, Fault> {
        self.text(column, row).map(String::from_utf8_lossy)
    }

    /// The row at `row`: the batch read across instead of down.
    ///
    /// # Panics
    /// If the row is not in the batch.
    pub fn row(&self, row: usize) -> Row<'r> {
        assert!(row < self.rows, "row {row} of a batch of {}", self.rows);
        Row {
            batch: *self,
            index: row,
        }
    }

    /// The batch's rows in order — what `for row in batch` iterates.
    pub fn iter(&self) -> Rows<'r> {
        Rows {
            batch: *self,
            front: 0,
            back: self.rows,
        }
    }

    /// The text of the cell at `row` of column `column`, whatever its door and whatever
    /// its verdict: what a fault's span is a span of. Borrowed, unless the cell is quoted
    /// delimited text with a doubled quote in it, which has to be unescaped to be read.
    ///
    /// For a workbook this is a text cell's own text, and what a typed cell was said as
    /// if it failed its door (or went through the text door); a typed cell that cast has
    /// no text, and neither has an empty cell.
    ///
    /// # Panics
    /// If the column or the row is not in the batch.
    pub fn raw(&self, column: usize, row: usize) -> Cow<'r, [u8]> {
        self.store(column);
        assert!(row < self.rows, "row {row} of a batch of {}", self.rows);
        let at = match self.origin {
            Origin::Delimited => self.columns.plan[column].ordinal,
            Origin::Workbook => column,
        };
        let cell = self
            .cells
            .get(row * self.per_row + at)
            .copied()
            .unwrap_or_default();
        match self.origin {
            Origin::Workbook => Cow::Borrowed(self.bytes(cell)),
            Origin::Delimited => {
                let from = cell.offset as usize;
                let quoted = self.base.get(from..from + cell.len()).unwrap_or_default();
                if !cell.flagged() {
                    return Cow::Borrowed(quoted);
                }
                let mut text = vec![0; quoted.len()];
                let written = unescape_into(quoted, &mut text);
                text.truncate(written);
                Cow::Owned(text)
            }
        }
    }
}

impl<'r> IntoIterator for Batch<'r> {
    type Item = Row<'r>;
    type IntoIter = Rows<'r>;

    fn into_iter(self) -> Rows<'r> {
        self.iter()
    }
}

impl<'r> IntoIterator for &Batch<'r> {
    type Item = Row<'r>;
    type IntoIter = Rows<'r>;

    fn into_iter(self) -> Rows<'r> {
        self.iter()
    }
}

/// One row of a [`Batch`]: the batch read across rather than down, for code that builds a
/// value a row at a time. A view of the batch and nothing more — each method is the
/// batch's own with this row's index — so it borrows the reader as the batch does, and is
/// over when the next batch is read.
///
/// ```
/// # use hypertabular::{Column, DelimitedReader, Dialect};
/// let plan = [Column::text(0), Column::i32(1)];
/// let mut reader = DelimitedReader::from_slice(b"name,n\na,1\nb,2\n", Dialect::CSV, &plan)?;
/// while let Some(batch) = reader.read()? {
///     for row in batch {
///         let (name, n) = (row.text_str(0), row.get::<i32>(1));
///     }
/// }
/// # Ok::<(), hypertabular::Error>(())
/// ```
#[derive(Clone, Copy)]
pub struct Row<'r> {
    batch: Batch<'r>,
    index: usize,
}

impl<'r> Row<'r> {
    /// The row's place in its batch, from zero.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Where the row came from: [`Batch::line`].
    pub fn line(&self) -> u32 {
        self.batch.line(self.index)
    }

    /// The cell in column `column` as HyperCast judged it: [`Batch::get`].
    ///
    /// # Panics
    /// As [`Batch::get`].
    pub fn get<T: Value>(&self, column: usize) -> Result<T, Fault> {
        self.batch.get(column, self.index)
    }

    /// The verdict of the cell in column `column`.
    ///
    /// # Panics
    /// If the column is not in the batch.
    pub fn verdict(&self, column: usize) -> CellVerdict {
        self.batch.verdicts(column)[self.index]
    }

    /// The text cell in column `column`: [`Batch::text`].
    ///
    /// # Panics
    /// As [`Batch::text`].
    pub fn text(&self, column: usize) -> Result<&'r [u8], Fault> {
        self.batch.text(column, self.index)
    }

    /// The text cell in column `column` as a `str`: [`Batch::text_str`].
    ///
    /// # Panics
    /// As [`Batch::text`].
    pub fn text_str(&self, column: usize) -> Result<Cow<'r, str>, Fault> {
        self.batch.text_str(column, self.index)
    }

    /// The text the cell in column `column` was cast from: [`Batch::raw`].
    ///
    /// # Panics
    /// If the column is not in the batch.
    pub fn raw(&self, column: usize) -> Cow<'r, [u8]> {
        self.batch.raw(column, self.index)
    }
}

impl std::fmt::Debug for Row<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("index", &self.index)
            .field("line", &self.line())
            .finish()
    }
}

/// The rows of a [`Batch`], in order: [`Batch::iter`].
#[derive(Clone)]
pub struct Rows<'r> {
    batch: Batch<'r>,
    front: usize,
    back: usize,
}

impl<'r> Iterator for Rows<'r> {
    type Item = Row<'r>;

    fn next(&mut self) -> Option<Row<'r>> {
        (self.front < self.back).then(|| {
            self.front += 1;
            Row {
                batch: self.batch,
                index: self.front - 1,
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.back - self.front;
        (left, Some(left))
    }
}

impl DoubleEndedIterator for Rows<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        (self.front < self.back).then(|| {
            self.back -= 1;
            Row {
                batch: self.batch,
                index: self.back,
            }
        })
    }
}

impl ExactSizeIterator for Rows<'_> {}

impl std::iter::FusedIterator for Rows<'_> {}

impl std::fmt::Debug for Batch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Batch")
            .field("rows", &self.rows)
            .field("columns", &self.columns.plan.len())
            .finish()
    }
}
