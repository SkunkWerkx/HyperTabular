//! The allocation-free core against the row-at-a-time reader it replaces underneath the
//! bindings. The reader has its own conformance suite (against the `csv` crate, on every
//! engine); here it is the reference, and the core has to land on the same cells, the same
//! values and the same verdicts — for every door, however the input is cut into chunks,
//! and however little room it is given.

use hypertabular::delimited::{Dialect, Reader, engine};
use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE,
    Failure, Filled, OK, Span,
};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::{Cell, Column, Door, ExcelEpoch, Value};
use std::fmt::Debug;

/// xorshift64*: deterministic, no dependency.
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

    fn pick<'a>(&mut self, pool: &[&'a str]) -> &'a str {
        pool[self.below(pool.len())]
    }
}

/// One cell as either side saw it: the value's own `Debug`, or the fault.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seen {
    Value(String),
    Fault(u32, u32, u32),
}

const CELLS: &[&str] = &[
    "",
    "0",
    "1",
    "-17",
    "255",
    "256",
    "65536",
    "4294967296",
    "\"1,234\"",
    "(42)",
    "2.5",
    "-0.125",
    "1e10",
    "50%",
    "$12.50",
    " 7 ",
    "true",
    "No",
    "enabled",
    "2024-01-31",
    "1/7/2026",
    "2026-01-07 15:04:05",
    "1/7/2026 3:04 PM",
    "2024-01-31T10:30:00Z",
    "2024-01-31T10:30:00.123456789+02:00",
    "15:04:05.5",
    "PT1H30M",
    "1:30:00",
    "45292.75",
    "60",
    "1700000000",
    "1700000000123",
    "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "{6BA7B810-9DAD-11D1-80B4-00C04FD430C8}",
    "plain text",
    "日本語 é 🙂",
    "\"quoted\"",
    "\"has \"\"escapes\"\" inside\"",
    "\"line one\nline two\"",
    "\"cr\r\nlf, and a comma\"",
    "\"\"",
    "\"\"\"\"",
    "\"12\"\"3\"",
    "x\"\"y",
    "\"a\"b\"c\"",
    "12x4",
    "99999999999999999999999999",
];

