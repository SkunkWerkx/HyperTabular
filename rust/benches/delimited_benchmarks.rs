//! Throughput receipts. Three scopes on one generated file, HyperCast-style honest
//! comparison against the `csv` crate (the Rust reference; Sep is C#-side, measured from
//! the C# binding when it exists):
//!
//! - `rows`: structure only — every row delivered, no cell touched (Sep's "Row" scope).
//! - `cells`: every cell's bytes touched (Sep's "Cols" scope).
//! - `batch`: every cell cast through a HyperCast door into a typed batch — the payoff.
//!
//! The file: 200 000 rows × 10 columns, ~24 MB, one quoted column with embedded
//! separators, one with `""` escapes on every tenth row, integers, reals, an RFC 3339
//! timestamp, a UUID, a boolean, and a date. Also, the scanner alone on every engine
//! this CPU offers, so the SIMD tiers are visible next to each other.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::delimited::engine;
use hypertabular::delimited::scan::{Output, Scanner};
use hypertabular::delimited::{Dialect, Reader};
use hypertabular::{Batch, Column, Door, Plan, fill_batch};

fn generate(rows: usize) -> Vec<u8> {
    let mut text = b"id,name,city,amount,ratio,when,uuid,active,born,note\n".to_vec();
    for i in 0..rows {
        let note = if i % 10 == 0 {
            "\"said \"\"hi\"\" then left\""
        } else {
            "no comment"
        };
        text.extend_from_slice(
            format!(
                "{i},\"Doe, J{}\",Springfield,{}.{:02},0.{:03},2026-01-{:02}T{:02}:04:05Z,01020304-0506-0708-090a-0b0c0d0e{:04x},{},19{:02}-0{}-1{},{note}\n",
                i % 1000,
                i % 100_000,
                i % 100,
                i % 1000,
                i % 28 + 1,
                i % 24,
                i % 0xFFFF,
                if i % 3 == 0 { "yes" } else { "no" },
                i % 100,
                i % 9 + 1,
                i % 9
            )
            .as_bytes(),
        );
    }
    text
}

fn benchmarks(c: &mut Criterion) {
    let text = generate(200_000);
    let bytes = text.len() as u64;

    let mut group = c.benchmark_group("scan");
    group.throughput(Throughput::Bytes(bytes));
    for kind in engine::available() {
        group.bench_with_input(
            BenchmarkId::new("structure", kind.name()),
            &kind,
            |b, &kind| {
                let scanner = Scanner::with_engine(b',', true, kind);
                let mut out = Output::default();
                b.iter(|| {
                    let mut pos = 0;
                    let mut line = 1;
                    loop {
                        out.clear();
                        let stop = scanner.scan(&text, pos, line, true, 1024, &mut out);
                        pos = stop.next;
                        line = stop.line;
                        if out.rows.is_empty() {
                            break;
                        }
                    }
                    pos
                });
            },
        );
    }
    group.finish();

    let mut group = c.benchmark_group("reader");
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function("rows/hyperdelimited", |b| {
        b.iter(|| {
            let mut reader = Reader::from_slice(&text, Dialect::CSV).unwrap();
            let mut rows = 0u64;
            while reader.next_row().unwrap().is_some() {
                rows += 1;
            }
            rows
        });
    });
    group.bench_function("rows/csv", |b| {
        b.iter(|| {
            let mut reader = csv::ReaderBuilder::new().from_reader(&text[..]);
            let mut record = csv::ByteRecord::new();
            let mut rows = 0u64;
            while reader.read_byte_record(&mut record).unwrap() {
                rows += 1;
            }
            rows
        });
    });
    group.bench_function("cells/hyperdelimited", |b| {
        b.iter(|| {
            let mut reader = Reader::from_slice(&text, Dialect::CSV).unwrap();
            let mut total = 0usize;
            while let Some(row) = reader.next_row().unwrap() {
                for cell in row.iter() {
                    total += cell.len();
                }
            }
            total
        });
    });
    group.bench_function("cells/csv", |b| {
        b.iter(|| {
            let mut reader = csv::ReaderBuilder::new().from_reader(&text[..]);
            let mut record = csv::ByteRecord::new();
            let mut total = 0usize;
            while reader.read_byte_record(&mut record).unwrap() {
                for cell in record.iter() {
                    total += cell.len();
                }
            }
            total
        });
    });
    group.finish();

    let plan = Plan::new(vec![
        Column::new(0, Door::I64),
        Column::new(1, Door::Text),
        Column::new(2, Door::Text),
        Column::new(3, Door::F64),
        Column::new(4, Door::F32),
        Column::new(5, Door::Timestamp),
        Column::new(6, Door::Uuid),
        Column::new(7, Door::Bool),
        Column::new(8, Door::Date),
        Column::new(9, Door::Text),
    ]);
    let mut group = c.benchmark_group("batch");
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function("typed/hyperdelimited", |b| {
        let mut batch = Batch::new();
        b.iter(|| {
            let mut reader = Reader::from_slice(&text, Dialect::CSV).unwrap();
            let mut rows = 0usize;
            while fill_batch(&mut reader, &plan, &mut batch, 1024).unwrap() > 0 {
                rows += batch.rows();
            }
            rows
        });
    });
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
