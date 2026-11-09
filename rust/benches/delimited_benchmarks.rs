//! Throughput receipts. Three scopes on one generated file, HyperCast-style honest
//! comparison against the `csv` crate (the Rust reference; Sep is C#-side, measured from
//! the C# binding):
//!
//! - `structure`: the core alone, an empty plan — every row found, no cell cast — on
//!   every engine this CPU offers, so the SIMD tiers are visible next to each other.
//!   `structure_all_quoted` and `structure_stray_quote` run the same scope on two files that
//!   make the walker work hardest: every cell quoted, and a stray quote in every row.
//! - `cells`: every cell's bytes touched (Sep's "Cols" scope), through the text door, whose
//!   UTF-8 validation is part of the cost. `cells/hypertabular-utf8` does it on a file of the
//!   same shape whose every cell is multi-byte text, where validation has no ASCII fast path.
//! - `batch`: every cell cast through a HyperCast door into typed columns — the payoff.
//!
//! The file: 200 000 rows × 10 columns, ~24 MB, one quoted column with embedded
//! separators, one with `""` escapes on every tenth row, integers, reals, an RFC 3339
//! timestamp, a UUID, a boolean, and a date.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypertabular::kernel::abi::{Filled, OK, Span};
// Compile the same engine selector locally for labels and per-engine comparisons; it
// is not part of the library's supported API.
#[allow(dead_code, unused_imports)]
#[path = "../src/kernel/delimited/engine.rs"]
mod engine;
use engine::Kind;
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::{Column, DelimitedReader, Dialect};

const ROWS: usize = 200_000;

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

