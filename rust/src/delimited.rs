//! Delimited text — CSV, TSV, and their kin — read through a plan into batches.
//!
//! This is the Rust binding over the core, and the pattern every binding follows. The
//! core ([`crate::kernel::delimited::fill`]) owns no memory and reads no files: it fills
//! buffers it is handed and says how far it got. Everything else is here — the input
//! buffer and the reads that fill it, the column arrays, the cell table, the arena, each
//! allocated once and reused — and so is the loop that grows a buffer when the core says
//! it is too small and puts what the core did not consume back in front of it.
//!
//! The reader owns all of that and lends it: [`DelimitedReader::read`] returns a
//! [`Batch`] that borrows the reader, so a batch's columns are the arrays the core wrote
//! and its text is the input itself, and reading the next batch is what ends the last.

use crate::batch::{Batch, Columns, Origin};
use crate::column;
use crate::error::{Failure, FailureKind};
use crate::kernel::abi::{ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Filled, OK, Span};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::{Column, Error, Header};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The caller-declared dialect. The same philosophy as HyperCast's `NumFormat`: no
/// sniffing, no guessing — the separator is stated, quoting is stated, the header is
/// stated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dialect {
    /// The cell separator: a tab, or printable ASCII other than `"`.
    pub separator: u8,
    /// Whether `"` quotes a cell (RFC 4180, with `""` for a literal quote). Off, a quote
    /// is a byte like any other.
    pub quoting: bool,
    /// Whether the first record is the header rather than data.
    pub has_header: bool,
    /// Whether a line with nothing on it is skipped rather than read as a record.
    pub skip_blank_lines: bool,
}

impl Dialect {
    /// Comma-separated, quoted, with a header.
    pub const CSV: Dialect = Dialect {
        separator: b',',
        quoting: true,
        has_header: true,
        skip_blank_lines: true,
    };
    /// Tab-separated.
    pub const TSV: Dialect = Dialect {
        separator: b'\t',
        ..Dialect::CSV
    };
    /// Pipe-separated.
    pub const PSV: Dialect = Dialect {
        separator: b'|',
        ..Dialect::CSV
    };

    /// [`Dialect::CSV`] with another separator.
    pub const fn new(separator: u8) -> Dialect {
        Dialect {
            separator,
            ..Dialect::CSV
        }
    }

    /// The same dialect with quoting on or off.
    pub const fn with_quoting(self, quoting: bool) -> Dialect {
        Dialect { quoting, ..self }
    }

    /// The same dialect with or without a header record.
    pub const fn with_header(self, has_header: bool) -> Dialect {
        Dialect { has_header, ..self }
    }

    /// The same dialect skipping blank lines or reading them.
    pub const fn with_blank_lines_skipped(self, skip_blank_lines: bool) -> Dialect {
        Dialect {
            skip_blank_lines,
            ..self
        }
    }
}

/// How a reader sizes what it owns. [`DelimitedReader::options`] starts one; its
/// `open`, `from_slice` and `from_reader` finish it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelimitedOptions {
    batch_rows: usize,
    buffer_bytes: usize,
}

impl Default for DelimitedOptions {
    fn default() -> DelimitedOptions {
        DelimitedOptions::new()
    }
}

impl DelimitedOptions {
    /// 4096 rows a batch, through a 256 KiB input buffer.
    pub const fn new() -> DelimitedOptions {
        DelimitedOptions {
            batch_rows: 4096,
            buffer_bytes: 256 * 1024,
        }
    }

    /// The most rows a batch holds (at least one). Every column's arrays are this long.
    pub const fn batch_rows(self, rows: usize) -> DelimitedOptions {
        DelimitedOptions {
            batch_rows: if rows == 0 { 1 } else { rows },
            ..self
        }
    }

