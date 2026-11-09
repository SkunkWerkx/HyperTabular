//! HyperCast's PHP extension, the judge of every cell: its classes are looked up at runtime, by
//! name, and every verdict and value this binding hands out is one of them, made the way a PHP
//! caller makes it — `new Success($value)`, `Interop\NativeValues::fault(...)`,
//! `Decimal::fromLimbs(...)` and the rest of HyperCast's interop surface, called through the
//! engine (`zend_call_known_function`). Nothing here knows how HyperCast lays out its objects,
//! so the two extensions can be built and released apart, as long as HyperCast keeps its PHP
//! API.
//!
//! The lookup is not done at load: the extensions load in either order, and this one loads (and
//! passes the Forge's load check) on its own. Every call that needs HyperCast and doesn't find
//! it is a `\RuntimeException` naming the missing extension, never a cell verdict.

use std::ptr;

use ext_php_rs::exception::PhpResult;
use ext_php_rs::ffi::{
    object_properties_init, zend_class_entry, zend_function, zend_object, zend_objects_new,
    zend_property_info, zval,
};
use ext_php_rs::types::Zval;
use ext_php_rs::zend::ClassEntry;

use super::classes::{
    bool_zval, bytes_zval, call, call_known, int_zval, new_named, new_object, object_of,
    object_zval, read_own,
};
use super::errors::{pending, runtime};

/// `ZEND_INTERNAL_CLASS` (`zend_compile.h`): a class an extension registered, which a userland
/// namesake cannot be.
const ZEND_INTERNAL_CLASS: u8 = 1;

/// The message when HyperCast's extension is not loaded.
pub(crate) const MISSING: &str = "hypertabular: the hypercast extension is not loaded; \
     HyperTabular judges every cell with HyperCast (pie install skunkwerkx/hypercast)";

/// Whether HyperCast's extension has registered its classes.
pub(crate) fn loaded() -> bool {
    class("HyperCast\\Success").is_ok()
}

/// A HyperCast class by name.
fn class(name: &str) -> PhpResult<*mut zend_class_entry> {
    ClassEntry::try_find(name)
        .filter(|ce| ce.type_ as u8 == ZEND_INTERNAL_CLASS)
        .map(|ce| ptr::from_ref(ce).cast_mut().cast::<zend_class_entry>())
        .ok_or_else(|| runtime(MISSING))
}

/// The method `lower` (lowercase) of `ce`.
fn method(ce: *mut zend_class_entry, lower: &str) -> PhpResult<*mut zend_function> {
    // SAFETY: a live class entry, registered for the process; its function table's values are
    // `IS_PTR` zvals holding the functions.
    unsafe {
        (*ce)
            .function_table
            .get(lower)
            .and_then(|zv| zv.ptr::<zend_function>())
    }
    .ok_or_else(|| runtime(MISSING))
}

/// The offset of the one property of `ce`, `value`, if `ce` is a plain class of standard
/// objects with that property alone: how HyperCast declares `Success`
/// (`final readonly class Success { public function __construct(public mixed $value) {} }`).
fn value_slot(ce: *mut zend_class_entry) -> Option<u32> {
    // SAFETY: a live, registered class entry; its property table's values are `IS_PTR` zvals
    // holding property infos.
    unsafe {
        let class = &*ce;
        if class.__bindgen_anon_2.create_object.is_some() || class.default_properties_count != 1 {
            return None;
        }
        let info = class
            .properties_info
            .get("value")?
            .ptr::<zend_property_info>()?;
        Some((*info).offset)
    }
}

/// The classes and methods a batch builds its cells with, looked up once per column.
pub(crate) struct Judge {
    success: *mut zend_class_entry,
    success_new: *mut zend_function,
    /// Where a `Success` keeps its `value`, when the class is the plain one HyperCast declares:
    /// a `Success` is then made as its promoting constructor makes it, without the call.
    success_slot: Option<u32>,
    native_values: *mut zend_class_entry,
    fault: *mut zend_function,
    date: *mut zend_function,
    instant: *mut zend_function,
    civil: *mut zend_function,
    uuid: *mut zend_function,
    decimal: *mut zend_class_entry,
    from_limbs: *mut zend_function,
    duration: *mut zend_class_entry,
    duration_new: *mut zend_function,
}

