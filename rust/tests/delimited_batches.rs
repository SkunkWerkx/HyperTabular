//! The Rust binding over the core — [`BatchReader`] — against the path it replaces: the
//! row-at-a-time [`Reader`] cast into a [`Batch`] one cell at a time. Same values, same
//! verdicts, same text, same fault table, whether the input is a slice or a stream read
//! through a buffer too small for a row.

use hypertabular::delimited::{BatchReader, Dialect, Error, Reader};
use hypertabular::{Batch, Column, Door, Plan, Values, fill_batch};
use std::io::Cursor;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

const CELLS: &[&str] = &[
    "",
    "1",
    "-17",
    "4294967296",
    "\"1,234\"",
    "2.5",
    "50%",
    " 7 ",
    "true",
    "2024-01-31",
    "1/7/2026 3:04 PM",
    "2024-01-31T10:30:00Z",
    "15:04:05.5",
    "PT1H30M",
    "45292.75",
    "1700000000",
    "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "plain text",
    "日本語 é 🙂",
    "\"has \"\"escapes\"\" inside\"",
    "\"line one\nline two\"",
    "\"12\"\"3\"",
    "12x4",
];

/// Every row as strings: a value's `Debug`, a text cell's bytes, or its verdict.
fn rows_of(batch: &Batch, out: &mut Vec<Vec<String>>) {
    let base = out.len();
    out.extend((0..batch.rows()).map(|_| Vec::new()));
    for (index, data) in batch.columns().iter().enumerate() {
        macro_rules! show {
            ($($variant:ident),+) => {
                match data.values() {
                    $(Values::$variant(v) => v.iter().map(|value| format!("{value:?}")).collect::<Vec<_>>(),)+
                    Values::Text(_) => (0..batch.rows())
                        .map(|row| format!("{:?}", batch.text(index, row)))
                        .collect(),
                }
            };
        }
        let shown = show!(
            Bool, I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Uuid, Timestamp, Date, Time,
            Duration, Decimal, DateTime
        );
        for (row, value) in shown.into_iter().enumerate() {
            let verdict = data.verdicts()[row];
            out[base + row].push(if verdict.is_ok() {
                value
            } else {
                format!(
                    "fault {} at {}+{}",
                    verdict.reason, verdict.offset, verdict.len
                )
            });
        }
    }
}

fn faults_of(batch: &Batch, base: usize, out: &mut Vec<(usize, u32, Vec<u8>)>) {
    for fault in batch.faults() {
        out.push((
            base + fault.row as usize,
            fault.column,
            batch.raw(fault).to_vec(),
        ));
    }
}

type Read = (Vec<Vec<String>>, Vec<(usize, u32, Vec<u8>)>, Vec<Vec<u8>>);

fn through_the_reader(data: &[u8], plan: &Plan, max_rows: usize) -> Read {
    let mut reader = Reader::from_slice(data, Dialect::CSV).unwrap();
    let header = reader
        .header()
        .unwrap()
        .names()
        .map(<[u8]>::to_vec)
        .collect();
    let (mut rows, mut faults, mut batch) = (Vec::new(), Vec::new(), Batch::new());
    while fill_batch(&mut reader, plan, &mut batch, max_rows).unwrap() > 0 {
        faults_of(&batch, rows.len(), &mut faults);
        rows_of(&batch, &mut rows);
    }
    (rows, faults, header)
}

fn through_the_core(mut reader: BatchReader<'_>, max_rows: usize) -> Read {
    let header = reader
        .header()
        .unwrap()
        .names()
        .map(<[u8]>::to_vec)
        .collect();
    let (mut rows, mut faults, mut batch) = (Vec::new(), Vec::new(), Batch::new());
    while reader.fill(&mut batch, max_rows).unwrap() > 0 {
        faults_of(&batch, rows.len(), &mut faults);
        rows_of(&batch, &mut rows);
    }
    assert_eq!(
        reader.fill(&mut batch, max_rows).unwrap(),
        0,
        "the end is the end"
    );
    (rows, faults, header)
}

