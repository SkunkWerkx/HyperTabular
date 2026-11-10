//! The workbook core against itself and against what cannot be argued with.
//!
//! What a workbook reads *as* is settled by `corpus/workbook.json`, which the std reader
//! this core replaced wrote before it was deleted (`corpus_workbook.rs` replays it). What
//! is here is everything that file cannot say: that a read with buffers that start empty
//! and grow only as the core asks — every place a read can stop and resume passed through
//! — lands exactly where a read with room to spare lands, over generated packages that
//! reach for every shape a cell, a row and a part can take; that the core says and reads
//! a number as Rust does; and that containers that are broken, lying or hostile each come
//! back as a code.

use super::{packages, support};

use hypertabular::kernel::abi::{ColumnSpec, Failure};
use hypertabular::kernel::workbook::iwa::{BROKEN_CHUNK, MISSING_OBJECT, STORAGE_VERSION};
use hypertabular::kernel::workbook::number::{Text, parse_f64};
use hypertabular::kernel::workbook::part::{DEFLATE_RATIO, WINDOW_MIN};
use hypertabular::kernel::workbook::xml::UNTERMINATED_ELEMENT;
use hypertabular::workbook::SheetOptions;
use packages::{
    NCell, NSheet, NTable, NumbersShape, Rng, build, numbers, numbers_generated, ods,
    ods_generated, ods_table, xlsx, xlsx_generated, xlsx_sheet,
};
use std::path::PathBuf;
use support::{Harness, Read, Sizing, compare, compare_plan, every_door};

fn options() -> [SheetOptions; 4] {
    let base = SheetOptions::default();
    [
        base,
        base.with_header(false),
        base.with_empty_rows_skipped(false),
        base.with_header(false).with_empty_rows_skipped(false),
    ]
}

#[test]
fn generated_xlsx_reads_the_same_however_little_room_it_has() {
    let mut refusals = 0;
    let mut rows = 0;
    for seed in 1..=12u64 {
        for deflate in [false, true] {
            let package = xlsx_generated(seed * 7919, 40, 5, deflate);
            for (index, options) in options().into_iter().enumerate() {
                // Batches of one row, of a few, and of more than the sheet has.
                let max_rows = [1, 7, 1000][(seed as usize + index) % 3];
                let stingy = compare(&package, options, 5, max_rows);
                refusals += stingy.0;
                rows += stingy.1;
            }
        }
    }
    assert!(
        refusals > 1_000,
        "the stingy reads were refused {refusals} times"
    );
    assert!(rows > 5_000, "{rows} rows compared");
}

#[test]
fn generated_ods_reads_the_same_however_little_room_it_has() {
    let mut refusals = 0;
    let mut rows = 0;
    for seed in 1..=12u64 {
        for deflate in [false, true] {
            let package = ods_generated(seed * 104_729, 30, 5, deflate);
            for (index, options) in options().into_iter().enumerate() {
                let max_rows = [1, 7, 1000][(seed as usize + index) % 3];
                let stingy = compare(&package, options, 5, max_rows);
                refusals += stingy.0;
                rows += stingy.1;
            }
        }
    }
    assert!(
        refusals > 1_000,
        "the stingy reads were refused {refusals} times"
    );
    assert!(rows > 5_000, "{rows} rows compared");
}

#[test]
fn the_corpus_packages_read_the_same_however_little_room_they_have() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corpus/workbook");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !matches!(extension, "xlsx" | "xlsm" | "ods" | "numbers") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        // A package the core refuses to open has no rows, however much room it is given.
        let refused = name.contains("encrypted");
        let bytes = std::fs::read(&path).unwrap();
        // Across the ways of reading it: a sheet of one row has none once that row is its
        // header.
        let mut total = 0;
        for options in options() {
            // A real ODS sheet ends in a million empty rows written as one element. Read
            // with empty rows delivered, the comparison would hold every one of them twice,
            // in ten columns: gigabytes. `a_million_trailing_rows` covers that reading with
            // one column. Only the generated packages are known not to end that way.
            if extension == "ods" && !name.starts_with("generated") && !options.skip_empty_rows {
                continue;
            }
            let (_, rows) = compare(&bytes, options, 10, 3);
            total += rows;
        }
        assert_eq!(total > 0, !refused, "{} has rows", path.display());
        seen += 1;
    }
    assert!(seen >= 3, "{seen} fixtures read");
}

#[test]
fn generated_numbers_reads_the_same_however_little_room_it_has() {
    let mut refusals = 0;
    let mut rows = 0;
    // The seed picks the layout: every one of them, deflated and stored.
    for seed in 1..=12u64 {
        for deflate in [false, true] {
            let package = numbers_generated(seed, 40, 5, deflate);
            for (index, options) in options().into_iter().enumerate() {
                let max_rows = [1, 7, 1000][(seed as usize + index) % 3];
                let stingy = compare(&package, options, 5, max_rows);
                refusals += stingy.0;
                rows += stingy.1;
            }
        }
    }
    assert!(
        refusals > 1_000,
        "the stingy reads were refused {refusals} times"
    );
    assert!(rows > 5_000, "{rows} rows compared");
}

/// The cells of a Numbers table, as the Rust binding reads them, whatever the package's
/// layout: the same text out of a stored package in one chunk, a deflated one in chunks of
/// a few bytes with wide offsets and a segmented string list, and one of two-row tiles that
/// leaves its empty rows out. A text cell whose string is empty is an empty cell, as an
/// empty inline string is in XLSX.
#[test]
fn numbers_cells_are_read_as_written() {
    use NCell::{Bool, Currency, Date, Duration, Empty, Error, Merged, Number, Rich, Text};
    use hypertabular::{Column, Workbook};
    let text = |s: &str| Text(s.to_string());
    // 2024-01-31T10:30:00Z and 2024-01-31T00:00:00Z, in seconds since 2001-01-01.
    let (morning, midnight) = (728_389_800.0, 728_352_000.0);
    let rows = vec![
        vec![
            text("id"),
            text("num"),
            text("txt"),
            text("when"),
            text("flag"),
            text("span"),
            text("odd"),
        ],
        vec![
            Number(1.0),
            Number(2.5),
            text("alice"),
            Date(morning, false),
            Bool(true),
            Duration(90.5),
            Error,
        ],
        vec![
            Number(2.0),
            Number(-7.0),
            Rich("plain bold tail".to_string()),
            Date(37_800.0, true),
            Bool(false),
            Duration(-3661.0),
            Merged,
        ],
        vec![
            Number(3.0),
            Number(1e15),
            text("日本語 é 🙂"),
            Date(midnight, false),
            Empty,
            Currency(12.34),
            Empty,
        ],
        vec![Empty; 7],
        vec![
            Number(4.0),
            Number(1e-7),
            text(""),
            Date(-1.0, false),
            Bool(true),
            Duration(0.0),
            text("x <&> \"y\""),
        ],
    ];
    let sheets = vec![
        NSheet {
            name: "Data".to_string(),
            hidden: false,
            tables: vec![NTable {
                name: "Table 1".to_string(),
                header_rows: 1,
                rows,
            }],
        },
        NSheet {
            name: "Two".to_string(),
            hidden: true,
            tables: vec![
                NTable {
                    name: "A".to_string(),
                    header_rows: 0,
                    rows: vec![vec![Number(1.0)]],
                },
                NTable {
                    name: "B".to_string(),
                    header_rows: 0,
                    rows: vec![],
                },
            ],
        },
    ];
    let shapes = [
        NumbersShape::default(),
        NumbersShape {
            deflate: true,
            chunk: 23,
            wide: true,
            segments: true,
            ..NumbersShape::default()
        },
        NumbersShape {
            tile_size: 2,
            sparse: true,
            ..NumbersShape::default()
        },
    ];
    for shape in shapes {
        let book = Workbook::from_vec(numbers(&sheets, &shape)).unwrap();
        assert_eq!(book.format(), hypertabular::Format::Numbers);
        let names: Vec<(&str, bool)> = book
            .sheets()
            .iter()
            .map(|sheet| (sheet.name.as_str(), sheet.hidden))
            .collect();
        assert_eq!(
            names,
            [("Data", false), ("Two - A", true), ("Two - B", true)]
        );

        let plan: Vec<Column> = (0..7).map(Column::text).collect();
        let options = SheetOptions::default().with_empty_rows_skipped(false);
        let mut sheet = book.sheet(0, options, &plan).unwrap();
        let header: Vec<String> = sheet
            .header()
            .unwrap()
            .names()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect();
        assert_eq!(header, ["id", "num", "txt", "when", "flag", "span", "odd"]);
        let mut seen: Vec<(u32, Vec<String>)> = Vec::new();
        while let Some(batch) = sheet.read().unwrap() {
            for row in &batch {
                let cells = (0..7)
                    .map(|column| match row.text_str(column) {
                        Ok(text) => text.into_owned(),
                        Err(fault) => format!("<{:?}>", fault.reason),
                    })
                    .collect();
                seen.push((row.line(), cells));
            }
        }
        let expected: Vec<(u32, Vec<&str>)> = vec![
            (
                2,
                vec![
                    "1",
                    "2.5",
                    "alice",
                    "2024-01-31T10:30:00",
                    "true",
                    "PT1M30.5S",
                    "#VALUE!",
                ],
            ),
            (
                3,
                vec![
                    "2",
                    "-7",
                    "plain bold tail",
                    "10:30:00",
                    "false",
                    "-PT1H1M1S",
                    "<Empty>",
                ],
            ),
            (
                4,
                vec![
                    "3",
                    "1000000000000000",
                    "日本語 é 🙂",
                    "2024-01-31T00:00:00",
                    "<Empty>",
                    "12.34",
                    "<Empty>",
                ],
            ),
            (5, vec!["<Empty>"; 7]),
            (
                6,
                vec![
                    "4",
                    "1e-7",
                    "<Empty>",
                    "2000-12-31T23:59:59",
                    "true",
                    "PT0S",
                    "x <&> \"y\"",
                ],
            ),
        ];
        let expected: Vec<(u32, Vec<String>)> = expected
            .into_iter()
            .map(|(line, cells)| (line, cells.into_iter().map(str::to_string).collect()))
            .collect();
        assert_eq!(seen, expected, "shape {shape:?}");

        // The hidden sheet's tables: one cell, and nothing.
        let plan = [Column::f64(0)];
        let mut sheet = book
            .sheet(1, SheetOptions::default().with_header(false), &plan)
            .unwrap();
        let batch = sheet.read().unwrap().unwrap();
        assert_eq!((batch.rows(), batch.get::<f64>(0, 0).unwrap()), (1, 1.0));
        assert!(sheet.read().unwrap().is_none());
        let mut sheet = book
            .sheet(2, SheetOptions::default().with_header(false), &plan)
            .unwrap();
        assert!(sheet.read().unwrap().is_none());
    }

    // The time format names a custom format by uid, and its pattern lives in the
    // document's custom-format list. A document without the list leaves the format read
    // as it stands — a date — so the same cell is a wall clock.
    let book = Workbook::from_vec(numbers(
        &sheets,
        &NumbersShape {
            custom_formats: false,
            ..NumbersShape::default()
        },
    ))
    .unwrap();
    let plan = [Column::text(3)];
    let mut sheet = book.sheet(0, SheetOptions::default(), &plan).unwrap();
    let batch = sheet.read().unwrap().unwrap();
    assert_eq!(batch.text_str(0, 0).unwrap(), "2024-01-31T10:30:00");
    assert_eq!(batch.text_str(0, 1).unwrap(), "2001-01-01T10:30:00");
}

