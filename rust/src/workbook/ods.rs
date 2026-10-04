//! ODS: `mimetype` check, the `content.xml` table scan, and the sheet stream. Typing
//! comes from `office:value-type` and its paired value attribute — no style lookup —
//! and the repeat discipline is calamine's: repeated *empty* cells and rows are never
//! materialised beyond what the sheet cursor is asked to deliver.

use crate::workbook::error::Error;
use crate::workbook::iso::wall_from_iso;
use crate::workbook::sheet::{RowStore, Stored};
use crate::workbook::workbook::{Shared, SheetInfo, SheetPart};
use crate::workbook::xlsx::parse_u32;
use crate::workbook::xlsx::sheet::parse_f64;
use crate::workbook::xml::{self, Event, Reader, Tag};
use crate::workbook::zip::{Archive, EntryReader};
use crate::{CellError, ExcelEpoch};
use hypercast::cast_duration;
use std::io::{Read, Seek};

const CONTENT: &str = "content.xml";
/// ODF's own column ceiling; a repeat count past it is padding, not data.
const MAX_COLUMNS: usize = 16_384;

/// True when the container declares itself an OpenDocument spreadsheet.
pub(crate) fn is_ods<R: Read + Seek>(archive: &mut Archive<R>) -> Result<bool, Error> {
    let Some(index) = archive.find("mimetype") else {
        return Ok(false);
    };
    let mime = archive.read(index)?;
    Ok(mime.trim_ascii() == b"application/vnd.oasis.opendocument.spreadsheet")
}