#[test]
fn the_binding_and_the_reader_fill_the_same_batches() {
    let mut random = Random(0xB10C_5EED_0000_0001);
    let doors: Vec<Door> = (1..=22u32)
        .filter_map(|code| Door::from_code(code, 1))
        .collect();
    assert_eq!(doors.len(), 22);
    for round in 0..200 {
        let columns = 1 + random.below(6);
        let mut text = Vec::new();
        if random.below(4) == 0 {
            text.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        for row in 0..1 + random.below(80) {
            for column in 0..columns {
                if column > 0 {
                    text.push(b',');
                }
                let cell = if row == 0 {
                    ["id", "\"full name\"", "\"say \"\"hi\"\"\"", "when"][random.below(4)]
                } else {
                    CELLS[random.below(CELLS.len())]
                };
                let cell = if columns == 1 && cell.is_empty() {
                    "0"
                } else {
                    cell
                };
                text.extend_from_slice(cell.as_bytes());
            }
            text.extend_from_slice(["\n", "\r\n", "\r", "\n\n"][random.below(4)].as_bytes());
        }
        let plan = Plan::new(
            (0..1 + random.below(8))
                .map(|_| Column::new(random.below(columns + 1), doors[random.below(doors.len())]))
                .collect(),
        );
        let max_rows = [1, 3, 16, 1024][random.below(4)];
        let expected = through_the_reader(&text, &plan, max_rows);

        let slice = BatchReader::from_slice(&text, Dialect::CSV, plan.clone()).unwrap();
        assert_eq!(
            through_the_core(slice, max_rows),
            expected,
            "round {round}, slice"
        );

        let capacity = [1, 7, 64, 4096][random.below(4)];
        let stream = BatchReader::from_reader_with_capacity(
            Cursor::new(&text),
            Dialect::CSV,
            plan.clone(),
            capacity,
        )
        .unwrap();
        assert_eq!(
            through_the_core(stream, max_rows),
            expected,
            "round {round}, stream with a {capacity}-byte buffer"
        );
    }
}

#[test]
fn structural_errors_reach_the_caller_as_the_reader_reports_them() {
    let plan = Plan::new(vec![Column::new(0, Door::I32)]);
    let text = b"a,b\n1,2\n3\n4,5\n";
    let mut reader = BatchReader::from_slice(text, Dialect::CSV, plan.clone()).unwrap();
    let mut batch = Batch::new();
    assert_eq!(reader.fill(&mut batch, 100).unwrap(), 1);
    let error = reader.fill(&mut batch, 100).unwrap_err();
    assert!(
        matches!(
            error,
            Error::ColumnCount {
                expected: 2,
                found: 1,
                record: 2,
                line: 3,
                byte: 8
            }
        ),
        "{error:?}"
    );
    // Sticky.
    assert!(matches!(
        reader.fill(&mut batch, 100),
        Err(Error::ColumnCount { .. })
    ));

    let mut reader = BatchReader::from_slice(b"a\n1\n\"open\n", Dialect::CSV, plan).unwrap();
    assert_eq!(reader.fill(&mut batch, 100).unwrap(), 1);
    let error = reader.fill(&mut batch, 100).unwrap_err();
    assert!(
        matches!(
            error,
            Error::UnclosedQuote {
                record: 2,
                line: 3,
                byte: 4
            }
        ),
        "{error:?}"
    );

    // A plan the core would refuse is refused before any input is read.
    let colliding = hypertabular::NumFormat::new(',', ',', 0);
    let plan = Plan::new(vec![Column::new(0, Door::F64).with_format(colliding)]);
    let error = BatchReader::from_slice(b"a\n1\n", Dialect::CSV, plan).err();
    assert!(
        matches!(error, Some(Error::Notation { column: 0 })),
        "{error:?}"
    );
}
