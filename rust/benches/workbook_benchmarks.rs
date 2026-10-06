//! Throughput receipts for the workbook reader: the two real-application files of
//! `corpus/README.md` — Excel's `excel-win-300k.xlsx` and LibreOffice's
//! `libreoffice-300k.ods`, 300 000 rows × 8 columns each — read whole through the plan
//! every binding's workbook benchmark reads them through, so that one number per binding
//! sits beside this one:
//!
//! - `open`: the package opened — the directory, the workbook part, the shared strings and
//!   the styles — and nothing read.
//! - `read`: opened, then the first sheet read in batches of 4096, every column's verdicts
//!   looked at and every text cell's bytes, the checksum each binding prints.
//!
//! The files stay out of the tree (`corpus/generate/out/`, sha256 in the README's table);
//! `HYPERTABULAR_BENCH_DIR` names another directory holding them. A file that is not there
//! is skipped, said so, rather than failed.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::{Column, SheetOptions, Workbook};
use std::path::PathBuf;

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

fn benchmarks(c: &mut Criterion) {
    let directory = std::env::var_os("HYPERTABULAR_BENCH_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corpus/generate/out"),
        PathBuf::from,
    );
    for name in ["excel-win-300k.xlsx", "libreoffice-300k.ods"] {
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

criterion_group!(benches, benchmarks);
criterion_main!(benches);
