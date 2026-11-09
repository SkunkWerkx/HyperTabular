//! The allocation-free core against a reference that owes it nothing: the `csv` crate
//! (BurntSushi's csv-core underneath) says which cells a file has, and HyperCast's own
//! doors, asked directly, say what each cell's text is worth. The core has to land on the
//! same cells, the same values and the same verdicts — for every door, on every engine
//! this CPU can run, however the input is cut into chunks, and however little room it is
//! given.
//!
//! The files are standard RFC 4180: a cell is either unquoted, or quoted from its first
//! byte to its last with `""` for a literal quote — the quoting `csv` and this crate
//! agree on. (Both are lenient past that, in different ways. What this crate does with a
//! stray quote is pinned by hand in `corpus/delimited.json`, and held at the end of this file
//! to a byte-at-a-time state machine that owes the walker nothing.)

use hypercast::{Fault, Grouping, NumFormat, RawNumFormat};
use hypertabular::Door;
use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE,
    Failure, Filled, OK, Span,
};
use hypertabular::kernel::delimited::engine::{self, Kind};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::kernel::delimited::scan::{Flow, Scanner, Sink};
use hypertabular::kernel::delimited::unescape::unescape_into;
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

/// Every engine this build can run, as the standard library's own detection finds them.
fn available() -> Vec<Kind> {
    let mut kinds = Vec::new();
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("aes") {
            kinds.push(Kind::NeonPmull);
        }
        kinds.push(Kind::Neon);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let clmul = std::arch::is_x86_feature_detected!("pclmulqdq");
        if std::arch::is_x86_feature_detected!("avx2") && clmul {
            kinds.push(Kind::Avx2Clmul);
        }
        if clmul {
            kinds.push(Kind::Sse2Clmul);
        }
        kinds.push(Kind::Sse2);
    }
    kinds.push(Kind::Swar);
    kinds
}

/// What HyperCast makes of a cell's text through `door` — or of no cell at all, for an
/// ordinal past the record's width.
fn judge(door: Door, cell: Option<&[u8]>) -> Seen {
    fn seen<T: Debug>(verdict: Result<T, Fault>) -> Seen {
        match verdict {
            Ok(value) => Seen::Value(format!("{value:?}")),
            Err(fault) => Seen::Fault(fault.reason as u32, fault.offset, fault.len),
        }
    }
    let Some(text) = cell else {
        return Seen::Fault(1, 0, 0);
    };
    let format = &NumFormat::INVARIANT;
    match door {
        Door::Bool => seen(hypercast::cast_bool(text).map(u8::from)),
        Door::I8 => seen(hypercast::cast_i8(text, format)),
        Door::I16 => seen(hypercast::cast_i16(text, format)),
        Door::I32 => seen(hypercast::cast_i32(text, format)),
        Door::I64 => seen(hypercast::cast_i64(text, format)),
        Door::U8 => seen(hypercast::cast_u8(text, format)),
        Door::U16 => seen(hypercast::cast_u16(text, format)),
        Door::U32 => seen(hypercast::cast_u32(text, format)),
        Door::U64 => seen(hypercast::cast_u64(text, format)),
        Door::F32 => seen(hypercast::cast_f32(text, format)),
        Door::F64 => seen(hypercast::cast_f64(text, format)),
        Door::Decimal => seen(hypercast::cast_decimal(text, format)),
        Door::Uuid => seen(hypercast::cast_uuid(text)),
        Door::Timestamp => seen(hypercast::cast_timestamp(text)),
        Door::Unix(precision) => seen(hypercast::cast_unix(text, precision)),
        Door::ExcelSerial(epoch) => seen(hypercast::cast_excel_serial(text, epoch)),
        Door::Date => seen(hypercast::cast_date(text)),
        Door::DateOrdered(order) => seen(hypercast::cast_date_ordered(text, order)),
        Door::DateTime(order) => seen(hypercast::cast_datetime(text, order)),
        Door::Time => seen(hypercast::cast_time(text)),
        Door::Duration => seen(hypercast::cast_duration(text)),
        // The bytes themselves; no bytes at all is the one way to fail.
        Door::Text if text.is_empty() => Seen::Fault(1, 0, 0),
        Door::Text => Seen::Value(format!("{text:?}")),
    }
}

