//! How much a batch may ask for: the core's one rule. The boundary is held on the pure function,
//! which allocates nothing; the readers are held to asking it first, with sizes past any address
//! space, so that a regression can't reach for real memory.

use hypertabular::kernel::abi::{ColumnSpecWith, ERR_BATCH, ERR_CONTRACT, OK, RawBatchCheck, Span};
use hypertabular::kernel::door::Door;
use hypertabular::kernel::exports::hypertabular_batch_check;
use hypertabular::kernel::size::{
    BATCH_BYTES_LIMIT, CELL_BUDGET, MAX_ROW_BYTES, SAMPLE_BATCH_BYTES, check_batch,
};
use hypertabular::{Column, DelimitedOptions, DelimitedReader, Dialect, Error, LongRows};
use hypertabular::{SheetOptions, Workbook};
use std::io::Cursor;

const NONE: u32 = 0;
const NOTES: u32 = RawBatchCheck::NOTES;
const WIDTHS: u32 = RawBatchCheck::WIDTHS;
const SHEET: u32 = RawBatchCheck::SHEET;
const BUFFER: u64 = 256 * 1024;

fn spec(door: Door) -> ColumnSpecWith {
    let param = match door {
        Door::Unix(_) | Door::DateOrdered(_) | Door::DateTime(_) | Door::ExcelSerial(_) => 1,
        _ => 0,
    };
    ColumnSpecWith::new(0, door.code(), param)
}

fn check(specs: &[ColumnSpecWith], rows: u64, buffer: u64, flags: u32) -> (i32, RawBatchCheck) {
    let mut out = RawBatchCheck::default();
    let code = check_batch(specs, rows, buffer, flags, &mut out);
    (code, out)
}

/// The bytes a read of `specs` asks for at `rows`, whatever the limit says of them.
fn needed(specs: &[ColumnSpecWith], rows: u64, flags: u32) -> u64 {
    check(specs, rows, BUFFER, flags).1.needed
}

#[test]
fn exactly_at_the_limit_passes_and_one_byte_over_refuses() {
    // A delimited i32 with notes: 20 bytes a row, and a cell table of budget / 2 rows of 2 spans,
    // 16 bytes a row of it. 20 * 214,327,296 + 16 * 526,336 is exactly 2^32.
    let plan = [spec(Door::I32)];
    let at = 214_327_296;
    assert_eq!(needed(&plan, at, NOTES), BATCH_BYTES_LIMIT);
    let (code, out) = check(&plan, at, BUFFER, NOTES);
    assert_eq!((code, out.refused), (OK, RawBatchCheck::REFUSED_NONE));
    assert_eq!(out.limit, BATCH_BYTES_LIMIT);

    let (code, out) = check(&plan, at + 1, BUFFER, NOTES);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_OVER_LIMIT)
    );
    assert_eq!(out.needed, BATCH_BYTES_LIMIT + 20);
    assert_eq!(out.limit, BATCH_BYTES_LIMIT);

    // One byte past, not one row: a plan of bools and a table whose budget is spent.
    let plan = [spec(Door::Bool)];
    let at = (BATCH_BYTES_LIMIT - 16 * (CELL_BUDGET / 2)) / 13;
    let left = BATCH_BYTES_LIMIT - needed(&plan, at, NONE);
    assert!(left < 13);
    assert_eq!(check(&plan, at, BUFFER, NONE).0, OK);
    assert_eq!(check(&plan, at + 1, BUFFER, NONE).0, ERR_BATCH);
}

#[test]
fn a_sheet_at_the_limit() {
    // An i32 with no notes: 16 bytes a row, 2 spans of table a row, the whole batch of them, and
    // one 16-byte slot. The largest batch leaves 16 bytes over, so a row past it is refused.
    let plan = [spec(Door::I32)];
    let rows = (1 << 27) - 1;
    assert_eq!(needed(&plan, rows, SHEET), BATCH_BYTES_LIMIT - 16);
    assert_eq!(check(&plan, rows, 0, SHEET).0, OK);
    assert_eq!(check(&plan, rows + 1, 0, SHEET).0, ERR_BATCH);
    // Exactly at it: no plan has no slots, and a table of 2^29 rows of one span.
    assert_eq!(needed(&[], 1 << 29, SHEET), BATCH_BYTES_LIMIT);
    assert_eq!(check(&[], 1 << 29, 0, SHEET).0, OK);
    assert_eq!(check(&[], (1 << 29) + 1, 0, SHEET).0, ERR_BATCH);
}

