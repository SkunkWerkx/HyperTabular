//! `Tabular`, the binding's own facts, and `Scratch`, the test suite's hook into a workbook's
//! buffers.

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

/// The test suite's hook into a workbook's buffers: with `$stingy` set, every buffer a workbook
/// or a sheet is opened with starts with room for one element, so each read proves the core's
/// grow-and-repeat protocol; `$grown` counts the buffers grown — window, arena, cells.
///
/// @internal
#[php_class]
#[php(name = "HyperTabular\\Scratch")]
#[php(flags = ext_php_rs::flags::ClassFlags::Final)]
#[allow(dead_code)]
pub struct Scratch {
    /// Whether every workbook buffer starts with room for one element.
    #[php(prop, static, default = false)]
    stingy: bool,
    /// How many times a workbook call grew its window, its arena and its cell table.
    ///
    /// @var array{int, int, int}|null
    #[php(prop, static)]
    grown: Option<i64>,
}

#[php_impl]
impl Scratch {
    /// A test hook; not constructible.
    #[php(vis = "private")]
    pub fn __construct() -> PhpResult<Self> {
        guard(|| {
            Err(PhpException::new(
                "HyperTabular\\Scratch is a test hook".into(),
                0,
                ce::error(),
            ))
        })
    }
}
