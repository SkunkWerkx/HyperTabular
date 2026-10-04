//! The delimited reader as a caller meets it.
//!
//! First, conformance against the `csv` crate (BurntSushi's csv-core underneath): on
//! standard RFC 4180 input the two must deliver identical cells, over a slice and over
//! streams refilled through awkward buffer sizes, in batches small and large. csv-core
//! skips blank lines unconditionally, so every comparison runs with blank-line skipping
//! on, which is this crate's default. "Standard" means the quoting both agree on: a cell
//! is either entirely unquoted, or quoted from its first byte to its last with `""` for a
//! literal quote.
//!
//! Then the API's own promises: what a batch lends, what is an error, and what is the
//! caller's bug.

use hypertabular::{
    Column, CurrencySymbol, DelimitedReader, Dialect, Error, FailureKind, NumFormat, Reason,
    Timestamp,
};
use std::borrow::Cow;

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

/// Every cell of `text` through the text door, `cols` to a row.
fn ours(
    text: &[u8],
    dialect: Dialect,
    cols: usize,
    batch_rows: usize,
    buffer_bytes: Option<usize>,
) -> Vec<Vec<Vec<u8>>> {
    let plan: Vec<Column> = (0..cols).map(Column::text).collect();
    let options = DelimitedReader::options().batch_rows(batch_rows);
    let mut reader = match buffer_bytes {
        None => options.from_slice(text, dialect, &plan).unwrap(),
        Some(bytes) => options
            .buffer_bytes(bytes)
            .from_reader(text, dialect, &plan)
            .unwrap(),
    };
    let mut rows = Vec::new();
    while let Some(batch) = reader.read().unwrap() {
        for row in 0..batch.rows() {
            rows.push(
                (0..cols)
                    .map(|column| match batch.text(column, row) {
                        Ok(text) => text.to_vec(),
                        // No bytes at all is the text door's one fault.
                        Err(fault) => {
                            assert_eq!(fault.reason, Reason::Empty);
                            Vec::new()
                        }
                    })
                    .collect(),
            );
        }
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
            // A lone empty cell on a line is a blank line, which both readers skip.
            let len = rng.below(12) as usize + usize::from(cols == 1);
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
            assert_eq!(expected.len(), 300);
            for batch_rows in [1, 7, 4096] {
                assert_eq!(
                    ours(&text, dialect, cols, batch_rows, None),
                    expected,
                    "slice seed {seed} sep {sep} batch {batch_rows}"
                );
                for buffer_bytes in [1, 64, 65, 97, 1000, 4096] {
                    assert_eq!(
                        ours(&text, dialect, cols, batch_rows, Some(buffer_bytes)),
                        expected,
                        "buffer {buffer_bytes} seed {seed} sep {sep} batch {batch_rows}"
                    );
                }
            }
        }
    }
}

#[test]
fn hand_picked_edge_cases_match_the_csv_crate() {
    let cases: &[(&[u8], usize)] = &[
        (b"a,b,c\n1,2,3\n", 3),
        (b"a,b,c\r\n1,2,3\r\n", 3),
        (b"a,b,c\r1,2,3\r", 3),
        (b"a,b,c\n1,2,3", 3),
        (b"\"a\",\"b\"\n", 2),
        (b"\"a,b\",\"c\"\"d\",\"e\nf\"\n", 3),
        (b"\"\",\"\",\"\"\n", 3),
        (b",,\n", 3),
        (b"x\n", 1),
        (b"\"\xE2\x82\xAC\",\xC3\xA9\n", 2),
        (b"a,b\n\n\nc,d\n", 2),
        (b"a,b\n\n\n1,2\n\n", 2),
    ];
    for &(text, cols) in cases {
        let dialect = Dialect::CSV.with_header(false);
        let expected = reference(text, dialect);
        for buffer_bytes in [None, Some(1), Some(64)] {
            assert_eq!(
                ours(text, dialect, cols, 2, buffer_bytes),
                expected,
                "{:?} through {buffer_bytes:?}",
                String::from_utf8_lossy(text)
            );
        }
    }
}

const TEXT: &[u8] = b"id,amount,when,note\n\
1,\"1.234,50\",2024-01-31T10:30:00Z,\"said \"\"hi\"\"\"\n\
x2,7,not a time,\n\
3,\"2,5\",2024-02-01T00:00:00Z,plain\n";

