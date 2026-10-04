//! Forward-only spreadsheet parsing — XLSX and ODS — as a [`crate`] provider: every
//! cell reaches HyperCast's doors as the typed value the file stores (a double, a
//! boolean, a wall-clock date, a shared string), never as re-rendered text, and the batch
//! crosses the FFI boundary once per chunk.
//!
//! - [`Workbook`] — opens a container from a path, a byte slice, or any [`Source`];
//!   detects XLSX or ODS from the container itself (never the extension), lists sheets.
//! - [`Sheet`] — a forward-only cursor over one worksheet, independent of the workbook
//!   once opened; [`Sheet::next_row`] hands out a [`Row`] valid until the next call.
//! - [`SheetOptions`] — header yes/no, empty-row skipping. Nothing else is guessed.
//! - [`Error`] — structural failures (a broken container, a torn part): never a verdict.
//! - [`zip`] / [`xml`] — the streaming container and tokenizer underneath.
//! - [`ffi`] — the C-ABI exports the seven bindings call.
//!
//! ```no_run
//! use hypertabular::workbook::{SheetOptions, Workbook};
//! use hypertabular::{Batch, Column, Door, Plan, fill_batch};
//!
//! let workbook = Workbook::from_path("orders.xlsx").unwrap();
//! let mut sheet = workbook.sheet(0, SheetOptions::default()).unwrap();
//! let plan = Plan::new(vec![Column::new(0, Door::I64), Column::new(3, Door::Date)]);
//! let mut batch = Batch::new();
//! while fill_batch(&mut sheet, &plan, &mut batch, 1024).unwrap() > 0 {
//!     // batch.column(1).dates() — Excel serials already read under the workbook's date system
//! }
//! ```

mod error;
pub mod ffi;
mod iso;
mod ods;
mod sheet;
mod source;
// The module is named for its type; the directory is named for the format.
#[allow(clippy::module_inception)]
mod workbook;
pub mod xlsx;
pub mod xml;
pub mod zip;

pub use error::Error;
pub use sheet::{Row, Sheet, SheetOptions};
pub use source::{FileSource, Source};
pub use workbook::{Format, SheetInfo, Workbook};
