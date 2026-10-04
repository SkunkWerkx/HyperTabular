//! A binding to the workbook core, as small as one can be: it owns every buffer, hands
//! them over on every call, and grows the one a call names — into fresh memory, with the
//! old contents copied across and the rest filled with junk, so a core that leaned on
//! anything but what the contract promises would show it.
//!
//! And the comparison the tests make with it: the same read with buffers that start
//! empty and with buffers that never run out, which have to agree.

#![allow(dead_code, clippy::needless_range_loop)]

use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, ERR_WINDOW,
    Failure, Filled, OK, Opened, Slot, Span,
};
use hypertabular::kernel::door::Door;
use hypertabular::kernel::workbook::{Memory, State, book, rows};
use hypertabular::workbook::SheetOptions;
use hypertabular::{CivilDateTime, Date, Decimal, Duration, Timestamp};

/// How a harness sizes its buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sizing {
    /// Nothing to start with, and only what each refusal asks for.
    Stingy,
    /// Plenty from the start.
    Roomy,
}

/// One sheet as `sheets` listed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: Vec<u8>,
    pub part: Vec<u8>,
    pub hidden: bool,
    pub index: u32,
}

/// A whole sheet, read: the header's names, each row's number and cells, and the failure
/// that ended it, if one did.
#[derive(Debug, PartialEq, Eq)]
pub struct Read {
    pub header: Option<Vec<Vec<u8>>>,
    pub rows: Vec<(u32, Vec<String>)>,
    pub failed: bool,
}

pub struct Harness<'c> {
    pub container: &'c [u8],
    pub state: Box<State>,
    pub window: Vec<u8>,
    pub arena: Vec<u8>,
    pub cells: Vec<Span>,
    pub row: Vec<Slot>,
    pub strings: Vec<u8>,
    pub table: Vec<Span>,
    pub kinds: Vec<u8>,
    /// How many calls were refused for room and made again.
    pub refusals: usize,
    /// The largest window a refusal may ask for before the harness gives up.
    pub window_limit: usize,
    pub last_failure: Failure,
}

/// Grows `buffer` to `needed` elements the way the contract says: what it held is kept.
fn grow<T: Copy>(buffer: &mut Vec<T>, needed: u64, junk: T) {
    let needed = needed as usize;
    assert!(
        needed > buffer.len(),
        "asked for {needed}, already had {}",
        buffer.len()
    );
    let mut larger = Vec::with_capacity(needed);
    larger.extend_from_slice(buffer);
    larger.resize(needed, junk);
    *buffer = larger;
}

