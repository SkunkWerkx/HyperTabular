//! A batch of rows, column-major: for each column of the plan, the values its door made
//! and a verdict beside each. The reader owns the memory and lends it — a batch is a view,
//! valid until the reader is asked for the next one — so nothing in a batch was copied to
//! make it: a column of integers is the array the core wrote, and a text cell is a slice
//! of the input (or of the workbook's shared strings) wherever the bytes could stay where
//! they were.
//!
//! One type serves delimited text and workbooks alike; the two differ only in where a
//! cell's text lies, which the batch knows and its accessors hide.

use crate::column::{AllZero, zeroed};
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBufferWith, ColumnSpecWith, OK, RawDelimitedOptions, RawProblem,
    Span,
};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use crate::kernel::faults::faults;
use crate::kernel::size::value_bytes;
use crate::{Column, Error};
use hypercast::{
    Checked, CivilDateTime, Date, Decimal, Detail, Duration, Fault, Keyword, RawCheck, Reason,
    Rejection, Timestamp, Violation,
};
use std::borrow::Cow;

/// One column's room: `batch_rows` values of its door's type, and as many verdicts.
struct Store {
    /// The values, as bytes the core writes — held as `u64`s so that they are aligned for
    /// every type a door produces.
    values: Vec<u64>,
    verdicts: Vec<CellVerdict>,
    /// As many notes, or none for a read that doesn't take them.
    notes: Vec<CellNote>,
    /// As many checks for a column that declares constraints, or none.
    checks: Vec<RawCheck>,
}

/// The pointers handed to the core for one call.
struct Pointers<T>(Vec<T>);

impl<T> Default for Pointers<T> {
    fn default() -> Self {
        Pointers(Vec::new())
    }
}

// SAFETY: the pointers are into the stores beside them, written through only inside a
// call that holds the columns exclusively, and never read back out of this list.
unsafe impl<T> Send for Pointers<T> {}
// SAFETY: as above; a shared reference to the list reaches nothing.
unsafe impl<T> Sync for Pointers<T> {}

/// The plan as a fill takes it.
struct SpecsWith(Vec<ColumnSpecWith>);

// SAFETY: the pointers in these specs are to lists interned for the life of the program, which
// nothing writes.
unsafe impl Send for SpecsWith {}
// SAFETY: as above.
unsafe impl Sync for SpecsWith {}

/// The columns a reader owns: the plan, the plan as the core takes it, and a store for
/// each column.
pub(crate) struct Columns {
    plan: Vec<Column>,
    specs_with: SpecsWith,
    stores: Vec<Store>,
    pointers_with: Pointers<ColumnBufferWith>,
    /// The cells each row of the batch had, or none for a read that doesn't report long rows.
    widths: Vec<u32>,
    batch_rows: usize,
}

/// `count` zeroed elements, or [`Error::NoMemory`] when a size the core allowed can't be had.
fn lent<T: AllZero>(count: usize) -> Result<Vec<T>, Error> {
    zeroed(count).ok_or(Error::NoMemory {
        bytes: count.saturating_mul(size_of::<T>()) as u64,
    })
}

