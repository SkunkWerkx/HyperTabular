//! The Rust binding over the core: delimited text into owned [`Batch`]es.
//!
//! This is what every binding does, written in Rust. The core
//! ([`crate::kernel::delimited::fill`]) owns no memory and reads no files; it fills
//! buffers it is handed and says how far it got. Everything else is here: the input
//! buffer and the reads that fill it, the batch's column vectors, the cell table, the
//! arena — allocated once and reused — and the loop that puts what the core did not
//! consume back in front of it. A batch's text is copied into the batch, so that it
//! outlives the chunk it was read from; that copy is this binding's choice, not the
//! core's cost.

use crate::batch::Batch;
use crate::delimited::dialect::Dialect;
use crate::delimited::error::Error;
use crate::kernel::abi::{
    ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Failure, Filled, OK, Span,
};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::kernel::delimited::unescape::unescape_into;
use crate::plan::Plan;
use crate::source::Header;
use hypercast::RawNumFormat;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The column buffers of the call in progress. Raw pointers into a batch, so the list is
/// emptied before every call returns; it is a field only to keep its allocation.
#[derive(Default)]
struct Buffers(Vec<ColumnBuffer>);

// SAFETY: the list is empty whenever the reader is not inside `fill`.
unsafe impl Send for Buffers {}

enum Source<'a> {
    /// Bytes already in memory, read in place from `at`.
    Slice { bytes: &'a [u8], at: usize },
    /// A stream, read into `buf`; `buf[at..]` is what the core has not finished with.
    Stream {
        buf: Vec<u8>,
        at: usize,
        reader: Box<dyn Read + Send + 'a>,
        eof: bool,
    },
}

impl Source<'_> {
    /// What the core is to read next, and whether nothing follows it.
    fn window(&self) -> (&[u8], bool) {
        match self {
            Source::Slice { bytes, at } => (&bytes[*at..], true),
            Source::Stream { buf, at, eof, .. } => (&buf[*at..], *eof),
        }
    }

    fn advance(&mut self, consumed: usize) {
        match self {
            Source::Slice { at, .. } | Source::Stream { at, .. } => *at += consumed,
        }
    }

    /// Puts the unfinished row at the front of the buffer and reads more behind it,
    /// doubling the buffer when the row already fills it.
    fn refill(&mut self, state: &State) -> Result<(), Error> {
        let Source::Stream {
            buf,
            at,
            reader,
            eof,
        } = self
        else {
            return Ok(());
        };
        buf.drain(..*at);
        *at = 0;
        if buf.len() == buf.capacity() {
            if buf.capacity() >= BatchReader::MAX_ROW_BYTES {
                return Err(Error::RowTooLong {
                    limit: BatchReader::MAX_ROW_BYTES,
                    record: state.records,
                    line: state.line,
                    byte: state.offset,
                });
            }
            buf.reserve(buf.capacity());
        }
        let filled = buf.len();
        buf.resize(buf.capacity(), 0);
        let mut total = 0;
        while filled + total < buf.len() {
            match reader.read(&mut buf[filled + total..]) {
                Ok(0) => {
                    *eof = true;
                    break;
                }
                Ok(n) => total += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    buf.truncate(filled + total);
                    return Err(error.into());
                }
            }
        }
        buf.truncate(filled + total);
        Ok(())
    }
}

/// Delimited text, a batch at a time, through the core.
///
/// ```
/// use hypertabular::delimited::{BatchReader, Dialect};
/// use hypertabular::{Batch, Column, Door, Plan};
///
/// let text = b"id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n";
/// let plan = Plan::new(vec![Column::new(0, Door::I32), Column::new(2, Door::F64)]);
/// let mut reader = BatchReader::from_slice(text, Dialect::CSV, plan).unwrap();
/// assert_eq!(reader.header().unwrap().ordinal(b"score"), Some(2));
///
/// let mut batch = Batch::new();
/// while reader.fill(&mut batch, 1024).unwrap() > 0 {
///     assert_eq!(batch.column(0).i32s().unwrap(), &[1, 2]);
///     assert_eq!(batch.column(1).f64s().unwrap()[0], 2.5);
///     assert!(!batch.column(1).verdicts()[1].is_ok()); // `x` is not a number
/// }
/// ```
pub struct BatchReader<'a> {
    dialect: Dialect,
    plan: Plan,
    specs: Vec<ColumnSpec>,
    /// Table entries one row takes: the widest ordinal the plan reads, plus two.
    per_row: usize,
    state: State,
    source: Source<'a>,
    cells: Vec<Span>,
    arena: Vec<u8>,
    scratch: Vec<u8>,
    columns: Buffers,
    header: Option<Header>,
    error: Option<Error>,
}

