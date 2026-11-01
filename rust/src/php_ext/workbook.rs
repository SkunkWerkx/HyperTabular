//! `Workbook` and `Sheet`: XLSX, ODS and Numbers workbooks, opened in memory, and one sheet of
//! one read a batch at a time into typed columns — the same `Batch` delimited text gives.

use std::rc::Rc;

use ext_php_rs::class::RegisteredClass;
use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, Zval};
use ext_php_rs::zend::ce;

use super::args::{ArrayArg, Mixed, OptArray, SheetOptionsArg, Str};
use super::batch::{
    Array, Batch, BatchCore, IteratorObject, MaybeArray, MaybeBatch, RowIterator, this_zval,
    typed_return,
};
use super::classes::{bytes_zval, list_zval};
use super::enums::WorkbookFormat;
use super::errors::{contract_violation, object, out_of_bounds, out_of_range, runtime, type_error};
use super::guard;
use super::header::{Header, MaybeHeader};
use super::hypercast::excel_epoch;
use super::plan::Plan;
use super::streams;
use super::tabular::Scratch as Hook;
use super::value_name;
use super::values::{sheet_info, sheet_options, tabular_exception};
use crate::kernel::abi::{
    ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, ERR_WINDOW, Filled, OK, Opened, Slot, Span,
};
use crate::kernel::workbook::{Memory, State as Book, book, rows};

/// The smallest window the core works in.
const WINDOW_MIN: usize = 64 * 1024;
/// The most a workbook's shared strings are given up front.
const STRINGS_BOUND: u64 = 1 << 28;

typed_return!(
    /// `\HyperCast\ExcelEpoch`.
    EpochObject,
    "HyperCast\\ExcelEpoch",
    false
);
typed_return!(
    /// `SheetOptions`.
    SheetOptionsObject,
    "HyperTabular\\SheetOptions",
    false
);

/// Whether the tests asked every buffer to start with room for one element
/// (`Scratch::$stingy`).
fn stingy() -> bool {
    Hook::get_metadata()
        .ce()
        .get_static_property::<bool>("stingy")
        .unwrap_or(false)
}

/// Counts one growth of buffer `which` (window, arena, cells) in `Scratch::$grown`.
fn grown(which: usize) {
    let class = Hook::get_metadata().ce();
    let mut counts = class
        .get_static_property::<Vec<i64>>("grown")
        .unwrap_or_default();
    if counts.len() < 3 {
        counts.resize(3, 0);
    }
    counts[which] += 1;
    let _ = class.set_static_property("grown", counts);
}

/// The workbook's tables as a sheet's calls are handed them.
#[derive(Clone, Copy)]
struct Tables<'a> {
    strings: &'a [u8],
    table: &'a [Span],
    kinds: &'a [u8],
}

const NO_TABLES: Tables<'static> = Tables {
    strings: &[],
    table: &[],
    kinds: &[],
};

/// The buffers a workbook call may ask to have grown.
struct Scratch {
    window: Vec<u8>,
    arena: Vec<u8>,
    cells: Vec<Span>,
    row: Vec<Slot>,
}

impl Scratch {
    fn new(window: usize, arena: usize, cells: usize, row: usize) -> Scratch {
        Scratch {
            window: vec![0; window],
            arena: vec![0; arena],
            cells: vec![Span::default(); cells],
            row: vec![Slot::default(); row],
        }
    }

    /// The buffer at least `needed` long and longer than it was, what it held kept.
    fn grow<T: Copy + Default>(buffer: &mut Vec<T>, needed: u64) {
        let length = usize::try_from(needed)
            .unwrap_or(usize::MAX)
            .max(buffer.len() + 1);
        buffer.resize(length, T::default());
    }

    /// The buffer at least `needed` long if it is shorter, what it held kept.
    fn reserve<T: Copy + Default>(buffer: &mut Vec<T>, needed: usize) {
        if buffer.len() < needed {
            buffer.resize(needed, T::default());
        }
    }

