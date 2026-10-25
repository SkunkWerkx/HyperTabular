//! Delimited text — CSV, TSV, any single-byte ASCII separator — read in the caller's
//! memory: `engine` classifies 64 bytes at a time, `scan` walks the result into cells
//! and rows, `unescape` is the one rule for a quoted cell, and [`fill`] is what a
//! binding calls.

pub(crate) mod engine;
pub mod fill;
pub(crate) mod scan;
pub(crate) mod unescape;