/// What the reference says of every cell the plan reads, row by row.
fn reference(data: &[u8], specs: &[ColumnSpec]) -> Vec<Vec<Seen>> {
    // A byte-order mark is not part of the first cell; `csv` leaves it there.
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(data);
    let mut rows = Vec::new();
    for record in reader.byte_records() {
        let record = record.unwrap();
        rows.push(
            specs
                .iter()
                .map(|spec| {
                    let door = Door::from_code(spec.door, spec.param).unwrap();
                    judge(door, record.get(spec.ordinal as usize))
                })
                .collect(),
        );
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
fn every_door_agrees_with_the_reference_however_it_is_fed() {
    let mut random = Random(0x00C0_FFEE_D15E_A5E5);
    let all = doors();
    // Every engine the standard library finds must be one the core, asking the CPU
    // itself, agrees it can run.
    for kind in available() {
        assert!(engine::usable(kind), "the core cannot run {kind:?}");
    }
    let engines: Vec<u8> = std::iter::once(0)
        .chain(available().into_iter().map(|kind| kind.code()))
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
    colliding.format = RawNumFormat::from(NumFormat::new(',', ',', NumFormat::ALL));
    let (code, ..) = call(&mut state, b"1\n", true, &[colliding], 10);
    assert_eq!(code, ERR_CONTRACT);
    // A layout whose size the core does not read is a contract violation too (a size of
    // zero, as `ColumnSpec::new` builds, is the invariant notation, not one).
    let mut sized = ColumnSpec::new(0, 11, 0);
    sized.format = RawNumFormat::from(NumFormat::INVARIANT);
    sized.format.size = 12;
    let (code, ..) = call(&mut state, b"1\n", true, &[sized], 10);
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
    assert_eq!(i32::from_ne_bytes(values[0][..4].try_into().unwrap()), 1);
    assert!(verdicts[0][0].is_ok() && verdicts[1][0].is_ok());

    // An input with no record has no header, and says so by having no names.
    let mut state = State::init(CSV).unwrap();
    assert_eq!(
        fill::header(&mut state, b"\n\n", true, &mut names, &mut arena, &mut out),
        OK
    );
    assert_eq!((out.rows, out.consumed), (0, 2));
}

/// Cells as delivered, after unescape: the spec's table for where a quote decides a boundary.
const QUOTE_CASES: &[(&str, &[&[&str]])] = &[
    ("\"x\"y\"z\"\n", &[&["x\"y\"z"]]),
    ("\"x\"y\"z,w\"\n", &[&["x\"y\"z", "w\""]]),
    ("\"x\"y\"\"z\"\n", &[&["x\"y\"z"]]),
    (
        "12\" pipe,1\n3,4\n6\" pipe,2\n",
        &[&["12\" pipe", "1"], &["3", "4"], &["6\" pipe", "2"]],
    ),
    ("12\" pipe,1\n", &[&["12\" pipe", "1"]]),
];

fn owned(rows: &[&[&str]]) -> Vec<Vec<Vec<u8>>> {
    rows.iter()
        .map(|row| row.iter().map(|cell| cell.as_bytes().to_vec()).collect())
        .collect()
}

/// Every row of `data` through the core and the text door: chunks end at `splits`, then the
/// input ends. The structural failure, if there is one, comes back with the rows before it.
#[allow(clippy::needless_range_loop)]
fn delivered(
    data: &[u8],
    kind: Kind,
    width: usize,
    splits: &[usize],
) -> (Vec<Vec<Vec<u8>>>, Option<u32>) {
    let dialect = RawDialect {
        engine: kind.code(),
        ..CSV
    };
    let mut state = State::init(dialect).unwrap();
    let specs: Vec<ColumnSpec> = (0..width as u32)
        .map(|ordinal| ColumnSpec::new(ordinal, 18, 0))
        .collect();
    const MAX_ROWS: usize = 8;
    let mut values = vec![vec![0u8; size_of::<Span>() * MAX_ROWS]; width];
    let mut verdicts = vec![vec![CellVerdict::default(); MAX_ROWS]; width];
    let mut cells = vec![Span::default(); (width + 1) * MAX_ROWS];
    let mut arena = vec![0u8; data.len().max(1)];
    let mut rows = Vec::new();
    let mut at = 0;
    for &end in splits.iter().chain([&data.len()]) {
        loop {
            let last = end == data.len();
            let input = &data[at..end];
            let columns: Vec<ColumnBuffer> = values
                .iter_mut()
                .zip(verdicts.iter_mut())
                .map(|(values, verdicts)| ColumnBuffer {
                    values: values.as_mut_ptr().cast(),
                    verdicts: verdicts.as_mut_ptr(),
                })
                .collect();
            let mut out = Filled::default();
            // SAFETY: every column holds MAX_ROWS spans.
            let code = unsafe {
                fill::fill(
                    &mut state, input, last, &specs, &columns, MAX_ROWS, &mut cells, &mut arena,
                    &mut out,
                )
            };
            if code == ERR_STRUCTURE {
                return (rows, Some(out.failure.code));
            }
            assert_eq!(code, OK, "{:?}", String::from_utf8_lossy(data));
            for row in 0..out.rows as usize {
                let row_cells = (0..width).map(|column| {
                    // The text door's one refusal is an empty cell, and it writes no span.
                    if verdicts[column][row].reason == 1 {
                        return Vec::new();
                    }
                    assert!(verdicts[column][row].is_ok());
                    // SAFETY: within the column's buffer; the read is unaligned.
                    let span: Span = unsafe {
                        values[column]
                            .as_ptr()
                            .add(row * size_of::<Span>())
                            .cast::<Span>()
                            .read_unaligned()
                    };
                    let from: &[u8] = if span.flagged() { &arena } else { input };
                    let start = span.offset as usize;
                    from[start..start + span.len()].to_vec()
                });
                rows.push(row_cells.collect());
            }
            at += out.consumed as usize;
            if out.rows == 0 {
                break;
            }
        }
    }
    (rows, None)
}

#[test]
fn quotes_decide_boundaries_only() {
    for &(input, expected) in QUOTE_CASES {
        for kind in available() {
            let width = expected[0].len();
            assert_eq!(
                delivered(input.as_bytes(), kind, width, &[]),
                (owned(expected), None),
                "{input:?} on {kind:?}"
            );
        }
    }
}

/// `unescape.rs` is not part of the change: this is its whole rule, stated on the cells the
/// new boundaries hand it.
#[test]
fn quotes_leave_unescaping_as_it_was() {
    for (cell, expected) in [
        ("\"x\"y\"z\"", "x\"y\"z"),
        ("\"x\"y\"z", "x\"y\"z"),
        ("\"x\"y\"\"z\"", "x\"y\"z"),
        ("\"a\"\"b\"", "a\"b"),
        ("\"a\"b\"", "a\"b"),
        ("\"abc", "abc"),
        ("\"a\"", "a"),
        ("\"\"\"\"", "\""),
    ] {
        let mut out = [0u8; 16];
        let written = unescape_into(cell.as_bytes(), &mut out);
        assert_eq!(&out[..written], expected.as_bytes(), "{cell:?}");
    }
}

/// Each case behind a row of every length that puts each of its quotes on each of the 64
/// block offsets, and through every split point: the same rows.
#[test]
fn quotes_at_every_split() {
    for &(input, expected) in QUOTE_CASES {
        let width = expected[0].len();
        for pad in 0..64 {
            let mut data = vec![b'p'; pad + 1];
            data.extend(std::iter::repeat_n(b',', width - 1));
            data.push(b'\n');
            let first = data.clone();
            data.extend_from_slice(input.as_bytes());
            let mut rows = vec![vec![b"p".repeat(pad + 1)]];
            rows[0].resize(width, Vec::new());
            rows.extend(owned(expected));
            for kind in available() {
                assert_eq!(
                    delivered(&data, kind, width, &[]),
                    (rows.clone(), None),
                    "{input:?}, pad {pad}, on {kind:?}"
                );
                for split in first.len()..data.len() {
                    assert_eq!(
                        delivered(&data, kind, width, &[split]),
                        (rows.clone(), None),
                        "{input:?}, pad {pad}, split {split}, on {kind:?}"
                    );
                }
            }
        }
    }
}

/// A quoted cell and a stray quote that run past block edges, with the interesting quote on
/// every offset of the block it falls in: a close, a doubled pair across the edge, a stray
/// after the close, and a separator the stray must not hide.
#[test]
fn quotes_cross_block_edges() {
    for pad in 0..130 {
        let run = "a".repeat(pad);
        let unquoted = |text: String| text.into_bytes();
        for (input, expected) in [
            (
                format!("\"{run}\"y\"z,w\"\n"),
                [format!("{run}\"y\"z"), "w\"".into()],
            ),
            (
                format!("\"{run}\"\"z\",w\n"),
                [format!("{run}\"z"), "w".into()],
            ),
            (
                format!("\"{run},\n\",w\n"),
                [format!("{run},\n"), "w".into()],
            ),
            (
                format!("b{run}\",\"x\"\n"),
                [format!("b{run}\""), "x".into()],
            ),
            (format!("\"{run}\",\"x\"y\n"), [run.clone(), "x\"y".into()]),
        ] {
            let rows = vec![expected.map(unquoted).to_vec()];
            let data = input.as_bytes();
            for kind in available() {
                assert_eq!(
                    delivered(data, kind, 2, &[]),
                    (rows.clone(), None),
                    "{input:?}"
                );
            }
        }
    }
}

#[test]
fn an_unclosed_quoted_cell_still_fails() {
    for input in [
        "\"abc",
        "\"a\"\"",
        "\"a\nb",
        "x,\"abc",
        "\"a\"b,\"",
        "a\"\n\"b",
    ] {
        for kind in available() {
            let width = if input.starts_with('x') || input.contains(',') {
                2
            } else {
                1
            };
            let (rows, failure) = delivered(input.as_bytes(), kind, width, &[]);
            assert_eq!(
                failure,
                Some(Failure::UNCLOSED_QUOTE),
                "{input:?} on {kind:?}"
            );
            assert_eq!(
                rows.len(),
                usize::from(input == "a\"\n\"b"),
                "{input:?} on {kind:?}"
            );
        }
    }
    // A stray quote outside a quoted cell is an ordinary byte, however many there are.
    assert_eq!(
        delivered(b"a\"", Kind::Swar, 1, &[]),
        (vec![vec![b"a\"".to_vec()]], None)
    );
}

/// What a sink is told, to compare it with the reference's account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Account {
    cells: Vec<(usize, usize, u32)>,
    rows: Vec<(usize, usize, u32, u8, usize)>,
    pending: usize,
    stop_after_each: bool,
    /// Refuse every this-many rows; 0 for never.
    refuse_every: usize,
    offered: usize,
}

impl Sink for Account {
    fn cell(&mut self, start: usize, end: usize, quotes: u32) {
        self.cells.push((start, end, quotes));
        self.pending += 1;
    }

    fn discard(&mut self) {
        self.cells.truncate(self.cells.len() - self.pending);
        self.pending = 0;
    }

    fn row(&mut self, start: usize, next: usize, line: u32, terminator: u8) -> Flow {
        self.offered += 1;
        if self.refuse_every != 0 && self.offered.is_multiple_of(self.refuse_every) {
            self.discard();
            return Flow::Refused;
        }
        self.rows
            .push((start, next, line, terminator, self.pending));
        self.pending = 0;
        if self.stop_after_each {
            Flow::Full
        } else {
            Flow::More
        }
    }
}

/// The rule, a byte at a time, in the plainest terms it has: `(account, final line, unclosed)`.
fn reference_walk(data: &[u8], quoting: bool) -> (Account, u32, bool) {
    #[derive(PartialEq)]
    enum Cell {
        First,
        Plain,
        Open,
        After,
    }
    let mut account = Account::default();
    let (mut at, mut row_start, mut cell_start, mut quotes, mut line) = (0, 0, 0, 0, 1);
    let mut row_line = 1;
    let mut cell = Cell::First;
    while at < data.len() {
        let byte = data[at];
        if cell == Cell::Open {
            match byte {
                b'"' if data.get(at + 1) == Some(&b'"') => {
                    quotes += 2;
                    at += 2;
                    continue;
                }
                b'"' => {
                    quotes += 1;
                    cell = Cell::After;
                }
                b'\n' => line += 1,
                _ => {}
            }
            at += 1;
            continue;
        }
        match byte {
            b'"' if quoting => {
                quotes += 1;
                if cell == Cell::First {
                    cell = Cell::Open;
                }
            }
            b',' => {
                account.cell(cell_start, at, quotes);
                quotes = 0;
                cell_start = at + 1;
                cell = Cell::First;
                at += 1;
                continue;
            }
            b'\n' | b'\r' => {
                account.cell(cell_start, at, quotes);
                quotes = 0;
                let terminator = if byte == b'\r' && data.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                let next = at + terminator;
                account.row(row_start, next, row_line, terminator as u8);
                line += 1;
                row_line = line;
                row_start = next;
                cell_start = next;
                cell = Cell::First;
                at = next;
                continue;
            }
            _ => {
                if cell == Cell::First {
                    cell = Cell::Plain;
                }
            }
        }
        at += 1;
    }
    if cell == Cell::Open {
        account.discard();
        return (account, row_line, true);
    }
    if row_start < data.len() {
        account.cell(cell_start, data.len(), quotes);
        account.row(row_start, data.len(), row_line, 0);
        line += 1;
    }
    (account, line, false)
}

/// Strings of `", a b \n \r`: quote-heavy, or long runs that carry a quoted cell over block
/// edges, or anything between.
fn quote_soup(random: &mut Random) -> Vec<u8> {
    let length = match random.below(3) {
        0 => random.below(16),
        1 => random.below(80),
        _ => random.below(400),
    };
    let quote_one_in = [2, 4, 16, 64][random.below(4)];
    let structure_one_in = [2, 8, 32, 128][random.below(4)];
    (0..length)
        .map(|_| {
            if random.below(quote_one_in) == 0 {
                b'"'
            } else if random.below(structure_one_in) == 0 {
                b",,\n\n\r"[random.below(5)]
            } else {
                b"ab"[random.below(2)]
            }
        })
        .collect()
}

#[test]
fn the_scanner_matches_a_byte_at_a_time_reference() {
    let mut random = Random(0x5EED_C0DE_1234_5678);
    for round in 0..20_000 {
        let data = quote_soup(&mut random);
        let quoting = round % 8 != 0;
        let (expected, expected_line, expected_unclosed) = reference_walk(&data, quoting);
        // Where the scan was told more was coming, part of the way along.
        let split = random.below(data.len() + 1);
        for kind in available() {
            let scanner = Scanner::with_engine(b',', quoting, kind);
            let case = format!("{:?} on {kind:?}", String::from_utf8_lossy(&data));

            // Whole.
            let mut account = Account::default();
            let stop = scanner.scan_into(&data, 0, 1, true, &mut account);
            assert_eq!(account, expected, "{case}");
            assert_eq!(stop.unclosed, expected_unclosed, "{case}");
            assert_eq!(stop.line, expected_line, "{case}");
            assert!(!stop.partial, "{case}");

            // Cut in two: nothing of an unfinished row is delivered, and resuming where the
            // scan said gives the same account.
            let mut account = Account::default();
            let stop = scanner.scan_into(&data[..split], 0, 1, false, &mut account);
            assert_eq!(account.pending, 0, "{case}");
            let stop = scanner.scan_into(&data, stop.next, stop.line, true, &mut account);
            assert_eq!(account, expected, "{case}, split {split}");
            assert_eq!(stop.unclosed, expected_unclosed, "{case}, split {split}");

            // One row a scan: every row boundary is a place to start from.
            let mut account = Account {
                stop_after_each: true,
                ..Account::default()
            };
            let (mut next, mut line) = (0, 1);
            let mut scans = 0;
            let stop = loop {
                let stop = scanner.scan_into(&data, next, line, true, &mut account);
                scans += 1;
                if stop.next == data.len() || stop.unclosed {
                    break stop;
                }
                assert!(scans <= data.len() + 1, "{case}");
                (next, line) = (stop.next, stop.line);
            };
            account.stop_after_each = false;
            assert_eq!(account, expected, "{case}, row by row");
            assert_eq!(stop.unclosed, expected_unclosed, "{case}, row by row");

            // A refused row isn't taken: the scan stops before it, and starts on it again.
            for refuse_every in [2, 3] {
                let mut account = Account {
                    refuse_every,
                    ..Account::default()
                };
                let (mut next, mut line) = (0, 1);
                let mut scans = 0;
                let stop = loop {
                    let stop = scanner.scan_into(&data, next, line, true, &mut account);
                    scans += 1;
                    assert_eq!(account.pending, 0, "{case}");
                    assert!(!stop.partial, "{case}");
                    if stop.next == data.len() || stop.unclosed {
                        break stop;
                    }
                    assert!(scans <= 2 * data.len() + 1, "{case}");
                    (next, line) = (stop.next, stop.line);
                };
                assert_eq!(account.cells, expected.cells, "{case}, refused");
                assert_eq!(account.rows, expected.rows, "{case}, refused");
                assert_eq!(stop.unclosed, expected_unclosed, "{case}, refused");
                assert_eq!(stop.line, expected_line, "{case}, refused");
            }
        }
    }
}

/// Each cell through the text door, its verdict as HyperCast's door gives it for the unescaped
/// text: the bad cell is the one that says so, and the rows go on.
#[test]
#[allow(clippy::type_complexity)]
fn invalid_utf8_is_malformed_on_text() {
    let cases: &[(&[u8], &[u8], (u32, u32, u32))] = &[
        (b"\xe9t\xe9\n", b"\xe9t\xe9", (2, 0, 1)),
        // A sequence cut off by the cell's end, by the line's end, and by the input's.
        (b"ab\xc3,x\n", b"ab\xc3", (2, 2, 1)),
        (b"abc\xe2\x82\n", b"abc\xe2\x82", (2, 3, 2)),
        (b"abc\xf0\x9f\x98", b"abc\xf0\x9f\x98", (2, 3, 3)),
        // What from_utf8 refuses and a mask of high bits wouldn't: an encoded surrogate, an
        // overlong form, a code point past U+10FFFF.
        (b"\xed\xa0\x80\n", b"\xed\xa0\x80", (2, 0, 1)),
        (b"\xc0\x80\n", b"\xc0\x80", (2, 0, 1)),
        (b"\xf4\x90\x80\x80\n", b"\xf4\x90\x80\x80", (2, 0, 1)),
        // A span into the unescaped text, which the door judges: two quotes in, one out.
        (b"\"a\"\"\xff\"\n", b"a\"\xff", (2, 2, 1)),
        (b"\"\xff\"\n", b"\xff", (2, 0, 1)),
        // Past every word boundary a validator's fast path might take.
        (
            b"0123456789abcdef0123456789abcdef\xff\n",
            b"0123456789abcdef0123456789abcdef\xff",
            (2, 32, 1),
        ),
    ];
    let specs = [ColumnSpec::new(0, 18, 0)];
    for &(input, cell, (reason, offset, len)) in cases {
        for kind in available() {
            let dialect = RawDialect {
                engine: kind.code(),
                ..CSV
            };
            let mut state = State::init(dialect).unwrap();
            // A good row after it, on a line of its own, when the case has a terminator.
            let mut text = input.to_vec();
            let terminated = input.last() == Some(&b'\n');
            if terminated {
                text.extend_from_slice(if input.contains(&b',') {
                    b"fine,x\n"
                } else {
                    b"fine\n"
                });
            }
            let (code, out, _, verdicts, _) = call(&mut state, &text, true, &specs, 10);
            let what = format!("{input:?} on {kind:?}");
            assert_eq!(code, OK, "{what}");
            assert_eq!(out.rows, 1 + u64::from(terminated), "{what}");
            let fault = hypercast::cast_text(cell).unwrap_err();
            assert_eq!(
                (fault.reason as u32, fault.offset, fault.len),
                (reason, offset, len),
                "the door itself, {what}"
            );
            assert_eq!(
                verdicts[0][0],
                CellVerdict {
                    offset,
                    len,
                    reason
                },
                "{what}"
            );
            if terminated {
                assert!(verdicts[0][1].is_ok(), "{what}");
            }
        }
    }
    // Only the text door is in question: an invalid cell no column reads isn't looked at, and
    // a bare Empty is still the one with no span.
    let mut state = State::init(CSV).unwrap();
    let (code, out, _, verdicts, _) = call(&mut state, b"ok,\xff\n,\xff\n", true, &specs, 10);
    assert_eq!((code, out.rows), (OK, 2));
    assert!(verdicts[0][0].is_ok());
    assert_eq!(
        verdicts[0][1],
        CellVerdict {
            offset: 0,
            len: 0,
            reason: 1
        }
    );
}

/// The whole header, on one call: `(code, out, the state after)`.
fn header_of(input: &[u8], kind: Kind) -> (i32, Filled, State) {
    let dialect = RawDialect {
        engine: kind.code(),
        ..CSV
    };
    let mut state = State::init(dialect).unwrap();
    let mut names = [Span::default(); 8];
    let mut arena = [0u8; 64];
    let mut out = Filled::default();
    let code = fill::header(&mut state, input, true, &mut names, &mut arena, &mut out);
    (code, out, state)
}

#[test]
fn invalid_utf8_in_a_header_is_a_failure() {
    let encoding = |line, record, byte| Failure {
        code: Failure::ENCODING,
        line,
        record,
        byte,
        expected: 0,
        found: 0,
    };
    let cases: &[(&[u8], Failure)] = &[
        (b"\xe9,b\n1,2\n", encoding(1, 0, 0)),
        (b"a,\xe9\n1,2\n", encoding(1, 0, 0)),
        // The unescaped name is what's judged.
        (b"a,\"x\"\"\xff\"\n1,2\n", encoding(1, 0, 0)),
        (b"\"\xff\",b", encoding(1, 0, 0)),
        // Its record, line and byte count what came before it.
        (b"\n\n\xe9\n", encoding(3, 2, 2)),
        (b"\xef\xbb\xbf\xe9\n", encoding(1, 0, 3)),
    ];
    for &(input, expected) in cases {
        for kind in available() {
            let (code, out, mut state) = header_of(input, kind);
            let what = format!("{input:?} on {kind:?}");
            assert_eq!(code, ERR_STRUCTURE, "{what}");
            assert_eq!(
                (out.failure, out.rows, out.consumed),
                (expected, 0, 0),
                "{what}"
            );
            // Final: whatever comes next, the same answer.
            let (code, out, ..) = call(&mut state, b"1,2\n", true, &[], 10);
            assert_eq!((code, out.failure), (ERR_STRUCTURE, expected), "{what}");
            let mut names = [Span::default(); 8];
            let mut out = Filled::default();
            let code = fill::header(&mut state, input, true, &mut names, &mut [], &mut out);
            assert_eq!((code, out.failure), (ERR_STRUCTURE, expected), "{what}");
        }
    }

    // A call refused for room is retried and reaches the same failure, because nothing was
    // judged or consumed on the way.
    let input = b"a,\"x\"\"\xff\"\n";
    let mut state = State::init(CSV).unwrap();
    let mut out = Filled::default();
    let mut names = [Span::default(); 1];
    let code = fill::header(&mut state, input, true, &mut names, &mut [], &mut out);
    assert_eq!((code, out.needed, out.consumed), (ERR_CELLS, 2, 0));
    let mut names = [Span::default(); 2];
    let code = fill::header(&mut state, input, true, &mut names, &mut [], &mut out);
    assert_eq!((code, out.consumed), (ERR_ARENA, 0));
    let mut arena = vec![0u8; out.needed as usize];
    let code = fill::header(&mut state, input, true, &mut names, &mut arena, &mut out);
    assert_eq!((code, out.failure), (ERR_STRUCTURE, encoding(1, 0, 0)));

    // Valid multi-byte names and an empty one aren't failures.
    for kind in available() {
        let (code, out, _) = header_of("é,,\"€\"\"\",😀\n".as_bytes(), kind);
        assert_eq!((code, out.rows), (OK, 4), "{kind:?}");
    }
}

/// A row is judged whole, so no chunk boundary can cut a sequence in the door's sight. Every
/// split of a file whose every sequence is multi-byte, with and without a header.
#[test]
fn utf8_validity_is_chunk_invariant() {
    let data = "é,€\n😀,\"é\"\"é\"\nñ,\n".as_bytes();
    let expected = (
        vec![
            vec!["é".as_bytes().to_vec(), "€".as_bytes().to_vec()],
            vec!["😀".as_bytes().to_vec(), "é\"é".as_bytes().to_vec()],
            vec!["ñ".as_bytes().to_vec(), Vec::new()],
        ],
        None,
    );
    for kind in available() {
        for split in 0..=data.len() {
            assert_eq!(
                delivered(data, kind, 2, &[split]),
                expected,
                "split {split} on {kind:?}"
            );
        }
    }

    // The same for a verdict: the bad row's span doesn't move with the split.
    let data = b"\xc3\xa9\n\xc3\xa9\xc3\n\xc3\xa9\n";
    let specs = [ColumnSpec::new(0, 18, 0)];
    for split in 0..=data.len() {
        let mut state = State::init(CSV).unwrap();
        let mut verdicts = Vec::new();
        let mut at = 0;
        for end in [split, data.len()] {
            loop {
                let last = end == data.len();
                let (code, out, _, seen, _) = call(&mut state, &data[at..end], last, &specs, 1);
                assert_eq!(code, OK, "split {split}");
                if out.rows == 0 {
                    break;
                }
                verdicts.push(seen[0][0]);
                at += out.consumed as usize;
            }
        }
        let bad = CellVerdict {
            offset: 2,
            len: 1,
            reason: 2,
        };
        assert_eq!(
            verdicts,
            [CellVerdict::OK, bad, CellVerdict::OK],
            "split {split}"
        );
    }

    // And for a header: not a failure until the record is whole, and then the same one.
    for (data, failure) in [
        ("é,€\n".as_bytes(), 0),
        (b"\xc3\xa9,\xe2\x82\n", Failure::ENCODING),
    ] {
        for split in 0..data.len() {
            let mut state = State::init(CSV).unwrap();
            let mut names = [Span::default(); 4];
            let mut out = Filled::default();
            let code = fill::header(
                &mut state,
                &data[..split],
                false,
                &mut names,
                &mut [],
                &mut out,
            );
            assert_eq!((code, out.rows, out.consumed), (OK, 0, 0), "split {split}");
            fill::header(&mut state, data, true, &mut names, &mut [], &mut out);
            assert_eq!(out.failure.code, failure, "split {split}");
        }
    }
}

#[test]
fn embedded_nul_passes() {
    for kind in available() {
        assert_eq!(
            delivered(b"a\0b,\0\n", kind, 2, &[]),
            (vec![vec![b"a\0b".to_vec(), b"\0".to_vec()]], None),
            "{kind:?}"
        );
        let (code, out, _) = header_of(b"a\0b,c\n", kind);
        assert_eq!((code, out.rows), (OK, 2), "{kind:?}");
    }
}

/// One cell as a reading saw it: its verdict, and its text if it has one.
type JudgedCell = (CellVerdict, Vec<u8>);

/// Every row of `data` through the text door, in `max_rows` batches, chunks ending at `split`
/// and then at the end. Each cell is held on the spot to the rule the region check stands in
/// for: HyperCast's door, asked of that one cell's unescaped bytes, located by the cell table.
fn read_against_the_cell_rule(
    data: &[u8],
    kind: Kind,
    split: usize,
    max_rows: usize,
    width: usize,
) -> (Vec<Vec<JudgedCell>>, Option<u32>) {
    let dialect = RawDialect {
        engine: kind.code(),
        ..CSV
    };
    let mut state = State::init(dialect).unwrap();
    let specs: Vec<ColumnSpec> = (0..width as u32)
        .map(|ordinal| ColumnSpec::new(ordinal, 18, 0))
        .collect();
    let per_row = width + 1;
    let mut values = vec![vec![0u8; size_of::<Span>() * max_rows]; width];
    let mut verdicts = vec![vec![CellVerdict::default(); max_rows]; width];
    let mut cells = vec![Span::default(); per_row * max_rows];
    let mut arena = vec![0u8; data.len().max(1)];
    let mut rows = Vec::new();
    let mut at = 0;
    for end in [split, data.len()] {
        loop {
            let last = end == data.len();
            let input = &data[at..end];
            let columns: Vec<ColumnBuffer> = values
                .iter_mut()
                .zip(verdicts.iter_mut())
                .map(|(values, verdicts)| ColumnBuffer {
                    values: values.as_mut_ptr().cast(),
                    verdicts: verdicts.as_mut_ptr(),
                })
                .collect();
            let mut out = Filled::default();
            // SAFETY: every column holds `max_rows` spans and verdicts.
            let code = unsafe {
                fill::fill(
                    &mut state, input, last, &specs, &columns, max_rows, &mut cells, &mut arena,
                    &mut out,
                )
            };
            if code == ERR_STRUCTURE {
                return (rows, Some(out.failure.code));
            }
            assert_eq!(code, OK);
            for row in 0..out.rows as usize {
                let mut seen = Vec::with_capacity(width);
                for column in 0..width {
                    let cell = cells[row * per_row + column];
                    let raw = &input[cell.offset as usize..cell.offset as usize + cell.len()];
                    let mut unescaped = vec![0u8; raw.len()];
                    let text = if cell.flagged() {
                        let written = unescape_into(raw, &mut unescaped);
                        &unescaped[..written]
                    } else {
                        raw
                    };
                    let verdict = verdicts[column][row];
                    let what = format!(
                        "{:?}, column {column}, split {split}, {max_rows} rows, {kind:?}",
                        String::from_utf8_lossy(data)
                    );
                    let value = match hypercast::cast_text(text) {
                        Ok(_) => {
                            assert_eq!(verdict, CellVerdict::OK, "{what}");
                            // SAFETY: within the column's buffer; the read is unaligned.
                            let span: Span = unsafe {
                                values[column]
                                    .as_ptr()
                                    .add(row * size_of::<Span>())
                                    .cast::<Span>()
                                    .read_unaligned()
                            };
                            let from: &[u8] = if span.flagged() { &arena } else { input };
                            let start = span.offset as usize;
                            let value = from[start..start + span.len()].to_vec();
                            assert_eq!(value, text, "{what}");
                            value
                        }
                        Err(fault) => {
                            assert_eq!(verdict, CellVerdict::from_fault(fault), "{what}");
                            Vec::new()
                        }
                    };
                    seen.push((verdict, value));
                }
                rows.push(seen);
            }
            at += out.consumed as usize;
            if out.rows == 0 {
                break;
            }
        }
    }
    (rows, None)
}

/// One cell of `utf8_soup`, as written: valid and invalid sequences, each half of a two-byte
/// character so neighbours can make or break one across a separator or a quote, in plain cells,
/// quoted ones, doubled quotes and stray ones.
fn utf8_cell(random: &mut Random, valid_only: bool, out: &mut Vec<u8>) {
    const VALID: &[&[u8]] = &[b"a", b"0", "é".as_bytes(), "€".as_bytes(), "😀".as_bytes()];
    const INVALID: &[&[u8]] = &[
        b"\xC3",
        b"\xA9",
        b"\xE2\x82",
        b"\xF0\x9F\x98",
        b"\xED\xA0\x80",
        b"\xC0\x80",
        b"\xF4\x90\x80\x80",
        b"\xFF",
    ];
    fn content(random: &mut Random, valid_only: bool, out: &mut Vec<u8>, escape: Option<&[u8]>) {
        for _ in 0..random.below(4) {
            let pool = if valid_only || random.below(3) == 0 {
                VALID
            } else {
                INVALID
            };
            out.extend_from_slice(pool[random.below(pool.len())]);
            if let Some(escape) = escape
                && random.below(3) == 0
            {
                out.extend_from_slice(escape);
            }
        }
    }
    match random.below(8) {
        // Plain, or nothing at all.
        0..=2 => content(random, valid_only, out, None),
        // Quoted, with or without doubled quotes and a separator inside.
        3..=4 => {
            out.push(b'"');
            content(random, valid_only, out, Some(b"\"\""));
            if random.below(4) == 0 {
                out.push(b',');
            }
            content(random, valid_only, out, Some(b"\"\""));
            out.push(b'"');
        }
        // The two halves of a character with a doubled quote between: `"\xC3""\xA9"`.
        5 => out.extend_from_slice(if valid_only {
            "\"é\"\"é\"".as_bytes()
        } else {
            b"\"\xC3\"\"\xA9\""
        }),
        // A stray quote: after a close, then inside a plain cell.
        6 => {
            out.push(b'"');
            content(random, valid_only, out, None);
            out.push(b'"');
            content(random, valid_only, out, None);
            out.push(b'"');
        }
        _ => {
            content(random, valid_only, out, None);
            out.push(b'"');
            content(random, valid_only, out, None);
        }
    }
}

/// Rows of generated cells, mostly of one width. A fifth of the inputs are valid throughout,
/// and the rest are a mix, so that with small batches some regions pass and some don't.
fn utf8_soup(random: &mut Random, width: usize) -> (Vec<u8>, bool) {
    let valid_only = random.below(5) == 0;
    let mut out = Vec::new();
    for _ in 0..1 + random.below(5) {
        // Clean rows between bad ones, which is what puts a good region beside a bad one.
        let row_valid = valid_only || random.below(2) == 0;
        for column in 0..width {
            if column > 0 {
                out.push(b',');
            }
            utf8_cell(random, row_valid, &mut out);
        }
        out.extend_from_slice(if random.below(4) == 0 { b"\r\n" } else { b"\n" });
    }
    (out, valid_only)
}

/// The region check is an optimisation of the rule "each cell through the door": the same
/// verdicts, spans and text, cell for cell, however the input is cut and however many rows a
/// batch takes, on every engine. The reader holds each cell to the door as it goes.
#[test]
fn the_region_check_gives_what_each_cell_would() {
    let mut random = Random(0x0DD5_EED5_1234_5678);
    let (mut valid, mut mixed, mut faults, mut oks) = (0, 0, 0, 0);
    for round in 0..400 {
        let width = 1 + round % 3;
        let (data, valid_only) = utf8_soup(&mut random, width);
        if valid_only {
            valid += 1;
        } else {
            mixed += 1;
        }
        for kind in available() {
            let whole = read_against_the_cell_rule(&data, kind, data.len(), 64, width);
            for row in &whole.0 {
                for (verdict, _) in row {
                    match verdict.reason {
                        0 => oks += 1,
                        2 => faults += 1,
                        _ => {}
                    }
                }
            }
            for max_rows in [1, 2, 64] {
                for split in 0..=data.len() {
                    assert_eq!(
                        read_against_the_cell_rule(&data, kind, split, max_rows, width),
                        whole,
                        "{:?}, split {split}, {max_rows} rows, {kind:?}",
                        String::from_utf8_lossy(&data)
                    );
                }
            }
        }
    }
    assert!(valid > 30 && mixed > 200, "{valid} valid, {mixed} mixed");
    assert!(oks > 1000 && faults > 500, "{oks} ok, {faults} malformed");
}

/// A good row and a bad one in one batch: the bad cell alone is bad, and it doesn't matter
/// which of them the region's first refusal is in.
#[test]
fn a_refused_region_faults_only_the_bad_cell() {
    let specs = [ColumnSpec::new(0, 18, 0), ColumnSpec::new(1, 18, 0)];
    let mut state = State::init(CSV).unwrap();
    let text = "é,ok\nok,\u{0}\n".as_bytes();
    let mut data = text.to_vec();
    data.extend_from_slice(b"\xff,\xc3\xa9\n,\xe2\x82z\n");
    let (code, out, _, verdicts, _) = call(&mut state, &data, true, &specs, 10);
    assert_eq!((code, out.rows), (OK, 4));
    let reasons = |column: usize| -> Vec<u32> {
        verdicts[column][..4]
            .iter()
            .map(|verdict| verdict.reason)
            .collect()
    };
    assert_eq!(reasons(0), [0, 0, 2, 1]);
    assert_eq!(reasons(1), [0, 0, 0, 2]);
    assert_eq!(
        verdicts[1][3],
        CellVerdict {
            offset: 0,
            len: 2,
            reason: 2
        }
    );
}

// ---- Options and notes: `init_with` and `fill_with`, through the exports ---------------------

use hypercast::{DateOptions, Detail, RawDateOptions};
use hypertabular::kernel::abi::{
    BuffersWith, CellNote, ColumnBufferWith, ColumnSpecWith, RawDelimitedOptions,
};
use hypertabular::kernel::exports::{
    hypertabular_delimited_fill, hypertabular_delimited_fill_with, hypertabular_delimited_header,
    hypertabular_delimited_init, hypertabular_delimited_init_with,
    hypertabular_delimited_state_size,
};

const TEXT: u32 = 18;
const I32: u32 = 4;
const DATE: u32 = 15;

const EMPTY: CellVerdict = CellVerdict {
    offset: 0,
    len: 0,
    reason: 1,
};

fn options() -> RawDelimitedOptions {
    RawDelimitedOptions {
        size: RawDelimitedOptions::SIZE,
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
        skip: 0,
        header_row: 0,
        short_rows: 0,
        long_rows: 0,
    }
}

/// A list of tokens in HyperCast's vocabulary encoding: a little-endian length, then the bytes.
fn packed(tokens: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for token in tokens {
        out.extend_from_slice(&(token.len() as u32).to_le_bytes());
        out.extend_from_slice(token);
    }
    out
}

fn text_column(ordinal: u32) -> ColumnSpecWith {
    ColumnSpecWith::new(ordinal, TEXT, 0)
}

fn with_tokens(mut spec: ColumnSpecWith, tokens: &[u8]) -> ColumnSpecWith {
    // A non-null pointer, even for an empty list: null is what inherits.
    spec.tokens = if tokens.is_empty() {
        c"".as_ptr().cast()
    } else {
        tokens.as_ptr()
    };
    spec.tokens_len = tokens.len();
    spec
}

/// One cell of a [`read_with`]: its verdict and note, its text, and its value's first bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WithCell {
    verdict: CellVerdict,
    note: CellNote,
    text: Vec<u8>,
    value: [u8; 16],
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WithRead {
    header: Vec<Vec<u8>>,
    rows: Vec<Vec<WithCell>>,
    widths: Vec<u32>,
    lines: Vec<u32>,
    failure: Option<Failure>,
}

/// A state block as a binding allocates one: as many bytes as the library says.
fn state_block() -> Vec<u64> {
    // SAFETY: no contract.
    let bytes = unsafe { hypertabular_delimited_state_size() };
    vec![0xA5A5_A5A5_A5A5_A5A5; bytes.div_ceil(8)]
}

fn started(options: &RawDelimitedOptions) -> Vec<u64> {
    let mut block = state_block();
    // SAFETY: the block is as large as the library asks, the options are live.
    let code =
        unsafe { hypertabular_delimited_init_with(block.as_mut_ptr().cast::<State>(), options) };
    assert_eq!(code, OK);
    block
}

/// The whole of `data` through `init_with`, `_header` when `header` is set, and `fill_with`,
/// chunks ending at `split` and then at the end, room grown on every refusal.
fn read_split(
    data: &[u8],
    options: &RawDelimitedOptions,
    header: bool,
    specs: &[ColumnSpecWith],
    tokens: &[u8],
    split: usize,
    max_rows: usize,
) -> WithRead {
    let mut block = started(options);
    let state = block.as_mut_ptr().cast::<State>();
    let per_row = specs.iter().map(|s| s.ordinal as usize).max().unwrap_or(0) + 2;
    let mut cells = vec![Span::default(); 1];
    let mut arena = vec![0u8; 1];
    let mut names = vec![Span::default(); 1];
    let mut read = WithRead::default();
    let mut need_header = header;
    let mut at = 0;
    let mut end = split.min(data.len());
    loop {
        let last = end == data.len();
        let input = &data[at..end];
        let mut out = Filled::default();
        if need_header {
            // SAFETY: every buffer is the length given beside it.
            let code = unsafe {
                hypertabular_delimited_header(
                    state,
                    input.as_ptr(),
                    input.len(),
                    u32::from(last),
                    names.as_mut_ptr(),
                    names.len(),
                    arena.as_mut_ptr(),
                    arena.len(),
                    &mut out,
                )
            };
            match code {
                OK => {
                    for name in &names[..out.rows as usize] {
                        let from: &[u8] = if name.flagged() { &arena } else { input };
                        let start = name.offset as usize;
                        read.header.push(from[start..start + name.len()].to_vec());
                    }
                    if out.rows > 0 || (last && out.consumed as usize == input.len()) {
                        need_header = false;
                    }
                }
                ERR_CELLS => {
                    names.resize(out.needed as usize, Span::default());
                    continue;
                }
                ERR_ARENA => {
                    arena.resize(out.needed as usize, 0);
                    continue;
                }
                ERR_STRUCTURE => {
                    read.failure = Some(out.failure);
                    return read;
                }
                other => panic!("the header returned {other}"),
            }
        } else {
            let mut values = vec![vec![0xEEu8; 32 * max_rows]; specs.len()];
            let mut verdicts = vec![
                vec![
                    CellVerdict {
                        offset: 9,
                        len: 9,
                        reason: 9
                    };
                    max_rows
                ];
                specs.len()
            ];
            let mut notes = vec![
                vec![
                    CellNote {
                        detail: 99,
                        origin: 99
                    };
                    max_rows
                ];
                specs.len()
            ];
            let mut widths = vec![0u32; max_rows];
            let columns: Vec<ColumnBufferWith> = (0..specs.len())
                .map(|column| ColumnBufferWith {
                    size: ColumnBufferWith::SIZE,
                    values: values[column].as_mut_ptr().cast(),
                    verdicts: verdicts[column].as_mut_ptr(),
                    notes: notes[column].as_mut_ptr(),
                })
                .collect();
            let mut buffers = BuffersWith::empty();
            buffers.arena = arena.as_mut_ptr();
            buffers.arena_cap = arena.len();
            buffers.cells = cells.as_mut_ptr();
            buffers.cells_cap = cells.len();
            buffers.tokens = tokens.as_ptr();
            buffers.tokens_len = tokens.len();
            buffers.row_widths = widths.as_mut_ptr();
            // SAFETY: every column holds `max_rows` of everything.
            let code = unsafe {
                hypertabular_delimited_fill_with(
                    state,
                    input.as_ptr(),
                    input.len(),
                    u32::from(last),
                    specs.as_ptr(),
                    columns.as_ptr(),
                    specs.len(),
                    max_rows,
                    &buffers,
                    &mut out,
                )
            };
            match code {
                OK => {}
                ERR_CELLS => {
                    cells.resize(out.needed as usize * max_rows, Span::default());
                    continue;
                }
                ERR_ARENA => {
                    arena.resize(out.needed as usize, 0);
                    continue;
                }
                ERR_STRUCTURE => {
                    read.failure = Some(out.failure);
                    return read;
                }
                other => panic!("the fill returned {other}"),
            }
            for row in 0..out.rows as usize {
                read.widths.push(widths[row]);
                read.lines.push(cells[row * per_row + per_row - 1].len);
                let seen = specs.iter().enumerate().map(|(column, spec)| {
                    let verdict = verdicts[column][row];
                    let at = row * size_of::<Span>();
                    let mut value = [0u8; 16];
                    let size = match spec.door {
                        TEXT => 0,
                        I32 => 4,
                        DATE | 20 => size_of::<hypercast::Date>(),
                        _ => 16,
                    };
                    value[..size].copy_from_slice(&values[column][row * size..][..size]);
                    let mut text = Vec::new();
                    if spec.door == TEXT && verdict.is_ok() {
                        // SAFETY: within the column; the read is unaligned.
                        let span: Span = unsafe {
                            values[column]
                                .as_ptr()
                                .add(at)
                                .cast::<Span>()
                                .read_unaligned()
                        };
                        let from: &[u8] = if span.flagged() { &arena } else { input };
                        let start = span.offset as usize;
                        text = from[start..start + span.len()].to_vec();
                    }
                    WithCell {
                        verdict,
                        note: notes[column][row],
                        text,
                        value,
                    }
                });
                read.rows.push(seen.collect());
            }
            if out.rows == 0 && (out.consumed == 0 || last) && last {
                let more = input.len() != out.consumed as usize;
                assert!(!more || out.consumed == 0);
                return read;
            }
        }
        at += out.consumed as usize;
        if out.consumed == 0 && out.rows == 0 && !last {
            end = data.len().min(end + 1);
        }
    }
}

fn read_with(
    data: &[u8],
    options: &RawDelimitedOptions,
    header: bool,
    specs: &[ColumnSpecWith],
    tokens: &[u8],
) -> WithRead {
    let whole = read_split(data, options, header, specs, tokens, data.len(), 16);
    // However the input is cut, and however many rows a batch takes, the same read.
    for split in 0..data.len() {
        assert_eq!(
            read_split(data, options, header, specs, tokens, split, 16),
            whole,
            "split at {split} of {:?}",
            String::from_utf8_lossy(data)
        );
    }
    assert_eq!(
        read_split(data, options, header, specs, tokens, data.len(), 1),
        whole,
        "one row a batch of {:?}",
        String::from_utf8_lossy(data)
    );
    for kind in available() {
        let on = RawDelimitedOptions {
            engine: kind.code(),
            ..*options
        };
        assert_eq!(
            read_split(data, &on, header, specs, tokens, data.len(), 16),
            whole,
            "{kind:?} on {:?}",
            String::from_utf8_lossy(data)
        );
    }
    whole
}

/// The texts of a read through text doors, an empty one for a cell with none.
fn texts(read: &WithRead) -> Vec<Vec<&[u8]>> {
    read.rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.text.as_slice()).collect())
        .collect()
}

