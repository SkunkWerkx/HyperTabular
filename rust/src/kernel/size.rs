//! How much memory a batch may ask for. The core owns the rule, so that it changes in one place:
//! every binding asks [`check_batch`] (through `hypertabular_batch_check`, or directly) before it
//! allocates anything, and none holds a size rule of its own.
//!
//! A batch's bytes are what a read holds for `batch_rows` rows of a plan: each column's values and
//! verdicts, its notes when they're on, each row's width when a read reports long rows, and the
//! cell table. They're counted with checked arithmetic and nothing is allocated, so asking about
//! any size is safe.
//!
//! A plan of no columns has no value arrays. A delimited read's cell table stops at
//! [`CELL_BUDGET`] entries or one row, so with no plan and no row widths it reads at any
//! `batch_rows`. A sheet's cell table is `batch_rows` rows whatever the plan, so a sheet has a
//! limit with no plan too, at 2^29 rows. A sheet also holds a slot for each source column its plan
//! reaches, once, whatever the batch.
//!
//! A suggestion's sampler has a batch nobody declared, so its size is never a reason to refuse: it
//! reads through batches of [`RawBatchCheck::sample_rows`] rows, as many as it takes, and only a
//! row that can't fit in any batch stops it.

use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnSpecWith, ERR_BATCH, ERR_CONTRACT, OK, RawBatchCheck, Slot, Span,
};
use crate::kernel::door::Door;
use hypercast::{CivilDateTime, Date, Decimal, Duration, Timestamp};

/// The most bytes one batch may ask for: 4 GiB, fixed, not raisable by a caller.
pub const BATCH_BYTES_LIMIT: u64 = 1 << 32;

/// The most bytes a sampler's batch asks for, unless one row takes more. A sample keeps what it
/// reads and a batch is only how it travels, so a sampler has no use for a batch near the limit,
/// and a binding that writes what it allocates would pay for all of it.
pub const SAMPLE_BATCH_BYTES: u64 = 1 << 26;

/// The longest record a reader will buffer, in bytes. A stream's input buffer stops growing here,
/// and a `buffer_bytes` past it starts here.
pub const MAX_ROW_BYTES: u64 = 1 << 30;

/// The cell-table entries a delimited reader holds at most, unless one row takes more: what a
/// batch of 4096 rows takes for a plan reaching 256 source columns, so that an ordinary plan's
/// batches are full, and a plan reaching far past any column a header declared costs one row's
/// entries rather than a batch of them.
pub const CELL_BUDGET: u64 = 4096 * 257;

/// The bytes one value of a door takes.
pub const fn value_bytes(door: Door) -> u64 {
    (match door {
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
    }) as u64
}

/// The rows of `per_row` entries a delimited cell table holds: as many as [`CELL_BUDGET`] allows,
/// at least one and at most a batch. The core fills only the rows the table holds.
pub const fn cell_rows(per_row: u64, batch_rows: u64) -> u64 {
    let room = match CELL_BUDGET.checked_div(per_row) {
        Some(room) if room > 0 => room,
        _ => 1,
    };
    let batch = if batch_rows == 0 { 1 } else { batch_rows };
    if room < batch { room } else { batch }
}

/// The bytes `rows` rows ask for, or `None` for an ask too big to count: the values, verdicts and
/// so on at `row_bytes` a row, a sheet's `slot_bytes`, and the cell table of `per_row` entries a
/// row. A sheet's table is a whole batch; a delimited one is what [`cell_rows`] says.
const fn batch_bytes(
    row_bytes: u64,
    slot_bytes: u64,
    per_row: u64,
    sheet: bool,
    rows: u64,
) -> Option<u64> {
    let table_rows = if sheet {
        rows
    } else {
        cell_rows(per_row, rows)
    };
    let Some(entries) = per_row.checked_mul(table_rows) else {
        return None;
    };
    let Some(cells) = entries.checked_mul(Span::SIZE as u64) else {
        return None;
    };
    let Some(values) = row_bytes.checked_mul(rows) else {
        return None;
    };
    let Some(sum) = values.checked_add(cells) else {
        return None;
    };
    sum.checked_add(slot_bytes)
}

const fn within_sample(bytes: Option<u64>) -> bool {
    matches!(bytes, Some(bytes) if bytes <= SAMPLE_BATCH_BYTES)
}

/// The most rows of at most `asked` whose batch is within `budget` bytes, and at least the one
/// row: the ask only grows with the rows, so this is a search, and nothing is allocated.
const fn rows_within_budget(
    row_bytes: u64,
    slot_bytes: u64,
    per_row: u64,
    sheet: bool,
    asked: u64,
) -> u64 {
    if within_sample(batch_bytes(row_bytes, slot_bytes, per_row, sheet, asked)) {
        return asked;
    }
    let mut within = 1;
    let mut past = asked;
    while past - within > 1 {
        let middle = within + (past - within) / 2;
        if within_sample(batch_bytes(row_bytes, slot_bytes, per_row, sheet, middle)) {
            within = middle;
        } else {
            past = middle;
        }
    }
    within
}

