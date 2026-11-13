//! The workbook core's allocation story, which is the delimited core's: it has none. A
//! counting allocator watches whole workbooks go through it — XLSX, ODS and Numbers, deflated,
//! opened, their sheets listed, their strings and styles loaded, every sheet read through
//! every door, and one that breaks part-way — and the count is nothing at all: every byte
//! the core touched was handed to it.
//!
//! `check-core.sh` proves the same of the shipped library a different way (it imports no
//! allocator); this is the proof for the code as the test suite builds it.
//!
//! The count is the test thread's own: the core runs on its caller's thread and starts
//! none, while the harness's threads allocate as they please (a process-wide count once
//! caught four of theirs on linux-arm64).

#[path = "support/packages.rs"]
mod packages;
#[path = "support/workbook.rs"]
mod support;

use hypertabular::kernel::abi::{
    CellNote, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith,
    ERR_STRUCTURE, Failure, Filled, OK, Opened, RawSheetOptions, Slot, Span,
};
use hypertabular::kernel::door::Door;
use hypertabular::kernel::workbook::{Memory, State, book, rows};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;

thread_local! {
    // Const-initialised and without a destructor, so reading them never allocates.
    static COUNTED: Cell<bool> = const { Cell::new(false) };
    // Per thread: two tests here run at once, and neither's fixtures are the other's count.
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTED.try_with(Cell::get).unwrap_or(false) {
            ALLOC_COUNT.set(ALLOC_COUNT.get() + 1);
        }
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

    /// Opens `container`, loads its tables, and says where its sheets are.
    fn load(&mut self, container: &[u8]) -> ([([u8; 96], usize, u32); 4], usize) {
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
        (sheets, count)
    }

    /// Opens `container` and reads its first sheet through a range, a skip and a header row, and
    /// everything `fill_with` takes. Returns the rows read.
    fn read_with(
        &mut self,
        container: &[u8],
        specs: &[ColumnSpecWith],
        columns: &[ColumnBufferWith],
        tokens: &[u8],
        widths: &mut [u32],
    ) -> u64 {
        let (sheets, _) = self.load(container);
        let sheet = &sheets[0];
        let options = RawSheetOptions {
            size: RawSheetOptions::SIZE,
            has_header: 1,
            skip_empty_rows: 1,
            skip: 2,
            header_row: 1,
            first_row: 1,
            first_column: 2,
            last_row: 12,
            last_column: 5,
        };
        let mut out = Filled::default();
        let code = rows::sheet_with(
            &mut self.state,
            container,
            &sheet.0[..sheet.1],
            sheet.2,
            &options,
            &mut out,
        );
        assert_eq!(code, OK);
        let (code, _) = self.call(container, rows::header);
        assert_eq!(code, OK);
        let mut rows_read = 0u64;
        loop {
            let (code, out) = self.call(container, |state, container, memory, out| {
                // SAFETY: every column's arrays, and the widths, hold `ROWS` elements, and the
                // lists are the caller's for the whole call.
                unsafe {
                    rows::fill_with(
                        state, container, specs, columns, ROWS, memory, tokens, widths, out,
                    )
                }
            });
            assert_eq!(code, OK);
            rows_read += out.rows;
            if (out.rows as usize) < ROWS {
                return rows_read;
            }
        }
    }

    /// Reads every sheet of `container` to its end. Returns the rows read and the failure that
    /// stopped a sheet, if one did.
    fn read(
        &mut self,
        container: &[u8],
        specs: &[ColumnSpec],
        columns: &[ColumnBuffer],
    ) -> (u64, u32) {
        let (sheets, count) = self.load(container);
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

/// A range, a skip and a header row over each sheet, read through `fill_with` with tokens, notes,
/// date options and row widths: the options that came after the first core ask for no more.
#[test]
fn allocation_free_through_every_option() {
    COUNTED.set(true);
    let xlsx = packages::ranges_xlsx();
    let ods = packages::ranges_ods();
    let numbers = packages::ranges_numbers();
    let uncached_xlsx = packages::uncached_xlsx();
    let uncached_ods = packages::uncached_ods();
    let specs = support::every_door(WIDTH);
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
    let tokens = [2u8, 0, 0, 0, b'N', b'A', 1, 0, 0, 0, b'-'];
    // Constraints on every door that takes them, an `enum` in each: a bound and a list of numbers,
    // and a trimmed list of text.
    let entries: Vec<hypercast::Decimal> = ["1", "2", "7"]
        .iter()
        .map(|entry| hypercast::decimal_for_declaration(entry).expect("a decimal"))
        .collect();
    let bound = hypercast::NumericRules::new()
        .maximum(hypercast::decimal_for_declaration("5").expect("a decimal"));
    let numeric = hypercast::Constraints::<hypercast::Decimal>::numeric(bound, Some(&entries))
        .expect("it declares");
    let words: &[&str] = &["a", "x", "hi"];
    let text = hypercast::Constraints::text(hypercast::TextRules::new().trim(true), Some(words))
        .expect("it declares");
    let mut numeric_list = vec![0u8; numeric.pack_enum(&mut []).expect("it packs")];
    numeric.pack_enum(&mut numeric_list);
    let mut text_list = vec![0u8; text.pack_enum(&mut []).expect("it packs")];
    text.pack_enum(&mut text_list);
    let (numeric_raw, text_raw) = (numeric.raw(), text.raw());
    let with: Vec<ColumnSpecWith> = specs
        .iter()
        .map(|spec| {
            let mut with = ColumnSpecWith::new(spec.ordinal, spec.door, spec.param);
            with.date.size = hypercast::RawDateOptions::SIZE;
            with.date.flags = hypertabular::DateOptions::COMPACT_DATE;
            with.months = months.as_ptr();
            with.months_len = months.len();
            match Door::from_code(spec.door, spec.param) {
                Some(Door::Text) => {
                    with.constraints = &text_raw;
                    with.enum_ptr = text_list.as_ptr();
                    with.enum_len = text_list.len();
                }
                Some(
                    Door::I8
                    | Door::I16
                    | Door::I32
                    | Door::I64
                    | Door::U8
                    | Door::U16
                    | Door::U32
                    | Door::U64
                    | Door::F32
                    | Door::F64
                    | Door::Decimal,
                ) => {
                    with.constraints = &numeric_raw;
                    with.enum_ptr = numeric_list.as_ptr();
                    with.enum_len = numeric_list.len();
                }
                _ => {}
            }
            with
        })
        .collect();
    let constraints_xlsx = packages::constraints_xlsx();
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
    let mut notes: Vec<Vec<CellNote>> = specs
        .iter()
        .map(|_| vec![CellNote::default(); ROWS])
        .collect();
    let unchecked = hypercast::RawCheck {
        keyword: 0,
        adjusted: 0,
        enum_index: 0,
    };
    let mut checks: Vec<Vec<hypercast::RawCheck>> =
        specs.iter().map(|_| vec![unchecked; ROWS]).collect();
    let columns: Vec<ColumnBufferWith> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .zip(notes.iter_mut())
        .zip(checks.iter_mut())
        .map(|(((values, verdicts), notes), checks)| ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
            notes: notes.as_mut_ptr(),
            checks: checks.as_mut_ptr(),
        })
        .collect();
    let mut widths = vec![0u32; ROWS];
    let mut room = Room::new();

    let before = ALLOC_COUNT.get();
    let mut total = 0u64;
    for container in [&xlsx[..], &ods[..], &numbers[..]] {
        total += room.read_with(container, &with, &columns, &tokens, &mut widths);
    }
    let uncached = room.read_with(&uncached_xlsx, &with, &columns, &tokens, &mut widths)
        + room.read_with(&uncached_ods, &with, &columns, &tokens, &mut widths);
    let constrained = room.read_with(&constraints_xlsx, &with, &columns, &tokens, &mut widths);
    let allocations = ALLOC_COUNT.get() - before;
    let broken = verdicts
        .iter()
        .flat_map(|column| column.iter().take(constrained as usize))
        .filter(|verdict| verdict.reason == CellVerdict::VIOLATION)
        .count();
    assert!(
        constrained > 0 && broken > 0,
        "{constrained} rows, {broken} violations"
    );

    assert_eq!(allocations, 0, "the workbook core allocated");
    // Three formats, rows 5 to 12 less the empty one, the header (row 4) read off by `skip` first.
    assert_eq!(total, 3 * 7, "the same rows from each");
    // What's left of rows 1 to 12 from column B, the filter having had the formulas with nothing
    // spelled, and `skip`, `header_row` and the header what comes first of the rest.
    assert_eq!(uncached, 9);
}

#[test]
fn allocation_free() {
    COUNTED.set(true);
    // Everything the caller owns, allocated before the count starts.
    let xlsx = packages::xlsx_generated(5, 400, u64::from(WIDTH), true);
    let stored = packages::xlsx_generated(6, 400, u64::from(WIDTH), false);
    let ods = packages::ods_generated(5, 300, u64::from(WIDTH), true);
    let numbers = packages::numbers_generated(5, 300, u64::from(WIDTH), true);
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

    let before = ALLOC_COUNT.get();
    let (xlsx_rows, xlsx_failure) = room.read(&xlsx, &specs, &columns);
    let (stored_rows, stored_failure) = room.read(&stored, &specs, &columns);
    let (ods_rows, ods_failure) = room.read(&ods, &specs, &columns);
    let (numbers_rows, numbers_failure) = room.read(&numbers, &specs, &columns);
    let (broken_rows, broken_failure) = room.read(&broken, &specs, &columns);
    let allocations = ALLOC_COUNT.get() - before;

    assert_eq!(allocations, 0, "the workbook core allocated");
    assert!(xlsx_rows > 400 && stored_rows > 400 && ods_rows > 300 && numbers_rows > 300);
    assert_eq!(
        (xlsx_failure, stored_failure, ods_failure, numbers_failure),
        (0, 0, 0, 0)
    );
    assert!(broken_rows > 100);
    assert_eq!(broken_failure, Failure::SHARED_STRING);
}
