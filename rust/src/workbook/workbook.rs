//! The workbook: opens the container, tells XLSX from ODS by what the container holds
//! (never by file extension), loads what every sheet needs once — shared strings, the
//! style table's number-format kinds, the date system — and opens sheets as independent
//! forward-only cursors.

use crate::ExcelEpoch;
use crate::workbook::error::Error;
use crate::workbook::ods;
use crate::workbook::sheet::{Parser, Sheet, SheetOptions};
use crate::workbook::source::{FileSource, Source};
use crate::workbook::xlsx;
use crate::workbook::zip::Archive;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// Which format the container turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Office Open XML SpreadsheetML (`.xlsx`, `.xlsm`).
    Xlsx,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
}

impl Format {
    /// The ABI discriminant: `1` XLSX, `2` ODS.
    pub const fn code(self) -> i64 {
        match self {
            Format::Xlsx => 1,
            Format::Ods => 2,
        }
    }
}

/// Where a sheet's data lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SheetPart {
    /// The zip entry of the XLSX worksheet part.
    Xlsx(String),
    /// The zero-based index of the `table:table` in ODS `content.xml`.
    Ods(usize),
}

/// One sheet as the workbook lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetInfo {
    /// The sheet's name as written.
    pub name: String,
    /// Hidden (`state="hidden"`/`"veryHidden"` in XLSX). ODS hidden state lives in styles
    /// this reader does not consult; it is always `false` there.
    pub hidden: bool,
    pub(crate) part: SheetPart,
}

/// What every sheet of a workbook shares.
#[derive(Debug)]
pub struct Shared {
    /// Every shared string, concatenated.
    pub(crate) sst_bytes: Vec<u8>,
    /// `n + 1` offsets into `sst_bytes`.
    pub(crate) sst_offsets: Vec<u32>,
    /// `cellXfs` index → number-format kind.
    pub(crate) styles: Vec<xlsx::styles::NumberKind>,
    /// The date system serials are read under.
    pub date_system: ExcelEpoch,
}

impl Default for Shared {
    /// No strings, no styles, and Excel's own default date system.
    fn default() -> Shared {
        Shared {
            sst_bytes: Vec::new(),
            sst_offsets: Vec::new(),
            styles: Vec::new(),
            date_system: ExcelEpoch::Y1900,
        }
    }
}

impl Shared {
    /// Shared string `index`, or empty bytes past the table (parsers validate indices).
    pub(crate) fn shared_string(&self, index: usize) -> &[u8] {
        match (self.sst_offsets.get(index), self.sst_offsets.get(index + 1)) {
            (Some(&start), Some(&end)) => &self.sst_bytes[start as usize..end as usize],
            _ => &[],
        }
    }

    /// The number of shared strings.
    pub fn shared_string_count(&self) -> usize {
        self.sst_offsets.len().saturating_sub(1)
    }
}

/// An open workbook. See the module doc.
pub struct Workbook<R> {
    archive: Archive<R>,
    format: Format,
    sheets: Vec<SheetInfo>,
    shared: Arc<Shared>,
}

impl<'a> Workbook<Cursor<&'a [u8]>> {
    /// Opens a workbook over bytes already in memory.
    pub fn from_slice(bytes: &'a [u8]) -> Result<Workbook<Cursor<&'a [u8]>>, Error> {
        Workbook::open(Cursor::new(bytes))
    }
}

impl Workbook<Cursor<Arc<[u8]>>> {
    /// Opens a workbook over owned bytes.
    pub fn from_vec(bytes: Vec<u8>) -> Result<Workbook<Cursor<Arc<[u8]>>>, Error> {
        Workbook::open(Cursor::new(Arc::from(bytes)))
    }
}

impl Workbook<FileSource> {
    /// Opens a workbook file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Workbook<FileSource>, Error> {
        Workbook::open(FileSource::open(path)?)
    }
}

