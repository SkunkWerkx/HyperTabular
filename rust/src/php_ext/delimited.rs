//! `DelimitedReader`: delimited text read a batch at a time into typed columns.

// The factories' parameters are named as PHP names them, for named arguments: 1.0's
// `$batchRows` and `$bufferBytes`.
#![allow(non_snake_case)]

use std::rc::Rc;

use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, Zval};
use ext_php_rs::zend::ce;

use super::args::{ArrayArg, BatchRows, BufferBytes, DialectArg, Mixed, OptArray, Str};
use super::batch::{
    Array, BatchCore, IteratorObject, MaybeArray, MaybeBatch, RowIterator, this_zval, typed_return,
};
use super::classes::{bytes_zval, list_zval};
use super::enums::TabularFailure;
use super::errors::{contract_violation, invalid_argument, logic, object, runtime};
use super::guard;
use super::header::{Header, MaybeHeader};
use super::plan::Plan;
use super::streams;
use super::values::{MAX_ROW_BYTES, dialect_fields, tabular_exception, tabular_exception_of};
use crate::column;
use crate::kernel::abi::{ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Filled, OK, Span};
use crate::kernel::delimited::fill::{self, RawDialect, State};

/// The arena a read starts with.
const ARENA_BYTES: usize = 4096;
/// The header's name table a read starts with.
const NAMES: usize = 64;

typed_return!(
    /// `Dialect`.
    DialectObject,
    "HyperTabular\\Dialect",
    false
);

/// Where the bytes come from.
enum Source {
    /// A string, handed over a chunk at a time.
    Text { text: Vec<u8>, at: usize },
    /// A stream, closed with the reader when the reader opened it.
    Stream { stream: Zval, owned: bool },
    /// Closed.
    Gone,
}

/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
/// typed columns, every cell a HyperCast `Success|Fault` verdict.
///
/// The core owns no memory and reads no files. It is handed a chunk of input and the buffers
/// to fill, casts each plan column in one loop, and says how many rows it wrote and how many
/// bytes it is finished with. This class allocates the input buffer, one value array and one
/// verdict array per column, the table that locates each cell and the arena for the rare
/// escaped cell — once, reused for every batch — and puts what the core did not consume back
/// in front of it. `read()` returns the rows as a `Batch`.
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
    dialect: Zval,
    state: State,
    plan: Option<Plan>,
    batch_rows: usize,
    per_row: usize,
    cells: Vec<Span>,
    arena: Vec<u8>,
    cramped: bool,
    out: Filled,
    source: Source,
    /// The input buffer: the bytes at `start..end` are what the core has not finished with.
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    header: Option<Zval>,
    header_index: Option<Zval>,
    failure: Option<Zval>,
    closed: bool,
}

/// 1.0's refusal of a buffer with no room.
fn check_buffer(buffer_bytes: i64) -> PhpResult<usize> {
    usize::try_from(buffer_bytes)
        .ok()
        .filter(|bytes| *bytes >= 1)
        .ok_or_else(|| {
            invalid_argument(format!(
                "The buffer must hold at least one byte; got {buffer_bytes}"
            ))
        })
}

/// Refuses a plan column past the width the header gave every record.
fn check_width(plan: &Plan, width: Option<u32>) -> PhpResult<()> {
    let Some(width) = width else {
        return Ok(());
    };
    for (index, ordinal) in plan.ordinals.iter().enumerate() {
        if *ordinal >= width as usize {
            return Err(invalid_argument(format!(
                "Plan column {index} reads source column {ordinal}, past the {width} columns the header gives every record."
            )));
        }
    }
    Ok(())
}