#[test]
fn overflow_refuses() {
    let plan = [spec(Door::Decimal)];
    for rows in [1 << 62, u64::MAX, u64::MAX / 2 + 1] {
        for flags in [NONE, NOTES, WIDTHS, SHEET, SHEET | NOTES] {
            let (code, out) = check(&plan, rows, BUFFER, flags);
            assert_eq!(
                (code, out.refused),
                (ERR_BATCH, RawBatchCheck::REFUSED_OVER_LIMIT)
            );
            assert_eq!((out.needed, out.limit), (u64::MAX, BATCH_BYTES_LIMIT));
        }
    }
    // Each product on its own: the values, the sheet's table and the widths.
    assert_eq!(needed(&plan, u64::MAX / 16, NONE), u64::MAX);
    assert_eq!(needed(&[], u64::MAX / 4, SHEET), u64::MAX);
    assert_eq!(needed(&[], u64::MAX / 2, WIDTHS), u64::MAX);
    // The table's bytes alone fit where its entries times their size doesn't.
    assert_eq!(
        needed(&[], u64::MAX / Span::SIZE as u64 + 1, SHEET),
        u64::MAX
    );
    // The sum: values and table each fit, and together don't.
    let rows = (u64::MAX / 20) & !3;
    assert_eq!(needed(&[spec(Door::I64)], rows, SHEET), u64::MAX);
}

#[test]
fn every_door_is_counted_at_its_size() {
    // Delimited, one row: the value, 12 of verdict and 2 spans of table.
    for (door, size) in [
        (Door::Bool, 1),
        (Door::I8, 1),
        (Door::U8, 1),
        (Door::I16, 2),
        (Door::U16, 2),
        (Door::I32, 4),
        (Door::U32, 4),
        (Door::F32, 4),
        (Door::I64, 8),
        (Door::U64, 8),
        (Door::F64, 8),
        (Door::Time, 8),
        (Door::Uuid, 16),
        (Door::Text, 8),
        (Door::Timestamp, 16),
        (Door::Duration, 16),
        (Door::Decimal, 16),
        (Door::Date, 4),
        (Door::Unix(hypertabular::UnixPrecision::Seconds), 16),
        (Door::ExcelSerial(hypertabular::ExcelEpoch::Y1900), 16),
        (Door::DateOrdered(hypertabular::DateOrder::YearMonthDay), 4),
        (
            Door::DateTime(hypertabular::DateOrder::YearMonthDay),
            size_of::<hypertabular::CivilDateTime>() as u64,
        ),
    ] {
        assert_eq!(needed(&[spec(door)], 1, NONE), size + 12 + 16, "{door:?}");
    }
}

#[test]
fn each_optional_array_is_counted() {
    let plan = [spec(Door::I32)];
    let rows = 1000;
    let table = 2 * rows * Span::SIZE as u64;
    assert_eq!(needed(&plan, rows, NONE), (4 + 12) * rows + table);
    assert_eq!(needed(&plan, rows, NOTES), (4 + 12 + 4) * rows + table);
    assert_eq!(needed(&plan, rows, WIDTHS), (4 + 12 + 4) * rows + table);
    assert_eq!(
        needed(&plan, rows, NOTES | WIDTHS),
        (4 + 12 + 4 + 4) * rows + table
    );
    // Every column's, not the first's.
    let two = [spec(Door::I32), spec(Door::I64)];
    assert_eq!(needed(&two, rows, NOTES), (20 + 24) * rows + table);
}

#[test]
fn a_constrained_columns_checks_are_counted() {
    // A column that declares constraints is given a check a row beside its verdict, 12 bytes;
    // one that declares none, nothing.
    let declared = hypercast::Constraints::<i32>::numeric(hypercast::NumericRules::new(), None)
        .expect("no rules")
        .raw();
    let mut checked = spec(Door::I32);
    checked.constraints = &declared;
    let rows = 1000;
    let table = 2 * rows * Span::SIZE as u64;
    assert_eq!(needed(&[checked], rows, NONE), (4 + 12 + 12) * rows + table);
    assert_eq!(
        needed(&[checked], rows, NOTES),
        (4 + 12 + 4 + 12) * rows + table
    );
    assert_eq!(
        needed(&[spec(Door::I32)], rows, NOTES),
        (4 + 12 + 4) * rows + table
    );
}