/// What a Numbers package that is not what it says comes back as.
#[test]
fn numbers_failures_name_what_they_found() {
    let read = |package: &[u8]| -> Result<usize, Failure> {
        let mut harness = Harness::new(package, Sizing::Stingy);
        harness.open()?;
        let sheets = harness.sheets()?;
        harness.load()?;
        let mut rows = 0;
        for sheet in &sheets {
            let read = harness.read(sheet, SheetOptions::default(), &every_door(3), 64);
            if read.failed {
                return Err(harness.last_failure);
            }
            rows += read.rows.len();
        }
        Ok(rows)
    };
    let sheets = vec![NSheet {
        name: "S".to_string(),
        hidden: false,
        tables: vec![NTable {
            name: "T".to_string(),
            header_rows: 1,
            rows: vec![
                vec![
                    NCell::Text("a".to_string()),
                    NCell::Number(1.0),
                    NCell::Bool(true),
                ],
                vec![
                    NCell::Text("b".to_string()),
                    NCell::Number(2.0),
                    NCell::Date(0.0, false),
                ],
            ],
        }],
    }];
    let shape = NumbersShape::default();
    assert_eq!(read(&numbers(&sheets, &shape)), Ok(1));

    // Cells saved by a Numbers older than the storage the core reads.
    let old = numbers(
        &sheets,
        &NumbersShape {
            storage_version: 4,
            ..shape.clone()
        },
    );
    let failure = read(&old).unwrap_err();
    assert_eq!(
        (
            failure.code,
            failure.record,
            failure.found,
            failure.expected
        ),
        (
            Failure::IWA,
            u64::from(Failure::PART_IWA),
            STORAGE_VERSION,
            4
        )
    );

    // Object 1 is not a document: a Keynote or Pages file, or none of Apple's.
    let other = numbers(
        &sheets,
        &NumbersShape {
            document_kind: 2,
            ..shape.clone()
        },
    );
    assert_eq!(read(&other).unwrap_err().code, Failure::NOT_A_WORKBOOK);

    // A tile the table names is not in the package.
    let lost = numbers(
        &sheets,
        &NumbersShape {
            drop_tiles: true,
            ..shape.clone()
        },
    );
    let failure = read(&lost).unwrap_err();
    assert_eq!(
        (failure.code, failure.found),
        (Failure::IWA, MISSING_OBJECT)
    );
    assert_ne!(failure.expected, 0, "the identifier is named");

    // A text cell names a string the list does not have: the cell's row is not delivered,
    // the rows before it are.
    let unstrung = numbers(
        &sheets,
        &NumbersShape {
            lose_strings: true,
            ..shape.clone()
        },
    );
    let failure = read(&unstrung).unwrap_err();
    assert_eq!((failure.code, failure.found), (Failure::SHARED_STRING, 1));

    // A tile part whose first chunk header is not one.
    let mut torn = numbers(&sheets, &shape);
    let needle = b"Index/Tables/Tile-";
    let at = (0..torn.len() - needle.len())
        .find(|&at| {
            &torn[at..at + needle.len()] == needle && &torn[at - 30..at - 26] == b"PK\x03\x04"
        })
        .expect("a tile's local header");
    let name_len = usize::from(u16::from_le_bytes([torn[at - 4], torn[at - 3]]));
    let extra_len = usize::from(u16::from_le_bytes([torn[at - 2], torn[at - 1]]));
    torn[at + name_len + extra_len] = 1;
    let failure = read(&torn).unwrap_err();
    assert_eq!(
        (failure.code, failure.found, failure.byte),
        (Failure::IWA, BROKEN_CHUNK, 0)
    );

    // A chunk that declares more than its block decodes to.
    let mut short = numbers(&sheets, &shape);
    short[at + name_len + extra_len + 4] = 0x7F;
    let failure = read(&short).unwrap_err();
    assert_eq!((failure.code, failure.found), (Failure::IWA, BROKEN_CHUNK));
}

/// A sheet large enough that its part is read through a window that slides many times,
/// with one cell too large for the window it starts in.
#[test]
fn a_large_deflated_sheet_slides_the_window_and_grows_it_once() {
    let mut data = xlsx_sheet(99, 20_000, 6);
    let huge = "long text ".repeat(30_000);
    data.push_str(&format!(
        "<row r=\"900000\"><c r=\"A900000\" t=\"inlineStr\"><is><t>{huge}</t></is></c></row>"
    ));
    assert!(data.len() > 2_000_000);
    let package = xlsx(&[("Big", &data)], packages::SHARED, false, true);
    // Both readings are held whole to be compared, so: a few doors a column, not all.
    let plan: Vec<ColumnSpec> = (0..6u32)
        .flat_map(|ordinal| {
            [18, 11, 13 + ordinal % 5]
                .map(|door| ColumnSpec::new(ordinal, door, u32::from(door == 14)))
        })
        .collect();
    let (refusals, rows) = compare_plan(&package, SheetOptions::default(), &plan, 512);
    assert!(rows > 15_000, "{rows} rows");
    assert!(refusals > 10);

    // The window: asked for at its minimum, and grown only for the one long cell.
    let mut harness = Harness::new(&package, Sizing::Stingy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    assert_eq!(harness.window.len(), WINDOW_MIN);
    let read = harness.read(&sheets[0], SheetOptions::default(), &every_door(1), 512);
    assert!(!read.failed);
    assert!(harness.window.len() > huge.len() && harness.window.len() <= 8 * huge.len());

    let tables = ods_table(5, "Big", 15_000, 6, 2);
    assert!(tables.len() > 2_000_000);
    let package = ods(&tables, true);
    let (_, rows) = compare_plan(&package, SheetOptions::default(), &plan, 512);
    assert!(rows > 10_000, "{rows} rows");
}

/// A real ODS sheet ends in a million empty rows, written as one element. Skipped, they
/// cost nothing; delivered, they are a million empty rows and nothing more.
#[test]
fn a_million_trailing_rows() {
    let package = ods(&ods_table(11, "Sheet", 10, 4, 1_048_000), true);
    let (_, rows) = compare(&package, SheetOptions::default(), 4, 4096);
    assert!(rows < 100);

    let mut harness = Harness::new(&package, Sizing::Roomy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let delivered = SheetOptions::default().with_empty_rows_skipped(false);
    let read = harness.read(&sheets[0], delivered, &[ColumnSpec::new(0, 18, 0)], 65_536);
    assert!(read.rows.len() > 1_048_000);
    assert_eq!(read.rows.last().unwrap().1[0], "empty @0+0");
    let numbers: Vec<u32> = read.rows.iter().map(|row| row.0).collect();
    assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1));
}

