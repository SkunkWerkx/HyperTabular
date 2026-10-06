//! Workbooks — XLSX and ODS — read through a plan into the batches delimited text is
//! read into.
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

use crate::batch::{Batch, Columns, Origin};
use crate::column;
use crate::kernel::abi::{
    ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, ERR_WINDOW, Filled, OK, Opened, Slot, Span,
};
use crate::kernel::workbook::part::WINDOW_MIN;
use crate::kernel::workbook::{FORMAT_XLSX, Memory, State, book, rows};
use crate::{Column, Error, Header};
use hypercast::ExcelEpoch;
use std::borrow::Cow;
use std::path::Path;

/// Which kind of workbook a container holds — told by what is in it, never by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Office Open XML (`.xlsx`).
    Xlsx,
    /// OpenDocument (`.ods`).
    Ods,
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

/// How a sheet is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetOptions {
    /// Whether the first row delivered is the header rather than data.
    pub has_header: bool,
    /// Whether rows with no cell in them — and the rows a gap in the row numbers stands
    /// for — are left out rather than delivered as rows of empty cells.
    pub skip_empty_rows: bool,
    /// The most rows a batch holds (at least one).
    pub batch_rows: usize,
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

    /// The same options with another batch size.
    pub const fn with_batch_rows(self, rows: usize) -> SheetOptions {
        SheetOptions {
            batch_rows: if rows == 0 { 1 } else { rows },
            ..self
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
    fn drive(
        &mut self,
        row: &mut [Slot],
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
                ERR_CELLS => self
                    .cells
                    .resize(needed.max(self.cells.len() + 1), Span::default()),
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
        let format = if opened.format == FORMAT_XLSX {
            Format::Xlsx
        } else {
            Format::Ods
        };
        let epoch = ExcelEpoch::from_code(opened.epoch).unwrap_or(ExcelEpoch::Y1900);

        let none = Tables::default();
        let (code, out) = scratch.drive(&mut [], none, |memory, out| {
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
        let (code, out) = scratch.drive(&mut [], none, |memory, out| {
            book::strings(&mut state, bytes, memory, out)
        });
        settle(code, &out)?;
        let strings = scratch.arena[..out.arena_used as usize].to_vec();
        let table = scratch.cells[..out.rows as usize].to_vec();

        let (code, out) = scratch.drive(&mut [], none, |memory, out| {
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
    /// Python binding shares among them rather than copying into each.
    #[cfg(feature = "python")]
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
        Sheet::open(Book::Borrowed(self), which.into(), options, plan)
    }

    /// The sheet `which` of a workbook shared behind an `Arc`, read by a sheet that holds a
    /// clone of it — which is what lets the Python binding keep a sheet in an object of its
    /// own, beside the workbook rather than borrowing from it.
    #[cfg(feature = "python")]
    pub(crate) fn shared_sheet(
        book: &std::sync::Arc<Workbook<'static>>,
        which: SheetRef<'_>,
        options: SheetOptions,
        plan: &[Column],
    ) -> Result<Sheet<'static>, Error> {
        Sheet::open(
            Book::Shared(std::sync::Arc::clone(book)),
            which,
            options,
            plan,
        )
    }
}

/// The workbook a sheet reads from: borrowed, or — for the Python binding — shared.
enum Book<'w> {
    Borrowed(&'w Workbook<'w>),
    #[cfg(feature = "python")]
    Shared(std::sync::Arc<Workbook<'static>>),
}

impl<'w> Book<'w> {
    fn get(&self) -> &Workbook<'w> {
        match self {
            Book::Borrowed(book) => book,
            #[cfg(feature = "python")]
            Book::Shared(book) => book,
        }
    }
}

impl<'w> Sheet<'w> {
    fn open(
        book: Book<'w>,
        which: SheetRef<'_>,
        options: SheetOptions,
        plan: &[Column],
    ) -> Result<Sheet<'w>, Error> {
        let workbook = book.get();
        let info = match which {
            SheetRef::Index(index) => workbook
                .sheets
                .get(index)
                .ok_or_else(|| Error::NoSheet(format!("at index {index}")))?,
            SheetRef::Name(name) => workbook
                .sheets
                .iter()
                .find(|sheet| sheet.name == name)
                .ok_or_else(|| Error::NoSheet(format!("named {name:?}")))?,
        };
        let (part, index) = (info.part.clone(), info.index);
        let (specs, width) = column::specs(plan)?;
        let batch_rows = options.batch_rows.max(1);
        let state = copy_of(&workbook.state);
        let mut sheet = Sheet {
            book,
            options,
            state,
            scratch: Scratch {
                window: Vec::new(),
                arena: vec![0; 4096],
                cells: vec![Span::default(); batch_rows * (plan.len() + 1)],
            },
            row: vec![Slot::default(); width],
            per_row: plan.len() + 1,
            columns: Columns::new(plan, specs, batch_rows),
            header: None,
            pending: None,
            error: None,
        };
        let mut out = Filled::default();
        let code = rows::sheet(
            &mut sheet.state,
            &sheet.book.get().container,
            &part,
            index,
            options.has_header,
            options.skip_empty_rows,
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
    options: SheetOptions,
    state: Box<State>,
    scratch: Scratch,
    /// The row the core is assembling: a slot for every source column the plan reaches.
    row: Vec<Slot>,
    /// Cell-table entries to a row: one per plan column, and one more.
    per_row: usize,
    columns: Columns,
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

impl<'w> Sheet<'w> {
    fn read_header(&mut self) -> Result<(), Error> {
        let book = self.book.get();
        let tables = tables(book);
        let state = &mut *self.state;
        let (code, out) = self.scratch.drive(&mut self.row, tables, |memory, out| {
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

    /// The options the sheet is read with.
    pub fn options(&self) -> SheetOptions {
        self.options
    }

    /// The plan the sheet is read through.
    pub fn plan(&self) -> &[Column] {
        self.columns.plan()
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
    /// delivered, and again on every call after.
    pub fn read(&mut self) -> Result<Option<Batch<'_>>, Error> {
        if let Some(error) = self.pending.take() {
            self.error = Some(error);
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let book = self.book.get();
        let tables = tables(book);
        let max_rows = self.columns.batch_rows();
        let state = &mut *self.state;
        let (specs, buffers) = self.columns.for_core();
        let (code, out) = self.scratch.drive(&mut self.row, tables, |memory, out| {
            // SAFETY: every buffer has room for `max_rows` values of its column's door
            // and as many verdicts, which is what `Columns` allocated.
            unsafe {
                rows::fill(
                    state,
                    &book.container,
                    specs,
                    buffers,
                    max_rows,
                    memory,
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
            &self.columns,
            out.rows as usize,
            &self.scratch.cells,
            self.per_row,
            &book.strings,
            &self.scratch.arena,
            Origin::Workbook,
        )))
    }
}