/// Judges a read's sizes before anything is allocated, and writes what it found to `out`:
/// [`OK`], or [`ERR_BATCH`] with `out.refused` saying why. `flags` is
/// [`RawBatchCheck::NOTES`], [`RawBatchCheck::WIDTHS`] and [`RawBatchCheck::SHEET`].
///
/// `batch_rows` of `0` is refused first, then `buffer_bytes` of `0`, which a sheet has none of
/// and doesn't read. A plan one row of which is past [`BATCH_BYTES_LIMIT`] is refused then, whose
/// `out.needed` is that row's bytes and `out.width` its columns, since no batch size would help.
/// An ask past the limit is refused last, and so is one too big to count, whose `out.needed` is
/// `u64::MAX`.
///
/// `out.sample_rows` is the rows of a sampler's batch for a sampler that wants `batch_rows`, within
/// [`SAMPLE_BATCH_BYTES`] and at least one, whatever the answer says of `batch_rows` itself: `0`
/// only where one row is past the limit or `batch_rows` is `0`.
///
/// [`ERR_CONTRACT`] writes nothing: flags this core doesn't define, a spec of the wrong `size`, or
/// a door that isn't one.
pub const fn check_batch(
    specs: &[ColumnSpecWith],
    batch_rows: u64,
    buffer_bytes: u64,
    flags: u32,
    out: &mut RawBatchCheck,
) -> i32 {
    if flags & !RawBatchCheck::FLAGS != 0 {
        return ERR_CONTRACT;
    }
    let notes = flags & RawBatchCheck::NOTES != 0;
    let widths = flags & RawBatchCheck::WIDTHS != 0;
    let sheet = flags & RawBatchCheck::SHEET != 0;

    let mut row_bytes = if widths { size_of::<u32>() as u64 } else { 0 };
    let mut reach = 0u64;
    let mut at = 0;
    while at < specs.len() {
        let spec = &specs[at];
        if spec.size != ColumnSpecWith::SIZE {
            return ERR_CONTRACT;
        }
        let Some(door) = Door::from_code(spec.door, spec.param) else {
            return ERR_CONTRACT;
        };
        let mut column = value_bytes(door) + CellVerdict::SIZE as u64;
        if notes {
            column += CellNote::SIZE as u64;
        }
        row_bytes = row_bytes.saturating_add(column);
        let ordinal = spec.ordinal as u64 + 1;
        if ordinal > reach {
            reach = ordinal;
        }
        at += 1;
    }

    // A sheet's table is indexed by plan column, a delimited one by source column.
    let (per_row, width, slot_bytes) = if sheet {
        (
            specs.len() as u64 + 1,
            // Whichever drives the row's count: a slot per column it reaches, a table entry per plan
            // column.
            if reach > specs.len() as u64 {
                reach
            } else {
                specs.len() as u64
            },
            reach * Slot::SIZE as u64,
        )
    } else {
        (reach + 1, reach, 0)
    };
    let one_row = batch_bytes(row_bytes, slot_bytes, per_row, sheet, 1);
    let too_wide = !matches!(one_row, Some(bytes) if bytes <= BATCH_BYTES_LIMIT);
    let needed = if too_wide {
        one_row
    } else {
        batch_bytes(row_bytes, slot_bytes, per_row, sheet, batch_rows)
    };

    let refused = if batch_rows == 0 {
        RawBatchCheck::REFUSED_BATCH_ROWS_ZERO
    } else if buffer_bytes == 0 && !sheet {
        RawBatchCheck::REFUSED_BUFFER_BYTES_ZERO
    } else if too_wide {
        RawBatchCheck::REFUSED_ROW_TOO_WIDE
    } else {
        match needed {
            Some(bytes) if bytes <= BATCH_BYTES_LIMIT => RawBatchCheck::REFUSED_NONE,
            _ => RawBatchCheck::REFUSED_OVER_LIMIT,
        }
    };
    *out = RawBatchCheck {
        needed: match needed {
            _ if batch_rows == 0 => 0,
            Some(bytes) => bytes,
            None => u64::MAX,
        },
        limit: BATCH_BYTES_LIMIT,
        buffer_bytes: if sheet {
            0
        } else if buffer_bytes < MAX_ROW_BYTES {
            buffer_bytes
        } else {
            MAX_ROW_BYTES
        },
        sample_rows: if too_wide || batch_rows == 0 {
            0
        } else {
            rows_within_budget(row_bytes, slot_bytes, per_row, sheet, batch_rows)
        },
        width,
        refused,
        reserved: 0,
    };
    if refused == RawBatchCheck::REFUSED_NONE {
        OK
    } else {
        ERR_BATCH
    }
}