impl DelimitedReader {
    /// 1.0's private constructor: the batch size, the plan, then the dialect.
    fn new(
        dialect: Zval,
        plan: Option<Zval>,
        batch_rows: i64,
        buffer_bytes: usize,
        source: Source,
    ) -> PhpResult<Self> {
        let batch_rows = usize::try_from(batch_rows)
            .ok()
            .filter(|rows| *rows >= 1)
            .ok_or_else(|| {
                invalid_argument(format!(
                    "A batch must hold at least one row; got {batch_rows}"
                ))
            })?;
        // Checked before the dialect, as 1.0 bound it before starting the state; the width is
        // checked once the header has been read.
        let plan = plan.map(|plan| Plan::new(&plan, batch_rows)).transpose()?;
        let (separator, quoting, _, skip_blank_lines) = dialect_fields(&dialect)?;
        let separator = separator.first().copied().unwrap_or(0);
        let raw = RawDialect {
            separator,
            quoting: u8::from(quoting),
            skip_blank_lines: u8::from(skip_blank_lines),
            engine: 0,
        };
        let state = State::init(raw).ok_or_else(|| {
            invalid_argument(format!(
                "Separator 0x{separator:02X} is not tab or printable ASCII other than '\"'"
            ))
        })?;
        let mut reader = DelimitedReader {
            dialect,
            state,
            plan: None,
            batch_rows,
            per_row: 0,
            cells: Vec::new(),
            arena: vec![0; ARENA_BYTES],
            cramped: false,
            out: Filled::default(),
            source,
            buffer: vec![0; buffer_bytes],
            start: 0,
            end: 0,
            eof: false,
            header: None,
            header_index: None,
            failure: None,
            closed: false,
        };
        if let Some(plan) = plan {
            reader.take(plan);
        }
        Ok(reader)
    }

    /// Sizes the cell table for `plan` and keeps it.
    fn take(&mut self, plan: Plan) {
        self.per_row = plan.width + 1;
        self.cells =
            vec![Span::default(); self.per_row * column::cell_rows(self.per_row, self.batch_rows)];
        self.plan = Some(plan);
    }

    /// Fills the buffer, reads the header if the dialect has one, and checks a plan given up
    /// front against it.
    fn begin(&mut self) -> PhpResult<()> {
        if !self.eof {
            self.refill()?;
        }
        let (_, _, has_header, _) = dialect_fields(&self.dialect)?;
        if has_header {
            self.read_header()?;
            if let Some(plan) = &self.plan {
                check_width(plan, self.width())?;
            }
        }
        Ok(())
    }

    fn width(&self) -> Option<u32> {
        (self.state.expected != 0).then_some(self.state.expected)
    }

    /// At most `room` more bytes of the source.
    fn pull(&mut self, room: usize) -> PhpResult<Vec<u8>> {
        match &mut self.source {
            Source::Text { text, at } => {
                let end = at.saturating_add(room).min(text.len());
                let chunk = text[*at..end].to_vec();
                *at = end;
                if *at >= text.len() {
                    // Known without asking again, so the core hears `last` with the final bytes.
                    self.eof = true;
                }
                Ok(chunk)
            }
            Source::Stream { stream, .. } => {
                let Some(chunk) = streams::read(stream, room)? else {
                    return Err(runtime("hypertabular: reading the stream failed"));
                };
                if chunk.is_empty() && !streams::at_end(stream)? {
                    return Err(runtime(
                        "hypertabular: the stream returned no data before its end (a non-blocking stream, or a \
                         read that timed out); the reader needs one that blocks",
                    ));
                }
                Ok(chunk)
            }
            Source::Gone => Ok(Vec::new()),
        }
    }

