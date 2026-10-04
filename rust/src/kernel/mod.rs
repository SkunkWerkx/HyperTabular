//! The core: everything a binding calls, and nothing a binding has to trust.
//!
//! Three properties hold for every line under this directory, and each is enforced by a
//! tool rather than by care (`rust/check-core.sh` runs them all; `docs/design.md` has the
//! reasoning):
//!
//! - **No standard library.** The shipped library is this code built with `std` off, where
//!   naming `std` does not compile.
//! - **No allocation.** The crate never declares `alloc`, so no collection type exists to
//!   name; every byte of working memory is the caller's, passed in. The library that
//!   results imports no allocator, and that import list is checked.
//! - **No panic.** Every export is declared through [`exports`]' one macro, which puts it
//!   under the no-panic proof; a link fails, naming the export, if any path in it can.
//!
//! The same code is compiled into every build — there is no `cfg(feature)` here other
//! than the two in that macro — so what the proofs say of the shipped library they say of
//! the core wherever it is linked. Collections, files, and anything else a host language
//! does better itself belong to the layer above.

pub mod abi;
pub mod delimited;
pub mod door;
pub mod exports;