#[test]
fn the_cell_tables() {
    // Delimited: indexed by source column, the widest ordinal plus two a row, up to the budget.
    let mut far = spec(Door::Bool);
    far.ordinal = 9;
    assert_eq!(needed(&[far], 10, NONE), 13 * 10 + 11 * 10 * 8);
    let plan = [spec(Door::Bool), far];
    assert_eq!(needed(&plan, 10, NONE), 26 * 10 + 11 * 10 * 8);
    let rows = 1 << 21;
    let held = CELL_BUDGET / 11;
    assert_eq!(needed(&[far], rows, NONE), 13 * rows + 11 * held * 8);
    // One row at least, however far a plan reaches.
    far.ordinal = (1 << 20) - 1;
    assert_eq!(needed(&[far], 10, NONE), 13 * 10 + ((1 << 20) + 1) * 8);

    // A sheet: indexed by plan column, a whole batch of rows, wherever the ordinals are.
    // Beside it a slot for each source column the plan reaches, once.
    let slots = (1 << 20) * 16;
    assert_eq!(needed(&[far], 10, SHEET), 13 * 10 + 2 * 10 * 8 + slots);
    assert_eq!(needed(&plan, 10, SHEET), 26 * 10 + 3 * 10 * 8 + 10 * 16);
    assert_eq!(
        needed(&[spec(Door::Bool)], 10, SHEET),
        13 * 10 + 2 * 10 * 8 + 16
    );
}

#[test]
fn one_row_past_the_limit_is_refused_as_its_width() {
    let mut far = spec(Door::I32);
    far.ordinal = u32::MAX;
    // A sheet's slots: 2^32 of them at 16 bytes each, at one row.
    let (code, out) = check(&[far], 1, 0, SHEET);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_ROW_TOO_WIDE)
    );
    assert_eq!(out.width, 1 << 32);
    assert_eq!(out.needed, 16 + 2 * 8 + (1 << 36));
    assert_eq!(out.sample_rows, 0);
    // A sheet's width is what drives it: the plan's, where the plan is the longer.
    assert_eq!(check(&[spec(Door::I32); 3], 1, 0, SHEET).1.width, 3);
    let mut second = spec(Door::I32);
    second.ordinal = 7;
    assert_eq!(check(&[second, spec(Door::I32)], 1, 0, SHEET).1.width, 8);
    // A zero is refused ahead of the width, as ahead of every other reason.
    assert_eq!(
        check(&[far], 0, 0, SHEET).1.refused,
        RawBatchCheck::REFUSED_BATCH_ROWS_ZERO
    );
    assert_eq!(
        check(&[far], 1, 0, NONE).1.refused,
        RawBatchCheck::REFUSED_BUFFER_BYTES_ZERO
    );
    // A delimited table: 2^32 + 1 spans. The width is what the plan reaches.
    let (code, out) = check(&[far], 1 << 40, BUFFER, NONE);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_ROW_TOO_WIDE)
    );
    assert_eq!(out.width, 1 << 32);
    assert_eq!(out.needed, 16 + ((1 << 32) + 1) * 8);
    assert_eq!(out.sample_rows, 0);
    // Exactly one row at the limit isn't too wide; one column of reach less is 8 bytes under.
    far.ordinal = (1 << 29) - 3;
    let (code, out) = check(&[far], 1, BUFFER, NONE);
    assert_eq!(out.needed, 16 + ((1 << 29) - 1) * 8);
    assert_eq!(out.needed, BATCH_BYTES_LIMIT + 8);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_ROW_TOO_WIDE)
    );
    far.ordinal -= 1;
    let (code, out) = check(&[far], 1, BUFFER, NONE);
    assert_eq!((code, out.needed), (OK, BATCH_BYTES_LIMIT));
    assert_eq!(out.sample_rows, 1, "one row past the budget still reads");
    // A zero is its own error, ahead of the width.
    let (_, out) = check(&[far], 0, BUFFER, NONE);
    assert_eq!(
        (out.refused, out.needed, out.sample_rows),
        (RawBatchCheck::REFUSED_BATCH_ROWS_ZERO, 0, 0)
    );
}

