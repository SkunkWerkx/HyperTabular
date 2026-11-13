//! Tests of private core implementation details; allocation proofs remain isolated integration tests.

// The two workbook suites share these, and a file is loaded as a module once.
#[path = "support/packages.rs"]
mod packages;
#[path = "support/workbook.rs"]
mod support;

mod delimited_reach;
mod delimited_windows;
mod kernel_checked;
mod kernel_delimited;
mod kernel_inflate;
mod kernel_workbook;
mod kernel_workbook_rules;
mod suggest_batches;
mod suggest_candidates;
