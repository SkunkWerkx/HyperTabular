//! Throughput receipts for the workbook reader: the three real-application files of
//! `corpus/README.md` — Excel's `excel-win-300k.xlsx`, LibreOffice's `libreoffice-300k.ods`
//! and Numbers' `numbers-300k.numbers`, 300 000 rows × 8 columns each — read whole through
//! the plan every binding's workbook benchmark reads them through, so that one number per
//! binding sits beside this one:
//!
//! - `open`: the package opened — the directory, the workbook part, the shared strings and
//!   the styles — and nothing read.
//! - `read`: opened, then the first sheet read in batches of 4096, every column's verdicts
//!   looked at and every text cell's bytes, the checksum each binding prints.
//!
//! The files stay out of the tree (`corpus/generate/out/`, sha256 in the README's table);
//! `HYPERTABULAR_BENCH_DIR` names another directory holding them. A file that is not there
//! is skipped, said so, rather than failed.
//!
//! Those files hold one column of text in eight. `text_heavy` is a generated XLSX whose every
//! cell is a shared string through the text door, half of them multi-byte: what validating
//! text as UTF-8 costs a workbook at the most it can.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::{Column, SheetOptions, Workbook};
use std::path::PathBuf;

#[path = "../tests/support/packages.rs"]
mod packages;

/// The plan: the README's eight columns, each through the door its cells are for.
const PLAN: [Column; 8] = [
    Column::i64(0),
    Column::f64(1),
    Column::text(2),
    Column::date(3),
    Column::time(4),
    Column::bool(5),
    Column::duration(6),
    Column::i64(7),
];

/// Every cell that cast, plus every text cell's length in bytes: the number each binding's
/// benchmark arrives at too, which is what says they all did the same work.
fn read(book: &Workbook) -> usize {
    let mut sheet = book
        .sheet(0, SheetOptions::new().with_batch_rows(4096), &PLAN)
        .unwrap();
    let mut checksum = 0;
    while let Some(batch) = sheet.read().unwrap() {
        for column in 0..PLAN.len() {
            checksum += batch.verdicts(column).iter().filter(|v| v.is_ok()).count();
        }
        for row in 0..batch.rows() {
            checksum += batch.text(2, row).map_or(0, <[u8]>::len);
        }
    }
    checksum
}

/// 100 000 rows of eight shared strings, from a table of 2 000 with a thousand of them
/// multi-byte, deflated.
fn text_heavy() -> Vec<u8> {
    let mut strings = String::new();
    for index in 0..2000 {
        if index % 2 == 0 {
            strings.push_str(&format!("<si><t>plain text number {index}</t></si>"));
        } else {
            strings.push_str(&format!("<si><t>Müller 東京 Καλημέρα {index}</t></si>"));
        }
    }
    let mut sheet = String::new();
    for row in 0..100_000u32 {
        sheet.push_str("<row>");
        for column in 0..8u32 {
            let index = (row.wrapping_mul(31) + column * 7) % 2000;
            sheet.push_str(&format!("<c t=\"s\"><v>{index}</v></c>"));
        }
        sheet.push_str("</row>");
    }
    packages::xlsx(&[("Text", &sheet)], &strings, false, true)
}

fn text_heavy_read(c: &mut Criterion) {
    let container = text_heavy();
    let plan: Vec<Column> = (0..8).map(Column::text).collect();
    let book = Workbook::from_slice(&container).unwrap();
    let mut group = c.benchmark_group("text_heavy");
    group.sample_size(10);
    group.bench_function("read", |b| {
        b.iter(|| {
            let options = SheetOptions::new().with_header(false).with_batch_rows(4096);
            let mut sheet = book.sheet(0, options, &plan).unwrap();
            let mut total = 0;
            while let Some(batch) = sheet.read().unwrap() {
                for column in 0..8 {
                    for row in 0..batch.rows() {
                        total += batch.text(column, row).map_or(0, <[u8]>::len);
                    }
                }
            }
            assert!(total > 0);
            total
        });
    });
    group.finish();
}

fn benchmarks(c: &mut Criterion) {
    let directory = std::env::var_os("HYPERTABULAR_BENCH_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corpus/generate/out"),
        PathBuf::from,
    );
    for name in [
        "excel-win-300k.xlsx",
        "libreoffice-300k.ods",
        "numbers-300k.numbers",
    ] {
        let Ok(container) = std::fs::read(directory.join(name)) else {
            eprintln!("{name}: not in {}, skipped", directory.display());
            continue;
        };
        let book = Workbook::from_slice(&container).unwrap();
        eprintln!("{name}: checksum {}", read(&book));

        let mut group = c.benchmark_group(name);
        group.sample_size(10);
        group.throughput(Throughput::Bytes(container.len() as u64));
        group.bench_function("open", |b| {
            b.iter(|| Workbook::from_slice(&container).unwrap().sheets().len());
        });
        group.bench_function("read", |b| {
            b.iter(|| read(&Workbook::from_slice(&container).unwrap()));
        });
        group.finish();
    }
}

criterion_group!(benches, text_heavy_read, benchmarks);
criterion_main!(benches);