/// The parts of a package a reader is tempted to take on trust and the core must not.
#[test]
fn broken_containers_come_back_as_codes() {
    let read = |bytes: &[u8]| -> Result<usize, u32> {
        let mut harness = Harness::new(bytes, Sizing::Stingy);
        harness.window_limit = 1 << 22;
        harness.open().map_err(|failure| failure.code)?;
        let sheets = harness.sheets().map_err(|failure| failure.code)?;
        harness.load().map_err(|failure| failure.code)?;
        let mut rows = 0;
        for sheet in &sheets {
            let read = harness.read(sheet, SheetOptions::default(), &every_door(3), 64);
            if read.failed {
                return Err(harness.last_failure.code);
            }
            rows += read.rows.len();
        }
        Ok(rows)
    };

    assert_eq!(read(b""), Err(Failure::NOT_A_ZIP));
    assert_eq!(read(b"PK\x03\x04 not really"), Err(Failure::NOT_A_ZIP));
    assert_eq!(read(&[0u8; 100_000]), Err(Failure::NOT_A_ZIP));
    let plain = build(&[("hello.txt", b"hello", true)]);
    assert_eq!(read(&plain), Err(Failure::NOT_A_WORKBOOK));
    let wrong = build(&[(
        "mimetype",
        b"application/vnd.oasis.opendocument.text",
        false,
    )]);
    assert_eq!(read(&wrong), Err(Failure::NOT_A_WORKBOOK));

    for deflate in [false, true] {
        for (name, package) in [
            ("xlsx", xlsx_generated(3, 30, 4, deflate)),
            ("ods", ods_generated(3, 30, 4, deflate)),
            ("numbers", numbers_generated(3, 30, 4, deflate)),
        ] {
            let whole = read(&package).unwrap_or_else(|code| panic!("{name}: code {code}"));
            assert!(whole > 20);

            // Cut short anywhere: never a workbook that reads to its end as before.
            let mut rng = Rng(package.len() as u64);
            for _ in 0..300 {
                let cut = rng.below(package.len() as u64) as usize;
                assert!(read(&package[..cut]).is_err(), "{name} cut at {cut}");
            }

            // One byte changed anywhere: a code or a read, never anything else. (What it
            // reads is not checked — a changed digit is a different, valid workbook.)
            for _ in 0..1_500 {
                let mut damaged = package.clone();
                let at = rng.below(damaged.len() as u64) as usize;
                damaged[at] ^= 1 << rng.below(8);
                let _ = read(&damaged);
            }

            // A run of the file replaced with noise.
            for _ in 0..300 {
                let mut damaged = package.clone();
                let at = rng.below(damaged.len() as u64) as usize;
                let len = (rng.below(64) as usize).min(damaged.len() - at);
                for byte in &mut damaged[at..at + len] {
                    *byte = rng.next() as u8;
                }
                let _ = read(&damaged);
            }

            // The central directory replaced with noise, and with nothing.
            let directory = package.len() - 22 - 46;
            let mut damaged = package.clone();
            for byte in &mut damaged[directory - 200..directory] {
                *byte = rng.next() as u8;
            }
            assert!(read(&damaged).is_err(), "{name} with a garbage directory");
        }
    }
}

/// Where the central-directory record of `name` starts.
fn directory_record(package: &[u8], name: &str) -> usize {
    let needle = name.as_bytes();
    let at = (0..package.len() - needle.len())
        .rev()
        .find(|&at| {
            &package[at..at + needle.len()] == needle
                && at >= 46
                && package[at - 46..at - 42] == [0x50, 0x4B, 0x01, 0x02]
        })
        .expect("the entry is in the directory");
    at - 46
}

/// One four-byte field of the central-directory record of `name`.
fn directory_field(package: &[u8], name: &str, field: usize) -> u32 {
    let at = directory_record(package, name) + field;
    u32::from_le_bytes(package[at..at + 4].try_into().unwrap())
}

/// Finds the central-directory record of `name` and overwrites one of its fields.
fn patch_directory(package: &mut [u8], name: &str, field: usize, value: u32) {
    let at = directory_record(package, name) + field;
    package[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn sizes_that_lie_are_not_believed() {
    let sheet = xlsx_sheet(8, 200, 4);
    let package = xlsx(&[("Data", &sheet)], packages::SHARED, false, true);
    let specs = every_door(4);
    let rows_of = |bytes: &[u8]| -> Result<usize, u32> {
        let mut harness = Harness::new(bytes, Sizing::Stingy);
        harness.window_limit = 1 << 22;
        harness.open().map_err(|failure| failure.code)?;
        let sheets = harness.sheets().map_err(|failure| failure.code)?;
        harness.load().map_err(|failure| failure.code)?;
        let read = harness.read(&sheets[0], SheetOptions::default(), &specs, 64);
        if read.failed {
            return Err(harness.last_failure.code);
        }
        Ok(read.rows.len())
    };
    let whole = rows_of(&package).unwrap();

    // An inflated size far past the truth: the part still ends where its stream does.
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 24, u32::MAX - 1);
    assert_eq!(rows_of(&lying), Ok(whole));

    // One short of the truth: the part ends early, inside the XML.
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 24, 2_000);
    assert_eq!(rows_of(&lying), Err(Failure::XML));

    // A compressed size past the end of the container, and one that stops short.
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 20, u32::MAX - 1);
    assert_eq!(rows_of(&lying), Ok(whole));
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 20, 100);
    assert_eq!(rows_of(&lying), Err(Failure::DEFLATE));

    // A local header that is somewhere else entirely.
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 42, u32::MAX - 7);
    assert_eq!(rows_of(&lying), Err(Failure::CONTAINER));
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/worksheets/sheet0.xml", 42, 3);
    assert_eq!(rows_of(&lying), Err(Failure::CONTAINER));

    // A method that is neither, and the encrypted bit.
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/sharedStrings.xml", 10, 12);
    assert_eq!(rows_of(&lying), Err(Failure::METHOD));
    let mut lying = package.clone();
    patch_directory(&mut lying, "xl/styles.xml", 8, 1);
    assert_eq!(rows_of(&lying), Err(Failure::ENCRYPTED));

    // A shared string that is not in the table.
    let sheet = "<row><c t=\"s\"><v>0</v></c></row><row><c t=\"s\"><v>7</v></c></row>";
    let package = xlsx(&[("Data", sheet)], packages::SHARED, false, false);
    let mut harness = Harness::new(&package, Sizing::Stingy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let read = harness.read(
        &sheets[0],
        SheetOptions::default().with_header(false),
        &specs,
        64,
    );
    assert!(read.failed);
    assert_eq!(read.rows.len(), 1, "the row before the break is delivered");
    assert_eq!(harness.last_failure.code, Failure::SHARED_STRING);
    assert_eq!(
        (harness.last_failure.found, harness.last_failure.expected),
        (7, 7)
    );
    assert_eq!(harness.last_failure.line, 2);
}

/// The smallest XLSX package the core opens, around one worksheet and one string table.
fn xlsx_around(sheet: &[u8], strings: &[u8], deflate: bool) -> Vec<u8> {
    build(&[
        ("[Content_Types].xml", b"<Types/>", false),
        (
            "xl/workbook.xml",
            b"<workbook xmlns:r=\"r\"><sheets><sheet name=\"S\" r:id=\"r1\"/></sheets></workbook>",
            deflate,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            b"<Relationships><Relationship Id=\"r1\" Type=\"x/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>",
            deflate,
        ),
        ("xl/sharedStrings.xml", strings, deflate),
        ("xl/worksheets/sheet1.xml", sheet, deflate),
    ])
}

