//! The XLSX sheet stream: `<sheetData>` → `<row>` → `<c>`, parsed straight from the
//! tokenizer's borrowed bytes into the row store. `r` gives the column (missing ⇒ the
//! previous column plus one, per ISO 29500), `s` the `cellXfs` index, `t` the type;
//! `<v>` is parsed by type and — for numbers — classified by the style's number-format
//! kind; `<is>` is collected like a shared string; `<f>` is skipped: the cached value is
//! the value.

use crate::serial::{self, SerialKind};
use crate::workbook::error::Error;
use crate::workbook::iso::wall_from_iso;
use crate::workbook::sheet::{RowStore, Stored};
use crate::workbook::workbook::Shared;
use crate::workbook::xlsx::styles::NumberKind;
use crate::workbook::xlsx::{collect_rich_text, parse_u32};
use crate::workbook::xml::{self, Event, Reader, Tag};
use crate::workbook::zip::EntryReader;
use crate::{Cell, CellError};
use std::io::Read;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellType {
    Number,
    SharedString,
    FormulaString,
    InlineString,
    Bool,
    Error,
    Date,
}

/// The parser over one worksheet part.
pub struct XlsxParser<R> {
    xml: Reader<EntryReader<R>>,
    part: String,
    in_data: bool,
    done: bool,
    /// The row number the next `<row>` gets if it carries no `r`.
    next_number: u32,
    scratch: Vec<u8>,
    value: Vec<u8>,
}

impl<R: Read> XlsxParser<R> {
    pub(crate) fn new(stream: EntryReader<R>, part: String) -> XlsxParser<R> {
        XlsxParser {
            xml: Reader::new(stream),
            part,
            in_data: false,
            done: false,
            next_number: 1,
            scratch: Vec::new(),
            value: Vec::new(),
        }
    }

    fn xml_error(&self, error: xml::Malformed) -> Error {
        Error::malformed(&self.part, error.0)
    }

    fn next_event(&mut self) -> Result<Event<'_>, Error> {
        // Borrow dance: map the error before returning the borrowed event.
        let part = &self.part;
        self.xml.next().map_err(|e| Error::malformed(part, e.0))
    }

    /// Fills `store` with the next `<row>`; false at the end of `<sheetData>`.
    pub(crate) fn next_row(
        &mut self,
        store: &mut RowStore,
        shared: &Shared,
    ) -> Result<bool, Error> {
        if self.done {
            return Ok(false);
        }
        if !self.in_data {
            loop {
                match self.next_event()? {
                    Event::Start(tag) if tag.local() == b"sheetData" => break,
                    Event::Empty(tag) if tag.local() == b"sheetData" => {
                        self.done = true;
                        return Ok(false);
                    }
                    Event::Eof => {
                        self.done = true;
                        return Ok(false);
                    }
                    _ => {}
                }
            }
            self.in_data = true;
        }
        loop {
            let (number, empty) = match self.next_event()? {
                Event::Start(tag) if tag.local() == b"row" => {
                    (tag.attr(b"r").and_then(parse_u32), false)
                }
                Event::Empty(tag) if tag.local() == b"row" => {
                    (tag.attr(b"r").and_then(parse_u32), true)
                }
                Event::End(name) if xml::local_name(name) == b"sheetData" => {
                    self.done = true;
                    return Ok(false);
                }
                Event::Eof => {
                    self.done = true;
                    return Ok(false);
                }
                Event::Start(_) => {
                    // Something other than a row inside sheetData: skip it whole.
                    self.xml.skip_subtree().map_err(|e| self.xml_error(e))?;
                    continue;
                }
                _ => continue,
            };
            let number = number
                .filter(|&n| n >= self.next_number)
                .unwrap_or(self.next_number);
            store.number = number;
            store.gap = number - self.next_number;
            store.repeat = 1;
            self.next_number = number + 1;
            if !empty {
                self.read_cells(store, shared)?;
            }
            return Ok(true);
        }
    }

    /// Reads `<c>` elements until `</row>`.
    fn read_cells(&mut self, store: &mut RowStore, shared: &Shared) -> Result<(), Error> {
        let mut next_col = 0usize;
        loop {
            let (col, cell_type, kind, is_empty) = match self.next_event()? {
                Event::Start(tag) if tag.local() == b"c" => {
                    let (col, cell_type, kind) = cell_attrs(&tag, next_col, shared);
                    (col, cell_type, kind, false)
                }
                Event::Empty(tag) if tag.local() == b"c" => {
                    let (col, cell_type, kind) = cell_attrs(&tag, next_col, shared);
                    (col, cell_type, kind, true)
                }
                Event::Start(_) => {
                    self.xml.skip_subtree().map_err(|e| self.xml_error(e))?;
                    continue;
                }
                Event::End(name) if xml::local_name(name) == b"row" => return Ok(()),
                Event::Eof => {
                    return Err(Error::malformed(
                        &self.part,
                        "unexpected end of input inside a row",
                    ));
                }
                _ => continue,
            };
            next_col = col + 1;
            if is_empty {
                store.place(col, Stored::Empty);
                continue;
            }
            let stored = self.read_cell_body(cell_type, kind, store, shared)?;
            store.place(col, stored);
        }
    }

    /// Reads a `<c>`'s children up to `</c>` and returns the stored value.
    fn read_cell_body(
        &mut self,
        cell_type: CellType,
        kind: NumberKind,
        store: &mut RowStore,
        shared: &Shared,
    ) -> Result<Stored, Error> {
        let mut stored = Stored::Empty;
        loop {
            match self.next_event()? {
                Event::Start(tag) => match tag.local() {
                    b"v" => {
                        self.value.clear();
                        self.read_text_until_end()?;
                        stored = self.value_to_stored(cell_type, kind, store, shared)?;
                    }
                    b"is" => {
                        self.value.clear();
                        let part = self.part.clone();
                        collect_rich_text(
                            &part,
                            &mut self.xml,
                            &mut self.scratch,
                            &mut self.value,
                        )?;
                        stored = store.text(&self.value);
                    }
                    _ => self.xml.skip_subtree().map_err(|e| self.xml_error(e))?,
                },
                Event::End(name) if xml::local_name(name) == b"c" => return Ok(stored),
                Event::Eof => {
                    return Err(Error::malformed(
                        &self.part,
                        "unexpected end of input inside a cell",
                    ));
                }
                _ => {}
            }
        }
    }

    /// Appends the text content up to the matching end tag into `self.value`.
    fn read_text_until_end(&mut self) -> Result<(), Error> {
        let mut depth = 1usize;
        loop {
            let part = &self.part;
            match self.xml.next().map_err(|e| Error::malformed(part, e.0))? {
                Event::Text(text) => self
                    .value
                    .extend_from_slice(xml::unescape(text, &mut self.scratch)),
                Event::CData(text) => self.value.extend_from_slice(text),
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Event::Empty(_) => {}
                Event::Eof => {
                    return Err(Error::malformed(
                        part,
                        "unexpected end of input inside a value",
                    ));
                }
            }
        }
    }

    fn value_to_stored(
        &mut self,
        cell_type: CellType,
        kind: NumberKind,
        store: &mut RowStore,
        shared: &Shared,
    ) -> Result<Stored, Error> {
        let value = self.value.trim_ascii();
        Ok(match cell_type {
            CellType::SharedString => {
                let Some(index) = parse_u32(value) else {
                    return Err(Error::malformed(
                        &self.part,
                        "shared-string index is not a number",
                    ));
                };
                if index as usize >= shared.shared_string_count() {
                    return Err(Error::malformed(
                        &self.part,
                        format!("shared-string index {index} past the table"),
                    ));
                }
                Stored::Shared(index)
            }
            CellType::FormulaString | CellType::InlineString => store.text(value),
            CellType::Bool => match value {
                b"1" => Stored::Bool(true),
                b"0" => Stored::Bool(false),
                _ => store.text(value),
            },
            CellType::Error => match CellError::from_text(value) {
                Some(error) => Stored::Error(error),
                None => store.text(value),
            },
            CellType::Date => match wall_from_iso(value) {
                Some(cell) => Stored::from_cell(cell, store),
                None => store.text(value),
            },
            CellType::Number => match parse_f64(value) {
                None => store.text(value),
                Some(number) => {
                    let serial_kind = match kind {
                        NumberKind::DateTime => Some(SerialKind::DateTime),
                        NumberKind::Elapsed => Some(SerialKind::Elapsed),
                        NumberKind::Number | NumberKind::Text => None,
                    };
                    match serial_kind
                        .and_then(|k| serial::to_cell(number, shared.date_system, k).ok())
                    {
                        Some(cell) => Stored::from_cell(cell, store),
                        // A non-temporal format, or a serial the rules reject: keep the
                        // number and let the door say what it thinks.
                        None => Stored::from_cell(Cell::Number(number), store),
                    }
                }
            },
        })
    }
}