#[test]
fn skip_counts_physical_lines() {
    // The stray quote would open a quoted cell, and swallow the rest, if the line were tokenized.
    let data = b"t\"itle\na,b\n1,2\n";
    let on = RawDelimitedOptions {
        skip: 1,
        ..options()
    };
    let read = read_with(data, &on, true, &[text_column(0), text_column(1)], &[]);
    assert_eq!(read.header, [b"a", b"b"]);
    assert_eq!(texts(&read), [[b"1", b"2"]]);
    assert_eq!(read.lines, [3], "lines count what was skipped");
    assert_eq!(read.failure, None);

    // Every line ending, a blank line included, and quotes that are nothing to `skip`.
    let data = b"\"a\n\r\nb\"\rc,d\n1,2\n";
    let on = RawDelimitedOptions {
        skip: 3,
        ..options()
    };
    let read = read_with(data, &on, true, &[text_column(0)], &[]);
    assert_eq!(read.header, [b"c", b"d"]);
    assert_eq!(read.lines, [5]);

    // Past more lines than one chunk holds, by the fill alone as by the header.
    let on = RawDelimitedOptions {
        skip: 3,
        ..options()
    };
    let read = read_with(b"a\nb\nc\n1\n", &on, false, &[text_column(0)], &[]);
    assert_eq!(texts(&read), [[b"1"]]);
    assert_eq!(read.lines, [4]);

    // Without a header, the first row after the skip fixes the width.
    let on = RawDelimitedOptions {
        skip: 1,
        ..options()
    };
    let read = read_with(b"a,b,c\n1\n2\n", &on, false, &[text_column(0)], &[]);
    assert_eq!(texts(&read), [[b"1"], [b"2"]]);
}

#[test]
fn skip_past_the_end_reads_nothing() {
    let on = RawDelimitedOptions {
        skip: 9,
        ..options()
    };
    let read = read_with(b"a,b\n1,2\n", &on, true, &[text_column(0)], &[]);
    assert_eq!(read, WithRead::default());
    let read = read_with(b"a,b\n1,2", &on, false, &[text_column(0)], &[]);
    assert_eq!(read, WithRead::default());
    // A record's worth of records discarded likewise.
    let on = RawDelimitedOptions {
        header_row: 9,
        ..options()
    };
    let read = read_with(b"a,b\n1,2\n", &on, true, &[text_column(0)], &[]);
    assert_eq!(read, WithRead::default());
    let read = read_with(b"a,b\n1,2", &on, false, &[text_column(0)], &[]);
    assert_eq!(read, WithRead::default());
}

#[test]
fn a_bom_does_not_count_as_a_line() {
    let on = RawDelimitedOptions {
        skip: 1,
        ..options()
    };
    let read = read_with(
        b"\xEF\xBB\xBFtitle\na\n1\n",
        &on,
        true,
        &[text_column(0)],
        &[],
    );
    assert_eq!(read.header, [b"a"]);
    assert_eq!(texts(&read), [[b"1"]]);
}

#[test]
fn header_row_discards_records() {
    let on = RawDelimitedOptions {
        header_row: 1,
        ..options()
    };
    let columns = [text_column(0), text_column(1)];
    let read = read_with(b"x,y\na,b\n1,2\n", &on, true, &columns, &[]);
    assert_eq!(read.header, [b"a", b"b"]);
    assert_eq!(texts(&read), [[b"1", b"2"]]);

    // Records, so a line break in a quoted cell doesn't count. The discarded record's width
    // fixes nothing, and it is after the skip.
    let on = RawDelimitedOptions {
        skip: 1,
        header_row: 1,
        ..options()
    };
    let read = read_with(b"t\"\n\"x\ny\",z,w\na,b\n1,2\n", &on, true, &columns, &[]);
    assert_eq!(read.header, [b"a", b"b"]);
    assert_eq!(texts(&read), [[b"1", b"2"]]);
    assert_eq!(read.lines, [5]);

    // A blank line a dialect skips isn't a record; one it keeps is.
    let on = RawDelimitedOptions {
        header_row: 1,
        ..options()
    };
    let read = read_with(b"\nx\n\na\n1\n", &on, true, &[text_column(0)], &[]);
    assert_eq!(read.header, [b"a"]);
    let keeps = RawDelimitedOptions {
        skip_blank_lines: 0,
        ..on
    };
    let read = read_with(b"\nx\n1\n", &keeps, true, &[text_column(0)], &[]);
    assert_eq!(read.header, [b"x"]);

    // With no header declared, the discarded records are simply gone.
    let read = read_with(b"x,y,z\n1,2\n", &on, false, &columns, &[]);
    assert_eq!(texts(&read), [[b"1", b"2"]]);

    // A structural failure's position counts what was discarded, with no header too.
    let read = read_with(b"x\n1,2\n3\n", &on, false, &columns, &[]);
    let failure = read.failure.expect("a record of the wrong width");
    assert_eq!(
        (failure.code, failure.record, failure.line, failure.byte),
        (Failure::COLUMN_COUNT, 2, 3, 6)
    );

    // A structural failure's position counts what was discarded, but not what was skipped.
    let on = RawDelimitedOptions {
        skip: 1,
        header_row: 1,
        ..options()
    };
    let read = read_with(b"title\nx\na,b\n1\n", &on, true, &columns, &[]);
    let failure = read.failure.expect("a record of the wrong width");
    assert_eq!(
        (failure.code, failure.record, failure.line, failure.byte),
        (Failure::COLUMN_COUNT, 2, 4, 12)
    );
}

#[test]
fn short_rows_pad() {
    let on = RawDelimitedOptions {
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        ..options()
    };
    let columns = [
        text_column(0),
        text_column(1),
        ColumnSpecWith::new(1, I32, 0),
    ];
    let read = read_with(b"1,2\n3\n", &on, false, &columns, &[]);
    assert_eq!(read.widths, [2, 1]);
    assert_eq!(read.failure, None);
    assert_eq!(read.rows[0][1].text, b"2");
    let padded = &read.rows[1];
    assert_eq!(padded[0].text, b"3");
    for cell in &padded[1..] {
        assert_eq!(cell.verdict, EMPTY);
        assert_eq!(
            cell.note,
            CellNote {
                detail: 0,
                origin: CellNote::ORIGIN_PADDED
            }
        );
    }
    // An empty cell that's there is the door's, and it isn't padding, at any ordinal.
    let read = read_with(b",\n,x\n3\n", &on, false, &columns, &[]);
    for cell in &read.rows[0] {
        assert_eq!(cell.verdict, EMPTY);
        assert_eq!(
            cell.note,
            CellNote {
                detail: Detail::Blank as u16,
                origin: CellNote::ORIGIN_CELL
            }
        );
    }
    assert_eq!(read.rows[2][1].note.origin, CellNote::ORIGIN_PADDED);
}

#[test]
fn short_rows_reject() {
    let read = read_with(b"1,2\n3\n", &options(), false, &[text_column(0)], &[]);
    assert_eq!(texts(&read), [[b"1"]]);
    let failure = read.failure.expect("a record of the wrong width");
    assert_eq!(
        (
            failure.code,
            failure.expected,
            failure.found,
            failure.record
        ),
        (Failure::COLUMN_COUNT, 2, 1, 1)
    );
}

#[test]
fn long_rows_reject() {
    let read = read_with(b"1\n2,3\n", &options(), false, &[text_column(0)], &[]);
    assert_eq!(texts(&read), [[b"1"]]);
    let failure = read.failure.expect("a record of the wrong width");
    assert_eq!(
        (failure.code, failure.expected, failure.found, failure.line),
        (Failure::COLUMN_COUNT, 1, 2, 2)
    );
}

fn long_rows_keep(policy: u32) {
    let on = RawDelimitedOptions {
        long_rows: policy,
        ..options()
    };
    // Ordinal 1 is past the width the first record fixed: the extra cell is dropped, even from
    // a plan that reads there. Its bad bytes and its quotes are nothing to anyone.
    let columns = [text_column(0), text_column(1)];
    let read = read_with(b"1\n2,3\n4,\"\xff\"\"\"\n", &on, false, &columns, &[]);
    assert_eq!(read.failure, None);
    assert_eq!(read.widths, [1, 2, 2]);
    assert_eq!(
        read.rows
            .iter()
            .map(|row| row[0].text.as_slice())
            .collect::<Vec<_>>(),
        [b"1", b"2", b"4"]
    );
    for row in &read.rows {
        assert_eq!(row[1].verdict, EMPTY);
        assert_eq!(row[1].note.origin, CellNote::ORIGIN_PADDED);
    }
}

#[test]
fn long_rows_report() {
    long_rows_keep(RawDelimitedOptions::LONG_ROWS_REPORT);
}

#[test]
fn long_rows_truncate() {
    long_rows_keep(RawDelimitedOptions::LONG_ROWS_TRUNCATE);
}

#[test]
fn a_header_fixes_the_width_the_policies_answer_to() {
    let on = RawDelimitedOptions {
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        long_rows: RawDelimitedOptions::LONG_ROWS_TRUNCATE,
        ..options()
    };
    let columns = [text_column(0), text_column(1)];
    let read = read_with(b"a,b\n1\n2,3,4\n", &on, true, &columns, &[]);
    assert_eq!(read.widths, [1, 3]);
    assert_eq!(read.rows[1][1].text, b"3");
    assert_eq!(read.rows[0][1].note.origin, CellNote::ORIGIN_PADDED);
}

#[test]
fn tokens_replace_and_inherit() {
    let dialect = packed(&[b"NA"]);
    let dash = packed(&[b"-"]);
    let columns = [
        text_column(0),
        with_tokens(text_column(1), &[]),
        with_tokens(text_column(2), &dash),
    ];
    let read = read_with(b"NA,NA,NA\n-,-,-\n", &options(), false, &columns, &dialect);
    let token = CellNote {
        detail: 0,
        origin: CellNote::ORIGIN_TOKEN,
    };
    let origins: Vec<Vec<u16>> = read
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.note.origin).collect())
        .collect();
    assert_eq!(origins, [[1, 0, 0], [0, 0, 1]]);
    assert_eq!(read.rows[0][0].verdict, EMPTY);
    assert_eq!(read.rows[0][0].note, token);
    assert_eq!(read.rows[0][1].text, b"NA");
    assert_eq!(read.rows[1][2].verdict, EMPTY);
    assert_eq!(read.rows[1][0].text, b"-");

    // Every door, and before the door: a number door never sees the token.
    let numbers = [ColumnSpecWith::new(0, I32, 0)];
    let read = read_with(b"NA\n7\nx\n", &options(), false, &numbers, &dialect);
    assert_eq!(read.rows[0][0].verdict, EMPTY);
    assert_eq!(read.rows[0][0].note, token);
    assert!(read.rows[1][0].verdict.is_ok());
    assert_eq!(read.rows[2][0].verdict.reason, 2);

    // Exact bytes: no trimming, no folding, and several in a list.
    let several = packed(&[b"N/A", b"na"]);
    let read = read_with(
        b"N/A\nna\n na\nNA\n",
        &options(),
        false,
        &[text_column(0)],
        &several,
    );
    let origins: Vec<u16> = read.rows.iter().map(|row| row[0].note.origin).collect();
    assert_eq!(origins, [1, 1, 0, 0]);
}

#[test]
fn tokens_meet_blanks_and_padding() {
    let on = RawDelimitedOptions {
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        ..options()
    };
    let blank = packed(&[b""]);
    let columns = [text_column(0), text_column(1)];
    let read = read_with(b"a,\nb\n", &on, false, &columns, &blank);
    assert_eq!(read.rows[0][1].verdict, EMPTY);
    assert_eq!(read.rows[0][1].note.origin, CellNote::ORIGIN_TOKEN);
    assert_eq!(read.rows[1][1].verdict, EMPTY);
    assert_eq!(read.rows[1][1].note.origin, CellNote::ORIGIN_PADDED);
    // Spaces aren't blank to a token, which matches bytes, and ordinal 0 can't be padding.
    let read = read_with(b",x\n ,y\n", &on, false, &columns, &blank);
    assert_eq!(read.rows[0][0].note.origin, CellNote::ORIGIN_TOKEN);
    assert_eq!(read.rows[1][0].note.origin, CellNote::ORIGIN_CELL);
    assert_eq!(read.rows[1][0].text, b" ");
}

#[test]
fn a_quoted_token_is_text() {
    let tokens = packed(&[b"NA", b"", b"N\"A"]);
    let columns = [text_column(0)];
    // Quoted plainly, quoted with an escape, and quoted empty, against the same cells unquoted.
    let data = b"\"NA\"\nNA\n\"N\"\"A\"\nN\"A\n\"\"\n\n";
    let on = RawDelimitedOptions {
        skip_blank_lines: 0,
        ..options()
    };
    let read = read_with(data, &on, false, &columns, &tokens);
    let origins: Vec<u16> = read.rows.iter().map(|row| row[0].note.origin).collect();
    assert_eq!(origins, [0, 1, 0, 1, 0, 1]);
    assert_eq!(read.rows[0][0].text, b"NA");
    assert!(read.rows[0][0].verdict.is_ok());
    assert_eq!(read.rows[1][0].verdict, EMPTY);
    assert_eq!(read.rows[2][0].text, b"N\"A");
    // A quoted empty cell is no token, and still empty: the door says so.
    assert_eq!(read.rows[4][0].verdict, EMPTY);
    assert_eq!(read.rows[4][0].note.detail, Detail::Blank as u16);

    // With quoting off a quote is a byte, so nothing is quoted.
    let tokens = packed(&[b"\"NA\""]);
    let off = RawDelimitedOptions {
        quoting: 0,
        ..options()
    };
    let read = read_with(b"\"NA\"\n", &off, false, &columns, &tokens);
    assert_eq!(read.rows[0][0].note.origin, CellNote::ORIGIN_TOKEN);
}

#[test]
fn notes_carry_detail() {
    let columns = [ColumnSpecWith::new(0, I32, 0)];
    let read = read_with(
        b"12a\n7\n99999999999\n\n",
        &{
            RawDelimitedOptions {
                skip_blank_lines: 0,
                ..options()
            }
        },
        false,
        &columns,
        &[],
    );
    let notes: Vec<CellNote> = read.rows.iter().map(|row| row[0].note).collect();
    let cell = |detail: Detail| CellNote {
        detail: detail as u16,
        origin: CellNote::ORIGIN_CELL,
    };
    assert_eq!(
        notes,
        [
            cell(Detail::UnexpectedCharacter),
            CellNote {
                detail: 0,
                origin: 0
            },
            cell(Detail::TooLarge),
            cell(Detail::Blank),
        ]
    );
    // The same verdict the plain door gives, with its span.
    assert_eq!(
        read.rows[0][0].verdict,
        CellVerdict::from_fault(hypercast::cast_i32(b"12a", &NumFormat::INVARIANT).unwrap_err())
    );
    // And the text door's.
    let read = read_with(b"\xff\n", &options(), false, &[text_column(0)], &[]);
    assert_eq!(read.rows[0][0].note, cell(Detail::InvalidUtf8));
}

#[test]
fn date_options_reach_the_door() {
    let plain = [ColumnSpecWith::new(0, DATE, 0)];
    let read = read_with(b"20260107\n", &options(), false, &plain, &[]);
    assert_eq!(
        read.rows[0][0].verdict.reason, 2,
        "no option, no compact date"
    );

    let mut compact = plain;
    compact[0].date = RawDateOptions {
        size: RawDateOptions::SIZE,
        flags: DateOptions::COMPACT_DATE,
        pivot: 0,
    };
    let read = read_with(b"20260107\n", &options(), false, &compact, &[]);
    assert!(read.rows[0][0].verdict.is_ok());
    let date = hypercast::cast_date(b"2026-01-07").unwrap();
    assert_eq!(read.rows[0][0].value[..2], date.year.to_ne_bytes());
    assert_eq!(read.rows[0][0].value[2..4], [date.month, date.day]);

    // The vocabulary, packed as HyperCast packs it, and the pivot, at the door that reads them.
    let mut months = Vec::new();
    for (month, name) in [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .enumerate()
    {
        months.push(month as u8 + 1);
        months.extend_from_slice(&(name.len() as u32).to_le_bytes());
        months.extend_from_slice(name.as_bytes());
    }
    let mut named = ColumnSpecWith::new(0, 20, 3);
    named.date = RawDateOptions {
        size: RawDateOptions::SIZE,
        flags: 0,
        pivot: 2029,
    };
    named.months = months.as_ptr();
    named.months_len = months.len();
    let read = read_with(b"07-Jan-26\n", &options(), false, &[named], &[]);
    assert!(read.rows[0][0].verdict.is_ok());
    assert_eq!(read.rows[0][0].value[..2], date.year.to_ne_bytes());
    // With notes null too: options don't need notes, and notes don't need options.
    let mut block = started(&options());
    let (mut values, mut verdicts) = (vec![0u8; 16], vec![CellVerdict::default()]);
    let mut cells = vec![Span::default(); 2];
    let columns = [ColumnBufferWith {
        size: ColumnBufferWith::SIZE,
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
        notes: std::ptr::null_mut(),
    }];
    let mut buffers = BuffersWith::empty();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    let mut out = Filled::default();
    let data = b"20260107\n";
    // SAFETY: room for the one row.
    let code = unsafe {
        hypertabular_delimited_fill_with(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            1,
            compact.as_ptr(),
            columns.as_ptr(),
            1,
            1,
            &buffers,
            &mut out,
        )
    };
    assert_eq!((code, out.rows, verdicts[0].reason), (OK, 1, 0));
}

/// A column's grouping is its own: in one read an Indian column takes `1,23,456` and refuses
/// `1,234,567`, and a Western one beside it does the reverse, through `_with` and `fill::fill`.
#[test]
fn a_column_s_grouping_reaches_its_door_alone() {
    let indian = RawNumFormat::from(NumFormat::INVARIANT.with_grouping(Grouping::Indian));
    let western = RawNumFormat::from(NumFormat::INVARIANT);
    assert_eq!(indian.format.flags, 607);
    assert_eq!(western.format.flags, 95);
    let mut columns = [
        ColumnSpecWith::new(0, I32, 0),
        ColumnSpecWith::new(1, I32, 0),
    ];
    columns[0].format = indian;
    columns[1].format = western;
    let data = b"\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";
    let read = read_with(data, &options(), false, &columns, &[]);
    assert!(read.rows[0][0].verdict.is_ok());
    assert_eq!(read.rows[0][0].value[..4], 123456i32.to_ne_bytes());
    assert_eq!(read.rows[0][1].verdict.reason, 2);
    assert_eq!(read.rows[0][1].note.detail, Detail::GroupSize as u16);
    assert_eq!(read.rows[1][0].verdict.reason, 2);
    assert_eq!(read.rows[1][0].note.detail, Detail::GroupSize as u16);
    assert!(read.rows[1][1].verdict.is_ok());
    assert_eq!(read.rows[1][1].value[..4], 1234567i32.to_ne_bytes());

    let mut state = State::init(CSV).unwrap();
    let mut spec = ColumnSpec::new(0, I32, 0);
    spec.format = indian;
    let (code, _, verdict) = call_one(&mut state, b"\"1,23,456\"\n", &spec);
    assert_eq!((code, verdict.reason), (OK, 0));
}

/// A format of all zeros is no format: `NumFormat::INVARIANT`, Western grouping, through `_with` as
/// through a 1.0 fill.
#[test]
fn no_format_is_western_grouping() {
    let column = ColumnSpecWith::new(0, I32, 0);
    assert_eq!(column.format.size, 0);
    let read = read_with(
        b"\"1,23,456\"\n\"1,234,567\"\n\"12,34\"\n",
        &options(),
        false,
        &[column],
        &[],
    );
    assert_eq!(read.rows[0][0].verdict.reason, 2);
    assert_eq!(read.rows[0][0].note.detail, Detail::GroupSize as u16);
    assert!(read.rows[1][0].verdict.is_ok());
    assert_eq!(read.rows[1][0].value[..4], 1234567i32.to_ne_bytes());
    assert_eq!(read.rows[2][0].verdict.reason, 2);

    // The same through the extensions' and the Rust API's `fill`.
    let mut state = State::init(CSV).unwrap();
    let spec = ColumnSpec::new(0, I32, 0);
    let (code, _, verdict) = call_one(&mut state, b"\"1,23,456\"\n", &spec);
    assert_eq!((code, verdict.reason), (OK, 2));
}

/// The words a format of 1.0's reads as, and the ones a read refuses: a retired grouping bit, set
/// alone or with Indian's, whatever the door and with a `size` or without one.
const RETIRED: [u32; 3] = [NumFormat::ALL | 256, NumFormat::ALL | 768, 256];

#[test]
fn a_retired_grouping_bit_is_refused_before_any_cell() {
    assert_eq!(RETIRED[0], 351);
    assert_eq!(RETIRED[1], 863);
    for door in [I32, TEXT] {
        for flags in RETIRED {
            let mut format = NumFormat::INVARIANT;
            format.flags = flags;
            let mut spec = [ColumnSpecWith::new(0, door, 0)];
            spec[0].format = RawNumFormat::from(format);
            refused(
                &spec,
                ColumnBufferWith::SIZE,
                BuffersWith::SIZE,
                &[],
                options(),
            );
            // The same word with a `size` of `0` isn't a format to ignore.
            spec[0].format.size = 0;
            refused(
                &spec,
                ColumnBufferWith::SIZE,
                BuffersWith::SIZE,
                &[],
                options(),
            );

            let mut plain = ColumnSpec::new(0, door, 0);
            plain.format = RawNumFormat::from(format);
            assert_eq!(plain.num_format(), None, "{door} {flags}");
            let mut state = State::init(CSV).unwrap();
            let (code, out, _) = call_one(&mut state, b"\"1,234\"\n", &plain);
            assert_eq!((code, out.rows), (ERR_CONTRACT, 0), "{door} {flags}");
            plain.format.size = 0;
            assert_eq!(plain.num_format(), None, "{door} {flags}, no size");
        }
    }
    // The control: the word it would be without the bit, and Indian's own.
    for flags in [NumFormat::ALL, NumFormat::ALL | NumFormat::GROUPING_INDIAN] {
        let mut format = NumFormat::INVARIANT;
        format.flags = flags;
        let mut spec = [ColumnSpecWith::new(0, I32, 0)];
        spec[0].format = RawNumFormat::from(format);
        let read = read_with(b"\"1,234\"\n", &options(), false, &spec, &[]);
        assert!(read.rows[0][0].verdict.is_ok(), "{flags}");
    }
}

#[test]
fn null_notes_write_nothing() {
    // The verdicts are the same with no notes or row widths to write: null is never written
    // through, and the loop that doesn't ask a door why gives the same answers.
    let data = b"12a,x\n7\n";
    let on = RawDelimitedOptions {
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        ..options()
    };
    let specs = [ColumnSpecWith::new(0, I32, 0), text_column(1)];
    let noted = read_split(data, &on, false, &specs, &[], data.len(), 4);

    let mut block = started(&on);
    let mut values = [[0u8; 64]; 2];
    let mut verdicts = [[CellVerdict::default(); 4]; 2];
    let mut cells = vec![Span::default(); 3 * 4];
    let columns: Vec<ColumnBufferWith> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
            notes: std::ptr::null_mut(),
        })
        .collect();
    let mut buffers = BuffersWith::empty();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    let mut out = Filled::default();
    // SAFETY: room for four rows of everything.
    let code = unsafe {
        hypertabular_delimited_fill_with(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            1,
            specs.as_ptr(),
            columns.as_ptr(),
            2,
            4,
            &buffers,
            &mut out,
        )
    };
    assert_eq!((code, out.rows), (OK, 2));
    for (row, seen) in noted.rows.iter().enumerate() {
        for (column, cell) in seen.iter().enumerate() {
            assert_eq!(verdicts[column][row], cell.verdict, "{row} {column}");
        }
    }
}

