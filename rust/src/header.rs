//! The header a source declared: the names of its columns, in order, as bytes — a name is
//! whatever the file holds, and a reader that wants a string decides how to read one.

use crate::Error;

/// A source's column names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    bytes: Vec<u8>,
    spans: Vec<(usize, usize)>,
}

impl Header {
    /// No names.
    pub fn new() -> Header {
        Header::default()
    }

    /// Adds a name after the last.
    pub fn push(&mut self, name: &[u8]) {
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(name);
        self.spans.push((offset, name.len()));
    }

    /// How many columns the header names.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// True for a header with no names: the source had no record to read one from.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The name of the column at `ordinal`.
    pub fn name(&self, ordinal: usize) -> Option<&[u8]> {
        let &(offset, len) = self.spans.get(ordinal)?;
        self.bytes.get(offset..offset + len)
    }

    /// Every name, in column order.
    pub fn names(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.spans.len()).map(|ordinal| self.name(ordinal).unwrap_or_default())
    }

    /// The first column with this name: what a plan's ordinal is looked up by. The match
    /// is exact — byte for byte, case and spaces included — so a `&str` and a `&[u8]` find
    /// the same column.
    pub fn ordinal(&self, name: impl AsRef<[u8]>) -> Option<usize> {
        let name = name.as_ref();
        self.names().position(|candidate| candidate == name)
    }

    /// [`Header::ordinal`], with a missing name an error that says which: what a plan built
    /// from names reads best with, a `?` on each column.
    ///
    /// ```
    /// # use hypertabular::{Column, DelimitedReader, Dialect};
    /// let mut reader = DelimitedReader::from_slice_unbound(b"id,name\n1,a\n", Dialect::CSV)?;
    /// let header = reader.header().unwrap();
    /// let plan = [Column::text(header.require("name")?), Column::i32(header.require("id")?)];
    /// reader.bind(&plan)?;
    /// # Ok::<(), hypertabular::Error>(())
    /// ```
    pub fn require(&self, name: impl AsRef<[u8]>) -> Result<usize, Error> {
        let name = name.as_ref();
        self.ordinal(name)
            .ok_or_else(|| Error::NoColumn(String::from_utf8_lossy(name).into_owned()))
    }
}
