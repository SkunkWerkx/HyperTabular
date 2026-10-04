//! Conformance against the `csv` crate (BurntSushi's csv-core underneath): on standard
//! RFC 4180 input the two must deliver identical cells, on every SIMD engine this build
//! can run, over a slice and over streams refilled at awkward buffer sizes.
//!
//! csv-core skips blank lines unconditionally, so every comparison runs with blank-line
//! skipping on (our default) — the blank-line-delivering mode is covered by unit tests.
//!
//! "Standard" here means the quoting `csv` and this crate agree on: a cell is either
//! entirely unquoted, or quoted from its first byte to its last with `""` for a literal
//! quote. Both crates are lenient beyond that, in different ways (documented in
//! `src/unescape.rs`), so the generator stays inside the shared subset.

use hypertabular::delimited::engine::{self, Kind};
use hypertabular::delimited::scan::{Output, Scanner};
use hypertabular::delimited::{Dialect, Reader};

fn reference(text: &[u8], dialect: Dialect) -> Vec<Vec<Vec<u8>>> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(dialect.separator)
        .has_headers(false)
        .flexible(true)
        .quoting(dialect.quoting)
        .from_reader(text);
    let mut rows = Vec::new();
    for record in reader.byte_records() {
        let record = record.unwrap();
        rows.push(record.iter().map(<[u8]>::to_vec).collect());
    }
    rows
}

fn ours(text: &[u8], dialect: Dialect, capacity: Option<usize>) -> Vec<Vec<Vec<u8>>> {
    let mut reader = match capacity {
        None => Reader::from_slice(text, dialect).unwrap(),
        Some(capacity) => Reader::from_reader_with_capacity(text, dialect, capacity).unwrap(),
    };
    let mut rows = Vec::new();
    while let Some(row) = reader.next_row().unwrap() {
        rows.push(row.iter().map(<[u8]>::to_vec).collect::<Vec<_>>());
    }
    rows
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A random standard CSV: `rows` rows of `cols` cells, mixing plain cells, quoted cells
/// with embedded separators/quotes/newlines, non-ASCII, and every terminator.
fn generate(seed: u64, rows: usize, cols: usize, sep: u8) -> Vec<u8> {
    let mut rng = XorShift(seed);
    let mut text = Vec::new();
    let alphabet: &[&[u8]] = &[
        b"a",
        b"Z",
        b"0",
        b"9",
        b" ",
        b"\xC3\xA9",
        b"\xE2\x82\xAC",
        b"-",
        b".",
    ];
    for _ in 0..rows {
        for col in 0..cols {
            if col > 0 {
                text.push(sep);
            }
            let quoted = rng.below(4) == 0;
            let len = rng.below(12) as usize;
            if quoted {
                text.push(b'"');
                for _ in 0..len {
                    match rng.below(10) {
                        0 => text.push(sep),
                        1 => text.extend_from_slice(b"\"\""),
                        2 => text.push(b'\n'),
                        3 => text.extend_from_slice(b"\r\n"),
                        _ => text
                            .extend_from_slice(alphabet[rng.below(alphabet.len() as u64) as usize]),
                    }
                }
                text.push(b'"');
            } else {
                for _ in 0..len {
                    text.extend_from_slice(alphabet[rng.below(alphabet.len() as u64) as usize]);
                }
            }
        }
        match rng.below(3) {
            0 => text.push(b'\n'),
            1 => text.extend_from_slice(b"\r\n"),
            _ => text.push(b'\r'),
        }
    }
    text
}

#[test]
fn random_standard_csv_matches_the_csv_crate_on_every_path() {
    for (seed, cols) in [(1u64, 1usize), (2, 3), (3, 7), (4, 12), (5, 2)] {
        for &sep in b",\t|;" {
            let dialect = Dialect::new(sep).with_header(false);
            let text = generate(seed * 7919 + cols as u64, 300, cols, sep);
            let expected = reference(&text, dialect);
            assert!(!expected.is_empty());
            assert_eq!(
                ours(&text, dialect, None),
                expected,
                "slice seed {seed} sep {sep}"
            );
            for capacity in [64, 65, 97, 128, 1000, 4096] {
                assert_eq!(
                    ours(&text, dialect, Some(capacity)),
                    expected,
                    "capacity {capacity} seed {seed} sep {sep}"
                );
            }
        }
    }
}

#[test]
fn every_engine_produces_the_same_structure() {
    let text = generate(42, 500, 5, b',');
    let mut baseline: Option<(Vec<_>, Vec<_>)> = None;
    for kind in engine::available() {
        let scanner = Scanner::with_engine(b',', true, kind);
        let mut out = Output::default();
        let stop = scanner.scan(&text, 0, 1, true, usize::MAX, &mut out);
        assert!(!stop.unclosed);
        let got = (out.cells.clone(), out.rows.clone());
        match &baseline {
            None => baseline = Some(got),
            Some(expected) => assert_eq!(
                &got,
                expected,
                "engine {:?} differs from {:?}",
                kind,
                engine::available()[0]
            ),
        }
    }
    assert!(engine::available().contains(&Kind::Swar));
}

#[test]
fn hand_picked_edge_cases_match_the_csv_crate() {
    let cases: &[&[u8]] = &[
        b"a,b,c\n1,2,3\n",
        b"a,b,c\r\n1,2,3\r\n",
        b"a,b,c\r1,2,3\r",
        b"a,b,c\n1,2,3",
        b"\"a\",\"b\"\n",
        b"\"a,b\",\"c\"\"d\",\"e\nf\"\n",
        b"\"\",\"\",\"\"\n",
        b",,\n",
        b"x\n",
        b"\"\xE2\x82\xAC\",\xC3\xA9\n",
        b"a,b\n\n\nc,d\n",
    ];
    for text in cases {
        let dialect = Dialect::CSV.with_header(false);
        let expected = reference(text, dialect);
        assert_eq!(
            ours(text, dialect, None),
            expected,
            "{:?}",
            String::from_utf8_lossy(text)
        );
        assert_eq!(
            ours(text, dialect, Some(64)),
            expected,
            "{:?}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn blank_line_skipping_matches_csv_core_by_default() {
    // csv-core skips blank lines at record start; so do we, by default.
    let text = b"a,b\n\n\n1,2\n\n";
    let expected = {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(&text[..]);
        reader
            .byte_records()
            .map(|r| r.unwrap().iter().map(<[u8]>::to_vec).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    assert_eq!(ours(text, Dialect::CSV.with_header(false), None), expected);
}
