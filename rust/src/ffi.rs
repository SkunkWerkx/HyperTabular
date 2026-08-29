//! The `#[repr(C)]` shapes every provider export and every binding share. This crate
//! exports no functions — HyperDelimited and HyperWorkbook each ship a `cdylib` whose
//! `read_batch` fills a [`RawBatchView`] over a [`crate::Batch`] — but the layouts are
//! defined once, here, so a binding's `Verdict<T>` folding is written once for both.
//!
//! Return-code conventions for provider exports: a non-negative value is a count; `-1`
//! is a contract violation (a caller bug: bad door code, colliding separators, null
//! handle); `-2` a structural error in the data, retrievable through the provider's
//! `last_error`; `-3` an I/O error.

use crate::batch::{Batch, CellVerdict, FaultRaw};
use crate::plan::{Column, Door, Plan};
use core::ffi::c_void;
use hypercast::NumFormat;

/// A caller bug, not a data verdict.
pub const ERR_CONTRACT: i64 = -1;
/// The data is structurally broken (torn row, bad container); details via `last_error`.
pub const ERR_STRUCTURE: i64 = -2;
/// The underlying read failed.
pub const ERR_IO: i64 = -3;

/// One plan column as it crosses the ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawColumnSpec {
    /// Zero-based source ordinal.
    pub ordinal: u32,
    /// [`Door::code`].
    pub door: u32,
    /// Unix precision (`1` s … `4` ns) for the Unix door; ignored otherwise.
    pub precision: u32,
    /// Decimal separator code point; `0` selects [`NumFormat::INVARIANT`] wholesale.
    pub decimal_sep: u32,
    /// Group separator code point.
    pub group_sep: u32,
    /// [`NumFormat`] lenience flags.
    pub flags: u32,
}

/// One column's output as it crosses the ABI: `rows` values at `values` (see
/// [`crate::Values::as_ptr`] for the element type per door) and `rows` verdicts.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawColumnView {
    pub values: *const c_void,
    pub verdicts: *const CellVerdict,
}

/// The batch as it crosses the ABI. Pointers stay valid until the next fill or close.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawBatchView {
    /// Rows in every column.
    pub rows: u64,
    /// The text arena `Text` spans and fault raws index into.
    pub arena: *const u8,
    pub arena_len: usize,
    /// The fault table.
    pub faults: *const FaultRaw,
    pub fault_count: usize,
}

/// Resolves a raw spec's numeric notation the way HyperCast's exports do: a zero
/// decimal separator means the invariant profile; otherwise both separators must be
/// valid, distinct code points.
pub fn num_format_from_raw(spec: &RawColumnSpec) -> Option<NumFormat> {
    if spec.decimal_sep == 0 {
        return Some(NumFormat::INVARIANT);
    }
    let decimal_sep = char::from_u32(spec.decimal_sep)?;
    let group_sep = char::from_u32(spec.group_sep)?;
    if decimal_sep == group_sep {
        return None;
    }
    Some(NumFormat {
        decimal_sep,
        group_sep,
        flags: spec.flags,
    })
}

/// Builds a [`Plan`] from raw specs; `None` is a contract violation.
pub fn plan_from_raw(specs: &[RawColumnSpec]) -> Option<Plan> {
    let mut columns = Vec::with_capacity(specs.len());
    for spec in specs {
        let door = Door::from_code(spec.door, spec.precision)?;
        let format = num_format_from_raw(spec)?;
        columns.push(Column {
            ordinal: spec.ordinal as usize,
            door,
            format,
        });
    }
    Some(Plan::new(columns))
}

impl Batch {
    /// The batch-level view.
    pub fn view(&self) -> RawBatchView {
        RawBatchView {
            rows: self.rows() as u64,
            arena: self.arena().as_ptr(),
            arena_len: self.arena().len(),
            faults: self.faults().as_ptr(),
            fault_count: self.faults().len(),
        }
    }

    /// One column's view, or `None` past the plan's width.
    pub fn column_view(&self, index: usize) -> Option<RawColumnView> {
        let column = self.columns().get(index)?;
        Some(RawColumnView {
            values: column.values().as_ptr(),
            verdicts: column.verdicts().as_ptr(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercast::UnixPrecision;

    #[test]
    fn raw_specs_become_a_plan_or_a_contract_violation() {
        let specs = [
            RawColumnSpec {
                ordinal: 2,
                door: 4,
                precision: 0,
                decimal_sep: 0,
                group_sep: 0,
                flags: 0,
            },
            RawColumnSpec {
                ordinal: 0,
                door: 14,
                precision: 2,
                decimal_sep: 0,
                group_sep: 0,
                flags: 0,
            },
            RawColumnSpec {
                ordinal: 1,
                door: 11,
                precision: 0,
                decimal_sep: ',' as u32,
                group_sep: '.' as u32,
                flags: 1,
            },
        ];
        let plan = plan_from_raw(&specs).unwrap();
        assert_eq!(plan.columns()[0], Column::new(2, Door::I32));
        assert_eq!(
            plan.columns()[1],
            Column::new(0, Door::Unix(UnixPrecision::Millis))
        );
        assert_eq!(
            plan.columns()[2],
            Column::new(1, Door::F64).with_format(NumFormat {
                decimal_sep: ',',
                group_sep: '.',
                flags: 1
            })
        );
        assert!(
            plan_from_raw(&[RawColumnSpec {
                door: 99,
                ..specs[0]
            }])
            .is_none()
        );
        assert!(
            plan_from_raw(&[RawColumnSpec {
                precision: 7,
                ..specs[1]
            }])
            .is_none()
        );
        assert!(
            plan_from_raw(&[RawColumnSpec {
                group_sep: ',' as u32,
                ..specs[2]
            }])
            .is_none()
        );
        assert!(
            plan_from_raw(&[RawColumnSpec {
                decimal_sep: 0xD800,
                ..specs[2]
            }])
            .is_none()
        );
    }

    #[test]
    fn every_door_code_round_trips() {
        for code in 1..=18 {
            let door = Door::from_code(code, 1).unwrap();
            assert_eq!(door.code(), code);
        }
        assert!(Door::from_code(0, 1).is_none());
        assert!(Door::from_code(19, 1).is_none());
    }
}
