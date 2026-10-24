//! Workbook packages built in memory for the core's tests: XLSX, ODS and Numbers, by hand and by a
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
        !seed.is_multiple_of(2),
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
/// system, the Numbers pair in the layouts Numbers itself never writes small (chunks of a
/// few bytes, tiles of seven rows, wide offsets, a segmented string list, sparse rows) —
/// and one whose sheet names a shared string that is not there.
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
        ("generated-a.numbers", numbers_generated(5, 12, 4, true)),
        ("generated-b.numbers", numbers_generated(3, 12, 4, false)),
        (
            "broken.xlsx",
            xlsx(&[("Data", broken)], SHARED, false, true),
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Apple Numbers: a document model written out as the IWA parts of a package.

/// A Protocol Buffers message under construction, field by field.
#[derive(Clone, Debug, Default)]
pub struct Proto(pub Vec<u8>);

pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

impl Proto {
    pub fn new() -> Proto {
        Proto(Vec::new())
    }

    fn key(&mut self, number: u32, wire: u8) {
        put_varint(&mut self.0, u64::from(number) << 3 | u64::from(wire));
    }

    pub fn varint(mut self, number: u32, value: u64) -> Proto {
        self.key(number, 0);
        put_varint(&mut self.0, value);
        self
    }

    pub fn double(mut self, number: u32, value: f64) -> Proto {
        self.key(number, 1);
        self.0.extend_from_slice(&value.to_bits().to_le_bytes());
        self
    }

    pub fn bytes(mut self, number: u32, value: &[u8]) -> Proto {
        self.key(number, 2);
        put_varint(&mut self.0, value.len() as u64);
        self.0.extend_from_slice(value);
        self
    }

    pub fn message(self, number: u32, value: &Proto) -> Proto {
        self.bytes(number, &value.0)
    }

    /// A `TSP.Reference` to object `identifier`.
    pub fn reference(self, number: u32, identifier: u64) -> Proto {
        self.message(number, &Proto::new().varint(1, identifier))
    }
}

/// An IWA stream under construction: objects, each a `TSP.ArchiveInfo` header and its one
/// message. An object of type 0 has no message.
#[derive(Clone, Debug, Default)]
pub struct Stream(pub Vec<u8>);

impl Stream {
    pub fn object(&mut self, identifier: u64, kind: u32, payload: &[u8]) {
        let mut info = Proto::new().varint(1, identifier);
        if kind != 0 {
            let message = Proto::new()
                .varint(1, u64::from(kind))
                .varint(3, payload.len() as u64);
            info = info.message(2, &message);
        }
        put_varint(&mut self.0, info.0.len() as u64);
        self.0.extend_from_slice(&info.0);
        if kind != 0 {
            self.0.extend_from_slice(payload);
        }
    }
}

fn snappy_literal(out: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let n = bytes.len() - 1;
    if n < 60 {
        out.push((n as u8) << 2);
    } else {
        let extra = match n {
            n if n < 1 << 8 => 1,
            n if n < 1 << 16 => 2,
            n if n < 1 << 24 => 3,
            _ => 4,
        };
        out.push(((59 + extra) as u8) << 2);
        out.extend_from_slice(&(n as u32).to_le_bytes()[..extra]);
    }
    out.extend_from_slice(bytes);
}

/// `bytes` as a Snappy block: literals, and a copy wherever four or more bytes repeat
/// within the last 255 — in every one of the three copy encodings, in turn, so that each
/// is decoded.
pub fn snappy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, bytes.len() as u64);
    let mut literal_start = 0usize;
    let mut pos = 0usize;
    let mut copies = 0usize;
    while pos < bytes.len() {
        let mut best = (0usize, 0usize);
        if pos + 4 <= bytes.len() {
            for start in pos.saturating_sub(255)..pos {
                let mut len = 0;
                while len < 64 && pos + len < bytes.len() && bytes[start + len] == bytes[pos + len]
                {
                    len += 1;
                }
                if len >= 4 && len > best.0 {
                    best = (len, pos - start);
                }
            }
        }
        if best.0 < 4 {
            pos += 1;
            continue;
        }
        snappy_literal(&mut out, &bytes[literal_start..pos]);
        let (len, offset) = best;
        match copies % 3 {
            0 if len <= 11 => {
                out.push(1 | ((len - 4) as u8) << 2 | ((offset >> 8) as u8) << 5);
                out.push(offset as u8);
            }
            1 => {
                out.push(2 | ((len - 1) as u8) << 2);
                out.extend_from_slice(&(offset as u16).to_le_bytes());
            }
            _ => {
                out.push(3 | ((len - 1) as u8) << 2);
                out.extend_from_slice(&(offset as u32).to_le_bytes());
            }
        }
        copies += 1;
        pos += len;
        literal_start = pos;
    }
    snappy_literal(&mut out, &bytes[literal_start..]);
    out
}