/// The same shape with every cell quoted, so every cell opens and closes a quote.
fn generate_all_quoted(rows: usize) -> Vec<u8> {
    let mut text = b"id,name,city,amount,ratio,when,uuid,active,born,note\n".to_vec();
    for i in 0..rows {
        let note = if i % 10 == 0 {
            "said \"\"hi\"\" then left"
        } else {
            "no comment"
        };
        text.extend_from_slice(
            format!(
                "\"{i}\",\"Doe, J{}\",\"Springfield\",\"{}.{:02}\",\"0.{:03}\",\"2026-01-{:02}T{:02}:04:05Z\",\"01020304-0506-0708-090a-0b0c0d0e{:04x}\",\"{}\",\"19{:02}-0{}-1{}\",\"{note}\"\n",
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

/// The same shape with stray quotes in every row: an inch mark in a cell that doesn't start
/// with a quote, and a cell that goes on after its closing quote.
fn generate_stray_quote(rows: usize) -> Vec<u8> {
    let mut text = b"id,name,city,amount,ratio,when,uuid,active,born,note\n".to_vec();
    for i in 0..rows {
        text.extend_from_slice(
            format!(
                "{i},{}\" pipe,\"Spring\"field\",{}.{:02},0.{:03},2026-01-{:02}T{:02}:04:05Z,01020304-0506-0708-090a-0b0c0d0e{:04x},{},19{:02}-0{}-1{},no comment\n",
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

/// Ten columns of text, every cell of it multi-byte: Latin with accents, Greek, Cyrillic, CJK
/// and a little emoji, as much of a mix as a real file's would be and more.
fn generate_utf8(rows: usize) -> Vec<u8> {
    let mut text = "番号,名前,都市,数量,比率,時刻,識別,状態,誕生,備考\n"
        .as_bytes()
        .to_vec();
    for i in 0..rows {
        let note = if i % 10 == 0 {
            "\"dit \"\"salut\"\" é puis parti\""
        } else {
            "pas de commentaire à faire"
        };
        text.extend_from_slice(
            format!(
                "№{i},\"Müller, Jürgen {}\",São Paulo,€{}.{:02},Ω{:03},二〇二六年一月{}日,Идентификатор{:04x},{},Καλημέρα {},{note}\n",
                i % 1000,
                i % 100_000,
                i % 100,
                i % 1000,
                i % 28 + 1,
                i % 0xFFFF,
                if i % 3 == 0 { "はい😀" } else { "いいえ😐" },
                i % 100,
            )
            .as_bytes(),
        );
    }
    text
}

/// `generate`'s file with one byte that isn't UTF-8 in every 1024 rows: every 1024th row has
/// `Springf\xFFeld` for its city, so every batch holds at least one. Every batch's region then
/// fails validation and falls to judging cell by cell, which is the most text validation can cost
/// a read.
fn generate_bad_per_batch(rows: usize) -> Vec<u8> {
    let text = generate(rows);
    let mut out = Vec::with_capacity(text.len());
    for (line, record) in text.split_inclusive(|&b| b == b'\n').enumerate() {
        // Line 0 is the header.
        if line > 0 && (line - 1) % 1024 == 1023 {
            let at = record
                .windows(11)
                .position(|w| w == b"Springfield")
                .unwrap();
            out.extend_from_slice(&record[..at]);
            out.extend_from_slice(b"Springf\xFFeld");
            out.extend_from_slice(&record[at + 11..]);
        } else {
            out.extend_from_slice(record);
        }
    }
    out
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

/// The `structure` scope: every row of `text` found, on every engine.
fn structure(c: &mut Criterion, name: &str, text: &[u8]) {
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Bytes(text.len() as u64));
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
                        assert_eq!(rows, ROWS as u64 + 1);
                        return rows;
                    }
                    rows += out.rows;
                    at += out.consumed as usize;
                }
            });
        });
    }
    group.finish();
}

fn benchmarks(c: &mut Criterion) {
    let text = generate(ROWS);
    let bytes = text.len() as u64;

    structure(c, "structure", &text);
    structure(c, "structure_all_quoted", &generate_all_quoted(ROWS));
    structure(c, "structure_stray_quote", &generate_stray_quote(ROWS));

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
    let multibyte = generate_utf8(ROWS);
    group.throughput(Throughput::Bytes(multibyte.len() as u64));
    group.bench_function("hypertabular-utf8", |b| {
        b.iter(|| {
            let mut reader = DelimitedReader::from_slice(&multibyte, Dialect::CSV, &every).unwrap();
            let mut total = 0usize;
            while let Some(batch) = reader.read().unwrap() {
                for column in 0..10 {
                    for row in 0..batch.rows() {
                        total += batch.text(column, row).map_or(0, <[u8]>::len);
                    }
                }
            }
            assert!(total > 0);
            total
        });
    });
    // The worst case for validation: every batch's region refused, every cell judged alone.
    let bad = generate_bad_per_batch(ROWS);
    {
        let mut reader = DelimitedReader::from_slice(&bad, Dialect::CSV, &every).unwrap();
        while let Some(batch) = reader.read().unwrap() {
            assert!((0..batch.rows()).any(|row| !batch.verdicts(2)[row].is_ok()));
        }
    }
    group.throughput(Throughput::Bytes(bad.len() as u64));
    group.bench_function("hypertabular-bad-per-batch", |b| {
        b.iter(|| {
            let mut reader = DelimitedReader::from_slice(&bad, Dialect::CSV, &every).unwrap();
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
    // Four cells in five fail their door, so each has a note to write: the worst case for notes,
    // where `batch` is the best.
    let mut faulty = Vec::from(&b"a,b,c,d,e\n"[..]);
    for row in 0..100_000u32 {
        faulty.extend_from_slice(format!("{row}x,y{row},,{row}z,\"q{row}\"\n").as_bytes());
    }
    let faulty_plan = [
        Column::i32(0),
        Column::i32(1),
        Column::i32(2),
        Column::f64(3),
        Column::text(4),
    ];
    let mut group = c.benchmark_group("faulty");
    group.throughput(Throughput::Bytes(faulty.len() as u64));
    group.bench_function("hypertabular", |b| {
        b.iter(|| {
            let mut reader =
                DelimitedReader::from_slice(&faulty, Dialect::CSV, &faulty_plan).unwrap();
            let mut rows = 0usize;
            while let Some(batch) = reader.read().unwrap() {
                rows += batch.rows();
            }
            rows
        });
    });
    group.finish();

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
