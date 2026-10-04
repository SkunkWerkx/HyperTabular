//! The forward-only sheet cursor shared by both formats: a row store the format parsers
//! fill, empty-row and repeat delivery, the declared header, and the
//! [`TabularSource`] implementation. A `Sheet` owns its own reader over the container
//! (see [`Source`](crate::workbook::Source)), so it outlives the [`Workbook`](crate::workbook::Workbook)
//! that opened it and several can be open at once.

use crate::workbook::error::Error;
use crate::workbook::ods::OdsParser;
use crate::workbook::workbook::Shared;
use crate::workbook::xlsx::sheet::XlsxParser;
use crate::{Cell, CellError, ExcelEpoch, Header, TabularSource, cast_text};
use hypercast::{Date, Duration};
use std::io::Read;
use std::sync::Arc;

/// What the caller declares about a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetOptions {
    /// Whether the first delivered row is a header, exposed through [`Sheet::header`] and
    /// never delivered as data.
    pub has_header: bool,
    /// Whether rows with no cells — explicit empty `<row>`s, gaps in row numbering, and
    /// LibreOffice's repeated padding rows — are skipped rather than delivered empty.
    pub skip_empty_rows: bool,
}

impl Default for SheetOptions {
    fn default() -> SheetOptions {
        SheetOptions {
            has_header: true,
            skip_empty_rows: true,
        }
    }
}

impl SheetOptions {
    /// The same options with or without a header.
    pub const fn with_header(self, has_header: bool) -> SheetOptions {
        SheetOptions { has_header, ..self }
    }

    /// The same options, skipping or delivering empty rows.
    pub const fn with_empty_rows_skipped(self, skip_empty_rows: bool) -> SheetOptions {
        SheetOptions {
            skip_empty_rows,
            ..self
        }
    }
}

/// One stored cell of the current row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stored {
    Empty,
    /// Bytes in the row arena.
    Arena {
        start: u32,
        len: u32,
    },
    /// A shared-string index.
    Shared(u32),
    Number(f64),
    Bool(bool),
    Wall {
        date: Date,
        nanos: u64,
    },
    Clock(u64),
    Span(Duration),
    Error(CellError),
}

impl Stored {
    /// A stored form of a cell the format layer already resolved (never `Text` — text
    /// goes through [`RowStore::text`] so it lands in the arena).
    pub(crate) fn from_cell(cell: Cell<'_>, store: &mut RowStore) -> Stored {
        match cell {
            Cell::Empty => Stored::Empty,
            Cell::Text(text) => store.text(text),
            Cell::Number(value) => Stored::Number(value),
            Cell::Bool(value) => Stored::Bool(value),
            Cell::Wall { date, nanos } => Stored::Wall { date, nanos },
            Cell::Clock(nanos) => Stored::Clock(nanos),
            Cell::Span(span) => Stored::Span(span),
            Cell::Error(error) => Stored::Error(error),
        }
    }
}

/// The current row as the parser filled it, plus how it is delivered.
#[derive(Debug, Default)]
pub(crate) struct RowStore {
    pub(crate) cells: Vec<Stored>,
    pub(crate) arena: Vec<u8>,
    /// Empty rows owed *before* this row (a gap in XLSX row numbering).
    pub(crate) gap: u32,
    /// Times this row is delivered (ODS `number-rows-repeated`); parsers set ≥ 1.
    pub(crate) repeat: u32,
    /// One-based sheet row number of this row's first delivery.
    pub(crate) number: u32,
}

impl RowStore {
    pub(crate) fn clear(&mut self) {
        self.cells.clear();
        self.arena.clear();
        self.gap = 0;
        self.repeat = 1;
    }

    /// Copies `bytes` into the arena and returns the stored reference.
    pub(crate) fn text(&mut self, bytes: &[u8]) -> Stored {
        let start = self.arena.len() as u32;
        self.arena.extend_from_slice(bytes);
        Stored::Arena {
            start,
            len: bytes.len() as u32,
        }
    }

    /// Places `cell` at `column`, padding any gap with empties.
    pub(crate) fn place(&mut self, column: usize, cell: Stored) {
        if column >= self.cells.len() {
            self.cells.resize(column + 1, Stored::Empty);
        }
        self.cells[column] = cell;
    }

    /// Drops trailing empties so `cells.len()` is the last populated column plus one.
    pub(crate) fn trim_trailing(&mut self) {
        while self.cells.last() == Some(&Stored::Empty) {
            self.cells.pop();
        }
    }
}

pub(crate) enum Parser<R> {
    Xlsx(XlsxParser<R>),
    Ods(OdsParser<R>),
}

/// A forward-only cursor over one worksheet. See the module doc.
pub struct Sheet<R> {
    parser: Parser<R>,
    shared: Arc<Shared>,
    options: SheetOptions,
    store: RowStore,
    /// Deliveries still owed from the current store: gap rows first, then repeats.
    gap_left: u32,
    repeat_left: u32,
    /// True while the delivery in progress is one of the gap rows.
    empty_delivery: bool,
    /// One-based sheet row number of the delivery in progress.
    current_number: u32,
    header: Option<Header>,
    error: Option<Error>,
    done: bool,
}