/// An IWA part: `stream` in chunks of at most `chunk` decoded bytes, each a zero byte, a
/// three-byte length and a Snappy block.
pub fn iwa_part(stream: &[u8], chunk: usize) -> Vec<u8> {
    let chunk = chunk.clamp(1, 65536);
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let end = (at + chunk).min(stream.len());
        let block = snappy(&stream[at..end]);
        out.push(0);
        out.extend_from_slice(&(block.len() as u32).to_le_bytes()[..3]);
        out.extend_from_slice(&block);
        at = end;
        if at >= stream.len() {
            return out;
        }
    }
}

/// A cell of a Numbers table, as the document model has it.
#[derive(Clone, Debug, PartialEq)]
pub enum NCell {
    Empty,
    /// A number, stored as the 128-bit decimal Numbers writes.
    Number(f64),
    /// A number stored as a double, as a currency cell is.
    Currency(f64),
    Text(String),
    /// Text with styling: two runs of a rich-text storage.
    Rich(String),
    /// Seconds since 2001-01-01T00:00:00Z, and whether its format shows only a time.
    Date(f64, bool),
    Bool(bool),
    /// Seconds.
    Duration(f64),
    /// A formula that failed.
    Error,
    /// A cell a merged cell spans.
    Merged,
}

#[derive(Clone, Debug)]
pub struct NTable {
    pub name: String,
    pub header_rows: u32,
    pub rows: Vec<Vec<NCell>>,
}

#[derive(Clone, Debug)]
pub struct NSheet {
    pub name: String,
    pub hidden: bool,
    pub tables: Vec<NTable>,
}

/// How a Numbers package is laid out: the knobs the reader has to be indifferent to, and
/// the ones that break it on purpose.
#[derive(Clone, Debug)]
pub struct NumbersShape {
    /// Deflate the `.iwa` entries (Numbers stores them).
    pub deflate: bool,
    /// Decoded bytes per chunk: small, and every object straddles chunks.
    pub chunk: usize,
    /// Rows per tile.
    pub tile_size: u32,
    /// Cell offsets in units of four bytes.
    pub wide: bool,
    /// The string list split into the list and one segment.
    pub segments: bool,
    /// Rows with nothing in them left out of their tile.
    pub sparse: bool,
    /// What every cell's first byte says; the core reads 5.
    pub storage_version: u8,
    /// The type of object 1; a document is 1.
    pub document_kind: u32,
    /// Leave every tile part out of the package.
    pub drop_tiles: bool,
    /// Leave every string list's entries out.
    pub lose_strings: bool,
    /// In the first table, the payload of its first tile object, verbatim.
    pub raw_tile: Option<Vec<u8>>,
    /// In the first table, the storage buffer and offsets of its first row, verbatim.
    pub raw_row: Option<(Vec<u8>, Vec<u8>)>,
}

impl Default for NumbersShape {
    fn default() -> NumbersShape {
        NumbersShape {
            deflate: false,
            chunk: 65536,
            tile_size: 256,
            wide: false,
            segments: false,
            sparse: false,
            storage_version: 5,
            document_kind: 1,
            drop_tiles: false,
            lose_strings: false,
            raw_tile: None,
            raw_row: None,
        }
    }
}

