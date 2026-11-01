//! The plain classes' machinery: classes registered in MINIT the way the engine registers its own
//! internal classes from a stub, their typed readonly properties, their objects, and calls into
//! PHP. HyperCast's extension registers its value classes the same way (`classes.rs` there).
//!
//! 1.0 declared `Dialect` and `Column` as `final readonly` classes, `SheetInfo`, `SheetOptions`
//! and `TabularException` as `final` classes, each with constructor-promoted public readonly
//! properties. Registered with no object override and their properties declared with
//! `zend_declare_typed_property`, their objects are standard objects: the engine compares,
//! clones, dumps, serializes and exports them, and refuses every write and unset of a readonly
//! property, with its own messages. A constructor called from PHP writes through the engine
//! (`zend_update_property`), so a second `__construct` call is refused as 1.0's promoted
//! assignment was; a value the binding builds itself is written straight into its fresh
//! object's slots.

use std::ffi::c_char;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use ext_php_rs::args::ArgInfoTables;
use ext_php_rs::builders::{ClassBuilder, FunctionBuilder};
use ext_php_rs::class::ClassEntryInfo;
use ext_php_rs::exception::PhpResult;
use ext_php_rs::ffi::{
    _ZEND_TYPE_NAME_BIT, _call_user_function_impl, IS_FALSE, IS_LONG, IS_OBJECT_EX, IS_STRING,
    IS_TRUE, IS_UNDEF, ZEND_ACC_PROMOTED, ZEND_ACC_PUBLIC, object_properties_init,
    zend_call_known_function, zend_class_entry, zend_function, zend_object, zend_objects_new,
    zend_read_property, zend_string, zend_type, zval,
};
use ext_php_rs::flags::{ClassFlags, ConstantFlags, MethodFlags};
use ext_php_rs::types::{ZendHashTable, ZendObject, ZendStr, Zval};
use ext_php_rs::zend::{ClassEntry, ExecuteData, ExecutorGlobals};

use super::errors::{error, pending, runtime};

// Engine functions bindgen doesn't carry (`zend_API.h`, PHP 8.3 to 8.5).
#[cfg_attr(windows, link(name = "php8"))]
unsafe extern "C" {
    fn zend_declare_typed_property(
        ce: *mut zend_class_entry,
        name: *mut zend_string,
        property: *mut zval,
        access_type: i32,
        doc_comment: *mut zend_string,
        type_: zend_type,
    ) -> *mut std::ffi::c_void;
    fn zend_update_property(
        scope: *mut zend_class_entry,
        object: *mut zend_object,
        name: *const c_char,
        name_length: usize,
        value: *mut zval,
    );
    fn object_init_ex(arg: *mut zval, class_type: *mut zend_class_entry) -> i32;
}

/// `ZEND_ACC_READONLY` (`zend_compile.h`), for a property.
const ACC_READONLY: u32 = 1 << 7;

/// A plain class, by its slot in [`ENTRIES`].
#[derive(Clone, Copy)]
pub(crate) enum Plain {
    Dialect = 0,
    Column = 1,
    SheetInfo = 2,
    SheetOptions = 3,
    TabularException = 4,
}

/// The plain classes' entries, written once each, in MINIT.
static ENTRIES: [AtomicPtr<ClassEntry>; 5] = [
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
];

/// A property's declared type.
#[derive(Clone, Copy)]
pub(crate) enum Type {
    Int,
    String,
    Bool,
    Class(&'static str),
}

/// A plain class's declaration: its name, its promoted properties in declaration order, its
/// constants, whether the class itself is `readonly` (1.0's `final readonly class`) or only its
/// properties are (1.0's `final class` with `public readonly` promoted parameters), and its
/// parent.
pub(crate) struct Declaration {
    pub(crate) name: &'static str,
    pub(crate) properties: &'static [(&'static str, Type)],
    pub(crate) constants: &'static [(&'static str, i64)],
    pub(crate) readonly_class: bool,
    pub(crate) parent: Option<ClassEntryInfo>,
}

