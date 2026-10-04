//! Workbook packages built in memory for the core's tests: XLSX and ODS, by hand and by a
//! seeded generator that reaches for every shape a cell, a row and a part can take.

#![allow(dead_code, clippy::should_implement_trait)]

use std::io::Write;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A zip of `entries` — each a name, its content, and whether to deflate it — with no
/// zip64 and nothing optional: what the tests build their packages with.
pub fn build(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for &(name, content, deflate) in entries {
        let data = if deflate {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(content).unwrap();
            encoder.finish().unwrap()
        } else {
            content.to_vec()
        };
        let method: u16 = if deflate { 8 } else { 0 };
        let offset = out.len() as u32;
        let crc = crc32(content);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0; 4]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(content.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let directory = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A small deterministic generator (an LCG; the tests want the same packages every run).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    pub fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }

    pub fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len() as u64) as usize]
    }
}

const MAIN: &str = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"";

/// Cell formats the generated sheets use, by `s`: general, two built-in dates, the built-in
/// elapsed and text formats, a built-in time, and three custom ones (a date, an elapsed, a
/// plain number) — one of them declared twice, so the later declaration has to win.
pub const STYLES: &str = "<numFmts count=\"4\">\
<numFmt numFmtId=\"164\" formatCode=\"0.00\"/>\
<numFmt numFmtId=\"166\" formatCode=\"0.000\"/>\
<numFmt numFmtId=\"165\" formatCode=\"[h]:mm:ss\"/>\
<numFmt numFmtId=\"164\" formatCode=\"yyyy\\-mm\\-dd&quot; at &quot;hh:mm\"/>\
</numFmts>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"14\"/></cellStyleXfs>\
<cellXfs count=\"9\">\
<xf numFmtId=\"0\"/><xf numFmtId=\"14\"/><xf numFmtId=\"22\"/><xf numFmtId=\"46\"/>\
<xf numFmtId=\"49\"/><xf numFmtId=\"20\"/><xf numFmtId=\"164\"/><xf numFmtId=\"165\"/>\
<xf numFmtId=\"166\"/></cellXfs>";

/// An XLSX package: each sheet's name and its `<sheetData>` content, the shared strings'
/// `<si>` elements, and whether to deflate the parts.
pub fn xlsx(sheets: &[(&str, &str)], strings: &str, date1904: bool, deflate: bool) -> Vec<u8> {
    let mut workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook {MAIN} xmlns:r=\"r\">\
<workbookPr{}/><sheets>",
        if date1904 { " date1904=\"1\"" } else { "" }
    );
    // The relationships are written in an order that is not the sheets', so a sheet has
    // to be found by its id.
    let mut rels = String::from("<Relationships>");
    rels.push_str("<Relationship Id=\"rIdS\" Type=\"http://x/styles\" Target=\"styles.xml\"/>");
    for (index, _) in sheets.iter().enumerate().rev() {
        rels.push_str(&format!(
            "<Relationship Id=\"rId{index}\" Type=\"http://x/worksheet\" \
Target=\"worksheets/sheet{index}.xml\"/>"
        ));
    }
    rels.push_str(
        "<Relationship Id=\"rIdT\" Type=\"http://x/sharedStrings\" Target=\"/xl/sharedStrings.xml\"/>\
<Relationship Id=\"rIdX\" Type=\"http://x/hyperlink\" Target=\"http://example.com\" TargetMode=\"External\"/>\
<Relationship Id=\"rIdC\" Type=\"http://x/chartsheet\" Target=\"chartsheets/sheet1.xml\"/>\
</Relationships>",
    );
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for (index, (name, data)) in sheets.iter().enumerate() {
        let state = if index % 3 == 2 {
            " state=\"hidden\""
        } else {
            ""
        };
        workbook.push_str(&format!(
            "<sheet name=\"{name}\" sheetId=\"{}\"{state} r:id=\"rId{index}\"/>",
            index + 1
        ));
        parts.push((
            format!("xl/worksheets/sheet{index}.xml"),
            format!(
                "<?xml version=\"1.0\"?><worksheet {MAIN}><dimension ref=\"A1\"/>\
<sheetViews><sheetView workbookViewId=\"0\"/></sheetViews><sheetData>{data}</sheetData>\
<pageMargins left=\"0.7\"/></worksheet>"
            )
            .into_bytes(),
        ));
    }
    // A chart sheet is listed and has no cells; it must not be a sheet.
    workbook.push_str("<sheet name=\"Chart\" sheetId=\"99\" r:id=\"rIdC\"/></sheets></workbook>");
    parts.push(("[Content_Types].xml".into(), b"<Types/>".to_vec()));
    parts.push((
        "_rels/.rels".into(),
        b"<Relationships><Relationship Id=\"rId1\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
Target=\"xl/workbook.xml\"/></Relationships>"
            .to_vec(),
    ));
    parts.push(("xl/workbook.xml".into(), workbook.into_bytes()));
    parts.push(("xl/_rels/workbook.xml.rels".into(), rels.into_bytes()));
    parts.push((
        "xl/styles.xml".into(),
        format!("<styleSheet {MAIN}>{STYLES}</styleSheet>").into_bytes(),
    ));
    parts.push((
        "xl/sharedStrings.xml".into(),
        format!("<sst {MAIN} count=\"9\" uniqueCount=\"7\">{strings}</sst>").into_bytes(),
    ));
    let entries: Vec<(&str, &[u8], bool)> = parts
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_slice(), deflate))
        .collect();
    build(&entries)
}

