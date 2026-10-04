//! The forward-only reader: buffer management, row delivery, header, column-count
//! discipline, and the [`crate::TabularSource`] implementation.
//!
//! A slice source is scanned in place — no copy ever. A `Read` source owns one growable
//! buffer: the scanner is run over what has been read so far; when it reports an
//! unfinished row, that row is moved to the front, more bytes are read behind it, and
//! it is scanned again from its start (a row start is always a clean scanner state, so
//! nothing needs rebasing). A row that never completes doubles the buffer, up to
//! [`Reader::MAX_ROW_BYTES`].

use crate::delimited::dialect::Dialect;
use crate::delimited::error::Error;
use crate::delimited::scan::{Output, Scanner};
use crate::delimited::unescape::unescape;
use crate::{Cell, Header, TabularSource};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Rows scanned per call into the walker.
const ROWS_PER_SCAN: usize = 1024;

/// One delivered cell: where its bytes are (buffer or scratch) and how long.
#[derive(Clone, Copy, Debug)]
struct CellRef {
    start: u32,
    len: u32,
    scratch: bool,
}

enum Source<'a> {
    Slice(&'a [u8]),
    Stream {
        buf: Vec<u8>,
        reader: Box<dyn Read + 'a>,
        eof: bool,
    },
}

/// The forward-only cursor. See the module doc.
pub struct Reader<'a> {
    dialect: Dialect,
    scanner: Scanner,
    source: Source<'a>,
    /// Absolute offset of buffer byte 0.
    base: u64,
    /// Where the next scan starts (a row boundary).
    scan_pos: usize,
    /// One-based line number at `scan_pos`.
    line: u32,
    out: Output,
    row_cursor: usize,
    /// The current row's cells, resolved (quotes stripped, `""` collapsed).
    resolved: Vec<CellRef>,
    scratch: Vec<u8>,
    current_line: u32,
    current_record: u64,
    header: Option<Header>,
    expected: Option<usize>,
    /// Records delivered or consumed so far (header included).
    records: u64,
    /// An unclosed quote the scanner found past the rows it did deliver: raised once the
    /// cursor reaches it, so every intact row before it is still handed out.
    pending_unclosed: Option<(u32, u64)>,
    error: Option<Error>,
}

impl<'a> Reader<'a> {
    /// The row ceiling: a single record larger than this is a structural error.
    pub const MAX_ROW_BYTES: usize = 1 << 30;
    /// The initial buffer for `Read` sources.
    pub const DEFAULT_CAPACITY: usize = 256 * 1024;

