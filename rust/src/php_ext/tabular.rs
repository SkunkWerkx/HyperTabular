//! `Tabular`, the binding's own facts.

use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::prelude::*;
use ext_php_rs::zend::ce;

use super::guard;
use super::hypercast;

/// The binding's own facts.
#[php_class]
#[php(name = "HyperTabular\\Tabular")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
pub struct Tabular;

#[php_impl]
impl Tabular {
    /// Holds only static methods; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Tabular holds only static methods".into(),
                0,
                ce::error(),
            ))
        })
    }

    /// Whether the binding can read: the hypertabular extension is loaded — this class is part of
    /// it — and so is HyperCast's, the judge of every cell. Never throws.
    pub fn is_available() -> PhpResult<bool> {
        guard(|| Ok(hypercast::loaded()))
    }

    /// The version of the hypertabular extension, `major.minor.patch`.
    pub fn native_version() -> PhpResult<String> {
        guard(|| Ok(env!("CARGO_PKG_VERSION").to_owned()))
    }
}