/// A fill_with that must refuse: nothing it was handed is written.
fn refused(
    specs: &[ColumnSpecWith],
    column_size: u32,
    buffers_size: u32,
    tokens: &[u8],
    state_options: RawDelimitedOptions,
) {
    let mut block = started(&state_options);
    let before = block.clone();
    let mut values = vec![0x11u8; 64];
    let mut verdicts = vec![
        CellVerdict {
            offset: 5,
            len: 5,
            reason: 5
        };
        4
    ];
    let mut notes = vec![
        CellNote {
            detail: 5,
            origin: 5
        };
        4
    ];
    let mut widths = vec![5u32; 4];
    let mut cells = vec![Span { offset: 5, len: 5 }; 16];
    let columns: Vec<ColumnBufferWith> = specs
        .iter()
        .map(|_| ColumnBufferWith {
            size: column_size,
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
            notes: notes.as_mut_ptr(),
        })
        .collect();
    let mut buffers = BuffersWith::empty();
    buffers.size = buffers_size;
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    buffers.row_widths = widths.as_mut_ptr();
    buffers.tokens = tokens.as_ptr();
    buffers.tokens_len = tokens.len();
    let sentinel = Filled {
        rows: 5,
        consumed: 5,
        arena_used: 5,
        needed: 5,
        failure: Failure::default(),
    };
    let mut out = sentinel;
    let data = b"1,2\n3,4\n";
    // SAFETY: room for four rows, whatever the sizes say.
    let code = unsafe {
        hypertabular_delimited_fill_with(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            1,
            specs.as_ptr(),
            columns.as_ptr(),
            specs.len(),
            4,
            &buffers,
            &mut out,
        )
    };
    assert_eq!(code, ERR_CONTRACT);
    assert_eq!(out, sentinel);
    assert_eq!(block, before);
    assert!(values.iter().all(|&byte| byte == 0x11));
    assert!(verdicts.iter().all(|verdict| verdict.reason == 5));
    assert!(notes.iter().all(|note| note.origin == 5));
    assert!(widths.iter().all(|&width| width == 5));
    assert!(cells.iter().all(|cell| cell.offset == 5));
}

#[test]
fn unknown_sizes_are_refused() {
    let good = [text_column(0)];
    let (column, buffers) = (ColumnBufferWith::SIZE, BuffersWith::SIZE);
    // A control: `refused` would fail on a call that isn't.
    let mut block = started(&options());
    let mut sentinel = block.clone();
    // SAFETY: the options are live, the block is the size the library asks.
    let code = unsafe {
        hypertabular_delimited_init_with(
            block.as_mut_ptr().cast(),
            &RawDelimitedOptions {
                size: RawDelimitedOptions::SIZE + 1,
                ..options()
            },
        )
    };
    assert_eq!(code, ERR_CONTRACT);
    assert_eq!(block, sentinel);
    // Every option the state can't hold.
    for bad in [
        RawDelimitedOptions {
            separator: b'"',
            ..options()
        },
        RawDelimitedOptions {
            short_rows: 2,
            ..options()
        },
        RawDelimitedOptions {
            long_rows: 3,
            ..options()
        },
        RawDelimitedOptions {
            engine: 200,
            ..options()
        },
    ] {
        // SAFETY: as above.
        let code = unsafe { hypertabular_delimited_init_with(block.as_mut_ptr().cast(), &bad) };
        assert_eq!(code, ERR_CONTRACT, "{bad:?}");
        assert_eq!(block, sentinel);
    }
    sentinel.clear();

    refused(&good, column + 1, buffers, &[], options());
    refused(&good, column, buffers + 1, &[], options());
    let mut spec = good;
    spec[0].size += 1;
    refused(&spec, column, buffers, &[], options());
    // Inside the column spec: the format's size, the date options' size, the bit and the pivot
    // the door can't honour, a vocabulary with a month missing.
    let mut spec = good;
    spec[0].format.size = RawNumFormat::SIZE + 1;
    refused(&spec, column, buffers, &[], options());
    let mut spec = good;
    spec[0].date.size = RawDateOptions::SIZE + 1;
    refused(&spec, column, buffers, &[], options());
    let mut spec = good;
    spec[0].date = RawDateOptions {
        size: RawDateOptions::SIZE,
        flags: 1 << 20,
        pivot: 0,
    };
    refused(&spec, column, buffers, &[], options());
    let mut spec = good;
    spec[0].date = RawDateOptions {
        size: RawDateOptions::SIZE,
        flags: 0,
        pivot: 1 << 20,
    };
    refused(&spec, column, buffers, &[], options());
    let mut spec = good;
    let months = [1u8, 1, 0, 0, 0, b'J'];
    spec[0].months = months.as_ptr();
    spec[0].months_len = months.len();
    refused(&spec, column, buffers, &[], options());
    let mut spec = good;
    spec[0].months_len = 3;
    refused(&spec, column, buffers, &[], options());
    // An undefined door; a token list cut short, in the buffers and in a column; a length with
    // no list.
    let mut spec = good;
    spec[0].door = 0;
    refused(&spec, column, buffers, &[], options());
    refused(&good, column, buffers, &[9, 0, 0, 0, b'x'], options());
    refused(&good, column, buffers, &[1, 0], options());
    let cut = [2u8, 0, 0, 0, b'x'];
    refused(
        &[with_tokens(text_column(0), &cut)],
        column,
        buffers,
        &[],
        options(),
    );
    let mut spec = good;
    spec[0].tokens_len = 4;
    refused(&spec, column, buffers, &[], options());
}

/// What the 1.0 surface promises a caller who declared 64 bytes because the 1.0 header said so:
/// the core never touches a byte past them. Those bytes hold what would change the read if they
/// were read as the options a longer state holds.
#[test]
fn one_point_oh_callers_keep_their_64_bytes() {
    assert_eq!(State::SIZE as usize, size_of::<State>());
    // SAFETY: no contract.
    assert_eq!(
        unsafe { hypertabular_delimited_state_size() },
        size_of::<State>()
    );
    const WORDS: usize = 8;
    let tail = [0x0101_0101_0101_0101u64; 2];
    let mut block = [0u64; WORDS + 2];
    block[WORDS..].copy_from_slice(&tail);
    let state = block.as_mut_ptr().cast::<State>();
    let dialect = CSV;
    let data = b"a,b\n1,2\n3,4\n";
    let mut names = [Span::default(); 4];
    let mut cells = [Span::default(); 16];
    let mut values = [0u8; 64];
    let mut verdicts = [CellVerdict::default(); 4];
    let spec = [ColumnSpec::new(0, TEXT, 0)];
    let columns = [ColumnBuffer {
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
    }];
    let buffers = hypertabular::kernel::abi::Buffers {
        window: std::ptr::null_mut(),
        window_cap: 0,
        arena: std::ptr::null_mut(),
        arena_cap: 0,
        cells: cells.as_mut_ptr(),
        cells_cap: cells.len(),
        row: std::ptr::null_mut(),
        row_cap: 0,
        strings: std::ptr::null(),
        strings_len: 0,
        table: std::ptr::null(),
        table_len: 0,
        kinds: std::ptr::null(),
        kinds_len: 0,
    };
    let mut out = Filled::default();
    // SAFETY: the block's first 64 bytes are what a 1.0 caller gives the core.
    unsafe {
        assert_eq!(hypertabular_delimited_init(state, &dialect), OK);
        assert_eq!(block[WORDS..], tail, "init");
        let code = hypertabular_delimited_header(
            state,
            data.as_ptr(),
            data.len(),
            1,
            names.as_mut_ptr(),
            names.len(),
            std::ptr::null_mut(),
            0,
            &mut out,
        );
        assert_eq!(
            (code, out.rows, out.consumed),
            (OK, 2, 4),
            "no skip, no discard"
        );
        assert_eq!(block[WORDS..], tail, "header");
        let rest = &data[out.consumed as usize..];
        let code = hypertabular_delimited_fill(
            state,
            rest.as_ptr(),
            rest.len(),
            1,
            spec.as_ptr(),
            columns.as_ptr(),
            1,
            4,
            &buffers,
            &mut out,
        );
        assert_eq!((code, out.rows), (OK, 2));
        assert_eq!(block[WORDS..], tail, "fill");
    }
    // The fields a 1.0 caller reads are where they were.
    let line = u32::from_ne_bytes(block[0].to_ne_bytes()[4..8].try_into().unwrap());
    assert_eq!(line, 4);
    assert_eq!(block[1], 3, "records");
    assert_eq!(block[2], data.len() as u64, "offset");

    // A state from `init_with` reads through the 1.0 fill, its options still its own.
    let on = RawDelimitedOptions {
        skip: 1,
        ..options()
    };
    let mut block = started(&on);
    let mut out = Filled::default();
    // SAFETY: as above, with a block of the size the library asks for.
    let code = unsafe {
        hypertabular_delimited_fill(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            1,
            spec.as_ptr(),
            columns.as_ptr(),
            1,
            4,
            &buffers,
            &mut out,
        )
    };
    assert_eq!(
        (code, out.rows),
        (OK, 2),
        "the header line skipped: 1 and 3"
    );
    // And a state from the 1.0 `init` through `fill_with`.
    let mut block = [0u64; WORDS];
    let state = block.as_mut_ptr().cast::<State>();
    let specs = [text_column(0)];
    let mut notes = [CellNote::default(); 4];
    let with = [ColumnBufferWith {
        size: ColumnBufferWith::SIZE,
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
        notes: notes.as_mut_ptr(),
    }];
    let mut buffers = BuffersWith::empty();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    // SAFETY: as above.
    unsafe {
        assert_eq!(hypertabular_delimited_init(state, &dialect), OK);
        let code = hypertabular_delimited_fill_with(
            state,
            data.as_ptr(),
            data.len(),
            1,
            specs.as_ptr(),
            with.as_ptr(),
            1,
            4,
            &buffers,
            &mut out,
        );
        assert_eq!((code, out.rows), (OK, 3));
    }
}

/// The region check meets what isn't delivered: a skipped line, a discarded record or a dropped
/// cell holding bad bytes never refuses a row or faults a cell. At most it sends the batch down
/// the slow path, which gives the same answer.
#[test]
fn what_isnt_delivered_cant_poison_a_verdict() {
    let columns = [text_column(0)];
    let clean = read_split(b"a\n1\n", &options(), true, &columns, &[], 4, 16);
    assert_eq!(clean.header, [b"a"]);

    let on = RawDelimitedOptions {
        skip: 1,
        ..options()
    };
    let read = read_with(b"\xff\xfe\na\n1\n", &on, true, &columns, &[]);
    assert_eq!(
        (read.header.clone(), read.failure),
        (vec![b"a".to_vec()], None)
    );
    assert_eq!(read.rows, clean.rows.clone());

    let on = RawDelimitedOptions {
        header_row: 1,
        ..options()
    };
    let read = read_with(b"\xff,\xfe\na\n1\n", &on, true, &columns, &[]);
    assert_eq!(
        (read.header.clone(), read.failure),
        (vec![b"a".to_vec()], None)
    );
    assert_eq!(read.rows, clean.rows.clone());

    // Without a header, the same through the fill alone.
    let read = read_with(b"\xff\n1\n", &on, false, &columns, &[]);
    assert_eq!(read.rows.len(), 1);
    assert_eq!(read.rows[0][0].text, b"1");
    assert_eq!(read.rows[0][0].verdict, CellVerdict::OK);

    // A bad byte in a cell the row kept is still that cell's alone.
    let read = read_with(b"\xff\n\xfe\n1\n", &on, false, &columns, &[]);
    assert_eq!(read.rows[0][0].verdict.reason, 2);
    assert_eq!(read.rows[1][0].verdict, CellVerdict::OK);

    // A token is matched before the text door, on bytes the door would refuse.
    let tokens = packed(&[b"\xff"]);
    let read = read_with(b"\xff\nx\n", &options(), false, &columns, &tokens);
    assert_eq!(read.rows[0][0].verdict, EMPTY);
    assert_eq!(read.rows[0][0].note.origin, CellNote::ORIGIN_TOKEN);
}

// ---- The region check against the cell rule, across plans, chunks and buffers ---------------

/// Which export a [`read_chaotic`] goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FillExport {
    OnePointOh,
    With,
}

/// A cell the differential reads: `(reason, offset, len, text)`.
type ChaoticCell = (u32, u32, u32, Vec<u8>);

/// What a [`read_chaotic`] delivered: the header, the rows, and a failure's code, record, byte
/// and line.
type Chaotic = (
    Option<Vec<Vec<u8>>>,
    Vec<Vec<ChaoticCell>>,
    Option<(u32, u64, u64, u32)>,
);

/// One reading of `data` through a plan, by `export`, with each text cell held on the spot to
/// HyperCast's door asked about that cell. With `stingy` randomness, the chunks are random, each
/// batch is a random size, and the arena, the name table and the cell table all start as small
/// as they can and grow only when the core refuses, keeping nothing consumed.
#[allow(clippy::type_complexity)]
fn read_chaotic(
    data: &[u8],
    dialect: RawDialect,
    specs: &[ColumnSpec],
    header: bool,
    export: FillExport,
    stingy: Option<&mut Random>,
    max_rows: usize,
) -> Chaotic {
    let mut unused = Random(1);
    let is_stingy = stingy.is_some();
    let random = stingy.unwrap_or(&mut unused);
    let mut block = state_block();
    let state = block.as_mut_ptr().cast::<State>();
    // SAFETY: the block is the size the library asks; the dialect is live.
    assert_eq!(unsafe { hypertabular_delimited_init(state, &dialect) }, OK);
    let per_row = specs.iter().map(|s| s.ordinal as usize).max().unwrap_or(0) + 2;
    let mut arena = vec![
        0u8;
        if is_stingy {
            random.below(6)
        } else {
            data.len() + 1
        }
    ];
    let mut cells = vec![
        Span::default();
        if is_stingy {
            per_row * random.below(3)
        } else {
            per_row * max_rows
        }
    ];
    let mut names = vec![Span::default(); if is_stingy { 1 } else { 64 }];
    let with: Vec<ColumnSpecWith> = specs
        .iter()
        .map(|spec| ColumnSpecWith::new(spec.ordinal, spec.door, spec.param))
        .collect();
    let mut at = 0usize;
    let mut end = if is_stingy {
        random.below(data.len() + 1)
    } else {
        data.len()
    };
    let mut head = None;
    let mut need_header = header;
    let mut rows = Vec::new();
    let mut spins = 0;
    loop {
        spins += 1;
        assert!(spins < 100_000, "no progress on {data:?}");
        let last = end == data.len();
        let input = &data[at..end];
        let mut out = Filled::default();
        if need_header {
            // SAFETY: each buffer is the size given beside it.
            let code = unsafe {
                hypertabular_delimited_header(
                    state,
                    input.as_ptr(),
                    input.len(),
                    u32::from(last),
                    names.as_mut_ptr(),
                    names.len(),
                    arena.as_mut_ptr(),
                    arena.len(),
                    &mut out,
                )
            };
            match code {
                OK => {
                    if out.rows > 0 {
                        let mut seen = Vec::new();
                        for name in &names[..out.rows as usize] {
                            let store: &[u8] = if name.flagged() { &arena } else { input };
                            let start = name.offset as usize;
                            let bytes = store[start..start + name.len()].to_vec();
                            assert!(std::str::from_utf8(&bytes).is_ok(), "{bytes:?} got through");
                            seen.push(bytes);
                        }
                        head = Some(seen);
                        need_header = false;
                    } else if last && out.consumed as usize == input.len() {
                        return (Some(Vec::new()), rows, None);
                    }
                    at += out.consumed as usize;
                    if out.rows == 0 && out.consumed == 0 {
                        if last {
                            return (head, rows, None);
                        }
                        end = (end + 1 + random.below(9)).min(data.len());
                    }
                    continue;
                }
                ERR_CELLS => {
                    assert_eq!(out.consumed, 0);
                    names.resize(out.needed as usize, Span::default());
                    continue;
                }
                ERR_ARENA => {
                    assert_eq!(out.consumed, 0);
                    arena.resize(out.needed as usize, 0);
                    continue;
                }
                ERR_STRUCTURE => {
                    let f = out.failure;
                    return (head, rows, Some((f.code, f.record, f.byte, f.line)));
                }
                other => panic!("the header returned {other}"),
            }
        }
        let batch = if is_stingy {
            1 + random.below(max_rows)
        } else {
            max_rows
        };
        let mut values = vec![vec![0xAAu8; 32 * batch]; specs.len()];
        let mut verdicts = vec![
            vec![
                CellVerdict {
                    offset: 77,
                    len: 77,
                    reason: 77
                };
                batch
            ];
            specs.len()
        ];
        // SAFETY: room for `batch` of everything, in both shapes of the call.
        let code = unsafe {
            match export {
                FillExport::OnePointOh => {
                    let columns: Vec<ColumnBuffer> = values
                        .iter_mut()
                        .zip(verdicts.iter_mut())
                        .map(|(values, verdicts)| ColumnBuffer {
                            values: values.as_mut_ptr().cast(),
                            verdicts: verdicts.as_mut_ptr(),
                        })
                        .collect();
                    let mut buffers = BuffersWith::empty();
                    buffers.arena = arena.as_mut_ptr();
                    buffers.arena_cap = arena.len();
                    buffers.cells = cells.as_mut_ptr();
                    buffers.cells_cap = cells.len();
                    hypertabular_delimited_fill(
                        state,
                        input.as_ptr(),
                        input.len(),
                        u32::from(last),
                        specs.as_ptr(),
                        columns.as_ptr(),
                        specs.len(),
                        batch,
                        &buffers.buffers(),
                        &mut out,
                    )
                }
                FillExport::With => {
                    let columns: Vec<ColumnBufferWith> = values
                        .iter_mut()
                        .zip(verdicts.iter_mut())
                        .map(|(values, verdicts)| ColumnBufferWith {
                            size: ColumnBufferWith::SIZE,
                            values: values.as_mut_ptr().cast(),
                            verdicts: verdicts.as_mut_ptr(),
                            notes: std::ptr::null_mut(),
                        })
                        .collect();
                    let mut buffers = BuffersWith::empty();
                    buffers.arena = arena.as_mut_ptr();
                    buffers.arena_cap = arena.len();
                    buffers.cells = cells.as_mut_ptr();
                    buffers.cells_cap = cells.len();
                    hypertabular_delimited_fill_with(
                        state,
                        input.as_ptr(),
                        input.len(),
                        u32::from(last),
                        with.as_ptr(),
                        columns.as_ptr(),
                        with.len(),
                        batch,
                        &buffers,
                        &mut out,
                    )
                }
            }
        };
        match code {
            OK => {}
            ERR_ARENA => {
                assert_eq!((out.consumed, out.rows), (0, 0));
                arena.resize(out.needed as usize, 0);
                continue;
            }
            ERR_CELLS => {
                assert_eq!((out.consumed, out.rows), (0, 0));
                let many = 1 + random.below(3);
                cells.resize(out.needed as usize * many, Span::default());
                continue;
            }
            ERR_STRUCTURE => {
                let f = out.failure;
                return (head, rows, Some((f.code, f.record, f.byte, f.line)));
            }
            other => panic!("the fill returned {other}"),
        }
        for row in 0..out.rows as usize {
            let mut seen = Vec::new();
            for (column, spec) in specs.iter().enumerate() {
                let verdict = verdicts[column][row];
                assert_ne!(verdict.reason, 77, "a verdict was never written");
                let mut text = Vec::new();
                if spec.door == TEXT {
                    let cell = cells[row * per_row + spec.ordinal as usize];
                    let start = cell.offset as usize;
                    let raw = &input[start..start + cell.len()];
                    let mut unescaped = vec![0u8; raw.len()];
                    let bytes = if cell.flagged() {
                        let written = unescape_into(raw, &mut unescaped);
                        &unescaped[..written]
                    } else {
                        raw
                    };
                    match hypercast::cast_text(bytes) {
                        Ok(_) => {
                            assert_eq!(verdict, CellVerdict::OK, "{data:?}");
                            // SAFETY: within the column; the read is unaligned.
                            let span: Span = unsafe {
                                values[column]
                                    .as_ptr()
                                    .add(row * size_of::<Span>())
                                    .cast::<Span>()
                                    .read_unaligned()
                            };
                            let store: &[u8] = if span.flagged() { &arena } else { input };
                            let from = span.offset as usize;
                            text = store[from..from + span.len()].to_vec();
                            assert_eq!(text, bytes);
                        }
                        Err(fault) => assert_eq!(
                            verdict,
                            CellVerdict::from_fault(fault),
                            "{data:?}, row {row}, column {column}"
                        ),
                    }
                }
                seen.push((verdict.reason, verdict.offset, verdict.len, text));
            }
            rows.push(seen);
        }
        at += out.consumed as usize;
        if out.rows == 0 && out.consumed == 0 {
            if last {
                return (head, rows, None);
            }
            end = (end + 1 + random.below(9)).min(data.len());
            continue;
        }
        if last && at == data.len() {
            return (head, rows, None);
        }
        if end < data.len() && random.below(2) == 0 {
            end = (end + random.below(9)).min(data.len());
        }
    }
}

