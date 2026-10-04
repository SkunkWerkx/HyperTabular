//! Forward-only parsing for CSV, TSV, and arbitrary ASCII-delimited text, as a
//! [`crate`] provider: every cell comes out as a HyperCast verdict, and the batch
//! crosses the FFI boundary once per chunk.
//!
//! - [`Dialect`] — the caller-declared dialect: one ASCII separator byte, quoting on/off,
//!   header yes/no, blank-line skipping. Nothing is sniffed.
//! - [`Reader`] — the forward-only cursor over a slice (zero-copy), a `Read`, or a path;
//!   [`Reader::next_row`] hands out a [`Row`] of byte slices that lives until the next call.
//! - [`Error`] — structural failures (a torn row, EOF inside quotes, an oversized row):
//!   these are never cell verdicts.
//! - [`engine`] / [`scan`] — the 64-byte-block SIMD scanner underneath, documented for the
//!   curious and for the benchmarks.
//!
//! What a binding calls is not here but in the allocation-free core
//! ([`crate::kernel::delimited::fill`]), which this reader shares its scanner with.
//!
//! ```
//! use hypertabular::delimited::{Dialect, Reader};
//! use hypertabular::{Batch, Column, Door, Plan, fill_batch};
//!
//! let text = b"id,name,score\n1,alice,2.5\n2,\"bob, jr\",\"3\"\n";
//! let mut reader = Reader::from_slice(text, Dialect::CSV).unwrap();
//! assert_eq!(reader.header().unwrap().ordinal(b"score"), Some(2));
//!
//! // Row at a time: borrowed bytes, no copies.
//! let row = reader.next_row().unwrap().unwrap();
//! assert_eq!(row.get(1), Some(&b"alice"[..]));
//!
//! // Or a typed batch through HyperCast's doors, column-major.
//! let plan = Plan::new(vec![Column::new(0, Door::I32), Column::new(2, Door::F64)]);
//! let mut batch = Batch::new();
//! fill_batch(&mut reader, &plan, &mut batch, 1024).unwrap();
//! assert_eq!(batch.column(0).i32s().unwrap(), &[2]);
//! assert_eq!(batch.column(1).f64s().unwrap(), &[3.0]);
//! ```

mod dialect;
pub mod engine;
mod error;
mod reader;
pub mod scan;
mod unescape;

pub use dialect::Dialect;
pub use error::Error;
pub use reader::{Reader, Row};
pub use unescape::unescape;