    /// A reader over bytes already in memory — scanned in place, never copied. Reads the
    /// header record immediately if the dialect declares one.
    pub fn from_slice(bytes: &'a [u8], dialect: Dialect) -> Result<Reader<'a>, Error> {
        let bytes = strip_bom(bytes);
        Reader::build(dialect, Source::Slice(bytes))
    }

    /// A reader over a `Read`, buffering [`Reader::DEFAULT_CAPACITY`] bytes at a time.
    pub fn from_reader<R: Read + 'a>(reader: R, dialect: Dialect) -> Result<Reader<'a>, Error> {
        Reader::from_reader_with_capacity(reader, dialect, Reader::DEFAULT_CAPACITY)
    }

    /// A reader over a `Read` with a chosen initial buffer (the conformance suite uses
    /// tiny ones to exercise every refill path).
    pub fn from_reader_with_capacity<R: Read + 'a>(
        reader: R,
        dialect: Dialect,
        capacity: usize,
    ) -> Result<Reader<'a>, Error> {
        let source = Source::Stream {
            buf: Vec::with_capacity(capacity.max(64)),
            reader: Box::new(reader),
            eof: false,
        };
        Reader::build(dialect, source)
    }

    /// A reader over a file.
    pub fn from_path(path: impl AsRef<Path>, dialect: Dialect) -> Result<Reader<'static>, Error> {
        let file = File::open(path)?;
        Reader::from_reader(file, dialect)
    }

    fn build(dialect: Dialect, source: Source<'a>) -> Result<Reader<'a>, Error> {
        dialect.validate()?;
        let mut reader = Reader {
            dialect,
            scanner: Scanner::new(dialect.separator, dialect.quoting),
            source,
            base: 0,
            scan_pos: 0,
            line: 1,
            out: Output::default(),
            row_cursor: 0,
            resolved: Vec::new(),
            scratch: Vec::new(),
            current_line: 1,
            current_record: 0,
            header: None,
            expected: None,
            records: 0,
            pending_unclosed: None,
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

    /// The SIMD engine the scanner chose.
    pub fn engine(&self) -> crate::delimited::engine::Kind {
        self.scanner.engine()
    }

    /// The declared header, once read.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// The cell count every record must have, once the first record fixed it.
    pub fn column_count(&self) -> Option<usize> {
        self.expected
    }

    /// Records consumed so far, header included.
    pub fn records(&self) -> u64 {
        self.records
    }

    fn data(&self) -> &[u8] {
        match &self.source {
            Source::Slice(bytes) => bytes,
            Source::Stream { buf, .. } => buf,
        }
    }

    fn at_eof(&self) -> bool {
        match &self.source {
            Source::Slice(_) => true,
            Source::Stream { eof, .. } => *eof,
        }
    }

    fn read_header(&mut self) -> Result<(), Error> {
        if let Some(index) = self.advance()? {
            let row = self.out.rows[index];
            let mut header = Header::new();
            for j in 0..row.cells as usize {
                header.push(self.cell_bytes(self.resolved[j]));
            }
            self.header = Some(header);
        } else {
            // An empty input has no header and no rows; the width is unknown.
            self.header = Some(Header::new());
        }
        Ok(())
    }

    /// The next row, borrowed until the next call; `None` at the end. A structural error
    /// is sticky.
    pub fn next_row(&mut self) -> Result<Option<Row<'_, 'a>>, Error> {
        match self.advance()? {
            Some(index) => Ok(Some(Row {
                reader: self,
                index,
            })),
            None => Ok(None),
        }
    }

    /// Positions on the next deliverable row (blank lines skipped, cell count checked,
    /// cells resolved) and returns its index in `out.rows`.
    fn advance(&mut self) -> Result<Option<usize>, Error> {
        loop {
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            if self.row_cursor >= self.out.rows.len() {
                if self.pending_unclosed.is_none()
                    && let Err(error) = self.fill()
                {
                    self.error = Some(error.clone());
                    return Err(error);
                }
                if self.row_cursor >= self.out.rows.len() {
                    if let Some((line, byte)) = self.pending_unclosed.take() {
                        let error = Error::UnclosedQuote {
                            record: self.records,
                            line,
                            byte,
                        };
                        self.error = Some(error.clone());
                        return Err(error);
                    }
                    return Ok(None);
                }
            }
            let index = self.row_cursor;
            self.row_cursor += 1;
            let row = self.out.rows[index];
            let record = self.records;
            self.records += 1;
            let first = row.first_cell as usize;
            if row.cells == 1
                && self.out.cells[first].end == row.start
                && self.dialect.skip_blank_lines
                && row.terminator != 0
            {
                continue;
            }
            self.resolve(index);
            self.current_line = row.line;
            self.current_record = record;
            let found = row.cells as usize;
            match self.expected {
                None => self.expected = Some(found),
                Some(expected) if expected != found => {
                    let error = Error::ColumnCount {
                        expected,
                        found,
                        record,
                        line: row.line,
                        byte: self.base + u64::from(row.start),
                    };
                    self.error = Some(error.clone());
                    return Err(error);
                }
                Some(_) => {}
            }
            return Ok(Some(index));
        }
    }

    /// Resolves row `index`'s cells into `resolved`/`scratch`.
    fn resolve(&mut self, index: usize) {
        let row = self.out.rows[index];
        self.resolved.clear();
        self.scratch.clear();
        let first = row.first_cell as usize;
        let mut start = row.start as usize;
        for j in 0..row.cells as usize {
            let cell = self.out.cells[first + j];
            let end = cell.end as usize;
            let raw = &self.data()[start..end];
            let cell_ref = if cell.quotes == 0 || raw.first() != Some(&b'"') {
                CellRef {
                    start: start as u32,
                    len: (end - start) as u32,
                    scratch: false,
                }
            } else if cell.quotes == 2 && raw.len() >= 2 && raw[raw.len() - 1] == b'"' {
                CellRef {
                    start: start as u32 + 1,
                    len: (end - start - 2) as u32,
                    scratch: false,
                }
            } else {
                let offset = self.scratch.len();
                let (scratch, data) = match &mut self.source {
                    Source::Slice(bytes) => (&mut self.scratch, *bytes),
                    Source::Stream { buf, .. } => (&mut self.scratch, buf.as_slice()),
                };
                unescape(&data[start..end], scratch);
                CellRef {
                    start: offset as u32,
                    len: (self.scratch.len() - offset) as u32,
                    scratch: true,
                }
            };
            self.resolved.push(cell_ref);
            start = end + 1;
        }
    }

    fn cell_bytes(&self, cell: CellRef) -> &[u8] {
        let (start, end) = (cell.start as usize, (cell.start + cell.len) as usize);
        if cell.scratch {
            &self.scratch[start..end]
        } else {
            &self.data()[start..end]
        }
    }

    /// Scans the next batch of rows, refilling (and growing) the buffer as needed.
    fn fill(&mut self) -> Result<(), Error> {
        loop {
            self.out.clear();
            self.row_cursor = 0;
            let eof = self.at_eof();
            let stop = {
                let data = match &self.source {
                    Source::Slice(bytes) => *bytes,
                    Source::Stream { buf, .. } => buf.as_slice(),
                };
                self.scanner.scan(
                    data,
                    self.scan_pos,
                    self.line,
                    eof,
                    ROWS_PER_SCAN,
                    &mut self.out,
                )
            };
            self.scan_pos = stop.next;
            self.line = stop.line;
            if stop.unclosed {
                self.pending_unclosed = Some((stop.line, self.base + stop.next as u64));
                return Ok(());
            }
            if !self.out.rows.is_empty() || eof {
                return Ok(());
            }
            self.refill()?;
        }
    }

    /// Moves the unfinished row (everything from `scan_pos`) to the front and reads more.
    fn refill(&mut self) -> Result<(), Error> {
        let Source::Stream { buf, reader, eof } = &mut self.source else {
            return Ok(());
        };
        let keep = self.scan_pos;
        if keep > 0 {
            buf.copy_within(keep.., 0);
            let remaining = buf.len() - keep;
            buf.truncate(remaining);
            self.base += keep as u64;
            self.scan_pos = 0;
        }
        if buf.len() == buf.capacity() {
            if buf.capacity() >= Reader::MAX_ROW_BYTES {
                return Err(Error::RowTooLong {
                    limit: Reader::MAX_ROW_BYTES,
                    record: self.records,
                    line: self.line,
                    byte: self.base,
                });
            }
            buf.reserve(buf.capacity());
        }
        let filled = buf.len();
        let capacity = buf.capacity();
        buf.resize(capacity, 0);
        let mut total = 0;
        while filled + total < capacity {
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
        if self.base == 0 && filled == 0 {
            // First fill: honour a UTF-8 BOM.
            if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
                buf.drain(..3);
                self.base = 3;
            }
        }
        Ok(())
    }
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// One delivered row: byte slices into the reader's buffer (or its unescape scratch),
/// valid until the next [`Reader::next_row`].
pub struct Row<'r, 'a> {
    reader: &'r Reader<'a>,
    index: usize,
}

// `line`/`record` read the reader's current-row fields, which is exactly the point.
#[allow(clippy::misnamed_getters)]
impl<'r, 'a> Row<'r, 'a> {
    /// The number of cells.
    pub fn len(&self) -> usize {
        self.reader.resolved.len()
    }

    /// True when the row has no cells (never the case for a delivered row).
    pub fn is_empty(&self) -> bool {
        self.reader.resolved.is_empty()
    }

    /// The cell at `ordinal`, quotes resolved, or `None` past the end.
    pub fn get(&self, ordinal: usize) -> Option<&'r [u8]> {
        let cell = *self.reader.resolved.get(ordinal)?;
        Some(self.reader.cell_bytes(cell))
    }

    /// Every cell in order.
    pub fn iter(&self) -> impl Iterator<Item = &'r [u8]> + '_ {
        (0..self.len()).map(move |ordinal| self.get(ordinal).unwrap_or(&[]))
    }

    /// The whole row's bytes as they appear in the input, terminator excluded.
    pub fn raw(&self) -> &'r [u8] {
        let row = self.reader.out.rows[self.index];
        let last = self.reader.out.cells[(row.first_cell + row.cells - 1) as usize];
        &self.reader.data()[row.start as usize..last.end as usize]
    }

    /// One-based line the row starts on.
    pub fn line(&self) -> u32 {
        self.reader.current_line
    }

    /// Zero-based record index, header included.
    pub fn record(&self) -> u64 {
        self.reader.current_record
    }

    /// Absolute byte offset of the row's first byte.
    pub fn byte(&self) -> u64 {
        self.reader.base + u64::from(self.reader.out.rows[self.index].start)
    }
}

impl core::fmt::Debug for Row<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list()
            .entries(self.iter().map(String::from_utf8_lossy))
            .finish()
    }
}

