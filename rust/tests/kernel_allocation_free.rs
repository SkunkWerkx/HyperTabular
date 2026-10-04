//! The core's allocation story, which is that it has none. A counting allocator watches
//! a whole input go through it — the state, the header, every door, escaped cells, a
//! structural failure — and the count is not "nothing once warm" but nothing at all:
//! every byte the core touched was handed to it.
//!
//! `check-core.sh` proves the same thing of the shipped library a different way (it
//! imports no allocator); this is the proof for the code as the test suite builds it.
//!
//! One `#[test]` function on purpose: the counter is process-wide and `cargo test`
//! spawns a thread per test function.

use hypertabular::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_STRUCTURE, Filled, OK, Span,
};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
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

const ROWS: usize = 256;

#[test]
fn allocation_free() {
    // Everything the caller owns, allocated before the count starts.
    let mut text = b"\xEF\xBB\xBFid,amount,when,name,flag,note\n".to_vec();
    for row in 0..5_000 {
        text.extend_from_slice(
            format!(
                "{row},\"{}.{:02}\",2024-01-{:02}T10:30:00Z,\"n \"\"{row}\"\" x\",{},{}\n",
                row * 37,
                row % 100,
                1 + row % 28,
                if row % 2 == 0 { "yes" } else { "no" },
                if row % 7 == 0 { "not a number" } else { "12.5" },
            )
            .as_bytes(),
        );
    }
    text.extend_from_slice(b"too,few\n");

    // Every door, over the six columns.
    let mut specs = Vec::new();
    for code in 1..=22u32 {
        specs.push(ColumnSpec::new((code - 1) % 6, code, 1));
    }
    let mut values: Vec<Vec<u8>> = specs.iter().map(|_| vec![0u8; 16 * ROWS]).collect();
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
    let mut cells = vec![Span::default(); ROWS * 7];
    let mut arena = vec![0u8; 64 * 1024];
    let mut names = [Span::default(); 16];

    let before = ALLOC_COUNT.load(Ordering::SeqCst);

    let dialect = RawDialect {
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
    };
    let mut state = State::init(dialect).expect("a valid dialect");
    let mut out = Filled::default();
    let mut at = 0usize;
    let code = fill::header(&mut state, &text, true, &mut names, &mut arena, &mut out);
    at += out.consumed as usize;
    let header = (code, out.rows);
    let mut rows = 0u64;
    let mut faults = 0u64;
    let last = loop {
        // SAFETY: sixteen bytes a row covers every door's value.
        let code = unsafe {
            fill::fill(
                &mut state,
                &text[at..],
                true,
                &specs,
                &columns,
                ROWS,
                &mut cells,
                &mut arena,
                &mut out,
            )
        };
        if code != OK || (out.rows == 0 && out.consumed == 0) {
            break code;
        }
        at += out.consumed as usize;
        rows += out.rows;
        for column in &verdicts {
            faults += column
                .iter()
                .take(out.rows as usize)
                .filter(|verdict| !verdict.is_ok())
                .count() as u64;
        }
    };

    let allocations = ALLOC_COUNT.load(Ordering::SeqCst) - before;

    assert_eq!(header, (OK, 6));
    assert_eq!(rows, 5_000);
    assert!(faults > 0, "the failure paths were exercised too");
    assert_eq!(last, ERR_STRUCTURE, "and so was a structural failure");
    assert_eq!(allocations, 0, "the core allocated {allocations} time(s)");
}