#[test]
fn a_sampler_is_told_the_most_rows_within_its_budget() {
    // What the docs say it is.
    assert_eq!(SAMPLE_BATCH_BYTES, 64 * 1024 * 1024);
    let plan: Vec<_> = (0..700)
        .map(|ordinal| ColumnSpecWith::new(ordinal, Door::Text.code(), 0))
        .collect();
    for (flags, wanted) in [
        (NOTES, 100_001),
        (NONE, 100_001),
        (SHEET | NOTES, 100_001),
        (SHEET, 100_001),
        (NOTES, u64::MAX),
        (SHEET, u64::MAX),
        (WIDTHS, 1 << 40),
    ] {
        let (_, out) = check(&plan, wanted, BUFFER, flags);
        let rows = out.sample_rows;
        assert!(rows >= 1 && rows < wanted, "{flags} {wanted}");
        assert!(
            needed(&plan, rows, flags) <= SAMPLE_BATCH_BYTES,
            "{flags} {wanted}"
        );
        assert!(
            needed(&plan, rows + 1, flags) > SAMPLE_BATCH_BYTES,
            "{flags} {wanted}"
        );
    }
    // Within the budget, the rows wanted, whichever the read's own answer is.
    let (code, out) = check(&plan[..1], 1001, BUFFER, NOTES);
    assert_eq!((code, out.sample_rows), (OK, 1001));
    // At exactly the budget, and one row past it. No plan on a sheet is 8 bytes a row.
    let at = SAMPLE_BATCH_BYTES / 8;
    assert_eq!(needed(&[], at, SHEET), SAMPLE_BATCH_BYTES);
    assert_eq!(check(&[], at, 0, SHEET).1.sample_rows, at);
    assert_eq!(check(&[], at + 1, 0, SHEET).1.sample_rows, at);
    assert_eq!(check(&[], 1 << 40, 0, SHEET).1.sample_rows, at);
    // No arrays and a table held by its budget: any delimited count is within.
    assert_eq!(check(&[], u64::MAX, BUFFER, NONE).1.sample_rows, u64::MAX);
    // A sampler's rows are never refused by the check that named them.
    let (_, out) = check(&plan, 1 << 40, BUFFER, NOTES);
    assert_eq!(check(&plan, out.sample_rows, BUFFER, NOTES).0, OK);
    // One row wide past the budget and within the limit: one row at a time.
    let mut far = spec(Door::Bool);
    far.ordinal = 1 << 24;
    let (code, out) = check(&[far], 10, BUFFER, NONE);
    assert_eq!((code, out.sample_rows), (OK, 1));
}

#[test]
fn the_row_too_wide_error_names_its_width_and_the_limit() {
    let error = Error::RowTooWide {
        width: 700,
        needed: 5_000_000_000,
        limit: BATCH_BYTES_LIMIT,
    };
    assert_eq!(
        error.to_string(),
        "a row of 700 columns asks for 5000000000 bytes, past the limit of 4294967296"
    );
}

/// A 32-bit target can't count or hold what a 64-bit one can. It's refused or short of memory,
/// without a panic in the arithmetic on the way there.
#[cfg(target_pointer_width = "32")]
#[test]
fn a_late_bind_past_the_address_space_says_no_memory() {
    let book = book();
    let late = SheetOptions::new().with_batch_rows(1 << 29);
    let mut sheet = book.sheet_unbound(0usize, late).unwrap();
    assert!(matches!(sheet.bind(&[]), Err(Error::NoMemory { .. })));
}

#[test]
fn no_plan_reads_at_any_batch_size_delimited_and_not_on_a_sheet() {
    // No arrays, and a table stopped by its budget, so delimited passes at any count.
    for rows in [1, 1 << 40, 1 << 62, u64::MAX] {
        let (code, out) = check(&[], rows, BUFFER, NOTES);
        assert_eq!(code, OK, "{rows}");
        assert_eq!(out.needed, CELL_BUDGET.min(rows) * Span::SIZE as u64);
    }
    // But a read reporting long rows holds a width for each row.
    assert_eq!(check(&[], 1 << 30, BUFFER, WIDTHS).0, ERR_BATCH);
    assert_eq!(
        needed(&[], 1 << 30, WIDTHS),
        BATCH_BYTES_LIMIT + CELL_BUDGET * Span::SIZE as u64
    );
    // A sheet's table is a batch of rows with no plan as well: 2^29 is exactly the limit.
    assert_eq!(needed(&[], 1 << 29, SHEET), BATCH_BYTES_LIMIT);
    assert_eq!(check(&[], 1 << 29, 0, SHEET).0, OK);
    assert_eq!(check(&[], (1 << 29) + 1, 0, SHEET).0, ERR_BATCH);
}

