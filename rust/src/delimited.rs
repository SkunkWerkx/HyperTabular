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
//!
//! A dialect that declares an [`Encoding`] adds one step ahead of the fill: a file's bytes go
//! through the core's transcoder into the buffer the fill reads, and from there on the reader
//! is the same reader, its text being the decoded text.

use crate::batch::{Batch, Columns, Home, Row};
use crate::column::{self, zeroed};
use crate::error::{Failure, FailureKind};
use crate::kernel::abi::{
    ColumnSpecWith, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Filled, OK, RawBatchCheck,
    RawDelimitedOptions, RawTranscoded, Span,
};
use crate::kernel::delimited::fill::{self, State};
use crate::kernel::size;
use crate::kernel::transcode::transcode;
use crate::suggest::{Sample, SampleSource, SuggestOptions, Suggestion, check_width};
use crate::{Column, Error, Header};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// What a record with fewer cells than the width is.
///
/// New policies may be added in a minor release. Matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShortRows {
    /// A structural failure.
    #[default]
    Reject,
    /// A row whose missing cells are `Empty`, at [`Origin::Padded`](crate::Origin::Padded).
    Pad,
}

/// What a record with more cells than the width is.
///
/// New policies may be added in a minor release. Matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LongRows {
    /// A structural failure.
    #[default]
    Reject,
    /// A row cut to the width, and listed by [`Batch::faults`](crate::Batch::faults) with the
    /// width it had.
    Report,
    /// A row cut to the width, and nothing said.
    Truncate,
}

/// What a file's bytes are, declared and never sniffed.
///
/// Anything but [`Encoding::Utf8`] is decoded ahead of the read, so that everything a batch says,
/// its spans included, is about the decoded text. A unit that has no character is a structural
/// failure, [`FailureKind::Encoding`], whose `byte` alone counts the original bytes: nothing is
/// ever replaced. A workbook declares its own encoding, so this is delimited text's alone.
///
/// New encodings may be added in a minor release. Matches outside this crate need a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encoding {
    /// UTF-8, read without a copy. A cell that isn't valid is that cell's `Malformed` verdict.
    #[default]
    Utf8,
    /// Windows-1252. The five bytes the code page leaves undefined are their C1 controls, as
    /// WHATWG has them. `EF BB BF` is three characters here, not a byte-order mark.
    Windows1252,
    /// Latin-1 (ISO-8859-1): every byte the code point of its value.
    Latin1,
    /// UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code
    /// unit at the end of the input, is the failure.
    Utf16Le,
}

impl Encoding {
    /// The core's number for an encoding it has to decode.
    const fn code(self) -> Option<u32> {
        match self {
            Encoding::Utf8 => None,
            Encoding::Windows1252 => Some(RawTranscoded::WINDOWS_1252),
            Encoding::Latin1 => Some(RawTranscoded::LATIN_1),
            Encoding::Utf16Le => Some(RawTranscoded::UTF_16LE),
        }
    }
}

/// The caller-declared dialect. The same philosophy as HyperCast's `NumFormat`: no
/// sniffing, no guessing — the separator is stated, quoting is stated, the header is
/// stated.
///
/// Start with [`Dialect::new`] or a named constant and change settings with the
/// `with_*` methods. Additional settings may be added in a minor release.
///
/// ```compile_fail,E0639
/// use hypertabular::Dialect;
/// let dialect = Dialect { separator: b',', quoting: true,
///     has_header: true, skip_blank_lines: true };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// Physical lines to pass over after a byte-order mark, before anything is read of them: a
    /// quote in one means nothing.
    pub skip: u32,
    /// Records to discard after that, ahead of the header if there is one. A blank line this
    /// dialect skips isn't a record.
    pub header_row: u32,
    /// What a record with fewer cells than the width is.
    pub short_rows: ShortRows,
    /// What a record with more cells than the width is.
    pub long_rows: LongRows,
    /// What the input's bytes are.
    pub encoding: Encoding,
    /// What stands for no value in every column without tokens of its own: packed.
    tokens: &'static [u8],
}