/// Text a cell might hold: things every door has an opinion about.
const TEXTS: &[&str] = &[
    "alice",
    "42",
    "-7",
    "1,234.5",
    "3.5e2",
    "true",
    "no",
    "2024-01-31",
    "31/01/2024",
    "2024-01-31T10:30:00Z",
    "2024-01-31 10:30:00",
    "10:30:15.5",
    "P1DT2H",
    "550e8400-e29b-41d4-a716-446655440000",
    "45000.5",
    "1700000000",
    " padded ",
    "a &amp; b &lt;c&gt; &#65;&#x42; &#x1F600; &bogus; &",
    "",
    "#N/A",
    "ünïcödé — ✓",
];

/// The shared strings every generated XLSX carries: plain, escaped, rich (with a phonetic
/// run that is not part of the text), empty in both spellings, and CDATA.
pub const SHARED: &str = "<si><t>alice</t></si>\
<si><t xml:space=\"preserve\"> a &amp; b </t></si>\
<si><r><rPr><b/></rPr><t>rich </t></r><r><t>text</t></r><rPh sb=\"0\" eb=\"1\"><t>NOT</t></rPh><phoneticPr fontId=\"1\"/></si>\
<si/>\
<si><t></t></si>\
<si><t><![CDATA[<raw> & such]]></t></si>\
<si><t>12.5</t></si>";
const SHARED_COUNT: u64 = 7;

