//! `Workbook` and `Sheet`: XLSX, ODS and Numbers workbooks, opened in memory, and one sheet of
//! one read a batch at a time into typed columns — the same `Batch` delimited text gives.

use std::cell::{Ref, RefCell, RefMut};
use std::sync::Arc;

use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, Zval};
use ext_php_rs::zend::ce;

use super::args::{ArrayArg, Mixed, OptArray, OptSuggestOptions, SheetOptionsArg, Str};
use super::batch::{
    Array, Batch, BatchCore, Holds, IteratorObject, MaybeArray, MaybeBatch, RowIterator, this_zval,
    typed_return,
};
use super::classes::{bytes_zval, list_zval};
use super::convert::{
    failure_zval, rust_error, sheet_options_of, suggest_options_of, suggestion_of,
};
use super::delimited::SuggestionObject;
use super::enums::WorkbookFormat;
use super::errors::{logic, object, out_of_bounds, out_of_range, runtime, type_error};
use super::guard;
use super::header::{Header, MaybeHeader};
use super::hypercast::excel_epoch;
use super::plan::Plan;
use super::streams;
use super::value_name;
use super::values::sheet_info;
use crate::{Error, SheetRef};

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
    book: Arc<crate::Workbook<'static>>,
    /// `sheets()`, built once.
    list: Zval,
}

impl Workbook {
    fn of(bytes: Vec<u8>) -> PhpResult<Workbook> {
        let book = crate::Workbook::from_vec(bytes).map_err(rust_error)?;
        let mut sheets = Vec::with_capacity(book.sheets().len());
        for sheet in book.sheets() {
            sheets.push(sheet_info(
                sheet.name.as_bytes(),
                sheet.hidden,
                sheet.part(),
                sheet.index(),
            )?);
        }
        Ok(Workbook {
            book: Arc::new(book),
            list: list_zval(sheets)?,
        })
    }

    fn open_sheet(
        &self,
        which: SheetRef<'_>,
        options: &Zval,
        plan: Option<Zval>,
        no_sheet: impl FnOnce() -> PhpException,
    ) -> PhpResult<Sheet> {
        let plan = plan.map(|plan| Plan::new(&plan)).transpose()?;
        let settings = sheet_options_of(options)?;
        let sheet = crate::Workbook::shared_sheet(
            &self.book,
            which,
            settings,
            plan.as_ref().map(|plan| plan.columns.as_slice()),
        )
        .map_err(|error| match error {
            Error::NoSheet(_) => no_sheet(),
            other => rust_error(other),
        })?;
        let header = sheet
            .header()
            .map(|header| list_zval(header.names().map(bytes_zval).collect()))
            .transpose()?;
        Ok(Sheet {
            state: RefCell::new(SheetState {
                sheet,
                book: Arc::clone(&self.book),
                plan,
                header,
                header_index: None,
                failure: None,
            }),
            options: options.shallow_clone(),
        })
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
        guard(|| {
            Ok(match self.book.format() {
                crate::Format::Xlsx => WorkbookFormat::Xlsx,
                crate::Format::Ods => WorkbookFormat::Ods,
                crate::Format::Numbers => WorkbookFormat::Numbers,
            })
        })
    }

    /// The workbook's date system, as HyperCast's `ExcelEpoch`: what `Column::excelSerial()`
    /// takes to read its date serials.
    pub fn date_system(&self) -> PhpResult<EpochObject> {
        guard(|| excel_epoch(self.book.date_system() as u32).map(|epoch| EpochObject(Some(epoch))))
    }

    /// The workbook's sheets, in its own order, hidden ones included.
    pub fn sheets(&self) -> PhpResult<Array> {
        guard(|| Ok(Array(self.list.shallow_clone())))
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
            if let Some(name) = which.zend_str() {
                let name = name.as_bytes();
                let shown = || {
                    out_of_bounds(format!(
                        "The workbook has no sheet named \"{}\"",
                        String::from_utf8_lossy(name)
                    ))
                };
                // A name that isn't UTF-8 can't be a sheet's, whose names are text.
                let text = std::str::from_utf8(name).map_err(|_| shown())?;
                return self.open_sheet(SheetRef::Name(text), &options.0, plan.0, shown);
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
            let count = self.book.sheets().len();
            let shown = || {
                out_of_range(format!(
                    "The workbook has {count} sheets; there is no sheet {index}"
                ))
            };
            let at = usize::try_from(index).map_err(|_| shown())?;
            self.open_sheet(SheetRef::Index(at), &options.0, plan.0, shown)
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
    /// Everything a call changes, borrowed for the call: a call back into the sheet from PHP that
    /// runs inside one of its own calls is refused, never a second hold on the Rust sheet.
    state: RefCell<SheetState>,
    options: Zval,
}

/// A sheet's state, behind its `RefCell`.
struct SheetState {
    /// The Rust sheet, holding a clone of `book`.
    sheet: crate::Sheet<'static>,
    book: Arc<crate::Workbook<'static>>,
    plan: Option<Plan>,
    header: Option<Zval>,
    header_index: Option<Zval>,
    failure: Option<Zval>,
}

/// What calling back into a sheet mid-call says.
fn mid_call() -> PhpException {
    logic("The sheet is in the middle of a call; nothing it runs can call back into it")
}

impl Sheet {
    /// The sheet's state for a call that changes it; refused while another call holds it.
    fn state(&self) -> PhpResult<RefMut<'_, SheetState>> {
        self.state.try_borrow_mut().map_err(|_| mid_call())
    }

    /// The sheet's state for a call that only looks; refused while a read holds it.
    fn view(&self) -> PhpResult<Ref<'_, SheetState>> {
        self.state.try_borrow().map_err(|_| mid_call())
    }
}