impl Dialect {
    /// Comma-separated, quoted, with a header.
    pub const CSV: Dialect = Dialect {
        separator: b',',
        quoting: true,
        has_header: true,
        skip_blank_lines: true,
        skip: 0,
        header_row: 0,
        short_rows: ShortRows::Reject,
        long_rows: LongRows::Reject,
        encoding: Encoding::Utf8,
        tokens: &[],
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

    /// The same dialect passing over this many physical lines first.
    pub const fn with_skip(self, skip: u32) -> Dialect {
        Dialect { skip, ..self }
    }

    /// The same dialect discarding this many records after those.
    pub const fn with_header_row(self, header_row: u32) -> Dialect {
        Dialect { header_row, ..self }
    }

    /// The same dialect with another policy for short records.
    pub const fn with_short_rows(self, short_rows: ShortRows) -> Dialect {
        Dialect { short_rows, ..self }
    }

    /// The same dialect with another policy for long records.
    pub const fn with_long_rows(self, long_rows: LongRows) -> Dialect {
        Dialect { long_rows, ..self }
    }

    /// The same dialect reading input in another encoding.
    pub const fn with_encoding(self, encoding: Encoding) -> Dialect {
        Dialect { encoding, ..self }
    }

    /// The same dialect with these tokens for no value: a cell whose exact, unquoted bytes are one
    /// of them is `Empty`, whatever its door, and [`Batch::origin`] says
    /// [`Origin::Token`](crate::Origin::Token). A column's own replace them
    /// ([`Column::tokens`]).
    ///
    /// Each distinct list is kept for the life of the process, so that a dialect can stay `Copy`: a
    /// program building lists from request data grows without bound.
    pub fn with_tokens(self, tokens: &[&str]) -> Dialect {
        Dialect {
            tokens: column::packed_tokens(tokens),
            ..self
        }
    }

    /// The dialect as the core's state takes it.
    const fn raw(&self) -> RawDelimitedOptions {
        RawDelimitedOptions {
            size: RawDelimitedOptions::SIZE,
            separator: self.separator,
            quoting: self.quoting as u8,
            skip_blank_lines: self.skip_blank_lines as u8,
            engine: 0,
            skip: self.skip,
            header_row: self.header_row,
            short_rows: match self.short_rows {
                ShortRows::Reject => RawDelimitedOptions::SHORT_ROWS_REJECT,
                ShortRows::Pad => RawDelimitedOptions::SHORT_ROWS_PAD,
            },
            long_rows: match self.long_rows {
                LongRows::Reject => RawDelimitedOptions::LONG_ROWS_REJECT,
                LongRows::Report => RawDelimitedOptions::LONG_ROWS_REPORT,
                LongRows::Truncate => RawDelimitedOptions::LONG_ROWS_TRUNCATE,
            },
        }
    }
}

/// How a reader sizes what it owns. [`DelimitedReader::options`] starts one; its
/// `open`, `from_slice` and `from_reader` finish it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelimitedOptions {
    batch_rows: usize,
    buffer_bytes: usize,
    notes: bool,
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
            notes: true,
        }
    }

    /// Whether a batch carries [`Batch::detail`] and [`Batch::origin`]: on unless this turns them
    /// off, when every cell says `None` and [`Origin::Cell`](crate::Origin::Cell) and a read that
    /// declares no tokens or date options runs the loop that doesn't ask a door for them.
    pub const fn notes(self, notes: bool) -> DelimitedOptions {
        DelimitedOptions { notes, ..self }
    }

    /// The most rows a batch holds. Every column's arrays are this long. Opening a reader refuses
    /// `0`, and a batch of the plan asking for more than the core allows
    /// ([`Error::ZeroBatchRows`], [`Error::BatchTooLarge`]).
    pub const fn batch_rows(self, rows: usize) -> DelimitedOptions {
        DelimitedOptions {
            batch_rows: rows,
            ..self
        }
    }

    /// The input buffer a file or a stream is read through to begin with. It grows to
    /// hold the longest record, up to [`DelimitedReader::MAX_ROW_BYTES`], so this is a
    /// starting size and not a limit, and a request past the ceiling starts there; a slice
    /// is read in place and has no buffer. Opening a reader refuses `0`
    /// ([`Error::ZeroBufferBytes`]), a slice's as well.
    pub const fn buffer_bytes(self, bytes: usize) -> DelimitedOptions {
        DelimitedOptions {
            buffer_bytes: bytes,
            ..self
        }
    }

    /// The core's answer on these sizes for `plan`, ahead of everything a reader allocates. A plan
    /// that can't be honoured says so where it always has, so this judges the sizes without it.
    pub(crate) fn checked(self, dialect: Dialect, plan: &[Column]) -> Result<RawBatchCheck, Error> {
        self.checked_specs(dialect, &column::specs_with(plan).unwrap_or_default())
    }

    fn checked_specs(
        self,
        dialect: Dialect,
        specs: &[ColumnSpecWith],
    ) -> Result<RawBatchCheck, Error> {
        column::check_sizes(
            specs,
            self.batch_rows,
            self.buffer_bytes,
            self.size_flags(dialect),
        )
    }

    fn size_flags(self, dialect: Dialect) -> u32 {
        let mut flags = 0;
        if self.notes {
            flags |= RawBatchCheck::NOTES;
        }
        if dialect.long_rows == LongRows::Report {
            flags |= RawBatchCheck::WIDTHS;
        }
        flags
    }

    /// Opens the file at `path`, to be read through `plan`.
    pub fn open(
        self,
        path: impl AsRef<Path>,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'static>, Error> {
        self.checked(dialect, plan)?;
        self.from_reader(File::open(path)?, dialect, plan)
    }

    /// Reads `bytes` in place through `plan`: nothing is copied, and a batch's text is
    /// `bytes` itself. A dialect that declares an [`Encoding`] has to be decoded, and reads
    /// them as a stream instead.
    pub fn from_slice<'a>(
        self,
        bytes: &'a [u8],
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        let sizes = self.checked(dialect, plan)?;
        DelimitedReader::build(
            self.slice(bytes, dialect, sizes)?,
            dialect,
            Some(plan),
            self,
        )
    }

    /// Reads from any byte source through `plan`.
    pub fn from_reader<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
        plan: &[Column],
    ) -> Result<DelimitedReader<'a>, Error> {
        let sizes = self.checked(dialect, plan)?;
        DelimitedReader::build(
            self.stream(reader, dialect, sizes)?,
            dialect,
            Some(plan),
            self,
        )
    }

    /// Opens the file at `path` and reads its header, leaving the plan to be bound
    /// ([`DelimitedReader::bind`]) once the header has said where each column is.
    pub fn open_unbound(
        self,
        path: impl AsRef<Path>,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'static>, Error> {
        self.checked(dialect, &[])?;
        self.from_reader_unbound(File::open(path)?, dialect)
    }

    /// [`DelimitedOptions::from_slice`], with the plan bound later: see
    /// [`DelimitedOptions::open_unbound`].
    pub fn from_slice_unbound(
        self,
        bytes: &[u8],
        dialect: Dialect,
    ) -> Result<DelimitedReader<'_>, Error> {
        let sizes = self.checked(dialect, &[])?;
        DelimitedReader::build(self.slice(bytes, dialect, sizes)?, dialect, None, self)
    }

    /// [`DelimitedOptions::from_reader`], with the plan bound later: see
    /// [`DelimitedOptions::open_unbound`].
    pub fn from_reader_unbound<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
    ) -> Result<DelimitedReader<'a>, Error> {
        let sizes = self.checked(dialect, &[])?;
        DelimitedReader::build(self.stream(reader, dialect, sizes)?, dialect, None, self)
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
        let sizes = self.checked(dialect, &[])?;
        let size = sizes.buffer_bytes as usize;
        let source = Source::Async {
            buf: zeroed(size).ok_or(Error::NoMemory {
                bytes: sizes.buffer_bytes,
            })?,
            at: 0,
            end: 0,
            reader: Box::pin(reader),
            eof: false,
            ceiling: DelimitedReader::MAX_ROW_BYTES,
            decoder: self.decoder(dialect, sizes)?,
        };
        let mut reader = DelimitedReader::start(source, dialect, None, self)?;
        if dialect.has_header {
            let mut names = vec![Span::default(); 64];
            while !reader.header_step(&mut names)? {
                reader.refill_async().await?;
            }
        }
        reader.within_header()?;
        Ok(reader)
    }

    fn slice<'a>(
        self,
        bytes: &'a [u8],
        dialect: Dialect,
        sizes: RawBatchCheck,
    ) -> Result<Source<'a>, Error> {
        match dialect.encoding {
            Encoding::Utf8 => Ok(Source::slice(bytes)),
            _ => self.stream(bytes, dialect, sizes),
        }
    }

    fn stream<'a, R: Read + Send + 'a>(
        self,
        reader: R,
        dialect: Dialect,
        sizes: RawBatchCheck,
    ) -> Result<Source<'a>, Error> {
        let limit = sizes.buffer_bytes as usize;
        let mut buf = Vec::new();
        buf.try_reserve_exact(limit).map_err(|_| Error::NoMemory {
            bytes: sizes.buffer_bytes,
        })?;
        Ok(Source::Stream {
            buf,
            at: 0,
            limit,
            reader: Box::new(reader),
            eof: false,
            ceiling: DelimitedReader::MAX_ROW_BYTES,
            decoder: self.decoder(dialect, sizes)?,
        })
    }

    /// What a dialect's encoding asks of a stream.
    fn decoder(self, dialect: Dialect, sizes: RawBatchCheck) -> Result<Option<Decoder>, Error> {
        match dialect.encoding.code() {
            Some(encoding) => Decoder::new(encoding, sizes.buffer_bytes as usize).map(Some),
            None => Ok(None),
        }
    }
}