/// `value` as the 128-bit decimal Numbers stores a number in.
pub fn decimal128(value: f64) -> [u8; 16] {
    let text = format!("{value:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap();
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len()) as i32;
    let exponent = exponent.parse::<i32>().unwrap() - fraction;
    let mantissa: u128 = digits.parse().unwrap();
    let biased = (exponent + 0x1820) as u32;
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().take(14).enumerate() {
        *byte = (mantissa >> (8 * index)) as u8;
    }
    bytes[14] = ((mantissa >> 112) as u8 & 1) | ((biased & 0x7F) as u8) << 1;
    bytes[15] = ((biased >> 7) & 0x7F) as u8 | if negative { 0x80 } else { 0 };
    bytes
}

/// A v5 cell storage: the version, the type, a flag word, then the fields in flag order.
/// `ids` are (flag index from 0x8, identifier) pairs: 0 the string, 1 the rich text, 12 the
/// date format.
pub fn cell_storage(
    version: u8,
    kind: u8,
    decimal: Option<[u8; 16]>,
    double: Option<f64>,
    seconds: Option<f64>,
    ids: &[(usize, u32)],
) -> Vec<u8> {
    let mut flags = 0u32;
    let mut payload = Vec::new();
    if let Some(decimal) = decimal {
        flags |= 1;
        payload.extend_from_slice(&decimal);
    }
    if let Some(double) = double {
        flags |= 2;
        payload.extend_from_slice(&double.to_le_bytes());
    }
    if let Some(seconds) = seconds {
        flags |= 4;
        payload.extend_from_slice(&seconds.to_le_bytes());
    }
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    for (index, id) in ids {
        flags |= 8 << index;
        payload.extend_from_slice(&id.to_le_bytes());
    }
    let mut out = vec![version, kind, 0, 0, 0, 0, 0, 0];
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend(payload);
    out
}

