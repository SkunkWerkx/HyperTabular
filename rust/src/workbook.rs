//! Workbooks — XLSX, ODS and Numbers — read through a plan into the batches delimited text
//! is read into.
//!
//! This is the Rust binding over the workbook core ([`crate::kernel::workbook`]), built
//! the way [`crate::delimited`] is over its own: the core is handed the container's bytes
//! and every buffer it works in, and says how far it got; the binding owns the memory,
//! grows the buffer a call names when the core runs out of room, and calls again.
//!
//! A [`Workbook`] holds the container and what every sheet of it reads its cells against:
//! the shared strings and the number-format kind of each cell format, loaded once. A
//! [`Sheet`] is one forward-only read of one sheet; several can be open at once, each
//! with its own buffers, all borrowing the workbook.

use crate::batch::{Batch, Columns, Home, Row};
use crate::column;
use crate::kernel::abi::{
    ColumnSpecWith, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, ERR_WINDOW, Filled, OK, Opened,
    RawBatchCheck, RawRange, RawSheetOptions, Slot, Span,
};
use crate::kernel::workbook::part::WINDOW_MIN;
use crate::kernel::workbook::{FORMAT_NUMBERS, FORMAT_XLSX, Memory, State, book, rows};
use crate::suggest::{Sample, SampleSource, SuggestOptions, Suggestion, check_width};
use crate::{Column, Error, Header};
use hypercast::ExcelEpoch;
use std::borrow::Cow;
use std::path::Path;

/// Which kind of workbook a container holds — told by what is in it, never by its name.
///
/// A format the core learns to read later is a new variant, so a match outside this crate
/// names a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// Office Open XML (`.xlsx`).
    Xlsx,
    /// OpenDocument (`.ods`).
    Ods,
    /// Apple Numbers (`.numbers`), where each table is a sheet.
    Numbers,
}

/// One sheet of a workbook, as its listing names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetInfo {
    /// The sheet's name. (Bytes that are not UTF-8 are replaced.)
    pub name: String,
    /// Whether the workbook hides the sheet. A hidden sheet reads like any other.
    pub hidden: bool,
    /// What the core is given to position on the sheet.
    part: Vec<u8>,
    index: u32,
}

#[cfg(feature = "php")]
impl SheetInfo {
    /// The part the sheet is in, which the PHP binding's `SheetInfo` has always shown.
    pub(crate) fn part(&self) -> &[u8] {
        &self.part
    }

    /// The sheet's place within its part, as the core numbers it, likewise.
    pub(crate) fn index(&self) -> u32 {
        self.index
    }
}

/// The part of a sheet a read keeps: its corners, 1-based like the `A5:F200` they're written as,
/// and included. The first column is ordinal `0` of a plan, while a cell's line and source column
/// stay the sheet's own.
///
/// A range that runs past the sheet's end reads what's there, and adds no rows of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    first_row: u32,
    first_column: u32,
    last_row: u32,
    last_column: u32,
}

impl Range {
    /// `A5:F200`: both corners, in capitals, the second no less than the first in either
    /// direction. The core reads it, so every binding agrees on what one is.
    ///
    /// # Errors
    /// [`Error::Range`] for anything else, `A5` alone, `a5:f200` or `F200:A5`.
    pub fn parse(text: &str) -> Result<Range, Error> {
        rows::parse_range(text.as_bytes())
            .map(
                |RawRange {
                     first_row,
                     first_column,
                     last_row,
                     last_column,
                 }| Range {
                    first_row,
                    first_column,
                    last_row,
                    last_column,
                },
            )
            .ok_or_else(|| Error::Range(text.to_owned()))
    }

    /// A range from its four numbers, each 1-based, with `0` for no limit on that side. That's
    /// how to say what `A5:F200` can't: every row from the fifth, or columns `B` to `D` of all of
    /// them.
    ///
    /// # Errors
    /// [`Error::Range`] if a last is before its first.
    pub fn new(
        first_row: u32,
        first_column: u32,
        last_row: u32,
        last_column: u32,
    ) -> Result<Range, Error> {
        let inverted = |first: u32, last: u32| first != 0 && last != 0 && first > last;
        if inverted(first_row, last_row) || inverted(first_column, last_column) {
            return Err(Error::Range(format!(
                "rows {first_row} to {last_row}, columns {first_column} to {last_column}"
            )));
        }
        Ok(Range {
            first_row,
            first_column,
            last_row,
            last_column,
        })
    }