impl<R: Source> Workbook<R> {
    /// Opens a workbook over any [`Source`].
    pub fn open(reader: R) -> Result<Workbook<R>, Error> {
        let mut archive = Archive::open(reader)?;
        let (format, sheets, shared) = if xlsx::is_xlsx(&archive) {
            let (sheets, shared) = xlsx::open(&mut archive)?;
            (Format::Xlsx, sheets, shared)
        } else if ods::is_ods(&mut archive)? {
            let (sheets, shared) = ods::open(&mut archive)?;
            (Format::Ods, sheets, shared)
        } else {
            return Err(Error::NotAWorkbook);
        };
        Ok(Workbook {
            archive,
            format,
            sheets,
            shared: Arc::new(shared),
        })
    }

    /// Which format the container is.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The sheets, in workbook order, hidden ones included.
    pub fn sheets(&self) -> &[SheetInfo] {
        &self.sheets
    }

    /// The date system serials are read under.
    pub fn date_system(&self) -> ExcelEpoch {
        self.shared.date_system
    }

    /// What every sheet shares.
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// Opens sheet `index` (workbook order) as an independent forward-only cursor.
    pub fn sheet(&self, index: usize, options: SheetOptions) -> Result<Sheet<R>, Error> {
        let info = self
            .sheets
            .get(index)
            .ok_or_else(|| Error::SheetNotFound(format!("#{index}")))?;
        let archive = self.archive.with_reader(self.archive.reader().reopen()?);
        let parser = match &info.part {
            SheetPart::Xlsx(part) => {
                let entry = archive
                    .find(part)
                    .ok_or_else(|| Error::malformed(part, "worksheet part missing"))?;
                let stream = archive.into_stream(entry)?;
                Parser::Xlsx(xlsx::sheet::XlsxParser::new(stream, part.clone()))
            }
            SheetPart::Ods(table) => {
                let entry = archive
                    .find("content.xml")
                    .ok_or_else(|| Error::malformed("content.xml", "missing"))?;
                let stream = archive.into_stream(entry)?;
                Parser::Ods(ods::OdsParser::new(stream, *table))
            }
        };
        Sheet::new(parser, Arc::clone(&self.shared), options)
    }