    /// The input buffer a file or a stream is read through to begin with. It grows to
    /// hold the longest record, so this is a starting size and not a limit; a slice is
    /// read in place and has no buffer.
    pub const fn buffer_bytes(self, bytes: usize) -> DelimitedOptions {
        DelimitedOptions {
            buffer_bytes: if bytes == 0 { 1 } else { bytes },
            ..self
        }
    }

    /// Opens the file at `path`.
    pub fn open(
        self,
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'static>, Error> {
        self.from_reader(File::open(path)?, dialect, plan)
    }

    /// Reads `bytes` in place: nothing is copied, and a batch's text is `bytes` itself.
    pub fn from_slice<'a>(
        self,
        bytes: &'a [u8],
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        DelimitedReader::build(Source::Slice { bytes, at: 0 }, dialect, plan, self)
    }

    /// Reads from any byte source.
    pub fn from_reader<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        let source = Source::Stream {
            buf: Vec::with_capacity(self.buffer_bytes),
            at: 0,
            reader: Box::new(reader),
            eof: false,
        };
        DelimitedReader::build(source, dialect, plan, self)
    }
}

/// Where the bytes come from.
enum Source<'a> {
    /// The whole input, in memory.
    Slice { bytes: &'a [u8], at: usize },
    /// A stream, read through a buffer that holds what the core has not consumed.
    Stream {
        buf: Vec<u8>,
        at: usize,
        reader: Box<dyn Read + Send + 'a>,
        eof: bool,
    },
}

impl Source<'_> {
    /// The unconsumed input, and whether it is the last of it.
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

    /// Moves the unconsumed tail to the front of the buffer and reads more behind it,
    /// growing the buffer if the tail fills it.
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
            if buf.capacity() >= DelimitedReader::MAX_ROW_BYTES {
                return Err(Error::Structure(Failure {
                    kind: FailureKind::RowTooLong,
                    // The offending record's index: how many were read whole before it.
                    record: state.records,
                    line: state.line,
                    byte: state.offset,
                    expected: 0,
                    found: 0,
                }));
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

/// A forward-only reader of delimited text. See the module documentation.
pub struct DelimitedReader<'a> {
    dialect: Dialect,
    state: State,
    source: Source<'a>,
    /// Bytes the batch last lent covers: consumed when the next read begins.
    pending: usize,
    columns: Columns,
    /// Cell-table entries to a row: one per source column the plan reaches, and one more.
    per_row: usize,
    cells: Vec<Span>,
    arena: Vec<u8>,
    /// The last batch came up short with the arena mostly used: it wants a larger one.
    cramped: bool,
    header: Option<Header>,
    error: Option<Error>,
}

impl<'a> DelimitedReader<'a> {
    /// The largest single record a stream is buffered for: past this the input is taken
    /// to be something other than delimited text.
    pub const MAX_ROW_BYTES: usize = 1 << 30;

    /// The options a reader is built with, to change before building one.
    pub const fn options() -> DelimitedOptions {
        DelimitedOptions::new()
    }

    /// Opens the file at `path`.
    pub fn open(
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'static>, Error> {
        DelimitedOptions::new().open(path, dialect, plan)
    }

    /// Reads `bytes` in place: nothing is copied, and a batch's text is `bytes` itself.
    pub fn from_slice(bytes: &'a [u8], dialect: Dialect, plan: &[Column]) -> Result<Self, Error> {
        DelimitedOptions::new().from_slice(bytes, dialect, plan)
    }

    /// Reads from any byte source.
    pub fn from_reader<R: Read + Send + 'a>(
        reader: R,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<Self, Error> {
        DelimitedOptions::new().from_reader(reader, dialect, plan)
    }