impl<'a> BatchReader<'a> {
    /// The row ceiling for a stream: a single record larger than this is a structural
    /// error rather than a buffer that grows without end.
    pub const MAX_ROW_BYTES: usize = 1 << 30;
    /// The initial buffer for a stream.
    pub const DEFAULT_CAPACITY: usize = 256 * 1024;

    /// A reader over bytes already in memory — read in place, never copied. Reads the
    /// header record immediately if the dialect declares one.
    pub fn from_slice(bytes: &'a [u8], dialect: Dialect, plan: Plan) -> Result<Self, Error> {
        BatchReader::build(Source::Slice { bytes, at: 0 }, dialect, plan)
    }

    /// A reader over a stream, buffering [`BatchReader::DEFAULT_CAPACITY`] bytes at a
    /// time.
    pub fn from_reader<R: Read + Send + 'a>(
        reader: R,
        dialect: Dialect,
        plan: Plan,
    ) -> Result<Self, Error> {
        BatchReader::from_reader_with_capacity(reader, dialect, plan, Self::DEFAULT_CAPACITY)
    }

    /// A reader over a stream with a chosen initial buffer (the tests use tiny ones to
    /// exercise every refill path).
    pub fn from_reader_with_capacity<R: Read + Send + 'a>(
        reader: R,
        dialect: Dialect,
        plan: Plan,
        capacity: usize,
    ) -> Result<Self, Error> {
        let source = Source::Stream {
            buf: Vec::with_capacity(capacity.max(64)),
            at: 0,
            reader: Box::new(reader),
            eof: false,
        };
        BatchReader::build(source, dialect, plan)
    }

    /// A reader over a file.
    pub fn from_path(
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: Plan,
    ) -> Result<BatchReader<'static>, Error> {
        BatchReader::from_reader(File::open(path)?, dialect, plan)
    }

    fn build(source: Source<'a>, dialect: Dialect, plan: Plan) -> Result<Self, Error> {
        dialect.validate()?;
        let raw = RawDialect {
            separator: dialect.separator,
            quoting: u8::from(dialect.quoting),
            skip_blank_lines: u8::from(dialect.skip_blank_lines),
            engine: 0,
        };
        let state = State::init(raw).ok_or(Error::Separator(dialect.separator))?;
        let mut specs = Vec::with_capacity(plan.len());
        for (index, column) in plan.columns().iter().enumerate() {
            let (param, format) = (declared(column.door), column.format);
            let mut currency = [0u8; 16];
            let symbol = format.currency.as_bytes();
            currency[..symbol.len()].copy_from_slice(symbol);
            let spec = ColumnSpec {
                ordinal: column.ordinal as u32,
                door: column.door.code(),
                param,
                format: RawNumFormat {
                    decimal_sep: u32::from(format.decimal_sep),
                    group_sep: u32::from(format.group_sep),
                    flags: format.flags,
                    currency_len: symbol.len() as u32,
                    currency,
                },
            };
            if spec.num_format().is_none() {
                return Err(Error::Notation { column: index });
            }
            specs.push(spec);
        }
        let mut reader = BatchReader {
            dialect,
            per_row: plan.max_ordinal().map_or(0, |ordinal| ordinal + 1) + 1,
            plan,
            specs,
            state,
            source,
            cells: Vec::new(),
            arena: Vec::new(),
            scratch: Vec::new(),
            columns: Buffers::default(),
            header: None,
            error: None,
        };
        if dialect.has_header {
            reader.read_header()?;
        }
        Ok(reader)
    }

    /// The dialect in force.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The plan every batch is filled through.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The declared header, once read.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// The cell count every record must have, once the first record fixed it.
    pub fn column_count(&self) -> Option<usize> {
        (self.state.expected != 0).then_some(self.state.expected as usize)
    }

    /// Records consumed so far, the header and skipped blank lines included.
    pub fn records(&self) -> u64 {
        self.state.records
    }

    fn fail(&mut self, error: Error) -> Error {
        self.error = Some(error.clone());
        error
    }

    fn structural(&mut self, failure: Failure) -> Error {
        let error = match failure.code {
            Failure::COLUMN_COUNT => Error::ColumnCount {
                expected: failure.expected as usize,
                found: failure.found as usize,
                record: failure.record,
                line: failure.line,
                byte: failure.byte,
            },
            _ => Error::UnclosedQuote {
                record: failure.record,
                line: failure.line,
                byte: failure.byte,
            },
        };
        self.fail(error)
    }

    fn read_header(&mut self) -> Result<(), Error> {
        let mut names = vec![Span::default(); 64];
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window();
            let code = fill::header(
                &mut self.state,
                window,
                last,
                &mut names,
                &mut self.arena,
                &mut out,
            );
            match code {
                OK if out.rows > 0 => {
                    let mut header = Header::new();
                    for name in &names[..out.rows as usize] {
                        let from: &[u8] = if name.flagged() { &self.arena } else { window };
                        let start = name.offset as usize;
                        header.push(&from[start..start + name.len()]);
                    }
                    self.header = Some(header);
                    self.source.advance(out.consumed as usize);
                    return Ok(());
                }
                OK => {
                    let (rest, consumed) = (window.len(), out.consumed as usize);
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        // An empty input has no header and no rows; the width is unknown.
                        self.header = Some(Header::new());
                        return Ok(());
                    }
                    if consumed == 0
                        && let Err(error) = self.source.refill(&self.state)
                    {
                        return Err(self.fail(error));
                    }
                }
                ERR_CELLS => names.resize(out.needed as usize, Span::default()),
                ERR_ARENA => self.arena.resize(out.needed as usize, 0),
                ERR_STRUCTURE => return Err(self.structural(out.failure)),
                _ => return Err(self.fail(Error::Separator(self.dialect.separator))),
            }
        }
    }

    /// Fills `batch` with up to `max_rows` rows and returns how many landed — `0` once
    /// the input is exhausted. A structural error is returned after every intact row
    /// before it has been delivered, and is sticky.
    pub fn fill(&mut self, batch: &mut Batch, max_rows: usize) -> Result<usize, Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        batch.prepare(&self.plan);
        if max_rows == 0 {
            return Ok(0);
        }
        if self.cells.len() < self.per_row * max_rows {
            self.cells.resize(self.per_row * max_rows, Span::default());
        }
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window();
            self.columns.0.clear();
            batch.raw_columns(max_rows, &mut self.columns.0);
            // SAFETY: `raw_columns` reserved `max_rows` values of each column's own type,
            // which is the type its door writes, and `max_rows` verdicts.
            let code = unsafe {
                fill::fill(
                    &mut self.state,
                    window,
                    last,
                    &self.specs,
                    &self.columns.0,
                    max_rows,
                    &mut self.cells,
                    &mut self.arena,
                    &mut out,
                )
            };
            self.columns.0.clear();
            match code {
                OK if out.rows > 0 => {
                    let rows = out.rows as usize;
                    // SAFETY: the core wrote `rows` values and verdicts to every column.
                    unsafe { batch.commit(rows) };
                    let arena = &self.arena;
                    let located = |span: Span| -> &[u8] {
                        let from: &[u8] = if span.flagged() { arena } else { window };
                        let start = span.offset as usize;
                        &from[start..start + span.len()]
                    };
                    batch.own_text(located);
                    // The fault table: the raw text of every cell that did not cast, which
                    // the cell table locates in the window.
                    for (index, spec) in self.specs.iter().enumerate() {
                        for row in 0..rows {
                            if batch.column(index).verdicts()[row].reason < 2 {
                                continue;
                            }
                            let cell = self.cells[row * self.per_row + spec.ordinal as usize];
                            let start = cell.offset as usize;
                            let raw = &window[start..start + cell.len()];
                            if cell.flagged() {
                                self.scratch.resize(raw.len(), 0);
                                let written = unescape_into(raw, &mut self.scratch);
                                batch.push_fault(
                                    row as u32,
                                    index as u32,
                                    &self.scratch[..written],
                                );
                            } else {
                                batch.push_fault(row as u32, index as u32, raw);
                            }
                        }
                    }
                    batch.sort_faults();
                    self.source.advance(out.consumed as usize);
                    return Ok(rows);
                }
                OK => {
                    let (rest, consumed) = (window.len(), out.consumed as usize);
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        return Ok(0);
                    }
                    if consumed == 0
                        && let Err(error) = self.source.refill(&self.state)
                    {
                        return Err(self.fail(error));
                    }
                }
                ERR_CELLS => {
                    let needed = out.needed as usize * max_rows;
                    self.cells.resize(needed, Span::default());
                }
                ERR_ARENA => {
                    let needed = (out.needed as usize).max(self.arena.len() * 2);
                    self.arena.resize(needed, 0);
                }
                ERR_STRUCTURE => return Err(self.structural(out.failure)),
                _ => return Err(self.fail(Error::Separator(self.dialect.separator))),
            }
        }
    }
}

/// What a door declares beside its code, as the core reads it.
fn declared(door: crate::Door) -> u32 {
    match door {
        crate::Door::Unix(precision) => precision as u32,
        crate::Door::DateOrdered(order) | crate::Door::DateTime(order) => order as u32,
        crate::Door::ExcelSerial(epoch) => epoch as u32,
        _ => 0,
    }
}