    fn memory<'a>(&'a mut self, tables: Tables<'a>) -> Memory<'a> {
        Memory {
            window: &mut self.window,
            arena: &mut self.arena,
            cells: &mut self.cells,
            row: &mut self.row,
            strings: tables.strings,
            table: tables.table,
            kinds: tables.kinds,
        }
    }

    /// Makes `call` until it stops asking for room, growing the buffer it names each time.
    /// With `row_follows_cells`, the row's slots are grown with the cell table.
    fn drive(
        &mut self,
        tables: Tables<'_>,
        row_follows_cells: bool,
        out: &mut Filled,
        mut call: impl FnMut(&mut Memory<'_>, &mut Filled) -> i32,
    ) -> i32 {
        loop {
            let code = call(&mut self.memory(tables), out);
            match code {
                ERR_WINDOW => {
                    Scratch::grow(&mut self.window, out.needed);
                    grown(0);
                }
                ERR_ARENA => {
                    Scratch::grow(&mut self.arena, out.needed);
                    grown(1);
                }
                ERR_CELLS => {
                    Scratch::grow(&mut self.cells, out.needed);
                    if row_follows_cells {
                        Scratch::reserve(&mut self.row, self.cells.len());
                    }
                    grown(2);
                }
                _ => return code,
            }
        }
    }
}

/// What a call's code means: nothing more to do, or a structural failure. Anything else is
/// this binding's bug.
fn settle(code: i32, out: &Filled) -> PhpResult<()> {
    match code {
        OK => Ok(()),
        ERR_STRUCTURE => Err(object(&tabular_exception(&out.failure)?)),
        _ => Err(contract_violation()),
    }
}

/// One sheet as the workbook listed it.
struct Listed {
    name: Vec<u8>,
    part: Vec<u8>,
    index: u32,
    info: Zval,
}

/// A workbook opened: nothing in it changes once it is, and every sheet read from it shares it.
struct BookData {
    container: Vec<u8>,
    state: Box<Book>,
    format: u32,
    epoch: u32,
    sheets: Vec<Listed>,
    /// `sheets()`, built once.
    list: Zval,
    strings: Rc<Vec<u8>>,
    table: Vec<Span>,
    kinds: Vec<u8>,
}

impl BookData {
    fn tables(&self) -> Tables<'_> {
        Tables {
            strings: &self.strings,
            table: &self.table,
            kinds: &self.kinds,
        }
    }

    fn open(container: Vec<u8>) -> PhpResult<BookData> {
        let stingy = stingy();
        let mut scratch = if stingy {
            Scratch::new(1, 1, 1, 0)
        } else {
            Scratch::new(WINDOW_MIN, 1024, 64, 0)
        };
        let mut state = Box::new(Book::new());
        // Opening starts over when it is refused, and reports through its own shape.
        let mut found = Opened::default();
        loop {
            match book::open(
                &mut state,
                &container,
                &mut scratch.memory(NO_TABLES),
                &mut found,
            ) {
                OK => break,
                ERR_WINDOW => Scratch::grow(&mut scratch.window, found.needed),
                ERR_ARENA => Scratch::grow(&mut scratch.arena, found.needed),
                ERR_STRUCTURE => return Err(object(&tabular_exception(&found.failure)?)),
                _ => return Err(contract_violation()),
            }
        }

        // The sheets: three spans each — the name and the part, in the arena, then one whose
        // offset's low bit says hidden and whose length is the sheet's index.
        let mut out = Filled::default();
        let code = scratch.drive(NO_TABLES, false, &mut out, |memory, out| {
            book::sheets(&mut state, &container, memory, out)
        });
        settle(code, &out)?;
        let listed = (out.rows as usize * 3).min(scratch.cells.len());
        let slice = |span: Span| {
            let offset = (span.offset as usize).min(scratch.arena.len());
            let end = offset.saturating_add(span.len()).min(scratch.arena.len());
            scratch.arena[offset..end].to_vec()
        };
        let mut sheets = Vec::with_capacity(listed / 3);
        for entry in scratch.cells[..listed].as_chunks::<3>().0 {
            let (name, part, index) = (slice(entry[0]), slice(entry[1]), entry[2].len);
            let info = sheet_info(&name, entry[2].offset & 1 == 1, &part, index)?;
            sheets.push(Listed {
                name,
                part,
                index,
                info,
            });
        }
        let list = list_zval(
            sheets
                .iter()
                .map(|sheet| sheet.info.shallow_clone())
                .collect(),
        )?;

        // The shared strings take no more room than their part inflates to; asking for it
        // once saves growing into it.
        let bound = found.strings_bytes.min(STRINGS_BOUND);
        if !stingy && (scratch.arena.len() as u64) < bound {
            Scratch::grow(&mut scratch.arena, bound);
        }
        let code = scratch.drive(NO_TABLES, false, &mut out, |memory, out| {
            book::strings(&mut state, &container, memory, out)
        });
        settle(code, &out)?;
        let used = (out.arena_used as usize).min(scratch.arena.len());
        let strings = Rc::new(scratch.arena[..used].to_vec());
        let table = scratch.cells[..(out.rows as usize).min(scratch.cells.len())].to_vec();
        let code = scratch.drive(NO_TABLES, false, &mut out, |memory, out| {
            book::styles(&mut state, &container, memory, out)
        });
        settle(code, &out)?;
        let kinds = scratch.arena[..(out.rows as usize).min(scratch.arena.len())].to_vec();

        Ok(BookData {
            container,
            state,
            format: found.format,
            epoch: found.epoch,
            sheets,
            list,
            strings,
            table,
            kinds,
        })
    }
}