impl Columns {
    /// Room for `batch_rows` rows of `plan`, with room for a note beside each verdict when
    /// `notes` says so, and for each row's width when `widths` does.
    pub(crate) fn with_options(
        plan: &[Column],
        specs: Vec<ColumnSpecWith>,
        batch_rows: usize,
        notes: bool,
        widths: bool,
    ) -> Result<Columns, Error> {
        // The core has said how much this all comes to, and allowed it, before anything here was
        // allocated. The caller had that answer, so this can't overflow a size the core allowed.
        let stores = plan
            .iter()
            .map(|column| {
                let bytes = value_bytes(column.door) as usize * batch_rows;
                Ok(Store {
                    values: lent(bytes.div_ceil(8))?,
                    verdicts: lent(batch_rows)?,
                    notes: lent(if notes { batch_rows } else { 0 })?,
                    checks: lent(if column.is_checked() { batch_rows } else { 0 })?,
                })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Columns {
            plan: plan.to_vec(),
            specs_with: SpecsWith(specs),
            stores,
            pointers_with: Pointers::default(),
            widths: lent(if widths { batch_rows } else { 0 })?,
            batch_rows,
        })
    }

    pub(crate) fn plan(&self) -> &[Column] {
        &self.plan
    }

    pub(crate) fn batch_rows(&self) -> usize {
        self.batch_rows
    }

    /// The plan and the column buffers as the core takes them. Each buffer has room for
    /// `batch_rows` values of its column's door and as many verdicts, and as many notes unless it
    /// asks for none, and the row widths are `batch_rows` counts or none.
    pub(crate) fn for_core_with(&mut self) -> (&[ColumnSpecWith], &[ColumnBufferWith], &mut [u32]) {
        self.pointers_with.0.clear();
        for store in &mut self.stores {
            self.pointers_with.0.push(ColumnBufferWith {
                size: ColumnBufferWith::SIZE,
                values: store.values.as_mut_ptr().cast(),
                verdicts: store.verdicts.as_mut_ptr(),
                notes: if store.notes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    store.notes.as_mut_ptr()
                },
                checks: if store.checks.is_empty() {
                    std::ptr::null_mut()
                } else {
                    store.checks.as_mut_ptr()
                },
            });
        }
        (&self.specs_with.0, &self.pointers_with.0, &mut self.widths)
    }
}

/// Where a cell's verdict came from: [`Batch::origin`].
///
/// New origins may be added in a minor release. Matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Origin {
    /// A door judged the cell's own text.
    #[default]
    Cell,
    /// The cell's exact unquoted bytes are a declared token, so it is `Empty` and no door ran.
    Token,
    /// The row has no such cell: padding for a short row, or an ordinal past the row's end. It is
    /// `Empty` and no door ran.
    Padded,
    /// A workbook's formula cell with no cached value. It is `Empty` and no door ran.
    Uncached,
}

impl Origin {
    pub(crate) fn from_code(code: u16) -> Origin {
        match code {
            CellNote::ORIGIN_TOKEN => Origin::Token,
            CellNote::ORIGIN_PADDED => Origin::Padded,
            CellNote::ORIGIN_UNCACHED => Origin::Uncached,
            _ => Origin::Cell,
        }
    }
}

