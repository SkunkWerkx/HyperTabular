//! The header a source declared: the names of its columns, in order, as bytes — a name is
//! whatever the file holds, and a reader that wants a string decides how to read one.

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

    /// The first column with this name: what a plan's ordinal is looked up by.
    pub fn ordinal(&self, name: &[u8]) -> Option<usize> {
        self.names().position(|candidate| candidate == name)
    }
}