#[test]
fn zero_is_refused_batch_rows_first() {
    let plan = [spec(Door::I32)];
    let (code, out) = check(&plan, 0, BUFFER, NOTES);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_BATCH_ROWS_ZERO)
    );
    assert_eq!((out.needed, out.limit), (0, BATCH_BYTES_LIMIT));
    let (code, out) = check(&plan, 1, 0, NOTES);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_BUFFER_BYTES_ZERO)
    );
    let (code, out) = check(&plan, 0, 0, NOTES);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_BATCH_ROWS_ZERO)
    );
    // Ahead of a size too big as well.
    let (code, out) = check(&plan, 1 << 62, 0, NOTES);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_BUFFER_BYTES_ZERO)
    );
    // A sheet has no input buffer to be zero, and a zero batch all the same.
    assert_eq!(check(&plan, 1, 0, SHEET).0, OK);
    let (code, out) = check(&plan, 0, 0, SHEET);
    assert_eq!(
        (code, out.refused),
        (ERR_BATCH, RawBatchCheck::REFUSED_BATCH_ROWS_ZERO)
    );
}

#[test]
fn the_buffer_starts_at_what_was_asked_up_to_the_ceiling() {
    assert_eq!(check(&[], 1, 1, NONE).1.buffer_bytes, 1);
    assert_eq!(check(&[], 1, BUFFER, NONE).1.buffer_bytes, BUFFER);
    assert_eq!(
        check(&[], 1, MAX_ROW_BYTES - 1, NONE).1.buffer_bytes,
        MAX_ROW_BYTES - 1
    );
    assert_eq!(
        check(&[], 1, MAX_ROW_BYTES, NONE).1.buffer_bytes,
        MAX_ROW_BYTES
    );
    assert_eq!(
        check(&[], 1, MAX_ROW_BYTES + 1, NONE).1.buffer_bytes,
        MAX_ROW_BYTES
    );
    let (code, out) = check(&[], 1, 1 << 50, NONE);
    assert_eq!((code, out.buffer_bytes), (OK, MAX_ROW_BYTES));
    assert_eq!(check(&[], 1, u64::MAX, NONE).1.buffer_bytes, MAX_ROW_BYTES);
    assert_eq!(check(&[], 1, BUFFER, SHEET).1.buffer_bytes, 0);
}

#[test]
fn the_export_answers_as_the_function_does() {
    let plan = [spec(Door::I32)];
    let mut out = RawBatchCheck::default();
    // SAFETY: the plan and `out` are live, as the export's contract asks, and the nulls are what
    // the next calls test.
    unsafe {
        assert_eq!(
            hypertabular_batch_check(plan.as_ptr(), 1, 10, BUFFER, NOTES, &mut out),
            OK
        );
        assert_eq!(out, check(&plan, 10, BUFFER, NOTES).1);
        let code = hypertabular_batch_check(plan.as_ptr(), 1, 1 << 40, BUFFER, NOTES, &mut out);
        assert_eq!(
            (code, out.refused),
            (ERR_BATCH, RawBatchCheck::REFUSED_OVER_LIMIT)
        );
        assert_eq!(
            hypertabular_batch_check(std::ptr::null(), 0, 10, BUFFER, 0, &mut out),
            OK
        );

        // Each refusal of the contract writes nothing.
        let before = RawBatchCheck {
            needed: 7,
            ..RawBatchCheck::default()
        };
        out = before;
        let mut bad_size = [spec(Door::I32)];
        bad_size[0].size -= 1;
        assert_eq!(
            hypertabular_batch_check(bad_size.as_ptr(), 1, 10, BUFFER, 0, &mut out),
            ERR_CONTRACT
        );
        assert_eq!(out, before);
        let bad_door = [ColumnSpecWith::new(0, 99, 0)];
        let bad_param = [ColumnSpecWith::new(
            0,
            Door::Unix(hypertabular::UnixPrecision::Seconds).code(),
            0,
        )];
        let mut bad_size = spec(Door::I32);
        bad_size.size -= 1;
        for (specs, count, flags) in [
            (std::ptr::null(), 1, 0),
            (bad_door.as_ptr(), 1, 0),
            (bad_param.as_ptr(), 1, 0),
            (&raw const bad_size, 1, 0),
            (plan.as_ptr(), 1, 8),
        ] {
            assert_eq!(
                hypertabular_batch_check(specs, count, 10, BUFFER, flags, &mut out),
                ERR_CONTRACT
            );
            assert_eq!(out, before);
        }
        // The function holds to the same answer without the export ahead of it.
        assert_eq!(
            check(std::slice::from_ref(&bad_size), 10, BUFFER, NONE).0,
            ERR_CONTRACT
        );
        let misaligned = plan
            .as_ptr()
            .cast::<u8>()
            .wrapping_add(1)
            .cast::<ColumnSpecWith>();
        assert_eq!(
            hypertabular_batch_check(misaligned, 1, 10, BUFFER, 0, &mut out),
            ERR_CONTRACT
        );
        assert_eq!(
            hypertabular_batch_check(plan.as_ptr(), 1, 10, BUFFER, 0, std::ptr::null_mut()),
            ERR_CONTRACT
        );
    }
}

