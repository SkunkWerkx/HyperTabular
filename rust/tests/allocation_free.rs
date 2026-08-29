//! The cast engine's allocation story, asserted by a counting allocator rather than a
//! doc comment (HyperCast's own proof, re-applied): every door on every typed cell shape
//! — success and failure — allocates nothing, and a warm batch fill allocates nothing.
//! The two documented allocating paths are exercised and named: the text door's
//! rendering of a *typed* cell (an owned `Cow`), and the first, cold fill of a batch.
//!
//! One `#[test]` function on purpose: the counter is process-wide and `cargo test`
//! spawns a thread per test function.

use hypertabular::{
    Batch, Cell, CellError, Column, Date, DateSystem, Door, Duration, Header, NumFormat, Plan, Row,
    TabularSource, UnixPrecision, cast_bool, cast_date, cast_duration, cast_f64, cast_i32,
    cast_text, cast_time, cast_timestamp, cast_u64, cast_unix, cast_uuid, fill_batch,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::convert::Infallible;
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

#[track_caller]
fn assert_allocation_free<T>(door: &str, f: impl Fn() -> T) {
    for _ in 0..1000 {
        let allocs = allocs_during(&f);
        assert_eq!(allocs, 0, "{door} allocated {allocs} time(s) in one call");
    }
}

struct Cycle<'c> {
    rows: Vec<Vec<Cell<'c>>>,
    next: usize,
    remaining: usize,
}

impl<'c> TabularSource for Cycle<'c> {
    type Row<'a>
        = &'a [Cell<'c>]
    where
        Self: 'a;
    type Error = Infallible;

    fn header(&self) -> Option<&Header> {
        None
    }

    fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Infallible> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let row = &self.rows[self.next % self.rows.len()];
        self.next += 1;
        Ok(Some(row.as_slice()))
    }
}

#[test]
fn allocation_free() {
    let format = NumFormat::INVARIANT;
    let wall = Cell::Wall {
        date: Date {
            year: 2026,
            month: 1,
            day: 2,
        },
        nanos: 54_245_123_000_000,
    };
    let span = Cell::Span(Duration {
        seconds: 5_400,
        nanos: 0,
    });

    // Text cells: the HyperCast doors, through the matrix.
    assert_allocation_free("text i32", || {
        cast_i32(&Cell::Text(b"(1,234)"), &format).unwrap()
    });
    assert_allocation_free("text f64", || {
        cast_f64(&Cell::Text(b"1,234.5e-3"), &format).unwrap()
    });
    assert_allocation_free("text timestamp", || {
        cast_timestamp(
            &Cell::Text(b"2026-01-02T15:04:05.123456789+05:00"),
            DateSystem::Excel1900,
        )
        .unwrap()
    });
    assert_allocation_free("text uuid", || {
        cast_uuid(&Cell::Text(b"01020304-0506-0708-090a-0b0c0d0e0f10")).unwrap()
    });
    assert_allocation_free("text door", || cast_text(&Cell::Text(b"borrowed")).unwrap());
    assert_allocation_free("text failure", || {
        cast_i32(&Cell::Text(b"12x4"), &format).unwrap_err()
    });

    // Typed cells: direct conversion.
    assert_allocation_free("number i32", || {
        cast_i32(&Cell::Number(42.0), &format).unwrap()
    });
    assert_allocation_free("number u64", || {
        cast_u64(&Cell::Number(1e15), &format).unwrap()
    });
    assert_allocation_free("number date", || {
        cast_date(&Cell::Number(45_000.0), DateSystem::Excel1900).unwrap()
    });
    assert_allocation_free("number timestamp", || {
        cast_timestamp(&Cell::Number(45_000.5), DateSystem::Excel1904).unwrap()
    });
    assert_allocation_free("number unix", || {
        cast_unix(&Cell::Number(1_700_000_000.0), UnixPrecision::Seconds).unwrap()
    });
    assert_allocation_free("number time", || cast_time(&Cell::Number(0.75)).unwrap());
    assert_allocation_free("number duration", || {
        cast_duration(&Cell::Number(1.5)).unwrap()
    });
    assert_allocation_free("bool", || cast_bool(&Cell::Bool(true)).unwrap());
    assert_allocation_free("wall timestamp", || {
        cast_timestamp(&wall, DateSystem::Excel1900).unwrap()
    });
    assert_allocation_free("span time", || cast_time(&span).unwrap());
    assert_allocation_free("number failure", || {
        cast_i32(&Cell::Number(1.5), &format).unwrap_err()
    });
    assert_allocation_free("wall failure", || cast_i32(&wall, &format).unwrap_err());
    assert_allocation_free("error failure", || {
        cast_f64(&Cell::Error(CellError::NotAvailable), &format).unwrap_err()
    });

    // The documented exception: rendering a typed cell for the text door owns its bytes.
    assert!(allocs_during(|| cast_text(&Cell::Number(2.5)).unwrap()) > 0);

    // A batch fill: cold once, then warm forever.
    let mut source = Cycle {
        rows: vec![
            vec![
                Cell::Text(b"1"),
                Cell::Text(b"2.5"),
                Cell::Text(b"true"),
                Cell::Number(45_000.0),
            ],
            vec![Cell::Text(b"x"), Cell::Number(7.0), Cell::Bool(false), wall],
            vec![Cell::Empty, Cell::Empty, Cell::Empty, Cell::Empty],
        ],
        next: 0,
        remaining: usize::MAX,
    };
    let plan = Plan::new(vec![
        Column::new(0, Door::I32),
        Column::new(1, Door::F64),
        Column::new(2, Door::Bool),
        Column::new(3, Door::Date),
    ]);
    let mut batch = Batch::new();
    let cold = allocs_during(|| fill_batch(&mut source, &plan, &mut batch, 512).unwrap());
    assert!(cold > 0, "the first fill sizes the batch");
    // The fault table and arena grow on the failure path too; warm them with the same
    // row mix once more, then demand zero.
    fill_batch(&mut source, &plan, &mut batch, 512).unwrap();
    for _ in 0..100 {
        let warm = allocs_during(|| fill_batch(&mut source, &plan, &mut batch, 512).unwrap());
        assert_eq!(warm, 0, "a warm fill allocated {warm} time(s)");
        assert_eq!(batch.rows(), 512);
    }

    // Row access through the trait is free as well.
    let row: &[Cell<'_>] = &[Cell::Text(b"a"), Cell::Number(1.0)];
    assert_allocation_free("row cell", || row.cell(1));
}
