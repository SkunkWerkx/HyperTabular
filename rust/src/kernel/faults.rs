//! What went wrong in a batch, as a list: the batch's verdicts and notes read across, for a caller
//! who wants the bad cells and not every cell. It reads what a fill wrote and writes only the
//! caller's list, so it runs after the fill, on the caller's own buffers, when it's asked for.

use crate::kernel::abi::{ColumnBufferWith, ERR_CONTRACT, OK, RawDelimitedOptions, RawProblem};

/// Writes as much of the list as `out` holds, from where `cursor` says, and moves `cursor` past what
/// it looked at. `written` says how many it wrote.
///
/// The list runs by row, then within a row a row-level entry first, then by plan column. It holds:
///
/// - A cell at reason `2` (malformed) or `3` (out of range).
/// - A cell at reason `1` (empty) whose note says [`CellNote::ORIGIN_UNCACHED`]: a workbook's formula
///   with no cached value. A column with no notes can't say so, and its cell reads as a blank.
/// - Under [`RawDelimitedOptions::LONG_ROWS_REPORT`], a row with more than `expected_width` cells.
///
/// So not a blank, not a declared token, not padding, and not a long row cut under
/// [`RawDelimitedOptions::LONG_ROWS_TRUNCATE`], whose widths are the same.
///
/// `cursor` counts `column_count + 1` places a row, so it's `rows * (column_count + 1)` at the end,
/// where a call that writes fewer than `out` holds leaves it. An empty `out` looks at nothing.
///
/// [`ERR_CONTRACT`], with nothing written, for a `cursor` past the end, a `rows` or a column count
/// past `u32::MAX`, a `long_rows` that isn't one of the codes, a null verdicts array, or a report
/// with `row_widths` shorter than `rows`.
///
/// [`CellNote::ORIGIN_UNCACHED`]: crate::kernel::abi::CellNote::ORIGIN_UNCACHED
///
/// # Safety
/// Each column's `verdicts`, and its `notes` unless they're null, hold `rows` elements, at any
/// alignment.
#[allow(clippy::too_many_arguments)]
pub unsafe fn faults(
    columns: &[ColumnBufferWith],
    rows: usize,
    row_widths: &[u32],
    expected_width: u32,
    long_rows: u32,
    cursor: &mut u64,
    out: &mut [RawProblem],
    written: &mut usize,
) -> i32 {
    let (Ok(row_count), Ok(column_count)) = (u32::try_from(rows), u32::try_from(columns.len()))
    else {
        return ERR_CONTRACT;
    };
    let report = match long_rows {
        RawDelimitedOptions::LONG_ROWS_REPORT => {
            if row_widths.len() < rows {
                return ERR_CONTRACT;
            }
            true
        }
        RawDelimitedOptions::LONG_ROWS_REJECT | RawDelimitedOptions::LONG_ROWS_TRUNCATE => false,
        _ => return ERR_CONTRACT,
    };
    // Neither can overflow, each being a `u32` and one more.
    let stride = u64::from(column_count) + 1;
    let end = u64::from(row_count) * stride;
    if *cursor > end || columns.iter().any(|column| column.verdicts.is_null()) {
        return ERR_CONTRACT;
    }

    let mut row = *cursor / stride;
    let mut place = *cursor % stride;
    let mut count = 0;
    while row < u64::from(row_count) {
        while place < stride {
            let Some(entry) = out.get_mut(count) else {
                *cursor = row * stride + place;
                *written = count;
                return OK;
            };
            // The row and the place are in bounds: below `rows`, and at most the column count.
            let found = if place == 0 {
                if report {
                    long_row(row as u32, row_widths, expected_width)
                } else {
                    None
                }
            } else {
                // SAFETY: per the function contract, and `row` is below `rows`.
                columns
                    .get((place - 1) as usize)
                    .and_then(|column| unsafe { cell(column, row as u32, (place - 1) as u32) })
            };
            place += 1;
            if let Some(problem) = found {
                *entry = problem;
                count += 1;
            }
        }
        place = 0;
        row += 1;
    }
    *cursor = end;
    *written = count;
    OK
}

/// The row-level entry of row `row`, if it had more cells than the width.
fn long_row(row: u32, row_widths: &[u32], expected_width: u32) -> Option<RawProblem> {
    let found = row_widths.get(row as usize).copied()?;
    (found > expected_width).then_some(RawProblem {
        row,
        plan_column: RawProblem::ROW_LEVEL,
        expected: expected_width,
        found,
        ..RawProblem::default()
    })
}

/// The entry of one cell, if it's one the list holds.
///
/// # Safety
/// As [`faults`], and `row` is below `rows`.
unsafe fn cell(column: &ColumnBufferWith, row: u32, plan_column: u32) -> Option<RawProblem> {
    use crate::kernel::abi::{CellNote, CellVerdict};
    use hypercast::Reason;
    const EMPTY: u32 = Reason::Empty as u32;
    const MALFORMED: u32 = Reason::Malformed as u32;
    const OUT_OF_RANGE: u32 = Reason::OutOfRange as u32;

    let at = row as usize;
    // SAFETY: per the function contract.
    let verdict: CellVerdict = unsafe { column.verdicts.add(at).read_unaligned() };
    if !matches!(verdict.reason, EMPTY | MALFORMED | OUT_OF_RANGE) {
        return None;
    }
    let note = if column.notes.is_null() {
        CellNote::default()
    } else {
        // SAFETY: per the function contract.
        unsafe { column.notes.add(at).read_unaligned() }
    };
    if verdict.reason == EMPTY && note.origin != CellNote::ORIGIN_UNCACHED {
        return None;
    }
    Some(RawProblem {
        row,
        plan_column,
        reason: verdict.reason,
        detail: note.detail,
        origin: note.origin,
        offset: verdict.offset,
        len: verdict.len,
        expected: 0,
        found: 0,
    })
}