    /// The first row, or `0` for no limit.
    pub const fn first_row(&self) -> u32 {
        self.first_row
    }

    /// The first column, `A` being `1`, or `0` for no limit.
    pub const fn first_column(&self) -> u32 {
        self.first_column
    }

    /// The last row, or `0` for no limit.
    pub const fn last_row(&self) -> u32 {
        self.last_row
    }

    /// The last column, or `0` for no limit.
    pub const fn last_column(&self) -> u32 {
        self.last_column
    }
}

/// How a sheet is read.
///
/// Start with [`SheetOptions::new`] or [`SheetOptions::default`] and change settings
/// with the `with_*` methods. Additional settings may be added in a minor release.
///
/// ```compile_fail,E0639
/// use hypertabular::SheetOptions;
/// let options = SheetOptions { has_header: true, skip_empty_rows: true, batch_rows: 4096 };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SheetOptions {
    /// Whether the first row delivered is the header rather than data.
    pub has_header: bool,
    /// Whether rows with no cell in them — and the rows a gap in the row numbers stands
    /// for — are left out rather than delivered as rows of empty cells.
    ///
    /// **Set this to `false` to keep every row.** A row whose only cells are formulas with nothing
    /// spelled where a value goes counts as empty, and nothing says it was left out. That depends
    /// on the writer: `<f>…</f>` or `<f>…</f><v />` counts as empty, and `<f>…</f><v></v>` holds a
    /// cell, so a table of nothing but formulas from the first kind of writer can read as no rows.
    /// With this off those rows are delivered, their cells at
    /// [`Origin::Uncached`](crate::Origin::Uncached).
    pub skip_empty_rows: bool,
    /// The most rows a batch holds (at least one).
    pub batch_rows: usize,
    /// Rows of the sheet to pass over at the start of the range, empty ones included. Their cells
    /// aren't judged, so a bad cell there doesn't matter; the XML is still tokenized, or for Numbers
    /// stepped over by its offsets.
    pub skip: u32,
    /// Rows to read and discard after those, as `skip_empty_rows` delivers them, so with it on an
    /// empty row doesn't count.
    pub header_row: u32,
    /// The part of the sheet to read, or all of it.
    pub range: Option<Range>,
    tokens: &'static [u8],
    notes: bool,
}

impl Default for SheetOptions {
    fn default() -> SheetOptions {
        SheetOptions::new()
    }
}

impl SheetOptions {
    /// A header, empty rows skipped, 4096 rows a batch.
    pub const fn new() -> SheetOptions {
        SheetOptions {
            has_header: true,
            skip_empty_rows: true,
            batch_rows: 4096,
            skip: 0,
            header_row: 0,
            range: None,
            tokens: &[],
            notes: true,
        }
    }

    /// The same options with or without a header row.
    pub const fn with_header(self, has_header: bool) -> SheetOptions {
        SheetOptions { has_header, ..self }
    }

    /// The same options skipping empty rows or delivering them.
    pub const fn with_empty_rows_skipped(self, skip_empty_rows: bool) -> SheetOptions {
        SheetOptions {
            skip_empty_rows,
            ..self
        }
    }

    /// The same options with another batch size. Opening a sheet refuses `0`, and a batch of the
    /// plan asking for more than the core allows ([`Error::ZeroBatchRows`],
    /// [`Error::BatchTooLarge`]).
    pub const fn with_batch_rows(self, rows: usize) -> SheetOptions {
        SheetOptions {
            batch_rows: rows,
            ..self
        }
    }

    /// The core's answer on this batch size for `specs`, ahead of everything a sheet allocates.
    fn checked(&self, specs: &[ColumnSpecWith]) -> Result<(), Error> {
        column::check_sizes(specs, self.batch_rows, 0, self.size_flags()).map(|_| ())
    }

    fn size_flags(&self) -> u32 {
        let mut flags = RawBatchCheck::SHEET;
        if self.notes {
            flags |= RawBatchCheck::NOTES;
        }
        flags
    }

