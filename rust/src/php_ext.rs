//! This crate as a Zend extension, through `ext-php-rs` — the module entry point the
//! forge's pipeline builds (`--features php`), loads into a real PHP and attests on every
//! darwin and linux leg, as it does HyperCast's.
//!
//! Deliberately not the PHP binding: the `skunkwerkx/hypertabular` Composer package is
//! `ext-ffi` over the plain shared library (`../../php/src`), which already crosses the
//! boundary once per batch rather than once per cell, so there is no per-call cost here for
//! an extension to remove. What this module registers is what the load-check needs and
//! nothing else: the module itself — named after the crate, which is what
//! `extension_loaded("hypertabular")` asks for — and a probe that says which core it
//! carries. The build still exports every C ABI symbol the `ext-ffi` binding calls.

use ext_php_rs::prelude::*;

/// The core's own version, `major.minor.patch`, from the crate's manifest — the same
/// number `hypertabular_version` packs for the C ABI.
#[php_function]
#[php(name = "hypertabular_native_version")]
pub fn hypertabular_native_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[php_module]
pub fn get_module(module: ModuleBuilder) -> ModuleBuilder {
    module.function(wrap_function!(hypertabular_native_version))
}