/// The most bytes of UTF-8 one character takes: with this much room, a transcode that writes
/// nothing and doesn't fail wants more input.
const MAX_CHARACTER: usize = 4;

/// The step ahead of the fill, for a dialect that declares an encoding. A stream's reads land
/// here, as the original bytes, and go through the core's transcoder into the stream's buffer,
/// where the fill reads them as it would have read UTF-8.
struct Decoder {
    encoding: u32,
    /// The original bytes read, `at..end` of them not yet decoded. Fixed in size: what's left
    /// when more is wanted is less than a character.
    raw: Vec<u8>,
    at: usize,
    end: usize,
    /// The source has ended.
    eof: bool,
    /// Original bytes decoded so far: added to a call's `failure_at`, this is the input's offset.
    offset: u64,
    /// The original offset of a unit with no character. The failure is raised only when the fill
    /// has had all that decoded ahead of it, and wants more.
    failed_at: Option<u64>,
}

/// What one turn of the decoder came to.
enum Decoded {
    /// This many bytes of decoded text for the fill.
    Wrote(usize),
    /// The text ahead of a failure is all written.
    Failed,
    /// The source is over and decoded. The fill can be told it's the last.
    Finished,
    /// The next character wants more room than this.
    Room,
    /// A read's worth more of the source, please.
    Input,
}