    /// The same options passing over this many rows at the start of the range.
    pub const fn with_skip(self, skip: u32) -> SheetOptions {
        SheetOptions { skip, ..self }
    }

    /// The same options reading and discarding this many rows before the header or the first
    /// row of data.
    pub const fn with_header_row(self, header_row: u32) -> SheetOptions {
        SheetOptions { header_row, ..self }
    }

    /// The same options reading only this part of the sheet.
    pub const fn with_range(self, range: Range) -> SheetOptions {
        SheetOptions {
            range: Some(range),
            ..self
        }
    }

    /// The same options with these tokens for no value: a text cell whose exact bytes are one of
    /// them is `Empty`, whatever its door, and [`Batch::origin`] says
    /// [`Origin::Token`](crate::Origin::Token). A number, a boolean or a date is never one, whatever
    /// it would print as. A column's own replace them ([`Column::tokens`]).
    ///
    /// Each distinct list is kept for the life of the process, so that options can stay `Copy`: a
    /// program building lists from request data grows without bound.
    pub fn with_tokens(self, tokens: &[&str]) -> SheetOptions {
        SheetOptions {
            tokens: column::packed_tokens(tokens),
            ..self
        }
    }

    /// The same options writing a note beside each verdict, for [`Batch::detail`] and
    /// [`Batch::origin`], or not. They're on by default; off, the two answer nothing.
    pub const fn with_notes(self, notes: bool) -> SheetOptions {
        SheetOptions { notes, ..self }
    }

    /// The options as the core's state takes them.
    fn raw(&self) -> RawSheetOptions {
        let range = self.range.unwrap_or_default();
        RawSheetOptions {
            size: RawSheetOptions::SIZE,
            has_header: u32::from(self.has_header),
            skip_empty_rows: u32::from(self.skip_empty_rows),
            skip: self.skip,
            header_row: self.header_row,
            first_row: range.first_row,
            first_column: range.first_column,
            last_row: range.last_row,
            last_column: range.last_column,
        }
    }
}

/// Which sheet: by its place in the workbook, or by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetRef<'n> {
    /// The sheet at this zero-based index of [`Workbook::sheets`].
    Index(usize),
    /// The first sheet with this name.
    Name(&'n str),
}

impl From<usize> for SheetRef<'_> {
    fn from(index: usize) -> Self {
        SheetRef::Index(index)
    }
}

impl<'n> From<&'n str> for SheetRef<'n> {
    fn from(name: &'n str) -> Self {
        SheetRef::Name(name)
    }
}

/// The three buffers a call to the core may ask to have grown.
#[derive(Default)]
struct Scratch {
    window: Vec<u8>,
    arena: Vec<u8>,
    cells: Vec<Span>,
}

/// The tables a sheet reads its cells against, borrowed for one call.
#[derive(Clone, Copy, Default)]
struct Tables<'t> {
    strings: &'t [u8],
    table: &'t [Span],
    kinds: &'t [u8],
}