/// A well-formed file: `columns` cells in every record, any terminator, blank lines here
/// and there, with or without a byte-order mark and a final terminator.
fn generate(random: &mut Random, rows: usize, columns: usize) -> Vec<u8> {
    let mut text = Vec::new();
    if random.below(4) == 0 {
        text.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    for row in 0..rows {
        for column in 0..columns {
            if column > 0 {
                text.push(b',');
            }
            let cell = random.pick(CELLS);
            // A lone empty cell on a line is a blank line, which is skipped — keep the
            // single-column case's rows real.
            let cell = if columns == 1 && cell.is_empty() {
                "0"
            } else {
                cell
            };
            text.extend_from_slice(cell.as_bytes());
        }
        if row + 1 == rows && random.below(3) == 0 {
            break;
        }
        text.extend_from_slice(random.pick(&["\n", "\n", "\r\n", "\r"]).as_bytes());
        if random.below(9) == 0 {
            text.extend_from_slice(random.pick(&["\n", "\r\n"]).as_bytes());
        }
    }
    text
}

fn doors() -> Vec<(u32, u32)> {
    let mut doors = Vec::new();
    for code in 1..=22u32 {
        let params: &[u32] = match code {
            14 => &[1, 2, 3, 4],
            20 | 21 => &[1, 2, 3],
            22 => &[1, 2],
            _ => &[0],
        };
        doors.extend(params.iter().map(|&param| (code, param)));
    }
    doors
}

fn value_size(door: Door) -> usize {
    match door {
        Door::Bool | Door::I8 | Door::U8 => 1,
        Door::I16 | Door::U16 => 2,
        Door::I32 | Door::U32 | Door::F32 => 4,
        Door::I64 | Door::U64 | Door::F64 | Door::Time => 8,
        Door::Decimal => size_of::<hypercast::Decimal>(),
        Door::Uuid => 16,
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => size_of::<hypercast::Timestamp>(),
        Door::Date | Door::DateOrdered(_) => size_of::<hypercast::Date>(),
        Door::DateTime(_) => size_of::<hypercast::CivilDateTime>(),
        Door::Duration => size_of::<hypercast::Duration>(),
        Door::Text => size_of::<Span>(),
    }
}

fn show<T: Copy + Debug>(values: &[u8], row: usize) -> String {
    let at = row * size_of::<T>();
    assert!(at + size_of::<T>() <= values.len());
    // SAFETY: bounds just checked; the read is unaligned.
    format!("{:?}", unsafe {
        values.as_ptr().add(at).cast::<T>().read_unaligned()
    })
}

/// What the reference says of every cell the plan reads, row by row.
fn reference(data: &[u8], specs: &[ColumnSpec]) -> Vec<Vec<Seen>> {
    let dialect = Dialect::CSV.with_header(false);
    let mut reader = Reader::from_slice(data, dialect).unwrap();
    let mut rows = Vec::new();
    while let Some(row) = reader.next_row().unwrap() {
        let mut seen = Vec::new();
        for spec in specs {
            let door = Door::from_code(spec.door, spec.param).unwrap();
            let column = Column::new(spec.ordinal as usize, door);
            let cell = match row.get(spec.ordinal as usize) {
                Some(bytes) => Cell::Text(bytes),
                None => Cell::Empty,
            };
            seen.push(match column.cast(&cell, ExcelEpoch::Y1900) {
                Ok(value) => Seen::Value(match value {
                    Value::Bool(v) => format!("{:?}", u8::from(v)),
                    Value::I8(v) => format!("{v:?}"),
                    Value::I16(v) => format!("{v:?}"),
                    Value::I32(v) => format!("{v:?}"),
                    Value::I64(v) => format!("{v:?}"),
                    Value::U8(v) => format!("{v:?}"),
                    Value::U16(v) => format!("{v:?}"),
                    Value::U32(v) => format!("{v:?}"),
                    Value::U64(v) => format!("{v:?}"),
                    Value::F32(v) => format!("{v:?}"),
                    Value::F64(v) => format!("{v:?}"),
                    Value::Uuid(v) => format!("{v:?}"),
                    Value::Timestamp(v) => format!("{v:?}"),
                    Value::Date(v) => format!("{v:?}"),
                    Value::Time(v) => format!("{v:?}"),
                    Value::Duration(v) => format!("{v:?}"),
                    Value::Text(v) => format!("{:?}", v.as_ref()),
                    Value::Decimal(v) => format!("{v:?}"),
                    Value::DateTime(v) => format!("{v:?}"),
                }),
                Err(fault) => Seen::Fault(fault.reason as u32, fault.offset, fault.len),
            });
        }
        rows.push(seen);
    }
    rows
}

/// How a run feeds the core: the chunk sizes it reads, the rows it asks for per call, and
/// how much table and arena it starts with (both grown on demand, as a binding would).
struct Feeding {
    chunk: usize,
    max_rows: usize,
    cells: usize,
    arena: usize,
    engine: u8,
}

/// What the core says of the same cells, streamed.
#[allow(clippy::needless_range_loop)]
fn streamed(
    data: &[u8],
    specs: &[ColumnSpec],
    feeding: &Feeding,
    random: &mut Random,
) -> Vec<Vec<Seen>> {
    let dialect = RawDialect {
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: feeding.engine,
    };
    let mut state = State::init(dialect).unwrap();
    let doors: Vec<Door> = specs
        .iter()
        .map(|spec| Door::from_code(spec.door, spec.param).unwrap())
        .collect();
    let mut values: Vec<Vec<u8>> = doors
        .iter()
        .map(|&door| vec![0xAA; value_size(door) * feeding.max_rows])
        .collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = doors
        .iter()
        .map(|_| vec![CellVerdict::default(); feeding.max_rows])
        .collect();
    let mut cells = vec![Span::default(); feeding.cells];
    let mut arena = vec![0u8; feeding.arena];

    let mut rows = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut read = 0usize;
    let mut calls = 0;
    loop {
        calls += 1;
        assert!(calls < 1_000_000, "the core is not making progress");
        let last = read == data.len();
        let columns: Vec<ColumnBuffer> = values
            .iter_mut()
            .zip(verdicts.iter_mut())
            .map(|(values, verdicts)| ColumnBuffer {
                values: values.as_mut_ptr().cast(),
                verdicts: verdicts.as_mut_ptr(),
            })
            .collect();
        let mut out = Filled::default();
        // SAFETY: every column was sized for `max_rows` of its door's type.
        let code = unsafe {
            fill::fill(
                &mut state,
                &pending,
                last,
                specs,
                &columns,
                feeding.max_rows,
                &mut cells,
                &mut arena,
                &mut out,
            )
        };
        match code {
            OK => {}
            ERR_CELLS => {
                assert_eq!((out.rows, out.consumed), (0, 0));
                cells.resize(out.needed as usize, Span::default());
                continue;
            }
            ERR_ARENA => {
                assert_eq!((out.rows, out.consumed), (0, 0));
                assert!(out.needed as usize > arena.len());
                arena.resize(out.needed as usize, 0);
                continue;
            }
            other => panic!("fill returned {other}: {:?}", out.failure),
        }
        assert!(out.arena_used as usize <= arena.len());
        for row in 0..out.rows as usize {
            let mut seen = Vec::new();
            for (index, &door) in doors.iter().enumerate() {
                let verdict = verdicts[index][row];
                if !verdict.is_ok() {
                    seen.push(Seen::Fault(verdict.reason, verdict.offset, verdict.len));
                    continue;
                }
                let v = &values[index];
                seen.push(Seen::Value(match door {
                    Door::Bool | Door::U8 => show::<u8>(v, row),
                    Door::I8 => show::<i8>(v, row),
                    Door::I16 => show::<i16>(v, row),
                    Door::I32 => show::<i32>(v, row),
                    Door::I64 => show::<i64>(v, row),
                    Door::U16 => show::<u16>(v, row),
                    Door::U32 => show::<u32>(v, row),
                    Door::U64 | Door::Time => show::<u64>(v, row),
                    Door::F32 => show::<f32>(v, row),
                    Door::F64 => show::<f64>(v, row),
                    Door::Decimal => show::<hypercast::Decimal>(v, row),
                    Door::Uuid => show::<[u8; 16]>(v, row),
                    Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => {
                        show::<hypercast::Timestamp>(v, row)
                    }
                    Door::Date | Door::DateOrdered(_) => show::<hypercast::Date>(v, row),
                    Door::DateTime(_) => show::<hypercast::CivilDateTime>(v, row),
                    Door::Duration => show::<hypercast::Duration>(v, row),
                    Door::Text => {
                        let at = row * size_of::<Span>();
                        // SAFETY: within the column's buffer; the read is unaligned.
                        let span: Span =
                            unsafe { v.as_ptr().add(at).cast::<Span>().read_unaligned() };
                        let from: &[u8] = if span.flagged() { &arena } else { &pending };
                        let start = span.offset as usize;
                        format!("{:?}", &from[start..start + span.len()])
                    }
                }));
            }
            rows.push(seen);
        }
        let consumed = out.consumed as usize;
        assert!(consumed <= pending.len());
        pending.drain(..consumed);
        if last {
            if pending.is_empty() {
                break;
            }
            assert!(out.rows > 0 || consumed > 0, "stalled on the final chunk");
        } else if out.rows == 0 && consumed == 0 {
            // An unfinished row: it wants more input behind it.
            let take = (1 + random.below(feeding.chunk)).min(data.len() - read);
            pending.extend_from_slice(&data[read..read + take]);
            read += take;
        }
    }
    assert_eq!(state.offset, data.len() as u64);
    rows
}

#[test]
fn every_door_agrees_with_the_reader_however_it_is_fed() {
    let mut random = Random(0x00C0_FFEE_D15E_A5E5);
    let all = doors();
    // Every engine the standard library finds must be one the core, asking the CPU
    // itself, agrees it can run.
    for kind in engine::available() {
        assert!(engine::usable(kind), "the core cannot run {kind:?}");
    }
    let engines: Vec<u8> = std::iter::once(0)
        .chain(engine::available().into_iter().map(|kind| kind.code()))
        .collect();
    for round in 0..300 {
        let columns = 1 + random.below(7);
        let rows = random.below(60);
        let data = generate(&mut random, rows, columns);
        // A plan in no particular order: repeats, gaps, and one ordinal past the width.
        let mut specs = Vec::new();
        for _ in 0..1 + random.below(10) {
            let (door, param) = all[random.below(all.len())];
            specs.push(ColumnSpec::new(
                random.below(columns + 1) as u32,
                door,
                param,
            ));
        }
        if round % 10 == 0 {
            specs = all
                .iter()
                .enumerate()
                .map(|(index, &(door, param))| {
                    ColumnSpec::new((index % columns) as u32, door, param)
                })
                .collect();
        }
        let expected = reference(&data, &specs);
        for &engine in &engines {
            let feeding = Feeding {
                chunk: [1, 3, 17, 64, 200, 100_000][random.below(6)],
                max_rows: [1, 2, 7, 1024][random.below(4)],
                cells: [0, 1, 40, 100_000][random.below(4)],
                arena: [0, 1, 16, 100_000][random.below(4)],
                engine,
            };
            let got = streamed(&data, &specs, &feeding, &mut random);
            assert_eq!(
                got,
                expected,
                "round {round}, engine {engine}, chunk {}, max_rows {}, cells {}, arena {}\n{:?}",
                feeding.chunk,
                feeding.max_rows,
                feeding.cells,
                feeding.arena,
                String::from_utf8_lossy(&data)
            );
        }
    }
}

#[allow(clippy::type_complexity)]
fn call(
    state: &mut State,
    input: &[u8],
    last: bool,
    specs: &[ColumnSpec],
    max_rows: usize,
) -> (i32, Filled, Vec<Vec<u8>>, Vec<Vec<CellVerdict>>, Vec<Span>) {
    let mut values: Vec<Vec<u8>> = specs
        .iter()
        .map(|_| vec![0u8; 16 * max_rows.max(1)])
        .collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = specs
        .iter()
        .map(|_| vec![CellVerdict::default(); max_rows.max(1)])
        .collect();
    let columns: Vec<ColumnBuffer> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        })
        .collect();
    let mut cells = vec![Span::default(); 4096];
    let mut arena = vec![0u8; 4096];
    let mut out = Filled::default();
    // SAFETY: sixteen bytes a row covers every door's value.
    let code = unsafe {
        fill::fill(
            state, input, last, specs, &columns, max_rows, &mut cells, &mut arena, &mut out,
        )
    };
    (code, out, values, verdicts, cells)
}

