//! XLSX: the OPC package resolution (`_rels/.rels` → workbook part → its relationships →
//! sheets, styles, shared strings), the style table's number-format kinds, and the
//! shared-string preload. The sheet stream itself is in [`sheet`].
//!
//! The part-resolution order is Sylvan's; everything is read through the streaming
//! tokenizer — the small parts are read whole into memory first, the shared-string
//! table streams straight into its arena.

pub mod sheet;
pub mod styles;

use crate::ExcelEpoch;
use crate::workbook::error::Error;
use crate::workbook::workbook::{Shared, SheetInfo, SheetPart};
use crate::workbook::xml::{self, Event, Reader};
use crate::workbook::zip::Archive;
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use styles::NumberKind;

/// True when the container looks like an OPC package.
pub(crate) fn is_xlsx<R>(archive: &Archive<R>) -> bool {
    archive.find("[Content_Types].xml").is_some() || archive.find("xl/workbook.xml").is_some()
}

fn read_part<R: Read + Seek>(archive: &mut Archive<R>, name: &str) -> Result<Vec<u8>, Error> {
    let index = archive
        .find(name)
        .ok_or_else(|| Error::malformed(name, "part missing"))?;
    archive.read(index)
}

fn xml_error(part: &str, error: xml::Malformed) -> Error {
    Error::malformed(part, error.0)
}

