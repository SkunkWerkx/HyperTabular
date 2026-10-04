//! The workbook core's allocation story, which is the delimited core's: it has none. A
//! counting allocator watches whole workbooks go through it — XLSX and ODS, deflated,
//! opened, their sheets listed, their strings and styles loaded, every sheet read through
//! every door, and one that breaks part-way — and the count is nothing at all: every byte
//! the core touched was handed to it.
//!
//! `check-core.sh` proves the same of the shipped library a different way (it imports no
//! allocator); this is the proof for the code as the test suite builds it.
//!
//! One `#[test]` function on purpose: the counter is process-wide and `cargo test` spawns
//! a thread per test function.

#[path = "support/packages.rs"]
mod packages;
#[path = "support/workbook.rs"]
mod support;

use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_STRUCTURE, Failure, Filled, OK, Opened, Slot, Span,
};
use hypertabular::kernel::door::Door;
use hypertabular::kernel::workbook::{Memory, State, book, rows};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const ROWS: usize = 128;
const WIDTH: u32 = 5;

/// Everything a binding owns, allocated once.
struct Room {
    state: Box<State>,
    window: Vec<u8>,
    arena: Vec<u8>,
    cells: Vec<Span>,
    row: Vec<Slot>,
    strings: Vec<u8>,
    strings_len: usize,
    table: Vec<Span>,
    table_len: usize,
    kinds: Vec<u8>,
    kinds_len: usize,
}

impl Room {
    fn new() -> Room {
        Room {
            state: Box::new(State::new()),
            window: vec![0; 256 * 1024],
            arena: vec![0; 1 << 20],
            cells: vec![Span::default(); 1 << 16],
            row: vec![Slot::default(); WIDTH as usize],
            strings: vec![0; 1 << 16],
            strings_len: 0,
            table: vec![Span::default(); 1 << 10],
            table_len: 0,
            kinds: vec![0; 1 << 10],
            kinds_len: 0,
        }
    }

    fn call(
        &mut self,
        container: &[u8],
        call: impl FnOnce(&mut State, &[u8], &mut Memory<'_>, &mut Filled) -> i32,
    ) -> (i32, Filled) {
        let mut out = Filled::default();
        let mut memory = Memory {
            window: &mut self.window,
            arena: &mut self.arena,
            cells: &mut self.cells,
            row: &mut self.row,
            strings: &self.strings[..self.strings_len],
            table: &self.table[..self.table_len],
            kinds: &self.kinds[..self.kinds_len],
        };
        let code = call(&mut self.state, container, &mut memory, &mut out);
        (code, out)
    }

    /// Opens `container` and reads every sheet of it to its end. Returns the rows read
    /// and the failure that stopped a sheet, if one did.
    fn read(
        &mut self,
        container: &[u8],
        specs: &[ColumnSpec],
        columns: &[ColumnBuffer],
    ) -> (u64, u32) {
        (self.strings_len, self.table_len, self.kinds_len) = (0, 0, 0);
        let mut opened = Opened::default();
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
            book::open(&mut self.state, container, &mut memory, &mut opened)
        };
        assert_eq!(code, OK);

        // The sheets, copied out of the buffers the next calls reuse.
        let (code, out) = self.call(container, book::sheets);
        assert_eq!(code, OK);
        let mut sheets = [([0u8; 96], 0usize, 0u32); 4];
        let count = out.rows as usize;
        assert!((1..=sheets.len()).contains(&count));
        for (index, sheet) in sheets.iter_mut().enumerate().take(count) {
            let part = self.cells[index * 3 + 1];
            let bytes = &self.arena[part.offset as usize..part.offset as usize + part.len()];
            sheet.0[..bytes.len()].copy_from_slice(bytes);
            sheet.1 = bytes.len();
            sheet.2 = self.cells[index * 3 + 2].len;
        }

        let (code, out) = self.call(container, book::strings);
        assert_eq!(code, OK);
        let (bytes, count_strings) = (out.arena_used as usize, out.rows as usize);
        self.strings[..bytes].copy_from_slice(&self.arena[..bytes]);
        self.table[..count_strings].copy_from_slice(&self.cells[..count_strings]);
        let (code, out) = self.call(container, book::styles);
        assert_eq!(code, OK);
        self.kinds_len = out.rows as usize;
        self.kinds[..self.kinds_len].copy_from_slice(&self.arena[..self.kinds_len]);
        (self.strings_len, self.table_len) = (bytes, count_strings);

        let mut rows_read = 0u64;
        let mut failure = 0u32;
        for (sheet_index, sheet) in sheets.iter().enumerate().take(count) {
            let mut out = Filled::default();
            let code = rows::sheet(
                &mut self.state,
                container,
                &sheet.0[..sheet.1],
                sheet.2,
                sheet_index % 2 == 0,
                sheet_index % 3 != 1,
                &mut out,
            );
            assert_eq!(code, OK);
            let (code, _) = self.call(container, rows::header);
            assert_eq!(code, OK);
            loop {
                let (code, out) = self.call(container, |state, container, memory, out| {
                    // SAFETY: every column's arrays hold `ROWS` elements.
                    unsafe { rows::fill(state, container, specs, columns, ROWS, memory, out) }
                });
                rows_read += out.rows;
                if code == ERR_STRUCTURE {
                    failure = out.failure.code;
                    break;
                }
                assert_eq!(code, OK);
                if (out.rows as usize) < ROWS {
                    break;
                }
            }
        }
        (rows_read, failure)
    }
}

#[test]
fn allocation_free() {
    // Everything the caller owns, allocated before the count starts.
    let xlsx = packages::xlsx_generated(5, 400, u64::from(WIDTH), true);
    let stored = packages::xlsx_generated(6, 400, u64::from(WIDTH), false);
    let ods = packages::ods_generated(5, 300, u64::from(WIDTH), true);
    let mut sheet = packages::xlsx_sheet(9, 200, u64::from(WIDTH));
    sheet.push_str("<row><c t=\"s\"><v>99</v></c></row>");
    let broken = packages::xlsx(&[("Data", &sheet)], packages::SHARED, false, true);

    let specs = support::every_door(WIDTH);
    let mut values: Vec<Vec<u8>> = specs
        .iter()
        .map(|spec| {
            let door = Door::from_code(spec.door, spec.param).unwrap();
            vec![0u8; support::size_of_door(door) * ROWS]
        })
        .collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = specs
        .iter()
        .map(|_| vec![CellVerdict::default(); ROWS])
        .collect();
    let columns: Vec<ColumnBuffer> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        })
        .collect();
    let mut room = Room::new();

    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    let (xlsx_rows, xlsx_failure) = room.read(&xlsx, &specs, &columns);
    let (stored_rows, stored_failure) = room.read(&stored, &specs, &columns);
    let (ods_rows, ods_failure) = room.read(&ods, &specs, &columns);
    let (broken_rows, broken_failure) = room.read(&broken, &specs, &columns);
    let allocations = ALLOC_COUNT.load(Ordering::SeqCst) - before;

    assert_eq!(allocations, 0, "the workbook core allocated");
    assert!(xlsx_rows > 400 && stored_rows > 400 && ods_rows > 300);
    assert_eq!((xlsx_failure, stored_failure, ods_failure), (0, 0, 0));
    assert!(broken_rows > 100);
    assert_eq!(broken_failure, Failure::SHARED_STRING);
}