/// A cell of [`the_region_check_holds_across_plans_and_buffers`]'s files: valid or broken text,
/// plain, quoted, escaped, holding structure, or with stray quotes.
fn chaotic_cell(random: &mut Random, separator: u8, out: &mut Vec<u8>) {
    const PIECES: &[&[u8]] = &[
        b"a",
        b"7",
        "\u{e9}".as_bytes(),
        "\u{20ac}".as_bytes(),
        "\u{1f600}".as_bytes(),
        "\u{feff}".as_bytes(),
        b"\0",
        b"\xC3",
        b"\xA9",
        b"\xE2\x82",
        b"\xED\xA0\x80",
        b"\xFF",
        b"\xEF\xBB",
    ];
    fn body(random: &mut Random, out: &mut Vec<u8>, clean: bool) {
        for _ in 0..random.below(4) {
            let pieces = if clean { 7 } else { PIECES.len() };
            out.extend_from_slice(PIECES[random.below(pieces)]);
        }
    }
    let clean = random.below(3) != 0;
    match random.below(7) {
        0..=2 => body(random, out, clean),
        3 => {
            out.push(b'"');
            body(random, out, clean);
            out.push(b'"');
        }
        4 => {
            out.push(b'"');
            body(random, out, clean);
            out.extend_from_slice(b"\"\"");
            body(random, out, clean);
            out.push(b'"');
        }
        5 => {
            out.push(b'"');
            body(random, out, clean);
            out.push(separator);
            out.push(b'\n');
            body(random, out, clean);
            out.extend_from_slice(b"\r\n\"");
        }
        _ => {
            out.push(b'"');
            body(random, out, clean);
            out.push(b'"');
            body(random, out, clean);
            out.push(b'"');
            body(random, out, clean);
        }
    }
}

/// [`the_region_check_gives_what_each_cell_would`], on what it leaves out: a plan reading only
/// some ordinals, or none through the text door; a byte-order mark; blank lines; every line
/// ending; rows of the wrong width; and a stingy arena and cell table grown on refusal. Each
/// file goes through both fill exports, which must agree with each other too.
#[test]
fn the_region_check_holds_across_plans_and_buffers() {
    let mut random = Random(0xFEED_F00D_1234_5678);
    let (mut seen, mut malformed, mut failures, mut no_text) = (0u64, 0u64, 0u64, 0u64);
    for _ in 0..1500 {
        let separator = b",;\t"[random.below(3)];
        let quoting = (random.below(5) != 0) as u8;
        let skip_blank_lines = random.below(2) as u8;
        let width = 1 + random.below(4);
        let mut data = Vec::new();
        if random.below(4) == 0 {
            data.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        let header = random.below(3) == 0;
        for row in 0..1 + random.below(6) {
            if random.below(6) == 0 {
                data.push(b'\n');
            }
            let cells = if random.below(25) == 0 {
                width + 1
            } else {
                width
            };
            for column in 0..cells {
                if column > 0 {
                    data.push(separator);
                }
                if row == 0 && header && random.below(2) == 0 {
                    data.push(b'n');
                } else {
                    chaotic_cell(&mut random, separator, &mut data);
                }
            }
            data.extend_from_slice([&b"\n"[..], b"\r\n", b"\r"][random.below(3)]);
        }
        if random.below(4) == 0 {
            data.pop();
        }
        let mut specs = Vec::new();
        for _ in 0..1 + random.below(3) {
            let ordinal = random.below(width + 1) as u32;
            let door = if random.below(4) == 0 { I32 } else { TEXT };
            specs.push(ColumnSpec::new(ordinal, door, 0));
        }
        if specs.iter().all(|spec| spec.door != TEXT) {
            no_text += 1;
        }
        for kind in available() {
            let dialect = RawDialect {
                separator,
                quoting,
                skip_blank_lines,
                engine: kind.code(),
            };
            let whole = read_chaotic(
                &data,
                dialect,
                &specs,
                header,
                FillExport::OnePointOh,
                None,
                64,
            );
            if whole.2.is_some() {
                failures += 1;
            }
            for row in &whole.1 {
                for cell in row {
                    seen += 1;
                    if cell.0 == 2 {
                        malformed += 1;
                    }
                }
            }
            for export in [FillExport::OnePointOh, FillExport::With] {
                assert_eq!(
                    read_chaotic(&data, dialect, &specs, header, export, None, 64),
                    whole,
                    "{data:?} by {export:?}, whole"
                );
                assert_eq!(
                    read_chaotic(&data, dialect, &specs, header, export, None, 1),
                    whole,
                    "{data:?} by {export:?}, a row a batch"
                );
                for _ in 0..3 {
                    assert_eq!(
                        read_chaotic(&data, dialect, &specs, header, export, Some(&mut random), 4),
                        whole,
                        "{data:?} {specs:?} by {export:?}, stingy"
                    );
                }
            }
        }
    }
    assert!(
        seen > 1000 && malformed > 100,
        "{seen} cells, {malformed} malformed"
    );
    assert!(
        failures > 25 && no_text > 25,
        "{failures} failed, {no_text} no text"
    );
}

/// The cases a generator takes a while to reach, each said by hand.
#[test]
fn the_region_check_on_cases_said_by_hand() {
    let specs = [ColumnSpec::new(0, TEXT, 0)];
    for export in [FillExport::OnePointOh, FillExport::With] {
        // A header fine with a bad row behind it: no failure, and the bad row is a verdict.
        let read = read_chaotic(b"a,b\n\xff,\xff\n", CSV, &specs, true, export, None, 8);
        assert_eq!(read.2, None);
        assert_eq!(read.1, vec![vec![(2, 0, 1, Vec::new())]]);
        // A bad byte in the second column of the header.
        let read = read_chaotic(b"a,\xff\n1,2\n", CSV, &specs, true, export, None, 8);
        assert_eq!(read.2, Some((Failure::ENCODING, 0, 0, 1)));
        // An empty header name is fine.
        let read = read_chaotic(b"a,\n1,2\n", CSV, &specs, true, export, None, 8);
        assert_eq!(read.2, None);
        // Bad bytes in the refused row: the rows before it are ok, and the failure comes after.
        let read = read_chaotic(b"\xc3\xa9,x\n\xff\n", CSV, &specs, false, export, None, 4);
        assert_eq!(read.1[0][0].0, 0);
        assert_eq!(read.2.map(|failure| failure.0), Some(Failure::COLUMN_COUNT));
        // A plan with no text column: bad bytes through a typed door are that door's verdict.
        let typed = [ColumnSpec::new(0, I32, 0)];
        let read = read_chaotic(b"\xff\n", CSV, &typed, false, export, None, 4);
        assert_eq!((read.1[0][0].0, read.1[0][0].1), (2, 0));
    }
    // A sequence split across chunks, with the row terminated only in the next.
    let data = "\u{e9},x\n".as_bytes();
    for split in 0..data.len() {
        let mut state = State::init(CSV).unwrap();
        let (code, out, ..) = call(&mut state, &data[..split], false, &specs, 4);
        assert_eq!((code, out.rows, out.consumed), (OK, 0, 0), "{split}");
    }
}

fn call_with_room(
    block: &mut Vec<u64>,
    data: &[u8],
    last: u32,
    cells: &mut [Span],
    arena: &mut [u8],
) -> (i32, Filled) {
    let mut values = [0u8; 256];
    let mut verdicts = [CellVerdict::default(); 8];
    let specs = [text_column(0)];
    let columns = [ColumnBufferWith {
        size: ColumnBufferWith::SIZE,
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
        notes: std::ptr::null_mut(),
    }];
    let mut b = BuffersWith::empty();
    b.cells = cells.as_mut_ptr();
    b.cells_cap = cells.len();
    b.arena = arena.as_mut_ptr();
    b.arena_cap = arena.len();
    let mut out = Filled::default();
    // SAFETY: every buffer is the size given beside it.
    let code = unsafe {
        hypertabular_delimited_fill_with(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            last,
            specs.as_ptr(),
            columns.as_ptr(),
            1,
            8,
            &b,
            &mut out,
        )
    };
    (code, out)
}

#[test]
fn too_small_consumes_nothing_past_skipped_lines() {
    let on = RawDelimitedOptions {
        skip: 2,
        header_row: 1,
        ..options()
    };
    let data = b"\xEF\xBB\xBFt\"1\nt2\r\nx,y\n\"a\"\"b\",1\nc,2\n";
    let mut block = started(&on);
    let before = block.clone();
    let (code, out) = call_with_room(&mut block, data, 1, &mut [], &mut []);
    assert_eq!((code, out.consumed, out.rows), (ERR_CELLS, 0, 0));
    assert_eq!(block, before, "cells refused: state changed");
    let mut cells = [Span::default(); 16];
    let (code, out) = call_with_room(&mut block, data, 1, &mut cells, &mut []);
    assert_eq!((code, out.consumed, out.rows), (ERR_ARENA, 0, 0));
    assert_eq!(block, before, "arena refused: state changed");
    let mut arena = [0u8; 16];
    let (code, out) = call_with_room(&mut block, data, 1, &mut cells, &mut arena);
    assert_eq!((code, out.rows, out.consumed as usize), (OK, 2, data.len()));
    assert_eq!(cells[1].len, 4, "first row is on physical line 4");
    assert_eq!(cells[3].len, 5, "second row is on physical line 5");

    // the header refusing keeps the state too
    let mut block = started(&on);
    let before = block.clone();
    let mut out = Filled::default();
    // SAFETY: nothing is written: the names table holds no name.
    let code = unsafe {
        hypertabular_delimited_header(
            block.as_mut_ptr().cast(),
            data.as_ptr(),
            data.len(),
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            &mut out,
        )
    };
    assert_eq!((code, out.consumed), (ERR_CELLS, 0));
    assert_eq!(block, before);
}

#[test]
fn sizes_minus_one_and_more_are_refused() {
    let good = [text_column(0)];
    let (c, b) = (ColumnBufferWith::SIZE, BuffersWith::SIZE);
    for bad in [c - 1, 0, 1, u32::MAX] {
        refused(&good, bad, b, &[], options());
    }
    for bad in [b - 1, 0, 1, u32::MAX] {
        refused(&good, c, bad, &[], options());
    }
    for bad in [ColumnSpecWith::SIZE - 1, 0, 1, u32::MAX] {
        let mut s = good;
        s[0].size = bad;
        refused(&s, c, b, &[], options());
    }
    let mut block = started(&options());
    let snapshot = block.clone();
    for size in [
        RawDelimitedOptions::SIZE - 1,
        RawDelimitedOptions::SIZE + 1,
        0,
        4,
        u32::MAX,
    ] {
        // SAFETY: the block is as large as the library asks, and the options are live.
        let code = unsafe {
            hypertabular_delimited_init_with(
                block.as_mut_ptr().cast(),
                &RawDelimitedOptions { size, ..options() },
            )
        };
        assert_eq!(code, ERR_CONTRACT, "{size}");
        assert_eq!(block, snapshot);
    }
    for bad in [RawNumFormat::SIZE + 1, 1, RawNumFormat::MIN_SIZE - 1] {
        let mut s = good;
        s[0].format.size = bad;
        refused(&s, c, b, &[], options());
    }
    for bad in [
        RawDateOptions::SIZE - 1,
        RawDateOptions::SIZE + 1,
        1,
        RawDateOptions::MIN_SIZE - 1,
    ] {
        let mut s = good;
        s[0].date.size = bad;
        refused(&s, c, b, &[], options());
    }
    // tokens malformed: every truncation of a good list that isn't whole, and absurd lengths
    let list = packed(&[b"NA", b"", b"xyz"]);
    let whole = [0usize, 6, 10, 17];
    for cut in 0..list.len() {
        if whole.contains(&cut) {
            continue;
        }
        refused(&good, c, b, &list[..cut], options());
        refused(
            &[with_tokens(text_column(0), &list[..cut])],
            c,
            b,
            &[],
            options(),
        );
    }
    for head in [u32::MAX, u32::MAX - 3, i32::MAX as u32, 0x8000_0000] {
        let mut l = head.to_le_bytes().to_vec();
        l.push(b'x');
        refused(&good, c, b, &l, options());
        refused(&[with_tokens(text_column(0), &l)], c, b, &[], options());
    }
    // a whole list passes the same gate (control)
    let r = read_split(b"NA,1\n", &options(), false, &good, &list, 5, 4);
    assert_eq!(r.rows[0][0].note.origin, CellNote::ORIGIN_TOKEN);
}

/// The notes of a batch don't outlive it: a cell with nothing to note, in a column that faulted
/// last time, says so. A fault-heavy column and a clean one, the same notes array.
#[test]
fn a_note_is_cleared_by_the_next_batch() {
    let mut block = started(&options());
    let (mut values, mut verdicts) = (vec![0u8; 16], vec![CellVerdict::default(); 4]);
    let mut notes = vec![
        CellNote {
            detail: 99,
            origin: 99
        };
        5
    ];
    let mut cells = vec![Span::default(); 8];
    let specs = [ColumnSpecWith::new(0, I32, 0)];
    let columns = [ColumnBufferWith {
        size: ColumnBufferWith::SIZE,
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
        notes: notes.as_mut_ptr(),
    }];
    let mut buffers = BuffersWith::empty();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    let mut fill = |data: &[u8]| {
        let mut out = Filled::default();
        // SAFETY: room for four rows of everything.
        let code = unsafe {
            hypertabular_delimited_fill_with(
                block.as_mut_ptr().cast(),
                data.as_ptr(),
                data.len(),
                1,
                specs.as_ptr(),
                columns.as_ptr(),
                1,
                4,
                &buffers,
                &mut out,
            )
        };
        assert_eq!(
            (code, out.rows as usize),
            (OK, data.iter().filter(|&&b| b == b'\n').count())
        );
    };
    fill(b"a\nb\n1\nc\n");
    let fault = CellNote {
        detail: Detail::UnexpectedCharacter as u16,
        origin: 0,
    };
    assert_eq!(notes[..4], [fault, fault, CellNote::default(), fault]);
    assert_eq!(notes[4].origin, 99, "a note past the rows is untouched");
    fill(b"1\n2\n");
    assert_eq!(notes[..2], [CellNote::default(); 2]);
    assert_eq!(notes[4].origin, 99);
}

/// `_fill` reads a format's flags as 1.0 did, from HyperCast's own rule: bits 8 and up, defined
/// now or not, are nothing to it. The same format through `fill::fill`, which the extensions and
/// the Rust API call, means what this version says, and `_fill_with` refuses what it can't honour.
#[test]
fn undefined_flags_are_nothing_to_fill_and_refused_by_fill_with() {
    let read = |flags: u32| -> (i32, Vec<CellVerdict>) {
        let mut format = NumFormat::INVARIANT;
        format.flags = flags;
        let mut spec = ColumnSpec::new(0, I32, 0);
        spec.format = RawNumFormat::from(format);
        let mut state = [0u64; 8];
        let (mut values, mut verdicts) = ([0u8; 16], [CellVerdict::default(); 2]);
        let mut cells = [Span::default(); 4];
        let columns = [ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        }];
        let mut buffers = BuffersWith::empty();
        buffers.cells = cells.as_mut_ptr();
        buffers.cells_cap = cells.len();
        let plain = buffers.buffers();
        let data = b"\"1,23,456\"\n\"1,234\"\n";
        let mut out = Filled::default();
        // SAFETY: the 64 bytes a 1.0 caller gives, and room for two rows.
        let code = unsafe {
            assert_eq!(
                hypertabular_delimited_init(state.as_mut_ptr().cast(), &CSV),
                OK
            );
            hypertabular_delimited_fill(
                state.as_mut_ptr().cast(),
                data.as_ptr(),
                data.len(),
                1,
                [spec].as_ptr(),
                columns.as_ptr(),
                1,
                2,
                &plain,
                &mut out,
            )
        };
        (code, verdicts.to_vec())
    };
    let western = read(NumFormat::ALL);
    assert_eq!(western.0, OK);
    assert_eq!(western.1[0].reason, 2, "`1,23,456` is refused as Western");
    assert!(western.1[1].is_ok(), "`1,234` reads");
    assert_eq!(
        read(NumFormat::ALL | (1 << 30)),
        western,
        "an undefined bit"
    );
    for (flags, name) in [
        (351, "the retired bit"),
        (863, "policy 11"),
        (607, "Indian"),
    ] {
        assert_eq!(read(flags), western, "{name}: every format reads Western");
    }

    // Below that line, what a bad layout always was.
    let mut spec = ColumnSpec::new(0, I32, 0);
    spec.format = RawNumFormat::from(NumFormat::new('.', '.', NumFormat::ALL));
    assert_eq!(spec.num_format_1_0(), None, "equal separators");
    spec.format = RawNumFormat::from(NumFormat::INVARIANT);
    spec.format.size = RawNumFormat::SIZE + 1;
    assert_eq!(spec.num_format_1_0(), None, "a size the core doesn't read");

    // The extensions' and the Rust API's `fill`: a policy means what it says.
    let mut state = State::init(CSV).unwrap();
    let mut spec = ColumnSpec::new(0, I32, 0);
    spec.format = RawNumFormat::from(NumFormat::INVARIANT);
    let (code, _, verdicts) = call_one(&mut state, b"\"1,23,456\"\n", &spec);
    assert_eq!((code, verdicts.reason), (OK, 2));
    let mut state = State::init(CSV).unwrap();
    spec.format = RawNumFormat::from(NumFormat::INVARIANT.with_grouping(Grouping::Indian));
    let (code, _, verdicts) = call_one(&mut state, b"\"1,23,456\"\n", &spec);
    assert_eq!((code, verdicts.reason), (OK, 0));

    // `_fill_with` refuses a bit it can't honour, and takes the same format without it.
    let good = [text_column(0)];
    for flags in [
        NumFormat::ALL | (1 << 30),
        NumFormat::ALL | NumFormat::GROUPING_POLICY,
        NumFormat::ALL | 256,
    ] {
        let mut format = NumFormat::INVARIANT;
        format.flags = flags;
        let mut spec = good;
        spec[0].format = RawNumFormat::from(format);
        refused(
            &spec,
            ColumnBufferWith::SIZE,
            BuffersWith::SIZE,
            &[],
            options(),
        );
    }
    let mut spec = [ColumnSpecWith::new(0, I32, 0)];
    spec[0].format = RawNumFormat::from(NumFormat::INVARIANT);
    let western = read_split(b"\"1,23,456\"\n", &options(), false, &spec, &[], 8, 4);
    assert_eq!(western.rows[0][0].verdict.reason, 2);
}

/// One row through `fill::fill` and a plan of `spec`.
fn call_one(state: &mut State, data: &[u8], spec: &ColumnSpec) -> (i32, Filled, CellVerdict) {
    let (mut values, mut verdicts) = ([0u8; 16], [CellVerdict::default(); 1]);
    let mut cells = [Span::default(); 2];
    let columns = [ColumnBuffer {
        values: values.as_mut_ptr().cast(),
        verdicts: verdicts.as_mut_ptr(),
    }];
    let mut out = Filled::default();
    // SAFETY: room for the one row.
    let code = unsafe {
        fill::fill(
            state,
            data,
            true,
            std::slice::from_ref(spec),
            &columns,
            1,
            &mut cells,
            &mut [],
            &mut out,
        )
    };
    (code, out, verdicts[0])
}

// ---- Transcoding: `hypertabular_transcode`, ahead of the fill. ----

mod transcoding {
    use hypertabular::kernel::abi::{ERR_CONTRACT, OK, RawTranscoded};
    use hypertabular::kernel::exports::hypertabular_transcode;
    use std::ptr;

    const WINDOWS_1252: u32 = RawTranscoded::WINDOWS_1252;
    const LATIN_1: u32 = RawTranscoded::LATIN_1;
    const UTF_16LE: u32 = RawTranscoded::UTF_16LE;