impl Scratch {
    /// Makes `call` until it stops asking for room, growing the buffer it names each time
    /// — with what the buffer held kept, which is what lets the core go on from where it
    /// stopped. Returns the code it ended on (`OK` or a refusal that room does not cure)
    /// and what it reported.
    ///
    /// With `row_follows_cells`, the row's slots are grown with the cell table, so that
    /// there is a slot for every cell a header names: what a sheet whose plan is not yet
    /// known reads its header with (see [`Sheet::read_header`]).
    fn drive(
        &mut self,
        row: &mut Vec<Slot>,
        row_follows_cells: bool,
        tables: Tables<'_>,
        mut call: impl FnMut(&mut Memory<'_>, &mut Filled) -> i32,
    ) -> (i32, Filled) {
        loop {
            let mut out = Filled::default();
            let code = {
                let mut memory = Memory {
                    window: &mut self.window,
                    arena: &mut self.arena,
                    cells: &mut self.cells,
                    row: &mut *row,
                    strings: tables.strings,
                    table: tables.table,
                    kinds: tables.kinds,
                };
                call(&mut memory, &mut out)
            };
            let needed = out.needed as usize;
            match code {
                ERR_WINDOW => self.window.resize(needed.max(self.window.len() + 1), 0),
                ERR_ARENA => self.arena.resize(needed.max(self.arena.len() + 1), 0),
                ERR_CELLS => {
                    self.cells
                        .resize(needed.max(self.cells.len() + 1), Span::default());
                    if row_follows_cells && row.len() < self.cells.len() {
                        row.resize(self.cells.len(), Slot::default());
                    }
                }
                _ => return (code, out),
            }
        }
    }
}

/// What the core's answer means to a caller: `OK`, a structural failure, or — the one
/// thing left — a refusal of the binding's own call, which is this crate's bug.
fn settle(code: i32, out: &Filled) -> Result<(), Error> {
    match code {
        OK => Ok(()),
        ERR_STRUCTURE => Err(out.failure.into()),
        other => unreachable!("the workbook core refused its own binding's call: {other}"),
    }
}

/// A new state block with the contents of one that has been opened: the copy the core's
/// contract allows, so that each sheet has a block of its own.
fn copy_of(state: &State) -> Box<State> {
    let mut copy = Box::new(State::new());
    // SAFETY: a state block is plain integers with no destructor, the two do not
    // overlap, and the core documents an opened block as copyable.
    unsafe { std::ptr::copy_nonoverlapping(state, &mut *copy, 1) };
    copy
}

/// An open workbook. See the module documentation.
pub struct Workbook<'a> {
    container: Cow<'a, [u8]>,
    /// The state as `open` left it: the template every sheet's own state is copied from.
    state: Box<State>,
    format: Format,
    epoch: ExcelEpoch,
    sheets: Vec<SheetInfo>,
    strings: Vec<u8>,
    table: Vec<Span>,
    kinds: Vec<u8>,
}

impl Workbook<'static> {
    /// Reads the file at `path` into memory and opens it.
    pub fn open(path: impl AsRef<Path>) -> Result<Workbook<'static>, Error> {
        Workbook::from_vec(std::fs::read(path)?)
    }

    /// Opens a workbook from bytes it then owns.
    pub fn from_vec(bytes: Vec<u8>) -> Result<Workbook<'static>, Error> {
        Workbook::build(Cow::Owned(bytes))
    }

    /// Reads `reader` to its end and opens what it held — an embedded resource, a network
    /// response, anything that is a stream rather than a file. A workbook is read from its
    /// end (a zip's directory is there), so the whole container is read first, into memory
    /// the workbook then owns.
    pub fn from_reader(mut reader: impl std::io::Read) -> Result<Workbook<'static>, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Workbook::from_vec(bytes)
    }

    /// [`Workbook::from_reader`] over an asynchronous byte source, which is awaited to its
    /// end. Cancelling it (dropping the future) drops what had been read.
    #[cfg(feature = "async")]
    pub async fn from_async_reader(
        reader: impl futures_io::AsyncRead + Send,
    ) -> Result<Workbook<'static>, Error> {
        let mut reader = std::pin::pin!(reader);
        let mut bytes = Vec::new();
        let mut chunk = vec![0; 64 * 1024];
        loop {
            let read = std::future::poll_fn(|cx| reader.as_mut().poll_read(cx, &mut chunk)).await;
            match read {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Workbook::from_vec(bytes)
    }
}