/// An XLSX, ODS or Numbers workbook, opened in memory: its format, its date system, its sheets.
/// A sheet is read through a plan, as delimited text is, into the same `Batch`.
///
/// ```php
/// $book = Workbook::open('data.xlsx');
/// $sheet = $book->sheet('Data', new SheetOptions());
/// $header = $sheet->headerIndex();
/// $sheet->bind([Column::decimal($header->ordinal('amount'))]);
/// while (($batch = $sheet->read()) !== null) {
///     $amounts = $batch->values(0);
/// }
/// ```
///
/// Nothing is sniffed: a cell's text is cast through the column's door, as delimited text is,
/// and a typed cell (a number, a date) is judged the same way. Opening a workbook that is not
/// one — not a zip, encrypted, broken — is a `TabularException`.
#[php_class]
#[php(name = "HyperTabular\\Workbook")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
pub struct Workbook {
    data: Rc<BookData>,
}

impl Workbook {
    fn of(bytes: Vec<u8>) -> PhpResult<Workbook> {
        Ok(Workbook {
            data: Rc::new(BookData::open(bytes)?),
        })
    }

    fn sheet_at(&self, at: usize, options: &Zval, plan: Option<Zval>) -> PhpResult<Sheet> {
        Sheet::new(&self.data, at, options, plan)
    }
}

#[php_impl]
impl Workbook {
    /// Made by `fromString()`, `fromStream()` or `open()`; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Workbook is made by its factories".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// A workbook from its bytes.
    ///
    /// @throws TabularException when the bytes are not a workbook this library reads.
    pub fn from_string(bytes: Str) -> PhpResult<Workbook> {
        guard(|| Workbook::of(bytes.0))
    }

    /// A workbook read whole from an open stream, which stays the caller's.
    ///
    /// @throws \InvalidArgumentException for anything but an open stream.
    /// @throws \RuntimeException when reading the stream fails or stops short.
    /// @throws TabularException when the bytes are not a workbook this library reads.
    pub fn from_stream(stream: Mixed) -> PhpResult<Workbook> {
        guard(|| {
            streams::check_stream(&stream.0)?;
            let bytes = streams::rest(&stream.0)?
                .ok_or_else(|| runtime("hypertabular: reading the stream failed"))?;
            if !streams::at_end(&stream.0)? {
                return Err(runtime(
                    "hypertabular: the stream stopped before its end (a non-blocking stream, or a read that \
                     timed out); the workbook needs one that blocks",
                ));
            }
            Workbook::of(bytes)
        })
    }

