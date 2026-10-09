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

use crate::batch::{Batch, Columns, Origin, Row};
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

    /// Opens the file at `path`, to be read through `plan`.
    pub fn open(
        self,
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'static>, Error> {
        self.from_reader(File::open(path)?, dialect, plan)
    }

    /// Reads `bytes` in place through `plan`: nothing is copied, and a batch's text is
    /// `bytes` itself.
    pub fn from_slice<'a>(
        self,
        bytes: &'a [u8],
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        DelimitedReader::build(Source::Slice { bytes, at: 0 }, dialect, Some(plan), self)
    }

    /// Reads from any byte source through `plan`.
    pub fn from_reader<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        DelimitedReader::build(self.stream(reader), dialect, Some(plan), self)
    }

    /// Opens the file at `path` and reads its header, leaving the plan to be bound
    /// ([`DelimitedReader::bind`]) once the header has said where each column is.
    pub fn open_unbound(
        self,
        path: impl AsRef<Path>,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'static>, Error> {
        self.from_reader_unbound(File::open(path)?, dialect)
    }

    /// [`DelimitedOptions::from_slice`], with the plan bound later: see
    /// [`DelimitedOptions::open_unbound`].
    pub fn from_slice_unbound(
        self,
        bytes: &[u8],
        dialect: Dialect,
    ) -> Result<DelimitedReader<'_>, Error> {
        DelimitedReader::build(Source::Slice { bytes, at: 0 }, dialect, None, self)
    }

    /// [`DelimitedOptions::from_reader`], with the plan bound later: see
    /// [`DelimitedOptions::open_unbound`].
    pub fn from_reader_unbound<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'a>, Error> {
        DelimitedReader::build(self.stream(reader), dialect, None, self)
    }

    /// Reads from an asynchronous byte source — the header as this is awaited, and each
    /// batch as [`DelimitedReader::read_async`] is — with the plan bound later: see
    /// [`DelimitedOptions::open_unbound`]. Any runtime's reader serves through
    /// [`futures_io::AsyncRead`] (Tokio's through `tokio_util::compat`).
    #[cfg(feature = "async")]
    pub async fn from_async_reader<'a, R: futures_io::AsyncRead + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'a>, Error> {
        let source = Source::Async {
            buf: vec![0; self.buffer_bytes],
            at: 0,
            end: 0,
            reader: Box::pin(reader),
            eof: false,
        };
        let mut reader = DelimitedReader::start(source, dialect, None, self)?;
        if dialect.has_header {
            let mut names = vec![Span::default(); 64];
            while !reader.header_step(&mut names)? {
                reader.refill_async().await?;
            }
        }
        Ok(reader)
    }

    fn stream<'a, R: Read + Send + 'a>(self, reader: R) -> Source<'a> {
        Source::Stream {
            buf: Vec::with_capacity(self.buffer_bytes),
            at: 0,
            reader: Box::new(reader),
            eof: false,
        }
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
    /// An asynchronous stream, read through a buffer that is all initialized, `end` bytes
    /// of it read: each read is counted the moment it lands, so a read that is abandoned
    /// part-way (its future dropped) loses nothing, and the next one goes on from there.
    #[cfg(feature = "async")]
    Async {
        buf: Vec<u8>,
        at: usize,
        end: usize,
        reader: std::pin::Pin<Box<dyn futures_io::AsyncRead + Send + 'a>>,
        eof: bool,
    },
}

/// What one turn of the core made of the input it was given.
enum Step {
    /// A batch of this many rows.
    Rows(usize),
    /// The input is over.
    End,
    /// The core needs more input than is buffered.
    Input,
}

