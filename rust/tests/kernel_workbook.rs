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

#[path = "support/packages.rs"]
mod packages;
#[path = "support/workbook.rs"]
mod support;

use hypertabular::kernel::abi::{ColumnSpec, Failure};
use hypertabular::kernel::workbook::number::{Text, parse_f64};
use hypertabular::kernel::workbook::part::WINDOW_MIN;
use hypertabular::workbook::SheetOptions;
use packages::{Rng, build, ods, ods_generated, ods_table, xlsx, xlsx_generated, xlsx_sheet};
use std::path::PathBuf;
use support::{Harness, Sizing, compare, compare_plan, every_door};

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
        if !matches!(extension, "xlsx" | "ods") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        for options in options() {
            // A real ODS sheet ends in a million empty rows written as one element. Read
            // with empty rows delivered, the comparison would hold every one of them twice,
            // in ten columns: gigabytes. `a_million_trailing_rows` covers that reading with
            // one column.
            if path.ends_with("basic.ods") && !options.skip_empty_rows {
                continue;
            }
            let (_, rows) = compare(&bytes, options, 10, 3);
            assert!(rows > 0, "{} has rows", path.display());
        }
        seen += 1;
    }
    assert!(seen >= 3, "{seen} fixtures read");
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

/// Finds the central-directory record of `name` and overwrites one of its fields.
fn patch_directory(package: &mut [u8], name: &str, field: usize, value: u32) {
    let needle = name.as_bytes();
    let at = (0..package.len() - needle.len())
        .rev()
        .find(|&at| {
            &package[at..at + needle.len()] == needle
                && at >= 46
                && package[at - 46..at - 42] == [0x50, 0x4B, 0x01, 0x02]
        })
        .expect("the entry is in the directory");
    package[at - 46 + field..at - 46 + field + 4].copy_from_slice(&value.to_le_bytes());
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
    // one who decides whether to give it.
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

/// The core reads a number as Rust does, where what Rust reads is finite.
#[test]
fn numbers_are_read_as_rust_reads_them() {
    let expected = |text: &str| text.parse::<f64>().ok().filter(|value| value.is_finite());
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
/// `cargo test --release --test kernel_workbook throughput -- --ignored --nocapture`
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