/// Lists the sheets (`table:table` names in `content.xml`, in order).
pub(crate) fn open<R: Read + Seek>(
    archive: &mut Archive<R>,
) -> Result<(Vec<SheetInfo>, Shared), Error> {
    if let Some(index) = archive.find("META-INF/manifest.xml") {
        let manifest = archive.read(index)?;
        if xml::find_seq(&manifest, 0, b"encryption-data").is_some() {
            return Err(Error::Container("the document is encrypted".to_string()));
        }
    }
    let index = archive
        .find(CONTENT)
        .ok_or_else(|| Error::malformed(CONTENT, "missing"))?;
    let mut reader = Reader::new(archive.stream(index)?);
    let mut scratch = Vec::new();
    let mut sheets = Vec::new();
    loop {
        match reader.next().map_err(|e| Error::malformed(CONTENT, e.0))? {
            Event::Start(tag) if is_table(&tag) => {
                let name = tag
                    .attr(b"name")
                    .map(|raw| {
                        String::from_utf8_lossy(xml::unescape(raw, &mut scratch)).into_owned()
                    })
                    .unwrap_or_default();
                sheets.push(SheetInfo {
                    name,
                    hidden: false,
                    part: SheetPart::Ods(sheets.len()),
                });
                // Nested tables (inside cells) are not sheets.
                reader
                    .skip_subtree()
                    .map_err(|e| Error::malformed(CONTENT, e.0))?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let shared = Shared {
        sst_offsets: vec![0],
        date_system: ExcelEpoch::Y1900,
        ..Shared::default()
    };
    Ok((sheets, shared))
}

fn is_table(tag: &Tag<'_>) -> bool {
    tag.local() == b"table" && tag.name.ends_with(b"table:table")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueType {
    None,
    Float,
    Bool,
    Date,
    Time,
    String,
    Error,
}

/// The cell's declared type, with its paired value attribute decoded into `value`;
/// the flag says whether a value attribute was present at all (an empty
/// `office:string-value` is a value).
fn read_value_attrs(
    tag: &Tag<'_>,
    value: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> (ValueType, bool) {
    value.clear();
    let type_attr = tag
        .attr_qualified(b"office:value-type")
        .or_else(|| tag.attr(b"value-type"));
    // LibreOffice marks an error cell `office:value-type="string"` *and*
    // `calcext:value-type="error"`; the extension is the truth there.
    let calcext_error = tag.attr_qualified(b"calcext:value-type") == Some(&b"error"[..]);
    let value_type = match type_attr {
        _ if calcext_error => ValueType::Error,
        Some(b"float") | Some(b"percentage") | Some(b"currency") => ValueType::Float,
        Some(b"boolean") => ValueType::Bool,
        Some(b"date") => ValueType::Date,
        Some(b"time") => ValueType::Time,
        Some(b"string") => ValueType::String,
        Some(b"error") => ValueType::Error,
        Some(_) => ValueType::String,
        None => ValueType::None,
    };
    let raw = match value_type {
        ValueType::Float => tag.attr(b"value"),
        ValueType::Bool => tag.attr(b"boolean-value"),
        ValueType::Date => tag.attr(b"date-value"),
        ValueType::Time => tag.attr(b"time-value"),
        ValueType::String => tag.attr(b"string-value"),
        ValueType::Error | ValueType::None => None,
    };
    match raw {
        Some(raw) => {
            let decoded = xml::unescape(raw, scratch);
            value.extend_from_slice(decoded);
            (value_type, true)
        }
        None => (value_type, false),
    }
}

/// The parser over one `table:table`.
pub struct OdsParser<R> {
    xml: Reader<EntryReader<R>>,
    table: usize,
    positioned: bool,
    done: bool,
    next_number: u32,
    scratch: Vec<u8>,
    /// The paired value attribute of the cell being read.
    value: Vec<u8>,
    /// The cell's paragraph text.
    text: Vec<u8>,
    /// Whether the cell carried an `office:string-value` (possibly empty).
    value_present: bool,
}

fn xml_err(error: xml::Malformed) -> Error {
    Error::malformed(CONTENT, error.0)
}

fn is_cell(tag: &Tag<'_>) -> bool {
    matches!(tag.local(), b"table-cell" | b"covered-table-cell")
}

impl<R: Read> OdsParser<R> {
    pub(crate) fn new(stream: EntryReader<R>, table: usize) -> OdsParser<R> {
        OdsParser {
            xml: Reader::new(stream),
            table,
            positioned: false,
            done: false,
            next_number: 1,
            scratch: Vec::new(),
            value: Vec::new(),
            text: Vec::new(),
            value_present: false,
        }
    }

    fn next_event(&mut self) -> Result<Event<'_>, Error> {
        self.xml.next().map_err(|e| Error::malformed(CONTENT, e.0))
    }

    fn skip(&mut self) -> Result<(), Error> {
        self.xml
            .skip_subtree()
            .map_err(|e| Error::malformed(CONTENT, e.0))
    }

    /// Fills `store` with the next row; false at the end of the table.
    pub(crate) fn next_row(
        &mut self,
        store: &mut RowStore,
        _shared: &Shared,
    ) -> Result<bool, Error> {
        if self.done {
            return Ok(false);
        }
        if !self.positioned {
            let mut seen = 0usize;
            loop {
                match self.next_event()? {
                    Event::Start(tag) if is_table(&tag) => {
                        if seen == self.table {
                            break;
                        }
                        seen += 1;
                        self.skip()?;
                    }
                    Event::Eof => {
                        self.done = true;
                        return Ok(false);
                    }
                    _ => {}
                }
            }
            self.positioned = true;
        }
        loop {
            let (repeat, empty) = match self.next_event()? {
                Event::Start(tag) if tag.local() == b"table-row" => (
                    tag.attr(b"number-rows-repeated")
                        .and_then(parse_u32)
                        .unwrap_or(1)
                        .max(1),
                    false,
                ),
                Event::Empty(tag) if tag.local() == b"table-row" => (
                    tag.attr(b"number-rows-repeated")
                        .and_then(parse_u32)
                        .unwrap_or(1)
                        .max(1),
                    true,
                ),
                Event::Start(tag) if is_table(&tag) => {
                    self.skip()?;
                    continue;
                }
                Event::Start(tag)
                    if matches!(
                        tag.local(),
                        b"table-header-rows" | b"table-rows" | b"table-row-group"
                    ) =>
                {
                    continue;
                }
                Event::Start(tag) if tag.local() == b"soft-page-break" => continue,
                Event::Start(_) => {
                    self.skip()?;
                    continue;
                }
                Event::End(name) if xml::local_name(name) == b"table" => {
                    self.done = true;
                    return Ok(false);
                }
                Event::Eof => {
                    self.done = true;
                    return Ok(false);
                }
                _ => continue,
            };
            store.number = self.next_number;
            store.gap = 0;
            store.repeat = repeat;
            self.next_number = self.next_number.saturating_add(repeat);
            if !empty {
                self.read_cells(store)?;
            }
            return Ok(true);
        }
    }

    fn read_cells(&mut self, store: &mut RowStore) -> Result<(), Error> {
        let mut col = 0usize;
        loop {
            let (value_type, repeat, has_body) = match self.xml.next().map_err(xml_err)? {
                Event::Start(tag) if is_cell(&tag) => {
                    let repeat = tag
                        .attr(b"number-columns-repeated")
                        .and_then(parse_u32)
                        .unwrap_or(1)
                        .max(1) as usize;
                    let (value_type, present) =
                        read_value_attrs(&tag, &mut self.value, &mut self.scratch);
                    self.value_present = present;
                    (value_type, repeat, true)
                }
                Event::Empty(tag) if is_cell(&tag) => {
                    let repeat = tag
                        .attr(b"number-columns-repeated")
                        .and_then(parse_u32)
                        .unwrap_or(1)
                        .max(1) as usize;
                    let (value_type, present) =
                        read_value_attrs(&tag, &mut self.value, &mut self.scratch);
                    self.value_present = present;
                    (value_type, repeat, false)
                }
                Event::Start(_) => {
                    self.skip()?;
                    continue;
                }
                Event::End(name) if xml::local_name(name) == b"table-row" => return Ok(()),
                Event::Eof => {
                    return Err(Error::malformed(
                        CONTENT,
                        "unexpected end of input inside a row",
                    ));
                }
                _ => continue,
            };
            self.text.clear();
            if has_body {
                self.collect_text()?;
            }
            let stored = self.finish_cell(value_type, store);
            if stored != Stored::Empty {
                let end = (col + repeat).min(MAX_COLUMNS);
                for c in col..end {
                    store.place(c, stored);
                }
            }
            col += repeat;
        }
    }

    /// Collects the cell's paragraphs into `self.text`, up to `</table-cell>`.
    fn collect_text(&mut self) -> Result<(), Error> {
        let mut depth = 1usize;
        let mut paragraphs = 0usize;
        loop {
            // Borrow only the tokenizer, so the arms can write the parser's own buffers.
            match self.xml.next().map_err(xml_err)? {
                Event::Start(tag) => {
                    if tag.name.starts_with(b"draw:")
                        || tag.name.starts_with(b"office:annotation")
                        || is_table(&tag)
                    {
                        self.skip()?;
                        continue;
                    }
                    match tag.local() {
                        b"p" => {
                            if paragraphs > 0 {
                                self.text.push(b'\n');
                            }
                            paragraphs += 1;
                        }
                        b"s" => {
                            let count = tag.attr(b"c").and_then(parse_u32).unwrap_or(1) as usize;
                            self.text.extend(std::iter::repeat_n(b' ', count));
                        }
                        b"tab" => self.text.push(b'\t'),
                        b"line-break" => self.text.push(b'\n'),
                        _ => {}
                    }
                    depth += 1;
                }
                Event::Empty(tag) => match tag.local() {
                    b"s" => {
                        let count = tag.attr(b"c").and_then(parse_u32).unwrap_or(1) as usize;
                        self.text.extend(std::iter::repeat_n(b' ', count));
                    }
                    b"tab" => self.text.push(b'\t'),
                    b"line-break" => self.text.push(b'\n'),
                    b"p" => {
                        if paragraphs > 0 {
                            self.text.push(b'\n');
                        }
                        paragraphs += 1;
                    }
                    _ => {}
                },
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Event::Text(text) => {
                    let decoded = xml::unescape(text, &mut self.scratch);
                    self.text.extend_from_slice(decoded);
                }
                Event::CData(text) => self.text.extend_from_slice(text),
                Event::Eof => {
                    return Err(Error::malformed(
                        CONTENT,
                        "unexpected end of input inside a cell",
                    ));
                }
            }
        }
    }

    fn finish_cell(&mut self, value_type: ValueType, store: &mut RowStore) -> Stored {
        match value_type {
            ValueType::Float => match parse_f64(&self.value) {
                Some(number) => Stored::Number(number),
                None => store.text(&self.value),
            },
            ValueType::Bool => match self.value.trim_ascii() {
                b"true" => Stored::Bool(true),
                b"false" => Stored::Bool(false),
                _ => store.text(&self.value),
            },
            ValueType::Date => match wall_from_iso(&self.value) {
                Some(cell) => Stored::from_cell(cell, store),
                None => store.text(&self.value),
            },
            ValueType::Time => match cast_duration(&self.value) {
                Ok(span) => Stored::Span(span),
                Err(_) => store.text(&self.value),
            },
            ValueType::String => {
                if self.value_present {
                    store.text(&self.value)
                } else {
                    store.text(&self.text)
                }
            }
            ValueType::Error => match CellError::from_text(self.text.trim_ascii()) {
                Some(error) => Stored::Error(error),
                None => store.text(&self.text),
            },
            ValueType::None => {
                if self.text.is_empty() {
                    Stored::Empty
                } else {
                    store.text(&self.text)
                }
            }
        }
    }
}