/// A part that ends between two rows — cut off there, or declared that long by a directory
/// that lies — is a broken part, not a shorter one: the rows before the cut are delivered,
/// then the failure, which names the part and the element left open. Every XML part is
/// held to this; a string table cut between two strings is one.
#[test]
fn a_part_cut_between_rows_is_broken() {
    const ONE: &str = "<worksheet><sheetData><row r=\"1\"><c t=\"s\"><v>0</v></c></row>";
    const TWO: &str = "<row r=\"2\"><c t=\"s\"><v>1</v></c></row></sheetData></worksheet>";
    const STRINGS: &str = "<sst><si><t>kept</t></si><si><t>lost</t></si></sst>";
    let whole = format!("{ONE}{TWO}");
    for deflate in [false, true] {
        for sizing in [Sizing::Stingy, Sizing::Roomy] {
            let read = |package: &[u8]| -> (Vec<String>, Option<Failure>) {
                let mut harness = Harness::new(package, sizing);
                harness.open().unwrap();
                let sheets = harness.sheets().unwrap();
                harness.load().unwrap();
                let read = harness.read(
                    &sheets[0],
                    SheetOptions::default().with_header(false),
                    &[ColumnSpec::new(0, 18, 0)],
                    16,
                );
                let cells = read
                    .rows
                    .iter()
                    .map(|(_, cells)| cells[0].clone())
                    .collect();
                (cells, read.failed.then_some(harness.last_failure))
            };
            let intact = read(&xlsx_around(whole.as_bytes(), STRINGS.as_bytes(), deflate));
            assert_eq!(
                intact,
                (vec!["ok \"kept\"".into(), "ok \"lost\"".into()], None)
            );

            // The worksheet's XML stops after the first row.
            let cut = read(&xlsx_around(ONE.as_bytes(), STRINGS.as_bytes(), deflate));
            let (rows, failure) = &cut;
            assert_eq!(rows, &["ok \"kept\""], "deflate {deflate}, {sizing:?}");
            let failure = failure.expect("a part cut inside sheetData is broken");
            assert_eq!(failure.code, Failure::XML);
            assert_eq!(failure.found, UNTERMINATED_ELEMENT);
            assert_eq!(failure.record, u64::from(Failure::PART_SHEET));
            assert_eq!(failure.byte, ONE.len() as u64);

            // The worksheet is whole; the directory says it ends after the first row.
            let mut lying = xlsx_around(whole.as_bytes(), STRINGS.as_bytes(), deflate);
            patch_directory(&mut lying, "xl/worksheets/sheet1.xml", 24, ONE.len() as u32);
            assert_eq!(read(&lying), cut, "deflate {deflate}, {sizing:?}");

            // The string table stops after its first string: the workbook does not load.
            let strings = "<sst><si><t>kept</t></si>";
            let package = xlsx_around(whole.as_bytes(), strings.as_bytes(), deflate);
            let mut harness = Harness::new(&package, sizing);
            harness.open().unwrap();
            harness.sheets().unwrap();
            let failure = harness.load().unwrap_err();
            assert_eq!(
                (failure.code, failure.found, failure.record),
                (
                    Failure::XML,
                    UNTERMINATED_ELEMENT,
                    u64::from(Failure::PART_STRINGS)
                )
            );
        }
    }

    // Through the Rust binding: the intact row, then the failure, which is final.
    let package = xlsx_around(ONE.as_bytes(), STRINGS.as_bytes(), true);
    let book = hypertabular::Workbook::from_vec(package).unwrap();
    let mut sheet = book
        .sheet(
            0,
            SheetOptions::default().with_header(false),
            &[hypertabular::Column::text(0)],
        )
        .unwrap();
    let batch = sheet.read().unwrap().expect("the row before the cut");
    assert_eq!(batch.rows(), 1);
    assert_eq!(batch.text_str(0, 0).unwrap(), "kept");
    for _ in 0..3 {
        let Err(hypertabular::Error::Structure(failure)) = sheet.read() else {
            panic!("a cut sheet ends in a failure, every time it is read");
        };
        assert_eq!(failure.kind, hypertabular::FailureKind::Xml);
    }
}

/// The size a directory declares for the string table is what a caller sizes its arena by
/// before the table is read, so it is held to what the part's bytes can inflate to: a
/// tiny part that claims a gigabyte is not given one, and still reads.
#[test]
fn a_string_table_that_claims_a_gigabyte_is_not_given_one() {
    let sheet = xlsx_sheet(8, 50, 4);
    for deflate in [false, true] {
        let package = xlsx(&[("Data", &sheet)], packages::SHARED, false, deflate);
        let compressed = u64::from(directory_field(&package, "xl/sharedStrings.xml", 20));
        let rows_of = |bytes: &[u8]| -> (u64, usize) {
            let mut harness = Harness::new(bytes, Sizing::Stingy);
            let strings_bytes = harness.open().unwrap().strings_bytes;
            let sheets = harness.sheets().unwrap();
            harness.load().unwrap();
            let read = harness.read(&sheets[0], SheetOptions::default(), &every_door(4), 64);
            assert!(!read.failed);
            (strings_bytes, read.rows.len())
        };
        let (honest, rows) = rows_of(&package);
        assert!(honest > 0 && rows > 0);

        let mut lying = package.clone();
        patch_directory(&mut lying, "xl/sharedStrings.xml", 24, 1 << 30);
        let (bounded, read) = rows_of(&lying);
        assert_eq!(read, rows, "a lying size does not change what is read");
        let inflatable = if deflate {
            compressed * DEFLATE_RATIO + 64
        } else {
            compressed
        };
        assert_eq!(bounded, inflatable.min(1 << 30), "deflate {deflate}");
        assert!(
            bounded < 1 << 20,
            "{bounded} bytes asked of a {compressed}-byte part"
        );

        // Both sizes lie: what the container holds past the part's bytes is the bound.
        patch_directory(&mut lying, "xl/sharedStrings.xml", 20, 1 << 30);
        let mut harness = Harness::new(&lying, Sizing::Stingy);
        let bounded = harness.open().unwrap().strings_bytes;
        assert!(
            bounded <= (lying.len() as u64) * DEFLATE_RATIO + 64,
            "{bounded} bytes asked of a {}-byte package",
            lying.len()
        );
    }
}

/// A deflate bomb is only as large as the window it is read through.
#[test]
fn a_bomb_is_bounded_by_the_window() {
    // 64 MiB of nothing in particular before the first row: tokens, none of them large.
    let mut sheet = String::from("<?xml version=\"1.0\"?><worksheet>");
    for _ in 0..16 << 20 {
        sheet.push_str("<x/>");
    }
    sheet.push_str("<sheetData><row><c><v>1</v></c></row></sheetData></worksheet>");
    let package = build(&[
        ("[Content_Types].xml", b"<Types/>", false),
        (
            "xl/workbook.xml",
            b"<workbook xmlns:r=\"r\"><sheets><sheet name=\"S\" r:id=\"r1\"/></sheets></workbook>",
            true,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            b"<Relationships><Relationship Id=\"r1\" Type=\"x/worksheet\" Target=\"s.xml\"/></Relationships>",
            true,
        ),
        ("xl/s.xml", sheet.as_bytes(), true),
    ]);
    drop(sheet);
    assert!(
        package.len() < 1 << 20,
        "{} bytes hold 64 MiB",
        package.len()
    );
    let mut harness = Harness::new(&package, Sizing::Stingy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let read = harness.read(
        &sheets[0],
        SheetOptions::default().with_header(false),
        &every_door(1),
        16,
    );
    assert_eq!(read.rows.len(), 1);
    assert_eq!(
        harness.window.len(),
        WINDOW_MIN,
        "the window never had to grow"
    );
    assert!(harness.arena.len() < 4096);

    // One token of 64 MiB: the core asks for a window it would fit, and the caller is the
    // one who decides whether to give it. The harness says no by panicking, which only a
    // target that unwinds can catch; under wasm32-wasip1 panics abort.
    #[cfg(panic = "unwind")]
    a_token_larger_than_the_caller_allows();
}

#[cfg(panic = "unwind")]
fn a_token_larger_than_the_caller_allows() {
    let mut strings = String::from("<sst><si><t>");
    strings.push_str(&"a".repeat(64 << 20));
    strings.push_str("</t></si></sst>");
    let inflated = strings.len() as u64;
    let package = build(&[
        ("[Content_Types].xml", b"<Types/>", false),
        ("xl/workbook.xml", b"<workbook/>", true),
        ("xl/sharedStrings.xml", strings.as_bytes(), true),
    ]);
    drop(strings);
    assert!(package.len() < 1 << 20);
    let mut harness = Harness::new(&package, Sizing::Stingy);
    assert_eq!(harness.open().unwrap().strings_bytes, inflated);
    harness.window_limit = 1 << 20;
    let asked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| harness.load()));
    assert!(
        asked.is_err(),
        "the harness refused to grow the window past 1 MiB"
    );
    assert!(harness.window.len() <= 1 << 20);
}