    /// The workbook in the file at `$path`, read whole with `file_get_contents()`.
    ///
    /// @throws \RuntimeException when the file cannot be read.
    /// @throws TabularException when the bytes are not a workbook this library reads.
    pub fn open(path: Str) -> PhpResult<Workbook> {
        guard(|| Workbook::of(streams::contents(&path.0)?))
    }

    /// The workbook's format.
    pub fn format(&self) -> PhpResult<WorkbookFormat> {
        guard(|| WorkbookFormat::from_code(self.data.format).ok_or_else(contract_violation))
    }

    /// The workbook's date system, as HyperCast's `ExcelEpoch`: what `Column::excelSerial()`
    /// takes to read its date serials.
    pub fn date_system(&self) -> PhpResult<EpochObject> {
        guard(|| excel_epoch(self.data.epoch).map(|epoch| EpochObject(Some(epoch))))
    }

    /// The workbook's sheets, in its own order, hidden ones included.
    pub fn sheets(&self) -> PhpResult<Array> {
        guard(|| Ok(Array(self.data.list.shallow_clone())))
    }

    /// One sheet, by its place in `sheets()` (an int) or by its name (a string, matched
    /// exactly), to read with `$options` and, if it is known, `$plan`.
    ///
    /// @throws \OutOfRangeException for a place past the last sheet.
    /// @throws \OutOfBoundsException for a name no sheet has.
    /// @throws TabularException when the sheet's part is broken.
    pub fn sheet(
        &self,
        which: Mixed,
        options: SheetOptionsArg,
        plan: OptArray,
    ) -> PhpResult<Sheet> {
        guard(|| {
            let which = &which.0;
            let count = self.data.sheets.len();
            if let Some(name) = which.zend_str() {
                let name = name.as_bytes();
                let at = self
                    .data
                    .sheets
                    .iter()
                    .position(|sheet| sheet.name == name)
                    .ok_or_else(|| {
                        out_of_bounds(format!(
                            "The workbook has no sheet named \"{}\"",
                            String::from_utf8_lossy(name)
                        ))
                    })?;
                return self.sheet_at(at, &options.0, plan.0);
            }
            // An int, or what PHP's weak mode turns into one for an `int|string` parameter.
            let index = if let Some(index) = which.long() {
                index
            } else if which.is_bool() {
                i64::from(which.is_true())
            } else if let Some(float) = which
                .double()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.2e18)
            {
                float as i64
            } else {
                return Err(type_error(format!(
                    "HyperTabular\\Workbook::sheet(): Argument #1 ($which) must be of type string|int, {} given",
                    value_name(which)
                )));
            };
            let at = usize::try_from(index)
                .ok()
                .filter(|at| *at < count)
                .ok_or_else(|| {
                    out_of_range(format!(
                        "The workbook has {count} sheets; there is no sheet {index}"
                    ))
                })?;
            self.sheet_at(at, &options.0, plan.0)
        })
    }
}

/// One sheet of a `Workbook`, read a batch at a time into typed columns — the same `Batch`
/// delimited text gives, its rows numbered by the sheet's own row numbers.
///
/// As a delimited reader's, the plan can wait for the header: `headerIndex()` looks each name
/// up, and `bind()` declares the plan before the first read. A broken part is a
/// `TabularException`, thrown once every intact row before it has been delivered, and thrown
/// again by every read after.
#[php_class]
#[php(name = "HyperTabular\\Sheet")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
pub struct Sheet {
    book: Rc<BookData>,
    options: Zval,
    batch_rows: usize,
    state: Box<Book>,
    plan: Option<Plan>,
    scratch: Scratch,
    per_row: usize,
    out: Filled,
    header: Option<Zval>,
    header_index: Option<Zval>,
    pending: Option<Zval>,
    failure: Option<Zval>,
}

