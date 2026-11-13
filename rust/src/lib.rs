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
//! let mut sheet = book.sheet_unbound("Data", SheetOptions::default())?;
//! let header = sheet.header().unwrap();
//! sheet.bind(&[Column::decimal(header.require("amount")?)])?;
//! while let Some(batch) = sheet.read()? {
//!     let amounts = batch.values::<hypertabular::Decimal>(0);
//!     let verdicts = batch.verdicts(0);
//! }
//! # Ok::<(), hypertabular::Error>(())
//! ```
//!
//! - [`Column`] — one output column: a source ordinal, a [`Door`], a numeric format. A
//!   plan is a slice of them.
//! - [`DelimitedReader`] and [`Dialect`]; [`Workbook`], [`Sheet`], [`SheetOptions`] and
//!   [`Range`].
//! - [`Header`] — the names a source declared, and the ordinal each is at:
//!   [`Header::require`] refuses a name two columns share, and [`Header::ordinal`] keeps the
//!   first.
//! - [`Batch`] — the rows of one read, column-major, lent by the reader that owns them, with what
//!   failed in a cell ([`Batch::detail`]), where its verdict came from ([`Batch::origin`]) and
//!   what went wrong in the batch ([`Batch::faults`]); [`Row`] — one of them, read across.
//! - [`DelimitedReader::suggest`] and [`Sheet::suggest`] — what a sample's columns look like, as
//!   a plan to look at and pass to a read: never applied, since nothing here sniffs.
//! - [`Error`] — the read, the caller's declarations, or a structural [`Failure`]. A cell
//!   that does not cast is never an error: it is a [`Fault`] in the batch.
//!
//! Nothing here sniffs, infers, or guesses: the caller declares the doors, the number
//! formats, the header, and the dialect. The design record is `docs/design.md`.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(all(windows, feature = "php"), feature(abi_vectorcall))]

// The panic handler for the no_std libraries this crate links itself (`cargo staticlib`,
// `cargo cdylib`), built with `panic = "abort"`. No export can reach it — that is what the
// no-panic proof says — but a no_std artifact has to name one. HyperCast's lib.rs has the
// whole account of this and of the blocks after it; the mechanics are identical.
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

// The static library's half: the same reference, left for the final link, fails it
// ("undefined reference to `rust_eh_personality'") whenever the program pulls in an
// intrinsic whose unwind entry names it (the 128-bit division helpers) and the linker keeps
// unwind tables of unused code (no --gc-sections / -dead_strip, so a plain `cc app.c
// libhypertabular.a`). A weak hidden definition resolves it, and two Hyper* archives (or a
// Rust program with std's own, strong, definition) linked into one program coalesce to one
// instead of colliding.
#[cfg(all(
    feature = "staticlib",
    not(feature = "cdylib"),
    not(feature = "std"),
    target_vendor = "apple"
))]
core::arch::global_asm!(
    ".globl _rust_eh_personality",
    ".weak_definition _rust_eh_personality",
    ".private_extern _rust_eh_personality",
    "_rust_eh_personality:",
    "ret",
);
#[cfg(all(
    feature = "staticlib",
    not(feature = "cdylib"),
    not(feature = "std"),
    not(target_vendor = "apple"),
    not(target_os = "windows"),
    not(target_arch = "wasm32"),
))]
core::arch::global_asm!(
    ".weak rust_eh_personality",
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
#[cfg(feature = "std")]
mod kept;
#[cfg(feature = "php")]
mod php_ext;
#[cfg(feature = "std")]
mod suggest;
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
    Checked, CivilDateTime, Constraints, CurrencySymbol, Date, DateOptions, DateOrder, Decimal,
    Detail, Duration, ExcelEpoch, Fault, Grouping, Keyword, MonthNames, NativeDigits, NumFormat,
    NumericRules, Reason, Rejection, Rounding, Symbol, Text, TextRules, Timestamp, UnixPrecision,
    Violation,
};

pub use kernel::abi::{CellNote, CellVerdict, RawCheck, Span};
pub use kernel::door::Door;

#[cfg(feature = "std")]
pub use batch::{Batch, Faults, Origin, Problem, Row, Rows, Value};
#[cfg(feature = "std")]
pub use column::{CheckedDoor, Column};
#[cfg(feature = "std")]
pub use delimited::{DelimitedOptions, DelimitedReader, Dialect, Encoding, LongRows, ShortRows};
#[cfg(feature = "std")]
pub use error::{Error, Failure, FailureKind};
#[cfg(feature = "std")]
pub use header::Header;
#[cfg(feature = "std")]
pub use kept::{KeptBatch, KeptColumn, KeptFaults};
#[cfg(feature = "std")]
pub use suggest::{Candidate, SuggestOptions, SuggestStatus, SuggestedColumn, Suggestion};
#[cfg(feature = "std")]
pub use workbook::{Format, Range, Sheet, SheetInfo, SheetOptions, SheetRef, Workbook};

// Internal tests can exercise parser machinery without publishing it to consumers.
#[cfg(all(test, feature = "std"))]
extern crate self as hypertabular;
#[cfg(all(test, feature = "std"))]
#[path = "../tests/internal.rs"]
mod tests;