/// The class entry of `class`; registered in MINIT, so present whenever PHP calls in.
pub(crate) fn entry(class: Plain) -> *mut zend_class_entry {
    ENTRIES[class as usize]
        .load(Ordering::Acquire)
        .cast::<zend_class_entry>()
}

/// Records the class entry `register` returned in `class`'s slot and declares its properties.
pub(crate) fn finish_registration(
    class: Plain,
    declaration: &Declaration,
    ce: &'static mut ClassEntry,
) {
    for (name, ty) in declaration.properties {
        declare(ce, name, *ty);
    }
    ENTRIES[class as usize].store(ce, Ordering::Release);
}

/// Registers a plain class: final; readonly and closed to dynamic properties when the
/// declaration says the class is readonly; with `methods`, then (in `registration`, which must
/// call [`finish_registration`]) its properties.
pub(crate) fn register(
    declaration: &Declaration,
    methods: Vec<(FunctionBuilder<'static>, MethodFlags)>,
    registration: fn(&'static mut ClassEntry, ArgInfoTables),
) -> ext_php_rs::error::Result<()> {
    let flags = if declaration.readonly_class {
        ClassFlags::Final | ClassFlags::ReadonlyClass | ClassFlags::NoDynamicProperties
    } else {
        ClassFlags::Final
    };
    let mut builder = ClassBuilder::new(declaration.name).flags(flags);
    if let Some(parent) = declaration.parent {
        builder = builder.extends(parent);
    }
    for (function, flags) in methods {
        builder = builder.method(function, flags);
    }
    for (name, value) in declaration.constants {
        builder = builder.constant(*name, *value, &[], ConstantFlags::Public)?;
    }
    builder.registration(registration).register()
}

/// Declares the public readonly promoted property `name` of type `ty` on `ce`, with no default,
/// as the compiler declares a promoted constructor parameter.
fn declare(ce: &mut ClassEntry, name: &str, ty: Type) {
    let mask = match ty {
        Type::Int => 1 << IS_LONG,
        Type::String => 1 << IS_STRING,
        Type::Bool => (1 << IS_FALSE) | (1 << IS_TRUE),
        Type::Class(_) => _ZEND_TYPE_NAME_BIT,
    };
    let type_ptr = match ty {
        // An internal class's type names a persistent interned string, as a stub's does.
        Type::Class(class) => ZendStr::new_interned(class, true).into_raw().cast(),
        _ => ptr::null_mut(),
    };
    let name = ZendStr::new_interned(name, true).into_raw();
    let mut default = Zval::new();
    default.u1.type_info = IS_UNDEF;
    // SAFETY: MINIT, on the class `register` just created; the engine keeps the interned name
    // and type, and copies the (undefined) default.
    unsafe {
        zend_declare_typed_property(
            ce,
            name,
            &raw mut default,
            (ZEND_ACC_PUBLIC | ACC_READONLY | ZEND_ACC_PROMOTED) as i32,
            ptr::null_mut(),
            zend_type {
                ptr: type_ptr,
                type_mask: mask,
            },
        );
    }
}

/// A new object of `class`, as `new` makes one before the constructor runs: through the class's
/// own `create_object` when it has one (an exception's, which records the file and line).
pub(crate) fn instantiate(class: Plain) -> PhpResult<Zval> {
    let ce = entry(class);
    if ce.is_null() {
        return Err(runtime("hypertabular: class not registered"));
    }
    new_object(ce)
}

/// A new object of the class `ce`, its constructor not yet run, as `new` makes one.
pub(crate) fn new_object(ce: *mut zend_class_entry) -> PhpResult<Zval> {
    let mut zv = Zval::new();
    // SAFETY: a registered class; `object_init_ex` runs its `create_object` (or the standard
    // one) and initializes the property slots as undefined.
    if unsafe { object_init_ex(&raw mut zv, ce) } != 0 {
        return Err(pending("object creation"));
    }
    Ok(zv)
}

/// A zval holding `object`, taking over the reference the caller holds.
pub(crate) fn object_zval(object: *mut zend_object) -> Zval {
    let mut zv = Zval::new();
    zv.value.obj = object;
    zv.u1.type_info = IS_OBJECT_EX;
    zv
}

/// A zval holding `object`, with a reference of its own.
pub(crate) fn object_zval_ref(object: *mut zend_object) -> Zval {
    let mut zv = Zval::new();
    // SAFETY: a live object; `set_object` takes a reference of its own.
    zv.set_object(unsafe { &mut *object.cast::<ZendObject>() });
    zv
}

/// Builds a value of `class` from `values`, one per declared property in declaration order,
/// written straight into the fresh object's slots.
pub(crate) fn build(class: Plain, values: Vec<Zval>) -> PhpResult<Zval> {
    let ce = entry(class);
    if ce.is_null() {
        return Err(runtime("hypertabular: class not registered"));
    }
    // SAFETY: a registered class with the standard object handlers. Its property slots,
    // `default_properties_count` of them, start undefined, so each write takes over the zval's
    // reference and leaves nothing to release. The slot's extra bits are the property flags
    // (`Z_PROP_FLAG_P`), cleared as an initializing assignment clears them.
    unsafe {
        let object = zend_objects_new(ce);
        object_properties_init(object, ce);
        let count = (*ce).default_properties_count as usize;
        let table = (*object).properties_table.as_mut_ptr();
        for (i, value) in values.into_iter().enumerate().take(count) {
            let slot = table.add(i);
            ptr::write(slot, value);
            (*slot).u2.extra = 0;
        }
        Ok(object_zval(object))
    }
}

/// `$this` of the running method.
pub(crate) fn this(ex: &mut ExecuteData) -> PhpResult<*mut zend_object> {
    ex.This
        .object_mut()
        .map(ptr::from_mut)
        .map(|object: *mut ZendObject| object.cast::<zend_object>())
        .ok_or_else(|| error("Using $this when not in object context"))
}

/// Assigns `$object->name = value` from inside `scope`, as 1.0's promoted parameters did: the
/// engine checks the readonly rules and the type.
pub(crate) fn assign(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: &str,
    mut new: Zval,
) -> PhpResult<()> {
    // SAFETY: a live object and a registered class entry.
    unsafe {
        zend_update_property(
            scope,
            object,
            name.as_ptr().cast(),
            name.len(),
            &raw mut new,
        );
    }
    if ExecutorGlobals::has_exception() {
        return Err(pending("property assignment"));
    }
    Ok(())
}

/// Reads `$object->name` from inside `scope`, as 1.0's methods did: an uninitialized property
/// is the engine's `Error`.
pub(crate) fn read(scope: *mut zend_class_entry, object: &Zval, name: &str) -> PhpResult<Zval> {
    let object = object_of(object).ok_or_else(|| pending("property read"))?;
    let mut rv = Zval::new();
    // SAFETY: a live object and a registered class; the result points at the property or `rv`.
    let found = unsafe {
        zend_read_property(
            scope,
            object,
            name.as_ptr().cast(),
            name.len(),
            false,
            &raw mut rv,
        )
    };
    if ExecutorGlobals::has_exception() {
        return Err(pending("property read"));
    }
    // SAFETY: a live zval, the property's or `rv`; the copy holds its own reference.
    let copy = unsafe { found.as_ref() }
        .map(Zval::shallow_clone)
        .unwrap_or_default();
    Ok(copy)
}

/// The object a zval holds, if it holds one.
pub(crate) fn object_of(zv: &Zval) -> Option<*mut zend_object> {
    zv.object()
        .map(|object| ptr::from_ref(object).cast_mut().cast::<zend_object>())
}

/// Whether `zv` holds an object of exactly `class` (every plain class is final).
pub(crate) fn is_a(zv: &Zval, class: Plain) -> bool {
    // SAFETY: a live object's class entry.
    object_of(zv).is_some_and(|object| unsafe { (*object).ce } == entry(class))
}

/// A string zval of `bytes`, which need not be UTF-8.
pub(crate) fn bytes_zval(bytes: &[u8]) -> Zval {
    let mut zv = Zval::new();
    zv.set_zend_string(ZendStr::new(bytes, false));
    zv
}

/// An int zval.
pub(crate) fn int_zval(n: i64) -> Zval {
    let mut zv = Zval::new();
    zv.set_long(n);
    zv
}

/// A bool zval.
pub(crate) fn bool_zval(b: bool) -> Zval {
    let mut zv = Zval::new();
    zv.set_bool(b);
    zv
}

/// A float zval.
pub(crate) fn float_zval(f: f64) -> Zval {
    let mut zv = Zval::new();
    zv.set_double(f);
    zv
}

/// An array zval of `table`.
pub(crate) fn array_zval(table: ext_php_rs::boxed::ZBox<ZendHashTable>) -> Zval {
    let mut zv = Zval::new();
    zv.set_hashtable(table);
    zv
}

/// A list of `values`.
pub(crate) fn list_zval(values: Vec<Zval>) -> PhpResult<Zval> {
    let mut table = ZendHashTable::with_capacity(values.len().try_into().unwrap_or(u32::MAX));
    for value in values {
        table.push(value).map_err(|_| pending("array append"))?;
    }
    Ok(array_zval(table))
}

/// Calls the PHP function or static method `name` (`"Class::method"`), or with `object` the
/// method `name` on it, with `args`, as a PHP caller would. Whatever it throws stays pending
/// and is returned as the error.
pub(crate) fn call(object: Option<&Zval>, name: &str, args: &mut [Zval]) -> PhpResult<Zval> {
    let mut function = bytes_zval(name.as_bytes());
    let mut target = object.map(Zval::shallow_clone);
    let mut retval = Zval::new();
    let count = u32::try_from(args.len()).map_err(|_| pending(name))?;
    // SAFETY: live zvals for the call; the engine copies what it keeps.
    let status = unsafe {
        _call_user_function_impl(
            target.as_mut().map_or(ptr::null_mut(), ptr::from_mut),
            &raw mut function,
            &raw mut retval,
            count,
            args.as_mut_ptr(),
            ptr::null_mut(),
        )
    };
    if status < 0 || ExecutorGlobals::has_exception() {
        return Err(pending(name));
    }
    Ok(retval)
}

/// Calls the method `function` (a static one with no `object`) found in `scope`'s function
/// table, as a PHP caller would. Whatever it throws stays pending and is returned as the error.
///
/// # Safety
///
/// `function` must be a live function of `scope`, and `object` null or a live object of it.
pub(crate) unsafe fn call_known(
    function: *mut zend_function,
    object: *mut zend_object,
    scope: *mut zend_class_entry,
    args: &mut [Zval],
) -> PhpResult<Zval> {
    let mut retval = Zval::new();
    let count = u32::try_from(args.len()).map_err(|_| pending("call"))?;
    // SAFETY: the caller's contract; live zvals for the call, which the engine copies.
    unsafe {
        zend_call_known_function(
            function,
            object,
            scope,
            &raw mut retval,
            count,
            args.as_mut_ptr(),
            ptr::null_mut(),
        );
    }
    if ExecutorGlobals::has_exception() {
        return Err(pending("call"));
    }
    Ok(retval)
}

/// Sets a hand-registered handler's return value, or throws.
pub(crate) fn finish(result: PhpResult<Zval>, retval: &mut Zval) {
    match result {
        Ok(value) => *retval = value,
        Err(e) => e.throw(),
    }
}