impl Judge {
    /// HyperCast's classes, or the `\RuntimeException` that says its extension is missing.
    pub(crate) fn find() -> PhpResult<Judge> {
        let success = class("HyperCast\\Success")?;
        let native_values = class("HyperCast\\Interop\\NativeValues")?;
        let decimal = class("HyperCast\\Decimal")?;
        let duration = class("HyperCast\\Duration")?;
        Ok(Judge {
            success,
            success_new: method(success, "__construct")?,
            success_slot: value_slot(success),
            native_values,
            fault: method(native_values, "fault")?,
            date: method(native_values, "date")?,
            instant: method(native_values, "instant")?,
            civil: method(native_values, "civil")?,
            uuid: method(native_values, "uuid")?,
            decimal,
            from_limbs: method(decimal, "fromlimbs")?,
            duration,
            duration_new: method(duration, "__construct")?,
        })
    }

    /// `new $class(...$args)`.
    fn construct(
        &self,
        class: *mut zend_class_entry,
        constructor: *mut zend_function,
        args: &mut [Zval],
    ) -> PhpResult<Zval> {
        let object = new_object(class)?;
        let raw: *mut zend_object = object_of(&object).ok_or_else(|| pending("object creation"))?;
        // SAFETY: the class's own constructor, on a fresh object of the class.
        unsafe { call_known(constructor, raw, class, args)? };
        Ok(object)
    }

    /// A static method of `class`.
    fn statically(
        &self,
        class: *mut zend_class_entry,
        function: *mut zend_function,
        args: &mut [Zval],
    ) -> PhpResult<Zval> {
        // SAFETY: a static method of the class, found in its function table.
        unsafe { call_known(function, ptr::null_mut(), class, args) }
    }

    /// `new Success($value)`.
    pub(crate) fn success(&self, value: Zval) -> PhpResult<Zval> {
        let Some(offset) = self.success_slot else {
            return self.construct(self.success, self.success_new, &mut [value]);
        };
        // SAFETY: `value_slot` found the class to have the standard object handlers and one
        // property, `value`, at `offset`; a fresh object's slot starts undefined, so the write
        // takes over the zval's reference and leaves nothing to release, and clearing the
        // slot's flags (`Z_PROP_FLAG_P`) is what the constructor's initializing assignment does.
        unsafe {
            let object = zend_objects_new(self.success);
            object_properties_init(object, self.success);
            let slot = object.cast::<u8>().add(offset as usize).cast::<zval>();
            ptr::write(slot.cast::<Zval>(), value);
            (*slot).u2.extra = 0;
            Ok(object_zval(object))
        }
    }

    /// `NativeValues::fault($code, $offset, $length)`: the C ABI's reason code and span.
    pub(crate) fn fault(&self, code: u32, offset: u32, length: u32) -> PhpResult<Zval> {
        let mut args = [
            int_zval(i64::from(code)),
            int_zval(i64::from(offset)),
            int_zval(i64::from(length)),
        ];
        self.statically(self.native_values, self.fault, &mut args)
    }

    /// `NativeValues::date($year, $month, $day)`: a `\DateTimeImmutable` at UTC midnight.
    pub(crate) fn date(&self, year: i64, month: i64, day: i64) -> PhpResult<Zval> {
        let mut args = [int_zval(year), int_zval(month), int_zval(day)];
        self.statically(self.native_values, self.date, &mut args)
    }

    /// `NativeValues::instant($seconds, $nanos)`: a UTC `\DateTimeImmutable`.
    pub(crate) fn instant(&self, seconds: i64, nanos: i64) -> PhpResult<Zval> {
        let mut args = [int_zval(seconds), int_zval(nanos)];
        self.statically(self.native_values, self.instant, &mut args)
    }

    /// `NativeValues::civil($year, $month, $day, $nanosOfDay)`.
    pub(crate) fn civil(&self, year: i64, month: i64, day: i64, nanos: i64) -> PhpResult<Zval> {
        let mut args = [
            int_zval(year),
            int_zval(month),
            int_zval(day),
            int_zval(nanos),
        ];
        self.statically(self.native_values, self.civil, &mut args)
    }