/// The core says a number back as Rust does: an integral one below 1e16 as an integer,
/// any other as the shortest decimal that reads back as the same double (`{:?}`).
#[test]
#[allow(clippy::excessive_precision)]
fn numbers_are_said_as_rust_says_them() {
    let said = |value: f64| {
        let mut text = Text::new();
        text.number(value);
        String::from_utf8(text.as_bytes().to_vec()).unwrap()
    };
    let expected = |value: f64| {
        if value.fract() == 0.0 && value.abs() < 1e16 {
            format!("{}", value as i64)
        } else {
            format!("{value:?}")
        }
    };
    let edges = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        0.5,
        2.5,
        1e15,
        1e16,
        9.999999999999998e15,
        1e-4,
        9.999e-5,
        1e-5,
        123456789.123456789,
        5e-324,
        2.2250738585072014e-308,
        2.225073858507201e-308,
        1.7976931348623157e308,
        4.35,
        0.3,
        1e21,
        1e22,
        1e23,
        9007199254740993.0,
        0.000123,
        123456.7,
        1.0e300,
        4.9406564584124654e-324,
        1.5,
        45000.5,
        f64::EPSILON,
        299792458.0,
        6.02214076e23,
        1.0 / 3.0,
        2.0 / 3.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        8.41e21,
        5.0e-324,
        9.5e-5,
        0.09999999999999999,
    ];
    for value in edges {
        assert_eq!(said(value), expected(value), "{value:e}");
        assert_eq!(said(-value), expected(-value), "-{value:e}");
    }
    let mut rng = Rng(42);
    for _ in 0..200_000 {
        // Every bit pattern is a double; most are finite.
        let bits = rng.next() << 33 ^ rng.next() << 2 ^ rng.next() >> 29;
        let value = f64::from_bits(bits);
        assert_eq!(said(value), expected(value), "{bits:#x}");
        // And doubles of the size a workbook holds.
        let value = (rng.below(1 << 40) as f64) / (10f64.powi(rng.below(12) as i32));
        assert_eq!(said(value), expected(value), "{value:e}");
    }
}

/// The core reads a number as Rust does, where what Rust reads is finite, but for zero: every
/// spelling of it is read (`-0`, `+0.00000000`, an underflow such as `-1e-999`), and the core
/// only ever returns unsigned zero (HyperCast's contract: zero carries no sign). And but for
/// white space after a leading sign, which HyperCast's numeric doors read and Rust refuses: a
/// spreadsheet doesn't write it, and what it means is not in doubt.
#[test]
fn numbers_are_read_as_rust_reads_them() {
    let expected = |text: &str| {
        let closed = match text.split_at_checked(1) {
            Some((sign @ ("+" | "-"), rest)) => format!("{sign}{}", rest.trim_start()),
            _ => text.to_string(),
        };
        closed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| if value == 0.0 { 0.0 } else { value })
    };
    let same = |text: &str| {
        let (ours, theirs) = (parse_f64(text.as_bytes()), expected(text));
        assert_eq!(
            ours.map(f64::to_bits),
            theirs.map(f64::to_bits),
            "{text:?}: {ours:?} against {theirs:?}"
        );
    };
    for text in [
        "",
        " ",
        "0",
        "-0",
        "+0",
        "+0.00000000",
        "-0.0",
        "-.0e5",
        "-0e-999",
        "5.",
        ".5",
        "+.5",
        "-.5e-3",
        "1e5",
        "1E5",
        "1e+5",
        "1.e5",
        ".",
        "e5",
        "1e",
        "+",
        "-",
        "1e+",
        "inf",
        "-inf",
        "infinity",
        "NaN",
        "nan",
        "1e999",
        "-1e999",
        "1e-999",
        "-1e-999",
        "4.9e-324",
        "2.4e-324",
        "1,234",
        "1_000",
        "0x10",
        "50%",
        "(5)",
        " 5",
        "5 ",
        "5\n",
        "\t5",
        "1.2.3",
        "1e5.5",
        "+-1",
        "--1",
        "1.7976931348623157e308",
        "1.7976931348623159e308",
        "123456789012345678901234567890",
        "0.1",
        "45000.5",
        "٣",
        "１",
        "1e0005",
        "0e99999999999",
        "\u{0b}5",
        "5\u{0b}",
        "5\u{0c}",
        "$5",
        "5$",
        "1d",
        "1f",
        "0.30000000000000004",
    ] {
        same(text);
    }
    assert_eq!(parse_f64(b"+  4418"), Some(4418.0));
    assert_eq!(parse_f64("-\u{a0}.5".as_bytes()), Some(-0.5));
    assert_eq!(parse_f64(b" 4418"), None);
    assert_eq!(parse_f64(b"4418 "), None);
    same(&("1".repeat(400) + "." + &"3".repeat(400) + "e-390"));
    same(&("0.".to_string() + &"0".repeat(400) + "1e390"));
    let alphabet: Vec<char> = "0123456789+-.eE ,x%()infa\u{0b}".chars().collect();
    let mut rng = Rng(7);
    for _ in 0..300_000 {
        let len = 1 + rng.below(9) as usize;
        let text: String = (0..len)
            .map(|_| {
                // Mostly digits, so that most strings are numbers.
                let at = if rng.chance(2) {
                    rng.below(10)
                } else {
                    rng.below(alphabet.len() as u64)
                };
                alphabet[at as usize]
            })
            .collect();
        same(&text);
    }
}