    /// Opens the first sheet named exactly `name`.
    pub fn sheet_named(&self, name: &str, options: SheetOptions) -> Result<Sheet<R>, Error> {
        let index = self
            .sheets
            .iter()
            .position(|info| info.name == name)
            .ok_or_else(|| Error::SheetNotFound(format!("named {name:?}")))?;
        self.sheet(index, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbook::zip::write;
    use crate::{Cell, CellError, Date};
    use hypercast::Duration;

    const NS: &str = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"";

    /// A minimal package: one sheet, an optional shared-string table and style sheet.
    fn package(sheet: &str, sst: Option<&str>, styles: Option<&str>, date1904: bool) -> Vec<u8> {
        let workbook = format!(
            "<workbook {NS} xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><workbookPr date1904=\"{}\"/><sheets><sheet name=\"S\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Chart\" sheetId=\"2\" r:id=\"rId9\"/></sheets></workbook>",
            u8::from(date1904)
        );
        let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"/xl/worksheets/sheet1.xml\"/><Relationship Id=\"rId9\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet\" Target=\"chartsheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/></Relationships>";
        let root = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";
        let sheet = format!(
            "<worksheet {NS} xmlns:x=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{sheet}</worksheet>"
        );
        let mut entries: Vec<(&str, Vec<u8>, bool)> = vec![
            ("[Content_Types].xml", b"<Types/>".to_vec(), false),
            ("_rels/.rels", root.as_bytes().to_vec(), true),
            ("xl/workbook.xml", workbook.into_bytes(), true),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes().to_vec(), true),
            ("xl/worksheets/sheet1.xml", sheet.into_bytes(), true),
        ];
        if let Some(sst) = sst {
            entries.push((
                "xl/sharedStrings.xml",
                format!("<sst {NS}>{sst}</sst>").into_bytes(),
                true,
            ));
        }
        if let Some(styles) = styles {
            entries.push((
                "xl/styles.xml",
                format!("<styleSheet {NS}>{styles}</styleSheet>").into_bytes(),
                true,
            ));
        }
        let borrowed: Vec<(&str, &[u8], bool)> = entries
            .iter()
            .map(|(n, c, d)| (*n, c.as_slice(), *d))
            .collect();
        write::build(&borrowed)
    }

    /// A cell that outlives its row (text is leaked; tests are short-lived).
    fn own(cell: Cell<'_>) -> Cell<'static> {
        match cell {
            Cell::Empty => Cell::Empty,
            Cell::Text(text) => Cell::Text(Box::leak(text.to_vec().into_boxed_slice())),
            Cell::Number(v) => Cell::Number(v),
            Cell::Bool(v) => Cell::Bool(v),
            Cell::Wall { date, nanos } => Cell::Wall { date, nanos },
            Cell::Clock(v) => Cell::Clock(v),
            Cell::Span(v) => Cell::Span(v),
            Cell::Error(v) => Cell::Error(v),
        }
    }

    fn rows(bytes: &[u8], options: SheetOptions) -> Vec<(u32, Vec<Cell<'static>>)> {
        let workbook = Workbook::from_slice(bytes).unwrap();
        let mut sheet = workbook.sheet(0, options).unwrap();
        let mut out = Vec::new();
        while let Some(row) = sheet.next_row().unwrap() {
            let cells: Vec<Cell<'static>> = row.iter().map(own).collect();
            out.push((row.number(), cells));
        }
        out
    }

    #[test]
    fn every_cell_shape_the_spec_allows() {
        let sheet = "<dimension ref=\"A1:F9\"/><sheetData>\
            <row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c t=\"s\"><v>1</v></c><c r=\"D1\" t=\"inlineStr\"><is><t>in</t><r><t xml:space=\"preserve\"> line</t></r><rPh sb=\"0\"><t>skip</t></rPh></is></c></row>\
            <row r=\"2\"><x:c r=\"A2\"><x:v>1.5</x:v></x:c><c r=\"B2\" t=\"b\"><v>1</v></c><c r=\"C2\" t=\"e\"><v>#N/A</v></c><c r=\"D2\" t=\"str\"><f>A2&amp;\"x\"</f><v>1.5x</v></c><c r=\"E2\" t=\"d\"><v>1976-11-22T08:30Z</v></c><c r=\"F2\" s=\"1\"><v>45000.5</v></c></row>\
            <row r=\"3\"><c r=\"A3\" s=\"2\"><v>0.75</v></c><c r=\"B3\" s=\"3\"><v>1.5</v></c><c r=\"C3\" s=\"4\"><v>7</v></c><c r=\"D3\" s=\"1\"><v>-1</v></c><c r=\"E3\"><f>1/0</f></c><c r=\"F3\"><v>abc</v></c></row>\
            <row r=\"5\"/>\
            <row r=\"7\"><c r=\"B7\" t=\"s\"><v>1</v></c><c r=\"C7\"/></row>\
            <row><c><v>9</v></c><c><v>10</v></c></row>\
            </sheetData><mergeCells/>";
        let sst = "<si><t>alpha</t></si><si><r><rPr><b/></rPr><t>be</t></r><r><t>ta</t></r></si>";
        let styles = "<numFmts count=\"2\"><numFmt numFmtId=\"164\" formatCode=\"[h]:mm\"/><numFmt numFmtId=\"165\" formatCode=\"&quot;Day &quot;d\"/></numFmts><cellXfs count=\"5\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/><xf numFmtId=\"21\"/><xf numFmtId=\"164\"/><xf numFmtId=\"165\"/></cellXfs>";
        let bytes = package(sheet, Some(sst), Some(styles), false);
        let workbook = Workbook::from_slice(&bytes).unwrap();
        assert_eq!(workbook.sheets().len(), 1, "the chart sheet is not a sheet");
        assert_eq!(workbook.shared().shared_string_count(), 2);

        let rows = rows(&bytes, SheetOptions::default().with_header(false));
        let (number, cells) = &rows[0];
        assert_eq!(*number, 1);
        assert_eq!(cells[0], Cell::Text(b"alpha"));
        assert_eq!(
            cells[1],
            Cell::Text(b"beta"),
            "rich runs concatenate, the missing r means B"
        );
        assert_eq!(cells[2], Cell::Empty);
        assert_eq!(cells[3], Cell::Text(b"in line"));

        let cells = &rows[1].1;
        assert_eq!(cells[0], Cell::Number(1.5), "a prefixed <x:c> is a cell");
        assert_eq!(cells[1], Cell::Bool(true));
        assert_eq!(cells[2], Cell::Error(CellError::NotAvailable));
        assert_eq!(
            cells[3],
            Cell::Text(b"1.5x"),
            "the cached formula string, not the formula"
        );
        assert_eq!(
            cells[4],
            Cell::Wall {
                date: Date {
                    year: 1976,
                    month: 11,
                    day: 22
                },
                nanos: 30_600_000_000_000
            }
        );
        assert_eq!(
            cells[5],
            Cell::Wall {
                date: Date {
                    year: 2023,
                    month: 3,
                    day: 15
                },
                nanos: 43_200_000_000_000
            }
        );

        let cells = &rows[2].1;
        assert_eq!(
            cells[0],
            Cell::Clock(64_800_000_000_000),
            "a time format under 1 is a clock"
        );
        assert_eq!(
            cells[1],
            Cell::Span(Duration {
                seconds: 129_600,
                nanos: 0
            }),
            "[h]:mm is elapsed"
        );
        assert_eq!(
            cells[2],
            Cell::Wall {
                date: Date {
                    year: 1900,
                    month: 1,
                    day: 7
                },
                nanos: 0
            },
            "\"Day \"d is a date format"
        );
        assert_eq!(
            cells[3],
            Cell::Number(-1.0),
            "a negative date serial stays a number"
        );
        assert_eq!(cells[4], Cell::Empty, "a formula without a cached value");
        assert_eq!(
            cells[5],
            Cell::Text(b"abc"),
            "an unparsable number is delivered as text"
        );

        assert_eq!(rows[3].0, 7, "rows 4-6 are empty and skipped");
        assert_eq!(
            rows[3].1,
            vec![Cell::Empty, Cell::Text(b"beta")],
            "trailing empties trimmed"
        );
        assert_eq!(
            rows[4],
            (8, vec![Cell::Number(9.0), Cell::Number(10.0)]),
            "no r on row or cells"
        );
        assert_eq!(rows.len(), 5);

        let numbers: Vec<u32> = rows_numbers(&bytes);
        assert_eq!(numbers, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    fn rows_numbers(bytes: &[u8]) -> Vec<u32> {
        rows(
            bytes,
            SheetOptions::default()
                .with_header(false)
                .with_empty_rows_skipped(false),
        )
        .into_iter()
        .map(|(n, _)| n)
        .collect()
    }

    #[test]
    fn the_1904_system_and_the_header_come_from_the_file() {
        let sheet = "<sheetData><row><c t=\"inlineStr\"><is><t>when</t></is></c><c t=\"inlineStr\"><is><t>n</t></is></c></row><row><c s=\"1\"><v>1</v></c><c><v>2</v></c></row></sheetData>";
        let styles = "<cellXfs><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/></cellXfs>";
        let bytes = package(sheet, None, Some(styles), true);
        let workbook = Workbook::from_slice(&bytes).unwrap();
        assert_eq!(workbook.date_system(), ExcelEpoch::Y1904);
        let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
        assert_eq!(sheet.header().unwrap().ordinal(b"n"), Some(1));
        let row = sheet.next_row().unwrap().unwrap();
        assert_eq!(
            row.cell(0),
            Cell::Wall {
                date: Date {
                    year: 1904,
                    month: 1,
                    day: 2
                },
                nanos: 0
            }
        );
        assert_eq!(row.number(), 2);
        assert!(sheet.next_row().unwrap().is_none());
    }

    #[test]
    fn a_bad_shared_string_index_is_structural() {
        let bytes = package(
            "<sheetData><row><c t=\"s\"><v>5</v></c></row></sheetData>",
            Some("<si><t>x</t></si>"),
            None,
            false,
        );
        let workbook = Workbook::from_slice(&bytes).unwrap();
        let error = workbook
            .sheet(0, SheetOptions::default())
            .err()
            .expect("a bad index is structural");
        assert!(matches!(error, Error::Malformed { .. }), "{error}");
        let mut sheet = workbook
            .sheet(0, SheetOptions::default().with_header(false))
            .unwrap();
        assert!(sheet.next_row().is_err());
        assert!(sheet.next_row().is_err(), "sticky");
    }
}
