//! Forward-only tabular parsing — delimited text and spreadsheets — with a HyperCast
//! verdict for every cell.
//!
//! The crate is two layers. [`kernel`] is the core: `no_std`, allocation-free and
//! panic-free, it fills buffers its caller owns and is what the native library
//! (`libhypertabular`) exports. Everything else here is the Rust binding over it — the
//! same thing each language's binding is, written in Rust: it owns the memory, reads the
//! files, grows a buffer when the core asks, and hands out what the core wrote.
//!
//! ```no_run
//! use hypertabular::{Column, DelimitedReader, Dialect, Workbook, SheetOptions};
//!
//! // Open, read the header, then bind the plan: which source column, through which door.
//! let mut reader = DelimitedReader::open_unbound("data.csv", Dialect::CSV)?;
//! let header = reader.header().unwrap();
//! let plan = [
//!     Column::i64(header.require("id")?),
//!     Column::f64(header.require("amount")?),
//!     Column::text(header.require("name")?),
//! ];
//! reader.bind(&plan)?;
//! while let Some(batch) = reader.read()? {
//!     let ids: &[i64] = batch.i64(0);                  // a whole column, as the core wrote it
//!     let verdicts = batch.verdicts(0);                // and a verdict beside each value
//!     for row in batch {                               // or a row at a time
//!         let amount: Result<f64, _> = row.get(1);     // one cell, as HyperCast judged it
//!         let name = row.text(2);
//!     }
//! }
//!
//! let book = Workbook::open("data.xlsx")?;
//! let mut sheet = book.sheet("Data", SheetOptions::default(), &plan)?;
//! while let Some(batch) = sheet.read()? { /* the same batch */ }
//! # Ok::<(), hypertabular::Error>(())
//! ```
//!
//! - [`Column`] — one output column: a source ordinal, a [`Door`], a numeric format. A
//!   plan is a slice of them.
//! - [`DelimitedReader`] and [`Dialect`]; [`Workbook`], [`Sheet`] and [`SheetOptions`].
//! - [`Header`] — the names a source declared, and the ordinal each is at.
//! - [`Batch`] — the rows of one read, column-major, lent by the reader that owns them;
//!   [`Row`] — one of them, read across.
//! - [`Error`] — the read, the caller's declarations, or a structural [`Failure`]. A cell
//!   that does not cast is never an error: it is a [`Fault`] in the batch.
//!
//! Nothing here sniffs, infers, or guesses: the caller declares the doors, the number
//! formats, the header, and the dialect. The design record is `docs/design.md`.

#![cfg_attr(not(feature = "std"), no_std)]

// The panic handler for the no_std libraries this crate links itself (`cargo staticlib`,
// `cargo cdylib`), built with `panic = "abort"`. No export can reach it — that is what the
// no-panic proof says — but a no_std artifact has to name one. HyperCast's lib.rs has the
// whole account of this and of the two blocks after it; the mechanics are identical.
#[cfg(all(feature = "staticlib", not(feature = "std")))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch = "wasm32"))]
    {
        unsafe extern "C" {
            safe fn abort() -> !;
        }
        abort()
    }
}

// A no_std shared library is its own final link, so it has to define the personality
// routine the precompiled `core` names — hidden, so that it is not one more export.
#[cfg(all(feature = "cdylib", not(feature = "std"), target_vendor = "apple"))]
core::arch::global_asm!(
    ".globl _rust_eh_personality",
    ".private_extern _rust_eh_personality",
    "_rust_eh_personality:",
    "ret",
);
#[cfg(all(
    feature = "cdylib",
    not(feature = "std"),
    not(target_vendor = "apple"),
    not(target_os = "windows"),
    not(target_arch = "wasm32"),
))]
core::arch::global_asm!(
    ".globl rust_eh_personality",
    ".hidden rust_eh_personality",
    ".type rust_eh_personality, %function",
    "rust_eh_personality:",
    "ret",
);

// And it has to name the C runtime std would have named for it.
#[cfg(all(feature = "cdylib", not(feature = "std"), unix))]
#[link(name = "c")]
unsafe extern "C" {}
#[cfg(all(feature = "cdylib", not(feature = "std"), target_env = "msvc"))]
#[cfg_attr(target_feature = "crt-static", link(name = "libcmt"))]
#[cfg_attr(not(target_feature = "crt-static"), link(name = "msvcrt"))]
unsafe extern "C" {}

pub mod kernel;

#[cfg(feature = "std")]
mod batch;
#[cfg(feature = "std")]
mod column;
#[cfg(feature = "std")]
pub mod delimited;
#[cfg(feature = "std")]
mod error;
#[cfg(feature = "std")]
mod header;
#[cfg(feature = "php")]
mod php_ext;
#[cfg(feature = "std")]
pub mod workbook;

// The Python binding over the core. A binding, not part of the core: it allocates, it
// uses std, and it is compiled only into the extension module.
#[cfg(feature = "python")]
mod python_ext;

// The Ruby binding's Magnus backend, on the same terms: a binding, compiled only into the
// extension.
#[cfg(feature = "ruby")]
mod ruby_ext;

#[cfg(feature = "std")]
pub use hypercast::{
    CivilDateTime, CurrencySymbol, Date, DateOrder, Decimal, Duration, ExcelEpoch, Fault,
    NumFormat, Reason, Timestamp, UnixPrecision,
};

pub use kernel::abi::{CellVerdict, Span};
pub use kernel::door::Door;

#[cfg(feature = "std")]
pub use batch::{Batch, Row, Rows, Value};
#[cfg(feature = "std")]
pub use column::Column;
#[cfg(feature = "std")]
pub use delimited::{DelimitedOptions, DelimitedReader, Dialect};
#[cfg(feature = "std")]
pub use error::{Error, Failure, FailureKind};
#[cfg(feature = "std")]
pub use header::Header;
#[cfg(feature = "std")]
pub use workbook::{Format, Sheet, SheetInfo, SheetOptions, SheetRef, Workbook};
