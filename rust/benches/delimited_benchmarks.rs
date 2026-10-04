//! Throughput receipts. Three scopes on one generated file, HyperCast-style honest
//! comparison against the `csv` crate (the Rust reference; Sep is C#-side, measured from
//! the C# binding):
//!
//! - `structure`: the core alone, an empty plan — every row found, no cell cast — on
//!   every engine this CPU offers, so the SIMD tiers are visible next to each other.
//! - `cells`: every cell's bytes touched (Sep's "Cols" scope), through the text door.
//! - `batch`: every cell cast through a HyperCast door into typed columns — the payoff.
//!
//! The file: 200 000 rows × 10 columns, ~24 MB, one quoted column with embedded
//! separators, one with `""` escapes on every tenth row, integers, reals, an RFC 3339
//! timestamp, a UUID, a boolean, and a date.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::kernel::abi::{Filled, OK, Span};
use hypertabular::kernel::delimited::engine::{self, Kind};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::{Column, DelimitedReader, Dialect};

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

/// The engines this build can run, best first.
fn engines() -> Vec<Kind> {
    #[cfg(target_arch = "x86_64")]
    let kinds = [Kind::Avx2Clmul, Kind::Sse2Clmul, Kind::Sse2, Kind::Swar];
    #[cfg(target_arch = "aarch64")]
    let kinds = [Kind::NeonPmull, Kind::Neon, Kind::Swar];
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let kinds = [Kind::Swar];
    kinds
        .into_iter()
        .filter(|&kind| engine::usable(kind))
        .collect()
}

fn benchmarks(c: &mut Criterion) {
    let text = generate(200_000);
    let bytes = text.len() as u64;

    let mut group = c.benchmark_group("structure");
    group.throughput(Throughput::Bytes(bytes));
    for kind in engines() {
        group.bench_with_input(BenchmarkId::new("core", kind.name()), &kind, |b, &kind| {
            let dialect = RawDialect {
                separator: b',',
                quoting: 1,
                skip_blank_lines: 1,
                engine: kind.code(),
            };
            // No columns: a row takes one entry of the cell table, and nothing is cast.
            let mut cells = vec![Span::default(); 1024];
            b.iter(|| {
                let mut state = State::init(dialect).unwrap();
                let (mut at, mut rows) = (0usize, 0u64);
                loop {
                    let mut out = Filled::default();
                    // SAFETY: there are no column buffers to have room in.
                    let code = unsafe {
                        fill::fill(
                            &mut state,
                            &text[at..],
                            true,
                            &[],
                            &[],
                            1024,
                            &mut cells,
                            &mut [],
                            &mut out,
                        )
                    };
                    assert_eq!(code, OK);
                    if out.rows == 0 {
                        return rows;
                    }
                    rows += out.rows;
                    at += out.consumed as usize;
                }
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("cells");
    group.throughput(Throughput::Bytes(bytes));
    let every: Vec<Column> = (0..10).map(Column::text).collect();
    group.bench_function("hypertabular", |b| {
        b.iter(|| {
            let mut reader = DelimitedReader::from_slice(&text, Dialect::CSV, &every).unwrap();
            let mut total = 0usize;
            while let Some(batch) = reader.read().unwrap() {
                for column in 0..10 {
                    for row in 0..batch.rows() {
                        total += batch.text(column, row).map_or(0, <[u8]>::len);
                    }
                }
            }
            total
        });
    });
    group.bench_function("csv", |b| {
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

    let plan = [
        Column::i64(0),
        Column::text(1),
        Column::text(2),
        Column::f64(3),
        Column::f32(4),
        Column::timestamp(5),
        Column::uuid(6),
        Column::bool(7),
        Column::date(8),
        Column::text(9),
    ];
    let mut group = c.benchmark_group("batch");
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function("typed/hypertabular", |b| {
        b.iter(|| {
            let mut reader = DelimitedReader::from_slice(&text, Dialect::CSV, &plan).unwrap();
            let mut rows = 0usize;
            while let Some(batch) = reader.read().unwrap() {
                rows += batch.rows();
            }
            rows
        });
    });
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
