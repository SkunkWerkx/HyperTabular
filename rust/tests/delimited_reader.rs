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
    Column, DelimitedReader, Dialect, Error, FailureKind, NumFormat, Reason, Timestamp,
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
    let continental = NumFormat::new(',', '.', NumFormat::ALL);
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
    // Equal separators: a format HyperCast's own doors refuse, said before any read.
    let same = NumFormat::new(',', ',', NumFormat::ALL);
    let plan = [Column::text(0), Column::f64(1).format(same)];
    assert!(matches!(
        DelimitedReader::from_slice(b"a,b\n", Dialect::CSV, &plan),
        Err(Error::Plan { column: 1 })
    ));
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

#[test]
fn the_options_reach_a_batch() {
    use hypertabular::{Detail, LongRows, Origin, ShortRows};
    let dialect = Dialect::new(b',')
        .with_skip(1)
        .with_header_row(1)
        .with_short_rows(ShortRows::Pad)
        .with_long_rows(LongRows::Truncate)
        .with_tokens(&["NA"]);
    assert_eq!(
        (
            dialect.skip,
            dialect.header_row,
            dialect.short_rows,
            dialect.long_rows
        ),
        (1, 1, ShortRows::Pad, LongRows::Truncate)
    );
    let data = b"a stray \" title\nx,y\nn,m\nNA,NA\n12a\n1,2,3\n";
    let plan = [Column::i32(0), Column::text(1).tokens(&[])];
    let mut reader = DelimitedReader::from_slice(data, dialect, &plan).unwrap();
    assert_eq!(reader.header().unwrap().name(0), Some(&b"n"[..]));
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.rows(), 3);
    assert_eq!(batch.origin(0, 0), Origin::Token);
    assert_eq!(batch.text(1, 0), Ok(&b"NA"[..]));
    assert_eq!(batch.origin(1, 0), Origin::Cell);
    assert_eq!(batch.detail(0, 1), Some(Detail::UnexpectedCharacter));
    let row = batch.row(1);
    assert_eq!(row.detail(0), Some(Detail::UnexpectedCharacter));
    assert_eq!(row.origin(1), Origin::Padded);
    assert_eq!(batch.get::<i32>(0, 2), Ok(1));
    assert_eq!(batch.detail(0, 2), None);
    assert_eq!(batch.line(0), 4);
}

#[test]
fn notes_can_be_left_out() {
    use hypertabular::Origin;
    let dialect = Dialect::CSV.with_header(false);
    let plan = [Column::i32(0), Column::i32(0).tokens(&["x"])];
    let mut reader = DelimitedReader::options()
        .notes(false)
        .from_slice(b"x\n", dialect, &plan)
        .unwrap();
    let batch = reader.read().unwrap().unwrap();
    // The verdicts are what they'd be: a door's fault, and a token's Empty.
    assert_eq!(batch.verdicts(0)[0].reason, 2);
    assert_eq!(batch.verdicts(1)[0].reason, 1);
    assert_eq!(
        (batch.detail(0, 0), batch.origin(1, 0)),
        (None, Origin::Cell)
    );
}

#[test]
fn a_date_declaration_hypercast_refuses_is_a_plan_error() {
    use hypertabular::{DateOptions, Error, MonthNames};
    let read = |column: Column| {
        DelimitedReader::from_slice(b"20260107\n", Dialect::CSV.with_header(false), &[column])
            .map(|_| ())
    };
    let refused = |read: Result<(), Error>| matches!(read, Err(Error::Plan { column: 0 }));
    let flags = DateOptions::from_flags(1 << 20);
    assert!(refused(read(Column::date(0).date_options(flags, None))));
    let pivot = DateOptions::NONE.with_pivot(50);
    assert!(refused(read(Column::date(0).date_options(pivot, None))));
    let months = MonthNames::from_entries(&[(13, "Nope")]);
    let vocabulary = Column::date(0).date_options(DateOptions::NONE, Some(months));
    assert!(refused(read(vocabulary)));
    // A control: the same reader takes what HyperCast takes, and it reaches the door.
    let good =
        Column::date(0).date_options(DateOptions::from_flags(DateOptions::COMPACT_DATE), None);
    let mut reader =
        DelimitedReader::from_slice(b"20260107\n", Dialect::CSV.with_header(false), &[good])
            .unwrap();
    assert!(reader.read().unwrap().unwrap().verdicts(0)[0].is_ok());
}