impl Source<'_> {
    /// The unconsumed input, and whether it is the last of it.
    fn window(&self) -> (&[u8], bool) {
        match self {
            Source::Slice { bytes, at } => (&bytes[*at..], true),
            Source::Stream { buf, at, eof, .. } => (&buf[*at..], *eof),
            #[cfg(feature = "async")]
            Source::Async {
                buf, at, end, eof, ..
            } => (&buf[*at..*end], *eof),
        }
    }

    fn advance(&mut self, consumed: usize) {
        match self {
            Source::Slice { at, .. } | Source::Stream { at, .. } => *at += consumed,
            #[cfg(feature = "async")]
            Source::Async { at, .. } => *at += consumed,
        }
    }

    /// Whether the source can only be read by awaiting it.
    fn is_async(&self) -> bool {
        #[cfg(feature = "async")]
        if let Source::Async { .. } = self {
            return true;
        }
        false
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
                return Err(row_too_long(state));
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

    /// [`Source::refill`] for an asynchronous stream, and the same for any other.
    #[cfg(feature = "async")]
    async fn refill_async(&mut self, state: &State) -> Result<(), Error> {
        let Source::Async {
            buf,
            at,
            end,
            reader,
            eof,
        } = self
        else {
            return self.refill(state);
        };
        buf.copy_within(*at..*end, 0);
        *end -= *at;
        *at = 0;
        if *end == buf.len() {
            if buf.len() >= DelimitedReader::MAX_ROW_BYTES {
                return Err(row_too_long(state));
            }
            buf.resize((buf.len() * 2).max(1), 0);
        }
        while *end < buf.len() {
            let read = std::future::poll_fn(|cx| reader.as_mut().poll_read(cx, &mut buf[*end..]));
            match read.await {
                Ok(0) => {
                    *eof = true;
                    break;
                }
                Ok(n) => *end += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

/// The failure of a record longer than a stream is buffered for.
fn row_too_long(state: &State) -> Error {
    Error::Structure(Failure {
        kind: FailureKind::RowTooLong,
        // The offending record's index: how many were read whole before it.
        record: state.records,
        line: state.line,
        byte: state.offset,
        expected: 0,
        found: 0,
    })
}

/// A forward-only reader of delimited text. See the module documentation.
pub struct DelimitedReader<'a> {
    dialect: Dialect,
    options: DelimitedOptions,
    state: State,
    source: Source<'a>,
    /// Bytes the batch last lent covers: consumed when the next read begins.
    pending: usize,
    /// The plan's columns, once one is bound.
    columns: Option<Columns>,
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

    /// Opens the file at `path`, to be read through `plan`.
    pub fn open(
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'static>, Error> {
        DelimitedOptions::new().open(path, dialect, plan)
    }

    /// Reads `bytes` in place through `plan`: nothing is copied, and a batch's text is
    /// `bytes` itself.
    pub fn from_slice(bytes: &'a [u8], dialect: Dialect, plan: &[Column]) -> Result<Self, Error> {
        DelimitedOptions::new().from_slice(bytes, dialect, plan)
    }

    /// Reads from any byte source through `plan`.
    pub fn from_reader<R: Read + Send + 'a>(
        reader: R,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<Self, Error> {
        DelimitedOptions::new().from_reader(reader, dialect, plan)
    }

    /// Opens the file at `path` and reads its header, leaving the plan to be bound
    /// ([`DelimitedReader::bind`]) once the header has said where each column is:
    ///
    /// ```no_run
    /// # use hypertabular::{Column, DelimitedReader, Dialect};
    /// let mut reader = DelimitedReader::open_unbound("countries.csv", Dialect::CSV)?;
    /// let header = reader.header().unwrap();
    /// let plan = [
    ///     Column::i32(header.require("M49 Code")?),
    ///     Column::text(header.require("ISO-alpha2 Code")?),
    /// ];
    /// reader.bind(&plan)?;
    /// while let Some(batch) = reader.read()? { /* ... */ }
    /// # Ok::<(), hypertabular::Error>(())
    /// ```
    pub fn open_unbound(
        path: impl AsRef<Path>,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'static>, Error> {
        DelimitedOptions::new().open_unbound(path, dialect)
    }

    /// [`DelimitedReader::from_slice`], with the plan bound later: see
    /// [`DelimitedReader::open_unbound`].
    pub fn from_slice_unbound(bytes: &'a [u8], dialect: Dialect) -> Result<Self, Error> {
        DelimitedOptions::new().from_slice_unbound(bytes, dialect)
    }

    /// [`DelimitedReader::from_reader`], with the plan bound later: see
    /// [`DelimitedReader::open_unbound`].
    pub fn from_reader_unbound<R: Read + Send + 'a>(
        reader: R,
        dialect: Dialect,
    ) -> Result<Self, Error> {
        DelimitedOptions::new().from_reader_unbound(reader, dialect)
    }

    /// Reads from an asynchronous byte source, with the plan bound later: see
    /// [`DelimitedOptions::from_async_reader`].
    #[cfg(feature = "async")]
    pub async fn from_async_reader<R: futures_io::AsyncRead + Send + 'a>(
        reader: R,
        dialect: Dialect,
    ) -> Result<Self, Error> {
        DelimitedOptions::new()
            .from_async_reader(reader, dialect)
            .await
    }

    /// A reader with its state initialized and, given a plan, bound to it — its header
    /// not yet read.
    fn start(
        source: Source<'a>,
        dialect: Dialect,
        plan: Option<&[Column]>,
        options: DelimitedOptions,
    ) -> Result<Self, Error> {
        let raw = RawDialect {
            separator: dialect.separator,
            quoting: u8::from(dialect.quoting),
            skip_blank_lines: u8::from(dialect.skip_blank_lines),
            engine: 0,
        };
        let state = State::init(raw).ok_or(Error::Separator(dialect.separator))?;
        let mut reader = DelimitedReader {
            dialect,
            options,
            state,
            source,
            pending: 0,
            columns: None,
            per_row: 0,
            cells: Vec::new(),
            arena: vec![0; 4096],
            cramped: false,
            header: None,
            error: None,
        };
        if let Some(plan) = plan {
            reader.bind(plan)?;
        }
        Ok(reader)
    }

    fn build(
        source: Source<'a>,
        dialect: Dialect,
        plan: Option<&[Column]>,
        options: DelimitedOptions,
    ) -> Result<Self, Error> {
        let mut reader = DelimitedReader::start(source, dialect, plan, options)?;
        if dialect.has_header {
            let mut names = vec![Span::default(); 64];
            while !reader.header_step(&mut names)? {
                reader.refill()?;
            }
        }
        Ok(reader)
    }

    /// Declares the plan a reader opened without one reads through — once, before the
    /// first read. Every column array, the cell table and the batch are sized here, by the
    /// plan and the reader's batch size.
    ///
    /// # Errors
    /// [`Error::AlreadyBound`] if the reader has a plan; [`Error::Plan`] if a column cannot
    /// be honoured.
    pub fn bind(&mut self, plan: &[Column]) -> Result<(), Error> {
        if self.columns.is_some() {
            return Err(Error::AlreadyBound);
        }
        let (specs, width) = column::specs(plan)?;
        let batch_rows = self.options.batch_rows;
        self.per_row = width + 1;
        self.cells = vec![Span::default(); self.per_row * batch_rows];
        self.columns = Some(Columns::new(plan, specs, batch_rows));
        Ok(())
    }

    /// Whether a plan has been bound: always, for a reader opened with one.
    pub fn is_bound(&self) -> bool {
        self.columns.is_some()
    }

    /// The dialect the reader was built with.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The plan the reader reads through: empty until one is bound.
    pub fn plan(&self) -> &[Column] {
        self.columns.as_ref().map_or(&[], Columns::plan)
    }

    /// The header record's names, or `None` if the dialect declares no header. A header
    /// with no names is an input that had no record.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// How many cells a record has: the header's count once it has been read, otherwise
    /// the first record's once it has been.
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

    /// Reads more of a stream, a failure to being the reader's last word.
    fn refill(&mut self) -> Result<(), Error> {
        match self.source.refill(&self.state) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }

    #[cfg(feature = "async")]
    async fn refill_async(&mut self) -> Result<(), Error> {
        match self.source.refill_async(&self.state).await {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }

    /// Reads the header from what is buffered: `true` once it has been read, `false` if the
    /// core needs more input first.
    fn header_step(&mut self, names: &mut Vec<Span>) -> Result<bool, Error> {
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window();
            let code = fill::header(
                &mut self.state,
                window,
                last,
                names,
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
                    return Ok(true);
                }
                OK => {
                    let (rest, consumed) = (window.len(), out.consumed as usize);
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        // An empty input has no header and no rows; the width is unknown.
                        self.header = Some(Header::new());
                        return Ok(true);
                    }
                    if consumed == 0 {
                        return Ok(false);
                    }
                }
                ERR_CELLS => names.resize(out.needed as usize, Span::default()),
                ERR_ARENA => self.arena.resize(out.needed as usize, 0),
                ERR_STRUCTURE => return Err(self.fail(out.failure.into())),
                _ => return Err(self.fail(Error::Separator(self.dialect.separator))),
            }
        }
    }

    /// What every read does first: gives back the batch lent last, and says whether there
    /// is anything to read through.
    fn begin(&mut self) -> Result<(), Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.columns.is_none() {
            return Err(Error::Unbound);
        }
        self.source.advance(std::mem::take(&mut self.pending));
        // The core stops a batch at the row the arena has no room for, so an arena too
        // small for a batch's unescaped text makes for short batches, not for an error.
        // The batch that was short has been given back by now, and the arena can move.
        if std::mem::take(&mut self.cramped) {
            self.arena.resize(self.arena.len() * 2, 0);
        }
        Ok(())
    }

    /// Fills a batch from what is buffered.
    fn fill_step(&mut self) -> Result<Step, Error> {
        let Some(columns) = self.columns.as_mut() else {
            return Err(Error::Unbound);
        };
        let max_rows = columns.batch_rows();
        let mut out = Filled::default();
        loop {
            let (window, last) = self.source.window();
            let (specs, buffers) = columns.for_core();
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
                OK if out.rows > 0 => {
                    let rows = out.rows as usize;
                    // The batch's text is the input it was read from, which stays where it
                    // is until the next read.
                    self.pending = out.consumed as usize;
                    self.cramped =
                        rows < max_rows && out.arena_used as usize * 2 >= self.arena.len();
                    return Ok(Step::Rows(rows));
                }
                OK => {
                    let (rest, consumed) = (window.len(), out.consumed as usize);
                    self.source.advance(consumed);
                    if last && (consumed == rest || consumed == 0) {
                        return Ok(Step::End);
                    }
                    if consumed == 0 {
                        return Ok(Step::Input);
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
        }
    }

    /// The batch the last fill wrote.
    fn batch(&self, rows: usize) -> Batch<'_> {
        let (window, _) = self.source.window();
        Batch::new(
            self.columns.as_ref().expect("a filled reader is bound"),
            rows,
            &self.cells,
            self.per_row,
            window,
            &self.arena,
            Origin::Delimited,
        )
    }

    /// Reads the next batch: up to the reader's batch size of rows, or `None` when the
    /// input has no more. The batch borrows the reader, and is over when the next is
    /// asked for.
    ///
    /// A structural failure is returned once every intact row before it has been
    /// delivered, and again on every call after. A reader opened without a plan returns
    /// [`Error::Unbound`] until one is bound, and one opened on an asynchronous source is
    /// read with [`DelimitedReader::read_async`].
    pub fn read(&mut self) -> Result<Option<Batch<'_>>, Error> {
        if self.source.is_async() {
            return Err(Error::Io(std::sync::Arc::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "an asynchronous source is read with read_async",
            ))));
        }
        self.begin()?;
        loop {
            match self.fill_step()? {
                Step::Rows(rows) => return Ok(Some(self.batch(rows))),
                Step::End => return Ok(None),
                Step::Input => self.refill()?,
            }
        }
    }

    /// [`DelimitedReader::read`], awaiting the source whenever the core needs more of it:
    /// for a reader opened with [`DelimitedReader::from_async_reader`], and the same as
    /// `read` for any other.
    ///
    /// Cancel-safe: a read whose future is dropped part-way loses nothing — what it had
    /// read stays buffered — and the next read goes on from there.
    #[cfg(feature = "async")]
    pub async fn read_async(&mut self) -> Result<Option<Batch<'_>>, Error> {
        self.begin()?;
        loop {
            match self.fill_step()? {
                Step::Rows(rows) => return Ok(Some(self.batch(rows))),
                Step::End => return Ok(None),
                Step::Input => self.refill_async().await?,
            }
        }
    }

    /// Reads every row left, batch by batch, handing each to `f` — the row view
    /// ([`Row`](crate::Row)) of a reader that cannot be an iterator, since each batch is
    /// over when the next is read. Stops at the first error, `f`'s or the reader's.
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
}