    /// Moves what the core has not finished with to the front, doubles the buffer if one
    /// record fills it, and reads more behind it.
    fn refill(&mut self) -> PhpResult<()> {
        let pending = self.end - self.start;
        if self.start > 0 {
            self.buffer.copy_within(self.start..self.end, 0);
            self.start = 0;
            self.end = pending;
        }
        if self.end == self.buffer.len() {
            // One record fills the buffer: it needs a bigger one.
            if self.buffer.len() >= MAX_ROW_BYTES {
                let failure = tabular_exception_of(
                    TabularFailure::RowTooLong,
                    self.state.records as i64,
                    i64::from(self.state.line),
                    self.state.offset as i64,
                    0,
                    0,
                )?;
                let thrown = object(&failure);
                self.failure = Some(failure);
                return Err(thrown);
            }
            let capacity = (self.buffer.len() * 2).min(MAX_ROW_BYTES);
            self.buffer.resize(capacity, 0);
        }
        let chunk = self.pull(self.buffer.len() - self.end)?;
        if chunk.is_empty() {
            self.eof = true;
            return Ok(());
        }
        self.buffer[self.end..self.end + chunk.len()].copy_from_slice(&chunk);
        self.end += chunk.len();
        Ok(())
    }

    fn grow_arena(&mut self, needed: usize) {
        let capacity = needed.max(self.arena.len() * 2);
        self.arena = vec![0; capacity];
    }

    /// The structural failure the core reported, kept and thrown.
    fn structural(&mut self) -> PhpException {
        match tabular_exception(&self.out.failure) {
            Ok(failure) => {
                let thrown = object(&failure);
                self.failure = Some(failure);
                thrown
            }
            Err(e) => e,
        }
    }

    fn read_header(&mut self) -> PhpResult<()> {
        let mut names = vec![Span::default(); NAMES];
        loop {
            let length = self.end - self.start;
            let last = self.eof;
            let window = &self.buffer[self.start..self.end];
            let code = fill::header(
                &mut self.state,
                window,
                last,
                &mut names,
                &mut self.arena,
                &mut self.out,
            );
            match code {
                OK => {
                    let consumed = self.out.consumed as usize;
                    if self.out.rows > 0 {
                        let mut header = Vec::with_capacity(self.out.rows as usize);
                        for name in names.iter().take(self.out.rows as usize) {
                            let from: &[u8] = if name.flagged() { &self.arena } else { window };
                            let offset = (name.offset as usize).min(from.len());
                            let end = offset.saturating_add(name.len()).min(from.len());
                            header.push(bytes_zval(&from[offset..end]));
                        }
                        self.start += consumed;
                        self.header = Some(list_zval(header)?);
                        return Ok(());
                    }
                    self.start += consumed;
                    if last && (consumed == length || consumed == 0) {
                        // An empty input has no header and no rows; the width is unknown.
                        self.header = Some(list_zval(Vec::new())?);
                        return Ok(());
                    }
                    if consumed == 0 {
                        self.refill()?;
                    }
                }
                ERR_CELLS => names = vec![Span::default(); self.out.needed as usize],
                ERR_ARENA => self.grow_arena(self.out.needed as usize),
                ERR_STRUCTURE => return Err(self.structural()),
                _ => return Err(contract_violation()),
            }
        }
    }