/// A column and a dialect stay `Copy` and comparable: equal lists are equal, however declared.
#[test]
fn token_lists_are_values() {
    let one = Column::text(0).tokens(&["a", "b"]);
    let two = Column::text(0).tokens(&["a", "b"]);
    assert_eq!(one, two);
    assert_ne!(one, Column::text(0).tokens(&["b", "a"]));
    assert_ne!(one, Column::text(0));
    assert_ne!(Column::text(0).tokens(&[]), Column::text(0));
    assert_eq!(
        Dialect::CSV.with_tokens(&["a"]),
        Dialect::CSV.with_tokens(&["a"])
    );
    assert_ne!(Dialect::CSV.with_tokens(&["a"]), Dialect::CSV);
}

// ---- A declared encoding, decoded ahead of the read. ----

mod encoded {
    use hypertabular::{Column, DelimitedReader, Dialect, Encoding, Error, Failure, FailureKind};
    use std::io::{self, Read};

    /// A source that hands over at most `most` bytes a read.
    struct Trickle<'a>(&'a [u8], usize);

    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.0.len().min(self.1).min(out.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    const UTF16: Dialect = Dialect::CSV
        .with_header(false)
        .with_encoding(Encoding::Utf16Le);

    /// Every row's text cells, and how the read ended.
    fn read_all(
        reader: Result<DelimitedReader<'_>, Error>,
        columns: usize,
    ) -> (Vec<Vec<String>>, Option<Failure>) {
        let mut reader = match reader {
            Ok(reader) => reader,
            Err(Error::Structure(failure)) => return (Vec::new(), Some(failure)),
            Err(other) => panic!("{other:?}"),
        };
        let mut rows = Vec::new();
        loop {
            match reader.read() {
                Ok(Some(batch)) => {
                    for row in 0..batch.rows() {
                        rows.push(
                            (0..columns)
                                .map(|column| {
                                    String::from_utf8(batch.text(column, row).unwrap().to_vec())
                                        .unwrap()
                                })
                                .collect(),
                        );
                    }
                }
                Ok(None) => return (rows, None),
                Err(Error::Structure(failure)) => {
                    // It stays the reader's last word.
                    match reader.read() {
                        Err(Error::Structure(again)) => assert_eq!(again, failure),
                        other => panic!("{:?}", other.map(|batch| batch.map(|b| b.rows()))),
                    }
                    return (rows, Some(failure));
                }
                Err(other) => panic!("{other:?}"),
            }
        }
    }

    /// The same read over every way of handing the bytes over: a slice, a file's worth of
    /// buffers at every small size, and reads at every small size, in batches of one and more.
    fn every_way(
        input: &[u8],
        dialect: Dialect,
        columns: usize,
    ) -> (Vec<Vec<String>>, Option<Failure>) {
        let plan: Vec<Column> = (0..columns).map(Column::text).collect();
        let first = read_all(DelimitedReader::from_slice(input, dialect, &plan), columns);
        for batch_rows in [1, 2, 1024] {
            let options = DelimitedReader::options().batch_rows(batch_rows);
            assert_eq!(
                read_all(options.from_slice(input, dialect, &plan), columns),
                first,
                "a slice, {batch_rows} a batch"
            );
            for buffer in 1..=9 {
                for most in 1..=5 {
                    let source = Trickle(input, most);
                    assert_eq!(
                        read_all(
                            options
                                .buffer_bytes(buffer)
                                .from_reader(source, dialect, &plan),
                            columns
                        ),
                        first,
                        "{buffer} buffered, {most} a read, {batch_rows} a batch"
                    );
                }
            }
        }
        first
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_reader_decodes_what_the_dialect_declares() {
        let dialect = Dialect::CSV.with_encoding(Encoding::Windows1252);
        let plan = [Column::text(0), Column::text(1)];
        let reader = DelimitedReader::from_slice(b"h\xe9,n\n\x80,\x81\n", dialect, &plan).unwrap();
        assert_eq!(reader.dialect().encoding, Encoding::Windows1252);
        assert_eq!(
            reader.header().unwrap().name(0).unwrap(),
            "h\u{e9}".as_bytes()
        );
        assert_eq!(
            read_all(Ok(reader), 2),
            (vec![row(&["\u{20AC}", "\u{81}"])], None)
        );
        let latin = Dialect::CSV
            .with_encoding(Encoding::Latin1)
            .with_header(false);
        assert_eq!(
            every_way(b"\xe9,\x80\n", latin, 2),
            (vec![row(&["\u{e9}", "\u{80}"])], None)
        );
        assert_eq!(Dialect::CSV.encoding, Encoding::Utf8);
    }

    #[test]
    fn a_batch_says_what_it_decoded() {
        // A fault's span is the decoded, unescaped text's, never the file's.
        let dialect = Dialect::CSV
            .with_header(false)
            .with_encoding(Encoding::Windows1252);
        let plan = [Column::i32(0)];
        let mut reader = DelimitedReader::from_slice(b"\"\x80\"\"x\"\n", dialect, &plan).unwrap();
        let batch = reader.read().unwrap().unwrap();
        let fault = batch.verdicts(0)[0].fault().unwrap();
        assert_eq!(fault.offset, 0);
        assert_eq!(batch.raw(0, 0).as_ref(), "\u{20AC}\"x".as_bytes());
    }

    #[test]
    fn the_reader_strips_one_bom() {
        // The stream's first U+FEFF goes through the skip at byte 0, and the next is text.
        let input = utf16("\u{FEFF}\u{FEFF}a\nb\n");
        assert_eq!(
            every_way(&input, UTF16, 1),
            (vec![row(&["\u{FEFF}a"]), row(&["b"])], None)
        );
    }

    #[test]
    fn a_later_feff_survives() {
        // Later in the first chunk, and at the start of every later one: every read size and
        // buffer in `every_way` lands a chunk on it.
        let input = utf16("a\n\u{FEFF}b\nc\u{FEFF}\n");
        assert_eq!(
            every_way(&input, UTF16, 1),
            (
                vec![row(&["a"]), row(&["\u{FEFF}b"]), row(&["c\u{FEFF}"])],
                None
            )
        );
    }

    #[test]
    fn windows_1252_ef_bb_bf_is_not_a_bom() {
        let dialect = Dialect::CSV
            .with_header(false)
            .with_encoding(Encoding::Windows1252);
        assert_eq!(
            every_way(b"\xef\xbb\xbfa\n", dialect, 1),
            (vec![row(&["\u{ef}\u{bb}\u{bf}a"])], None)
        );
        let latin = dialect.with_encoding(Encoding::Latin1);
        assert_eq!(
            every_way(b"\xef\xbb\xbfa\n", latin, 1),
            (vec![row(&["\u{ef}\u{bb}\u{bf}a"])], None)
        );
        // Undeclared, those three bytes are the mark.
        let plain = Dialect::CSV.with_header(false);
        assert_eq!(
            every_way(b"\xef\xbb\xbfa\n", plain, 1),
            (vec![row(&["a"])], None)
        );
    }

    fn encoding_failure(record: u64, line: u32, byte: u64) -> Option<Failure> {
        Some(Failure {
            kind: FailureKind::Encoding,
            record,
            line,
            byte,
            expected: 0,
            found: 0,
        })
    }

    #[test]
    fn the_reader_reports_the_original_offset() {
        // One byte decoded is three, and two units are four: a decoded offset would be wrong
        // here in either direction. The mark counts, as the file holds it.
        let good = "\u{FEFF}\u{20AC}\n\u{1F600}x\n";
        let input = [utf16(good), vec![0x00, 0xDC], utf16("after\n")].concat();
        let at = utf16(good).len() as u64;
        assert_eq!(
            every_way(&input, UTF16, 1),
            (
                vec![row(&["\u{20AC}"]), row(&["\u{1F600}x"])],
                encoding_failure(2, 3, at)
            )
        );
    }

    #[test]
    fn the_reader_delivers_rows_before_the_failure() {
        // A batch of rows, then a record the failure cut, which isn't one of them, however far
        // it got: not a row, not a width to refuse and not an open quote.
        for cut in ["3", "3,", "3,4,5", "\"3"] {
            let good = format!("a,b\n1,2\n{cut}");
            let input = [utf16(&good), vec![0x00, 0xD8, 0x61, 0x00]].concat();
            let at = utf16(&good).len() as u64;
            assert_eq!(
                every_way(&input, UTF16, 2),
                (
                    vec![row(&["a", "b"]), row(&["1", "2"])],
                    encoding_failure(2, 3, at)
                ),
                "{cut}"
            );
        }
        // A final half unit, the same.
        let input = [utf16("1\n2\n"), vec![0x33]].concat();
        assert_eq!(
            every_way(&input, UTF16, 1),
            (vec![row(&["1"]), row(&["2"])], encoding_failure(2, 3, 8))
        );
        // Lone high at the very end.
        let input = [utf16("1\n"), vec![0x00, 0xD8]].concat();
        assert_eq!(
            every_way(&input, UTF16, 1),
            (vec![row(&["1"])], encoding_failure(1, 2, 4))
        );
    }

    #[test]
    fn a_row_ended_by_a_lone_cr_is_delivered_before_the_failure() {
        // A `\r` is a whole line end, whatever follows it; the core waits only to see whether a
        // `\n` is part of it, and a failure says none is.
        let input = [utf16("1\r"), vec![0x00, 0xDC]].concat();
        assert_eq!(
            every_way(&input, UTF16, 1),
            (vec![row(&["1"])], encoding_failure(1, 2, 4))
        );
        // Inside a quote it ends nothing, and the record is still cut.
        let input = [utf16("1\n\"2\r"), vec![0x00, 0xDC]].concat();
        assert_eq!(
            every_way(&input, UTF16, 1),
            (vec![row(&["1"])], encoding_failure(1, 2, 10))
        );
    }

    #[test]
    fn a_failure_in_the_header_comes_from_the_constructor() {
        let dialect = UTF16.with_header(true);
        let input = [utf16("\u{FEFF}a,"), vec![0x00, 0xDC]].concat();
        let reader = DelimitedReader::from_slice(&input, dialect, &[Column::text(0)]);
        match reader {
            Err(Error::Structure(failure)) => {
                assert_eq!(Some(failure), encoding_failure(0, 1, 6));
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
        // A header that ended before the failure is read, and the first read says.
        let input = [utf16("a\r"), vec![0x00, 0xDC]].concat();
        let mut reader = DelimitedReader::from_slice(&input, dialect, &[Column::text(0)]).unwrap();
        assert_eq!(reader.header().unwrap().name(0).unwrap(), b"a");
        match reader.read() {
            Err(Error::Structure(failure)) => assert_eq!(Some(failure), encoding_failure(1, 2, 4)),
            other => panic!("{:?}", other.map(|batch| batch.map(|b| b.rows()))),
        }
    }

    #[test]
    fn what_passes_over_is_decoded_all_the_same() {
        // A skipped line isn't read, but nothing can be skipped before it is decoded.
        let dialect = UTF16.with_skip(1);
        let input = [utf16("title"), vec![0x00, 0xDC], utf16("\n1\n")].concat();
        assert_eq!(
            every_way(&input, dialect, 1),
            (Vec::new(), encoding_failure(0, 1, 10))
        );
        let input = utf16("t\"itle\n1\n");
        assert_eq!(every_way(&input, dialect, 1), (vec![row(&["1"])], None));
        // A skipped line a lone `\r` ended is one line more, as it is with no failure.
        let input = [utf16("title\r"), vec![0x00, 0xDC]].concat();
        assert_eq!(
            every_way(&input, dialect, 1),
            (Vec::new(), encoding_failure(0, 2, 12))
        );
    }

    #[test]
    fn an_empty_source_is_an_empty_read() {
        for encoding in [Encoding::Windows1252, Encoding::Latin1, Encoding::Utf16Le] {
            assert_eq!(
                every_way(
                    b"",
                    Dialect::CSV.with_header(false).with_encoding(encoding),
                    1
                ),
                (Vec::new(), None)
            );
        }
        // The mark and nothing else.
        assert_eq!(every_way(&[0xFF, 0xFE], UTF16, 1), (Vec::new(), None));
    }

    #[test]
    fn no_final_line_end_is_a_row_still() {
        assert_eq!(
            every_way(&utf16("1\n2"), UTF16, 1),
            (vec![row(&["1"]), row(&["2"])], None)
        );
        assert_eq!(
            every_way(&utf16("1\r"), UTF16, 1),
            (vec![row(&["1"])], None)
        );
    }

    /// Fails with an interruption every other call, and at `end` for good.
    struct Awkward<'a> {
        bytes: &'a [u8],
        interrupted: bool,
        end: io::ErrorKind,
    }

    impl Read for Awkward<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.interrupted = !self.interrupted;
            if self.interrupted {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.bytes.is_empty() {
                return Err(self.end.into());
            }
            let n = out.len().min(self.bytes.len()).min(3);
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_stream_that_fails_is_still_an_io_error() {
        let input = utf16("1\n2\n3");
        let source = Awkward {
            bytes: &input,
            interrupted: false,
            end: io::ErrorKind::BrokenPipe,
        };
        let mut reader = DelimitedReader::options()
            .buffer_bytes(2)
            .from_reader(source, UTF16, &[Column::text(0)])
            .unwrap();
        let mut seen = Vec::new();
        let error = loop {
            match reader.read() {
                Ok(Some(batch)) => {
                    for row in 0..batch.rows() {
                        seen.push(batch.text(0, row).unwrap().to_vec());
                    }
                }
                Ok(None) => panic!("the source never ends"),
                Err(error) => break error,
            }
        };
        assert_eq!(seen, [b"1", b"2"]);
        match error {
            Error::Io(error) => assert_eq!(error.kind(), io::ErrorKind::BrokenPipe),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_failure_reads_as_itself() {
        let failure = encoding_failure(2, 3, 8).unwrap();
        let text = failure.to_string();
        // Nothing in a failure says whether it's delimited text's or a workbook's.
        assert_eq!(
            text,
            "text that isn't valid in its encoding at byte 8 (record or part 2, line or row 3)"
        );
    }
}

#[cfg(feature = "async")]
mod encoded_async {
    use futures_io::AsyncRead;
    use hypertabular::{Column, DelimitedReader, Dialect, Encoding, Error, FailureKind};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
    }

    #[test]
    fn the_core_judges_the_sizes_of_an_asynchronous_read_before_it_starts() {
        let source = || Trickle {
            bytes: b"a\n1\n",
            most: 1,
            pending: false,
        };
        let dialect = Dialect::CSV.with_header(false);
        block_on(async {
            let zero = DelimitedReader::options()
                .batch_rows(0)
                .from_async_reader(source(), dialect)
                .await;
            assert!(matches!(zero, Err(Error::ZeroBatchRows)));
            let zero = DelimitedReader::options()
                .buffer_bytes(0)
                .from_async_reader(source(), dialect)
                .await;
            assert!(matches!(zero, Err(Error::ZeroBufferBytes)));
            let mut reader = DelimitedReader::options()
                .batch_rows(1 << 62)
                .from_async_reader(source(), dialect)
                .await
                .unwrap();
            assert!(matches!(
                reader.bind(&[Column::i32(0)]),
                Err(Error::BatchTooLarge { .. })
            ));
        });
    }

    /// At most `most` bytes a read, and pending every other poll.
    struct Trickle<'a> {
        bytes: &'a [u8],
        most: usize,
        pending: bool,
    }

    impl AsyncRead for Trickle<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            out: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            self.pending = !self.pending;
            if self.pending {
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            let n = self.bytes.len().min(self.most).min(out.len());
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Poll::Ready(Ok(n))
        }
    }

    const UTF16: Dialect = Dialect::CSV
        .with_header(false)
        .with_encoding(Encoding::Utf16Le);

    #[test]
    fn an_async_read_decodes_and_fails_the_same() {
        let good: Vec<u8> = "\u{FEFF}\u{20AC}\n\u{1F600}\r"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let bad = [&good[..], &[0x00, 0xDC][..]].concat();
        for most in 1..=5 {
            for buffer in 1..=9 {
                let got = block_on(async {
                    let source = Trickle {
                        bytes: &bad,
                        most,
                        pending: false,
                    };
                    let mut reader = DelimitedReader::options()
                        .buffer_bytes(buffer)
                        .from_async_reader(source, UTF16)
                        .await
                        .unwrap();
                    reader.bind(&[Column::text(0)]).unwrap();
                    let mut rows = Vec::new();
                    loop {
                        match reader.read_async().await {
                            Ok(Some(batch)) => {
                                for row in 0..batch.rows() {
                                    rows.push(batch.text(0, row).unwrap().to_vec());
                                }
                            }
                            Ok(None) => return (rows, None),
                            Err(Error::Structure(failure)) => return (rows, Some(failure)),
                            Err(other) => panic!("{other:?}"),
                        }
                    }
                });
                assert_eq!(
                    got.0,
                    ["\u{20AC}".as_bytes(), "\u{1F600}".as_bytes()],
                    "{most}, {buffer}"
                );
                let failure = got.1.expect("a failure");
                assert_eq!(failure.kind, FailureKind::Encoding);
                assert_eq!(failure.byte, good.len() as u64, "{most}, {buffer}");
            }
        }
    }

    /// The `\r` the failure closes can be the last byte the buffer has room for: the source's
    /// one read holds ten bytes, which decode to exactly the ten it starts with.
    #[test]
    fn a_failure_finds_room_for_the_line_end() {
        let good: Vec<u8> = "\u{20AC}\u{20AC}\u{20AC}\r"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let bad = [&good[..], &[0x00, 0xDC][..]].concat();
        let got = block_on(async {
            let source = Trickle {
                bytes: &bad,
                most: 64,
                pending: false,
            };
            let mut reader = DelimitedReader::options()
                .buffer_bytes(10)
                .from_async_reader(source, UTF16)
                .await
                .unwrap();
            reader.bind(&[Column::text(0)]).unwrap();
            let batch = reader.read_async().await.unwrap().unwrap();
            let text = batch.text(0, 0).unwrap().to_vec();
            let error = reader.read_async().await.map(|_| ()).unwrap_err();
            (text, error)
        });
        assert_eq!(got.0, "\u{20AC}\u{20AC}\u{20AC}".as_bytes());
        match got.1 {
            Error::Structure(failure) => {
                assert_eq!((failure.kind, failure.byte), (FailureKind::Encoding, 8));
            }
            other => panic!("{other:?}"),
        }
    }

    /// A read dropped at every await loses nothing of the source or of what was decoded.
    #[test]
    fn an_abandoned_read_loses_nothing() {
        let input: Vec<u8> = "\u{20AC}a\n\u{1F600}b\nc\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let got = block_on(async {
            let source = Trickle {
                bytes: &input,
                most: 2,
                pending: false,
            };
            let mut reader = DelimitedReader::options()
                .buffer_bytes(1)
                .from_async_reader(source, UTF16)
                .await
                .unwrap();
            reader.bind(&[Column::text(0)]).unwrap();
            let mut rows = Vec::new();
            // Far more than this input takes, so that a read that never gets anywhere fails.
            for _ in 0..10_000 {
                {
                    // Polled once and dropped, with the source pending half the time.
                    let read = std::pin::pin!(reader.read_async());
                    let mut context = Context::from_waker(Waker::noop());
                    match read.poll(&mut context) {
                        Poll::Ready(Ok(Some(batch))) => {
                            for row in 0..batch.rows() {
                                rows.push(batch.text(0, row).unwrap().to_vec());
                            }
                        }
                        Poll::Ready(Ok(None)) => return rows,
                        Poll::Ready(Err(error)) => panic!("{error:?}"),
                        Poll::Pending => {}
                    }
                }
            }
            panic!("10,000 polls, each dropped, and the read hasn't ended: {rows:?}");
        });
        assert_eq!(
            got,
            [
                "\u{20AC}a".as_bytes(),
                "\u{1F600}b".as_bytes(),
                "c".as_bytes()
            ]
        );
    }
}

/// A column given no format reads Western grouping, the same as one declaring
/// [`NumFormat::INVARIANT`], and neither reads a shape into a wrong value.
#[test]
fn a_column_given_no_format_reads_western_grouping() {
    let bytes = b"\"1,23,456\"\n\"12,34\"\n\"1,234,567\"\n";
    let read = |column: Column| {
        let dialect = Dialect::CSV.with_header(false);
        let mut reader = DelimitedReader::from_slice(bytes, dialect, &[column]).unwrap();
        let batch = reader.read().unwrap().unwrap();
        (
            batch.get::<i32>(0, 0),
            batch.get::<i32>(0, 1),
            batch.get::<i32>(0, 2),
        )
    };
    let (indian, short, western) = read(Column::i32(0));
    assert_eq!(indian.unwrap_err().reason, Reason::Malformed);
    assert_eq!(short.unwrap_err().reason, Reason::Malformed);
    assert_eq!(western, Ok(1234567));
    assert_eq!(
        read(Column::i32(0).format(NumFormat::INVARIANT)),
        read(Column::i32(0))
    );
}
