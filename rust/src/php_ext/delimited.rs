//! `DelimitedReader`: delimited text read a batch at a time into typed columns.

// The factories' parameters are named as PHP names them, for named arguments: 1.0's
// `$batchRows` and `$bufferBytes`.
#![allow(non_snake_case)]

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::future::Future;
use std::io::Read;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, Zval};
use ext_php_rs::zend::{ExecutorGlobals, ce};

use super::args::{
    ArrayArg, BatchRows, BufferBytes, DialectArg, Mixed, Notes, OptArray, OptSuggestOptions, Str,
};
use super::batch::{
    Array, Batch, BatchCore, Holds, IteratorObject, MaybeArray, MaybeBatch, RowIterator, this_zval,
    typed_return,
};
use super::classes::{bytes_zval, list_zval};
use super::convert::{
    count, dialect_of, failure_zval, rust_error, suggest_options_of, suggestion_of,
};
use super::errors::{contract_violation, invalid_argument, logic, object, taken};
use super::guard;
use super::header::{Header, MaybeHeader};
use super::plan::Plan;
use super::streams;
use crate::{DelimitedOptions, Encoding, Error};

typed_return!(
    /// `Dialect`.
    DialectObject,
    "HyperTabular\\Dialect",
    false
);

typed_return!(
    /// `Suggestion`.
    SuggestionObject,
    "HyperTabular\\Suggestion",
    false
);

/// The failure a stream's read left for its caller, which `Poll::Pending` can't carry.
type Stash = Rc<RefCell<Option<PhpException>>>;

/// A PHP stream as the Rust reader's asynchronous source: every read one `fread()`, ready at once,
/// and a read that fails `Poll::Pending` with the failure stashed. That goes to the caller and isn't
/// final, as it was in 1.0: the Rust reader's cancel-safety keeps what it had read, and the next
/// `read()` goes on from there.
struct Incoming {
    stream: Zval,
    stash: Stash,
}

// SAFETY: the Rust reader asks for `Send` so that it can go to another thread, and this binding never
// sends it: a PHP object, and so the reader inside it, stays on the request's thread for its life, as
// do this zval and this `Rc`.
unsafe impl Send for Incoming {}

impl futures_io::AsyncRead for Incoming {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        into: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match streams::read_into(&this.stream, into) {
            Ok(read) => Poll::Ready(Ok(read)),
            Err(failure) => {
                *this.stash.borrow_mut() = Some(failure);
                Poll::Pending
            }
        }
    }
}

/// A PHP stream as a suggestion's blocking source. A sample that has what it wanted when a read
/// fails, `fread()` returning `false`, only says the source was truncated, so the failure is taken
/// off the engine, to go to the caller only if the suggestion fails on it. What the stream's own PHP
/// throws is the caller's whatever the sample has, and is rethrown.
struct Sampled {
    stream: Zval,
    stash: Stash,
    /// Whether the stream's PHP threw. Once set, a later failure doesn't clear it; the core makes no read after its first I/O error today, so that is defensive.
    thrown: Rc<Cell<bool>>,
}

// SAFETY: as for `Incoming`: it lives and dies inside one call on the request's thread.
unsafe impl Send for Sampled {}

impl Read for Sampled {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        streams::read_into(&self.stream, into).map_err(|failure| {
            // `call` fails with an exception pending only when PHP code threw; `false` from
            // `fread()` and an empty read before the end are made here, with nothing pending.
            if ExecutorGlobals::has_exception() {
                self.thrown.set(true);
                *self.stash.borrow_mut() = Some(taken(failure));
            } else if !self.thrown.get() {
                *self.stash.borrow_mut() = Some(failure);
            }
            std::io::Error::other("the stream failed")
        })
    }
}

/// What a failed poll stashed.
fn stashed(stash: &Stash) -> PhpException {
    stash.borrow_mut().take().unwrap_or_else(contract_violation)
}