/// The sizes refused below are past any address space, so a reader that stopped asking fails on
/// the error's kind rather than reaching for memory.
const HUGE: usize = 1 << (usize::BITS - 2);

fn refused_too_large<T>(result: Result<T, Error>) {
    match result {
        Err(Error::BatchTooLarge { needed, limit }) => {
            assert!(needed > limit);
            assert_eq!(limit, BATCH_BYTES_LIMIT);
        }
        Err(other) => panic!("refused as {other}"),
        Ok(_) => panic!("not refused"),
    }
}

fn refused_zero_rows<T>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::ZeroBatchRows)),
        "not refused as zero rows"
    );
}

fn refused_zero_buffer<T>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::ZeroBufferBytes)),
        "not refused as a zero buffer"
    );
}

const TEXT: &[u8] = b"a,b\n1,2\n3,4\n";

fn csv() -> DelimitedOptions {
    DelimitedReader::options()
}

#[test]
fn every_delimited_entry_refuses_a_zero_batch() {
    let plan = [Column::i32(0)];
    let path = support_file();
    let options = csv().batch_rows(0);
    refused_zero_rows(options.from_slice(TEXT, Dialect::CSV, &plan));
    refused_zero_rows(options.from_reader(Cursor::new(TEXT), Dialect::CSV, &plan));
    refused_zero_rows(options.open(&path, Dialect::CSV, &plan));
    refused_zero_rows(options.from_slice_unbound(TEXT, Dialect::CSV));
    refused_zero_rows(options.from_reader_unbound(Cursor::new(TEXT), Dialect::CSV));
    refused_zero_rows(options.open_unbound(&path, Dialect::CSV));
    // Ahead of the file: a size is judged before anything is opened.
    refused_zero_rows(options.open("no such file", Dialect::CSV, &plan));
    refused_zero_rows(options.open_unbound("no such file", Dialect::CSV));
}

#[test]
fn every_delimited_entry_refuses_a_zero_buffer() {
    let plan = [Column::i32(0)];
    let path = support_file();
    let options = csv().buffer_bytes(0);
    refused_zero_buffer(options.from_slice(TEXT, Dialect::CSV, &plan));
    refused_zero_buffer(options.from_reader(Cursor::new(TEXT), Dialect::CSV, &plan));
    refused_zero_buffer(options.open(&path, Dialect::CSV, &plan));
    refused_zero_buffer(options.from_slice_unbound(TEXT, Dialect::CSV));
    refused_zero_buffer(options.from_reader_unbound(Cursor::new(TEXT), Dialect::CSV));
    refused_zero_buffer(options.open_unbound(&path, Dialect::CSV));
}

#[test]
fn every_delimited_entry_refuses_a_batch_past_the_limit() {
    let plan = [Column::i32(0)];
    let path = support_file();
    let options = csv().batch_rows(HUGE);
    refused_too_large(options.from_slice(TEXT, Dialect::CSV, &plan));
    refused_too_large(options.from_reader(Cursor::new(TEXT), Dialect::CSV, &plan));
    refused_too_large(options.open(&path, Dialect::CSV, &plan));
    // With no plan it has asked for nothing yet, and a plan is judged when it's bound.
    let mut reader = options.from_slice_unbound(TEXT, Dialect::CSV).unwrap();
    refused_too_large(reader.bind(&plan));
    assert!(!reader.is_bound());
    // Bound later, a refusal leaves the reader to take a plan it can hold: none of any size.
    reader.bind(&[]).unwrap();
    assert!(reader.is_bound());
}