impl Sheet {
    fn new(data: &Rc<BookData>, at: usize, options: &Zval, plan: Option<Zval>) -> PhpResult<Sheet> {
        let (has_header, skip_empty_rows, batch_rows) = sheet_options(options)?;
        let listed = data.sheets.get(at).ok_or_else(contract_violation)?;
        let mut state = Box::new(Book::new());
        // SAFETY: a state block is plain integers with no destructor, the two do not overlap,
        // and the core documents an opened block as copyable.
        unsafe { std::ptr::copy_nonoverlapping(&*data.state, &mut *state, 1) };
        // With a plan, binding it sizes the cell table and the row; without one, the header row
        // is read into a cell table of its own size and a row of at least as many slots, which a
        // plan bound later grows and never shrinks.
        let stingy = stingy();
        let scratch = match (plan.is_some(), stingy) {
            (false, true) => Scratch::new(1, 1, 1, 1),
            (false, false) => Scratch::new(0, 4096, 64, 64),
            (true, true) => Scratch::new(1, 1, 1, 0),
            (true, false) => Scratch::new(0, 4096, 0, 0),
        };
        let mut sheet = Sheet {
            book: Rc::clone(data),
            options: options.shallow_clone(),
            batch_rows,
            state,
            plan: None,
            scratch,
            per_row: 0,
            out: Filled::default(),
            header: None,
            header_index: None,
            pending: None,
            failure: None,
        };
        if let Some(plan) = plan {
            sheet.take(Plan::new(&plan, batch_rows)?);
        }
        let code = rows::sheet(
            &mut sheet.state,
            &data.container,
            &listed.part,
            listed.index,
            has_header,
            skip_empty_rows,
            &mut sheet.out,
        );
        settle(code, &sheet.out)?;
        if has_header {
            sheet.read_header()?;
        }
        Ok(sheet)
    }

    fn take(&mut self, plan: Plan) {
        self.per_row = plan.count() + 1;
        // The cell table takes a batch's rows up front — unless the tests asked every buffer to
        // start with no room, when the fill asks for it instead.
        if !stingy() {
            Scratch::reserve(&mut self.scratch.cells, self.batch_rows * self.per_row);
        }
        // A slot for every source column the plan reaches. What the slots already hold — a
        // header row the sheet may yet repeat — is kept.
        Scratch::reserve(&mut self.scratch.row, plan.width);
        self.plan = Some(plan);
    }

    /// The header row's names: in the shared strings as written, or — flagged — in the arena.
    fn read_header(&mut self) -> PhpResult<()> {
        // The header row's cells are kept in the row's slots as well as named — the slots are
        // what a row the sheet repeats (an ODS `number-rows-repeated`) is delivered again from.
        let unbound = self.plan.is_none();
        if unbound {
            Scratch::reserve(&mut self.scratch.row, self.scratch.cells.len());
        }
        let book = Rc::clone(&self.book);
        let state = &mut self.state;
        let code = self
            .scratch
            .drive(book.tables(), unbound, &mut self.out, |memory, out| {
                rows::header(state, &book.container, memory, out)
            });
        settle(code, &self.out)?;
        let count = (self.out.rows as usize).min(self.scratch.cells.len());
        let mut names = Vec::with_capacity(count);
        for span in &self.scratch.cells[..count] {
            let from: &[u8] = if span.flagged() {
                &self.scratch.arena
            } else {
                &book.strings
            };
            let offset = (span.offset as usize).min(from.len());
            let end = offset.saturating_add(span.len()).min(from.len());
            names.push(bytes_zval(&from[offset..end]));
        }
        self.header = Some(list_zval(names)?);
        Ok(())
    }