/// One poll for a waker nothing wakes: `Pending` is only ever a read that failed, its failure
/// stashed. Dropping the read here loses nothing: it's what the Rust reader's cancel-safety promises.
fn poll_read<'r>(
    reader: &'r mut crate::DelimitedReader<'static>,
) -> Poll<Result<Option<crate::Batch<'r>>, Error>> {
    let mut context = Context::from_waker(Waker::noop());
    pin!(reader.read_async()).poll(&mut context)
}

/// What a reader is opened with, a negative count refused as no count at all. `0`, and a size past
/// what the core allows, the core refuses.
fn settings(batch_rows: i64, buffer_bytes: i64, notes: bool) -> PhpResult<DelimitedOptions> {
    let buffer = count(buffer_bytes, "The buffer must hold at least one byte")?;
    let rows = count(batch_rows, "A batch must hold at least one row")?;
    Ok(DelimitedOptions::new()
        .batch_rows(rows)
        .buffer_bytes(buffer)
        .notes(notes))
}

/// A plan the crate refused: past the header's width in this binding's words, or else the core's.
fn plan_refusal(error: Error, plan: &Plan, width: Option<usize>) -> PhpException {
    if let (Error::Plan { column }, Some(width)) = (&error, width)
        && let Some(ordinal) = plan
            .ordinals
            .get(*column)
            .filter(|ordinal| **ordinal >= width)
    {
        return invalid_argument(format!(
            "Plan column {column} reads source column {ordinal}, past the {width} columns the header gives every record."
        ));
    }
    rust_error(error)
}

/// The list of the header's names.
fn header_list(header: &crate::Header) -> PhpResult<Zval> {
    list_zval(header.names().map(bytes_zval).collect())
}

/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
/// typed columns, every cell a HyperCast `Success|Fault` verdict.
///
/// The Rust reader underneath owns every buffer: it casts each plan column in one loop, and `read()`
/// returns a copy of the rows it read as a `Batch`.
///
/// A value that does not cast is that cell's verdict, and the read goes on. Input that is not
/// rows of cells at all — a record of the wrong width, a quote never closed — is a
/// `TabularException`, thrown after every intact row before it has been delivered, and thrown
/// again by every read after.
///
/// HyperCast is the judge: a verdict is HyperCast's own `Success` or `Fault`, a fault's reason
/// its `CastFailure`, and a value the same PHP carrier HyperCast's `Cast` returns for that door
/// (`Door` lists them).
///
/// The plan can wait for the header. Opened without one, a reader reads the header and stops:
/// `headerIndex()` looks each name up, and `bind()` declares the plan the names resolved to,
/// before the first read.
///
/// ```php
/// $reader = DelimitedReader::open('data.csv', Dialect::csv());
/// $header = $reader->headerIndex();
/// $reader->bind([Column::i64($header->ordinal('id')), Column::text($header->ordinal('name'))]);
/// foreach ($reader->rows() as $row) {
///     $id = $row->get(0);                          // Success or Fault
/// }
/// ```
#[php_class]
#[php(name = "HyperTabular\\DelimitedReader")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
pub struct DelimitedReader {
    /// Everything a call changes, borrowed for the call. A stream's `fread()` runs the caller's PHP
    /// mid-read, and that PHP can call this reader back: the borrow refuses it, where two calls
    /// would otherwise share the Rust reader, or one would free it under the other.
    state: RefCell<ReaderState>,
    dialect: Zval,
}

/// A reader's state, behind its `RefCell`.
struct ReaderState {
    /// The Rust reader, gone once closed. Ahead of `input`, which it borrows: fields drop in order,
    /// and `shut` drops it first too.
    reader: Option<crate::DelimitedReader<'static>>,
    /// The string the reader reads, which keeps its slice alive.
    input: Option<Rc<Vec<u8>>>,
    /// Whether a batch can share `input`: read in place, as anything but a declared encoding is.
    shares: bool,
    /// The stream a `read()` draws on, closed with the reader when the reader opened it.
    stream: Option<(Stash, Zval, bool)>,
    plan: Option<Plan>,
    header: Option<Zval>,
    header_index: Option<Zval>,
    failure: Option<Zval>,
    /// What `records()` and `columnCount()` answer once the Rust reader is gone.
    closed: Option<(u64, Option<usize>)>,
}