/// A Numbers package: `sheets` laid out as `shape` says. Object 1 is the document, in
/// `Index/Document.iwa` with the sheets; the tables' infos and models are in
/// `Index/CalculationEngine.iwa`; each tile and each data list has a part of its own under
/// `Index/Tables/`, as Numbers writes them.
pub fn numbers(sheets: &[NSheet], shape: &NumbersShape) -> Vec<u8> {
    const KIND_SHEET: u32 = 2;
    const KIND_TABLE_INFO: u32 = 6000;
    const KIND_TABLE_MODEL: u32 = 6001;
    const KIND_TILE: u32 = 6002;
    const KIND_LIST: u32 = 6005;
    const KIND_SEGMENT: u32 = 6006;
    const KIND_RICH_PAYLOAD: u32 = 6218;
    const KIND_STORAGE: u32 = 2001;
    const KIND_SHAPE: u32 = 3047;

    let mut next = 2u64;
    let mut alloc = || {
        next += 1;
        next - 1
    };
    let mut document = Proto::new();
    let mut sheet_stream = Stream::default();
    let mut engine = Stream::default();
    let mut table_parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut first_table = true;
    for sheet in sheets {
        let sheet_id = alloc();
        document = document.reference(1, sheet_id);
        let mut archive = Proto::new().bytes(1, sheet.name.as_bytes());
        // A drawable that is not a table, and one the package lost.
        let shape_id = alloc();
        sheet_stream.object(shape_id, KIND_SHAPE, &Proto::new().varint(1, 7).0);
        archive = archive.reference(2, shape_id).reference(2, 0xFFFF_FF00);
        for table in &sheet.tables {
            let info_id = alloc();
            let model_id = alloc();
            let strings_id = alloc();
            let rich_id = alloc();
            let formats_id = alloc();
            archive = archive.reference(2, info_id);
            engine.object(
                info_id,
                KIND_TABLE_INFO,
                &Proto::new().reference(2, model_id).0,
            );

            let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
            let tile_size = shape.tile_size.max(1) as usize;
            let mut strings: Vec<String> = Vec::new();
            let mut rich: Vec<(String, u64, u64)> = Vec::new();
            let mut tiles: Vec<Vec<Proto>> = Vec::new();
            for (r, row) in table.rows.iter().enumerate() {
                if shape.sparse && row.iter().all(|cell| *cell == NCell::Empty) {
                    continue;
                }
                let (tile, index) = (r / tile_size, r % tile_size);
                while tiles.len() <= tile {
                    tiles.push(Vec::new());
                }
                let mut storage = Vec::new();
                let mut offsets = Vec::new();
                let mut count = 0u64;
                for c in 0..cols {
                    let cell = row.get(c).unwrap_or(&NCell::Empty);
                    let version = shape.storage_version;
                    let bytes = match cell {
                        NCell::Empty => None,
                        NCell::Number(value) => Some(cell_storage(
                            version,
                            2,
                            Some(decimal128(*value)),
                            None,
                            None,
                            &[(10, 1)],
                        )),
                        NCell::Currency(value) => Some(cell_storage(
                            version,
                            10,
                            None,
                            Some(*value),
                            None,
                            &[(11, 1)],
                        )),
                        NCell::Text(text) => {
                            let key = match strings.iter().position(|s| s == text) {
                                Some(at) => at + 1,
                                None => {
                                    strings.push(text.clone());
                                    strings.len()
                                }
                            };
                            Some(cell_storage(
                                version,
                                3,
                                None,
                                None,
                                None,
                                &[(0, key as u32), (2, 1)],
                            ))
                        }
                        NCell::Rich(text) => {
                            let key = match rich.iter().position(|(s, _, _)| s == text) {
                                Some(at) => at + 1,
                                None => {
                                    rich.push((text.clone(), alloc(), alloc()));
                                    rich.len()
                                }
                            };
                            Some(cell_storage(
                                version,
                                9,
                                None,
                                None,
                                None,
                                &[(1, key as u32)],
                            ))
                        }
                        NCell::Date(seconds, time) => Some(cell_storage(
                            version,
                            5,
                            None,
                            None,
                            Some(*seconds),
                            &[(12, if *time { 2 } else { 1 })],
                        )),
                        NCell::Bool(value) => Some(cell_storage(
                            version,
                            6,
                            None,
                            Some(f64::from(u8::from(*value))),
                            None,
                            &[],
                        )),
                        NCell::Duration(seconds) => Some(cell_storage(
                            version,
                            7,
                            None,
                            Some(*seconds),
                            None,
                            &[(13, 1)],
                        )),
                        NCell::Error => Some(cell_storage(
                            version,
                            8,
                            None,
                            None,
                            None,
                            &[(6, 1), (8, 1)],
                        )),
                        NCell::Merged => Some(cell_storage(version, 1, None, None, None, &[])),
                    };
                    match bytes {
                        None => offsets.extend_from_slice(&(-1i16).to_le_bytes()),
                        Some(bytes) => {
                            let at = storage.len();
                            let unit = if shape.wide { at / 4 } else { at };
                            assert!(unit <= i16::MAX as usize, "a row too wide for its offsets");
                            offsets.extend_from_slice(&(unit as i16).to_le_bytes());
                            storage.extend(bytes);
                            if shape.wide {
                                while storage.len() % 4 != 0 {
                                    storage.push(0);
                                }
                            }
                            count += 1;
                        }
                    }
                }
                if first_table
                    && r == 0
                    && let Some((raw_storage, raw_offsets)) = &shape.raw_row
                {
                    storage = raw_storage.clone();
                    offsets = raw_offsets.clone();
                }
                let mut info = Proto::new()
                    .varint(1, index as u64)
                    .varint(2, count)
                    .varint(5, 5)
                    .bytes(6, &storage)
                    .bytes(7, &offsets);
                if shape.wide {
                    info = info.varint(8, 1);
                }
                tiles[tile].push(info);
            }

            // The tiles, each in a part of its own.
            let mut tile_storage = Proto::new().varint(2, tile_size as u64);
            for (n, rows) in tiles.iter().enumerate() {
                let tile_id = alloc();
                tile_storage = tile_storage
                    .message(1, &Proto::new().varint(1, n as u64).reference(2, tile_id));
                let mut payload = Proto::new()
                    .varint(1, cols as u64 - 1)
                    .varint(2, (rows.len() as u64).max(1) - 1);
                for row in rows {
                    payload = payload.message(5, row);
                }
                let mut payload = payload.0;
                if first_table
                    && n == 0
                    && let Some(raw) = &shape.raw_tile
                {
                    payload = raw.clone();
                }
                let mut stream = Stream::default();
                stream.object(tile_id, KIND_TILE, &payload);
                if !shape.drop_tiles {
                    table_parts.push((format!("Index/Tables/Tile-{tile_id}.iwa"), stream.0));
                }
            }

            // The strings: a list, or a list and a segment.
            let mut entries: Vec<Proto> = strings
                .iter()
                .enumerate()
                .map(|(n, text)| {
                    Proto::new()
                        .varint(1, n as u64 + 1)
                        .bytes(3, text.as_bytes())
                })
                .collect();
            if shape.lose_strings {
                entries.clear();
            }
            let mut list = Proto::new().varint(1, 1).varint(2, entries.len() as u64);
            let mut stream = Stream::default();
            if shape.segments {
                let segment_id = alloc();
                let tail = entries.split_off(entries.len() / 2);
                let mut segment = Proto::new();
                for entry in &tail {
                    segment = segment.message(3, entry);
                }
                list = list.reference(4, segment_id);
                for entry in &entries {
                    list = list.message(3, entry);
                }
                stream.object(strings_id, KIND_LIST, &list.0);
                stream.object(segment_id, KIND_SEGMENT, &segment.0);
            } else {
                for entry in &entries {
                    list = list.message(3, entry);
                }
                stream.object(strings_id, KIND_LIST, &list.0);
            }
            table_parts.push((format!("Index/Tables/DataList-{strings_id}.iwa"), stream.0));

            // The rich text: each entry names a payload, which names a storage of two runs.
            let mut list = Proto::new().varint(1, 1);
            let mut stream = Stream::default();
            for (n, (text, payload_id, storage_id)) in rich.iter().enumerate() {
                list = list.message(
                    3,
                    &Proto::new()
                        .varint(1, n as u64 + 1)
                        .reference(9, *payload_id),
                );
                stream.object(
                    *payload_id,
                    KIND_RICH_PAYLOAD,
                    &Proto::new().reference(1, *storage_id).0,
                );
                let cut = text
                    .char_indices()
                    .map(|(at, _)| at)
                    .find(|&at| at >= text.len() / 2)
                    .unwrap_or(text.len());
                let storage = Proto::new()
                    .bytes(3, &text.as_bytes()[..cut])
                    .bytes(3, &text.as_bytes()[cut..]);
                stream.object(*storage_id, KIND_STORAGE, &storage.0);
            }
            stream.object(rich_id, KIND_LIST, &list.0);
            table_parts.push((format!("Index/Tables/DataList-{rich_id}.iwa"), stream.0));

            // The formats: a date that shows a date, and one that shows only a time.
            let date = Proto::new()
                .varint(1, 261)
                .bytes(14, b"yyyy-MM-dd 'at' HH:mm:ss");
            let time = Proto::new().varint(1, 272).bytes(14, b"H:mm:ss");
            let list = Proto::new()
                .varint(1, 1)
                .message(3, &Proto::new().varint(1, 1).message(6, &date))
                .message(3, &Proto::new().varint(1, 2).message(6, &time));
            let mut stream = Stream::default();
            stream.object(formats_id, KIND_LIST, &list.0);
            table_parts.push((format!("Index/Tables/DataList-{formats_id}.iwa"), stream.0));

            let store = Proto::new()
                .message(3, &tile_storage)
                .reference(4, strings_id)
                .reference(17, rich_id)
                .reference(22, formats_id);
            let model = Proto::new()
                .message(4, &store)
                .varint(6, table.rows.len() as u64)
                .varint(7, cols as u64)
                .bytes(8, table.name.as_bytes())
                .varint(9, u64::from(table.header_rows));
            engine.object(model_id, KIND_TABLE_MODEL, &model.0);
            first_table = false;
        }
        if sheet.hidden {
            archive = archive.varint(25, 1);
        }
        sheet_stream.object(sheet_id, KIND_SHEET, &archive.0);
    }
    let mut document_stream = Stream::default();
    document_stream.object(1, shape.document_kind, &document.0);
    document_stream.0.extend_from_slice(&sheet_stream.0);
    let mut metadata = Stream::default();
    metadata.object(alloc(), 11006, &Proto::new().varint(1, 1).0);
    let mut view = Stream::default();
    view.object(alloc(), 0, &[]);

    let mut parts: Vec<(String, Vec<u8>)> = vec![
        ("Index/Document.iwa".to_string(), document_stream.0),
        ("Index/CalculationEngine.iwa".to_string(), engine.0),
        ("Index/Metadata.iwa".to_string(), metadata.0),
        ("Index/ViewState.iwa".to_string(), view.0),
    ];
    parts.extend(table_parts);
    let mut entries: Vec<(String, Vec<u8>, bool)> = vec![
        (
            "Metadata/DocumentIdentifier".to_string(),
            b"synthetic".to_vec(),
            false,
        ),
        (
            "Metadata/Properties.plist".to_string(),
            b"bplist00".to_vec(),
            false,
        ),
    ];
    for (name, stream) in parts {
        entries.push((name, iwa_part(&stream, shape.chunk), shape.deflate));
    }
    let borrowed: Vec<(&str, &[u8], bool)> = entries
        .iter()
        .map(|(name, bytes, deflate)| (name.as_str(), bytes.as_slice(), *deflate))
        .collect();
    build(&borrowed)
}