    /// 0x80 to 0x9F as WHATWG's table says, written out by hand: the five undefined bytes are
    /// their C1 controls.
    const HIGH_1252: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];

    fn call(encoding: u32, input: &[u8], last: bool, out: &mut [u8]) -> RawTranscoded {
        let mut progress = RawTranscoded::default();
        // SAFETY: each pointer is the slice's, for its length.
        let code = unsafe {
            hypertabular_transcode(
                encoding,
                input.as_ptr(),
                input.len(),
                u32::from(last),
                out.as_mut_ptr(),
                out.len(),
                &mut progress,
            )
        };
        assert_eq!(code, OK);
        progress
    }

    /// One call with room for all of it.
    fn whole(encoding: u32, input: &[u8]) -> (Vec<u8>, RawTranscoded) {
        let mut out = vec![0; input.len() * 3 + 4];
        let progress = call(encoding, input, true, &mut out);
        out.truncate(progress.written as usize);
        (out, progress)
    }

    /// `input` cut at each of `cuts`, and each chunk handed over behind what the last left
    /// unconsumed, as a reader does.
    fn in_chunks(encoding: u32, input: &[u8], cuts: &[usize]) -> Vec<u8> {
        let mut decoded = Vec::new();
        let mut held = Vec::new();
        let mut from = 0;
        for &cut in cuts.iter().chain([&input.len()]) {
            held.extend_from_slice(&input[from..cut]);
            from = cut;
            let mut out = vec![0; held.len() * 3 + 4];
            let progress = call(encoding, &held, cut == input.len(), &mut out);
            assert_eq!(progress.failed, 0);
            decoded.extend_from_slice(&out[..progress.written as usize]);
            held.drain(..progress.consumed as usize);
        }
        assert!(held.is_empty());
        decoded
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn windows_1252_table() {
        for byte in 0..=255u8 {
            let expected = match byte {
                0x80..=0x9F => HIGH_1252[usize::from(byte - 0x80)],
                other => char::from(other),
            };
            let (out, progress) = whole(WINDOWS_1252, &[byte]);
            assert_eq!(out, expected.to_string().as_bytes(), "{byte:#04x}");
            assert_eq!((progress.consumed, progress.failed), (1, 0), "{byte:#04x}");
        }
    }

    #[test]
    fn latin1_maps_every_byte() {
        for byte in 0..=255u8 {
            let (out, progress) = whole(LATIN_1, &[byte]);
            assert_eq!(out, char::from(byte).to_string().as_bytes(), "{byte:#04x}");
            assert_eq!((progress.consumed, progress.failed), (1, 0), "{byte:#04x}");
        }
    }

    #[test]
    fn utf16_bom_transcodes_to_feff() {
        for input in [&[0xFF, 0xFE][..], &[0x61, 0, 0xFF, 0xFE, 0x62, 0][..]] {
            let (out, progress) = whole(UTF_16LE, input);
            let expected: &[u8] = if input.len() == 2 {
                b"\xEF\xBB\xBF"
            } else {
                b"a\xEF\xBB\xBFb"
            };
            assert_eq!(out, expected);
            assert_eq!(progress.failed, 0);
        }
    }

    #[test]
    fn a_nul_passes_through() {
        assert_eq!(whole(UTF_16LE, &[0, 0, 0x61, 0]).0, b"\0a");
        assert_eq!(whole(LATIN_1, b"\0a").0, b"\0a");
        assert_eq!(whole(WINDOWS_1252, b"\0a").0, b"\0a");
    }

    #[test]
    fn utf16_rejects_a_lone_surrogate() {
        let a = [0x61, 0x00];
        // Each stops after one good unit, whatever the bad unit's neighbours.
        for bad in [
            // A low with no high before it.
            &[0x00, 0xDC][..],
            &[0xFF, 0xDF, 0x62, 0x00][..],
            // A high that nothing, or the wrong thing, follows.
            &[0x00, 0xD8][..],
            &[0xFF, 0xDB, 0x62, 0x00][..],
            &[0x00, 0xD8, 0x00, 0xD8][..],
        ] {
            let input = [&a[..], bad].concat();
            let mut out = [0; 64];
            let progress = call(UTF_16LE, &input, true, &mut out);
            assert_eq!(progress.failed, 1, "{bad:02x?}");
            assert_eq!(progress.failure_at, 2, "{bad:02x?}");
            assert_eq!(progress.consumed, 2, "{bad:02x?}");
            assert_eq!(progress.written, 1, "{bad:02x?}");
            assert_eq!(out[0], b'a');
            // Asked again from there, the same unit fails at the start of that call.
            let again = call(UTF_16LE, bad, true, &mut out);
            assert_eq!((again.failed, again.failure_at, again.consumed), (1, 0, 0));
            // A low is lone whatever comes after it; a high that the next unit doesn't
            // complete is lone with or without `last`.
            if bad.len() > 2 || bad[1] >= 0xDC {
                let early = call(UTF_16LE, &input, false, &mut out);
                assert_eq!((early.failed, early.failure_at), (1, 2), "{bad:02x?}");
            }
        }
    }

    #[test]
    fn a_failure_needs_no_room() {
        let mut none = [];
        let progress = call(UTF_16LE, &[0x00, 0xDC], true, &mut none);
        assert_eq!((progress.failed, progress.failure_at), (1, 0));
    }

    #[test]
    fn a_truncated_final_unit_fails_only_at_last() {
        // Half a unit, a high surrogate alone, and a high surrogate and half its low: each is
        // nothing yet until `last` says nothing follows, and each fails where its unit began.
        for tail in [&[0x62][..], &[0x00, 0xD8][..], &[0x00, 0xD8, 0x00][..]] {
            let input = [&[0x61, 0x00][..], tail].concat();
            let mut out = [0; 16];
            let more = call(UTF_16LE, &input, false, &mut out);
            assert_eq!(more.failed, 0, "{tail:02x?}");
            assert_eq!((more.consumed, more.written), (2, 1), "{tail:02x?}");
            assert_eq!(more.failure_at, 0, "{tail:02x?}");
            let done = call(UTF_16LE, &input, true, &mut out);
            assert_eq!(done.failed, 1, "{tail:02x?}");
            assert_eq!((done.failure_at, done.consumed), (2, 2), "{tail:02x?}");
            assert_eq!(done.written, 1, "{tail:02x?}");
        }
    }

    #[test]
    fn transcode_is_chunk_invariant() {
        let text = "a\u{e9}\u{20AC}\u{1F600}b\u{FEFF}\u{10FFFF}\u{FFFD}\0z\u{10000}";
        let input = utf16(text);
        for one in 0..=input.len() {
            assert_eq!(
                in_chunks(UTF_16LE, &input, &[one]),
                text.as_bytes(),
                "{one}"
            );
            for two in one..=input.len() {
                assert_eq!(
                    in_chunks(UTF_16LE, &input, &[one, two]),
                    text.as_bytes(),
                    "{one}, {two}"
                );
            }
        }
        let bytes: Vec<u8> = (0..=255).collect();
        let (latin, _) = whole(LATIN_1, &bytes);
        let (windows, _) = whole(WINDOWS_1252, &bytes);
        for one in 0..=bytes.len() {
            assert_eq!(in_chunks(LATIN_1, &bytes, &[one]), latin, "{one}");
            assert_eq!(in_chunks(WINDOWS_1252, &bytes, &[one]), windows, "{one}");
        }
    }

    /// Every encoding through `out` of `cap` bytes: resumed from `consumed`, in the same bytes,
    /// a whole character at a time. A character that doesn't fit gets the room it asks for, as
    /// a reader would grow its buffer, and only then.
    fn resumed(encoding: u32, input: &[u8], cap: usize) -> (Vec<u8>, RawTranscoded) {
        let mut decoded = Vec::new();
        let mut at = 0;
        loop {
            let mut out = vec![0xAA; cap];
            let mut progress = call(encoding, &input[at..], true, &mut out);
            assert!(progress.written as usize <= cap);
            assert!(
                out[progress.written as usize..].iter().all(|&b| b == 0xAA),
                "wrote past what it counted"
            );
            if progress.written == 0 && progress.failed == 0 && at < input.len() {
                // No room for one character: at most four bytes are ever needed.
                assert!(cap < 4, "stuck at {at} with {cap} bytes");
                out = vec![0xAA; 4];
                progress = call(encoding, &input[at..], true, &mut out);
                assert!(progress.written > 0);
            }
            let written = &out[..progress.written as usize];
            assert!(std::str::from_utf8(written).is_ok(), "cut a character");
            decoded.extend_from_slice(written);
            at += progress.consumed as usize;
            progress.written = decoded.len() as u64;
            progress.consumed = at as u64;
            if progress.failed != 0 || at == input.len() {
                return (decoded, progress);
            }
        }
    }

    #[test]
    fn a_small_out_stops_cleanly() {
        let text = "a\u{e9}\u{20AC}\u{1F600}b\u{10FFFF}";
        let bytes: Vec<u8> = (0..=255).collect();
        let cases: [(u32, Vec<u8>); 5] = [
            (UTF_16LE, utf16(text)),
            // Stops at a bad unit, wherever the room ran out before it.
            (UTF_16LE, [utf16(text), vec![0x00, 0xDC]].concat()),
            // Stops at a final odd byte.
            (UTF_16LE, [utf16(text), vec![0x61]].concat()),
            (LATIN_1, bytes.clone()),
            (WINDOWS_1252, bytes),
        ];
        for (encoding, input) in &cases {
            let (full, whole_progress) = whole(*encoding, input);
            for cap in 1..=full.len() + 1 {
                let (decoded, progress) = resumed(*encoding, input, cap);
                assert_eq!(decoded, full, "{encoding}, cap {cap}");
                assert_eq!(progress.consumed, whole_progress.consumed, "cap {cap}");
                assert_eq!(progress.failed, whole_progress.failed, "cap {cap}");
            }
        }
    }

    #[test]
    fn a_character_is_never_split_by_out() {
        // 4 bytes need 4 of room; 3 of room writes nothing and consumes nothing.
        let input = utf16("\u{1F600}");
        for cap in 0..4 {
            let mut out = vec![0; cap];
            let progress = call(UTF_16LE, &input, true, &mut out);
            assert_eq!(
                (progress.consumed, progress.written, progress.failed),
                (0, 0, 0)
            );
        }
    }

    #[test]
    fn every_width_ends_where_utf8_says() {
        for code in [
            0x00, 0x7F, 0x80, 0x7FF, 0x800, 0xD7FF, 0xE000, 0xFFFF, 0x10000, 0x10FFFF,
        ] {
            let character = char::from_u32(code).unwrap();
            let (out, progress) = whole(UTF_16LE, &utf16(&character.to_string()));
            assert_eq!(out, character.to_string().as_bytes(), "{code:#x}");
            assert_eq!(progress.failed, 0, "{code:#x}");
        }
    }

    /// `char::decode_utf16` owes this nothing: over random units, surrogates often, the same
    /// text and the same first failure.
    #[test]
    fn transcode_agrees_with_std() {
        let mut random = super::Random(0x1BAD_B002);
        for _ in 0..20_000 {
            let length = random.below(12);
            let units: Vec<u16> = (0..length)
                .map(|_| match random.below(4) {
                    0 => 0xD800 + random.below(0x400) as u16,
                    1 => 0xDC00 + random.below(0x400) as u16,
                    2 => random.below(0x80) as u16,
                    _ => random.below(0x10000) as u16,
                })
                .collect();
            let input: Vec<u8> = units.iter().copied().flat_map(u16::to_le_bytes).collect();
            let mut text = String::new();
            let mut bad = None;
            let mut at = 0;
            for decoded in char::decode_utf16(units.iter().copied()) {
                match decoded {
                    Ok(character) => {
                        text.push(character);
                        at += character.len_utf16() * 2;
                    }
                    Err(_) => {
                        bad = Some(at);
                        break;
                    }
                }
            }
            let (out, progress) = whole(UTF_16LE, &input);
            assert_eq!(out, text.as_bytes(), "{units:04x?}");
            assert_eq!(progress.failed != 0, bad.is_some(), "{units:04x?}");
            assert_eq!(
                progress.failure_at as usize,
                bad.unwrap_or(0),
                "{units:04x?}"
            );
            assert_eq!(progress.consumed as usize, bad.unwrap_or(input.len()));
        }
    }

    #[test]
    fn the_contract() {
        let input = [0x61, 0];
        let mut out = [0u8; 8];
        let unset = RawTranscoded {
            consumed: 7,
            written: 7,
            failure_at: 7,
            failed: 7,
            reserved: 7,
        };
        let mut progress = unset;
        // SAFETY: each pointer that isn't null is the array's, for its length.
        unsafe {
            for encoding in [0, 4, u32::MAX] {
                assert_eq!(
                    hypertabular_transcode(
                        encoding,
                        input.as_ptr(),
                        2,
                        1,
                        out.as_mut_ptr(),
                        8,
                        &mut progress
                    ),
                    ERR_CONTRACT
                );
                assert_eq!(progress, unset, "a refusal writes nothing");
            }
            // A null where the contract needs memory.
            assert_eq!(
                hypertabular_transcode(1, ptr::null(), 2, 1, out.as_mut_ptr(), 8, &mut progress),
                ERR_CONTRACT
            );
            assert_eq!(
                hypertabular_transcode(1, input.as_ptr(), 2, 1, ptr::null_mut(), 8, &mut progress),
                ERR_CONTRACT
            );
            assert_eq!(
                hypertabular_transcode(
                    1,
                    input.as_ptr(),
                    2,
                    1,
                    out.as_mut_ptr(),
                    8,
                    ptr::null_mut()
                ),
                ERR_CONTRACT
            );
            assert_eq!(progress, unset);
            // A length of 0 never dereferences its pointer.
            assert_eq!(
                hypertabular_transcode(3, ptr::null(), 0, 1, ptr::null_mut(), 0, &mut progress),
                OK
            );
            assert_eq!(progress, RawTranscoded::default());
            assert_eq!(
                hypertabular_transcode(3, input.as_ptr(), 2, 1, ptr::null_mut(), 0, &mut progress),
                OK
            );
            assert_eq!(
                (progress.consumed, progress.written, progress.failed),
                (0, 0, 0)
            );
            // `progress` can be at any alignment: the export writes it as bytes, whatever the caller gave.
            let mut bytes = [0u8; 40];
            let misaligned = bytes.as_mut_ptr().add(1 + bytes.as_ptr().align_offset(8));
            assert_eq!(
                hypertabular_transcode(
                    3,
                    input.as_ptr(),
                    2,
                    1,
                    out.as_mut_ptr(),
                    8,
                    misaligned.cast()
                ),
                OK
            );
            let got = misaligned.cast::<RawTranscoded>().read_unaligned();
            assert_eq!((got.consumed, got.written), (2, 1));
        }
    }
}

// ---- Faults: `hypertabular_faults`, over what a fill wrote. ----

mod faulting {
    use super::{I32, TEXT, options, packed, started};
    use hypercast::Detail;
    use hypertabular::kernel::abi::{
        BuffersWith, CellNote, CellVerdict, ColumnBufferWith, ColumnSpecWith, ERR_CONTRACT, Filled,
        OK, RawDelimitedOptions, RawProblem, Span,
    };
    use hypertabular::kernel::delimited::fill::State;
    use hypertabular::kernel::exports::{hypertabular_delimited_fill_with, hypertabular_faults};
    use hypertabular::kernel::faults::faults;
    use std::ptr;

    const U8: u32 = 6;
    const REJECT: u32 = RawDelimitedOptions::LONG_ROWS_REJECT;
    const REPORT: u32 = RawDelimitedOptions::LONG_ROWS_REPORT;
    const TRUNCATE: u32 = RawDelimitedOptions::LONG_ROWS_TRUNCATE;

    /// A batch as `_fill_with` wrote it, with the buffers `_faults` is handed.
    struct Written {
        _values: Vec<Vec<u64>>,
        verdicts: Vec<Vec<CellVerdict>>,
        notes: Vec<Vec<CellNote>>,
        columns: Vec<ColumnBufferWith>,
        widths: Vec<u32>,
        rows: usize,
        expected: u32,
    }

    /// One whole fill of `data`. `notes` says whether the columns take notes.
    fn written(
        data: &[u8],
        policy: &RawDelimitedOptions,
        specs: &[ColumnSpecWith],
        tokens: &[u8],
        notes: bool,
    ) -> Written {
        const ROWS: usize = 16;
        let mut block = started(policy);
        let mut values = vec![vec![0u64; ROWS * 8]; specs.len()];
        let mut verdicts = vec![vec![CellVerdict::OK; ROWS]; specs.len()];
        let mut kept = vec![vec![CellNote::default(); ROWS]; specs.len()];
        let columns: Vec<ColumnBufferWith> = (0..specs.len())
            .map(|at| ColumnBufferWith {
                size: ColumnBufferWith::SIZE,
                values: values[at].as_mut_ptr().cast(),
                verdicts: verdicts[at].as_mut_ptr(),
                notes: if notes {
                    kept[at].as_mut_ptr()
                } else {
                    ptr::null_mut()
                },
            })
            .collect();
        let mut widths = vec![0u32; ROWS];
        let mut cells = vec![Span::default(); 8 * ROWS];
        let mut arena = vec![0u8; 1024];
        let mut buffers = BuffersWith::empty();
        buffers.cells = cells.as_mut_ptr();
        buffers.cells_cap = cells.len();
        buffers.arena = arena.as_mut_ptr();
        buffers.arena_cap = arena.len();
        buffers.tokens = tokens.as_ptr();
        buffers.tokens_len = tokens.len();
        buffers.row_widths = widths.as_mut_ptr();
        let mut out = Filled::default();
        // SAFETY: room for `ROWS` rows of everything, sixty-four bytes a value.
        let code = unsafe {
            hypertabular_delimited_fill_with(
                block.as_mut_ptr().cast(),
                data.as_ptr(),
                data.len(),
                1,
                specs.as_ptr(),
                columns.as_ptr(),
                specs.len(),
                ROWS,
                &buffers,
                &mut out,
            )
        };
        assert_eq!(code, OK);
        // SAFETY: the block was initialized, and the fill kept it.
        let expected = unsafe { &*block.as_ptr().cast::<State>() }.expected;
        widths.truncate(out.rows as usize);
        Written {
            _values: values,
            verdicts,
            notes: kept,
            columns,
            widths,
            rows: out.rows as usize,
            expected,
        }
    }

    /// The whole list, `cap` entries a call, checking what every call may and may not do.
    fn listed(batch: &Written, long_rows: u32, cap: usize) -> Vec<RawProblem> {
        const UNTOUCHED: RawProblem = RawProblem {
            row: 7,
            plan_column: 7,
            reason: 7,
            detail: 7,
            origin: 7,
            offset: 7,
            len: 7,
            expected: 7,
            found: 7,
        };
        let mut all = Vec::new();
        let mut cursor = 0u64;
        let end = (batch.rows * (batch.columns.len() + 1)) as u64;
        loop {
            let mut out = vec![UNTOUCHED; cap + 1];
            let mut written = usize::MAX;
            let before = cursor;
            // SAFETY: every array holds `rows` elements, and `out` holds `cap` and one more.
            let code = unsafe {
                hypertabular_faults(
                    batch.columns.as_ptr(),
                    batch.columns.len(),
                    batch.rows,
                    batch.widths.as_ptr(),
                    batch.expected,
                    long_rows,
                    &mut cursor,
                    out.as_mut_ptr(),
                    cap,
                    &mut written,
                )
            };
            assert_eq!(code, OK);
            assert!(written <= cap);
            assert!(out[written..].iter().all(|entry| *entry == UNTOUCHED));
            all.extend_from_slice(&out[..written]);
            if written < cap {
                assert_eq!(cursor, end, "a short call has reached the end");
                return all;
            }
            assert!(cursor > before, "a full call moves on");
        }
    }

    /// One of everything, and one row that's clean: tokens, a blank, padding, a long row.
    fn mixed(notes: bool) -> Written {
        let policy = RawDelimitedOptions {
            short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
            long_rows: REPORT,
            ..options()
        };
        let specs = [
            ColumnSpecWith::new(0, I32, 0),
            ColumnSpecWith::new(1, U8, 0),
            ColumnSpecWith::new(2, TEXT, 0),
        ];
        written(
            b"1,2,x\n12a,300,y\nNA,,z\n7\n8,9,z,extra\n",
            &policy,
            &specs,
            &packed(&[b"NA"]),
            notes,
        )
    }

    fn cell(
        row: u32,
        plan_column: u32,
        reason: u32,
        detail: Detail,
        span: (u32, u32),
    ) -> RawProblem {
        RawProblem {
            row,
            plan_column,
            reason,
            detail: detail as u16,
            offset: span.0,
            len: span.1,
            ..RawProblem::default()
        }
    }

    fn long(row: u32, expected: u32, found: u32) -> RawProblem {
        RawProblem {
            row,
            plan_column: RawProblem::ROW_LEVEL,
            expected,
            found,
            ..RawProblem::default()
        }
    }

    #[test]
    fn faults_list_only_what_they_should() {
        let mut batch = mixed(true);
        // What a workbook's fill would have said of a formula with no cached value, on a cell
        // that's otherwise the padding this fill wrote; the workbook tests read the real thing.
        batch.notes[1][3].origin = CellNote::ORIGIN_UNCACHED;
        assert_eq!(batch.verdicts[1][3].reason, 1);
        assert_eq!(batch.verdicts[0][2].reason, 1, "the token is empty");
        assert_eq!(batch.notes[0][2].origin, CellNote::ORIGIN_TOKEN);
        assert_eq!(batch.verdicts[1][2].reason, 1, "the blank is empty");
        assert_eq!(batch.notes[1][2].detail, Detail::Blank as u16);
        assert_eq!(batch.verdicts[2][3].reason, 1, "the padding is empty");
        assert_eq!(batch.notes[2][3].origin, CellNote::ORIGIN_PADDED);
        assert_eq!(
            listed(&batch, REPORT, 64),
            [
                cell(1, 0, 2, Detail::UnexpectedCharacter, (2, 1)),
                cell(1, 1, 3, Detail::TooLarge, (0, 3)),
                RawProblem {
                    row: 3,
                    plan_column: 1,
                    reason: 1,
                    origin: CellNote::ORIGIN_UNCACHED,
                    ..RawProblem::default()
                },
                long(4, 3, 4),
            ]
        );
        // A reason that isn't one of the listed three isn't listed, whatever its note says: that's
        // the first entry gone, and the long row isn't reported this time.
        batch.verdicts[0][1] = CellVerdict {
            offset: 0,
            len: 0,
            reason: 9,
        };
        assert_eq!(listed(&batch, REJECT, 64).len(), 2);
    }

    #[test]
    fn a_long_row_is_listed_only_when_reported() {
        let specs = [ColumnSpecWith::new(0, I32, 0)];
        let data = b"1\n2,3\n4\n";
        let on = |long_rows| RawDelimitedOptions {
            long_rows,
            ..options()
        };
        let reported = written(data, &on(REPORT), &specs, &[], true);
        let truncated = written(data, &on(TRUNCATE), &specs, &[], true);
        assert_eq!(reported.widths, [1, 2, 1]);
        assert_eq!(reported.widths, truncated.widths);
        assert_eq!(reported.expected, 1);
        assert_eq!(truncated.expected, 1);
        assert_eq!(listed(&reported, REPORT, 8), [long(1, 1, 2)]);
        assert_eq!(listed(&truncated, TRUNCATE, 8), []);
        assert_eq!(listed(&reported, REJECT, 8), []);
        // Exactly the width isn't long, and neither is a short row, padded or not.
        let padded = RawDelimitedOptions {
            short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
            ..on(REPORT)
        };
        let batch = written(b"1,2\n3\n4,5\n", &padded, &specs, &[], true);
        assert_eq!(batch.widths, [2, 1, 2]);
        assert_eq!(listed(&batch, REPORT, 8), []);
    }

    #[test]
    fn faults_resume_at_every_cap() {
        let batch = mixed(true);
        let whole = listed(&batch, REPORT, 64);
        assert_eq!(whole.len(), 3);
        for cap in 1..=8 {
            assert_eq!(listed(&batch, REPORT, cap), whole, "{cap} a call");
        }
    }