/// What calling back into a reader mid-read says.
fn mid_read() -> PhpException {
    logic("The reader is in the middle of a read; its stream can't call back into it")
}

impl DelimitedReader {
    /// The reader's state for a call that changes it; refused while another call holds it.
    fn state(&self) -> PhpResult<RefMut<'_, ReaderState>> {
        self.state.try_borrow_mut().map_err(|_| mid_read())
    }

    /// The reader's state for a call that only looks; refused while a read holds it.
    fn view(&self) -> PhpResult<Ref<'_, ReaderState>> {
        self.state.try_borrow().map_err(|_| mid_read())
    }

    /// What the opened Rust reader, its header read, becomes: the plan bound, if one was handed over.
    fn opened(
        reader: crate::DelimitedReader<'static>,
        input: Option<Rc<Vec<u8>>>,
        stream: Option<(Stash, Zval, bool)>,
        dialect: Zval,
        encoding: Encoding,
        plan: Option<Plan>,
    ) -> PhpResult<Self> {
        let header = reader.header().map(header_list).transpose()?;
        let mut made = ReaderState {
            reader: Some(reader),
            shares: input.is_some() && encoding == Encoding::Utf8,
            input,
            stream,
            plan: None,
            header,
            header_index: None,
            failure: None,
            closed: None,
        };
        if let Some(plan) = plan {
            made.bind_plan(plan)?;
        }
        Ok(DelimitedReader {
            state: RefCell::new(made),
            dialect,
        })
    }
}

impl ReaderState {
    /// Binds a checked plan. One refused leaves the reader as it was: unbound.
    fn bind_plan(&mut self, plan: Plan) -> PhpResult<()> {
        let reader = self.reader.as_mut().ok_or_else(closed)?;
        let width = reader.column_count();
        reader
            .bind(&plan.columns)
            .map_err(|error| plan_refusal(error, &plan, width))?;
        self.plan = Some(plan);
        Ok(())
    }
}

impl DelimitedReader {
    /// The opening every factory but `fromString` shares, on a stream already checked. A stream
    /// failing during the header goes to the caller, and with it the reader, since there is none yet.
    fn over_stream(
        stream: Zval,
        owned: bool,
        dialect: Zval,
        checked: (crate::Dialect, DelimitedOptions, Option<Plan>),
    ) -> PhpResult<Self> {
        let (rust_dialect, options, plan) = checked;
        let stash: Stash = Rc::default();
        let incoming = Incoming {
            stream: stream.shallow_clone(),
            stash: Rc::clone(&stash),
        };
        let mut context = Context::from_waker(Waker::noop());
        let opening = pin!(options.from_async_reader(incoming, rust_dialect));
        let reader = match opening.poll(&mut context) {
            Poll::Ready(reader) => reader.map_err(rust_error)?,
            Poll::Pending => return Err(stashed(&stash)),
        };
        DelimitedReader::opened(
            reader,
            None,
            Some((stash, stream, owned)),
            dialect,
            rust_dialect.encoding,
            plan,
        )
    }
}

impl ReaderState {
    fn next_batch(&mut self) -> PhpResult<Option<Zval>> {
        let ReaderState {
            reader,
            input,
            shares,
            stream,
            plan,
            failure,
            ..
        } = self;
        let Some(reader) = reader.as_mut() else {
            return Err(closed());
        };
        let Some(plan) = plan.as_ref() else {
            return Err(logic(
                "The reader has no plan to read through; bind() one first",
            ));
        };
        if let Some(failure) = failure {
            return Err(object(failure));
        }
        let read = match stream {
            Some((stash, ..)) => match poll_read(reader) {
                Poll::Ready(read) => read,
                Poll::Pending => return Err(stashed(stash)),
            },
            None => reader.read(),
        };
        match read {
            Ok(None) => Ok(None),
            Ok(Some(batch)) => {
                let holds = match input {
                    Some(input) if *shares => Holds::Shared(input),
                    _ => Holds::Window,
                };
                Batch::object(BatchCore::of(plan, &batch, holds)).map(Some)
            }
            Err(Error::Structure(found)) => {
                let exception = failure_zval(found)?;
                let thrown = object(&exception);
                *failure = Some(exception);
                Err(thrown)
            }
            Err(other) => Err(rust_error(other)),
        }
    }