const CSV: RawDialect = RawDialect {
    separator: b',',
    quoting: 1,
    skip_blank_lines: 1,
    engine: 0,
};

#[test]
fn structural_failures_come_after_the_rows_before_them_and_stay() {
    let specs = [ColumnSpec::new(0, 4, 0), ColumnSpec::new(1, 18, 0)];

    // A record with the wrong width: the two intact rows first, then the failure.
    let text = b"1,a\n2,b\n3\n4,d\n";
    let mut state = State::init(CSV).unwrap();
    let (code, out, ..) = call(&mut state, text, true, &specs, 100);
    assert_eq!((code, out.rows, out.consumed), (OK, 2, 8));
    let (code, out, ..) = call(&mut state, &text[8..], true, &specs, 100);
    assert_eq!(code, ERR_STRUCTURE);
    let expected = Failure {
        code: Failure::COLUMN_COUNT,
        line: 3,
        record: 2,
        byte: 8,
        expected: 2,
        found: 1,
    };
    assert_eq!(out.failure, expected);
    // Final: whatever it is handed next, the same answer.
    let (code, out, ..) = call(&mut state, b"5,e\n", true, &specs, 100);
    assert_eq!((code, out.failure, out.rows), (ERR_STRUCTURE, expected, 0));

    // The input ends inside a quoted cell: the intact row, then the failure.
    let text = b"1,a\n2,\"never closed\n";
    let mut state = State::init(CSV).unwrap();
    let (code, out, ..) = call(&mut state, text, true, &specs, 100);
    assert_eq!((code, out.rows, out.consumed), (OK, 1, 4));
    let (code, out, ..) = call(&mut state, &text[4..], true, &specs, 100);
    assert_eq!(code, ERR_STRUCTURE);
    assert_eq!(
        (
            out.failure.code,
            out.failure.line,
            out.failure.record,
            out.failure.byte
        ),
        (Failure::UNCLOSED_QUOTE, 2, 1, 4)
    );
    // The same bytes with more to come are only an unfinished row.
    let mut state = State::init(CSV).unwrap();
    let (code, out, ..) = call(&mut state, &text[4..], false, &specs, 100);
    assert_eq!((code, out.rows, out.consumed), (OK, 0, 0));
}

