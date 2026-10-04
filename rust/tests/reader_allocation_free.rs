//! The binding's allocation story: it allocates — that is its job, and the core's not to
//! — but once, not per row and not per batch. A reader's buffers are sized by its first
//! batches; after them a read allocates nothing, whether the text is a slice, a stream,
//! or a sheet of a workbook. A counting allocator watches.
//!
//! One `#[test]` function on purpose: the counter is process-wide and `cargo test` spawns
//! a thread per test function.

#[path = "support/packages.rs"]
mod packages;

use hypertabular::{Column, DateOrder, DelimitedReader, Dialect, SheetOptions, Workbook};
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

/// Reads `warm` batches, then the rest under the count. Returns the rows read after the
/// warm-up and the allocations made reading them.
fn steady<E: std::fmt::Debug>(
    warm: usize,
    mut read: impl FnMut() -> Result<Option<usize>, E>,
) -> (usize, usize) {
    for _ in 0..warm {
        read().unwrap().expect("more batches than the warm-up");
    }
    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    let mut rows = 0;
    while let Some(read) = read().unwrap() {
        rows += read;
    }
    (rows, ALLOC_COUNT.load(Ordering::SeqCst) - before)
}

#[test]
fn a_read_allocates_nothing_once_the_reader_is_warm() {
    let mut text = b"id,amount,when,name,note\n".to_vec();
    for row in 0..20_000 {
        text.extend_from_slice(
            format!(
                "{row},{}.{:02},2024-01-{:02}T10:30:00Z,\"n \"\"{row}\"\" x\",{}\n",
                row * 37,
                row % 100,
                1 + row % 28,
                if row % 7 == 0 { "not a number" } else { "12.5" },
            )
            .as_bytes(),
        );
    }
    let plan = [
        Column::i64(0),
        Column::decimal(1),
        Column::timestamp(2),
        Column::text(3),
        Column::f64(4),
        Column::datetime(2, DateOrder::YearMonthDay),
    ];
    let options = DelimitedReader::options().batch_rows(512);

    let mut reader = options.from_slice(&text, Dialect::CSV, &plan).unwrap();
    let (rows, allocations) = steady(4, || {
        reader.read().map(|batch| {
            batch.map(|batch| {
                // Using a batch costs nothing either, short of unescaping a quoted cell.
                let sum: i64 = batch.i64(0).iter().sum();
                let _ = batch.get::<f64>(4, 0);
                assert!(sum >= 0 && batch.text(3, 0).is_ok());
                batch.rows()
            })
        })
    });
    assert!(rows > 17_000, "{rows} rows of a slice");
    assert_eq!(allocations, 0, "a slice");

    let mut reader = options
        .buffer_bytes(4096)
        .from_reader(&text[..], Dialect::CSV, &plan)
        .unwrap();
    let (rows, allocations) = steady(4, || reader.read().map(|b| b.map(|b| b.rows())));
    assert!(rows > 17_000, "{rows} rows of a stream");
    assert_eq!(allocations, 0, "a stream");

    let data = packages::xlsx_sheet(31, 6_000, 6);
    for deflate in [true, false] {
        let package = packages::xlsx(&[("Data", &data)], packages::SHARED, false, deflate);
        let book = Workbook::from_slice(&package).unwrap();
        let plan = [
            Column::text(0),
            Column::f64(1),
            Column::timestamp(2),
            Column::i64(3),
            Column::text(4),
            Column::duration(5),
        ];
        let options = SheetOptions::default().with_batch_rows(512);
        let mut sheet = book.sheet(0, options, &plan).unwrap();
        let (rows, allocations) = steady(3, || sheet.read().map(|b| b.map(|b| b.rows())));
        assert!(rows > 3_000, "{rows} rows of a sheet");
        assert_eq!(allocations, 0, "a sheet, deflated: {deflate}");
    }
}