/// Resolves `target` against the directory `dir` (which ends with `/` or is empty):
/// rooted targets are stripped of their leading slash, `..` segments are folded.
fn resolve(dir: &str, target: &str) -> String {
    let joined = if let Some(rooted) = target.strip_prefix('/') {
        rooted.to_string()
    } else {
        format!("{dir}{target}")
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// The directory of a part name, with its trailing slash (`""` for the root).
fn dir_of(part: &str) -> (&str, &str) {
    match part.rfind('/') {
        Some(slash) => (&part[..slash + 1], &part[slash + 1..]),
        None => ("", part),
    }
}

/// One relationship: `(Id, Type, resolved Target)`.
struct Relationship {
    id: Vec<u8>,
    kind: Vec<u8>,
    target: String,
}

fn relationships<R: Read + Seek>(
    archive: &mut Archive<R>,
    part: &str,
    dir: &str,
) -> Result<Vec<Relationship>, Error> {
    let Some(index) = archive.find(part) else {
        return Ok(Vec::new());
    };
    let bytes = archive.read(index)?;
    let mut reader = Reader::new(Cursor::new(bytes));
    let mut scratch = Vec::new();
    let mut out = Vec::new();
    loop {
        match reader.next().map_err(|e| xml_error(part, e))? {
            Event::Start(tag) | Event::Empty(tag) if tag.local() == b"Relationship" => {
                if tag
                    .attr(b"TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case(b"External"))
                {
                    continue;
                }
                let (Some(id), Some(kind), Some(target)) =
                    (tag.attr(b"Id"), tag.attr(b"Type"), tag.attr(b"Target"))
                else {
                    continue;
                };
                let target =
                    String::from_utf8_lossy(xml::unescape(target, &mut scratch)).into_owned();
                out.push(Relationship {
                    id: id.to_vec(),
                    kind: kind.to_vec(),
                    target: resolve(dir, &target),
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// The workbook part per `_rels/.rels`, or the conventional default.
fn workbook_part<R: Read + Seek>(archive: &mut Archive<R>) -> Result<String, Error> {
    let rels = relationships(archive, "_rels/.rels", "")?;
    Ok(rels
        .into_iter()
        .find(|rel| rel.kind.ends_with(b"/officeDocument"))
        .map(|rel| rel.target)
        .unwrap_or_else(|| "xl/workbook.xml".to_string()))
}

/// Opens the package: sheets, date system, styles, shared strings.
pub(crate) fn open<R: Read + Seek>(
    archive: &mut Archive<R>,
) -> Result<(Vec<SheetInfo>, Shared), Error> {
    let workbook = workbook_part(archive)?;
    let (dir, base) = dir_of(&workbook);
    let rels = relationships(archive, &format!("{dir}_rels/{base}.rels"), dir)?;
    let mut shared = Shared {
        date_system: ExcelEpoch::Y1900,
        ..Shared::default()
    };

    // workbook.xml: sheets and the date system.
    let bytes = read_part(archive, &workbook)?;
    let mut reader = Reader::new(Cursor::new(bytes));
    let mut scratch = Vec::new();
    let mut sheets = Vec::new();
    loop {
        match reader.next().map_err(|e| xml_error(&workbook, e))? {
            Event::Start(tag) | Event::Empty(tag) => match tag.local() {
                b"sheet" => {
                    let name = tag
                        .attr(b"name")
                        .map(|raw| {
                            String::from_utf8_lossy(xml::unescape(raw, &mut scratch)).into_owned()
                        })
                        .unwrap_or_default();
                    let hidden = tag
                        .attr(b"state")
                        .is_some_and(|state| state == b"hidden" || state == b"veryHidden");
                    let Some(rid) = tag.attr(b"id") else { continue };
                    let Some(rel) = rels.iter().find(|rel| rel.id == rid) else {
                        continue;
                    };
                    if !rel.kind.ends_with(b"/worksheet") {
                        // Chart sheets, dialog sheets, macro sheets: no cells.
                        continue;
                    }
                    sheets.push(SheetInfo {
                        name,
                        hidden,
                        part: SheetPart::Xlsx(rel.target.clone()),
                    });
                }
                b"workbookPr"
                    if tag
                        .attr(b"date1904")
                        .is_some_and(|v| v == b"1" || v.eq_ignore_ascii_case(b"true")) =>
                {
                    shared.date_system = ExcelEpoch::Y1904;
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }

    // styles.xml: cellXfs → number-format kind.
    let styles_part = rels
        .iter()
        .find(|rel| rel.kind.ends_with(b"/styles"))
        .map(|rel| rel.target.clone())
        .unwrap_or_else(|| format!("{dir}styles.xml"));
    if let Some(index) = archive.find(&styles_part) {
        let bytes = archive.read(index)?;
        shared.styles = read_styles(&styles_part, bytes)?;
    }

    // sharedStrings.xml: streamed into the arena.
    let sst_part = rels
        .iter()
        .find(|rel| rel.kind.ends_with(b"/sharedStrings"))
        .map(|rel| rel.target.clone())
        .unwrap_or_else(|| format!("{dir}sharedStrings.xml"));
    shared.sst_offsets.push(0);
    if let Some(index) = archive.find(&sst_part) {
        let stream = archive.stream(index)?;
        read_shared_strings(&sst_part, stream, &mut shared)?;
    }
    Ok((sheets, shared))
}

fn read_styles(part: &str, bytes: Vec<u8>) -> Result<Vec<NumberKind>, Error> {
    let mut reader = Reader::new(Cursor::new(bytes));
    let mut scratch = Vec::new();
    let mut custom: HashMap<u32, NumberKind> = HashMap::new();
    let mut kinds = Vec::new();
    let mut in_cell_xfs = false;
    loop {
        match reader.next().map_err(|e| xml_error(part, e))? {
            Event::Start(tag) | Event::Empty(tag) => match tag.local() {
                b"numFmt" => {
                    if let (Some(id), Some(code)) = (
                        tag.attr(b"numFmtId").and_then(parse_u32),
                        tag.attr(b"formatCode"),
                    ) {
                        custom.insert(id, styles::classify(xml::unescape(code, &mut scratch)));
                    }
                }
                b"cellXfs" => in_cell_xfs = true,
                b"xf" if in_cell_xfs => {
                    let id = tag.attr(b"numFmtId").and_then(parse_u32).unwrap_or(0);
                    kinds.push(
                        custom
                            .get(&id)
                            .copied()
                            .unwrap_or_else(|| styles::builtin(id)),
                    );
                }
                _ => {}
            },
            Event::End(name) if xml::local_name(name) == b"cellXfs" => in_cell_xfs = false,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(kinds)
}

/// Appends the text of a `<t>`, `<is>`, or `<si>` subtree — every `t` run concatenated,
/// `rPh` phonetics skipped — to `out`. The reader is positioned just after the
/// container's start tag; returns after its end tag.
pub(crate) fn collect_rich_text<R: Read>(
    part: &str,
    reader: &mut Reader<R>,
    scratch: &mut Vec<u8>,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    let mut depth = 1usize;
    loop {
        match reader.next().map_err(|e| xml_error(part, e))? {
            Event::Start(tag) => {
                if tag.local() == b"rPh" {
                    reader.skip_subtree().map_err(|e| xml_error(part, e))?;
                } else {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Text(text) => out.extend_from_slice(xml::unescape(text, scratch)),
            Event::CData(text) => out.extend_from_slice(text),
            Event::Empty(_) => {}
            Event::Eof => {
                return Err(Error::malformed(
                    part,
                    "unexpected end of input inside a string",
                ));
            }
        }
    }
}

fn read_shared_strings<R: Read>(part: &str, stream: R, shared: &mut Shared) -> Result<(), Error> {
    let mut reader = Reader::new(stream);
    let mut scratch = Vec::new();
    loop {
        match reader.next().map_err(|e| xml_error(part, e))? {
            Event::Start(tag) if tag.local() == b"si" => {
                collect_rich_text(part, &mut reader, &mut scratch, &mut shared.sst_bytes)?;
                shared.sst_offsets.push(shared.sst_bytes.len() as u32);
            }
            Event::Empty(tag) if tag.local() == b"si" => {
                shared.sst_offsets.push(shared.sst_bytes.len() as u32)
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub(crate) fn parse_u32(bytes: &[u8]) -> Option<u32> {
    let bytes = bytes.trim_ascii();
    if bytes.is_empty() || bytes.len() > 10 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u64::from(b - b'0');
    }
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_like_opc_says() {
        assert_eq!(
            resolve("xl/", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(resolve("xl/", "/xl/styles.xml"), "xl/styles.xml");
        assert_eq!(resolve("xl/", "../docProps/x.xml"), "docProps/x.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(dir_of("xl/workbook.xml"), ("xl/", "workbook.xml"));
        assert_eq!(dir_of("book.xml"), ("", "book.xml"));
        assert_eq!(parse_u32(b" 42 "), Some(42));
        assert_eq!(parse_u32(b"4x"), None);
        assert_eq!(parse_u32(b"99999999999"), None);
    }
}