#[test]
fn caller_bugs_are_contract_violations_and_room_is_asked_for_by_size() {
    let good = [ColumnSpec::new(0, 4, 0)];
    assert!(
        State::init(RawDialect {
            separator: b'"',
            ..CSV
        })
        .is_none()
    );
    assert!(
        State::init(RawDialect {
            separator: b'\n',
            ..CSV
        })
        .is_none()
    );
    assert!(State::init(RawDialect { engine: 99, ..CSV }).is_none());

    let mut state = State::init(CSV).unwrap();
    for bad in [
        ColumnSpec::new(0, 0, 0),
        ColumnSpec::new(0, 23, 0),
        ColumnSpec::new(0, 14, 0),
        ColumnSpec::new(0, 20, 9),
    ] {
        let (code, out, ..) = call(&mut state, b"1\n", true, &[bad], 10);
        assert_eq!((code, out.consumed), (ERR_CONTRACT, 0));
    }
    let mut colliding = ColumnSpec::new(0, 11, 0);
    colliding.format.decimal_sep = u32::from(',');
    colliding.format.group_sep = u32::from(',');
    let (code, ..) = call(&mut state, b"1\n", true, &[colliding], 10);
    assert_eq!(code, ERR_CONTRACT);

    // No rows asked for: nothing done, nothing wrong.
    let (code, out, ..) = call(&mut state, b"1\n", true, &good, 0);
    assert_eq!((code, out.rows, out.consumed), (OK, 0, 0));

    // A table too small for one row, and an arena too small for one row's escapes: each
    // says what one row takes, and takes nothing.
    let wide = [ColumnSpec::new(4, 18, 0)];
    let mut values = vec![0u8; 64];
    let mut verdicts = vec![CellVerdict::default(); 4];
    let columns = [ColumnBuffer {
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
    }];
    let text = b"a,b,c,d,\"e\"\"e\"\n";
    let mut out = Filled::default();
    let mut cells = vec![Span::default(); 5];
    let mut arena = [0u8; 2];
    // SAFETY: four rows of sixteen bytes.
    let code = unsafe {
        fill::fill(
            &mut state, text, true, &wide, &columns, 4, &mut cells, &mut arena, &mut out,
        )
    };
    assert_eq!((code, out.needed, out.consumed), (ERR_CELLS, 6, 0));
    cells.resize(6, Span::default());
    let code = unsafe {
        fill::fill(
            &mut state, text, true, &wide, &columns, 4, &mut cells, &mut arena, &mut out,
        )
    };
    assert_eq!((code, out.needed, out.consumed), (ERR_ARENA, 6, 0));
    let mut arena = [0u8; 6];
    let code = unsafe {
        fill::fill(
            &mut state, text, true, &wide, &columns, 4, &mut cells, &mut arena, &mut out,
        )
    };
    assert_eq!(
        (code, out.rows, out.consumed, out.arena_used),
        (OK, 1, 15, 3)
    );
    assert_eq!(&arena[..3], b"e\"e");
    // The table is the caller's map of the row: each cell's place in the input, the
    // escaped one flagged, then the row's own start and line.
    assert_eq!(cells[0], Span { offset: 0, len: 1 });
    assert_eq!(
        cells[4],
        Span {
            offset: 8,
            len: 6 | Span::FLAG
        }
    );
    assert_eq!(cells[5], Span { offset: 0, len: 1 });
}