#[test]
fn long_rows_report_counts_the_widths() {
    let dialect = Dialect::CSV.with_long_rows(LongRows::Report);
    // No plan and no widths reads at this size, with a width for each row it's refused.
    assert!(
        csv()
            .batch_rows(HUGE)
            .from_slice(TEXT, Dialect::CSV, &[])
            .is_ok()
    );
    refused_too_large(csv().batch_rows(HUGE).from_slice(TEXT, dialect, &[]));
    refused_too_large(csv().batch_rows(HUGE).from_slice_unbound(TEXT, dialect));
}

#[test]
fn a_plan_that_cannot_be_honoured_is_still_the_plans_error() {
    let plan = [Column::i32(1 << 20)];
    assert!(matches!(
        csv().from_slice(TEXT, Dialect::CSV, &plan),
        Err(Error::Plan { .. })
    ));
}

#[test]
fn a_size_past_the_ceiling_starts_there() {
    // A slice has no buffer to start. A stream's is reserved at the ceiling, not touched until a
    // read, so with no header to read this costs no pages; one that didn't take the core's size
    // would fail to reserve this one.
    let plan = [Column::i32(0)];
    let mut reader = csv()
        .buffer_bytes(usize::MAX / 2)
        .from_slice(TEXT, Dialect::CSV, &plan)
        .unwrap();
    assert_eq!(reader.read().unwrap().unwrap().rows(), 2);
    let headerless = Dialect::CSV.with_header(false);
    assert!(
        csv()
            .buffer_bytes(usize::MAX)
            .from_reader(Cursor::new(TEXT), headerless, &plan)
            .is_ok()
    );
}

#[test]
fn a_modest_batch_reads_and_defaults_are_not_refused() {
    let plan = [Column::i32(0), Column::text(1)];
    let mut reader = csv().from_slice(TEXT, Dialect::CSV, &plan).unwrap();
    assert_eq!(reader.read().unwrap().unwrap().rows(), 2);
    let mut reader = csv()
        .batch_rows(1)
        .from_slice(TEXT, Dialect::CSV, &plan)
        .unwrap();
    assert_eq!(reader.read().unwrap().unwrap().rows(), 1);
    let mut reader = csv()
        .batch_rows(1 << 16)
        .from_slice(TEXT, Dialect::CSV, &plan)
        .unwrap();
    assert_eq!(reader.read().unwrap().unwrap().rows(), 2);
}

fn book() -> Workbook<'static> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook/");
    let name = std::fs::read_dir(path)
        .unwrap()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "xlsx")
        })
        .unwrap();
    Workbook::open(name).unwrap()
}

#[test]
fn every_sheet_entry_refuses() {
    let book = book();
    let plan = [Column::text(0)];
    let zero = SheetOptions::new().with_batch_rows(0);
    refused_zero_rows(book.sheet(0usize, zero, &plan));
    refused_zero_rows(book.sheet_unbound(0usize, zero));
    let huge = SheetOptions::new().with_batch_rows(HUGE);
    refused_too_large(book.sheet(0usize, huge, &plan));
    // A sheet has a limit with no plan as well: its table is a batch of rows.
    refused_too_large(book.sheet_unbound(0usize, huge));
    let late = SheetOptions::new().with_batch_rows(1 << 29);
    let mut sheet = book.sheet_unbound(0usize, late).unwrap();
    refused_too_large(sheet.bind(&plan));
    assert!(!sheet.is_bound());
}

#[test]
fn a_modest_sheet_reads() {
    let book = book();
    let plan = [Column::text(0)];
    let options = SheetOptions::new().with_header(false).with_batch_rows(16);
    let mut sheet = book.sheet(0usize, options, &plan).unwrap();
    assert!(sheet.read().unwrap().is_some());
}

/// A file holding `TEXT`, for the entries that open one.
fn support_file() -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("batch_size.csv");
    std::fs::write(&path, TEXT).unwrap();
    path
}