impl<'a> Workbook<'a> {
    /// Opens a workbook in bytes the caller holds — a file it mapped, or read itself.
    pub fn from_slice(bytes: &'a [u8]) -> Result<Workbook<'a>, Error> {
        Workbook::build(Cow::Borrowed(bytes))
    }

    fn build(container: Cow<'a, [u8]>) -> Result<Workbook<'a>, Error> {
        let bytes: &[u8] = &container;
        let mut state = Box::new(State::new());
        let mut scratch = Scratch {
            window: vec![0; WINDOW_MIN],
            arena: vec![0; 1024],
            cells: vec![Span::default(); 64],
        };

        // Opening starts over when it is refused, and reports through its own shape.
        let opened = loop {
            let mut out = Opened::default();
            let code = {
                let mut memory = Memory {
                    window: &mut scratch.window,
                    arena: &mut scratch.arena,
                    cells: &mut scratch.cells,
                    row: &mut [],
                    strings: &[],
                    table: &[],
                    kinds: &[],
                };
                book::open(&mut state, bytes, &mut memory, &mut out)
            };
            let needed = out.needed as usize;
            match code {
                OK => break out,
                ERR_WINDOW => scratch
                    .window
                    .resize(needed.max(scratch.window.len() + 1), 0),
                ERR_ARENA => scratch.arena.resize(needed.max(scratch.arena.len() + 1), 0),
                ERR_STRUCTURE => return Err(out.failure.into()),
                other => unreachable!("the workbook core refused to open: {other}"),
            }
        };
        let format = match opened.format {
            FORMAT_XLSX => Format::Xlsx,
            FORMAT_NUMBERS => Format::Numbers,
            _ => Format::Ods,
        };
        let epoch = ExcelEpoch::from_code(opened.epoch).unwrap_or(ExcelEpoch::Y1900);

        let none = Tables::default();
        let (code, out) = scratch.drive(&mut Vec::new(), false, none, |memory, out| {
            book::sheets(&mut state, bytes, memory, out)
        });
        settle(code, &out)?;
        let text = |span: Span| {
            let from = span.offset as usize;
            &scratch.arena[from..from + span.len()]
        };
        let sheets = scratch.cells[..out.rows as usize * 3]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[name, part, last]| SheetInfo {
                name: String::from_utf8_lossy(text(name)).into_owned(),
                hidden: last.offset & 1 != 0,
                part: text(part).to_vec(),
                index: last.len,
            })
            .collect();

        // The shared strings will not take more room than their part inflates to; asking
        // for it once saves growing into it.
        let bound = usize::try_from(opened.strings_bytes).unwrap_or(usize::MAX);
        if scratch.arena.len() < bound.min(1 << 28) {
            scratch.arena.resize(bound.min(1 << 28), 0);
        }
        let (code, out) = scratch.drive(&mut Vec::new(), false, none, |memory, out| {
            book::strings(&mut state, bytes, memory, out)
        });
        settle(code, &out)?;
        let strings = scratch.arena[..out.arena_used as usize].to_vec();
        let table = scratch.cells[..out.rows as usize].to_vec();

        let (code, out) = scratch.drive(&mut Vec::new(), false, none, |memory, out| {
            book::styles(&mut state, bytes, memory, out)
        });
        settle(code, &out)?;
        let kinds = scratch.arena[..out.rows as usize].to_vec();

        Ok(Workbook {
            container,
            state,
            format,
            epoch,
            sheets,
            strings,
            table,
            kinds,
        })
    }

    /// Which kind of workbook this is.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The date system the workbook's serials count in: what a date-formatted number is
    /// read by.
    pub fn date_system(&self) -> ExcelEpoch {
        self.epoch
    }

    /// The workbook's sheets, in its own order. Sheets that hold no cells (chart sheets,
    /// macro sheets) are not among them.
    pub fn sheets(&self) -> &[SheetInfo] {
        &self.sheets
    }

    /// The shared strings' bytes, which every batch of every sheet indexes — what the
    /// Python and PHP bindings share among them rather than copying into each.
    #[cfg(any(feature = "python", feature = "php"))]
    pub(crate) fn strings(&self) -> &[u8] {
        &self.strings
    }

    /// Starts a read of one sheet — by index or by name — through `plan`.
    pub fn sheet<'n>(
        &self,
        which: impl Into<SheetRef<'n>>,
        options: SheetOptions,
        plan: &[Column],
    ) -> Result<Sheet<'_>, Error> {
        Sheet::open(Book::Borrowed(self), which.into(), options, Some(plan))
    }

    /// Starts a read of one sheet and reads its header, leaving the plan to be bound
    /// ([`Sheet::bind`]) once the header has said where each column is.
    pub fn sheet_unbound<'n>(
        &self,
        which: impl Into<SheetRef<'n>>,
        options: SheetOptions,
    ) -> Result<Sheet<'_>, Error> {
        Sheet::open(Book::Borrowed(self), which.into(), options, None)
    }

    /// The sheet `which` of a workbook shared behind an `Arc`, read by a sheet that holds a
    /// clone of it — which is what lets the Python and PHP bindings keep a sheet in an object of its
    /// own, beside the workbook rather than borrowing from it.
    #[cfg(any(feature = "python", feature = "php"))]
    pub(crate) fn shared_sheet(
        book: &std::sync::Arc<Workbook<'static>>,
        which: SheetRef<'_>,
        options: SheetOptions,
        plan: Option<&[Column]>,
    ) -> Result<Sheet<'static>, Error> {
        Sheet::open(
            Book::Shared(std::sync::Arc::clone(book)),
            which,
            options,
            plan,
        )
    }
}

