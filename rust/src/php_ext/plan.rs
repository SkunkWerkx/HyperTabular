//! A plan as the crate takes it: the caller's list, and each entry as a `crate::Column`, checked
//! ahead of anything being opened.

use ext_php_rs::exception::PhpResult;
use ext_php_rs::types::Zval;

use super::classes::list_zval;
use super::convert::column_of;
use super::enums::Door;
use super::errors::{contract_violation, invalid_argument};
use super::values::column_fields;

pub(crate) struct Plan {
    /// The plan as the caller gave it, its entries in order: what `plan()` and a batch's
    /// `columns()` return.
    pub(crate) list: Zval,
    pub(crate) columns: Vec<crate::Column>,
    pub(crate) doors: Vec<Door>,
    pub(crate) ordinals: Vec<usize>,
}

impl Plan {
    /// Checks the plan, an array of `Column`s (its keys ignored), each entry as far as a column
    /// can be judged alone.
    pub(crate) fn new(plan: &Zval) -> PhpResult<Plan> {
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
        let mut columns = Vec::with_capacity(fields.len());
        let mut doors = Vec::with_capacity(fields.len());
        let mut ordinals = Vec::with_capacity(fields.len());
        for (index, column) in fields.iter().enumerate() {
            let door = Door::from_code(column.door).ok_or_else(contract_violation)?;
            let ordinal = usize::try_from(column.ordinal).map_err(|_| contract_violation())?;
            columns.push(column_of(index, column, ordinal)?);
            doors.push(door);
            ordinals.push(ordinal);
        }
        Ok(Plan {
            list: list_zval(entries)?,
            columns,
            doors,
            ordinals,
        })
    }
}