/// The same read through the C ABI itself, as a binding makes it: the state block as bytes
/// of the caller's (junk to begin with), the buffers through one `Buffers`, nulls where
/// the contract allows them and refused where it does not.
#[test]
fn the_exports_read_what_the_core_reads() {
    use hypertabular::kernel::abi::{
        Buffers, CellVerdict, ColumnBuffer, ERR_CONTRACT, Filled, OK, Opened, Slot, Span,
    };
    use hypertabular::kernel::door::Door;
    use hypertabular::kernel::exports::{
        hypertabular_workbook_fill, hypertabular_workbook_header, hypertabular_workbook_open,
        hypertabular_workbook_sheet, hypertabular_workbook_sheets,
        hypertabular_workbook_state_size, hypertabular_workbook_strings,
        hypertabular_workbook_styles,
    };
    use std::ptr::{null, null_mut};
    use std::slice::from_raw_parts;

    const BATCH: usize = 16;
    let package = xlsx_generated(21, 60, 4, true);
    let specs = every_door(4);
    let mut harness = Harness::new(&package, Sizing::Roomy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let expected = harness.read(&sheets[0], SheetOptions::default(), &specs, BATCH);
    assert!(expected.rows.len() > 30 && !expected.failed);

    let (bytes, len) = (package.as_ptr(), package.len());
    let mut window = vec![0u8; 1 << 18];
    let mut arena = vec![0u8; 1 << 20];
    let mut cells = vec![Span::default(); 1 << 14];
    let mut row = vec![Slot::default(); 4];
    let mut buffers = Buffers {
        window: window.as_mut_ptr(),
        window_cap: window.len(),
        arena: arena.as_mut_ptr(),
        arena_cap: arena.len(),
        cells: cells.as_mut_ptr(),
        cells_cap: cells.len(),
        row: row.as_mut_ptr(),
        row_cap: row.len(),
        strings: null(),
        strings_len: 0,
        table: null(),
        table_len: 0,
        kinds: null(),
        kinds_len: 0,
    };
    let mut values: Vec<Vec<u8>> = specs
        .iter()
        .map(|spec| {
            let door = Door::from_code(spec.door, spec.param).unwrap();
            vec![0u8; support::size_of_door(door) * BATCH]
        })
        .collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = specs
        .iter()
        .map(|_| vec![CellVerdict::default(); BATCH])
        .collect();
    let columns: Vec<ColumnBuffer> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        })
        .collect();

    // SAFETY: every pointer handed over is live for the sizes beside it, and what the
    // core wrote is read back through the same pointers.
    unsafe {
        let size = hypertabular_workbook_state_size();
        assert_eq!(size % 8, 0);
        let mut block = vec![0xAAAA_AAAA_AAAA_AAAAu64; size / 8];
        let state = block.as_mut_ptr().cast();
        let mut opened = Opened::default();
        let mut out = Filled::default();

        // Nulls where memory is needed, and a block nothing has opened.
        let code = hypertabular_workbook_open(null_mut(), bytes, len, &buffers, &mut opened);
        assert_eq!(code, ERR_CONTRACT);
        let code = hypertabular_workbook_open(state, bytes, len, null(), &mut opened);
        assert_eq!(code, ERR_CONTRACT);
        let code = hypertabular_workbook_open(state, bytes, len, &buffers, null_mut());
        assert_eq!(code, ERR_CONTRACT);
        let code = hypertabular_workbook_sheets(state, bytes, len, &buffers, &mut out);
        assert_eq!(code, ERR_CONTRACT);

        let code = hypertabular_workbook_open(state, bytes, len, &buffers, &mut opened);
        assert_eq!((code, opened.format), (OK, 1));
        let code = hypertabular_workbook_sheets(state, bytes, len, &buffers, &mut out);
        assert_eq!((code, out.rows as usize), (OK, sheets.len()));
        let part = *buffers.cells.add(1);
        let part = from_raw_parts(buffers.arena.add(part.offset as usize), part.len()).to_vec();
        assert_eq!(part, sheets[0].part);

        let code = hypertabular_workbook_strings(state, bytes, len, &buffers, &mut out);
        assert_eq!((code, out.rows as usize), (OK, harness.table.len()));
        let strings = from_raw_parts(buffers.arena, out.arena_used as usize).to_vec();
        let table = from_raw_parts(buffers.cells, out.rows as usize).to_vec();
        let code = hypertabular_workbook_styles(state, bytes, len, &buffers, &mut out);
        assert_eq!((code, out.rows as usize), (OK, harness.kinds.len()));
        let kinds = from_raw_parts(buffers.arena, out.rows as usize).to_vec();
        assert_eq!((&strings, &kinds), (&harness.strings, &harness.kinds));
        (buffers.strings, buffers.strings_len) = (strings.as_ptr(), strings.len());
        (buffers.table, buffers.table_len) = (table.as_ptr(), table.len());
        (buffers.kinds, buffers.kinds_len) = (kinds.as_ptr(), kinds.len());

        // A fill before the header has been asked for is the caller's mistake.
        let code = hypertabular_workbook_sheet(
            state,
            bytes,
            len,
            part.as_ptr(),
            part.len(),
            0,
            1,
            1,
            &mut out,
        );
        assert_eq!(code, OK);
        let code = hypertabular_workbook_fill(
            state,
            bytes,
            len,
            specs.as_ptr(),
            columns.as_ptr(),
            specs.len(),
            BATCH,
            &buffers,
            &mut out,
        );
        assert_eq!(code, ERR_CONTRACT);
        let code = hypertabular_workbook_header(state, bytes, len, &buffers, &mut out);
        assert_eq!(
            (code, out.rows as usize),
            (OK, expected.header.as_ref().unwrap().len())
        );

        let mut read = 0usize;
        loop {
            let code = hypertabular_workbook_fill(
                state,
                bytes,
                len,
                specs.as_ptr(),
                columns.as_ptr(),
                specs.len(),
                BATCH,
                &buffers,
                &mut out,
            );
            assert_eq!(code, OK);
            for at in 0..out.rows as usize {
                let (number, cells) = &expected.rows[read + at];
                let entry = *buffers.cells.add(at * (specs.len() + 1) + specs.len());
                assert_eq!(entry.offset, *number);
                for (column, cell) in cells.iter().enumerate() {
                    let reason = match cell.as_bytes()[0] {
                        b'o' => 0,
                        b'e' => 1,
                        _ => u32::from(cell.as_bytes()[6] - b'0'),
                    };
                    let verdict = columns[column].verdicts.add(at).read();
                    assert_eq!(verdict.reason, reason, "row {}, column {column}", read + at);
                }
            }
            read += out.rows as usize;
            if (out.rows as usize) < BATCH {
                break;
            }
        }
        assert_eq!(read, expected.rows.len());
    }
}

/// One number, not a benchmark suite: the core called directly, and the same read through
/// the Rust binding over it, on one large, ordinary sheet — the same package, the same
/// plan, batches of the same size. The difference is what the binding costs.
/// `cargo test --release --lib tests::kernel_workbook::throughput -- --ignored --nocapture`
#[test]
#[ignore = "a timing, run on request in a release build"]
fn throughput() {
    use hypertabular::kernel::abi::{CellVerdict, ColumnBuffer, Filled, OK};
    use hypertabular::kernel::door::Door;
    use hypertabular::kernel::workbook::{Memory, rows};
    use hypertabular::{Column, Workbook};
    use std::time::Instant;

    const ROWS: u64 = 300_000;
    const BATCH: usize = 4096;
    let mut data = String::new();
    data.push_str("<row r=\"1\">");
    for name in [
        "id", "amount", "name", "when", "note", "flag", "ratio", "code",
    ] {
        data.push_str(&format!("<c t=\"inlineStr\"><is><t>{name}</t></is></c>"));
    }
    data.push_str("</row>");
    for row in 2..ROWS + 2 {
        data.push_str(&format!(
            "<row r=\"{row}\" spans=\"1:8\"><c r=\"A{row}\"><v>{row}</v></c>\
<c r=\"B{row}\"><v>{}.{:02}</v></c><c r=\"C{row}\" t=\"s\"><v>{}</v></c>\
<c r=\"D{row}\" s=\"2\"><v>{}.{}</v></c>\
<c r=\"E{row}\" t=\"inlineStr\"><is><t>note {row}</t></is></c>\
<c r=\"F{row}\" t=\"b\"><v>{}</v></c><c r=\"G{row}\"><v>{}E-3</v></c>\
<c r=\"H{row}\" t=\"str\"><v>{}</v></c></row>",
            row * 37 % 100_000,
            row % 100,
            row % 7,
            40_000 + row % 5_000,
            row % 1_000,
            row % 2,
            row * 13 % 9_973,
            row * 7,
        ));
    }
    let inflated = data.len() as f64 / 1e6;
    let package = xlsx(&[("Data", &data)], packages::SHARED, false, true);
    drop(data);
    let specs: Vec<ColumnSpec> = [5, 11, 18, 13, 18, 1, 11, 18]
        .into_iter()
        .enumerate()
        .map(|(ordinal, door)| ColumnSpec::new(ordinal as u32, door, 0))
        .collect();

    let core = || {
        let mut harness = Harness::new(&package, Sizing::Roomy);
        harness.open().unwrap();
        let sheets = harness.sheets().unwrap();
        harness.load().unwrap();
        harness.row = vec![Default::default(); specs.len()];
        let mut values: Vec<Vec<u8>> = specs
            .iter()
            .map(|spec| {
                let door = Door::from_code(spec.door, spec.param).unwrap();
                vec![0u8; support::size_of_door(door) * BATCH]
            })
            .collect();
        let mut verdicts: Vec<Vec<CellVerdict>> = specs
            .iter()
            .map(|_| vec![CellVerdict::default(); BATCH])
            .collect();
        let columns: Vec<ColumnBuffer> = values
            .iter_mut()
            .zip(verdicts.iter_mut())
            .map(|(values, verdicts)| ColumnBuffer {
                values: values.as_mut_ptr().cast(),
                verdicts: verdicts.as_mut_ptr(),
            })
            .collect();
        let mut out = Filled::default();
        let code = rows::sheet(
            &mut harness.state,
            &package,
            &sheets[0].part,
            0,
            true,
            true,
            &mut out,
        );
        assert_eq!(code, OK);
        let mut total = 0u64;
        let mut header = false;
        loop {
            let mut memory = Memory {
                window: &mut harness.window,
                arena: &mut harness.arena,
                cells: &mut harness.cells,
                row: &mut harness.row,
                strings: &harness.strings,
                table: &harness.table,
                kinds: &harness.kinds,
            };
            if !header {
                let code = rows::header(&mut harness.state, &package, &mut memory, &mut out);
                assert_eq!((code, out.rows), (OK, 8));
                header = true;
                continue;
            }
            // SAFETY: every column's arrays hold `BATCH` elements.
            let code = unsafe {
                rows::fill(
                    &mut harness.state,
                    &package,
                    &specs,
                    &columns,
                    BATCH,
                    &mut memory,
                    &mut out,
                )
            };
            assert_eq!(code, OK);
            total += out.rows;
            if (out.rows as usize) < BATCH {
                return total;
            }
        }
    };
    let binding = || {
        let plan: Vec<Column> = specs
            .iter()
            .map(|spec| {
                let door = Door::from_code(spec.door, spec.param).unwrap();
                Column::new(spec.ordinal as usize, door)
            })
            .collect();
        let workbook = Workbook::from_slice(&package).unwrap();
        let options = SheetOptions::default().with_batch_rows(BATCH);
        let mut sheet = workbook.sheet(0, options, &plan).unwrap();
        let mut total = 0u64;
        while let Some(batch) = sheet.read().unwrap() {
            total += batch.rows() as u64;
        }
        total
    };
    let best = |read: &dyn Fn() -> u64| {
        (0..5)
            .map(|_| {
                let start = Instant::now();
                assert_eq!(read(), ROWS);
                start.elapsed().as_secs_f64()
            })
            .fold(f64::INFINITY, f64::min)
    };
    let (ours, theirs) = (best(&core), best(&binding));
    println!(
        "{ROWS} rows, 8 columns, {inflated:.1} MB of sheet XML in a {:.1} MB package",
        package.len() as f64 / 1e6
    );
    println!(
        "core:    {:.0} ms, {:.0} MB/s, {:.2} M rows/s",
        ours * 1e3,
        inflated / ours,
        ROWS as f64 / ours / 1e6
    );
    println!(
        "binding: {:.0} ms, {:.0} MB/s, {:.2} M rows/s",
        theirs * 1e3,
        inflated / theirs,
        ROWS as f64 / theirs / 1e6
    );
}