fn numbers_cell(rng: &mut Rng) -> NCell {
    const WORDS: [&str; 8] = [
        "alice",
        "bob, jr",
        " padded ",
        "日本語 é 🙂",
        "x <&> \"y\"",
        "",
        "TRUE",
        "1e3",
    ];
    match rng.below(14) {
        0 | 1 => NCell::Empty,
        2 => NCell::Number(rng.below(2_000_000) as f64 - 1_000_000.0),
        3 => NCell::Number((rng.below(1_000_000) as f64 - 500_000.0) / 1000.0),
        4 => {
            NCell::Number([1e15, 0.1, -7.0, 2.5, 1e-7, 123_456_789_012.345][rng.below(6) as usize])
        }
        5 => NCell::Currency((rng.below(100_000) as f64) / 100.0),
        6 => NCell::Text(rng.pick(&WORDS).to_string()),
        7 => NCell::Text(format!("w{}", rng.below(50))),
        8 => NCell::Rich(format!("{} tail", rng.pick(&WORDS))),
        9 => NCell::Date(
            rng.below(1_500_000_000) as f64 - 500_000_000.0 + [0.0, 0.5][rng.below(2) as usize],
            false,
        ),
        10 => NCell::Date(rng.below(86_400) as f64, true),
        11 => NCell::Bool(rng.chance(2)),
        12 => NCell::Duration((rng.below(2_000_000) as f64 - 1_000_000.0) / 8.0),
        _ => {
            if rng.chance(3) {
                NCell::Merged
            } else {
                NCell::Error
            }
        }
    }
}