    fn next_batch(&mut self) -> PhpResult<Option<Zval>> {
        if self.plan.is_none() {
            return Err(super::errors::logic(
                "The sheet has no plan to read through; bind() one first",
            ));
        }
        if let Some(pending) = self.pending.take() {
            self.failure = Some(pending);
        }
        if let Some(failure) = &self.failure {
            return Err(object(failure));
        }
        let book = Rc::clone(&self.book);
        let plan = self.plan.as_mut().ok_or_else(contract_violation)?;
        let buffers = plan.buffers();
        let (state, specs, batch_rows) = (&mut self.state, &plan.specs, plan.batch_rows);
        let code = self
            .scratch
            .drive(book.tables(), false, &mut self.out, |memory, out| {
                // SAFETY: each column's two arrays have room for `batch_rows` values of its door's
                // type and `batch_rows` verdicts (`Plan::new`).
                unsafe {
                    rows::fill(
                        state,
                        &book.container,
                        specs,
                        &buffers,
                        batch_rows,
                        memory,
                        out,
                    )
                }
            });
        let rows = self.out.rows as usize;
        match code {
            OK => {}
            ERR_STRUCTURE => {
                let broken = tabular_exception(&self.out.failure)?;
                if rows == 0 {
                    let thrown = object(&broken);
                    self.failure = Some(broken);
                    return Err(thrown);
                }
                self.pending = Some(broken);
            }
            _ => return Err(contract_violation()),
        }
        if rows == 0 {
            return Ok(None);
        }
        // The arena as far as the core wrote into it, which is as far as any of the batch's
        // spans reach — the text typed cells were said as, and the raw text of those that
        // failed.
        let used = (self.out.arena_used as usize).min(self.scratch.arena.len());
        let cells =
            self.scratch.cells[..(rows * self.per_row).min(self.scratch.cells.len())].to_vec();
        let plan = self.plan.as_ref().ok_or_else(contract_violation)?;
        let core = BatchCore::new(
            plan,
            rows,
            cells,
            self.per_row,
            Rc::clone(&book.strings),
            self.scratch.arena[..used].to_vec(),
            true,
        );
        Batch::object(core).map(Some)
    }
}

#[php_impl]
impl Sheet {
    /// Made by `Workbook::sheet()`; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Sheet is made by Workbook::sheet()".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// The options the sheet is read with.
    pub fn options(&self) -> PhpResult<SheetOptionsObject> {
        guard(|| Ok(SheetOptionsObject(Some(self.options.shallow_clone()))))
    }

    /// Declares the plan, once, before the first read.
    ///
    /// @throws \LogicException when the sheet already has a plan.
    /// @throws \InvalidArgumentException for a plan entry that is not a `Column`.
    pub fn bind(&mut self, plan: ArrayArg) -> PhpResult<()> {
        guard(|| {
            if self.plan.is_some() {
                return Err(super::errors::logic(
                    "The sheet already has a plan; a plan is bound once",
                ));
            }
            let plan = Plan::new(&plan.0, self.batch_rows)?;
            self.take(plan);
            Ok(())
        })
    }

    /// Whether the sheet has a plan.
    pub fn is_bound(&self) -> PhpResult<bool> {
        guard(|| Ok(self.plan.is_some()))
    }

    /// The plan, as bound; an empty list before one is.
    pub fn plan(&self) -> PhpResult<Array> {
        guard(|| match &self.plan {
            Some(plan) => Ok(Array(plan.list.shallow_clone())),
            None => list_zval(Vec::new()).map(Array),
        })
    }

    /// The header row's names; null when the options say there is no header.
    pub fn header(&self) -> PhpResult<MaybeArray> {
        guard(|| Ok(MaybeArray(self.header.as_ref().map(Zval::shallow_clone))))
    }

    /// The header as a `Header`, to look columns up by name; null when there is none.
    pub fn header_index(&mut self) -> PhpResult<MaybeHeader> {
        guard(|| {
            let Some(names) = &self.header else {
                return Ok(MaybeHeader(None));
            };
            if self.header_index.is_none() {
                self.header_index = Some(Header::object(names)?);
            }
            Ok(MaybeHeader(
                self.header_index.as_ref().map(Zval::shallow_clone),
            ))
        })
    }

    /// Every row the sheet has left, a batch at a time, each a `Row` keyed by its place in the
    /// read: an iterator that reads as it goes, forward only.
    pub fn rows(self_: &mut ZendClassObject<Sheet>) -> PhpResult<IteratorObject> {
        guard(|| RowIterator::over_reader(this_zval(self_)).map(|it| IteratorObject(Some(it))))
    }

    /// The next batch of rows, or null at the end of the sheet.
    ///
    /// @throws \LogicException when the sheet has no plan.
    /// @throws TabularException at a broken part, once every intact row before it has been
    ///                          delivered — and again at every read after.
    pub fn read(&mut self) -> PhpResult<MaybeBatch> {
        guard(|| self.next_batch().map(MaybeBatch))
    }
}