/// The workbook a sheet reads from: borrowed, or — for the Python and PHP bindings — shared.
enum Book<'w> {
    Borrowed(&'w Workbook<'w>),
    #[cfg(any(feature = "python", feature = "php"))]
    Shared(std::sync::Arc<Workbook<'static>>),
}

impl<'w> Book<'w> {
    fn get(&self) -> &Workbook<'w> {
        match self {
            Book::Borrowed(book) => book,
            #[cfg(any(feature = "python", feature = "php"))]
            Book::Shared(book) => book,
        }
    }
}

impl<'w> Sheet<'w> {
    fn open(
        book: Book<'w>,
        which: SheetRef<'_>,
        options: SheetOptions,
        plan: Option<&[Column]>,
    ) -> Result<Sheet<'w>, Error> {
        // A plan that can't be honoured says so at `bind`, so this judges the sizes without it.
        options.checked(
            &plan
                .and_then(|plan| column::specs_with(plan).ok())
                .unwrap_or_default(),
        )?;
        let workbook = book.get();
        let source = match which {
            SheetRef::Index(index) => (index < workbook.sheets.len())
                .then_some(index)
                .ok_or_else(|| Error::NoSheet(format!("at index {index}")))?,
            SheetRef::Name(name) => workbook
                .sheets
                .iter()
                .position(|sheet| sheet.name == name)
                .ok_or_else(|| Error::NoSheet(format!("named {name:?}")))?,
        };
        let info = &workbook.sheets[source];
        let (part, index) = (info.part.clone(), info.index);
        let state = copy_of(&workbook.state);
        let mut sheet = Sheet {
            book,
            source,
            options,
            state,
            scratch: Scratch {
                window: Vec::new(),
                arena: vec![0; 4096],
                cells: vec![Span::default(); 64],
            },
            row: Vec::new(),
            per_row: 0,
            columns: None,
            header: None,
            pending: None,
            error: None,
        };
        if let Some(plan) = plan {
            sheet.bind(plan)?;
        }
        let mut out = Filled::default();
        let code = rows::sheet_with(
            &mut sheet.state,
            &sheet.book.get().container,
            &part,
            index,
            &options.raw(),
            &mut out,
        );
        settle(code, &out)?;
        if options.has_header {
            sheet.read_header()?;
        }
        Ok(sheet)
    }
}

/// A forward-only read of one sheet. See the module documentation.
pub struct Sheet<'w> {
    book: Book<'w>,
    /// The sheet's place in the workbook's list.
    source: usize,
    options: SheetOptions,
    state: Box<State>,
    scratch: Scratch,
    /// The row the core is assembling: a slot for every source column the plan reaches.
    row: Vec<Slot>,
    /// Cell-table entries to a row: one per plan column, and one more.
    per_row: usize,
    /// The plan's columns, once one is bound.
    columns: Option<Columns>,
    header: Option<Header>,
    /// A failure met with rows before it: those went out first, and this is next.
    pending: Option<Error>,
    error: Option<Error>,
}

/// The workbook's tables, as a read of one of its sheets is handed them.
fn tables<'b>(book: &'b Workbook<'_>) -> Tables<'b> {
    Tables {
        strings: &book.strings,
        table: &book.table,
        kinds: &book.kinds,
    }
}

impl SampleSource for Sheet<'_> {
    fn next_batch(&mut self) -> Result<Option<Batch<'_>>, Error> {
        self.read()
    }
}