impl SheetState {
    fn next_batch(&mut self) -> PhpResult<Option<Zval>> {
        let Some(plan) = self.plan.as_ref() else {
            return Err(logic(
                "The sheet has no plan to read through; bind() one first",
            ));
        };
        if let Some(failure) = &self.failure {
            return Err(object(failure));
        }
        match self.sheet.read() {
            Ok(None) => Ok(None),
            Ok(Some(batch)) => {
                Batch::object(BatchCore::of(plan, &batch, Holds::Book(&self.book))).map(Some)
            }
            Err(Error::Structure(found)) => {
                let exception = failure_zval(found)?;
                let thrown = object(&exception);
                self.failure = Some(exception);
                Err(thrown)
            }
            Err(other) => Err(rust_error(other)),
        }
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
    pub fn bind(&self, plan: ArrayArg) -> PhpResult<()> {
        guard(|| {
            let mut state = self.state()?;
            if state.plan.is_some() {
                return Err(logic("The sheet already has a plan; a plan is bound once"));
            }
            let plan = Plan::new(&plan.0)?;
            state.sheet.bind(&plan.columns).map_err(rust_error)?;
            state.plan = Some(plan);
            Ok(())
        })
    }

    /// What a sample of the sheet says its columns are: a plan to change or to pass to a read, and
    /// why. It starts you off, and it's yours to inspect: nothing is applied, and a read never
    /// sniffs. The sample reads the sheet from its start, whatever this sheet has read, with notes
    /// on, and takes one batch of this sheet's size at most. A date column is text: nothing in a
    /// sheet's cells says which order a date string is in unless it's declared.
    ///
    /// @throws \InvalidArgumentException for a format the core refuses, or too many columns.
    /// @throws TabularException at a broken part among the sampled rows.
    pub fn suggest(&self, options: OptSuggestOptions) -> PhpResult<SuggestionObject> {
        guard(|| {
            let made = suggest_options_of(options.0.as_ref())?;
            let found = self.view()?.sheet.suggest(&made).map_err(rust_error)?;
            suggestion_of(&found, options.0.as_ref()).map(|made| SuggestionObject(Some(made)))
        })
    }

    /// Whether the sheet has a plan.
    pub fn is_bound(&self) -> PhpResult<bool> {
        guard(|| Ok(self.view()?.plan.is_some()))
    }

    /// The plan, as bound; an empty list before one is.
    pub fn plan(&self) -> PhpResult<Array> {
        guard(|| match &self.view()?.plan {
            Some(plan) => Ok(Array(plan.list.shallow_clone())),
            None => list_zval(Vec::new()).map(Array),
        })
    }

    /// The header row's names; null when the options say there is no header.
    pub fn header(&self) -> PhpResult<MaybeArray> {
        guard(|| {
            Ok(MaybeArray(
                self.view()?.header.as_ref().map(Zval::shallow_clone),
            ))
        })
    }

    /// The header as a `Header`, to look columns up by name; null when there is none.
    pub fn header_index(&self) -> PhpResult<MaybeHeader> {
        guard(|| {
            let mut state = self.state()?;
            let state = &mut *state;
            let Some(names) = &state.header else {
                return Ok(MaybeHeader(None));
            };
            if state.header_index.is_none() {
                state.header_index = Some(Header::object(names)?);
            }
            Ok(MaybeHeader(
                state.header_index.as_ref().map(Zval::shallow_clone),
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
    pub fn read(&self) -> PhpResult<MaybeBatch> {
        guard(|| self.state()?.next_batch().map(MaybeBatch))
    }
}