impl<'c> Harness<'c> {
    pub fn new(container: &'c [u8], sizing: Sizing) -> Harness<'c> {
        let roomy = sizing == Sizing::Roomy;
        Harness {
            container,
            state: Box::new(State::new()),
            window: vec![0xAA; if roomy { 1 << 20 } else { 0 }],
            arena: vec![0xAA; if roomy { 1 << 20 } else { 0 }],
            cells: vec![Span::default(); if roomy { 1 << 16 } else { 0 }],
            row: Vec::new(),
            strings: Vec::new(),
            table: Vec::new(),
            kinds: Vec::new(),
            refusals: 0,
            window_limit: 1 << 28,
            last_failure: Failure::default(),
        }
    }

    /// Makes `call` until it stops asking for room. `Err` carries what a structural
    /// failure reported, with whatever the call had finished before it.
    fn drive(
        &mut self,
        mut call: impl FnMut(&mut State, &[u8], &mut Memory<'_>, &mut Filled) -> i32,
    ) -> Result<Filled, Filled> {
        loop {
            let mut out = Filled::default();
            let code = {
                let mut memory = Memory {
                    window: &mut self.window,
                    arena: &mut self.arena,
                    cells: &mut self.cells,
                    row: &mut self.row,
                    strings: &self.strings,
                    table: &self.table,
                    kinds: &self.kinds,
                };
                call(&mut self.state, self.container, &mut memory, &mut out)
            };
            match code {
                OK => return Ok(out),
                ERR_STRUCTURE => {
                    self.last_failure = out.failure;
                    assert_ne!(out.failure.code, 0, "a failure names itself");
                    return Err(out);
                }
                ERR_WINDOW => {
                    assert!(
                        out.needed as usize <= self.window_limit,
                        "window of {} asked for",
                        out.needed
                    );
                    grow(&mut self.window, out.needed, 0x55);
                }
                ERR_ARENA => grow(&mut self.arena, out.needed, 0x55),
                ERR_CELLS => grow(
                    &mut self.cells,
                    out.needed,
                    Span {
                        offset: 0x5555_5555,
                        len: 0x5555_5555,
                    },
                ),
                other => panic!("the core refused the harness's own call: {other}"),
            }
            self.refusals += 1;
            assert!(self.refusals < 1_000_000, "the core keeps asking");
        }
    }

    pub fn open(&mut self) -> Result<Opened, Failure> {
        loop {
            let mut out = Opened::default();
            let code = {
                let mut memory = Memory {
                    window: &mut self.window,
                    arena: &mut self.arena,
                    cells: &mut self.cells,
                    row: &mut self.row,
                    strings: &[],
                    table: &[],
                    kinds: &[],
                };
                book::open(&mut self.state, self.container, &mut memory, &mut out)
            };
            match code {
                OK => return Ok(out),
                ERR_STRUCTURE => {
                    assert_ne!(out.failure.code, 0);
                    return Err(out.failure);
                }
                ERR_WINDOW => grow(&mut self.window, out.needed, 0x55),
                ERR_ARENA => grow(&mut self.arena, out.needed, 0x55),
                other => panic!("open returned {other}"),
            }
            self.refusals += 1;
            assert!(self.refusals < 1_000, "open keeps asking");
        }
    }

    pub fn sheets(&mut self) -> Result<Vec<Listed>, Failure> {
        let out = self.drive(book::sheets).map_err(|out| out.failure)?;
        Ok((0..out.rows as usize)
            .map(|index| {
                let name = self.cells[index * 3];
                let part = self.cells[index * 3 + 1];
                let last = self.cells[index * 3 + 2];
                let text = |span: Span| {
                    self.arena[span.offset as usize..span.offset as usize + span.len()].to_vec()
                };
                Listed {
                    name: text(name),
                    part: text(part),
                    hidden: last.offset & 1 != 0,
                    index: last.len,
                }
            })
            .collect())
    }

    /// Loads the shared strings and the format kinds, and keeps them for the sheets.
    pub fn load(&mut self) -> Result<(), Failure> {
        let out = self.drive(book::strings).map_err(|out| out.failure)?;
        let strings = self.arena[..out.arena_used as usize].to_vec();
        let table = self.cells[..out.rows as usize].to_vec();
        let out = self.drive(book::styles).map_err(|out| out.failure)?;
        self.kinds = self.arena[..out.rows as usize].to_vec();
        self.strings = strings;
        self.table = table;
        Ok(())
    }

    fn text(&self, span: Span) -> &[u8] {
        let store = if span.flagged() {
            &self.arena
        } else {
            &self.strings
        };
        &store[span.offset as usize..span.offset as usize + span.len()]
    }

    /// Reads one sheet to its end through `specs`, `max_rows` at a time.
    pub fn read(
        &mut self,
        listed: &Listed,
        options: SheetOptions,
        specs: &[ColumnSpec],
        max_rows: usize,
    ) -> Read {
        let width = specs
            .iter()
            .map(|spec| spec.ordinal as usize + 1)
            .max()
            .unwrap_or(0);
        self.row = vec![Slot::default(); width];
        let mut read = Read {
            header: None,
            rows: Vec::new(),
            failed: false,
        };
        let mut out = Filled::default();
        let code = rows::sheet(
            &mut self.state,
            self.container,
            &listed.part,
            listed.index,
            options.has_header,
            options.skip_empty_rows,
            &mut out,
        );
        if code != OK {
            assert_eq!(code, ERR_STRUCTURE);
            assert_ne!(out.failure.code, 0, "a failure names itself");
            self.last_failure = out.failure;
            read.failed = true;
            return read;
        }
        if options.has_header {
            match self.drive(rows::header) {
                Ok(out) => {
                    read.header = Some(
                        (0..out.rows as usize)
                            .map(|index| self.text(self.cells[index]).to_vec())
                            .collect(),
                    );
                }
                Err(_) => {
                    read.failed = true;
                    return read;
                }
            }
        }
        let doors: Vec<Door> = specs
            .iter()
            .map(|spec| Door::from_code(spec.door, spec.param).unwrap())
            .collect();
        let mut values: Vec<Vec<u8>> = doors
            .iter()
            .map(|&door| vec![0xAA; size_of_door(door) * max_rows])
            .collect();
        let mut verdicts: Vec<Vec<CellVerdict>> = specs
            .iter()
            .map(|_| vec![CellVerdict::default(); max_rows])
            .collect();
        let columns: Vec<ColumnBuffer> = values
            .iter_mut()
            .zip(verdicts.iter_mut())
            .map(|(values, verdicts)| ColumnBuffer {
                values: values.as_mut_ptr().cast(),
                verdicts: verdicts.as_mut_ptr(),
            })
            .collect();
        loop {
            let filled = self.drive(|state, container, memory, out| {
                // SAFETY: every column's arrays hold `max_rows` elements.
                unsafe { rows::fill(state, container, specs, &columns, max_rows, memory, out) }
            });
            let (out, failed) = match filled {
                Ok(out) => (out, false),
                Err(out) => (out, true),
            };
            let per_row = specs.len() + 1;
            for row in 0..out.rows as usize {
                let mut cells = Vec::with_capacity(specs.len());
                for (index, &door) in doors.iter().enumerate() {
                    let verdict = verdicts[index][row];
                    let raw = self.cells[row * per_row + index];
                    cells.push(match verdict.reason {
                        0 => format!("ok {}", self.value(door, &values[index], row)),
                        1 => format!("empty @{}+{}", verdict.offset, verdict.len),
                        reason => format!(
                            "fault {reason} @{}+{} {:?}",
                            verdict.offset,
                            verdict.len,
                            String::from_utf8_lossy(self.text(raw))
                        ),
                    });
                }
                read.rows
                    .push((self.cells[row * per_row + specs.len()].offset, cells));
            }
            if failed {
                read.failed = true;
                // And it says so again.
                let again = self.drive(|state, container, memory, out| {
                    // SAFETY: as above.
                    unsafe { rows::fill(state, container, specs, &columns, max_rows, memory, out) }
                });
                assert_eq!(again.unwrap_err().rows, 0);
                return read;
            }
            if (out.rows as usize) < max_rows {
                return read;
            }
        }
    }

    fn value(&self, door: Door, values: &[u8], row: usize) -> String {
        fn at<T: Copy + std::fmt::Debug>(values: &[u8], row: usize) -> String {
            let size = std::mem::size_of::<T>();
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
            Door::U64 => at::<u64>(values, row),
            Door::F32 => at::<f32>(values, row),
            Door::F64 => at::<f64>(values, row),
            Door::Uuid => at::<[u8; 16]>(values, row),
            Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => at::<Timestamp>(values, row),
            Door::Date | Door::DateOrdered(_) => at::<Date>(values, row),
            Door::DateTime(_) => at::<CivilDateTime>(values, row),
            Door::Time => at::<u64>(values, row),
            Door::Duration => at::<Duration>(values, row),
            Door::Decimal => at::<Decimal>(values, row),
            Door::Text => {
                let size = std::mem::size_of::<Span>();
                let bytes = &values[row * size..(row + 1) * size];
                // SAFETY: as above.
                let span = unsafe { bytes.as_ptr().cast::<Span>().read_unaligned() };
                format!("{:?}", String::from_utf8_lossy(self.text(span)))
            }
        }
    }
}

pub fn size_of_door(door: Door) -> usize {
    use std::mem::size_of;
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

/// Every door over every one of `width` source columns, parameters varied by column.
pub fn every_door(width: u32) -> Vec<ColumnSpec> {
    let mut specs = Vec::new();
    for ordinal in 0..width {
        for code in 1..=22u32 {
            let param = match code {
                14 => 1 + ordinal % 4,
                20 | 21 => 1 + ordinal % 3,
                22 => 1 + ordinal % 2,
                _ => 0,
            };
            specs.push(ColumnSpec::new(ordinal, code, param));
        }
    }
    specs
}

/// Reads every sheet of `container` with buffers that start empty and grow only as the
/// core asks, and again with room to spare, and holds the two readings to each other: the
/// same sheets, the same header, the same rows cell for cell, the same place to fail.
/// Whatever one reading resumes across, the other reads in one piece. (What the right
/// reading *is* comes from `corpus/workbook.json`, written by the std reader this core
/// replaced, before that reader was deleted.) Returns how many times the stingy reading
/// was refused for room, and how many rows were compared.
pub fn compare(
    container: &[u8],
    options: SheetOptions,
    width: u32,
    max_rows: usize,
) -> (usize, usize) {
    compare_plan(container, options, &every_door(width), max_rows)
}

/// [`compare`] through a plan of the caller's.
pub fn compare_plan(
    container: &[u8],
    options: SheetOptions,
    specs: &[ColumnSpec],
    max_rows: usize,
) -> (usize, usize) {
    let mut stingy = Harness::new(container, Sizing::Stingy);
    let mut roomy = Harness::new(container, Sizing::Roomy);
    let open = |harness: &mut Harness<'_>| {
        let opened = harness.open().map_err(|failure| failure.code)?;
        let sheets = harness.sheets().map_err(|failure| failure.code)?;
        harness.load().map_err(|failure| failure.code)?;
        Ok::<_, u32>((opened, sheets))
    };
    let (opened, sheets) = match (open(&mut stingy), open(&mut roomy)) {
        (Ok(ours), Ok(theirs)) => {
            assert_eq!(ours, theirs);
            ours
        }
        (Err(ours), Err(theirs)) => {
            assert_eq!(ours, theirs);
            return (stingy.refusals, 0);
        }
        (ours, theirs) => panic!("stingy says {ours:?} and roomy says {theirs:?}"),
    };
    assert!(opened.format == 1 || opened.format == 2);
    assert_eq!(
        (&stingy.strings, &stingy.table, &stingy.kinds),
        (&roomy.strings, &roomy.table, &roomy.kinds)
    );
    let mut rows = 0;
    for (index, listed) in sheets.iter().enumerate() {
        let ours = stingy.read(listed, options, specs, max_rows);
        // The roomy reading also takes its rows in batches of another size.
        let theirs = roomy.read(listed, options, specs, max_rows * 3 + 1);
        assert_eq!(ours.header, theirs.header, "the header of sheet {index}");
        assert_eq!(
            ours.rows.len(),
            theirs.rows.len(),
            "the rows of sheet {index}"
        );
        for (row, (ours, theirs)) in ours.rows.iter().zip(&theirs.rows).enumerate() {
            assert_eq!(ours.0, theirs.0, "the number of row {row} of sheet {index}");
            for (column, (ours, theirs)) in ours.1.iter().zip(&theirs.1).enumerate() {
                assert_eq!(
                    ours, theirs,
                    "sheet {index}, row {row}, source column {}, door {}",
                    specs[column].ordinal, specs[column].door
                );
            }
        }
        assert_eq!(ours.failed, theirs.failed, "whether sheet {index} failed");
        if ours.failed {
            assert_eq!(stingy.last_failure, roomy.last_failure);
        }
        rows += ours.rows.len();
    }
    (stingy.refusals, rows)
}