    #[test]
    fn faults_order_by_row_then_column() {
        let policy = RawDelimitedOptions {
            long_rows: REPORT,
            ..options()
        };
        let specs = [
            ColumnSpecWith::new(1, I32, 0),
            ColumnSpecWith::new(0, I32, 0),
        ];
        let batch = written(b"1,2\nx,y,z\na,b\n3,4,5\n", &policy, &specs, &[], true);
        let order: Vec<(u32, u32)> = listed(&batch, REPORT, 1)
            .iter()
            .map(|problem| (problem.row, problem.plan_column))
            .collect();
        let row_level = RawProblem::ROW_LEVEL;
        assert_eq!(
            order,
            [
                (1, row_level),
                (1, 0),
                (1, 1),
                (2, 0),
                (2, 1),
                (3, row_level)
            ]
        );
    }

    #[test]
    fn no_notes_means_detail_zero() {
        let batch = mixed(false);
        assert_eq!(
            listed(&batch, REPORT, 64),
            [
                RawProblem {
                    detail: 0,
                    ..cell(1, 0, 2, Detail::Blank, (2, 1))
                },
                RawProblem {
                    detail: 0,
                    ..cell(1, 1, 3, Detail::Blank, (0, 3))
                },
                long(4, 3, 4),
            ]
        );
    }

    /// A call that must refuse, with the batch's own good arguments otherwise.
    struct Call {
        columns: *const ColumnBufferWith,
        column_count: usize,
        rows: usize,
        row_widths: *const u32,
        long_rows: u32,
        cursor: u64,
        cursor_ptr: Option<*mut u64>,
        out: *mut RawProblem,
        cap: usize,
        written_ptr: Option<*mut usize>,
    }

    /// Makes `call`, and returns its code with what it left in `cursor`, `written` and the list.
    fn make(call: Call, list: &mut [RawProblem]) -> (i32, u64, usize) {
        let mut cursor = call.cursor;
        let mut written = 99usize;
        let before = list.to_vec();
        // SAFETY: the test's calls give the core nothing it would read past: where a pointer is
        // wrong the call is refused first, and a huge `rows` is refused before it is read.
        let code = unsafe {
            hypertabular_faults(
                call.columns,
                call.column_count,
                call.rows,
                call.row_widths,
                3,
                call.long_rows,
                call.cursor_ptr.unwrap_or(&mut cursor),
                call.out,
                call.cap,
                call.written_ptr.unwrap_or(&mut written),
            )
        };
        if code != OK {
            assert_eq!(list, &before[..], "a refusal writes nothing");
        }
        (code, cursor, written)
    }