    /// `NativeValues::uuid($bytes)`: the lowercase hyphenated string.
    pub(crate) fn uuid(&self, bytes: &[u8]) -> PhpResult<Zval> {
        self.statically(self.native_values, self.uuid, &mut [bytes_zval(bytes)])
    }

    /// `Decimal::fromLimbs($lo, $hi, $scale, $negative)`.
    pub(crate) fn decimal(&self, lo: i64, hi: i64, scale: i64, negative: bool) -> PhpResult<Zval> {
        let mut args = [
            int_zval(lo),
            int_zval(hi),
            int_zval(scale),
            bool_zval(negative),
        ];
        self.statically(self.decimal, self.from_limbs, &mut args)
    }

    /// `new Duration($seconds, $nanos)`.
    pub(crate) fn duration(&self, seconds: i64, nanos: i64) -> PhpResult<Zval> {
        self.construct(
            self.duration,
            self.duration_new,
            &mut [int_zval(seconds), int_zval(nanos)],
        )
    }
}

/// `NumFormat::invariant()`, the notation a numeric column reads without one.
pub(crate) fn invariant_format() -> PhpResult<Zval> {
    class("HyperCast\\NumFormat")?;
    call(None, "HyperCast\\NumFormat::invariant", &mut [])
}

/// `$format->packed()`: the core's 92-byte size-tagged `RawNumFormat`, as HyperCast writes it
/// for its own doors.
pub(crate) fn packed_format(format: &Zval) -> PhpResult<hypercast::RawNumFormat> {
    // A method is called only on what is one: anything else isn't a format to pack.
    if !super::args::instance_of(format, b"HyperCast\\NumFormat") {
        return Err(super::errors::invalid_argument(
            "A numeric format must be a HyperCast\\NumFormat",
        ));
    }
    let packed = call(Some(format), "packed", &mut [])?;
    let bytes: [u8; 92] = packed
        .zend_str()
        .and_then(|s| s.as_bytes().try_into().ok())
        .ok_or_else(super::errors::contract_violation)?;
    Ok(hypercast::RawNumFormat::from_le_bytes(bytes))
}

/// `ExcelEpoch::from($code)`: HyperCast's case for the core's code.
pub(crate) fn excel_epoch(code: u32) -> PhpResult<Zval> {
    class("HyperCast\\ExcelEpoch")?;
    call(
        None,
        "HyperCast\\ExcelEpoch::from",
        &mut [int_zval(i64::from(code))],
    )
}

/// `new DetailedFault(CastFailure::from($reason), $offset, $length, $code)`.
pub(crate) fn detailed_fault(reason: u32, offset: u32, length: u32, code: u32) -> PhpResult<Zval> {
    class("HyperCast\\DetailedFault")?;
    let reason = hypercast_enum("CastFailure", reason)?;
    new_named(
        "HyperCast\\DetailedFault",
        &mut [
            reason,
            int_zval(i64::from(offset)),
            int_zval(i64::from(length)),
            int_zval(i64::from(code)),
        ],
    )
}

/// `\HyperCast\<name>::from($code)`.
pub(crate) fn hypercast_enum(name: &str, code: u32) -> PhpResult<Zval> {
    let class_name = format!("HyperCast\\{name}");
    class(&class_name)?;
    call(
        None,
        &format!("{class_name}::from"),
        &mut [int_zval(i64::from(code))],
    )
}

/// The notation HyperTabular's suggest calls the comma decimal, HyperCast's own object:
/// `new NumFormat(',', '.', NumFormat::ALL)`.
pub(crate) fn comma_decimal_format() -> PhpResult<Zval> {
    let all = call(
        None,
        "constant",
        &mut [bytes_zval(b"HyperCast\\NumFormat::ALL")],
    )?;
    new_named(
        "HyperCast\\NumFormat",
        &mut [bytes_zval(b","), bytes_zval(b"."), all],
    )
}

/// The backing value of one of HyperCast's enum cases (or this binding's own).
pub(crate) fn enum_value(case: &Zval) -> Option<i64> {
    // Read for reading (`zend_read_property`), in the case's own scope: `get_property` reads for
    // writing, which an enum's readonly `value` refuses.
    read_own(case, "value").ok()?.long()
}
