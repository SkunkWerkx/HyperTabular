//! Delimited text, any bytes, any dialect: the core must never panic, never stall, never
//! hand back a span outside the buffer it names, and must read the same rows — the same
//! cells, the same values, the same verdicts, the same failure — whichever engine scans
//! the input, however the input is cut into chunks, and however little room it is given.
//!
//! The leading five bytes choose the separator, the quoting and blank-line flags, whether
//! a header is read, the batch size, the chunk size and a small plan (two to four columns,
//! each an ordinal and a door); the rest is the input. One reading takes it whole, with the
//! best engine this CPU has and room to spare; the other takes it in chunks, on the
//! portable engine, with buffers that start empty and grow only when a call asks.

#![no_main]

use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, Failure, Filled,
    OK, Span,
};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::kernel::door::Door;
use hypertabular::{CivilDateTime, Date, Decimal, Duration, Timestamp};
use libfuzzer_sys::fuzz_target;

/// The most room a reading may be asked for before the target calls it a runaway: far
/// more than any input libFuzzer hands over can need.
const ROOM_LIMIT: usize = 1 << 24;

/// One cell as a reading saw it: the verdict, and the value when it is one — a text
/// cell's own bytes, wherever its span points, and any other value as its type prints it.
/// Not the value's raw bytes: a struct's padding is not written, and differs.
type Cell = (u32, u32, u32, Vec<u8>);

/// What a reading made of the input: the header's names, the rows, and the failure that
/// ended it, if one did.
#[derive(Debug, PartialEq, Eq)]
struct Reading {
    names: Option<Vec<Vec<u8>>>,
    rows: Vec<Vec<Cell>>,
    failure: Option<Failure>,
}

#[derive(Clone, Copy)]
struct Feeding {
    engine: u8,
    chunk: usize,
    max_rows: usize,
    roomy: bool,
}

fn size_of_door(door: Door) -> usize {
    use core::mem::size_of;
    match door {
        Door::Bool | Door::I8 | Door::U8 => 1,
        Door::I16 | Door::U16 => 2,
        Door::I32 | Door::U32 | Door::F32 => 4,
        Door::I64 | Door::U64 | Door::F64 | Door::Time => 8,
        Door::Uuid => 16,
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => size_of::<Timestamp>(),
        Door::Date | Door::DateOrdered(_) => size_of::<Date>(),
        Door::DateTime(_) => size_of::<CivilDateTime>(),
        Door::Duration => size_of::<Duration>(),
        Door::Decimal => size_of::<Decimal>(),
        Door::Text => size_of::<Span>(),
    }
}

/// The value at `row` of a column of `door`'s type, as that type prints it.
fn show(door: Door, values: &[u8], row: usize) -> String {
    fn at<T: Copy + core::fmt::Debug>(values: &[u8], row: usize) -> String {
        let size = size_of::<T>();
        let bytes = &values[row * size..(row + 1) * size];
        // SAFETY: the bytes are one `T` the core wrote, read unaligned.
        format!("{:?}", unsafe {
            bytes.as_ptr().cast::<T>().read_unaligned()
        })
    }
    match door {
        Door::Bool => format!("{:?}", values[row] != 0),
        Door::I8 => at::<i8>(values, row),
        Door::I16 => at::<i16>(values, row),
        Door::I32 => at::<i32>(values, row),
        Door::I64 => at::<i64>(values, row),
        Door::U8 => at::<u8>(values, row),
        Door::U16 => at::<u16>(values, row),
        Door::U32 => at::<u32>(values, row),
        Door::U64 | Door::Time => at::<u64>(values, row),
        // By bits, so that a NaN equals itself.
        Door::F32 => format!("{:?}", at::<u32>(values, row)),
        Door::F64 => format!("{:?}", at::<u64>(values, row)),
        Door::Uuid => at::<[u8; 16]>(values, row),
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => at::<Timestamp>(values, row),
        Door::Date | Door::DateOrdered(_) => at::<Date>(values, row),
        Door::DateTime(_) => at::<CivilDateTime>(values, row),
        Door::Duration => at::<Duration>(values, row),
        Door::Decimal => at::<Decimal>(values, row),
        Door::Text => unreachable!("text is resolved through its span"),
    }
}