/// One generated XLSX cell for column `col` (zero-based) of row `row`.
fn xlsx_cell(rng: &mut Rng, row: u64, col: u64) -> String {
    let reference = if rng.chance(4) {
        String::new()
    } else {
        let letter = (b'A' + col as u8) as char;
        format!(" r=\"{letter}{row}\"")
    };
    let number = |rng: &mut Rng| -> String {
        match rng.below(10) {
            0 => "0".into(),
            1 => "1".into(),
            2 => format!("{}", rng.below(100_000)),
            3 => format!("-{}", rng.below(1_000)),
            4 => format!("{}.{}", rng.below(60_000), rng.below(1_000_000)),
            5 => format!("0.{}", rng.below(100_000_000)),
            6 => format!("{}E+{}", rng.below(99), rng.below(20)),
            7 => format!("{}.{}e-{}", rng.below(9), rng.below(999), rng.below(12)),
            8 => format!("{}", 1_600_000_000 + rng.below(200_000_000)),
            _ => format!("{}.5", rng.below(3)),
        }
    };
    match rng.below(16) {
        0 => format!("<c{reference}/>"),
        1 => format!("<c{reference} s=\"1\"></c>"),
        2 => format!("<c{reference}><v>{}</v></c>", number(rng)),
        3 => format!("<c{reference} t=\"n\"><v> {} </v></c>", number(rng)),
        4 => format!(
            "<c{reference} s=\"{}\"><f>A1+1</f><v>{}</v></c>",
            rng.below(11),
            number(rng)
        ),
        5 => format!(
            "<c{reference} t=\"s\"><v>{}</v></c>",
            rng.below(SHARED_COUNT)
        ),
        6 => format!(
            "<c{reference} t=\"inlineStr\"><is><t>{}</t></is></c>",
            rng.pick(TEXTS)
        ),
        7 => format!(
            "<c{reference} t=\"inlineStr\"><is><r><t>{}</t></r><r><rPr/><t>!</t></r><rPh><t>x</t></rPh></is></c>",
            rng.pick(TEXTS)
        ),
        8 => format!(
            "<c{reference} t=\"str\"><f>CONCAT()</f><v>{}</v></c>",
            rng.pick(TEXTS)
        ),
        9 => format!(
            "<c{reference} t=\"b\"><v>{}</v></c>",
            rng.pick(&["1", "0", "TRUE", ""])
        ),
        10 => format!(
            "<c{reference} t=\"e\"><v>{}</v></c>",
            rng.pick(&[
                "#N/A", "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#NULL!", "#WAT"
            ])
        ),
        11 => format!(
            "<c{reference} t=\"d\"><v>{}</v></c>",
            rng.pick(&[
                "2024-01-31",
                "2024-01-31T10:00:00",
                "2024-01-31T10:00",
                "1976-11-22T08:30Z",
                "2024-01-01T00:30:00+01:00",
                "2024-01-31T10:00:00.25",
                "yesterday",
            ])
        ),
        12 => format!(
            "<c{reference} s=\"{}\"><v>{}</v></c>",
            1 + rng.below(8),
            rng.pick(&[
                "45000.5", "0.75", "1.5", "60", "-1", "2958466", "0", "36526", "-0.25", "1e300"
            ])
        ),
        13 => format!("<c{reference}><v><![CDATA[{}]]></v></c>", number(rng)),
        14 => format!(
            "<c{reference} t=\"weird\"><!-- a comment --><v>{}</v><extLst><ext><v>9</v></ext></extLst></c>",
            rng.pick(TEXTS)
        ),
        _ => format!("<x:c{reference} t=\"n\"><x:v>{}</x:v></x:c>", number(rng)),
    }
}

/// A generated `<sheetData>` body: rows with gaps in their numbers, rows without numbers,
/// empty rows in both spellings, and cells of every kind.
pub fn xlsx_sheet(seed: u64, rows: u64, width: u64) -> String {
    let mut rng = Rng(seed);
    let mut out = String::new();
    let mut number = 0u64;
    for _ in 0..rows {
        number += if rng.chance(6) { 1 + rng.below(4) } else { 1 };
        let reference = if rng.chance(5) {
            String::new()
        } else {
            format!(" r=\"{number}\"")
        };
        if rng.chance(9) {
            out.push_str(&format!("<row{reference}/>"));
            continue;
        }
        out.push_str(&format!("<row{reference} spans=\"1:9\">"));
        if rng.chance(12) {
            out.push_str("<!-- nothing here -->");
        } else {
            for col in 0..width {
                if rng.chance(4) {
                    continue;
                }
                out.push_str(&xlsx_cell(&mut rng, number, col));
            }
            if rng.chance(10) {
                // A cell well past any plan, and one that takes its place back.
                out.push_str(&format!("<c r=\"ZZ{number}\"><v>1</v></c>"));
            }
        }
        if rng.chance(15) {
            out.push_str("<extLst><ext uri=\"x\"><c><v>777</v></c></ext></extLst>");
        }
        out.push_str("</row>");
    }
    out
}

/// A generated XLSX package of three sheets.
pub fn xlsx_generated(seed: u64, rows: u64, width: u64, deflate: bool) -> Vec<u8> {
    let first = xlsx_sheet(seed, rows, width);
    let second = xlsx_sheet(seed ^ 0xABCD, rows / 2 + 1, width);
    let third = xlsx_sheet(seed ^ 0x1234_5678, 3, width);
    xlsx(
        &[
            ("Data", &first),
            ("More &amp; more", &second),
            ("Hidden", &third),
        ],
        SHARED,
        seed % 2 == 1,
        deflate,
    )
}