#[test]
fn the_header_is_one_record_located_like_text() {
    let text = b"\xEF\xBB\xBF\r\n\nid,\"full name\",\"say \"\"hi\"\"\"\r\n1,a,b\n";
    let mut state = State::init(CSV).unwrap();
    let mut out = Filled::default();
    let mut arena = [0u8; 64];

    // Too few names: told how many, nothing consumed.
    let mut names = [Span::default(); 2];
    assert_eq!(
        fill::header(&mut state, text, true, &mut names, &mut arena, &mut out),
        ERR_CELLS
    );
    assert_eq!((out.needed, out.consumed, state.offset), (3, 0, 0));

    let mut names = [Span::default(); 8];
    assert_eq!(
        fill::header(&mut state, text, true, &mut names, &mut [], &mut out),
        ERR_ARENA
    );
    assert_eq!((out.needed, out.consumed), (12, 0));

    assert_eq!(
        fill::header(&mut state, text, true, &mut names, &mut arena, &mut out),
        OK
    );
    assert_eq!((out.rows, out.arena_used), (3, 8));
    let name = |span: Span| {
        let from: &[u8] = if span.flagged() { &arena } else { text };
        &from[span.offset as usize..span.offset as usize + span.len()]
    };
    assert_eq!(name(names[0]), b"id");
    assert_eq!(name(names[1]), b"full name");
    assert_eq!(name(names[2]), b"say \"hi\"");
    // The byte-order mark and the blank lines are behind it, and the width is fixed.
    let rest = &text[out.consumed as usize..];
    assert_eq!(rest, b"1,a,b\n");
    assert_eq!((state.expected, state.records, state.line), (3, 3, 4));

    let specs = [ColumnSpec::new(0, 4, 0), ColumnSpec::new(2, 18, 0)];
    let (code, mut out, values, verdicts, _) = call(&mut state, rest, true, &specs, 10);
    assert_eq!((code, out.rows), (OK, 1));
    assert_eq!(i32::from_le_bytes(values[0][..4].try_into().unwrap()), 1);
    assert!(verdicts[0][0].is_ok() && verdicts[1][0].is_ok());

    // An input with no record has no header, and says so by having no names.
    let mut state = State::init(CSV).unwrap();
    assert_eq!(
        fill::header(&mut state, b"\n\n", true, &mut names, &mut arena, &mut out),
        OK
    );
    assert_eq!((out.rows, out.consumed), (0, 2));
}