/// The bytes a span names, checked against the buffer it names: a span that escapes it
/// is the bug this target exists to find.
fn resolve(span: Span, input: &[u8], arena: &[u8], used: usize) -> Vec<u8> {
    let (from, bound) = if span.flagged() {
        (arena, used)
    } else {
        (input, input.len())
    };
    let start = span.offset as usize;
    let end = start + span.len();
    assert!(
        end <= bound,
        "a span {start}..{end} escapes its {bound}-byte buffer"
    );
    from[start..end].to_vec()
}

fn grow<T: Copy>(buffer: &mut Vec<T>, needed: u64, fill: T) {
    let needed = needed as usize;
    assert!(
        needed > buffer.len(),
        "asked for {needed} with {} already",
        buffer.len()
    );
    assert!(needed <= ROOM_LIMIT, "asked for {needed}");
    buffer.resize(needed, fill);
}

#[allow(clippy::needless_range_loop)]
fn read(
    data: &[u8],
    dialect: RawDialect,
    header: bool,
    specs: &[ColumnSpec],
    feeding: Feeding,
) -> Reading {
    let mut state = State::init(RawDialect {
        engine: feeding.engine,
        ..dialect
    })
    .expect("the separator was chosen valid");
    let doors: Vec<Door> = specs
        .iter()
        .map(|spec| Door::from_code(spec.door, spec.param).expect("a door the plan can name"))
        .collect();
    let max_rows = feeding.max_rows;
    let mut values: Vec<Vec<u8>> = doors
        .iter()
        .map(|&door| vec![0xAA; size_of_door(door) * max_rows])
        .collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = doors
        .iter()
        .map(|_| vec![CellVerdict::default(); max_rows])
        .collect();
    let room = if feeding.roomy { 1 << 16 } else { 0 };
    let mut cells = vec![Span::default(); room];
    let mut arena = vec![0x55u8; room];

    let mut reading = Reading {
        names: None,
        rows: Vec::new(),
        failure: None,
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut read = 0usize;
    let feed = |pending: &mut Vec<u8>, read: &mut usize| {
        let take = feeding.chunk.min(data.len() - *read);
        pending.extend_from_slice(&data[*read..*read + take]);
        *read += take;
    };
    feed(&mut pending, &mut read);
    let mut calls = 0u32;

    if header {
        loop {
            calls += 1;
            assert!(calls < 1_000_000, "the header is not making progress");
            let last = read == data.len();
            pending.shrink_to_fit();
            let mut out = Filled::default();
            let code = fill::header(&mut state, &pending, last, &mut cells, &mut arena, &mut out);
            match code {
                OK => {}
                ERR_CELLS => {
                    grow(&mut cells, out.needed, Span::default());
                    continue;
                }
                ERR_ARENA => {
                    grow(&mut arena, out.needed, 0x55);
                    continue;
                }
                ERR_STRUCTURE => {
                    reading.failure = Some(out.failure);
                    return reading;
                }
                other => panic!("header returned {other}"),
            }
            let consumed = out.consumed as usize;
            assert!(consumed <= pending.len());
            if out.rows > 0 || (last && pending.len() == consumed) {
                let used = out.arena_used as usize;
                assert!(used <= arena.len());
                reading.names = Some(
                    (0..out.rows as usize)
                        .map(|index| resolve(cells[index], &pending, &arena, used))
                        .collect(),
                );
                pending.drain(..consumed);
                break;
            }
            pending.drain(..consumed);
            if consumed == 0 {
                assert!(!last, "the header stalled on the final chunk");
                feed(&mut pending, &mut read);
            }
        }
    }

    loop {
        calls += 1;
        assert!(calls < 1_000_000, "the core is not making progress");
        let last = read == data.len();
        pending.shrink_to_fit();
        let columns: Vec<ColumnBuffer> = values
            .iter_mut()
            .zip(verdicts.iter_mut())
            .map(|(values, verdicts)| ColumnBuffer {
                values: values.as_mut_ptr().cast(),
                verdicts: verdicts.as_mut_ptr(),
            })
            .collect();
        let mut out = Filled::default();
        // SAFETY: every column holds `max_rows` elements of its door's type.
        let code = unsafe {
            fill::fill(
                &mut state, &pending, last, specs, &columns, max_rows, &mut cells, &mut arena,
                &mut out,
            )
        };
        let failed = match code {
            OK => false,
            ERR_STRUCTURE => true,
            ERR_CELLS => {
                assert_eq!((out.rows, out.consumed), (0, 0), "a refusal reads nothing");
                grow(&mut cells, out.needed, Span::default());
                continue;
            }
            ERR_ARENA => {
                assert_eq!((out.rows, out.consumed), (0, 0), "a refusal reads nothing");
                grow(&mut arena, out.needed, 0x55);
                continue;
            }
            other => panic!("fill returned {other}"),
        };
        assert!(out.rows as usize <= max_rows);
        let used = out.arena_used as usize;
        assert!(used <= arena.len());
        for row in 0..out.rows as usize {
            let mut seen = Vec::with_capacity(doors.len());
            for (index, &door) in doors.iter().enumerate() {
                let verdict = verdicts[index][row];
                let bytes = if !verdict.is_ok() {
                    Vec::new()
                } else if door == Door::Text {
                    let at = row * size_of::<Span>();
                    // SAFETY: within the column's buffer; read unaligned.
                    let span: Span = unsafe {
                        values[index]
                            .as_ptr()
                            .add(at)
                            .cast::<Span>()
                            .read_unaligned()
                    };
                    resolve(span, &pending, &arena, used)
                } else {
                    show(door, &values[index], row).into_bytes()
                };
                seen.push((verdict.reason, verdict.offset, verdict.len, bytes));
            }
            reading.rows.push(seen);
        }
        let consumed = out.consumed as usize;
        assert!(consumed <= pending.len());
        pending.drain(..consumed);
        if failed {
            assert_ne!(out.failure.code, 0, "a failure names itself");
            reading.failure = Some(out.failure);
            return reading;
        }
        if last {
            if pending.is_empty() {
                return reading;
            }
            assert!(out.rows > 0 || consumed > 0, "stalled on the final chunk");
        } else if out.rows == 0 && consumed == 0 {
            feed(&mut pending, &mut read);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let [dialect_sel, sizes, plan_sel, plan_a, plan_b, input @ ..] = data else {
        return;
    };
    let separators = *b",\t;| a'";
    let dialect = RawDialect {
        separator: separators[usize::from(dialect_sel & 7) % separators.len()],
        quoting: (dialect_sel >> 3) & 1,
        skip_blank_lines: (dialect_sel >> 4) & 1,
        engine: 0,
    };
    let header = dialect_sel & 0x20 != 0;
    let max_rows = 1 + usize::from(sizes & 0x0F);
    let chunk = 1 + usize::from(sizes >> 4) * 7;

    // Two to four columns, each an ordinal below six and one of the 22 doors, with the
    // parameter the door takes.
    let width = 2 + usize::from(plan_sel & 3) % 3;
    let picks = [
        *plan_a,
        *plan_b,
        plan_a.rotate_left(3),
        plan_b.rotate_left(5),
    ];
    let specs: Vec<ColumnSpec> = picks[..width]
        .iter()
        .map(|&pick| {
            let code = 1 + u32::from(pick) % 22;
            let param = match code {
                14 => 1 + u32::from(pick >> 5) % 4,
                20 | 21 => 1 + u32::from(pick >> 5) % 3,
                22 => 1 + u32::from(pick >> 5) % 2,
                _ => 0,
            };
            ColumnSpec::new(u32::from(pick >> 2) % 6, code, param)
        })
        .collect();

    let whole = read(
        input,
        dialect,
        header,
        &specs,
        Feeding {
            engine: 0,
            chunk: input.len(),
            max_rows: 64,
            roomy: true,
        },
    );
    let pieces = read(
        input,
        dialect,
        header,
        &specs,
        Feeding {
            engine: 1,
            chunk,
            max_rows,
            roomy: false,
        },
    );
    assert_eq!(whole, pieces);
});
