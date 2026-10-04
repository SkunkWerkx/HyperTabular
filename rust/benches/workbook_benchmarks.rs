//! Throughput receipts for the workbook path, on files built in memory so nothing large
//! is committed: an XLSX and an ODS of the same 100 000 × 8 table (integers, doubles, a
//! shared string, an inline string, a date serial, a time serial, a boolean, an
//! elapsed serial). Two scopes each — rows delivered with no cell touched, and every cell
//! cast through a HyperCast door into a typed batch — plus the inflate + tokenizer floor
//! (the sheet part streamed through the XML reader alone).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::workbook::xml::{Event, Reader};
use hypertabular::workbook::zip::{Archive, write};
use hypertabular::workbook::{SheetOptions, Workbook};
use hypertabular::{Batch, Column, Door, Plan, fill_batch};
use std::fmt::Write as _;
use std::io::Cursor;

const ROWS: usize = 100_000;

fn xlsx() -> Vec<u8> {
    let mut sheet = String::with_capacity(ROWS * 300);
    sheet.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>");
    sheet.push_str("<row r=\"1\"><c t=\"s\"><v>0</v></c><c t=\"s\"><v>1</v></c><c t=\"s\"><v>2</v></c><c t=\"s\"><v>3</v></c><c t=\"s\"><v>4</v></c><c t=\"s\"><v>5</v></c><c t=\"s\"><v>6</v></c><c t=\"s\"><v>7</v></c></row>");
    for i in 0..ROWS {
        let r = i + 2;
        let _ = write!(
            sheet,
            "<row r=\"{r}\"><c r=\"A{r}\"><v>{i}</v></c><c r=\"B{r}\"><v>{}.{:02}</v></c><c r=\"C{r}\" t=\"s\"><v>{}</v></c><c r=\"D{r}\" t=\"inlineStr\"><is><t>note {i}</t></is></c><c r=\"E{r}\" s=\"1\"><v>{}</v></c><c r=\"F{r}\" s=\"2\"><v>0.{:03}</v></c><c r=\"G{r}\" t=\"b\"><v>{}</v></c><c r=\"H{r}\" s=\"3\"><v>{}.5</v></c></row>",
            i % 100_000,
            i % 100,
            8 + i % 100,
            45_000 + i % 365,
            i % 1000,
            i % 2,
            i % 50
        );
    }
    sheet.push_str("</sheetData></worksheet>");
    let mut sst = String::new();
    sst.push_str("<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">");
    for name in [
        "id", "amount", "city", "note", "when", "at", "active", "elapsed",
    ] {
        let _ = write!(sst, "<si><t>{name}</t></si>");
    }
    for i in 0..100 {
        let _ = write!(sst, "<si><t>City {i}</t></si>");
    }
    sst.push_str("</sst>");
    let styles = "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"[h]:mm:ss\"/></numFmts><cellXfs count=\"4\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/><xf numFmtId=\"21\"/><xf numFmtId=\"164\"/></cellXfs></styleSheet>";
    let workbook = "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/></Relationships>";
    let root_rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";
    write::build(&[
        ("[Content_Types].xml", b"<Types/>", false),
        ("_rels/.rels", root_rels.as_bytes(), true),
        ("xl/workbook.xml", workbook.as_bytes(), true),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes(), true),
        ("xl/styles.xml", styles.as_bytes(), true),
        ("xl/sharedStrings.xml", sst.as_bytes(), true),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes(), true),
    ])
}

fn ods() -> Vec<u8> {
    let mut content = String::with_capacity(ROWS * 500);
    content.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\"><office:body><office:spreadsheet><table:table table:name=\"Data\">");
    content.push_str("<table:table-row>");
    for name in [
        "id", "amount", "city", "note", "when", "at", "active", "elapsed",
    ] {
        let _ = write!(
            content,
            "<table:table-cell office:value-type=\"string\"><text:p>{name}</text:p></table:table-cell>"
        );
    }
    content.push_str("</table:table-row>");
    for i in 0..ROWS {
        let _ = write!(
            content,
            "<table:table-row><table:table-cell office:value-type=\"float\" office:value=\"{i}\"><text:p>{i}</text:p></table:table-cell><table:table-cell office:value-type=\"float\" office:value=\"{}.{:02}\"><text:p>x</text:p></table:table-cell><table:table-cell office:value-type=\"string\"><text:p>City {}</text:p></table:table-cell><table:table-cell office:value-type=\"string\"><text:p>note {i}</text:p></table:table-cell><table:table-cell office:value-type=\"date\" office:date-value=\"2023-{:02}-{:02}\"><text:p>d</text:p></table:table-cell><table:table-cell office:value-type=\"time\" office:time-value=\"PT{:02}H{:02}M00S\"><text:p>t</text:p></table:table-cell><table:table-cell office:value-type=\"boolean\" office:boolean-value=\"{}\"><text:p>b</text:p></table:table-cell><table:table-cell office:value-type=\"time\" office:time-value=\"PT{}H30M00S\"><text:p>e</text:p></table:table-cell></table:table-row>",
            i % 100_000,
            i % 100,
            i % 100,
            i % 12 + 1,
            i % 28 + 1,
            i % 24,
            i % 60,
            if i % 2 == 0 { "true" } else { "false" },
            i % 50
        );
    }
    content.push_str("</table:table></office:spreadsheet></office:body></office:document-content>");
    write::build(&[
        ("mimetype", b"application/vnd.oasis.opendocument.spreadsheet", false),
        ("META-INF/manifest.xml", b"<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\"/>", true),
        ("content.xml", content.as_bytes(), true),
    ])
}

fn plan() -> Plan {
    Plan::new(vec![
        Column::new(0, Door::I64),
        Column::new(1, Door::F64),
        Column::new(2, Door::Text),
        Column::new(3, Door::Text),
        Column::new(4, Door::Date),
        Column::new(5, Door::Time),
        Column::new(6, Door::Bool),
        Column::new(7, Door::Duration),
    ])
}

fn benchmarks(c: &mut Criterion) {
    for (label, file, part) in [
        ("xlsx", xlsx(), "xl/worksheets/sheet1.xml"),
        ("ods", ods(), "content.xml"),
    ] {
        let plan = plan();
        let mut group = c.benchmark_group(label);
        let mut archive = Archive::open(Cursor::new(&file[..])).unwrap();
        let entry = archive.find(part).unwrap();
        let uncompressed = archive.entries()[entry].size();
        group.throughput(Throughput::Bytes(uncompressed));
        group.bench_function("inflate+tokenize", |b| {
            b.iter(|| {
                let mut reader = Reader::new(archive.stream(entry).unwrap());
                let mut events = 0u64;
                while !matches!(reader.next().unwrap(), Event::Eof) {
                    events += 1;
                }
                events
            });
        });
        group.bench_function("rows", |b| {
            b.iter(|| {
                let workbook = Workbook::from_slice(&file).unwrap();
                let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
                let mut rows = 0u64;
                while sheet.next_row().unwrap().is_some() {
                    rows += 1;
                }
                rows
            });
        });
        group.bench_function("typed batch", |b| {
            let mut batch = Batch::new();
            b.iter(|| {
                let workbook = Workbook::from_slice(&file).unwrap();
                let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
                let mut rows = 0usize;
                while fill_batch(&mut sheet, &plan, &mut batch, 1024).unwrap() > 0 {
                    rows += batch.rows();
                }
                rows
            });
        });
        group.finish();
    }
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
