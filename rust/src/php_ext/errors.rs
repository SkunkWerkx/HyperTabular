//! The exceptions the binding throws, with 1.0's classes and messages, and the way back from a
//! PHP call that threw.

use ext_php_rs::exception::PhpException;
use ext_php_rs::zend::{ClassEntry, ExecutorGlobals, ce};

// SPL's class entries, read from the PHP binary's exported data symbols. Not
// `ClassEntry::try_find`: it returns `None` while `ExecutorGlobals::class_table()` is null,
// which it is at MINIT. A data symbol imported from a DLL needs the import library on Windows,
// as ext-php-rs's windows_build.rs adds to its own bindgen blocks: `php8` for an NTS
// PHP, `php8ts` for a thread-safe one (build.rs sets `php_zts`).
#[cfg_attr(all(windows, not(php_zts)), link(name = "php8"))]
#[cfg_attr(all(windows, php_zts), link(name = "php8ts"))]
unsafe extern "C" {
    static spl_ce_RuntimeException: *mut ext_php_rs::ffi::zend_class_entry;
    static spl_ce_InvalidArgumentException: *mut ext_php_rs::ffi::zend_class_entry;
    static spl_ce_LogicException: *mut ext_php_rs::ffi::zend_class_entry;
    static spl_ce_OutOfRangeException: *mut ext_php_rs::ffi::zend_class_entry;
    static spl_ce_OutOfBoundsException: *mut ext_php_rs::ffi::zend_class_entry;
}

/// What 1.0 threw for a code the core should never have returned to its own binding.
pub(crate) const CONTRACT: &str =
    "hypertabular: libhypertabular reported a contract violation — a binding bug, please report it";

/// `\RuntimeException`'s class entry.
pub(crate) fn runtime_exception() -> &'static ClassEntry {
    // SAFETY: SPL writes the pointer once, in its own MINIT, before shared extensions start, so
    // this unsynchronised read never races a write.
    unsafe { spl_ce_RuntimeException.as_ref() }.expect("SPL's RuntimeException is registered")
}

/// `\InvalidArgumentException`'s class entry.
fn invalid_argument_exception() -> &'static ClassEntry {
    // SAFETY: as for `runtime_exception`.
    unsafe { spl_ce_InvalidArgumentException.as_ref() }
        .expect("SPL's InvalidArgumentException is registered")
}

/// `\LogicException`'s class entry.
fn logic_exception() -> &'static ClassEntry {
    // SAFETY: as for `runtime_exception`.
    unsafe { spl_ce_LogicException.as_ref() }.expect("SPL's LogicException is registered")
}

/// `\OutOfRangeException`'s class entry.
fn out_of_range_exception() -> &'static ClassEntry {
    // SAFETY: as for `runtime_exception`.
    unsafe { spl_ce_OutOfRangeException.as_ref() }.expect("SPL's OutOfRangeException is registered")
}

/// `\OutOfBoundsException`'s class entry.
fn out_of_bounds_exception() -> &'static ClassEntry {
    // SAFETY: as for `runtime_exception`.
    unsafe { spl_ce_OutOfBoundsException.as_ref() }
        .expect("SPL's OutOfBoundsException is registered")
}

/// A `\RuntimeException`.
pub(crate) fn runtime(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, runtime_exception())
}

/// An `\InvalidArgumentException`.
pub(crate) fn invalid_argument(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, invalid_argument_exception())
}

/// A `\LogicException`.
pub(crate) fn logic(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, logic_exception())
}

/// An `\OutOfRangeException`.
pub(crate) fn out_of_range(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, out_of_range_exception())
}

/// An `\OutOfBoundsException`.
pub(crate) fn out_of_bounds(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, out_of_bounds_exception())
}

/// An `\Error`.
pub(crate) fn error(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, ce::error())
}

/// A `\TypeError`.
pub(crate) fn type_error(message: impl Into<String>) -> PhpException {
    PhpException::new(message.into(), 0, ce::type_error())
}

/// 1.0's `\RuntimeException` for a code the core should never have returned.
pub(crate) fn contract_violation() -> PhpException {
    runtime(CONTRACT)
}

/// An exception object this binding made (a `TabularException`), thrown as itself, so that a
/// failure kept and thrown again is the same object.
pub(crate) fn object(exception: &ext_php_rs::types::Zval) -> PhpException {
    PhpException::new(String::new(), 0, runtime_exception()).with_object(exception.shallow_clone())
}

/// The exception a PHP call this binding made has left pending: returned as the error, it
/// throws nothing more (`PhpException::throw` leaves a pending exception alone), so the
/// original propagates untouched. Without one pending, a `\RuntimeException` saying what failed.
pub(crate) fn pending(what: &str) -> PhpException {
    if ExecutorGlobals::has_exception() {
        // Never thrown: the pending exception stands.
        PhpException::new(String::new(), 0, ce::exception())
    } else {
        runtime(format!("hypertabular: {what} failed"))
    }
}
