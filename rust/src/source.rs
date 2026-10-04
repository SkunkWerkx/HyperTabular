//! The provider trait: a forward-only cursor over rows of [`Cell`]s, with a declared
//! header and the source's date system. Svartalfheim's `ITabularReader` (`FieldCount`,
//! `Ordinal`, `Read`, indexer) re-drawn for Rust: the row borrows the provider's buffers
//! and dies at the next `next_row`.

use crate::cell::Cell;
use hypercast::ExcelEpoch;

/// The header row as the provider read it (declared by the caller, never sniffed), with
/// ordinal lookup by exact bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    bytes: Vec<u8>,
    spans: Vec<(u32, u32)>,
}

impl Header {
    /// An empty header.
    pub fn new() -> Header {
        Header::default()
    }

    /// A header from names in ordinal order.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a [u8]>) -> Header {
        let mut header = Header::new();
        for name in names {
            header.push(name);
        }
        header
    }

    /// Appends the next column's name.
    pub fn push(&mut self, name: &[u8]) {
        let offset = self.bytes.len() as u32;
        self.bytes.extend_from_slice(name);
        self.spans.push((offset, name.len() as u32));
    }

    /// The number of named columns.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// True when no columns are named.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The name at `ordinal`.
    pub fn name(&self, ordinal: usize) -> Option<&[u8]> {
        let &(offset, len) = self.spans.get(ordinal)?;
        Some(&self.bytes[offset as usize..(offset + len) as usize])
    }

    /// Every name, in ordinal order.
    pub fn names(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.spans.len()).map(|ordinal| self.name(ordinal).unwrap_or(&[]))
    }

    /// The ordinal of the first column named exactly `name` (byte equality — no
    /// trimming, no case folding; declare what the file says).
    pub fn ordinal(&self, name: &[u8]) -> Option<usize> {
        self.names().position(|candidate| candidate == name)
    }
}

/// One row: cells by ordinal, [`Cell::Empty`] past the end.
pub trait Row {
    /// The number of cells the provider read for this row.
    fn len(&self) -> usize;

    /// The cell at `ordinal`, or [`Cell::Empty`] past the row's end.
    fn cell(&self, ordinal: usize) -> Cell<'_>;

    /// True when the row has no cells.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Row for [Cell<'_>] {
    fn len(&self) -> usize {
        <[Cell<'_>]>::len(self)
    }

    fn cell(&self, ordinal: usize) -> Cell<'_> {
        self.get(ordinal).copied().unwrap_or(Cell::Empty)
    }
}

impl<R: Row + ?Sized> Row for &R {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn cell(&self, ordinal: usize) -> Cell<'_> {
        (**self).cell(ordinal)
    }
}

impl Row for Vec<Cell<'_>> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn cell(&self, ordinal: usize) -> Cell<'_> {
        self.get(ordinal).copied().unwrap_or(Cell::Empty)
    }
}

/// A forward-only tabular source.
pub trait TabularSource {
    /// The row type, borrowing the provider.
    type Row<'a>: Row
    where
        Self: 'a;

    /// The provider's structural error: a torn delimited row, a corrupt container.
    /// Never a cell verdict.
    type Error;

    /// The declared header, if the caller declared one.
    fn header(&self) -> Option<&Header>;

    /// The date system serials are read under; workbooks override this from the file.
    fn date_system(&self) -> ExcelEpoch {
        ExcelEpoch::Y1900
    }

    /// The next row, or `None` at the end.
    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Self::Error>;
}

impl<S: TabularSource + ?Sized> TabularSource for &mut S {
    type Row<'a>
        = S::Row<'a>
    where
        Self: 'a;
    type Error = S::Error;

    fn header(&self) -> Option<&Header> {
        (**self).header()
    }

    fn date_system(&self) -> ExcelEpoch {
        (**self).date_system()
    }

    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Self::Error> {
        (**self).next_row()
    }
}