const OFFICE: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
xmlns:calcext=\"urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0\"";

/// An ODS package: the tables' XML (each a whole `<table:table>`), and whether to deflate.
pub fn ods(tables: &str, deflate: bool) -> Vec<u8> {
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><office:document-content {OFFICE}>\
<office:body><office:spreadsheet>{tables}</office:spreadsheet></office:body>\
</office:document-content>"
    );
    build(&[
        (
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet",
            false,
        ),
        (
            "META-INF/manifest.xml",
            b"<manifest:manifest><manifest:file-entry manifest:full-path=\"/\"/></manifest:manifest>",
            deflate,
        ),
        ("content.xml", content.as_bytes(), deflate),
    ])
}

/// One generated ODS cell.
fn ods_cell(rng: &mut Rng) -> String {
    let repeat = match rng.below(12) {
        0 => " table:number-columns-repeated=\"2\"".to_string(),
        1 => " table:number-columns-repeated=\"3\"".to_string(),
        _ => String::new(),
    };
    let text = |rng: &mut Rng| -> String {
        match rng.below(6) {
            0 => format!("<text:p>{}</text:p>", rng.pick(TEXTS)),
            1 => "<text:p>one<text:s/>two<text:s text:c=\"3\"/>three<text:tab/>four<text:line-break/>five</text:p>"
                .to_string(),
            2 => format!(
                "<text:p>{}</text:p><text:p><text:span text:style-name=\"T1\">second</text:span> para</text:p>",
                rng.pick(TEXTS)
            ),
            3 => "<office:annotation><text:p>a note</text:p></office:annotation><text:p>noted</text:p>"
                .to_string(),
            4 => "<text:p/>".to_string(),
            _ => "<text:p><![CDATA[c<d>ata]]></text:p>".to_string(),
        }
    };
    match rng.below(15) {
        0 => format!("<table:table-cell{repeat}/>"),
        1 => format!("<table:covered-table-cell{repeat}/>"),
        2 => format!(
            "<table:table-cell{repeat} office:value-type=\"float\" office:value=\"{}\"><text:p>shown</text:p></table:table-cell>",
            rng.pick(&[
                "42",
                "-7.25",
                "1e3",
                "0",
                "1",
                "45000.5",
                "0.1",
                "1700000000",
                "oops"
            ])
        ),
        3 => format!(
            "<table:table-cell{repeat} office:value-type=\"percentage\" office:value=\"0.{}\"/>",
            rng.below(1000)
        ),
        4 => format!(
            "<table:table-cell{repeat} office:value-type=\"currency\" office:currency=\"USD\" office:value=\"{}.5\"><text:p>$</text:p></table:table-cell>",
            rng.below(1000)
        ),
        5 => format!(
            "<table:table-cell{repeat} office:value-type=\"boolean\" office:boolean-value=\"{}\"/>",
            rng.pick(&["true", "false", " true ", "maybe"])
        ),
        6 => format!(
            "<table:table-cell{repeat} office:value-type=\"date\" office:date-value=\"{}\"><text:p>d</text:p></table:table-cell>",
            rng.pick(&[
                "2024-01-31",
                "2024-01-31T10:00:00",
                "1899-12-30",
                "2024-01-31T23:59:59.999999999",
                "soon",
            ])
        ),
        7 => format!(
            "<table:table-cell{repeat} office:value-type=\"time\" office:time-value=\"{}\"/>",
            rng.pick(&["PT10H30M15S", "PT0S", "P1DT2H", "-PT1.5S", "PT25H", "later"])
        ),
        8 => format!(
            "<table:table-cell{repeat} office:value-type=\"string\" office:string-value=\"{}\"><text:p>ignored</text:p></table:table-cell>",
            rng.pick(TEXTS)
        ),
        9 => format!(
            "<table:table-cell{repeat} office:value-type=\"string\" calcext:value-type=\"error\"><text:p>{}</text:p></table:table-cell>",
            rng.pick(&["#N/A", "#DIV/0!", " #NAME? ", "Err:502"])
        ),
        10 => format!(
            "<table:table-cell{repeat} office:value-type=\"error\"><text:p>#REF!</text:p></table:table-cell>"
        ),
        11 => format!(
            "<table:table-cell{repeat} office:value-type=\"float\"><text:p>no value</text:p></table:table-cell>"
        ),
        12 => format!(
            "<table:table-cell{repeat}>{}<table:table table:name=\"nested\"><table:table-row><table:table-cell><text:p>inner</text:p></table:table-cell></table:table-row></table:table></table:table-cell>",
            text(rng)
        ),
        13 => format!(
            "<table:table-cell{repeat} office:value-type=\"void\">{}</table:table-cell>",
            text(rng)
        ),
        _ => format!(
            "<table:table-cell{repeat} office:value-type=\"string\">{}</table:table-cell>",
            text(rng)
        ),
    }
}