impl Decoder {
    fn new(encoding: u32, size: usize) -> Result<Decoder, Error> {
        let size = size.max(MAX_CHARACTER);
        Ok(Decoder {
            encoding,
            raw: zeroed(size).ok_or(Error::NoMemory { bytes: size as u64 })?,
            at: 0,
            end: 0,
            eof: false,
            offset: 0,
            failed_at: None,
        })
    }

    /// Decodes what it has into `out`.
    fn step(&mut self, out: &mut [u8]) -> Decoded {
        let mut done = RawTranscoded::default();
        let code = transcode(
            self.encoding,
            &self.raw[self.at..self.end],
            self.eof,
            out,
            &mut done,
        );
        debug_assert_eq!(code, OK, "a code this type made");
        if done.failed != 0 {
            self.failed_at = Some(self.offset + done.failure_at);
        }
        self.at += done.consumed as usize;
        self.offset += done.consumed;
        if done.written > 0 {
            Decoded::Wrote(done.written as usize)
        } else if self.failed_at.is_some() {
            Decoded::Failed
        } else if self.at < self.end && out.len() < MAX_CHARACTER {
            Decoded::Room
        } else if self.eof {
            Decoded::Finished
        } else {
            Decoded::Input
        }
    }

    /// Where the next read of the source goes, after what's still to decode.
    fn room(&mut self) -> &mut [u8] {
        self.raw.copy_within(self.at..self.end, 0);
        self.end -= self.at;
        self.at = 0;
        &mut self.raw[self.end..]
    }

    /// Counts a read of the source into [`Decoder::room`].
    fn landed(&mut self, bytes: usize) {
        self.end += bytes;
        self.eof = bytes == 0;
    }
}

