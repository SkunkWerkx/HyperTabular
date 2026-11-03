//! `Header`: the names a source's header row declared, and the ordinal each is at.

use std::cell::OnceCell;
use std::collections::HashMap;

use ext_php_rs::exception::PhpResult;
use ext_php_rs::prelude::*;
use ext_php_rs::types::{ZendClassObject, Zval};
use ext_php_rs::zend::{ClassEntry, ce};

use super::args::{ArrayArg, Mixed, Str};
use super::batch::{Array, MixedValue, typed_return};
use super::classes::{bool_zval, call, new_object};
use super::errors::{logic, out_of_bounds, out_of_range, pending};
use super::guard;

// JSON's interface, part of every PHP build since 8.0, read from the binary's exported data
// symbol as SPL's are (`errors.rs`).
#[cfg_attr(windows, link(name = "php8"))]
unsafe extern "C" {
    static php_json_serializable_ce: *mut ext_php_rs::ffi::zend_class_entry;
}

/// `\JsonSerializable`'s class entry.
fn json_serializable() -> &'static ClassEntry {
    // SAFETY: written once, in the JSON extension's MINIT, which runs before any shared
    // extension's.
    unsafe { php_json_serializable_ce.cast::<ClassEntry>().as_ref() }
        .expect("JsonSerializable is registered")
}

typed_return!(
    /// `\ArrayIterator`.
    ArrayIteratorObject,
    "ArrayIterator",
    false
);
typed_return!(
    /// `?Header`.
    MaybeHeader,
    "HyperTabular\\Header",
    true
);

/// The names a source's header row declared, and the ordinal each is at: what a plan is built
/// from when the columns are known by name.
///
/// ```php
/// $header = $reader->headerIndex();
/// $plan = [Column::i64($header->ordinal('id')), Column::text($header->ordinal('name'))];
/// $reader->bind($plan);
/// ```
///
/// A name is matched exactly, byte for byte; where a name repeats, the first column of that
/// name is the one found. A header is read-only: it counts, iterates and indexes as the list of
/// its names, and serializes to JSON as that list.
#[php_class]
#[php(name = "HyperTabular\\Header")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
#[php(implements(ce = ce::arrayaccess, stub = "\\ArrayAccess"))]
#[php(implements(ce = ce::countable, stub = "\\Countable"))]
#[php(implements(ce = ce::aggregate, stub = "\\IteratorAggregate"))]
#[php(implements(ce = json_serializable, stub = "\\JsonSerializable"))]
pub struct Header {
    names: Zval,
    count: usize,
    first: OnceCell<HashMap<Vec<u8>, i64>>,
}

impl Header {
    fn of(names: Zval) -> Header {
        let count = names.array().map_or(0, |names| names.len());
        Header {
            names,
            count,
            first: OnceCell::new(),
        }
    }

    /// A header object over `names`, a list of strings.
    pub(crate) fn object(names: &Zval) -> PhpResult<Zval> {
        use ext_php_rs::convert::IntoZval;
        ZendClassObject::new(Header::of(names.shallow_clone()))
            .into_zval(false)
            .map_err(|_| pending("header"))
    }

    fn lookup(&self, name: &[u8]) -> Option<i64> {
        let first = self.first.get_or_init(|| {
            let mut first = HashMap::new();
            if let Some(names) = self.names.array() {
                for (ordinal, (key, each)) in names.iter().enumerate() {
                    let ordinal = match key {
                        ext_php_rs::types::ArrayKey::Long(index) => index,
                        _ => ordinal as i64,
                    };
                    if let Some(each) = each.zend_str() {
                        first.entry(each.as_bytes().to_vec()).or_insert(ordinal);
                    }
                }
            }
            first
        });
        first.get(name).copied()
    }

    fn name_at(&self, offset: &Zval) -> Option<Zval> {
        let index = offset.long()?;
        let names = self.names.array()?;
        names
            .get_index(index)
            .filter(|name| !name.is_null())
            .map(Zval::shallow_clone)
    }
}

#[php_impl]
impl Header {
    /// A header naming `$names`, in order: the first names column 0.
    pub fn __construct(names: ArrayArg) -> PhpResult<Self> {
        guard(|| Ok(Header::of(names.0)))
    }

    /// The names, in column order.
    pub fn names(&self) -> PhpResult<Array> {
        guard(|| Ok(Array(self.names.shallow_clone())))
    }

    /// The ordinal of the first column named `$name` exactly — what `Column`'s factories take.
    ///
    /// @throws \OutOfBoundsException when no column has that name.
    pub fn ordinal(&self, name: Str) -> PhpResult<i64> {
        guard(|| {
            self.lookup(&name.0).ok_or_else(|| {
                out_of_bounds(format!(
                    "The header has no column named \"{}\"; it names {} columns",
                    String::from_utf8_lossy(&name.0),
                    self.count
                ))
            })
        })
    }

    /// The ordinal of the first column named `$name` exactly, or null when none is.
    pub fn find(&self, name: Str) -> PhpResult<Option<i64>> {
        guard(|| Ok(self.lookup(&name.0)))
    }

    /// How many columns the header names.
    pub fn count(&self) -> PhpResult<i64> {
        guard(|| Ok(self.count as i64))
    }

    /// The names, keyed by ordinal.
    pub fn get_iterator(&self) -> PhpResult<ArrayIteratorObject> {
        guard(|| {
            let class =
                ClassEntry::try_find("ArrayIterator").ok_or_else(|| pending("ArrayIterator"))?;
            let iterator = new_object(std::ptr::from_ref(class).cast_mut().cast())?;
            call(
                Some(&iterator),
                "__construct",
                &mut [self.names.shallow_clone()],
            )?;
            Ok(ArrayIteratorObject(Some(iterator)))
        })
    }

    /// Whether `$offset` is the ordinal of a column.
    pub fn offset_exists(&self, offset: Mixed) -> PhpResult<bool> {
        guard(|| Ok(offset.0.is_long() && self.name_at(&offset.0).is_some()))
    }

    /// The name of the column at ordinal `$offset`.
    ///
    /// @throws \OutOfRangeException when there is no such column.
    pub fn offset_get(&self, offset: Mixed) -> PhpResult<MixedValue> {
        guard(|| {
            if offset.0.is_long()
                && let Some(name) = self.name_at(&offset.0)
            {
                return Ok(MixedValue(name));
            }
            let shown = call(
                None,
                "var_export",
                &mut [offset.0.shallow_clone(), bool_zval(true)],
            )?;
            let shown = shown
                .zend_str()
                .map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned());
            Err(out_of_range(format!(
                "The header names {} columns; there is no column {}",
                self.count,
                shown.unwrap_or_default()
            )))
        })
    }

    /// Refused: a header cannot be changed.
    pub fn offset_set(&self, offset: Mixed, value: Mixed) -> PhpResult<()> {
        guard(|| {
            let _ = (offset, value);
            Err(logic("A header cannot be changed"))
        })
    }

    /// Refused: a header cannot be changed.
    pub fn offset_unset(&self, offset: Mixed) -> PhpResult<()> {
        guard(|| {
            let _ = offset;
            Err(logic("A header cannot be changed"))
        })
    }

    /// The names, in column order.
    pub fn json_serialize(&self) -> PhpResult<Array> {
        guard(|| Ok(Array(self.names.shallow_clone())))
    }
}