    #[test]
    fn faults_refuse_what_they_cant_answer() {
        let batch = mixed(true);
        let mut list = [RawProblem::default(); 4];
        let end = (batch.rows * 4) as u64;
        let good = |list: &mut [RawProblem; 4]| Call {
            columns: batch.columns.as_ptr(),
            column_count: batch.columns.len(),
            rows: batch.rows,
            row_widths: batch.widths.as_ptr(),
            long_rows: REPORT,
            cursor: 0,
            cursor_ptr: None,
            out: list.as_mut_ptr(),
            cap: 4,
            written_ptr: None,
        };
        let refused = (ERR_CONTRACT, 0, 99);
        let out = |list: &mut [RawProblem; 4]| list.as_mut_ptr();

        // The control: the same call, good, is answered.
        let call = good(&mut list);
        assert_eq!(make(call, &mut list), (OK, end, 3));
        list = [RawProblem::default(); 4];

        // A cursor past the end is a caller's bug; at the end it's the end, however often asked.
        let call = Call {
            cursor: end + 1,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), (ERR_CONTRACT, end + 1, 99));
        let call = Call {
            cursor: u64::MAX,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), (ERR_CONTRACT, u64::MAX, 99));
        let call = Call {
            cursor: end,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), (OK, end, 0));

        // Nulls where the contract needs memory.
        let call = Call {
            cursor_ptr: Some(ptr::null_mut()),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let call = Call {
            written_ptr: Some(ptr::null_mut()),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let call = Call {
            out: ptr::null_mut(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let call = Call {
            columns: ptr::null(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let mut nulled = batch.columns.clone();
        nulled[2].verdicts = ptr::null_mut();
        let call = Call {
            columns: nulled.as_ptr(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);

        // A report needs the widths, and only a report does.
        let call = Call {
            row_widths: ptr::null(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        for policy in [REJECT, TRUNCATE] {
            let call = Call {
                row_widths: ptr::null(),
                long_rows: policy,
                ..good(&mut list)
            };
            assert_eq!(make(call, &mut list), (OK, end, 2));
        }
        list = [RawProblem::default(); 4];

        // A policy with no code, and a column of a size this core doesn't know.
        let call = Call {
            long_rows: 3,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let mut sized = batch.columns.clone();
        sized[1].size += 1;
        let call = Call {
            columns: sized.as_ptr(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);

        // Misaligned, each of them.
        let call = Call {
            columns: batch.columns.as_ptr().cast::<u8>().wrapping_add(1).cast(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let call = Call {
            row_widths: batch.widths.as_ptr().cast::<u8>().wrapping_add(1).cast(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        // A policy that doesn't read the widths has nothing to say about where they are.
        for policy in [REJECT, TRUNCATE] {
            let call = Call {
                row_widths: batch.widths.as_ptr().cast::<u8>().wrapping_add(1).cast(),
                long_rows: policy,
                ..good(&mut list)
            };
            assert_eq!(make(call, &mut list), (OK, end, 2));
        }
        list = [RawProblem::default(); 4];
        let call = Call {
            out: out(&mut list).cast::<u8>().wrapping_add(1).cast(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        let mut word = [0u64; 2];
        let call = Call {
            cursor_ptr: Some(word.as_mut_ptr().cast::<u8>().wrapping_add(1).cast()),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        assert_eq!(word, [0, 0]);
        let call = Call {
            written_ptr: Some(word.as_mut_ptr().cast::<u8>().wrapping_add(1).cast()),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);
        assert_eq!(word, [0, 0]);

        // A list whose first word still reads as the right size, one byte along, is refused for
        // where it is and not for what it says.
        let mut bytes = vec![0u64; batch.columns.len() * size_of::<ColumnBufferWith>() / 8 + 1];
        let along = bytes.as_mut_ptr().cast::<u8>().wrapping_add(1);
        // SAFETY: the words hold the list and a byte more.
        unsafe {
            ptr::copy_nonoverlapping(
                batch.columns.as_ptr().cast::<u8>(),
                along,
                batch.columns.len() * size_of::<ColumnBufferWith>(),
            );
        }
        let call = Call {
            columns: along.cast(),
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);

        // Counts a `u32` can't say, on a target where they exist.
        #[cfg(target_pointer_width = "64")]
        {
            let call = Call {
                rows: u32::MAX as usize + 1,
                ..good(&mut list)
            };
            assert_eq!(make(call, &mut list), refused);
        }
        // Refused before anything is formed over the widths, whatever the target's `usize`.
        let call = Call {
            rows: usize::MAX,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), refused);

        // A cap of nothing is a length of nothing: not dereferenced, and it looks at nothing.
        let call = Call {
            out: ptr::null_mut(),
            cap: 0,
            cursor: 2,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), (OK, 2, 0));
        let call = Call {
            cap: 0,
            ..good(&mut list)
        };
        assert_eq!(make(call, &mut list), (OK, 0, 0));
    }

    #[test]
    fn widths_mean_nothing_without_a_report() {
        // The Rust call, handed widths the export wouldn't pass on: only a report reads them.
        let widths = [1u32, 5];
        for (policy, count) in [(REJECT, 0), (TRUNCATE, 0), (REPORT, 1)] {
            let (mut cursor, mut written) = (0, 9);
            let mut out = [RawProblem::default(); 2];
            // SAFETY: no columns, so nothing to read but the widths.
            let code = unsafe {
                faults(
                    &[],
                    2,
                    &widths,
                    1,
                    policy,
                    &mut cursor,
                    &mut out,
                    &mut written,
                )
            };
            assert_eq!((code, cursor, written), (OK, 2, count), "{policy}");
        }
    }

    #[test]
    fn faults_with_no_rows_and_no_columns() {
        let mut cursor = 0u64;
        let mut written = 9usize;
        let mut list = [RawProblem::default(); 2];
        // No rows: the end is at the start, and nothing the rows would need is read.
        // SAFETY: nothing is read through what the counts say is empty.
        let code = unsafe {
            hypertabular_faults(
                ptr::null(),
                0,
                0,
                ptr::null(),
                0,
                REJECT,
                &mut cursor,
                list.as_mut_ptr(),
                2,
                &mut written,
            )
        };
        assert_eq!((code, cursor, written), (OK, 0, 0));
        // No columns: a row-level entry is all a row can have.
        let widths = [1u32, 2, 1];
        written = 9;
        // SAFETY: three widths, and two entries of room.
        let code = unsafe {
            hypertabular_faults(
                ptr::null(),
                0,
                3,
                widths.as_ptr(),
                1,
                REPORT,
                &mut cursor,
                list.as_mut_ptr(),
                2,
                &mut written,
            )
        };
        assert_eq!((code, cursor, written), (OK, 3, 1));
        assert_eq!(list[0], long(1, 1, 2));
    }
}

mod suggesting {
    use crate::kernel::abi::{ERR_CONTRACT, OK, RawSuggestion, Span};
    use crate::kernel::exports::hypertabular_suggest;
    use crate::kernel::suggest::{COMMA_DECIMAL, COMMA_GROUPS};
    use hypercast::{NumFormat, RawNumFormat};

    const BOOL: u32 = 1;
    const I32: u32 = 4;
    const I64: u32 = 5;
    const F64: u32 = 11;
    const UUID: u32 = 12;
    const TIMESTAMP: u32 = 13;
    const DATE: u32 = 15;
    const TEXT: u32 = 18;
    const ORDERED: u32 = 20;

    const RESOLVED: u32 = RawSuggestion::RESOLVED;
    const AMBIGUOUS: u32 = RawSuggestion::AMBIGUOUS;
    const UNRESOLVED: u32 = RawSuggestion::UNRESOLVED;

    /// `2` and `3` in a set: month-day-year and day-month-year.
    const EITHER_ORDER: u32 = 12;
    const GROUPS: u32 = RawSuggestion::NOTATION_COMMA_GROUPS;
    const EITHER_NOTATION: u32 =
        RawSuggestion::NOTATION_COMMA_GROUPS | RawSuggestion::NOTATION_COMMA_DECIMAL;

    fn ask(columns: &[&[&[u8]]], formats: Option<&[RawNumFormat]>) -> Vec<RawSuggestion> {
        let rows = columns.first().map_or(0, |column| column.len());
        let mut input = Vec::new();
        let mut cells = Vec::new();
        for column in columns {
            assert_eq!(column.len(), rows, "a sample's columns are one length");
            for cell in *column {
                cells.push(Span {
                    offset: input.len() as u32,
                    len: cell.len() as u32,
                });
                input.extend_from_slice(cell);
            }
        }
        let mut out = vec![RawSuggestion::default(); columns.len()];
        let code = unsafe {
            hypertabular_suggest(
                input.as_ptr(),
                input.len(),
                core::ptr::null(),
                0,
                cells.as_ptr(),
                core::ptr::null(),
                rows,
                columns.len(),
                formats.map_or(core::ptr::null(), <[RawNumFormat]>::as_ptr),
                out.as_mut_ptr(),
            )
        };
        assert_eq!(code, OK);
        out
    }

    #[track_caller]
    fn says(cells: &[&str], door: u32, param: u32, status: u32, matched: u32, sampled: u32) {
        let cells: Vec<&[u8]> = cells.iter().map(|cell| cell.as_bytes()).collect();
        assert_eq!(
            ask(&[&cells], None),
            [RawSuggestion {
                door,
                param,
                status,
                matched,
                sampled
            }],
            "{cells:?}"
        );
    }

    #[test]
    fn each_door_on_its_own_values() {
        says(&["true", "false"], BOOL, 0, RESOLVED, 2, 2);
        says(&["1", "2"], I32, 0, RESOLVED, 2, 2);
        says(&["5000000000"], I64, 0, RESOLVED, 1, 1);
        says(&["2.5"], F64, 0, RESOLVED, 1, 1);
        says(&["2026-01-07"], DATE, 0, RESOLVED, 1, 1);
        says(&["2026-01-07T00:00:00Z"], TIMESTAMP, 0, RESOLVED, 1, 1);
        says(
            &["6ba7b810-9dad-11d1-80b4-00c04fd430c8"],
            UUID,
            0,
            RESOLVED,
            1,
            1,
        );
        says(&["abc"], TEXT, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn one_cell_no_door_takes_leaves_a_column_unresolved() {
        says(&["1", "abc"], TEXT, 0, UNRESOLVED, 1, 2);
        // The most that any one door took: two for the integers, one for the boolean door's `1`.
        says(&["1", "2", "abc"], TEXT, 0, UNRESOLVED, 2, 3);
        says(
            &["2026-01-07", "2026-01-08", "2026-01-09", "abc"],
            TEXT,
            0,
            UNRESOLVED,
            3,
            4,
        );
        says(&["true", "2"], TEXT, 0, UNRESOLVED, 1, 2);
        // Whichever door took the most says how many, whichever that is.
        says(&["2,5", "3,5", "abc"], TEXT, 0, UNRESOLVED, 2, 3);
        says(
            &["2026-01-07T00:00:00Z", "2026-01-08T00:00:00Z", "abc"],
            TEXT,
            0,
            UNRESOLVED,
            2,
            3,
        );
        let key = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        says(&[key, key, "abc"], TEXT, 0, UNRESOLVED, 2, 3);
        says(
            &["02/13/2026", "03/13/2026", "abc"],
            TEXT,
            0,
            UNRESOLVED,
            2,
            3,
        );
        says(
            &["13/02/2026", "13/03/2026", "abc"],
            TEXT,
            0,
            UNRESOLVED,
            2,
            3,
        );
        // Doors that each take some, none all, count no more than the best of them.
        says(&["2026-01-07", "2.5", "x"], TEXT, 0, UNRESOLVED, 1, 3);
    }

    #[test]
    fn a_cell_that_fits_several_declarations_is_ambiguous() {
        says(&["1,234"], I32, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        says(&["01/02/2026"], ORDERED, EITHER_ORDER, AMBIGUOUS, 1, 1);
        // Every sampled cell agrees, under either order, and a later one needn't.
        says(
            &["01/01/2026", "02/02/2026"],
            ORDERED,
            EITHER_ORDER,
            AMBIGUOUS,
            2,
            2,
        );
    }

    #[test]
    fn a_cell_that_fits_one_declaration_settles_its_column() {
        // Thirteen is no month, and thirteen is no day.
        says(&["13/02/2026", "01/02/2026"], ORDERED, 3, RESOLVED, 2, 2);
        says(&["02/13/2026", "01/02/2026"], ORDERED, 2, RESOLVED, 2, 2);
        // A year first reads the same under every order, and is year-month-day.
        says(&["2026/01/02"], ORDERED, 1, RESOLVED, 1, 1);
        // An ISO date is a date before it's a separated one.
        says(&["2026-01-02", "2026-02-03"], DATE, 0, RESOLVED, 2, 2);
        says(&["2026-01-02", "2026/02/03"], ORDERED, 1, RESOLVED, 2, 2);
        // A year-first cell doesn't rule out either order, and the other one needs one of them.
        says(
            &["2026/01/02", "03/04/2026"],
            ORDERED,
            EITHER_ORDER,
            AMBIGUOUS,
            2,
            2,
        );
        // Neither order takes both, so no door takes either, and each order took one.
        says(&["13/02/2026", "02/13/2026"], TEXT, 0, UNRESOLVED, 1, 2);
        // Two grouped commas can't be one decimal separator.
        says(&["1,234,567"], I32, GROUPS, RESOLVED, 1, 1);
        // `.` as a decimal rules out `.` as a group.
        says(&["1,234", "2.5"], F64, GROUPS, RESOLVED, 2, 2);
        says(&["1,234.5"], F64, GROUPS, RESOLVED, 1, 1);
        // A group can't start with a lone zero, so `.` here is no group, and `,` no decimal in `1,234`.
        says(&["1,234", "0.125"], F64, GROUPS, RESOLVED, 2, 2);
        // No comma, and the invariant notation needs no saying.
        says(&["0.125"], F64, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn what_only_the_comma_decimal_reads_is_resolved_in_it() {
        let decimal = RawSuggestion::NOTATION_COMMA_DECIMAL;
        says(&["2,5"], F64, decimal, RESOLVED, 1, 1);
        says(&["2,50"], F64, decimal, RESOLVED, 1, 1);
        says(&["2,5000"], F64, decimal, RESOLVED, 1, 1);
        says(&["12,34"], F64, decimal, RESOLVED, 1, 1);
        says(&["1.234,5"], F64, decimal, RESOLVED, 1, 1);
        says(&["1.234", "2,5"], F64, decimal, RESOLVED, 2, 2);
        says(&["1,234", "2,5"], F64, decimal, RESOLVED, 2, 2);
        // Repeated, a `.` can't be a decimal.
        says(&["1.234.567"], I32, decimal, RESOLVED, 1, 1);
        says(&["1.234.567", "2"], I32, decimal, RESOLVED, 2, 2);
        // No thousands group starts with a zero, or has more than three digits.
        says(&["0,125"], F64, decimal, RESOLVED, 1, 1);
        says(&["-0,125"], F64, decimal, RESOLVED, 1, 1);
        says(&["0,5"], F64, decimal, RESOLVED, 1, 1);
        says(&["1,234", "0,125"], F64, decimal, RESOLVED, 2, 2);
        says(&["1234,567"], F64, decimal, RESOLVED, 1, 1);
    }

    #[test]
    fn three_digits_after_a_lone_separator_are_a_question() {
        says(&["1,234"], I32, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        says(&["1.234"], F64, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        says(&["12.345"], F64, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        says(&["123.456"], F64, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        says(&["-1.234"], F64, EITHER_NOTATION, AMBIGUOUS, 1, 1);
        // A separator anywhere in the column raises it, the last cell's or not.
        says(&["1,234", "5"], I32, EITHER_NOTATION, AMBIGUOUS, 2, 2);
        says(&["5", "1,234"], I32, EITHER_NOTATION, AMBIGUOUS, 2, 2);
        says(&["1.234", "5"], F64, EITHER_NOTATION, AMBIGUOUS, 2, 2);
        says(&["5", "1.234"], F64, EITHER_NOTATION, AMBIGUOUS, 2, 2);
        // Each holds under its own reading of the column: `1.234` as a decimal or as 1234.
        says(&["1.234", "1,234"], F64, EITHER_NOTATION, AMBIGUOUS, 2, 2);
    }

    #[test]
    fn a_point_that_can_only_be_a_decimal_settles_its_column() {
        says(&["2.5"], F64, 0, RESOLVED, 1, 1);
        says(&["2.50"], F64, 0, RESOLVED, 1, 1);
        says(&["2.5000"], F64, 0, RESOLVED, 1, 1);
        says(&["0.125"], F64, 0, RESOLVED, 1, 1);
        says(&["1234.567"], F64, 0, RESOLVED, 1, 1);
        says(&["1.234", "2.5"], F64, 0, RESOLVED, 2, 2);
        says(&["1.234", "0.125"], F64, 0, RESOLVED, 2, 2);
    }

    #[test]
    fn notations_that_contradict_each_other_are_no_door() {
        says(&["2,5", "2.5"], TEXT, 0, UNRESOLVED, 1, 2);
        says(&["1,234,567", "1.234.567"], TEXT, 0, UNRESOLVED, 1, 2);
        says(&["1,234.5", "1.234,5"], TEXT, 0, UNRESOLVED, 1, 2);
    }

    #[test]
    fn with_no_separator_the_notations_read_the_same() {
        says(&["1234"], I32, 0, RESOLVED, 1, 1);
        says(&["5", "7"], I32, 0, RESOLVED, 2, 2);
        says(&["1e3"], I32, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn a_declared_format_settles_it() {
        let western = [RawNumFormat::from(NumFormat::INVARIANT)];
        assert_eq!(
            ask(&[&[b"1,234"]], Some(&western)),
            [RawSuggestion {
                door: I32,
                param: 0,
                status: RESOLVED,
                matched: 1,
                sampled: 1
            }]
        );
        let eurozone = [RawNumFormat::from(COMMA_DECIMAL)];
        assert_eq!(
            ask(&[&[b"1,234"]], Some(&eurozone)),
            [RawSuggestion {
                door: F64,
                param: 0,
                status: RESOLVED,
                matched: 1,
                sampled: 1
            }]
        );
        assert_eq!(
            ask(&[&[b"1.234"]], Some(&eurozone)),
            [RawSuggestion {
                door: I32,
                param: 0,
                status: RESOLVED,
                matched: 1,
                sampled: 1
            }]
        );
        // Declared, a door's own rule is all there is: Indian grouping takes this.
        let indian = [RawNumFormat::from(
            NumFormat::INVARIANT.with_grouping(hypercast::Grouping::Indian),
        )];
        assert_eq!(
            ask(&[&[b"1,23,456"]], Some(&indian)),
            [RawSuggestion {
                door: I32,
                param: 0,
                status: RESOLVED,
                matched: 1,
                sampled: 1
            }]
        );
        // And what it refuses is a cell no door takes.
        for (cell, format) in [(&b"1,23,456"[..], &western), (&b"1,234,567"[..], &indian)] {
            assert_eq!(
                ask(&[&[cell]], Some(format)),
                [RawSuggestion {
                    door: TEXT,
                    param: 0,
                    status: RESOLVED,
                    matched: 1,
                    sampled: 1
                }]
            );
        }
    }

    #[test]
    fn the_header_s_flags_are_the_two_candidates_flags() {
        assert_eq!(RawSuggestion::NOTATION_FLAGS, 95);
        assert_eq!(COMMA_GROUPS.flags, RawSuggestion::NOTATION_FLAGS);
        assert_eq!(COMMA_DECIMAL.flags, RawSuggestion::NOTATION_FLAGS);
        assert_eq!(
            (
                COMMA_GROUPS.decimal_sep.as_str(),
                COMMA_GROUPS.group_sep.as_str()
            ),
            (".", ",")
        );
        assert_eq!(
            (
                COMMA_DECIMAL.decimal_sep.as_str(),
                COMMA_DECIMAL.group_sep.as_str()
            ),
            (",", ".")
        );
        assert_eq!(COMMA_GROUPS, NumFormat::INVARIANT);
    }

    #[test]
    fn a_format_is_a_column_s_own_and_size_zero_declares_nothing() {
        let formats = [
            RawNumFormat::from(COMMA_GROUPS),
            RawNumFormat {
                size: 0,
                ..RawNumFormat::ZERO
            },
        ];
        let out = ask(&[&[b"1,234"], &[b"1,234"]], Some(&formats));
        assert_eq!((out[0].door, out[0].status), (I32, RESOLVED));
        assert_eq!(
            (out[1].door, out[1].param, out[1].status),
            (I32, EITHER_NOTATION, AMBIGUOUS)
        );
    }

    #[test]
    fn blanks_do_not_count() {
        says(&["", "   ", "\t"], TEXT, 0, UNRESOLVED, 0, 0);
        says(&["", "1", "  ", "2"], I32, 0, RESOLVED, 2, 2);
        says(&["", "abc"], TEXT, 0, RESOLVED, 1, 1);
        says(&["1", "", "abc"], TEXT, 0, UNRESOLVED, 1, 2);
        // No rows at all is the same: nothing to go on.
        assert_eq!(
            ask(&[&[], &[]], None),
            [RawSuggestion {
                door: TEXT,
                param: 0,
                status: UNRESOLVED,
                matched: 0,
                sampled: 0
            }; 2]
        );
    }

    /// The order of preference is what decides between doors that each take every cell.
    #[test]
    fn the_first_door_that_takes_every_cell_wins() {
        // Bool over i32: HyperCast's door takes `0` and `1`.
        says(&["0", "1", "1"], BOOL, 0, RESOLVED, 3, 3);
        // Its lexicon, in any case.
        says(
            &["TRUE", "true", "False", "yes", "N", "On", "out"],
            BOOL,
            0,
            RESOLVED,
            7,
            7,
        );
        says(&["0", "1", "true"], BOOL, 0, RESOLVED, 3, 3);
        // One cell off the lexicon, and the integers have it.
        says(&["0", "1", "2"], I32, 0, RESOLVED, 3, 3);
        // i32 over i64, i64 over f64, f64 over a text column.
        says(&["2147483647"], I32, 0, RESOLVED, 1, 1);
        says(&["2147483648"], I64, 0, RESOLVED, 1, 1);
        says(&["-2147483649", "1"], I64, 0, RESOLVED, 2, 2);
        says(&["1", "2.5"], F64, 0, RESOLVED, 2, 2);
        says(&["5000000000", "2.5"], F64, 0, RESOLVED, 2, 2);
        says(&["9223372036854775807"], I64, 0, RESOLVED, 1, 1);
    }

    /// What the doors take, followed, so a plan reads what its suggestion saw.
    #[test]
    fn the_integer_doors_own_leniences_are_followed() {
        says(&["-0"], I32, 0, RESOLVED, 1, 1);
        says(&["+1"], I32, 0, RESOLVED, 1, 1);
        says(&["(5)"], I32, 0, RESOLVED, 1, 1);
        says(&["1e3"], I32, 0, RESOLVED, 1, 1);
        says(&["0x10"], I32, 0, RESOLVED, 1, 1);
        says(&["50%"], F64, 0, RESOLVED, 1, 1);
        says(&[" 1 "], BOOL, 0, RESOLVED, 1, 1);
        says(&[" 7 "], I32, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn a_leading_zero_is_an_identifier() {
        // Nothing takes it, so it's text, and a column of them is a text column.
        says(&["007"], TEXT, 0, RESOLVED, 1, 1);
        says(&["00"], TEXT, 0, RESOLVED, 1, 1);
        says(&["-007"], TEXT, 0, RESOLVED, 1, 1);
        says(&[" 007 "], TEXT, 0, RESOLVED, 1, 1);
        says(&["(007)"], TEXT, 0, RESOLVED, 1, 1);
        says(&["01.5"], TEXT, 0, RESOLVED, 1, 1);
        // Beside a number, a door took some and not all.
        says(&["07030", "10001"], TEXT, 0, UNRESOLVED, 1, 2);
        // Under a declared format just the same: a format says what `,` means, not what `0` does.
        let western = [RawNumFormat::from(NumFormat::INVARIANT)];
        assert_eq!(
            ask(&[&[b"007"]], Some(&western)),
            [RawSuggestion {
                door: TEXT,
                param: 0,
                status: RESOLVED,
                matched: 1,
                sampled: 1
            }]
        );
        // These lose nothing.
        says(&["0", "2"], I32, 0, RESOLVED, 2, 2);
        says(&["0.5"], F64, 0, RESOLVED, 1, 1);
        says(&["-0.5"], F64, 0, RESOLVED, 1, 1);
        says(&[".007"], F64, 0, RESOLVED, 1, 1);
        says(&["0e5"], I32, 0, RESOLVED, 1, 1);
        says(&["100"], I32, 0, RESOLVED, 1, 1);
        // Read as a decimal, this `,` is the sign of a fraction, not of a padded integer.
        says(
            &[",007"],
            F64,
            RawSuggestion::NOTATION_COMMA_DECIMAL,
            RESOLVED,
            1,
            1,
        );
    }

    #[test]
    fn digits_past_i64_would_lose_digits_in_f64() {
        says(&["9223372036854775808"], TEXT, 0, RESOLVED, 1, 1);
        says(&["99999999999999999999"], TEXT, 0, RESOLVED, 1, 1);
        says(&["-99999999999999999999"], TEXT, 0, RESOLVED, 1, 1);
        says(&["99,999,999,999,999,999,999"], TEXT, 0, RESOLVED, 1, 1);
        says(&["1", "99999999999999999999"], TEXT, 0, UNRESOLVED, 1, 2);
        says(&["2.5", "99999999999999999999"], TEXT, 0, UNRESOLVED, 1, 2);
        // An exponent says it's a float, fraction or not.
        says(&["1e30"], F64, 0, RESOLVED, 1, 1);
        says(&["1.5e300"], F64, 0, RESOLVED, 1, 1);
        // Digits in a fraction are a float's own business.
        says(
            &["3.14159265358979323846264338327950288"],
            F64,
            0,
            RESOLVED,
            1,
            1,
        );
        // 32 digits of no number are a UUID's N form.
        says(
            &["12345678901234567890123456789012"],
            UUID,
            0,
            RESOLVED,
            1,
            1,
        );
    }

    #[test]
    fn what_the_float_door_refuses_is_refused() {
        says(&["NaN"], TEXT, 0, RESOLVED, 1, 1);
        says(&["Infinity"], TEXT, 0, RESOLVED, 1, 1);
        says(&["-inf"], TEXT, 0, RESOLVED, 1, 1);
        says(&["2.5", "NaN"], TEXT, 0, UNRESOLVED, 1, 2);
        says(&["1e400"], TEXT, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn a_shape_no_door_here_tries_is_text() {
        // No zone, so no instant, and date-times aren't on the list.
        says(&["2026-01-07T10:30:00"], TEXT, 0, RESOLVED, 1, 1);
        says(&["2026-01-07 10:30:00"], TEXT, 0, RESOLVED, 1, 1);
        says(&["10:30:00"], TEXT, 0, RESOLVED, 1, 1);
        says(&["01/02/26"], TEXT, 0, RESOLVED, 1, 1);
        says(&["PT1H"], TEXT, 0, RESOLVED, 1, 1);
    }

    #[test]
    fn a_cell_that_is_not_text_leaves_a_column_unresolved() {
        assert_eq!(
            ask(&[&[b"\xe9t\xe9"]], None),
            [RawSuggestion {
                door: TEXT,
                param: 0,
                status: UNRESOLVED,
                matched: 0,
                sampled: 1
            }]
        );
        assert_eq!(
            ask(&[&[b"abc", b"\xff"]], None),
            [RawSuggestion {
                door: TEXT,
                param: 0,
                status: UNRESOLVED,
                matched: 0,
                sampled: 2
            }]
        );
        assert_eq!(
            ask(&[&[b"1", b"\xff"]], None),
            [RawSuggestion {
                door: TEXT,
                param: 0,
                status: UNRESOLVED,
                matched: 1,
                sampled: 2
            }]
        );
    }

    #[test]
    fn the_sample_runs_by_column_and_a_flagged_span_is_in_the_arena() {
        let input = b"1.5xyz";
        let arena = b"true";
        let cells = [
            Span { offset: 0, len: 3 },
            Span { offset: 0, len: 1 },
            Span {
                offset: 0,
                len: 4 | Span::FLAG,
            },
            Span { offset: 3, len: 3 },
        ];
        let mut out = [RawSuggestion::default(); 2];
        let code = unsafe {
            hypertabular_suggest(
                input.as_ptr(),
                input.len(),
                arena.as_ptr(),
                arena.len(),
                cells.as_ptr(),
                core::ptr::null(),
                2,
                2,
                core::ptr::null(),
                out.as_mut_ptr(),
            )
        };
        assert_eq!(code, OK);
        // By column: `1.5` and `1`; `true` and `xyz`. By row it would be `1.5`, `true`; `1`, `xyz`.
        assert_eq!(
            (out[0].door, out[0].status, out[0].matched),
            (F64, RESOLVED, 2)
        );
        assert_eq!(
            (out[1].door, out[1].status, out[1].matched),
            (TEXT, UNRESOLVED, 1)
        );
    }

    fn sentinel() -> RawSuggestion {
        RawSuggestion {
            door: 99,
            param: 99,
            status: 99,
            matched: 99,
            sampled: 99,
        }
    }

    /// A call with everything in order, for each test to break one thing of.
    struct Call {
        input: *const u8,
        input_len: usize,
        arena: *const u8,
        arena_len: usize,
        cells: *const Span,
        rows: usize,
        columns: usize,
        formats: *const RawNumFormat,
        out: *mut RawSuggestion,
    }

    impl Call {
        fn run(self) -> i32 {
            unsafe {
                hypertabular_suggest(
                    self.input,
                    self.input_len,
                    self.arena,
                    self.arena_len,
                    self.cells,
                    core::ptr::null(),
                    self.rows,
                    self.columns,
                    self.formats,
                    self.out,
                )
            }
        }
    }

    #[test]
    fn a_caller_s_mistake_is_refused_and_nothing_is_written() {
        let input = b"12";
        let arena = b"3";
        let cells = [Span { offset: 0, len: 1 }, Span { offset: 1, len: 1 }];
        let mut out = [sentinel(); 2];
        let held = out.as_mut_ptr();
        let good = || Call {
            input: input.as_ptr(),
            input_len: 2,
            arena: arena.as_ptr(),
            arena_len: 1,
            cells: cells.as_ptr(),
            rows: 2,
            columns: 1,
            formats: core::ptr::null(),
            out: held,
        };
        let refused = |name: &str, call: Call| {
            assert_eq!(call.run(), ERR_CONTRACT, "{name}");
            assert_eq!(unsafe { *held }, sentinel(), "{name}");
        };
        let null_bytes = core::ptr::null::<u8>();
        // Null with a length, even with every span blank and no byte of it to read.
        let blanks = [Span::default(), Span::default()];
        refused(
            "null input",
            Call {
                input: null_bytes,
                cells: blanks.as_ptr(),
                ..good()
            },
        );
        refused(
            "null arena",
            Call {
                arena: null_bytes,
                ..good()
            },
        );
        refused(
            "null cells",
            Call {
                cells: core::ptr::null(),
                ..good()
            },
        );
        refused(
            "null out",
            Call {
                out: core::ptr::null_mut(),
                ..good()
            },
        );
        // Off by a byte, against the alignment each type asks for.
        let cells_off = cells.as_ptr().cast::<u8>().wrapping_add(1).cast::<Span>();
        refused(
            "misaligned cells",
            Call {
                cells: cells_off,
                ..good()
            },
        );
        let out_off = held.cast::<u8>().wrapping_add(1).cast::<RawSuggestion>();
        refused(
            "misaligned out",
            Call {
                out: out_off,
                ..good()
            },
        );
        let formats = [RawNumFormat::from(NumFormat::INVARIANT)];
        let formats_off = formats
            .as_ptr()
            .cast::<u8>()
            .wrapping_add(1)
            .cast::<RawNumFormat>();
        refused(
            "misaligned formats",
            Call {
                formats: formats_off,
                ..good()
            },
        );
        // A product past the address space, and one that wraps around to a small one.
        refused(
            "wrapping",
            Call {
                rows: usize::MAX / 2 + 1,
                columns: 2,
                ..good()
            },
        );
        refused(
            "past a slice",
            Call {
                rows: 1,
                columns: isize::MAX as usize,
                ..good()
            },
        );
        #[cfg(target_pointer_width = "64")]
        refused(
            "too many spans",
            Call {
                rows: u32::MAX as usize,
                columns: 1 << 32,
                ..good()
            },
        );
        #[cfg(target_pointer_width = "64")]
        refused(
            "wrapping with countable rows",
            Call {
                rows: 1 << 31,
                columns: 1 << 33,
                ..good()
            },
        );
        refused(
            "too many formats",
            Call {
                rows: 0,
                columns: isize::MAX as usize / 50,
                ..good()
            },
        );
        refused(
            "null out, and no rows",
            Call {
                rows: 0,
                out: core::ptr::null_mut(),
                ..good()
            },
        );
        // Spans out of their bounds.
        let past = [Span { offset: 1, len: 2 }, Span { offset: 0, len: 0 }];
        refused(
            "past the input",
            Call {
                cells: past.as_ptr(),
                ..good()
            },
        );
        let past = [
            Span {
                offset: u32::MAX,
                len: 0x7FFF_FFFF,
            },
            Span { offset: 0, len: 0 },
        ];
        refused(
            "far past the input",
            Call {
                cells: past.as_ptr(),
                ..good()
            },
        );
        let past = [
            Span {
                offset: 1,
                len: 1 | Span::FLAG,
            },
            Span { offset: 0, len: 0 },
        ];
        refused(
            "past the arena",
            Call {
                cells: past.as_ptr(),
                ..good()
            },
        );
        // A bad cell in the last column, and the first column isn't written either.
        let mut two = [sentinel(); 2];
        let late = [Span { offset: 0, len: 1 }, Span { offset: 9, len: 1 }];
        assert_eq!(
            Call {
                cells: late.as_ptr(),
                rows: 1,
                columns: 2,
                out: two.as_mut_ptr(),
                ..good()
            }
            .run(),
            ERR_CONTRACT
        );
        assert_eq!(two, [sentinel(); 2]);
        // A format the doors would refuse, whichever column it's in.
        let mut same = RawNumFormat::from(NumFormat::INVARIANT);
        same.format = NumFormat::new('.', '.', 0);
        let bad = [same];
        refused(
            "a bad format",
            Call {
                formats: bad.as_ptr(),
                ..good()
            },
        );
        let mut short = RawNumFormat::from(NumFormat::INVARIANT);
        short.size = 1;
        let bad = [short];
        refused(
            "a short format",
            Call {
                formats: bad.as_ptr(),
                ..good()
            },
        );
        let mut flags = RawNumFormat::from(NumFormat::INVARIANT);
        flags.format.flags |= 1 << 20;
        let bad = [flags];
        refused(
            "an undefined flag",
            Call {
                formats: bad.as_ptr(),
                ..good()
            },
        );
        for word in [351, 863] {
            let mut stale = RawNumFormat::from(NumFormat::INVARIANT);
            stale.format.flags = word;
            let bad = [stale];
            refused(
                "a retired grouping bit",
                Call {
                    formats: bad.as_ptr(),
                    ..good()
                },
            );
            stale.size = 0;
            let bad = [stale];
            refused(
                "a retired grouping bit with no size",
                Call {
                    formats: bad.as_ptr(),
                    ..good()
                },
            );
        }
        // Past what a `u32` can count, whatever else is true. The cells needn't exist to see it:
        // the count is refused first.
        #[cfg(target_pointer_width = "64")]
        refused(
            "rows past a count",
            Call {
                rows: 1 << 32,
                columns: 1,
                ..good()
            },
        );
    }

    /// `ask`, with a mark for every cell: not `0` for a typed one.
    fn ask_typed(column: &[(&[u8], bool)], format: Option<&[RawNumFormat]>) -> RawSuggestion {
        use crate::kernel::suggest::suggest;
        let mut input = Vec::new();
        let mut cells = Vec::new();
        let mut marks = Vec::new();
        for (cell, typed) in column {
            cells.push(Span {
                offset: input.len() as u32,
                len: cell.len() as u32,
            });
            input.extend_from_slice(cell);
            marks.push(u8::from(*typed));
        }
        let mut out = [sentinel()];
        let code = suggest(
            &input,
            b"",
            &cells,
            Some(&marks),
            column.len(),
            format,
            &mut out,
        );
        assert_eq!(code, OK);
        out[0]
    }

    #[test]
    fn a_typed_number_has_no_notation() {
        let comma = [RawNumFormat::from(COMMA_DECIMAL)];
        let western = [RawNumFormat::from(NumFormat::INVARIANT)];
        let typed: &[(&[u8], bool)] = &[(b"1.234", true), (b"12.345", true)];
        // Nothing declared, a comma decimal and Western: the one value, and no question.
        for format in [None, Some(&comma[..]), Some(&western[..])] {
            let found = ask_typed(typed, format);
            assert_eq!((found.door, found.param, found.status), (F64, 0, RESOLVED));
            assert_eq!((found.matched, found.sampled), (2, 2));
        }
        // The same bytes as text are what they always were.
        let text: &[(&[u8], bool)] = &[(b"1.234", false), (b"12.345", false)];
        let found = ask_typed(text, None);
        assert_eq!(
            (found.door, found.param, found.status),
            (F64, EITHER_NOTATION, AMBIGUOUS)
        );
        assert_eq!(ask_typed(text, Some(&comma)).door, I32);
        // Each candidate counts the typed cell, and text beside it still asks or settles.
        let mixed: &[(&[u8], bool)] = &[(b"1.5", true), (b"1.234", false)];
        let found = ask_typed(mixed, None);
        assert_eq!(
            (found.door, found.param, found.status),
            (F64, EITHER_NOTATION, AMBIGUOUS)
        );
        let found = ask_typed(mixed, Some(&comma));
        assert_eq!((found.door, found.status), (F64, RESOLVED));
        let settled: &[(&[u8], bool)] = &[(b"1.25", true), (b"2,5", false)];
        let found = ask_typed(settled, None);
        assert_eq!(
            (found.door, found.param, found.status),
            (F64, RawSuggestion::NOTATION_COMMA_DECIMAL, RESOLVED)
        );
        // Its `,` isn't noted either, and no notation refuses what it's said to hold.
        let found = ask_typed(&[(b"1,5", true)], None);
        assert_eq!((found.door, found.status), (TEXT, RESOLVED));
        // Whole, and the doors' own limits, by value: `f64`'s twin, and not what the digits parse to.
        let whole = ask_typed(&[(b"2", true), (b"3000000", true)], Some(&comma));
        assert_eq!((whole.door, whole.status), (I32, RESOLVED));
        let wide = ask_typed(&[(b"3000000000", true)], None);
        assert_eq!((wide.door, wide.status), (I64, RESOLVED));
        for past in [
            &b"9223372036854775808"[..],
            b"99999999999999999999",
            b"1e30",
        ] {
            let found = ask_typed(&[(past, true)], None);
            assert_eq!((found.door, found.status), (F64, RESOLVED), "{past:?}");
        }
        // No rule of the text's about zeros: a shortest form has none to lose, and a typed non-number
        // takes no numeric door, in any notation.
        for (cell, door) in [
            (&b"true"[..], 1),
            (b"2023-03-15T00:00:00", TEXT),
            (b"#N/A", TEXT),
            (b"inf", TEXT),
            (b"NaN", TEXT),
        ] {
            let found = ask_typed(&[(cell, true)], None);
            assert_eq!((found.door, found.status), (door, RESOLVED), "{cell:?}");
            let found = ask_typed(&[(cell, true)], Some(&comma));
            assert_eq!((found.door, found.status), (door, RESOLVED), "{cell:?}");
        }
        // A mark that isn't `1` is a mark, and none is none.
        let mut out = [sentinel()];
        let cells = [Span { offset: 0, len: 5 }];
        let input = b"1.234";
        for (mark, status) in [(7u8, RESOLVED), (0, AMBIGUOUS)] {
            let code = crate::kernel::suggest::suggest(
                input,
                b"",
                &cells,
                Some(&[mark]),
                1,
                None,
                &mut out,
            );
            assert_eq!((code, out[0].status), (OK, status));
        }
        // Through the export, each column its own marks.
        let cells = [Span { offset: 0, len: 5 }, Span { offset: 0, len: 5 }];
        let marks = [1u8, 0];
        let mut out = [sentinel(); 2];
        let code = unsafe {
            hypertabular_suggest(
                input.as_ptr(),
                input.len(),
                core::ptr::null(),
                0,
                cells.as_ptr(),
                marks.as_ptr(),
                1,
                2,
                core::ptr::null(),
                out.as_mut_ptr(),
            )
        };
        assert_eq!(code, OK);
        assert_eq!((out[0].status, out[1].status), (RESOLVED, AMBIGUOUS));
    }

    #[test]
    fn the_kernel_function_holds_its_own_lengths() {
        use crate::kernel::suggest::suggest;
        let cells = [Span::default(); 2];
        let mut out = [sentinel(); 2];
        let format = [RawNumFormat::from(NumFormat::INVARIANT)];
        // Two cells aren't two rows of two columns.
        assert_eq!(
            suggest(b"", b"", &cells, None, 2, None, &mut out),
            ERR_CONTRACT
        );
        assert_eq!(out, [sentinel(); 2]);
        // One format isn't one for each of two columns.
        assert_eq!(
            suggest(b"", b"", &cells, None, 1, Some(&format), &mut out),
            ERR_CONTRACT
        );
        assert_eq!(out, [sentinel(); 2]);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            suggest(b"", b"", &[], None, 1 << 32, None, &mut []),
            ERR_CONTRACT
        );
        // One mark isn't one for each of two cells.
        assert_eq!(
            suggest(b"", b"", &cells, Some(&[0]), 1, None, &mut out),
            ERR_CONTRACT
        );
        assert_eq!(out, [sentinel(); 2]);
        assert_eq!(suggest(b"", b"", &cells, None, 1, None, &mut out), OK);
        assert_eq!(out[0].status, UNRESOLVED);
    }

    #[test]
    fn nothing_to_hold_needs_no_memory() {
        let code = Call {
            input: core::ptr::null(),
            input_len: 0,
            arena: core::ptr::null(),
            arena_len: 0,
            cells: core::ptr::null(),
            rows: 0,
            columns: 0,
            formats: core::ptr::null(),
            out: core::ptr::null_mut(),
        }
        .run();
        assert_eq!(code, OK);
        // Rows and no columns is nothing to hold too.
        let code = Call {
            input: core::ptr::null(),
            input_len: 0,
            arena: core::ptr::null(),
            arena_len: 0,
            cells: core::ptr::null(),
            rows: 5,
            columns: 0,
            formats: core::ptr::null(),
            out: core::ptr::null_mut(),
        }
        .run();
        assert_eq!(code, OK);
    }

    #[test]
    fn an_unaligned_sample_is_a_contract_not_a_read() {
        // A blank cell needs no bytes and points nowhere in particular.
        let blank = [Span { offset: 0, len: 0 }];
        let mut out = [sentinel()];
        let code = Call {
            input: core::ptr::null(),
            input_len: 0,
            arena: core::ptr::null(),
            arena_len: 0,
            cells: blank.as_ptr(),
            rows: 1,
            columns: 1,
            formats: core::ptr::null(),
            out: out.as_mut_ptr(),
        }
        .run();
        assert_eq!(code, OK);
        assert_eq!(out[0].status, UNRESOLVED);
    }
}
