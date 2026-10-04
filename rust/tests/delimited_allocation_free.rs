//! The reader's allocation story, asserted by a counting allocator: once the first scan
//! has sized the cell/row arrays, delivering rows over a slice allocates nothing — for
//! plain cells and for cells whose only quotes are their outer pair. Cells with `""`
//! inside use the scratch arena, which is the documented copying path; after it has
//! grown once, it too allocates nothing. One `#[test]` function, one thread.

use hypertabular::delimited::{Dialect, Reader};
use hypertabular::{Batch, Column, Door, Plan, fill_batch};
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

fn allocs_during<T>(f: impl FnOnce() -> T) -> usize {
    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    std::hint::black_box(f());
    ALLOC_COUNT.load(Ordering::SeqCst) - before
}

fn text(rows: usize, escaped: bool) -> Vec<u8> {
    let mut text = b"id,name,score\n".to_vec();
    for i in 0..rows {
        let name = if escaped {
            "\"n \"\"q\"\" x\""
        } else {
            "\"plain, quoted\""
        };
        text.extend_from_slice(format!("{i},{name},{}.5\n", i % 100).as_bytes());
    }
    text
}

#[test]
fn allocation_free() {
    // Plain and outer-quoted cells: zero allocations per row once warm.
    let plain = text(3000, false);
    let mut reader = Reader::from_slice(&plain, Dialect::CSV).unwrap();
    // The first scan sizes the arrays.
    assert!(reader.next_row().unwrap().is_some());
    let mut touched = 0usize;
    let allocs = allocs_during(|| {
        while let Some(row) = reader.next_row().unwrap() {
            touched += row.get(1).unwrap().len();
        }
    });
    assert_eq!(allocs, 0, "plain rows allocated {allocs} time(s)");
    assert!(touched > 0);

    // Escaped cells: the scratch arena grows once, then nothing.
    let escaped = text(3000, true);
    let mut reader = Reader::from_slice(&escaped, Dialect::CSV).unwrap();
    for _ in 0..3 {
        assert!(reader.next_row().unwrap().is_some());
    }
    let allocs = allocs_during(|| {
        while let Some(row) = reader.next_row().unwrap() {
            assert_eq!(row.get(1).unwrap(), b"n \"q\" x");
        }
    });
    assert_eq!(
        allocs, 0,
        "escaped rows allocated {allocs} time(s) after warm-up"
    );

    // A typed batch over a warm reader and a warm batch: zero.
    let plan = Plan::new(vec![
        Column::new(0, Door::I64),
        Column::new(1, Door::Text),
        Column::new(2, Door::F64),
    ]);
    let mut batch = Batch::new();
    let mut reader = Reader::from_slice(&plain, Dialect::CSV).unwrap();
    fill_batch(&mut reader, &plan, &mut batch, 512).unwrap();
    fill_batch(&mut reader, &plan, &mut batch, 512).unwrap();
    let allocs = allocs_during(|| fill_batch(&mut reader, &plan, &mut batch, 512).unwrap());
    assert_eq!(allocs, 0, "a warm batch fill allocated {allocs} time(s)");
    assert_eq!(batch.rows(), 512);
}