    fn shut(&mut self) -> PhpResult<()> {
        let Some(reader) = self.reader.take() else {
            return Ok(());
        };
        self.closed = Some((reader.records(), reader.column_count()));
        drop(reader);
        self.input = None;
        if let Some((_, stream, true)) = self.stream.take() {
            streams::close(&stream)?;
        }
        Ok(())
    }
}

/// What reading a closed reader says.
fn closed() -> PhpException {
    logic("The reader is closed")
}

/// A reader's arguments checked, as far as each can be without the source: the counts' types, the
/// plan's entries, the dialect, then what the core makes of the sizes. A bad size beats a missing
/// file, and nothing is allocated for one.
fn checked_arguments(
    dialect: &Zval,
    plan: Option<&Zval>,
    batch_rows: i64,
    buffer_bytes: i64,
    notes: bool,
) -> PhpResult<(crate::Dialect, DelimitedOptions, Option<Plan>)> {
    let options = settings(batch_rows, buffer_bytes, notes)?;
    let plan = plan.map(Plan::new).transpose()?;
    let rust_dialect = dialect_of(dialect)?;
    options
        .checked(
            rust_dialect,
            plan.as_ref()
                .map_or(&[][..], |plan| plan.columns.as_slice()),
        )
        .map_err(rust_error)?;
    Ok((rust_dialect, options, plan))
}

#[php_impl]
impl DelimitedReader {
    /// The longest a single record may be: 1 GiB. A record that outgrows it is a
    /// `TabularFailure::RowTooLong`.
    pub const MAX_ROW_BYTES: i64 = crate::DelimitedReader::MAX_ROW_BYTES as i64;
    /// The input buffer a reader starts with, in bytes; it doubles for a record that does not fit.
    pub const DEFAULT_BUFFER_BYTES: i64 = BufferBytes::DEFAULT;
    /// The rows a batch holds unless the caller says otherwise.
    pub const DEFAULT_BATCH_ROWS: i64 = BatchRows::DEFAULT;