/// The column, type, and number-format kind a `<c>` tag declares.
fn cell_attrs(tag: &Tag<'_>, next_col: usize, shared: &Shared) -> (usize, CellType, NumberKind) {
    let col = tag.attr(b"r").and_then(parse_col).unwrap_or(next_col);
    let kind = tag
        .attr(b"s")
        .and_then(parse_u32)
        .and_then(|s| shared.styles.get(s as usize).copied())
        .unwrap_or(NumberKind::Number);
    let cell_type = match tag.attr(b"t") {
        None | Some(b"n") => CellType::Number,
        Some(b"s") => CellType::SharedString,
        Some(b"str") => CellType::FormulaString,
        Some(b"inlineStr") => CellType::InlineString,
        Some(b"b") => CellType::Bool,
        Some(b"e") => CellType::Error,
        Some(b"d") => CellType::Date,
        Some(_) => CellType::FormulaString,
    };
    (col, cell_type, kind)
}

/// The zero-based column of a cell reference (`"C7"` → 2).
pub(crate) fn parse_col(reference: &[u8]) -> Option<usize> {
    let mut col: usize = 0;
    let mut letters = 0;
    for &b in reference {
        if b.is_ascii_alphabetic() {
            col = col * 26 + usize::from(b.to_ascii_uppercase() - b'A' + 1);
            letters += 1;
            if letters > 3 {
                return None;
            }
        } else {
            break;
        }
    }
    if letters == 0 { None } else { Some(col - 1) }
}

pub(crate) fn parse_f64(bytes: &[u8]) -> Option<f64> {
    let text = str::from_utf8(bytes).ok()?;
    let value: f64 = text.parse().ok()?;
    if value.is_finite() { Some(value) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_references() {
        assert_eq!(parse_col(b"A1"), Some(0));
        assert_eq!(parse_col(b"Z10"), Some(25));
        assert_eq!(parse_col(b"AA1"), Some(26));
        assert_eq!(parse_col(b"XFD1048576"), Some(16_383));
        assert_eq!(parse_col(b"1"), None);
        assert_eq!(parse_col(b""), None);
        assert_eq!(parse_f64(b"1E+15"), Some(1e15));
        assert_eq!(parse_f64(b"-0.5"), Some(-0.5));
        assert_eq!(parse_f64(b"x"), None);
    }
}