/// A package with `bad` where `PLACEHOLDER` was: the parts are stored, so the bytes land as they
/// are, whatever the format's own builder would let a string hold.
const PLACEHOLDER: &str = "Q1W2E3R4";

fn planted(mut package: Vec<u8>, bad: &[u8; 8]) -> Vec<u8> {
    let mut found = 0;
    for at in 0..package.len().saturating_sub(8) {
        if &package[at..at + 8] == PLACEHOLDER.as_bytes() {
            package[at..at + 8].copy_from_slice(bad);
            found += 1;
        }
    }
    assert!(found > 0, "the placeholder is in the package");
    package
}

/// One text cell where the placeholder was, and a good one beside it, in each format.
fn each_format(top: &[&str], bottom: &[&str], bad: &[u8; 8]) -> Vec<(&'static str, Vec<u8>)> {
    let shape = NumbersShape::default();
    let row = |cells: &[&str]| -> Vec<NCell> {
        cells
            .iter()
            .map(|&cell| NCell::Text(cell.to_string()))
            .collect()
    };
    let sheets = vec![NSheet {
        name: "S".to_string(),
        hidden: false,
        tables: vec![NTable {
            name: "T".to_string(),
            header_rows: 1,
            rows: vec![row(top), row(bottom)],
        }],
    }];
    let xlsx_row = |cells: &[&str], number: usize| -> String {
        let cells: String = cells
            .iter()
            .map(|cell| format!("<c t=\"inlineStr\"><is><t>{cell}</t></is></c>"))
            .collect();
        format!("<row r=\"{number}\">{cells}</row>")
    };
    let ods_row = |cells: &[&str]| -> String {
        let cells: String = cells
            .iter()
            .map(|cell| {
                format!(
                    "<table:table-cell office:value-type=\"string\"><text:p>{cell}</text:p></table:table-cell>"
                )
            })
            .collect();
        format!("<table:table-row>{cells}</table:table-row>")
    };
    let sheet = format!("{}{}", xlsx_row(top, 1), xlsx_row(bottom, 2));
    let table = format!(
        "<table:table table:name=\"S\">{}{}</table:table>",
        ods_row(top),
        ods_row(bottom)
    );
    vec![
        (
            "xlsx",
            planted(xlsx(&[("S", &sheet)], "", false, false), bad),
        ),
        ("ods", planted(ods(&table, false), bad)),
        ("numbers", planted(numbers(&sheets, &shape), bad)),
    ]
}

#[test]
fn invalid_utf8_in_a_workbook_cell_is_malformed() {
    let specs = [ColumnSpec::new(0, 18, 0), ColumnSpec::new(1, 18, 0)];
    let package = each_format(&["a", "b"], &[PLACEHOLDER, "fine"], b"ok\xe9okkkk");
    assert_eq!(package.len(), 3);
    for (format, package) in package {
        for sizing in [Sizing::Stingy, Sizing::Roomy] {
            let mut harness = Harness::new(&package, sizing);
            harness.open().unwrap();
            let sheets = harness.sheets().unwrap();
            harness.load().unwrap();
            let read = harness.read(&sheets[0], SheetOptions::default(), &specs, 4);
            assert!(!read.failed, "{format}, {sizing:?}");
            assert_eq!(read.rows.len(), 1, "{format}, {sizing:?}");
            assert_eq!(
                read.rows[0].1,
                [
                    format!("fault 2 @2+1 {:?}", String::from_utf8_lossy(b"ok\xe9okkkk")),
                    "ok \"fine\"".to_string(),
                ],
                "{format}, {sizing:?}"
            );
        }
    }
    // Through a shared string too, deflated, which is XLSX's own way of holding text.
    let package = xlsx_around(
        b"<worksheet><sheetData><row><c t=\"s\"><v>0</v></c></row></sheetData></worksheet>",
        b"<sst><si><t>\xc3</t></si></sst>",
        true,
    );
    let mut harness = Harness::new(&package, Sizing::Stingy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let options = SheetOptions::default().with_header(false);
    let read = harness.read(&sheets[0], options, &specs[..1], 4);
    assert_eq!(read.rows[0].1[0], format!("fault 2 @0+1 {:?}", "\u{FFFD}"));
}

/// A sheet of shared strings: three columns of `rows`, each cell naming a string by index.
fn shared_sheet(rows: &[[u32; 3]]) -> Vec<u8> {
    let mut sheet = String::from("<worksheet><sheetData>");
    for row in rows {
        sheet.push_str("<row>");
        for index in row {
            sheet.push_str(&format!("<c t=\"s\"><v>{index}</v></c>"));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    sheet.into_bytes()
}

/// What a text cell reads as in a [`Harness`] read: the door's verdict of `text`, spans and all.
fn judged(text: &[u8]) -> String {
    match hypercast::cast_text(text) {
        Ok(_) => format!("ok {:?}", String::from_utf8_lossy(text)),
        Err(fault) if text.is_empty() => {
            assert_eq!((fault.offset, fault.len), (0, 0));
            "empty @0+0".to_string()
        }
        Err(fault) => format!(
            "fault 2 @{}+{} {:?}",
            fault.offset,
            fault.len,
            String::from_utf8_lossy(text)
        ),
    }
}

/// A shared string is asked of the door once, when the strings are loaded, and every cell that
/// names it reads as the door says of it: valid ones as text, a broken one as its own fault with
/// the cell's own span, wherever and however often it's named. Two strings side by side that are
/// each broken and together one character are still each broken, and a string of no bytes is
/// empty. Both through a stored part and a deflated one, with room and without.
#[test]
fn a_shared_string_is_judged_once_and_reads_the_same_in_every_cell() {
    let strings: [&[u8]; 7] = [
        b"plain",
        "caf\u{e9} \u{6771}\u{4eac}".as_bytes(),
        b"ab\xc3",
        b"caf\xc3",
        b"\xa9t",
        b"",
        b"",
    ];
    let mut table = b"<sst>".to_vec();
    for (index, text) in strings.iter().enumerate() {
        if index == 6 {
            table.extend_from_slice(b"<si/>");
        } else {
            table.extend_from_slice(b"<si><t>");
            table.extend_from_slice(text);
            table.extend_from_slice(b"</t></si>");
        }
    }
    table.extend_from_slice(b"</sst>");
    let rows = [
        [0, 1, 2],
        [2, 3, 4],
        [4, 0, 2],
        [5, 6, 1],
        [1, 1, 1],
        [3, 4, 3],
    ];
    let specs = [
        ColumnSpec::new(0, 18, 0),
        ColumnSpec::new(1, 18, 0),
        ColumnSpec::new(2, 18, 0),
    ];
    let expected: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|&index| judged(strings[index as usize]))
                .collect()
        })
        .collect();
    assert!(expected[1].iter().all(|cell| cell.starts_with("fault 2 @")));
    assert_eq!(expected[1][1], format!("fault 2 @3+1 {:?}", "caf\u{FFFD}"));
    for deflate in [false, true] {
        let package = xlsx_around(&shared_sheet(&rows), &table, deflate);
        for sizing in [Sizing::Stingy, Sizing::Roomy] {
            let mut harness = Harness::new(&package, sizing);
            harness.open().unwrap();
            let sheets = harness.sheets().unwrap();
            harness.load().unwrap();
            let options = SheetOptions::default().with_header(false);
            for batch in [8, 1, 3] {
                let read = harness.read(&sheets[0], options, &specs, batch);
                assert!(!read.failed, "{deflate}, {sizing:?}, {batch}");
                let cells: Vec<Vec<String>> = read.rows.iter().map(|row| row.1.clone()).collect();
                assert_eq!(cells, expected, "{deflate}, {sizing:?}, {batch}");
            }
        }
    }
}