    fn build(
        source: Source<'a>,
        dialect: Dialect,
        plan: &[Column],
        options: DelimitedOptions,
    ) -> Result<Self, Error> {
        let raw = RawDialect {
            separator: dialect.separator,
            quoting: u8::from(dialect.quoting),
            skip_blank_lines: u8::from(dialect.skip_blank_lines),
            engine: 0,
        };
        let state = State::init(raw).ok_or(Error::Separator(dialect.separator))?;
        let (specs, width) = column::specs(plan)?;
        let per_row = width + 1;
        let mut reader = DelimitedReader {
            dialect,
            state,
            source,
            pending: 0,
            columns: Columns::new(plan, specs, options.batch_rows),
            per_row,
            cells: vec![Span::default(); per_row * options.batch_rows],
            arena: vec![0; 4096],
            cramped: false,
            header: None,
            error: None,
        };
        if dialect.has_header {
            reader.read_header()?;
        }
        Ok(reader)
    }

    /// The dialect the reader was built with.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The plan the reader was built with.
    pub fn plan(&self) -> &[Column] {
        self.columns.plan()
    }

    /// The header record's names, or `None` if the dialect declares no header. A header
    /// with no names is an input that had no record.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// How many cells a record has, once one has been read.
    pub fn column_count(&self) -> Option<usize> {
        (self.state.expected != 0).then_some(self.state.expected as usize)
    }

    /// How many records have been read, the header among them.
    pub fn records(&self) -> u64 {
        self.state.records
    }

    fn fail(&mut self, error: Error) -> Error {
        self.error = Some(error.clone());
        error
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
                ERR_STRUCTURE => return Err(self.fail(out.failure.into())),
                _ => return Err(self.fail(Error::Separator(self.dialect.separator))),
            }
        }
    }

    /// Reads the next batch: up to the reader's batch size of rows, or `None` when the
    /// input has no more. The batch borrows the reader, and is over when the next is
    /// asked for.
    ///
    /// A structural failure is returned once every intact row before it has been
    /// delivered, and again on every call after.
    pub fn read(&mut self) -> Result<Option<Batch<'_>>, Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.source.advance(std::mem::take(&mut self.pending));
        // The core stops a batch at the row the arena has no room for, so an arena too
        // small for a batch's unescaped text makes for short batches, not for an error.
        // The batch that was short has been given back by now, and the arena can move.
        if std::mem::take(&mut self.cramped) {
            self.arena.resize(self.arena.len() * 2, 0);
        }
        let max_rows = self.columns.batch_rows();
        let mut out = Filled::default();
        let rows = loop {
            let (window, last) = self.source.window();
            let (specs, buffers) = self.columns.for_core();
            // SAFETY: every buffer has room for `max_rows` values of its column's door
            // and as many verdicts, which is what `Columns` allocated.
            let code = unsafe {
                fill::fill(
                    &mut self.state,
                    window,
                    last,
                    specs,
                    buffers,
                    max_rows,
                    &mut self.cells,
                    &mut self.arena,
                    &mut out,
                )
            };
            match code {
                OK if out.rows > 0 => break out.rows as usize,
                OK => {
                    let (rest, consumed) = (window.len(), out.consumed as usize);
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        return Ok(None);
                    }
                    if consumed == 0
                        && let Err(error) = self.source.refill(&self.state)
                    {
                        return Err(self.fail(error));
                    }
                }
                // The core says how much one row takes; the table holds a batch of them.
                ERR_CELLS => {
                    self.per_row = self.per_row.max(out.needed as usize);
                    self.cells
                        .resize(out.needed as usize * max_rows, Span::default());
                }
                ERR_ARENA => {
                    let needed = (out.needed as usize).max(self.arena.len() * 2);
                    self.arena.resize(needed, 0);
                }
                ERR_STRUCTURE => return Err(self.fail(out.failure.into())),
                _ => return Err(self.fail(Error::Separator(self.dialect.separator))),
            }
        };
        // The batch's text is the input it was read from, which stays where it is until
        // the next read.
        self.pending = out.consumed as usize;
        self.cramped = rows < max_rows && out.arena_used as usize * 2 >= self.arena.len();
        let (window, _) = self.source.window();
        Ok(Some(Batch::new(
            &self.columns,
            rows,
            &self.cells,
            self.per_row,
            window,
            &self.arena,
            Origin::Delimited,
        )))
    }
}