/// A generated `<table:table>`: repeated rows (blank and not), row groups, and cells of
/// every kind, repeated and not.
pub fn ods_table(seed: u64, name: &str, rows: u64, width: u64, trailing: u64) -> String {
    let mut rng = Rng(seed);
    let mut out = format!(
        "<table:table table:name=\"{name}\"><table:table-column table:number-columns-repeated=\"{width}\"/>"
    );
    let mut grouped = false;
    for _ in 0..rows {
        if !grouped && rng.chance(20) {
            out.push_str("<table:table-header-rows>");
            grouped = true;
        }
        let repeat = match rng.below(10) {
            0 => " table:number-rows-repeated=\"3\"",
            1 => " table:number-rows-repeated=\"2\"",
            _ => "",
        };
        if rng.chance(10) {
            out.push_str(&format!("<table:table-row{repeat}/>"));
        } else if rng.chance(10) {
            out.push_str(&format!(
                "<table:table-row{repeat}><table:table-cell table:number-columns-repeated=\"{width}\"/></table:table-row>"
            ));
        } else {
            out.push_str(&format!("<table:table-row{repeat}>"));
            for _ in 0..width {
                if rng.chance(5) {
                    continue;
                }
                out.push_str(&ods_cell(&mut rng));
            }
            out.push_str("</table:table-row>");
        }
        if grouped && rng.chance(3) {
            out.push_str("</table:table-header-rows><text:soft-page-break/>");
            grouped = false;
        }
    }
    if grouped {
        out.push_str("</table:table-header-rows>");
    }
    // What a real document ends a sheet with: empty rows by the thousand, in one element.
    out.push_str(&format!(
        "<table:table-row table:number-rows-repeated=\"{trailing}\"><table:table-cell table:number-columns-repeated=\"1024\"/></table:table-row>"
    ));
    out.push_str("</table:table>");
    out
}

/// A generated ODS package of three tables.
pub fn ods_generated(seed: u64, rows: u64, width: u64, deflate: bool) -> Vec<u8> {
    let tables = format!(
        "{}{}{}",
        ods_table(seed, "First", rows, width, 4),
        ods_table(seed ^ 0xBEEF, "Second &amp; last", rows / 2 + 1, width, 3),
        ods_table(seed ^ 0xF00D, "", 2, width, 1)
    );
    ods(&tables, deflate)
}

/// The packages `corpus/workbook/` holds beside the fixtures real applications' libraries
/// wrote: two of each format — one deflated, one stored, the XLSX pair one in each date
/// system — and one whose sheet names a shared string that is not there.
pub fn corpus_packages() -> Vec<(&'static str, Vec<u8>)> {
    let broken = "<row><c t=\"s\"><v>0</v></c><c t=\"inlineStr\"><is><t>n</t></is></c></row>\
<row><c t=\"s\"><v>1</v></c><c><v>1</v></c></row>\
<row><c t=\"s\"><v>99</v></c><c><v>2</v></c></row>\
<row><c t=\"s\"><v>2</v></c><c><v>3</v></c></row>";
    vec![
        ("generated-a.xlsx", xlsx_generated(2, 12, 4, true)),
        ("generated-b.xlsx", xlsx_generated(3, 12, 4, false)),
        ("generated-a.ods", ods_generated(2, 12, 4, true)),
        ("generated-b.ods", ods_generated(3, 12, 4, false)),
        (
            "broken.xlsx",
            xlsx(&[("Data", broken)], SHARED, false, true),
        ),
    ]
}