impl crate::Row for Row<'_, '_> {
    fn len(&self) -> usize {
        Row::len(self)
    }

    fn cell(&self, ordinal: usize) -> Cell<'_> {
        match self.get(ordinal) {
            Some(bytes) => Cell::Text(bytes),
            None => Cell::Empty,
        }
    }
}

impl<'a> TabularSource for Reader<'a> {
    type Row<'r>
        = Row<'r, 'a>
    where
        Self: 'r;
    type Error = Error;

    fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Error> {
        Reader::next_row(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(reader: &mut Reader<'_>) -> Vec<Vec<Vec<u8>>> {
        let mut rows = Vec::new();
        while let Some(row) = reader.next_row().unwrap() {
            rows.push(row.iter().map(<[u8]>::to_vec).collect());
        }
        rows
    }

    fn strings(rows: &[Vec<Vec<u8>>]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|cell| String::from_utf8_lossy(cell).into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn slice_and_stream_agree_at_every_buffer_size() {
        let text = b"\xEF\xBB\xBFid,name,note\r\n1,\"Smith, J\",\"said \"\"hi\"\"\"\n2,plain,\"multi\nline\"\n\n3,x,\"\"\n";
        let expected = vec![
            vec!["1", "Smith, J", "said \"hi\""],
            vec!["2", "plain", "multi\nline"],
            vec!["3", "x", ""],
        ];
        let mut reader = Reader::from_slice(text, Dialect::CSV).unwrap();
        assert_eq!(
            reader.header().unwrap().names().collect::<Vec<_>>(),
            [&b"id"[..], b"name", b"note"]
        );
        assert_eq!(strings(&collect(&mut reader)), expected);
        for capacity in [64, 65, 70, 100, 128, 1000] {
            let mut reader =
                Reader::from_reader_with_capacity(&text[..], Dialect::CSV, capacity).unwrap();
            assert_eq!(
                reader.header().unwrap().ordinal(b"note"),
                Some(2),
                "capacity {capacity}"
            );
            assert_eq!(
                strings(&collect(&mut reader)),
                expected,
                "capacity {capacity}"
            );
        }
    }

    #[test]
    fn column_count_is_fixed_by_the_first_record() {
        let mut reader = Reader::from_slice(b"a,b\n1,2\n3\n4,5\n", Dialect::CSV).unwrap();
        assert!(reader.next_row().unwrap().is_some());
        let error = reader.next_row().unwrap_err();
        assert!(
            matches!(
                error,
                Error::ColumnCount {
                    expected: 2,
                    found: 1,
                    record: 2,
                    line: 3,
                    byte: 8
                }
            ),
            "{error:?}"
        );
        // Sticky.
        assert!(matches!(
            reader.next_row().unwrap_err(),
            Error::ColumnCount { .. }
        ));
    }

    #[test]
    fn unclosed_quote_is_a_structural_error() {
        let mut reader = Reader::from_slice(b"a,b\n\"open,1\n", Dialect::CSV).unwrap();
        let error = reader.next_row().unwrap_err();
        assert!(
            matches!(
                error,
                Error::UnclosedQuote {
                    record: 1,
                    line: 2,
                    byte: 4
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "input ended inside a quoted cell in record 1 (line 2, byte 4)"
        );
    }

    #[test]
    fn blank_lines_and_headers_are_dialect_choices() {
        let text = b"x\n\n1\n\n";
        let mut reader = Reader::from_slice(text, Dialect::CSV.with_header(false)).unwrap();
        assert_eq!(strings(&collect(&mut reader)), vec![vec!["x"], vec!["1"]]);
        let mut reader = Reader::from_slice(
            text,
            Dialect::CSV
                .with_header(false)
                .with_blank_lines_skipped(false),
        )
        .unwrap();
        assert_eq!(
            strings(&collect(&mut reader)),
            vec![vec!["x"], vec![""], vec!["1"], vec![""]]
        );
        let mut reader = Reader::from_slice(b"", Dialect::CSV).unwrap();
        assert!(reader.header().unwrap().is_empty());
        assert!(reader.next_row().unwrap().is_none());
        assert!(Reader::from_slice(b"", Dialect::new(b'\n')).is_err());
    }

    #[test]
    fn quoting_off_makes_quotes_ordinary() {
        let mut reader =
            Reader::from_slice(b"a,b\n\"x,y\",\"z\n", Dialect::CSV.with_quoting(false)).unwrap();
        let error = reader.next_row().unwrap_err();
        assert!(matches!(
            error,
            Error::ColumnCount {
                expected: 2,
                found: 3,
                ..
            }
        ));
        let mut reader = Reader::from_slice(
            b"\"x\",\"z\n",
            Dialect::CSV.with_quoting(false).with_header(false),
        )
        .unwrap();
        assert_eq!(strings(&collect(&mut reader)), vec![vec!["\"x\"", "\"z"]]);
    }

    #[test]
    fn rows_carry_their_position() {
        let mut reader = Reader::from_slice(b"h\r\n\"a\r\nb\"\r\nc", Dialect::CSV).unwrap();
        let row = reader.next_row().unwrap().unwrap();
        assert_eq!((row.line(), row.record(), row.byte()), (2, 1, 3));
        assert_eq!(row.raw(), b"\"a\r\nb\"");
        let row = reader.next_row().unwrap().unwrap();
        assert_eq!((row.line(), row.record(), row.byte()), (4, 2, 11));
    }

    #[test]
    fn a_row_larger_than_the_buffer_grows_it() {
        let mut text = b"h\n".to_vec();
        text.extend(std::iter::repeat_n(b'x', 5000));
        text.push(b'\n');
        let mut reader = Reader::from_reader_with_capacity(&text[..], Dialect::CSV, 64).unwrap();
        let row = reader.next_row().unwrap().unwrap();
        assert_eq!(row.get(0).unwrap().len(), 5000);
        assert!(reader.next_row().unwrap().is_none());
    }
}