impl<'w> Sheet<'w> {
    fn read_header(&mut self) -> Result<(), Error> {
        let book = self.book.get();
        let tables = tables(book);
        let state = &mut *self.state;
        // The header row's cells are kept in the row's slots as well as named — the slots
        // are what a row the sheet repeats (an ODS `number-rows-repeated`) is delivered
        // again from. A plan says how many slots it reads; without one, there is a slot for
        // every name, and binding a plan later keeps them.
        let unbound = self.columns.is_none();
        if unbound && self.row.len() < self.scratch.cells.len() {
            self.row.resize(self.scratch.cells.len(), Slot::default());
        }
        let (code, out) = self
            .scratch
            .drive(&mut self.row, unbound, tables, |memory, out| {
                rows::header(state, &book.container, memory, out)
            });
        settle(code, &out)?;
        let mut header = Header::new();
        for name in &self.scratch.cells[..out.rows as usize] {
            let from: &[u8] = if name.flagged() {
                &self.scratch.arena
            } else {
                &book.strings
            };
            let start = name.offset as usize;
            header.push(&from[start..start + name.len()]);
        }
        self.header = Some(header);
        Ok(())
    }

    /// Reads a sample of this sheet through the text door and says what its columns look like: a
    /// plan to look at, change and pass to a read, and a report.
    ///
    /// It's its own read of the sheet from its first row, under the sheet's options, so it leaves
    /// this one where it is and with the plan it has, and answers the same bound, unbound or part
    /// of the way through. See [`DelimitedReader::suggest`](crate::DelimitedReader::suggest),
    /// though the most rows a sample holds here is this sheet's batch size.
    ///
    /// A typed number is judged by its value and has no notation, whichever is declared: a column
    /// of them is a number under any format, and its plan reads every one, since a typed cell reads
    /// the same under them all. A text cell holding `1.234` keeps its question, in a column of its
    /// own or beside typed numbers. A boolean is judged by `true` or `false`. A date cell says
    /// `2023-03-15T00:00:00`, with a time on it even at midnight, which has no zone and so isn't a
    /// timestamp, and a time or a duration isn't a door this tries: all of those suggest text,
    /// which reads what the sheet says. So a column of workbook dates is a text column here: a
    /// plan that wants dates names the door itself.
    ///
    /// The width is the header's, or the first row's, so a row wider than that isn't sampled past
    /// it.
    ///
    /// # Errors
    /// [`Error::Plan`] first, before anything is read, for any format declared in `options` the
    /// core would refuse, and [`Error::Structure`] for a failure among the sampled rows.
    pub fn suggest(&self, options: &SuggestOptions) -> Result<Suggestion, Error> {
        options.check_formats()?;
        let book = self.book.get();
        let open = |read: SheetOptions| {
            Sheet::open(
                Book::Borrowed(book),
                SheetRef::Index(self.source),
                read,
                None,
            )
        };
        let limit = options.rows_within(self.options.batch_rows);
        // Notes, for what the sample's second door says of a cell, in `Sample::take`.
        let read = self.options.with_notes(true).with_batch_rows(1);
        // A headerless sheet's width is its first row's, which a read says through a plan of one
        // column, whose cells are verdicts as the sample's will be.
        let width = if read.has_header {
            None
        } else {
            let mut first = open(read)?;
            first.bind(&[Column::text(0)])?;
            let mut widths = [0];
            first.read_widths(&mut widths)?;
            Some(widths[0] as usize)
        };
        let mut sheet = open(read)?;
        let width = width.unwrap_or_else(|| sheet.header().map_or(0, Header::len));
        check_width(width)?;
        if width != 0 {
            let plan = Sample::sheet_plan(width);
            // One row more than the sample, so that a row past it costs no second batch to find, in
            // as many batches as the core says that takes.
            sheet.options.batch_rows =
                column::sample_rows(&column::specs_with(&plan)?, limit + 1, read.size_flags())?;
            sheet.bind(&plan)?;
        }
        let mut sample = Sample::of_sheet(width, limit);
        if width != 0 {
            sample.read_from(&mut sheet)?;
        }
        sample.judge(sheet.header(), options)
    }

    /// The options the sheet is read with.
    pub fn options(&self) -> SheetOptions {
        self.options
    }