    /// Made by `fromString()`, `fromStream()` or `open()`; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\DelimitedReader is made by its factories".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// A reader over `$utf8`, held in memory and read in place: `$bufferBytes` matters only under a
    /// declared encoding, which decodes ahead of the read.
    ///
    /// @throws \InvalidArgumentException for a batch or buffer of no rows or bytes or one past what
    ///                                   the core allows, a plan entry that is not a `Column`, a
    ///                                   separator the core refuses or a plan column past the
    ///                                   header's width.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(
        batchRows = BatchRows::DEFAULT,
        bufferBytes = BufferBytes::DEFAULT,
        notes = true
    ))]
    pub fn from_string(
        utf8: Str,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
        notes: Notes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            let (rust_dialect, options, plan) = checked_arguments(
                &dialect.0,
                plan.0.as_ref(),
                batchRows.0,
                bufferBytes.0,
                notes.0,
            )?;
            let input = Rc::new(utf8.0);
            let slice: &[u8] = &input;
            // SAFETY: `input` is held by the reader for as long as the Rust reader lives (it drops
            // first, in field order and in `shut`), and a `Vec` behind an `Rc` that nothing mutates
            // neither changes nor moves its bytes.
            let slice: &'static [u8] =
                unsafe { std::slice::from_raw_parts(slice.as_ptr(), slice.len()) };
            let reader = options
                .from_slice_unbound(slice, rust_dialect)
                .map_err(rust_error)?;
            DelimitedReader::opened(
                reader,
                Some(input),
                None,
                dialect.0,
                rust_dialect.encoding,
                plan,
            )
        })
    }

    /// A reader over an open stream, read a buffer at a time, one `fread()` for as many complete
    /// records as a read wants, so a pipe's records aren't held back for more. The stream stays the
    /// caller's: the reader never closes it. A stream that fails goes to the caller and isn't final:
    /// the next `read()` reads on from where it was.
    ///
    /// @throws \InvalidArgumentException as `fromString()`, and for anything but an open stream.
    /// @throws \RuntimeException when reading the stream fails.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(
        batchRows = BatchRows::DEFAULT,
        bufferBytes = BufferBytes::DEFAULT,
        notes = true
    ))]
    pub fn from_stream(
        stream: Mixed,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
        notes: Notes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            streams::check_stream(&stream.0)?;
            let checked = checked_arguments(
                &dialect.0,
                plan.0.as_ref(),
                batchRows.0,
                bufferBytes.0,
                notes.0,
            )?;
            DelimitedReader::over_stream(stream.0, false, dialect.0, checked)
        })
    }

    /// A reader over the file at `$path`, opened with `fopen($path, 'rb')` — any stream wrapper
    /// PHP knows — and closed by `close()` or when the reader is destroyed. The arguments are judged
    /// before the file is opened: a size the core refuses says so ahead of a missing file.
    ///
    /// @throws \RuntimeException when the file cannot be opened or read.
    /// @throws \InvalidArgumentException as `fromString()`.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(
        batchRows = BatchRows::DEFAULT,
        bufferBytes = BufferBytes::DEFAULT,
        notes = true
    ))]
    pub fn open(
        path: Str,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
        notes: Notes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            let checked = checked_arguments(
                &dialect.0,
                plan.0.as_ref(),
                batchRows.0,
                bufferBytes.0,
                notes.0,
            )?;
            let stream = streams::open(&path.0)?;
            // Refused, the stream is closed with the last reference to it, which is this one.
            DelimitedReader::over_stream(stream, true, dialect.0, checked)
        })
    }

    /// What a sample of the file at `$path`, opened as `open()` opens it, says the columns are: a
    /// plan to change or to pass to a read, and why. It starts you off, and it's yours to inspect:
    /// nothing is applied, and a read never sniffs.
    ///
    /// Every format `$options` holds is judged first, before anything is opened. Then what opening
    /// the file can throw, and `TabularException` for a failure among the sampled rows. A failure
    /// past them isn't met, or, when the stream's `fread()` returns `false`, only says the source was
    /// truncated. An exception the stream's own PHP throws is the caller's, and is rethrown wherever
    /// it comes. A sample of no header reads the file's first record alone and then opens it again.
    ///
    /// @throws \InvalidArgumentException for a format the core refuses, or too many columns.
    /// @throws \RuntimeException when the file cannot be opened, or reading it fails among the
    ///                           sampled rows.
    /// @throws \Throwable whatever the stream's own PHP throws.
    /// @throws TabularException at a broken record among the sampled rows.
    pub fn suggest(
        path: Str,
        dialect: DialectArg,
        options: OptSuggestOptions,
    ) -> PhpResult<SuggestionObject> {
        guard(|| {
            let made = suggest_options_of(options.0.as_ref())?;
            let rust_dialect = dialect_of(&dialect.0)?;
            let stash: Stash = Rc::default();
            let thrown: Rc<Cell<bool>> = Rc::default();
            let found =
                crate::DelimitedReader::suggest_from(rust_dialect, &made, |rows, chosen| {
                    let stream = streams::open(&path.0).map_err(|failure| {
                        *stash.borrow_mut() = Some(taken(failure));
                        Error::Io(Arc::new(std::io::Error::other("the file failed to open")))
                    })?;
                    DelimitedOptions::new()
                        .notes(false)
                        .batch_rows(rows)
                        .from_reader_unbound(
                            Sampled {
                                stream,
                                stash: Rc::clone(&stash),
                                thrown: Rc::clone(&thrown),
                            },
                            chosen,
                        )
                });
            match found {
                _ if thrown.get() => Err(stashed(&stash)),
                Ok(found) => suggestion_of(&found, options.0.as_ref())
                    .map(|made| SuggestionObject(Some(made))),
                Err(Error::Io(_)) if stash.borrow().is_some() => Err(stashed(&stash)),
                Err(other) => Err(rust_error(other)),
            }
        })
    }

    /// `suggest()`, over a string.
    ///
    /// @throws \InvalidArgumentException for a format the core refuses, or too many columns.
    /// @throws TabularException at a broken record among the sampled rows.
    pub fn suggest_string(
        utf8: Str,
        dialect: DialectArg,
        options: OptSuggestOptions,
    ) -> PhpResult<SuggestionObject> {
        guard(|| {
            let made = suggest_options_of(options.0.as_ref())?;
            let found =
                crate::DelimitedReader::suggest_slice(&utf8.0, dialect_of(&dialect.0)?, &made)
                    .map_err(rust_error)?;
            suggestion_of(&found, options.0.as_ref()).map(|made| SuggestionObject(Some(made)))
        })
    }

    /// Closes the file `open()` opened.
    pub fn __destruct(&self) {
        // Nothing holds the state when the last reference goes; were something to, it keeps it.
        guard(|| match self.state.try_borrow_mut() {
            Ok(mut state) => state.shut(),
            Err(_) => Ok(()),
        })
        .unwrap_or_else(|e| e.throw());
    }

    /// The header's names, as the source wrote them; an empty list for an empty input; null when
    /// the dialect says there is no header.
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

    /// Declares the plan, once, before the first read: which source column, through which door.
    ///
    /// @throws \LogicException when the reader already has a plan.
    /// @throws \InvalidArgumentException for a plan entry that is not a `Column`, or a plan
    ///                                   column past the header's width.
    pub fn bind(&self, plan: ArrayArg) -> PhpResult<()> {
        guard(|| {
            let mut state = self.state()?;
            if state.plan.is_some() {
                return Err(logic("The reader already has a plan; a plan is bound once"));
            }
            state.bind_plan(Plan::new(&plan.0)?)
        })
    }

    /// Whether the reader has a plan.
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

    /// How many cells every record has — the width of the first record, the header's when there
    /// is one — or null before one has been read.
    pub fn column_count(&self) -> PhpResult<Option<i64>> {
        guard(|| {
            let state = self.view()?;
            Ok(state
                .reader
                .as_ref()
                .map_or(state.closed.and_then(|closed| closed.1), |reader| {
                    reader.column_count()
                })
                .map(|width| width as i64))
        })
    }

    /// The dialect the reader reads.
    pub fn dialect(&self) -> PhpResult<DialectObject> {
        guard(|| Ok(DialectObject(Some(self.dialect.shallow_clone()))))
    }

    /// How many records the core has finished with, the header's included.
    pub fn records(&self) -> PhpResult<i64> {
        guard(|| {
            let state = self.view()?;
            Ok(state
                .reader
                .as_ref()
                .map_or(state.closed.map_or(0, |closed| closed.0), |reader| {
                    reader.records()
                }) as i64)
        })
    }

    /// The next batch of rows, or null at the end of the input.
    ///
    /// @throws \LogicException when the reader is closed or has no plan.
    /// @throws TabularException at a structural failure, once every intact row before it has
    ///                          been delivered — and again at every read after.
    /// @throws \RuntimeException when reading the stream fails.
    /// @throws \LogicException as well when its own stream, mid-read, calls back into the reader.
    pub fn read(&self) -> PhpResult<MaybeBatch> {
        guard(|| self.state()?.next_batch().map(MaybeBatch))
    }

    /// Every row the reader has left, a batch at a time, each a `Row` keyed by its place in the
    /// read: an iterator that reads as it goes, forward only.
    pub fn rows(self_: &mut ZendClassObject<DelimitedReader>) -> PhpResult<IteratorObject> {
        guard(|| RowIterator::over_reader(this_zval(self_)).map(|it| IteratorObject(Some(it))))
    }

    /// Lets go of the input, closing the file `open()` opened. A closed reader reads no more.
    ///
    /// @throws \LogicException when the reader's own stream, mid-read, calls this.
    pub fn close(&self) -> PhpResult<()> {
        guard(|| self.state()?.shut())
    }
}