/// A Numbers package of three tables on two sheets, `rows` by `width` of every kind of
/// cell, in the layout the seed picks: chunks of a few bytes or the whole stream, tiles of
/// seven rows or of 256, narrow or wide offsets, a segmented string list, sparse rows.
pub fn numbers_generated(seed: u64, rows: u64, width: u64, deflate: bool) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut table = |name: &str, rows: u64, header_rows: u32| NTable {
        name: name.to_string(),
        header_rows,
        rows: (0..rows)
            .map(|r| {
                (0..width)
                    .map(|c| {
                        if r == 0 {
                            NCell::Text(format!("col{c}"))
                        } else {
                            numbers_cell(&mut rng)
                        }
                    })
                    .collect()
            })
            .collect(),
    };
    let sheets = [
        NSheet {
            name: "Data".to_string(),
            hidden: false,
            tables: vec![table("Table 1", rows, 1)],
        },
        NSheet {
            name: "Ünïcödé sheet".to_string(),
            hidden: !seed.is_multiple_of(2),
            tables: vec![
                table("First", rows / 3 + 1, 0),
                table("Second", rows / 2 + 1, 1),
            ],
        },
    ];
    let shape = NumbersShape {
        deflate,
        chunk: if seed.is_multiple_of(2) {
            65536
        } else {
            37 + (seed % 200) as usize
        },
        tile_size: if seed.is_multiple_of(3) { 7 } else { 256 },
        wide: seed % 4 == 1,
        segments: !seed.is_multiple_of(2),
        sparse: seed.is_multiple_of(5),
        ..NumbersShape::default()
    };
    numbers(&sheets, &shape)
}