/// Where a batch came from, which is where its cells' text lies.
#[derive(Clone, Copy)]
pub(crate) enum Home {
    /// Delimited text: the cell table is indexed by source column, an unflagged span is
    /// in the input, and a flagged cell is quoted text still to be unescaped. A read that reports
    /// long rows says the width the first record fixed.
    Delimited { reported_width: Option<u32> },
    /// A workbook: the cell table is indexed by plan column, and an unflagged span is in
    /// the shared strings. Ordinal `0` of the plan is the sheet's column `first_column`, from zero.
    Workbook { first_column: u32 },
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
    home: Home,
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
        home: Home,
    ) -> Batch<'r> {
        Batch {
            columns,
            rows,
            cells,
            per_row,
            base,
            arena,
            home,
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
        match self.home {
            Home::Delimited { .. } => entry.len,
            Home::Workbook { .. } => entry.offset,
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
    /// [`CellVerdict::VIOLATION`], on a column that declares constraints, a value that broke one,
    /// which [`Batch::checks`] names; any other is a fault ([`CellVerdict::fault`]), with its span
    /// in the cell's text.
    pub fn verdicts(&self, column: usize) -> &'r [CellVerdict] {
        &self.store(column).verdicts[..self.rows]
    }

    /// The notes of column `column`, one a row: each cell's detail code and where its verdict came
    /// from, as [`Batch::detail`] and [`Batch::origin`] read them. Empty for a read with notes off.
    ///
    /// # Panics
    /// If the column is not in the batch.
    pub fn notes(&self, column: usize) -> &'r [CellNote] {
        self.store(column)
            .notes
            .get(..self.rows)
            .unwrap_or_default()
    }

    /// What failed in the cell at `row` of column `column`, in HyperCast's terms, beside the fault
    /// in its verdict: `None` for a cell that cast, or for an `Empty` no door gave
    /// ([`Batch::origin`]).
    ///
    /// # Panics
    /// If the column or the row is not in the batch.
    pub fn detail(&self, column: usize, row: usize) -> Option<Detail> {
        Detail::from_code(u32::from(self.note(column, row).detail))
    }

    /// Where the verdict of the cell at `row` of column `column` came from: a door, a declared
    /// token, padding, or a workbook's formula with no cached value. With notes off
    /// ([`DelimitedOptions::notes`](crate::DelimitedOptions::notes)) a token or a padded cell answers
    /// [`Origin::Cell`]: its verdict is still `Empty`, and nothing was kept of why.
    ///
    /// # Panics
    /// If the column or the row is not in the batch.
    pub fn origin(&self, column: usize, row: usize) -> Origin {
        Origin::from_code(self.note(column, row).origin)
    }

    /// What went wrong in this batch, in order by row and then by column, a row's own entry first:
    /// cells at [`Reason::Malformed`] or [`Reason::OutOfRange`], formulas with no cached value
    /// ([`Origin::Uncached`]), and rows longer than the width under
    /// [`LongRows::Report`](crate::LongRows::Report). Blanks, declared tokens, padding and a row cut
    /// under [`LongRows::Truncate`](crate::LongRows::Truncate) aren't problems, and aren't listed.
    ///
    /// It reads what the fill already wrote, when it's asked for, and allocates nothing.
    ///
    /// With notes off ([`DelimitedOptions::notes`](crate::DelimitedOptions::notes),
    /// [`SheetOptions::with_notes`](crate::SheetOptions::with_notes)) the list still holds every
    /// refused cell and long row, with no [`Problem::detail`], but no formula with no cached value:
    /// nothing was kept to tell one from a blank.
    ///
    /// The list can't name a row it wasn't given. A row of a sheet whose only cells are formulas with
    /// nothing spelled where a value goes is an empty row, so by default
    /// [`SheetOptions::skip_empty_rows`](crate::SheetOptions::skip_empty_rows) leaves it out, and it's
    /// in no batch. Turn that off to have those rows delivered and listed.
    ///
    /// ```
    /// # use hypertabular::{Column, DelimitedReader, Dialect, Reason};
    /// let plan = [Column::text(0), Column::i32(1)];
    /// let mut reader = DelimitedReader::from_slice(b"name,n\na,1\nb,x\n", Dialect::CSV, &plan)?;
    /// let batch = reader.read()?.unwrap();
    /// let problem = batch.faults().next().unwrap();
    /// assert_eq!((problem.row, problem.line, problem.plan_column), (1, 3, Some(1)));
    /// assert_eq!(problem.reason, Some(Reason::Malformed));
    /// # Ok::<(), hypertabular::Error>(())
    /// ```
    pub fn faults(&self) -> Faults<'r> {
        Faults {
            batch: *self,
            cursor: 0,
            held: [RawProblem::default(); FAULTS_HELD],
            at: 0,
            len: 0,
            done: false,
        }
    }

    /// A list entry, with what only the batch knows.
    fn problem(&self, raw: RawProblem) -> Problem {
        let plan_column =
            (raw.plan_column != RawProblem::ROW_LEVEL).then_some(raw.plan_column as usize);
        Problem::from_raw(
            raw,
            self.line(raw.row as usize),
            plan_column.map(|column| self.source_column(column)),
        )
    }

    /// The source column plan column `column` reads, from zero: for delimited text its ordinal, and
    /// for a sheet its own column, `A` being `0`, wherever the range starts. What
    /// [`Problem::source_column`] says of a cell in it.
    ///
    /// # Panics
    /// If the column is not in the plan.
    pub fn source_column(&self, column: usize) -> usize {
        let ordinal = self.columns.plan[column].ordinal;
        match self.home {
            Home::Delimited { .. } => ordinal,
            Home::Workbook { first_column } => ordinal + first_column as usize,
        }
    }

    /// For a read that reports long rows ([`LongRows::Report`](crate::LongRows::Report)), the width
    /// the first record fixed and each row's own, one a row; `None` for any other read, and for a
    /// sheet. With each column's [`Batch::verdicts`] and [`Batch::notes`] it's what a caller keeps
    /// to have the batch's faults list worked out later, from a [`KeptBatch`](crate::KeptBatch).
    pub fn reported_widths(&self) -> Option<(u32, &'r [u32])> {
        match self.home {
            Home::Delimited {
                reported_width: Some(width),
            } => Some((
                width,
                self.columns.widths.get(..self.rows).unwrap_or_default(),
            )),
            _ => None,
        }
    }

    fn note(&self, column: usize, row: usize) -> CellNote {
        let store = self.store(column);
        assert!(row < self.rows, "row {row} of a batch of {}", self.rows);
        store.notes.get(row).copied().unwrap_or_default()
    }

    /// Column `column` as the core wrote it, as bytes: `rows` values of its door, then its
    /// verdicts — what the Python and PHP bindings copy a batch out of.
    #[cfg(any(feature = "python", feature = "php"))]
    pub(crate) fn column_bytes(&self, column: usize) -> (&'r [u8], &'r [CellVerdict]) {
        let store = self.store(column);
        let length = (crate::kernel::size::value_bytes(self.store_door(column)) as usize
            * self.rows)
            .min(store.values.len() * 8);
        // SAFETY: the first `length` bytes of the store, which holds at least that many;
        // any bytes are valid `u8`s.
        let values =
            unsafe { std::slice::from_raw_parts(store.values.as_ptr().cast::<u8>(), length) };
        (values, &store.verdicts[..self.rows])
    }

    /// The batch's cell table — `per_row` entries a row — what its unflagged spans index,
    /// and what its flagged spans index: what the Python and PHP bindings copy a batch out of.
    #[cfg(any(feature = "python", feature = "php"))]
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

    /// A whole typed column, borrowed from the reader's buffers: one value per row,
    /// without copying. `T` must be the type the column's door produces (see [`Value`]).
    ///
    /// Read [`Batch::verdicts`] beside the values: a row whose verdict is not ok holds
    /// the door's zero, not a successfully cast value, but for a violation of a constrained
    /// column's declaration ([`CellVerdict::VIOLATION`]), which holds the value as cast. Text uses
    /// [`Batch::text`].
    /// The slice borrows the reader for the batch's lifetime, so it cannot be retained
    /// across another read of that reader.
    ///
    /// ```compile_fail,E0499
    /// use hypertabular::{Column, DelimitedReader, Dialect};
    /// let mut reader = DelimitedReader::from_slice(b"n\n1\n2\n", Dialect::CSV, &[Column::i32(0)]).unwrap();
    /// let batch = reader.read().unwrap().unwrap();
    /// let values = batch.values::<i32>(0);
    /// reader.read().unwrap();
    /// println!("{values:?}"); // the slice still borrows the reader
    /// ```
    ///
    /// # Panics
    /// If `column` is outside the plan or its door does not produce a `T`.
    pub fn values<T: Value>(&self, column: usize) -> &'r [T] {
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
    /// If the column's door does not make a `T`, the column declares constraints (read it with
    /// [`Batch::get_checked`]), or the row is not in the batch.
    pub fn get<T: Value>(&self, column: usize, row: usize) -> Result<T, Fault> {
        self.unchecked(column);
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
    /// a cell with none, or a `Malformed` one for bytes that aren't UTF-8, whose span is
    /// the first invalid sequence ([`Batch::raw`] still has them). A typed workbook cell is
    /// said the canonical way: `42`, `true`,
    /// `2024-01-31T10:30:00`, `PT1H30M`.
    ///
    /// # Panics
    /// If the column is not read through the text door, declares constraints (read it with
    /// [`Batch::text_checked`]), or the row is not in the batch.
    pub fn text(&self, column: usize, row: usize) -> Result<&'r [u8], Fault> {
        self.unchecked(column);
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

    /// [`Batch::text`] as a `str`, borrowed from the batch: the text door gives bytes that
    /// aren't UTF-8 a `Malformed` verdict, so a cell with a value never needs the copy the
    /// `Cow` allows for.
    ///
    /// # Panics
    /// As [`Batch::text`].
    pub fn text_str(&self, column: usize, row: usize) -> Result<Cow<'r, str>, Fault> {
        self.text(column, row).map(String::from_utf8_lossy)
    }

    /// Refuses a column that declares constraints, whose verdicts the plain getters can't say.
    fn unchecked(&self, column: usize) {
        assert!(
            !self.columns.plan[column].is_checked(),
            "column {column} declares constraints: read it with get_checked or text_checked"
        );
    }

    /// The checks of column `column`, one a row, as the core wrote them beside a constrained
    /// column's verdicts: on a cell that cast, whether rounding adjusted it and the `enum` entry it
    /// matched; on a violation, the keyword. Empty for a column that declares none. A cell that
    /// didn't cast has no check, and what's there is stale: read it beside the verdict, or through
    /// [`Batch::get_checked`].
    pub fn checks(&self, column: usize) -> &'r [RawCheck] {
        let store = self.store(column);
        store.checks.get(..self.rows).unwrap_or_default()
    }

    /// The verdict at `row` of a column read through the checked doors, its value set aside: the
    /// check of a cell that passed, or what refused it.
    fn judged(&self, column: usize, row: usize) -> Result<RawCheck, Refused> {
        let verdict = self.verdicts(column)[row];
        let check = self.checks(column).get(row).copied();
        if verdict.reason == CellVerdict::VIOLATION {
            return Err(Refused::Broke {
                keyword: check.and_then(|check| keyword_of(check.keyword)),
                offset: verdict.offset,
                len: verdict.len,
            });
        }
        match verdict.fault() {
            Some(fault) => Err(Refused::Fault(fault)),
            // A column without constraints has no checks, and nothing was adjusted or matched.
            None => Ok(check.unwrap_or(RawCheck {
                keyword: 0,
                adjusted: 0,
                enum_index: u32::MAX,
            })),
        }
    }

    /// The cell at `row` of column `column` as HyperCast's checked door judged it: the value,
    /// flagged when `multipleOf`'s rounding adjusted it; a [`Violation`] of a constraint the
    /// column declares ([`Column::checked`](crate::Column::checked)), carrying the value as cast
    /// and its span; or the cell's fault, a cell no door was asked about included. A column that
    /// declares none reads as its [`Batch::get`] does, never adjusted. `T` is the type the
    /// column's door makes.
    ///
    /// # Panics
    /// If the column's door does not make a `T`, or the row is not in the batch.
    pub fn get_checked<T: Value>(
        &self,
        column: usize,
        row: usize,
    ) -> Result<Checked<T>, Rejection<T>> {
        let value = self.values::<T>(column)[row];
        self.settled(column, row, value)
    }

    fn settled<V>(&self, column: usize, row: usize, value: V) -> Result<Checked<V>, Rejection<V>> {
        match self.judged(column, row) {
            Ok(check) => Ok(Checked::new(
                value,
                check.adjusted != 0,
                (check.enum_index != u32::MAX).then_some(check.enum_index),
            )),
            Err(Refused::Broke {
                keyword: Some(keyword),
                offset,
                len,
            }) => Err(Rejection::Violation(Violation::new(
                keyword, value, offset, len,
            ))),
            // A keyword no check names, which no fill writes: the cell is still refused.
            Err(Refused::Broke { offset, len, .. }) => Err(Rejection::Fault(Fault {
                reason: Reason::OutOfRange,
                offset,
                len,
            })),
            Err(Refused::Fault(fault)) => Err(Rejection::Fault(fault)),
        }
    }

    /// The text cell at `row` of column `column` as HyperCast's checked text door judged it: the
    /// text it judged (after the `trim` the column declares), or, matching an `enum`, the declared
    /// entry it matched, which [`Checked::enum_index`] names; a [`Violation`] carrying the text it
    /// judged; or the cell's fault. A column that declares none reads as its [`Batch::text`] does,
    /// as text: a cell whose bytes aren't UTF-8 is already `Malformed`.
    ///
    /// # Panics
    /// If the column is not read through the text door, or the row is not in the batch.
    pub fn text_checked(
        &self,
        column: usize,
        row: usize,
    ) -> Result<Checked<&'r str>, Rejection<&'r str>> {
        let door = self.store_door(column);
        assert!(
            door == Door::Text,
            "column {column} is read through the {door:?} door, not the text door"
        );
        let span = self.stored::<Span>(column)[row];
        // The text door passed these bytes, or they're the text a violation carries: UTF-8.
        let text = std::str::from_utf8(self.bytes(span)).unwrap_or_default();
        let judged = self.settled(column, row, text);
        let entries = self.columns.plan[column].text_constraints();
        match (judged, entries.and_then(|declared| declared.enum_entries())) {
            (Ok(passed), Some(mut entries)) => {
                let entry = passed
                    .enum_index
                    .and_then(|index| entries.nth(index as usize))
                    .and_then(|entry| std::str::from_utf8(entry).ok());
                match entry {
                    Some(entry) => Ok(Checked::new(entry, passed.adjusted, passed.enum_index)),
                    None => Ok(passed),
                }
            }
            (judged, _) => judged,
        }
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
        let at = match self.home {
            Home::Delimited { .. } => self.columns.plan[column].ordinal,
            Home::Workbook { .. } => column,
        };
        let cell = self
            .cells
            .get(row * self.per_row + at)
            .copied()
            .unwrap_or_default();
        match self.home {
            Home::Workbook { .. } => Cow::Borrowed(self.bytes(cell)),
            Home::Delimited { .. } => {
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

    /// The cell in column `column` as HyperCast's checked door judged it: [`Batch::get_checked`].
    ///
    /// # Panics
    /// As [`Batch::get_checked`].
    pub fn get_checked<T: Value>(&self, column: usize) -> Result<Checked<T>, Rejection<T>> {
        self.batch.get_checked(column, self.index)
    }

    /// The text cell in column `column` as HyperCast's checked text door judged it:
    /// [`Batch::text_checked`].
    ///
    /// # Panics
    /// As [`Batch::text_checked`].
    pub fn text_checked(&self, column: usize) -> Result<Checked<&'r str>, Rejection<&'r str>> {
        self.batch.text_checked(column, self.index)
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

    /// What failed in the cell in column `column`: [`Batch::detail`].
    ///
    /// # Panics
    /// If the column is not in the batch.
    pub fn detail(&self, column: usize) -> Option<Detail> {
        self.batch.detail(column, self.index)
    }

    /// Where the verdict of the cell in column `column` came from: [`Batch::origin`].
    ///
    /// # Panics
    /// If the column is not in the batch.
    pub fn origin(&self, column: usize) -> Origin {
        self.batch.origin(column, self.index)
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

/// One thing that went wrong in a [`Batch`]: [`Batch::faults`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Problem {
    /// The row in the batch, from zero.
    pub row: usize,
    /// Where the row came from: [`Batch::line`].
    pub line: u32,
    /// The column of the plan, or `None` for an entry about the row as a whole.
    pub plan_column: Option<usize>,
    /// The column in the source, from zero, or `None` for an entry about the row as a whole. For
    /// delimited text it's the plan column's ordinal, and for a sheet its own column, `A` being `0`,
    /// wherever the range starts.
    pub source_column: Option<usize>,
    /// The verdict's reason: `Malformed`, `OutOfRange`, or `Empty` for a formula with no cached
    /// value. `None` for an entry about the row as a whole, and for a violation, which `keyword`
    /// names.
    pub reason: Option<Reason>,
    /// What failed in the cell: [`Batch::detail`].
    pub detail: Option<Detail>,
    /// Where the verdict came from: [`Batch::origin`]. An entry about the row as a whole has no
    /// verdict, and says [`Origin::Cell`].
    pub origin: Origin,
    /// The fault's span within the cell's text; `0` where there's no span.
    pub offset: u32,
    /// The fault's span's length; `0` where there's no span.
    pub len: u32,
    /// For a long row, the width the first record fixed; otherwise `0`.
    pub expected: u32,
    /// For a long row, the cells it had; otherwise `0`.
    pub found: u32,
    /// For a [`CellVerdict::VIOLATION`], the keyword the cell broke; otherwise `None`.
    pub keyword: Option<Keyword>,
}

/// Entries of the list a [`Faults`] holds at a time.
const FAULTS_HELD: usize = 16;

/// The problems of a [`Batch`], in order: [`Batch::faults`].
#[derive(Clone)]
pub struct Faults<'r> {
    batch: Batch<'r>,
    cursor: u64,
    held: [RawProblem; FAULTS_HELD],
    at: usize,
    len: usize,
    done: bool,
}

impl Problem {
    /// A list entry from the core's, given what the core doesn't know of it.
    pub(crate) fn from_raw(raw: RawProblem, line: u32, source_column: Option<usize>) -> Problem {
        Problem {
            row: raw.row as usize,
            line,
            plan_column: (raw.plan_column != RawProblem::ROW_LEVEL)
                .then_some(raw.plan_column as usize),
            source_column,
            reason: Reason::from_code(raw.reason),
            detail: Detail::from_code(u32::from(raw.detail)),
            origin: Origin::from_code(raw.origin),
            offset: raw.offset,
            len: raw.len,
            expected: raw.expected,
            found: raw.found,
            keyword: keyword_of(raw.keyword),
        }
    }
}

impl Iterator for Faults<'_> {
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
        Some(self.batch.problem(raw))
    }
}

impl std::iter::FusedIterator for Faults<'_> {}

impl Faults<'_> {
    /// The next entries of the list, from where the last call stopped.
    fn fill(&mut self) {
        let batch = self.batch;
        let columns = batch.columns;
        let reported = match batch.home {
            Home::Delimited { reported_width } => reported_width,
            Home::Workbook { .. } => None,
        };
        let (long_rows, expected) = match reported {
            Some(width) => (RawDelimitedOptions::LONG_ROWS_REPORT, width),
            None => (RawDelimitedOptions::LONG_ROWS_REJECT, 0),
        };
        let mut written = 0;
        // SAFETY: the fill that made this batch built these buffers from the stores it borrows,
        // which hold `batch_rows` verdicts and notes, of which `rows` is at most; they don't move.
        let code = unsafe {
            faults(
                &columns.pointers_with.0,
                batch.rows,
                &columns.widths,
                expected,
                long_rows,
                &mut self.cursor,
                &mut self.held,
                &mut written,
            )
        };
        debug_assert_eq!(code, OK);
        (self.at, self.len) = (0, written);
        self.done = code != OK || written < FAULTS_HELD;
    }
}

impl std::fmt::Debug for Faults<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Faults")
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl std::fmt::Debug for Batch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Batch")
            .field("rows", &self.rows)
            .field("columns", &self.columns.plan.len())
            .finish()
    }
}

/// What refused a checked cell: a declared constraint, or the cell's fault.
enum Refused {
    Broke {
        keyword: Option<Keyword>,
        offset: u32,
        len: u32,
    },
    Fault(Fault),
}

/// The keyword a [`RawCheck`] or a [`RawProblem`] names: `1` minimum … `8` enum, `None` for `0`.
pub(crate) fn keyword_of(code: u32) -> Option<Keyword> {
    [
        Keyword::Minimum,
        Keyword::ExclusiveMinimum,
        Keyword::Maximum,
        Keyword::ExclusiveMaximum,
        Keyword::MultipleOf,
        Keyword::MinLength,
        Keyword::MaxLength,
        Keyword::Enum,
    ]
    .into_iter()
    .find(|keyword| keyword.code() == code)
}