    fn next_batch(&mut self) -> PhpResult<Option<Zval>> {
        if self.closed {
            return Err(logic("The reader is closed"));
        }
        if self.plan.is_none() {
            return Err(logic(
                "The reader has no plan to read through; bind() one first",
            ));
        }
        if let Some(failure) = &self.failure {
            return Err(object(failure));
        }
        if self.cramped {
            self.grow_arena(self.arena.len() * 2);
            self.cramped = false;
        }
        loop {
            let plan = self.plan.as_mut().ok_or_else(contract_violation)?;
            let batch_rows = plan.batch_rows;
            let buffers = plan.buffers();
            let length = self.end - self.start;
            let last = self.eof;
            // SAFETY: each column's two arrays have room for `batch_rows` values of its door's
            // type and `batch_rows` verdicts (`Plan::new`).
            let code = unsafe {
                fill::fill(
                    &mut self.state,
                    &self.buffer[self.start..self.end],
                    last,
                    &plan.specs,
                    &buffers,
                    batch_rows,
                    &mut self.cells,
                    &mut self.arena,
                    &mut self.out,
                )
            };
            match code {
                OK => {
                    let consumed = self.out.consumed as usize;
                    let rows = self.out.rows as usize;
                    if rows > 0 {
                        // Short of what the cell table holds, not of the batch size: a table
                        // sized below a batch is not the arena's doing.
                        let held = batch_rows.min(self.cells.len() / self.per_row.max(1));
                        let arena_used = self.out.arena_used as usize;
                        self.cramped = rows < held && arena_used * 2 >= self.arena.len();
                        let cells =
                            self.cells[..(rows * self.per_row).min(self.cells.len())].to_vec();
                        let input = self.buffer[self.start..self.start + consumed].to_vec();
                        let arena = self.arena[..arena_used.min(self.arena.len())].to_vec();
                        let plan = self.plan.as_ref().ok_or_else(contract_violation)?;
                        let core = BatchCore::new(
                            plan,
                            rows,
                            cells,
                            self.per_row,
                            Rc::new(input),
                            arena,
                            false,
                        );
                        self.start += consumed;
                        return super::batch::Batch::object(core).map(Some);
                    }
                    self.start += consumed;
                    if last && (consumed == length || consumed == 0) {
                        return Ok(None);
                    }
                    if consumed == 0 {
                        self.refill()?;
                    }
                }
                ERR_CELLS => {
                    self.per_row = self.per_row.max(self.out.needed as usize);
                    let rows = column::cell_rows(self.per_row, batch_rows);
                    self.cells = vec![Span::default(); self.per_row * rows];
                }
                ERR_ARENA => self.grow_arena(self.out.needed as usize),
                ERR_STRUCTURE => return Err(self.structural()),
                _ => return Err(contract_violation()),
            }
        }
    }

    fn shut(&mut self) -> PhpResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let source = std::mem::replace(&mut self.source, Source::Gone);
        if let Source::Stream {
            stream,
            owned: true,
        } = source
        {
            streams::close(&stream)?;
        }
        Ok(())
    }

    /// `fromStream` on a stream already checked.
    fn over_stream(
        stream: Zval,
        owned: bool,
        dialect: Zval,
        plan: Option<Zval>,
        batch_rows: i64,
        buffer_bytes: i64,
    ) -> PhpResult<Self> {
        let buffer = check_buffer(buffer_bytes)?.min(MAX_ROW_BYTES);
        let mut reader = DelimitedReader::new(
            dialect,
            plan,
            batch_rows,
            buffer,
            Source::Stream { stream, owned },
        )?;
        reader.begin()?;
        Ok(reader)
    }
}

#[php_impl]
impl DelimitedReader {
    /// The longest a single record may be: 1 GiB. A record that outgrows it is a
    /// `TabularFailure::RowTooLong`.
    pub const MAX_ROW_BYTES: i64 = 1 << 30;
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