/// What the load found of one workbook's strings is that workbook's alone. Two workbooks of the
/// same shape, one whose third string is broken, opened and loaded turn about and then read turn
/// about, and (where there are threads) on eight at once: each reads as it would alone, every time.
#[test]
fn workbooks_open_together_keep_their_own_strings() {
    let rows = [[0, 1, 2], [2, 2, 0], [1, 2, 1]];
    let specs = [
        ColumnSpec::new(0, 18, 0),
        ColumnSpec::new(1, 18, 0),
        ColumnSpec::new(2, 18, 0),
    ];
    let package = |third: &[u8]| {
        let mut table = b"<sst><si><t>one</t></si><si><t>tw\xc3\xb6</t></si><si><t>".to_vec();
        table.extend_from_slice(third);
        table.extend_from_slice(b"</t></si></sst>");
        xlsx_around(&shared_sheet(&rows), &table, true)
    };
    let expected = |third: &[u8]| -> Vec<Vec<String>> {
        let strings: [&[u8]; 3] = [b"one", "tw\u{f6}".as_bytes(), third];
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|&index| judged(strings[index as usize]))
                    .collect()
            })
            .collect()
    };
    let (sound, broken): (&[u8], &[u8]) = (b"three", b"thr\xffe");
    let (sound_package, broken_package) = (package(sound), package(broken));
    let (sound_rows, broken_rows) = (expected(sound), expected(broken));
    assert_ne!(sound_rows, broken_rows);
    let options = SheetOptions::default().with_header(false);
    let cells =
        |read: &Read| -> Vec<Vec<String>> { read.rows.iter().map(|row| row.1.clone()).collect() };

    for sizing in [Sizing::Stingy, Sizing::Roomy] {
        let mut one = Harness::new(&sound_package, sizing);
        let mut two = Harness::new(&broken_package, sizing);
        one.open().unwrap();
        two.open().unwrap();
        let one_sheets = one.sheets().unwrap();
        let two_sheets = two.sheets().unwrap();
        two.load().unwrap();
        one.load().unwrap();
        for _ in 0..3 {
            assert_eq!(
                cells(&one.read(&one_sheets[0], options, &specs, 2)),
                sound_rows
            );
            assert_eq!(
                cells(&two.read(&two_sheets[0], options, &specs, 2)),
                broken_rows
            );
        }
    }

    // wasm32-wasip1 has no threads: the turn-about reads above are the whole of it there.
    #[cfg(not(target_family = "wasm"))]
    std::thread::scope(|scope| {
        for thread in 0..8 {
            let (package, rows) = if thread % 2 == 0 {
                (&sound_package, &sound_rows)
            } else {
                (&broken_package, &broken_rows)
            };
            let specs = &specs;
            scope.spawn(move || {
                for round in 0..40 {
                    let sizing = if round % 2 == 0 {
                        Sizing::Stingy
                    } else {
                        Sizing::Roomy
                    };
                    let mut harness = Harness::new(package, sizing);
                    harness.open().unwrap();
                    let sheets = harness.sheets().unwrap();
                    harness.load().unwrap();
                    let read = harness.read(&sheets[0], options, specs, 2);
                    let cells: Vec<Vec<String>> =
                        read.rows.iter().map(|row| row.1.clone()).collect();
                    assert_eq!(&cells, rows, "thread {thread}, round {round}");
                }
            });
        }
    });
}

#[test]
fn invalid_utf8_in_a_workbook_header_is_a_failure() {
    let specs = [ColumnSpec::new(0, 18, 0)];
    for (format, package) in each_format(&["a", PLACEHOLDER], &["1", "2"], b"\xffokkkkkk") {
        for sizing in [Sizing::Stingy, Sizing::Roomy] {
            let mut harness = Harness::new(&package, sizing);
            harness.open().unwrap();
            let sheets = harness.sheets().unwrap();
            harness.load().unwrap();
            let read = harness.read(&sheets[0], SheetOptions::default(), &specs, 4);
            assert!(read.failed, "{format}, {sizing:?}");
            assert_eq!(
                (read.header, read.rows.len()),
                (None, 0),
                "{format}, {sizing:?}"
            );
            assert_eq!(
                harness.last_failure.code,
                Failure::ENCODING,
                "{format}, {sizing:?}"
            );
            assert_eq!(harness.last_failure.line, 1, "{format}, {sizing:?}");
            // A sheet opened without a header reads the same cell as a verdict.
            let options = SheetOptions::default().with_header(false);
            let two = [ColumnSpec::new(1, 18, 0)];
            let read = harness.read(&sheets[0], options, &two, 4);
            assert!(!read.failed, "{format}, {sizing:?}");
            assert!(
                read.rows[0].1[0].starts_with("fault 2 @0+1"),
                "{format}, {sizing:?}"
            );
        }
    }
    // Judged once the row is whole: a cell a later one overwrites was never the name.
    let package = xlsx_around(
        b"<worksheet><sheetData><row>\
<c r=\"A1\" t=\"inlineStr\"><is><t>a</t></is></c>\
<c r=\"B1\" t=\"inlineStr\"><is><t>\xff</t></is></c><c r=\"B1\"/>\
</row></sheetData></worksheet>",
        b"<sst/>",
        false,
    );
    let mut harness = Harness::new(&package, Sizing::Stingy);
    harness.open().unwrap();
    let sheets = harness.sheets().unwrap();
    harness.load().unwrap();
    let read = harness.read(&sheets[0], SheetOptions::default(), &specs, 4);
    assert!(!read.failed);
    assert_eq!(read.header, Some(vec![b"a".to_vec()]));
    // Through the Rust binding's error.
    let package = each_format(&[PLACEHOLDER], &["1"], b"\xffokkkkkk")
        .remove(0)
        .1;
    let book = hypertabular::Workbook::from_vec(package).unwrap();
    let plan = [hypertabular::Column::text(0)];
    let error = book
        .sheet("S", SheetOptions::default(), &plan)
        .map(|_| ())
        .unwrap_err();
    let hypertabular::Error::Structure(failure) = error else {
        panic!("{error:?}");
    };
    assert_eq!(failure.kind, hypertabular::FailureKind::Encoding);
    assert_eq!(failure.line, 1);
}

/// A headerless sheet's width is its first row's, and no name is asked of that row: a cell in it
/// that isn't text is the verdict a read has for it, in each format.
#[test]
fn suggesting_asks_no_name_of_a_headerless_first_row() {
    use hypertabular::{Column, Door, Error, SuggestOptions, SuggestStatus, Workbook};
    for (format, package) in each_format(&[PLACEHOLDER, "7"], &["ab", "8"], b"\xffokkkkkk") {
        let book = Workbook::from_slice(&package).unwrap();
        let options = SheetOptions::default().with_header(false);
        // A read goes through it, with the verdict.
        let mut read = book.sheet(0, options, &[Column::text(0)]).unwrap();
        assert!(read.read().unwrap().is_some(), "{format}");
        let found = book
            .sheet_unbound(0, options)
            .unwrap()
            .suggest(&SuggestOptions::new())
            .unwrap();
        assert_eq!(found.plan.len(), 2, "{format}");
        assert_eq!(
            found.report[0].status,
            SuggestStatus::Unresolved,
            "{format}"
        );
        assert_eq!(found.report[1].door, Door::I32, "{format}");
        // Declared a header, it's structural, as a read says.
        assert!(
            matches!(
                book.sheet_unbound(0, SheetOptions::default()),
                Err(Error::Structure(_))
            ),
            "{format}"
        );
    }
}