    /// Declares the plan a sheet opened without one reads through — once, before the
    /// first read: [`DelimitedReader::bind`](crate::DelimitedReader::bind), for a sheet.
    ///
    /// # Errors
    /// [`Error::AlreadyBound`] if the sheet has a plan; [`Error::Plan`] if a column cannot
    /// be honoured.
    pub fn bind(&mut self, plan: &[Column]) -> Result<(), Error> {
        if self.columns.is_some() {
            return Err(Error::AlreadyBound);
        }
        let (_, width, _) = column::specs(plan)?;
        let specs = column::specs_with(plan)?;
        self.options.checked(&specs)?;
        let batch_rows = self.options.batch_rows;
        let per_row = plan.len() + 1;
        let cells = batch_rows * per_row;
        if self.scratch.cells.len() < cells {
            // Nothing in the table is kept: it's only ever written before it's read.
            self.scratch.cells = column::zeroed(cells).ok_or_else(|| Error::NoMemory {
                bytes: (cells as u64).saturating_mul(size_of::<Span>() as u64),
            })?;
        }
        // What the slots hold — a header row still to be repeated — is kept.
        if self.row.len() < width {
            let more = width - self.row.len();
            self.row
                .try_reserve_exact(more)
                .map_err(|_| Error::NoMemory {
                    bytes: (more as u64).saturating_mul(size_of::<Slot>() as u64),
                })?;
            self.row.resize(width, Slot::default());
        }
        self.per_row = per_row;
        self.columns = Some(Columns::with_options(
            plan,
            specs,
            batch_rows,
            self.options.notes,
            false,
        )?);
        Ok(())
    }

    /// Whether a plan has been bound: always, for a sheet opened with one.
    pub fn is_bound(&self) -> bool {
        self.columns.is_some()
    }

    /// The plan the sheet is read through: empty until one is bound.
    pub fn plan(&self) -> &[Column] {
        self.columns.as_ref().map_or(&[], Columns::plan)
    }

    /// Reads every row left, batch by batch, handing each to `f`:
    /// [`DelimitedReader::for_each_row`](crate::DelimitedReader::for_each_row), for a sheet.
    pub fn for_each_row<E: From<Error>>(
        &mut self,
        mut f: impl FnMut(Row<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        while let Some(batch) = self.read()? {
            for row in batch {
                f(row)?;
            }
        }
        Ok(())
    }

    /// The header row's names — a typed cell said the way the text door says it — or
    /// `None` if the sheet was opened without a header. A header with no names is a sheet
    /// with no rows.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// Reads the next batch: up to the sheet's batch size of rows, or `None` when the
    /// sheet has no more. The batch borrows the sheet, and is over when the next is asked
    /// for.
    ///
    /// A structural failure is returned once every intact row before it has been
    /// delivered, and again on every call after. A sheet opened without a plan returns
    /// [`Error::Unbound`] until one is bound.
    pub fn read(&mut self) -> Result<Option<Batch<'_>>, Error> {
        self.read_widths(&mut [])
    }

    /// [`Sheet::read`], with `widths`, unless it's empty, getting the cells each row had.
    fn read_widths(&mut self, widths: &mut [u32]) -> Result<Option<Batch<'_>>, Error> {
        let Some(columns) = self.columns.as_mut() else {
            return Err(Error::Unbound);
        };
        if let Some(error) = self.pending.take() {
            self.error = Some(error);
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let book = self.book.get();
        let tables = tables(book);
        let max_rows = columns.batch_rows();
        let state = &mut *self.state;
        let tokens = self.options.tokens;
        let (specs, buffers, _) = columns.for_core_with();
        let (code, out) = self
            .scratch
            .drive(&mut self.row, false, tables, |memory, out| {
                // SAFETY: every buffer has room for `max_rows` values of its column's door
                // and as many verdicts and notes, which is what `Columns` allocated; the specs'
                // lists are interned.
                unsafe {
                    rows::fill_with(
                        state,
                        &book.container,
                        specs,
                        buffers,
                        max_rows,
                        memory,
                        tokens,
                        widths,
                        out,
                    )
                }
            });
        if let Err(error) = settle(code, &out) {
            if out.rows == 0 {
                self.error = Some(error.clone());
                return Err(error);
            }
            self.pending = Some(error);
        }
        if out.rows == 0 {
            return Ok(None);
        }
        Ok(Some(Batch::new(
            columns,
            out.rows as usize,
            &self.scratch.cells,
            self.per_row,
            &book.strings,
            &self.scratch.arena,
            Home::Workbook {
                first_column: self
                    .options
                    .range
                    .map_or(0, |range| range.first_column().saturating_sub(1)),
            },
        )))
    }
}