    /// A reader over `$utf8`, held in memory and handed to the core a buffer at a time.
    ///
    /// @throws \InvalidArgumentException for a batch or buffer of no rows or bytes, a plan entry
    ///                                   that is not a `Column`, a separator the core refuses or
    ///                                   a plan column past the header's width.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(batchRows = BatchRows::DEFAULT, bufferBytes = BufferBytes::DEFAULT))]
    pub fn from_string(
        utf8: Str,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            let requested = check_buffer(bufferBytes.0)?;
            let buffer = utf8.0.len().min(requested).clamp(1, MAX_ROW_BYTES);
            let empty = utf8.0.is_empty();
            let mut reader = DelimitedReader::new(
                dialect.0,
                plan.0,
                batchRows.0,
                buffer,
                Source::Text {
                    text: utf8.0,
                    at: 0,
                },
            )?;
            reader.eof = empty;
            reader.begin()?;
            Ok(reader)
        })
    }

    /// A reader over an open stream, read a buffer at a time. The stream stays the caller's: the
    /// reader never closes it.
    ///
    /// @throws \InvalidArgumentException as `fromString()`, and for anything but an open stream.
    /// @throws \RuntimeException when reading the stream fails.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(batchRows = BatchRows::DEFAULT, bufferBytes = BufferBytes::DEFAULT))]
    pub fn from_stream(
        stream: Mixed,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            streams::check_stream(&stream.0)?;
            DelimitedReader::over_stream(
                stream.0,
                false,
                dialect.0,
                plan.0,
                batchRows.0,
                bufferBytes.0,
            )
        })
    }

    /// A reader over the file at `$path`, opened with `fopen($path, 'rb')` — any stream wrapper
    /// PHP knows — and closed by `close()` or when the reader is destroyed.
    ///
    /// @throws \RuntimeException when the file cannot be opened or read.
    /// @throws \InvalidArgumentException as `fromString()`.
    /// @throws TabularException when the header itself is broken.
    #[php(defaults(batchRows = BatchRows::DEFAULT, bufferBytes = BufferBytes::DEFAULT))]
    pub fn open(
        path: Str,
        dialect: DialectArg,
        plan: OptArray,
        batchRows: BatchRows,
        bufferBytes: BufferBytes,
    ) -> PhpResult<DelimitedReader> {
        guard(|| {
            let stream = streams::open(&path.0)?;
            // Refused, the stream is closed with the last reference to it, which is this one.
            DelimitedReader::over_stream(
                stream,
                true,
                dialect.0,
                plan.0,
                batchRows.0,
                bufferBytes.0,
            )
        })
    }

    /// Closes the file `open()` opened.
    pub fn __destruct(&mut self) {
        guard(|| self.shut()).unwrap_or_else(|e| e.throw());
    }

    /// The header's names, as the source wrote them; an empty list for an empty input; null when
    /// the dialect says there is no header.
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

    /// Declares the plan, once, before the first read: which source column, through which door.
    ///
    /// @throws \LogicException when the reader already has a plan.
    /// @throws \InvalidArgumentException for a plan entry that is not a `Column`, or a plan
    ///                                   column past the header's width.
    pub fn bind(&mut self, plan: ArrayArg) -> PhpResult<()> {
        guard(|| {
            if self.plan.is_some() {
                return Err(logic("The reader already has a plan; a plan is bound once"));
            }
            let plan = Plan::new(&plan.0, self.batch_rows)?;
            check_width(&plan, self.width())?;
            self.take(plan);
            Ok(())
        })
    }

    /// Whether the reader has a plan.
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

    /// How many cells every record has — the width of the first record, the header's when there
    /// is one — or null before one has been read.
    pub fn column_count(&self) -> PhpResult<Option<i64>> {
        guard(|| Ok(self.width().map(i64::from)))
    }

    /// The dialect the reader reads.
    pub fn dialect(&self) -> PhpResult<DialectObject> {
        guard(|| Ok(DialectObject(Some(self.dialect.shallow_clone()))))
    }

    /// How many records the core has finished with, the header's included.
    pub fn records(&self) -> PhpResult<i64> {
        guard(|| Ok(self.state.records as i64))
    }

    /// The next batch of rows, or null at the end of the input.
    ///
    /// @throws \LogicException when the reader is closed or has no plan.
    /// @throws TabularException at a structural failure, once every intact row before it has
    ///                          been delivered — and again at every read after.
    /// @throws \RuntimeException when reading the stream fails.
    pub fn read(&mut self) -> PhpResult<MaybeBatch> {
        guard(|| self.next_batch().map(MaybeBatch))
    }

    /// Every row the reader has left, a batch at a time, each a `Row` keyed by its place in the
    /// read: an iterator that reads as it goes, forward only.
    pub fn rows(self_: &mut ZendClassObject<DelimitedReader>) -> PhpResult<IteratorObject> {
        guard(|| RowIterator::over_reader(this_zval(self_)).map(|it| IteratorObject(Some(it))))
    }

    /// Lets go of the input, closing the file `open()` opened. A closed reader reads no more.
    pub fn close(&mut self) -> PhpResult<()> {
        guard(|| self.shut())
    }
}
