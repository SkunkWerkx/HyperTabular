//! Forward-only parsing for CSV, TSV, and arbitrary ASCII-delimited text: every cell
//! comes out as a HyperCast verdict.
//!
//! This module is the Rust binding. The reading itself is the allocation-free core's
//! ([`crate::kernel::delimited`]), which owns no memory; what is here owns the buffers and
//! the files, as a binding in any other language does.
//!
//! - [`Dialect`] — the caller-declared dialect: one ASCII separator byte, quoting on/off,
//!   header yes/no, blank-line skipping. Nothing is sniffed.
//! - [`BatchReader`] — typed, column-major batches through the core's `fill`: the fast
//!   path, and the shape every other binding has.
//! - [`Reader`] — the row-at-a-time cursor over a slice (zero-copy), a `Read`, or a path;
//!   [`Reader::next_row`] hands out a [`Row`] of byte slices that lives until the next call.
//! - [`Error`] — structural failures (a torn row, EOF inside quotes, an oversized row):
//!   these are never cell verdicts.
//! - [`engine`] / [`scan`] — the 64-byte-block SIMD scanner underneath, documented for the
//!   curious and for the benchmarks.
//!
//! ```
//! use hypertabular::delimited::{BatchReader, Dialect, Reader};
//! use hypertabular::{Batch, Column, Door, Plan};
//!
//! let text = b"id,name,score\n1,alice,2.5\n2,\"bob, jr\",\"3\"\n";
//!
//! // Typed batches through HyperCast's doors, column-major.
//! let plan = Plan::new(vec![Column::new(0, Door::I32), Column::new(2, Door::F64)]);
//! let mut batches = BatchReader::from_slice(text, Dialect::CSV, plan).unwrap();
//! assert_eq!(batches.header().unwrap().ordinal(b"score"), Some(2));
//! let mut batch = Batch::new();
//! assert_eq!(batches.fill(&mut batch, 1024).unwrap(), 2);
//! assert_eq!(batch.column(0).i32s().unwrap(), &[1, 2]);
//! assert_eq!(batch.column(1).f64s().unwrap(), &[2.5, 3.0]);
//!
//! // Or row at a time: borrowed bytes, no copies.
//! let mut reader = Reader::from_slice(text, Dialect::CSV).unwrap();
//! let row = reader.next_row().unwrap().unwrap();
//! assert_eq!(row.get(1), Some(&b"alice"[..]));
//! ```

mod batches;
mod dialect;
pub mod engine;
mod error;
mod reader;
pub mod scan;
mod unescape;

pub use batches::BatchReader;
pub use dialect::Dialect;
pub use error::Error;
pub use reader::{Reader, Row};
pub use unescape::unescape;