#[test]
fn a_batch_is_columns_verdicts_and_text_lent_by_the_reader() {
    let continental = NumFormat {
        decimal_sep: ',',
        group_sep: '.',
        flags: NumFormat::ALL,
        currency: CurrencySymbol::NONE,
    };
    let plan = [
        Column::i64(0),
        Column::f64(1).format(continental),
        Column::timestamp(2),
        Column::text(3),
        // One source column may be read through several doors.
        Column::text(0),
    ];
    let mut reader = DelimitedReader::from_slice(TEXT, Dialect::CSV, &plan).unwrap();
    let header = reader.header().unwrap();
    assert_eq!(header.len(), 4);
    assert_eq!(header.ordinal(b"when"), Some(2));
    assert_eq!(reader.plan(), &plan);

    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.rows(), 3);
    assert_eq!(batch.columns(), &plan);
    // Whole columns, as the core wrote them: the door's zero where a cell did not cast.
    assert_eq!(batch.i64(0), &[1, 0, 3]);
    assert_eq!(batch.f64(1), &[1234.5, 7.0, 2.5]);
    let reasons: Vec<u32> = batch.verdicts(0).iter().map(|v| v.reason).collect();
    assert_eq!(reasons, [0, 2, 0]);
    // One cell at a time, as HyperCast judged it.
    assert_eq!(batch.get::<i64>(0, 0), Ok(1));
    let fault = batch.get::<i64>(0, 1).unwrap_err();
    assert_eq!(
        (fault.reason, fault.offset, fault.len),
        (Reason::Malformed, 0, 1)
    );
    assert_eq!(
        batch.get::<Timestamp>(2, 0),
        Ok(Timestamp {
            seconds: 1_706_697_000,
            nanos: 0
        })
    );
    assert_eq!(
        batch.get::<Timestamp>(2, 1).unwrap_err().reason,
        Reason::Malformed
    );
    // Text is the input itself, unless a doubled quote had to be taken out of it.
    assert_eq!(batch.text(3, 0), Ok(&b"said \"hi\""[..]));
    assert_eq!(batch.text(3, 1).unwrap_err().reason, Reason::Empty);
    let plain = batch.text(3, 2).unwrap();
    assert_eq!(plain, b"plain");
    assert!(
        TEXT.as_ptr_range().contains(&plain.as_ptr()),
        "borrowed from the input"
    );
    // The raw text of any cell, whatever its door and its verdict.
    assert_eq!(&*batch.raw(0, 1), b"x2");
    assert_eq!(&*batch.raw(2, 1), b"not a time");
    assert!(matches!(batch.raw(1, 0), Cow::Borrowed(b"1.234,50")));
    assert!(matches!(batch.raw(3, 0), Cow::Owned(ref text) if text == b"said \"hi\""));
    assert_eq!(batch.text(4, 1), Ok(&b"x2"[..]));
    // Where each row came from.
    assert_eq!([batch.line(0), batch.line(1), batch.line(2)], [2, 3, 4]);

    assert!(reader.read().unwrap().is_none());
    assert!(reader.read().unwrap().is_none(), "the end stays the end");
    assert_eq!(reader.records(), 4);
    assert_eq!(reader.column_count(), Some(4));
}

#[test]
fn what_the_caller_declared_wrong_is_an_error_before_any_read() {
    let plan = [Column::text(0)];
    for separator in [b'"', b'\n', b'\r', 0, 0x80] {
        let dialect = Dialect::new(separator);
        assert!(matches!(
            DelimitedReader::from_slice(b"a\n", dialect, &plan),
            Err(Error::Separator(found)) if found == separator
        ));
    }
    let same = NumFormat {
        decimal_sep: ',',
        group_sep: ',',
        ..NumFormat::INVARIANT
    };
    let none = NumFormat {
        decimal_sep: '\0',
        ..NumFormat::INVARIANT
    };
    for format in [same, none] {
        let plan = [Column::text(0), Column::f64(1).format(format)];
        assert!(matches!(
            DelimitedReader::from_slice(b"a,b\n", Dialect::CSV, &plan),
            Err(Error::Plan { column: 1 })
        ));
    }
    assert!(matches!(
        DelimitedReader::open("/no/such/file.csv", Dialect::CSV, &plan),
        Err(Error::Io(_))
    ));
}

#[test]
fn a_structural_failure_comes_after_the_rows_before_it_and_stays() {
    let plan = [Column::i32(0), Column::i32(1)];
    let text = b"a,b\n1,2\n3,4\n5\n7,8\n";
    for batch_rows in [1, 2, 100] {
        let options = DelimitedReader::options().batch_rows(batch_rows);
        let mut reader = options.from_slice(text, Dialect::CSV, &plan).unwrap();
        let mut seen = Vec::new();
        let error = loop {
            match reader.read() {
                Ok(Some(batch)) => seen.extend_from_slice(batch.i32(0)),
                Ok(None) => panic!("the input is broken"),
                Err(error) => break error,
            }
        };
        assert_eq!(seen, [1, 3]);
        let Error::Structure(failure) = error else {
            panic!("{error}");
        };
        assert_eq!(failure.kind, FailureKind::ColumnCount);
        // The fourth record, the header counted: index 3.
        assert_eq!((failure.record, failure.line, failure.byte), (3, 4, 12));
        assert_eq!((failure.expected, failure.found), (2, 1));
        assert!(matches!(reader.read(), Err(Error::Structure(again)) if again == failure));
    }
    let mut reader = DelimitedReader::from_slice(b"a\n\"open", Dialect::CSV, &plan[..1]).unwrap();
    assert!(matches!(
        reader.read(),
        Err(Error::Structure(failure)) if failure.kind == FailureKind::UnclosedQuote
    ));
}

#[test]
fn an_input_with_no_record_has_an_empty_header_and_no_batch() {
    let plan = [Column::text(0)];
    for text in [&b""[..], b"\n\n", b"\xEF\xBB\xBF"] {
        let mut reader = DelimitedReader::from_slice(text, Dialect::CSV, &plan).unwrap();
        assert!(reader.header().unwrap().is_empty());
        assert!(reader.read().unwrap().is_none());
    }
    let mut reader =
        DelimitedReader::from_slice(b"", Dialect::CSV.with_header(false), &plan).unwrap();
    assert!(reader.header().is_none());
    assert!(reader.read().unwrap().is_none());
}

#[test]
#[should_panic(expected = "column 0 is read through the I64 door, which does not make a f64")]
fn asking_a_column_for_another_doors_type_is_the_callers_bug() {
    let plan = [Column::i64(0)];
    let mut reader =
        DelimitedReader::from_slice(b"1\n", Dialect::CSV.with_header(false), &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let _ = batch.f64(0);
}
