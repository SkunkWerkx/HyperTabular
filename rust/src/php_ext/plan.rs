//! A plan as the core takes it, and the arrays the core casts it into: the column specs, one
//! value array and one verdict array per column sized for one batch, allocated once and reused
//! for every batch (1.0's `Columns`). A `DelimitedReader` and a `Sheet` each own one.

use ext_php_rs::exception::PhpResult;
use ext_php_rs::types::Zval;

use super::classes::list_zval;
use super::enums::Door;
use super::errors::{contract_violation, invalid_argument};
use super::hypercast::packed_format;
use super::values::column_fields;
use crate::kernel::abi::{CellVerdict, ColumnBuffer, ColumnSpec};

/// One column's arrays: its values as 8-byte words (so every door's type is aligned in them),
/// the bytes one value takes, and its verdicts.
struct Store {
    values: Vec<u64>,
    size: usize,
    verdicts: Vec<CellVerdict>,
}

pub(crate) struct Plan {
    /// The plan as the caller gave it, its entries in order: what `plan()` and a batch's
    /// `columns()` return.
    pub(crate) list: Zval,
    pub(crate) specs: Vec<ColumnSpec>,
    pub(crate) doors: Vec<Door>,
    pub(crate) ordinals: Vec<usize>,
    stores: Vec<Store>,
    pub(crate) batch_rows: usize,
    /// The widest ordinal the plan reads, plus one.
    pub(crate) width: usize,
}

impl Plan {
    /// Checks the plan, an array of `Column`s (its keys ignored), and allocates its arrays.
    pub(crate) fn new(plan: &Zval, batch_rows: usize) -> PhpResult<Plan> {
        let entries: Vec<Zval> = plan
            .array()
            .map(|table| table.values().map(Zval::shallow_clone).collect())
            .unwrap_or_default();
        let mut fields = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            match column_fields(entry)? {
                Some(column) => fields.push(column),
                None => {
                    return Err(invalid_argument(format!(
                        "Plan entry {index} is not a Column; build one with Column's factories"
                    )));
                }
            }
        }
        let mut specs = Vec::with_capacity(fields.len());
        let mut doors = Vec::with_capacity(fields.len());
        let mut ordinals = Vec::with_capacity(fields.len());
        let mut stores = Vec::with_capacity(fields.len());
        let mut width = 0;
        for column in &fields {
            let door = Door::from_code(column.door).ok_or_else(contract_violation)?;
            let ordinal = usize::try_from(column.ordinal).map_err(|_| contract_violation())?;
            width = width.max(ordinal + 1);
            let mut spec = ColumnSpec::new(
                u32::try_from(ordinal).map_err(|_| contract_violation())?,
                door as u32,
                u32::try_from(column.declared).unwrap_or(u32::MAX),
            );
            // HyperCast's NumFormat in the core's 92-byte size-tagged layout, written by
            // HyperCast as its own doors write it. The object validated itself when it was built.
            spec.format = packed_format(&column.format)?;
            let size = door.value_size();
            stores.push(Store {
                values: vec![0; (size * batch_rows).div_ceil(8)],
                size,
                verdicts: vec![CellVerdict::OK; batch_rows],
            });
            specs.push(spec);
            doors.push(door);
            ordinals.push(ordinal);
        }
        Ok(Plan {
            list: list_zval(entries)?,
            specs,
            doors,
            ordinals,
            stores,
            batch_rows,
            width,
        })
    }

    /// How many columns the plan has.
    pub(crate) fn count(&self) -> usize {
        self.specs.len()
    }

    /// The column buffers as the core takes them, pointing into the stores.
    pub(crate) fn buffers(&mut self) -> Vec<ColumnBuffer> {
        self.stores
            .iter_mut()
            .map(|store| ColumnBuffer {
                values: store.values.as_mut_ptr().cast(),
                verdicts: store.verdicts.as_mut_ptr(),
            })
            .collect()
    }

    /// The first `rows` values of every column, as the core wrote them, copied out.
    pub(crate) fn values(&self, rows: usize) -> Vec<Vec<u8>> {
        self.stores
            .iter()
            .map(|store| {
                // SAFETY: `u64`s are plain bytes; the length is within the store.
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        store.values.as_ptr().cast::<u8>(),
                        store.values.len() * 8,
                    )
                };
                bytes[..(store.size * rows).min(bytes.len())].to_vec()
            })
            .collect()
    }

    /// The first `rows` verdicts of every column, copied out.
    pub(crate) fn verdicts(&self, rows: usize) -> Vec<Vec<CellVerdict>> {
        self.stores
            .iter()
            .map(|store| store.verdicts[..rows.min(store.verdicts.len())].to_vec())
            .collect()
    }
}
