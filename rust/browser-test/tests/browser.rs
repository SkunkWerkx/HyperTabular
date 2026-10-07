//! HyperTabular inside a real browser: delimited text, an XLSX and an ODS read from bytes
//! the page already holds, a cell's fault, and the C ABI's version export, compiled for
//! wasm32-unknown-unknown. Run with `wasm-pack test --headless --chrome` (or `--firefox`);
//! on any other target this file compiles to nothing.

#![cfg(target_arch = "wasm32")]

use hypertabular::{
    Column, Date, DelimitedReader, Dialect, Format, Reason, SheetOptions, Timestamp, Workbook,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn the_core_reports_the_crate_version() {
    let manifest = include_str!("../../Cargo.toml");
    let want = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("a version line in rust/Cargo.toml");
    // SAFETY: none to uphold; the export takes nothing and reads nothing.
    let v = unsafe { hypertabular::kernel::exports::hypertabular_version() };
    assert_eq!(
        format!("{}.{}.{}", v >> 16, (v >> 8) & 0xFF, v & 0xFF),
        want
    );
}

#[wasm_bindgen_test]
fn delimited_text_in_typed_columns() {
    let text = b"id,name,score,day\n1,alice,2.5,2024-02-29\n2,\"bob, jr\",x,2026-02-29\n";
    let plan = [
        Column::i64(0),
        Column::text(1),
        Column::f64(2),
        Column::date(3),
    ];
    let mut reader = DelimitedReader::from_slice(text, Dialect::CSV, &plan).unwrap();
    let header: Vec<&[u8]> = reader.header().unwrap().names().collect();
    assert_eq!(header, [&b"id"[..], b"name", b"score", b"day"]);
    let batch = reader.read().unwrap().expect("a batch");
    assert_eq!(batch.rows(), 2);
    assert_eq!(batch.i64(0), [1, 2]);
    assert_eq!(batch.text(1, 1), Ok(&b"bob, jr"[..]));
    assert_eq!(batch.get::<f64>(2, 0), Ok(2.5));
    let fault = batch.get::<f64>(2, 1).unwrap_err();
    assert_eq!(fault.reason, Reason::Malformed);
    assert_eq!(&*batch.raw(2, 1), b"x");
    assert_eq!(
        batch.get::<Date>(3, 0),
        Ok(Date {
            year: 2024,
            month: 2,
            day: 29
        })
    );
    assert_eq!(
        batch.get::<Date>(3, 1).map_err(|f| f.reason),
        Err(Reason::OutOfRange)
    );
    assert!(reader.read().unwrap().is_none());
}

#[wasm_bindgen_test]
fn an_xlsx_from_bytes() {
    let book = Workbook::from_slice(include_bytes!("../../../corpus/workbook/basic.xlsx")).unwrap();
    assert_eq!(book.format(), Format::Xlsx);
    assert_eq!(book.sheets()[0].name, "Data");
    let plan = [
        Column::i64(0),
        Column::text(1),
        Column::f64(2),
        Column::timestamp(3),
    ];
    let mut sheet = book.sheet(0, SheetOptions::default(), &plan).unwrap();
    let batch = sheet.read().unwrap().expect("a batch");
    assert_eq!(batch.get::<i64>(0, 0), Ok(1));
    assert_eq!(batch.text(1, 0), Ok(&b"alice"[..]));
    assert_eq!(batch.text(1, 1), Ok(&b"bob, jr"[..]));
    assert_eq!(batch.get::<f64>(2, 0), Ok(2.5));
    assert_eq!(batch.get::<f64>(2, 1), Ok(-7.0));
    // 2024-01-31T10:30:00, an Excel serial read as the wall-clock time it shows.
    assert_eq!(
        batch.get::<Timestamp>(3, 0),
        Ok(Timestamp {
            seconds: 1_706_697_000,
            nanos: 0
        })
    );
}

#[wasm_bindgen_test]
fn an_ods_from_bytes() {
    let book = Workbook::from_slice(include_bytes!("../../../corpus/workbook/basic.ods")).unwrap();
    assert_eq!(book.format(), Format::Ods);
    let names: Vec<&str> = book.sheets().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Data", "Second"]);
    let plan = [Column::text(0)];
    let mut sheet = book
        .sheet("Second", SheetOptions::default(), &plan)
        .unwrap();
    assert_eq!(sheet.header().unwrap().name(0), Some(&b"only"[..]));
    let batch = sheet.read().unwrap().expect("a batch");
    assert_eq!(batch.text(0, 0), Ok(&b"42"[..]));
}

#[wasm_bindgen_test]
fn a_torn_sheet_delivers_the_rows_before_it_then_an_error() {
    assert!(Workbook::from_slice(b"PK\x03\x04 not a zip at all").is_err());
    let book =
        Workbook::from_slice(include_bytes!("../../../corpus/workbook/broken.xlsx")).unwrap();
    let plan = [Column::text(0)];
    let mut sheet = book.sheet(0, SheetOptions::default(), &plan).unwrap();
    let mut texts = Vec::new();
    let error = loop {
        match sheet.read() {
            Ok(Some(batch)) => {
                for row in 0..batch.rows() {
                    texts.push(batch.text(0, row).unwrap().to_vec());
                }
            }
            Ok(None) => panic!("the torn row was read as the end of the sheet"),
            Err(error) => break error,
        }
    };
    assert_eq!(texts, [b" a & b ".to_vec()]);
    assert!(!error.to_string().is_empty());
}