/// Where the bytes come from.
enum Source<'a> {
    /// The whole input, in memory, handed to the core at most `window` bytes a call.
    Slice {
        bytes: &'a [u8],
        at: usize,
        window: usize,
    },
    /// A stream, read through a buffer that holds what the core has not consumed. The
    /// buffer is filled to `limit`, which doubles when a record fills it, up to the
    /// ceiling ([`DelimitedReader::MAX_ROW_BYTES`], lowered only by a test).
    Stream {
        buf: Vec<u8>,
        at: usize,
        limit: usize,
        reader: Box<dyn Read + Send + 'a>,
        eof: bool,
        ceiling: usize,
        /// With a declared encoding, what the buffer's text is decoded by.
        decoder: Option<Decoder>,
    },
    /// An asynchronous stream, read through a buffer that is all initialized, `end` bytes
    /// of it read: each read is counted the moment it lands, so a read that is abandoned
    /// part-way (its future dropped) loses nothing, and the next one goes on from there.
    /// The buffer's length is the limit a synchronous stream keeps beside its buffer.
    #[cfg(feature = "async")]
    Async {
        buf: Vec<u8>,
        at: usize,
        end: usize,
        reader: std::pin::Pin<Box<dyn futures_io::AsyncRead + Send + 'a>>,
        eof: bool,
        ceiling: usize,
        /// With a declared encoding, what the buffer's text is decoded by.
        decoder: Option<Decoder>,
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

/// The most input the core takes in one call: its offsets are 31 bits.
const WINDOW: usize = i32::MAX as usize;

impl<'a> Source<'a> {
    fn slice(bytes: &'a [u8]) -> Source<'a> {
        Source::Slice {
            bytes,
            at: 0,
            window: WINDOW,
        }
    }

    /// The unconsumed input, and whether it is the last of it.
    fn window(&self) -> (&[u8], bool) {
        match self {
            Source::Slice { bytes, at, window } => {
                let rest = &bytes[*at..];
                match rest.get(..*window) {
                    Some(part) if rest.len() > *window => (part, false),
                    _ => (rest, true),
                }
            }
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
    /// growing the buffer if the tail fills it — up to the ceiling, which a record that
    /// fills without ending is too long, whatever buffer the reader began with. A slice
    /// is read in place, so there is nothing to make room in: one record is larger than
    /// the core takes in a call.
    fn refill(&mut self, state: &State) -> Result<(), Error> {
        let Source::Stream {
            buf,
            at,
            limit,
            reader,
            eof,
            ceiling,
            decoder,
        } = self
        else {
            return match self {
                Source::Slice { .. } => Err(row_too_long(state)),
                _ => Ok(()),
            };
        };
        buf.drain(..*at);
        *at = 0;
        if let Some(byte) = decoder.as_ref().and_then(|decoder| decoder.failed_at) {
            // All the text ahead of the failure has been read, but for what the fill can't
            // finish. A `\r` it's holding ends a row whatever comes next, and only waits to see
            // if a `\n` is part of it; nothing is coming, and this says so. In a quote it ends
            // nothing, and the record is as cut as it was.
            if buf.last() == Some(&b'\r') {
                buf.push(b'\n');
                return Ok(());
            }
            return Err(encoding_failure(state, byte));
        }
        if buf.len() >= *limit {
            if *limit >= *ceiling {
                return Err(row_too_long(state));
            }
            *limit = limit.saturating_mul(2).min(*ceiling);
        }
        let filled = buf.len();
        buf.resize(*limit, 0);
        if let Some(decoder) = decoder {
            loop {
                let written = match decoder.step(&mut buf[filled..]) {
                    Decoded::Wrote(written) => written,
                    // The next refill raises it, once the fill has had one more look.
                    Decoded::Failed => 0,
                    Decoded::Finished => {
                        *eof = true;
                        0
                    }
                    Decoded::Room => {
                        if *limit >= *ceiling {
                            buf.truncate(filled);
                            return Err(row_too_long(state));
                        }
                        *limit = limit.saturating_mul(2).min(*ceiling);
                        buf.resize(*limit, 0);
                        continue;
                    }
                    Decoded::Input => {
                        match reader.read(decoder.room()) {
                            Ok(bytes) => decoder.landed(bytes),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(error) => {
                                buf.truncate(filled);
                                return Err(error.into());
                            }
                        }
                        continue;
                    }
                };
                buf.truncate(filled + written);
                return Ok(());
            }
        }
        loop {
            match reader.read(&mut buf[filled..]) {
                Ok(0) => {
                    *eof = true;
                    buf.truncate(filled);
                    break;
                }
                Ok(n) => {
                    buf.truncate(filled + n);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    buf.truncate(filled);
                    return Err(error.into());
                }
            }
        }
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
            ceiling,
            decoder,
        } = self
        else {
            return self.refill(state);
        };
        buf.copy_within(*at..*end, 0);
        *end -= *at;
        *at = 0;
        if let Some(byte) = decoder.as_ref().and_then(|decoder| decoder.failed_at) {
            // As in `Source::refill`: nothing is coming to end a `\r` the fill is holding.
            if *end > 0 && buf[*end - 1] == b'\r' {
                if *end == buf.len() {
                    buf.push(0);
                }
                buf[*end] = b'\n';
                *end += 1;
                return Ok(());
            }
            return Err(encoding_failure(state, byte));
        }
        if *end >= buf.len() {
            if buf.len() >= *ceiling {
                return Err(row_too_long(state));
            }
            buf.resize(buf.len().saturating_mul(2).clamp(1, *ceiling), 0);
        }
        if let Some(decoder) = decoder {
            loop {
                match decoder.step(&mut buf[*end..]) {
                    Decoded::Wrote(written) => {
                        *end += written;
                        return Ok(());
                    }
                    // The next refill raises it, once the fill has had one more look.
                    Decoded::Failed => return Ok(()),
                    Decoded::Finished => {
                        *eof = true;
                        return Ok(());
                    }
                    Decoded::Room => {
                        if buf.len() >= *ceiling {
                            return Err(row_too_long(state));
                        }
                        buf.resize(buf.len().saturating_mul(2).clamp(1, *ceiling), 0);
                    }
                    Decoded::Input => {
                        let room = decoder.room();
                        let read =
                            std::future::poll_fn(|cx| reader.as_mut().poll_read(cx, &mut *room));
                        match read.await {
                            Ok(bytes) => decoder.landed(bytes),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
            }
        }
        loop {
            let read = std::future::poll_fn(|cx| reader.as_mut().poll_read(cx, &mut buf[*end..]));
            match read.await {
                Ok(0) => {
                    *eof = true;
                    break;
                }
                Ok(n) => {
                    *end += n;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "python"))]
impl DelimitedReader<'_> {
    /// Hands a slice to the core at most `bytes` a call from here on, as the reader does a
    /// slice of 2 GiB or more.
    pub(crate) fn limit_window(&mut self, bytes: usize) {
        if let Source::Slice { window, .. } = &mut self.source {
            *window = bytes;
        }
    }

    /// Lowers a stream's row ceiling from [`DelimitedReader::MAX_ROW_BYTES`] to `bytes`,
    /// so that the ceiling can be reached by a test: a buffer that began larger is cut to
    /// it. For a reader that has read nothing yet.
    pub(crate) fn limit_ceiling(&mut self, bytes: usize) {
        match &mut self.source {
            Source::Stream {
                buf,
                limit,
                ceiling,
                ..
            } => {
                *ceiling = bytes;
                *limit = (*limit).min(bytes);
                buf.truncate(*limit);
            }
            #[cfg(feature = "async")]
            Source::Async { buf, ceiling, .. } => {
                *ceiling = bytes;
                buf.truncate(bytes);
            }
            Source::Slice { .. } => {}
        }
    }

    /// The cell-table entries the reader holds.
    #[cfg(test)]
    pub(crate) fn cell_entries(&self) -> usize {
        self.cells.len()
    }
}

/// The failure of a record longer than a stream is buffered for, or than the core takes in
/// one call from a slice.
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

/// The failure of a unit a declared encoding has no character for: the record it cut, and where
/// the unit is in the original input.
fn encoding_failure(state: &State, byte: u64) -> Error {
    Error::Structure(Failure {
        kind: FailureKind::Encoding,
        record: state.records,
        line: state.line,
        byte,
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

impl SampleSource for DelimitedReader<'_> {
    fn next_batch(&mut self) -> Result<Option<Batch<'_>>, Error> {
        self.read()
    }
}

impl<'a> DelimitedReader<'a> {
    /// The most a stream is buffered for one record, whatever buffer the reader began
    /// with: a record that fills this much and has not ended is
    /// [`FailureKind::RowTooLong`], the input taken to be something other than delimited
    /// text.
    pub const MAX_ROW_BYTES: usize = size::MAX_ROW_BYTES as usize;

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
    /// `bytes` itself. A dialect that declares an [`Encoding`] has to be decoded, and reads
    /// them as a stream instead.
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

    /// Reads a sample of the file at `path` through the text door and says what its columns look
    /// like: a plan to look at, change and pass to a read, and a report. Nothing is sniffed in a
    /// read, and nothing here reaches one: this is its own read of the file, opened and closed
    /// here. There's no form of this for a stream, which would lose the rows it sampled.
    ///
    /// The width is the header's, or the first record's, and a plan has to name the columns before
    /// that record can be read. So with no header the file is opened twice, once to read the first
    /// record: a path that can only be read once, as a FIFO's can, is not for this. The dialect is
    /// what the sample is read with, its tokens included, and a cell it says is empty is blank to
    /// the suggestion.
    ///
    /// Declaring the locale is the recommended route: a number format and a date order are the
    /// plan's to say, and with them nothing needs inferring. This is a starting point for a plan to
    /// inspect.
    ///
    /// With no format declared, `,` and `.` follow from which of two notations reads the column:
    /// two of one delimiter group digits (`1,234,567`, `1.234.567`); one of each is a group then a
    /// decimal (`1,234.5`, `1.234,5`); one with 1, 2 or more than 3 digits after it is a decimal
    /// (`2,5`, `2.5`, `2,5000`). One with exactly 3 digits after it is ambiguous (`1,234`, `1.234`),
    /// unless a zero is what's left of it (`0,125` and `0.125` are 0.125), or another cell settles
    /// it (`1,234` beside `2,5` is a comma decimal). A comma-decimal column is resolved in that
    /// notation, and [`SuggestedColumn::format`] names it. `2,5` beside `2.5` is unresolved.
    ///
    /// What it says can surprise, so read the report before the plan:
    ///
    /// - A column of `0` and `1` is bool, the first door asked, and `F` or `No` read `false`.
    /// - A column of prices at three decimals, `1.234`, is ambiguous and reads as text until a
    ///   format is declared or a value settles it, because no comma doesn't establish a locale.
    /// - `1,23,456`, Indian grouping, reads under neither notation with none declared, because
    ///   nothing says its culture: alone it's text, resolved, and beside `5` unresolved. Declaring
    ///   `NumFormat::INVARIANT.with_grouping(Grouping::Indian)` for the column makes it a number.
    /// - `007` and an integer past `i64` are text, with no note saying why, so as not to lose the
    ///   zeros or the digits.
    /// - A sheet's date column is text: [`Sheet::suggest`](crate::Sheet::suggest) says why.
    ///
    /// # Errors
    /// [`Error::Plan`] first, before the file is opened, for any format declared in `options` the
    /// core would refuse. Then what opening the file can return, and [`Error::Structure`] for a
    /// failure among the sampled rows: there's
    /// no knowing what's past it, and a read would stop there too. A failure past them isn't met,
    /// or only says the source was [`Suggestion::truncated`].
    pub fn suggest(
        path: impl AsRef<Path>,
        dialect: Dialect,
        options: &SuggestOptions,
    ) -> Result<Suggestion, Error> {
        let path = path.as_ref();
        Self::suggest_from(dialect, options, |rows, dialect| {
            DelimitedOptions::new()
                .notes(false)
                .batch_rows(rows)
                .open_unbound(path, dialect)
        })
    }

    /// [`DelimitedReader::suggest`], over bytes.
    pub fn suggest_slice(
        bytes: &[u8],
        dialect: Dialect,
        options: &SuggestOptions,
    ) -> Result<Suggestion, Error> {
        Self::suggest_from(dialect, options, |rows, dialect| {
            DelimitedOptions::new()
                .notes(false)
                .batch_rows(rows)
                .from_slice_unbound(bytes, dialect)
        })
    }

    pub(crate) fn suggest_from<'s>(
        dialect: Dialect,
        options: &SuggestOptions,
        open: impl Fn(usize, Dialect) -> Result<DelimitedReader<'s>, Error>,
    ) -> Result<Suggestion, Error> {
        options.check_formats()?;
        let limit = options.rows_within(DelimitedOptions::new().batch_rows);
        // A headerless source's width is its first record's, which a read says through a plan of
        // one column, whose cells are verdicts as the sample's will be.
        let width = if dialect.has_header {
            None
        } else {
            let mut first = open(1, dialect)?;
            first.bind(&[Column::text(0)])?;
            first.read()?;
            Some(first.column_count().unwrap_or(0))
        };
        let mut reader = open(1, dialect)?;
        let width = width.unwrap_or_else(|| reader.header().map_or(0, Header::len));
        check_width(width)?;
        let plan: Vec<Column> = (0..width).map(Column::text).collect();
        if width != 0 {
            // One row more than the sample, so that a row past it costs no second batch to find, in
            // as many batches as the core says that takes.
            reader.options.batch_rows = column::sample_rows(
                &column::specs_with(&plan)?,
                limit + 1,
                reader.options.size_flags(dialect),
            )?;
            reader.bind(&plan)?;
        }
        let mut sample = Sample::new(width, limit);
        if width != 0 {
            sample.read_from(&mut reader)?;
        }
        sample.judge(reader.header(), options)
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
        let state = State::init_with(&dialect.raw()).ok_or(Error::Separator(dialect.separator))?;
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
        reader.within_header()?;
        Ok(reader)
    }

    /// A plan bound before the header was read, held to the width the header gives every
    /// record, as [`DelimitedReader::bind`] holds a plan bound after it.
    fn within_header(&self) -> Result<(), Error> {
        column::within(self.plan(), self.column_count())
    }

    /// Declares the plan a reader opened without one reads through — once, before the
    /// first read. Every column array, the cell table and the batch are sized here, by the
    /// plan and the reader's batch size.
    ///
    /// # Errors
    /// [`Error::AlreadyBound`] if the reader has a plan; [`Error::Plan`] if a column cannot
    /// be honoured — among them a column at or past the width a header has given every
    /// record.
    pub fn bind(&mut self, plan: &[Column]) -> Result<(), Error> {
        if self.columns.is_some() {
            return Err(Error::AlreadyBound);
        }
        let (_, width, widest) = column::specs(plan)?;
        let specs = column::specs_with(plan)?;
        column::within(plan, self.column_count())?;
        self.options.checked_specs(self.dialect, &specs)?;
        let batch_rows = self.options.batch_rows;
        let per_row = width + 1;
        let rows = column::cell_rows(per_row, batch_rows);
        self.cells = column::table(per_row.checked_mul(rows), widest)?;
        self.per_row = per_row;
        self.columns = Some(Columns::with_options(
            plan,
            specs,
            batch_rows,
            self.options.notes,
            self.dialect.long_rows == LongRows::Report,
        )?);
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

    /// How many records have been read, the header and any discarded among them; the lines
    /// [`Dialect::skip`] passes over aren't records.
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
            let (specs, buffers, widths) = columns.for_core_with();
            // SAFETY: every buffer has room for `max_rows` values of its column's door
            // and as many verdicts and notes, which is what `Columns` allocated; the specs'
            // lists are interned.
            let code = unsafe {
                fill::fill_with(
                    &mut self.state,
                    window,
                    last,
                    specs,
                    buffers,
                    max_rows,
                    &mut self.cells,
                    &mut self.arena,
                    self.dialect.tokens,
                    widths,
                    &mut out,
                )
            };
            match code {
                OK if out.rows > 0 => {
                    let rows = out.rows as usize;
                    // The batch's text is the input it was read from, which stays where it
                    // is until the next read.
                    self.pending = out.consumed as usize;
                    // A batch the cell table ends early (see `size::CELL_BUDGET`) is no
                    // reason to grow the arena.
                    let held = max_rows.min(self.cells.len() / self.per_row);
                    self.cramped = rows < held && out.arena_used as usize * 2 >= self.arena.len();
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
                // The core says how much one row takes; the table holds as many of them
                // as a batch has, or as the budget allows.
                ERR_CELLS => {
                    let needed = out.needed as usize;
                    self.per_row = self.per_row.max(needed);
                    let rows = column::cell_rows(needed, max_rows);
                    self.cells.resize(needed * rows, Span::default());
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
            Home::Delimited {
                reported_width: (self.dialect.long_rows == LongRows::Report)
                    .then_some(self.state.expected),
            },
        )
    }

    /// Reads the next batch: up to the reader's batch size of rows, or `None` when the
    /// input has no more. The batch borrows the reader, and is over when the next is
    /// asked for.
    ///
    /// A structural failure is returned once every intact row before it has been
    /// delivered, and again on every call after, a declared [`Encoding`]'s among them. A reader opened without a plan returns
    /// [`Error::Unbound`] until one is bound, and one opened on an asynchronous source is
    /// read with `read_async` (available with the `async` feature).
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
    /// ([`Row`]) of a reader that cannot be an iterator, since each batch is
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
