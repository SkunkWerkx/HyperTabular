//! The PHP binding: this crate linked straight into a Zend extension through `ext-php-rs`, the
//! same native-extension pattern as the Python (PyO3) and Ruby (Magnus) bindings, and as
//! HyperCast's own PHP extension, which judges every cell. The whole `HyperTabular\` API is
//! defined here, 1.0's userland classes rebuilt with the same names, signatures, defaults and
//! messages over the crate's Rust API (`crate::DelimitedReader`, `crate::Workbook`, `crate::Sheet`),
//! which owns every buffer and every rule; the Composer package ships no PHP.
//!
//! Three kinds of class. The readers and what they hand out — `DelimitedReader`, `Workbook`,
//! `Sheet`, `Batch`, `Row`, `RowIterator`, `Header` — hold Rust state and are `#[php_class]`es,
//! as is `Tabular`. `Dialect`, `Column`, `SheetInfo`, `SheetOptions`, `TabularException`, `Range`,
//! `Problem`, `SuggestOptions`, `Suggestion`, `SuggestedColumn` and `AmbiguousColumn` are plain
//! values with public readonly promoted properties, so they are registered as plain PHP classes
//! (`values.rs`, `results.rs`, `classes.rs`). `Door`, `TabularFailure`, `WorkbookFormat`,
//! `ShortRows`, `LongRows`, `Encoding`, `Origin` and `SuggestStatus` are int-backed enums.
//!
//! Every verdict is HyperCast's own object, made through HyperCast's PHP API at runtime
//! (`hypercast.rs`): the extensions load in either order and are released apart.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ext_php_rs::error::php_error;
use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::flags::ErrorType;
use ext_php_rs::prelude::*;
use ext_php_rs::types::Zval;

mod args;
mod batch;
mod classes;
mod convert;
mod delimited;
mod enums;
mod errors;
mod header;
mod hypercast;
mod plan;
mod registration;
mod results;
mod streams;
mod tabular;
mod values;
mod workbook;

/// Runs a PHP-callable body and turns a panic into a `\RuntimeException`, so unwinding never
/// crosses the Zend engine's C frames. Every `#[php_impl]` body and every hand-registered
/// handler in this module is `guard(|| ...)` as its first expression.
pub(crate) fn guard<T>(f: impl FnOnce() -> PhpResult<T>) -> PhpResult<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            Err(PhpException::new(
                format!("hypertabular: internal panic: {detail}"),
                0,
                errors::runtime_exception(),
            ))
        }
    }
}

/// How PHP 8.3+ names a value in a type error (`zend_zval_value_name`): `true` and `false` for
/// booleans, and the class name for an object, up to its first NUL byte, as the engine's `%s`
/// prints it.
pub(crate) fn value_name(zv: &Zval) -> String {
    let name = if zv.is_null() {
        "null"
    } else if zv.is_true() {
        "true"
    } else if zv.is_false() {
        "false"
    } else if zv.is_long() {
        "int"
    } else if zv.is_double() {
        "float"
    } else if zv.is_string() {
        "string"
    } else if zv.is_array() {
        "array"
    } else if zv.is_resource() {
        "resource"
    } else if let Some(object) = zv.object() {
        let class = object
            .get_class_name()
            .unwrap_or_else(|_| "object".to_owned());
        return class.split('\0').next().unwrap_or_default().to_owned();
    } else {
        "mixed"
    };
    name.to_owned()
}

/// RSHUTDOWN: stops caching, then releases the request's shared dialects while the engine can
/// still free them.
extern "C" fn request_shutdown(_type: i32, _module_number: i32) -> i32 {
    let _ = catch_unwind(values::release_shared);
    0
}

/// Post-RSHUTDOWN, run for every request after every module's RSHUTDOWN: ends the request for
/// the cache and forgets, without releasing, anything still cached.
extern "C" fn post_deactivate() -> i32 {
    let _ = catch_unwind(values::end_request);
    0
}

/// RINIT: forgets any shared dialect a request that never reached post-RSHUTDOWN left behind,
/// finishes the registration for a `dl()` load, and starts caching.
extern "C" fn request_startup(_type: i32, _module_number: i32) -> i32 {
    let _ = catch_unwind(values::forget_stale);
    registration::finish_late();
    values::begin_request();
    0
}

/// Why a thread-safe PHP's `dl()` fails: ext-php-rs keeps each class's entry in process-wide
/// state, and a `dl()` registers the classes again from the request thread that called it,
/// into that thread's class table, for that request only. Another thread's `dl()` would
/// overwrite the state with its own entries, freed when its request ends. A load at startup,
/// the `extension=` line PIE writes, registers them once, before any request thread exists.
const DL_REFUSED: &str = "hypertabular: a thread-safe PHP loads this extension at startup only: add extension=hypertabular to php.ini (PIE writes it) rather than calling dl()";

/// MINIT, run before ext-php-rs registers the `#[php_class]`es and the enums: registers the
/// plain classes, and schedules what can only be written once every class exists. A failed
/// registration fails MINIT, so the extension never loads without its classes; so does a
/// thread-safe PHP's `dl()` ([`DL_REFUSED`]), before anything registers.
extern "C" fn module_startup(module_type: i32, _module_number: i32) -> i32 {
    if cfg!(php_zts) && module_type != registration::MODULE_PERSISTENT {
        let _ = catch_unwind(|| php_error(&ErrorType::Warning, DL_REFUSED));
        return -1;
    }
    if !matches!(catch_unwind(values::register_all), Ok(Ok(()))) {
        return -1;
    }
    let _ = catch_unwind(|| registration::schedule(module_type));
    0
}

#[php_module]
pub fn get_module(module: ModuleBuilder) -> ModuleBuilder {
    module
        .version(env!("CARGO_PKG_VERSION"))
        .startup_function(module_startup)
        .request_startup_function(request_startup)
        .request_shutdown_function(request_shutdown)
        .post_deactivate_function(post_deactivate)
        .enumeration::<enums::Door>()
        .enumeration::<enums::TabularFailure>()
        .enumeration::<enums::WorkbookFormat>()
        .enumeration::<enums::ShortRows>()
        .enumeration::<enums::LongRows>()
        .enumeration::<enums::Encoding>()
        .enumeration::<enums::Origin>()
        .enumeration::<enums::SuggestStatus>()
        .class::<tabular::Tabular>()
        .class::<header::Header>()
        .class::<batch::Batch>()
        .class::<batch::Row>()
        .class::<batch::RowIterator>()
        .class::<delimited::DelimitedReader>()
        .class::<workbook::Workbook>()
        .class::<workbook::Sheet>()
}