impl<R: Read> Sheet<R> {
    pub(crate) fn new(
        parser: Parser<R>,
        shared: Arc<Shared>,
        options: SheetOptions,
    ) -> Result<Sheet<R>, Error> {
        let mut sheet = Sheet {
            parser,
            shared,
            options,
            store: RowStore::default(),
            gap_left: 0,
            repeat_left: 0,
            empty_delivery: false,
            current_number: 0,
            header: None,
            error: None,
            done: false,
        };
        if options.has_header {
            sheet.read_header()?;
        }
        Ok(sheet)
    }

    /// The options in force.
    pub fn options(&self) -> SheetOptions {
        self.options
    }

    /// The declared header, once read.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// The workbook's date system, which serials in this sheet are read under.
    pub fn date_system(&self) -> ExcelEpoch {
        self.shared.date_system
    }

    fn read_header(&mut self) -> Result<(), Error> {
        let mut header = Header::new();
        if self.advance()? {
            let row = Row { sheet: self };
            for ordinal in 0..row.len() {
                match cast_text(&row.cell(ordinal)) {
                    Ok(text) => header.push(&text),
                    Err(_) => header.push(b""),
                }
            }
        }
        self.header = Some(header);
        Ok(())
    }

    /// The next row, borrowed until the next call; `None` at the end. Structural errors
    /// are sticky.
    pub fn next_row(&mut self) -> Result<Option<Row<'_, R>>, Error> {
        if !self.advance()? {
            return Ok(None);
        }
        Ok(Some(Row { sheet: self }))
    }

    /// Positions on the next delivery. Returns false at the end.
    fn advance(&mut self) -> Result<bool, Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        loop {
            if self.gap_left > 0 {
                self.gap_left -= 1;
                self.current_number += 1;
                self.empty_delivery = true;
                return Ok(true);
            }
            if self.repeat_left > 0 {
                self.repeat_left -= 1;
                self.current_number += 1;
                self.empty_delivery = false;
                return Ok(true);
            }
            if self.done {
                return Ok(false);
            }
            self.store.clear();
            let parsed = match &mut self.parser {
                Parser::Xlsx(parser) => parser.next_row(&mut self.store, &self.shared),
                Parser::Ods(parser) => parser.next_row(&mut self.store, &self.shared),
            };
            match parsed {
                Ok(true) => {}
                Ok(false) => {
                    self.done = true;
                    return Ok(false);
                }
                Err(error) => {
                    self.error = Some(error.clone());
                    return Err(error);
                }
            }
            self.store.trim_trailing();
            let blank = self.store.cells.is_empty();
            let mut gap = self.store.gap;
            let mut repeat = self.store.repeat.max(1);
            if blank {
                // An explicit empty row is just more gap.
                gap += repeat;
                repeat = 0;
            }
            if self.options.skip_empty_rows {
                gap = 0;
            }
            // The first delivery is row `number - store.gap` when gaps are delivered;
            // `current_number` is pre-incremented on each delivery.
            self.current_number = if self.options.skip_empty_rows {
                self.store.number - 1
            } else {
                self.store.number - self.store.gap - 1
            };
            self.gap_left = gap;
            self.repeat_left = repeat;
        }
    }
}

/// One delivered row: cells by ordinal, valid until the next [`Sheet::next_row`].
pub struct Row<'s, R> {
    sheet: &'s Sheet<R>,
}

impl<R> Row<'_, R> {
    /// The number of stored cells (the last populated column plus one); `0` for an
    /// empty row.
    pub fn len(&self) -> usize {
        if self.sheet.empty_delivery {
            0
        } else {
            self.sheet.store.cells.len()
        }
    }

    /// True for an empty row.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One-based row number in the sheet.
    pub fn number(&self) -> u32 {
        self.sheet.current_number
    }

    /// The cell at `ordinal`, [`Cell::Empty`] past the end.
    pub fn cell(&self, ordinal: usize) -> Cell<'_> {
        if self.sheet.empty_delivery {
            return Cell::Empty;
        }
        let store = &self.sheet.store;
        match store.cells.get(ordinal).copied().unwrap_or(Stored::Empty) {
            Stored::Empty => Cell::Empty,
            Stored::Arena { start, len } => {
                Cell::Text(&store.arena[start as usize..(start + len) as usize])
            }
            Stored::Shared(index) => Cell::Text(self.sheet.shared.shared_string(index as usize)),
            Stored::Number(value) => Cell::Number(value),
            Stored::Bool(value) => Cell::Bool(value),
            Stored::Wall { date, nanos } => Cell::Wall { date, nanos },
            Stored::Clock(nanos) => Cell::Clock(nanos),
            Stored::Span(span) => Cell::Span(span),
            Stored::Error(error) => Cell::Error(error),
        }
    }

    /// Every cell in order.
    pub fn iter(&self) -> impl Iterator<Item = Cell<'_>> + '_ {
        (0..self.len()).map(move |ordinal| self.cell(ordinal))
    }
}

impl<R> core::fmt::Debug for Row<'_, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<R> crate::Row for Row<'_, R> {
    fn len(&self) -> usize {
        Row::len(self)
    }

    fn cell(&self, ordinal: usize) -> Cell<'_> {
        Row::cell(self, ordinal)
    }
}

impl<R: Read> TabularSource for Sheet<R> {
    type Row<'s>
        = Row<'s, R>
    where
        Self: 's;
    type Error = Error;

    fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    fn date_system(&self) -> ExcelEpoch {
        self.shared.date_system
    }

    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Error> {
        Sheet::next_row(self)
    }
}
