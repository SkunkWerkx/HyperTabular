//! The C ABI — every symbol the library exports, and the only place one is declared.
//!
//! [`export!`] is the one way to make an export, and it does two things no export may be
//! without: it names the symbol (under the `exports` feature) and it puts the function
//! under the no-panic proof (under the `no-panic` feature). `check-core.sh` holds the
//! other end: `no_mangle` appears nowhere else in the core, and the shipped library's
//! export list is exactly the functions declared here.
//!
//! Return codes are [`crate::kernel::abi`]'s. Every pointer is the caller's memory for the
//! length of the call and no longer; a null where the contract needs memory is
//! [`ERR_CONTRACT`], and a length of `0` never dereferences its pointer.

use crate::kernel::abi::{ColumnBuffer, ColumnSpec, ERR_CONTRACT, Filled, OK, Span};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::kernel::delimited::unescape::unescape_into;
use core::slice;

macro_rules! export {
    ($(#[$meta:meta])* fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        $(#[$meta])*
        #[cfg_attr(feature = "exports", unsafe(no_mangle))]
        #[cfg_attr(feature = "no-panic", no_panic::no_panic)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret $body
    };
}

/// # Safety
/// `ptr` points to `len` live bytes when `len > 0`.
unsafe fn bytes<'caller>(ptr: *const u8, len: usize) -> &'caller [u8] {
    if len == 0 || ptr.is_null() {
        &[]
    } else {
        // SAFETY: per the function contract.
        unsafe { slice::from_raw_parts(ptr, len) }
    }
}

/// # Safety
/// `ptr` points to `len` live, writable `T`s when `len > 0`, aligned for `T`.
unsafe fn buffer<'caller, T>(ptr: *mut T, len: usize) -> &'caller mut [T] {
    if len == 0 || ptr.is_null() {
        &mut []
    } else {
        // SAFETY: per the function contract.
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }
}

export! {
    /// This library's version, packed `major << 16 | minor << 8 | patch` from the crate's
    /// own manifest: the probe every binding makes before its first real call.
    ///
    /// # Safety
    /// None to uphold; `unsafe` only because every export is declared one way.
    fn hypertabular_version() -> u32 {
        const fn field(text: &str) -> u32 {
            let bytes = text.as_bytes();
            let mut value = 0u32;
            let mut i = 0;
            while i < bytes.len() {
                value = value * 10 + (bytes[i] - b'0') as u32;
                i += 1;
            }
            value
        }
        const VERSION: u32 = (field(env!("CARGO_PKG_VERSION_MAJOR")) << 16)
            | (field(env!("CARGO_PKG_VERSION_MINOR")) << 8)
            | field(env!("CARGO_PKG_VERSION_PATCH"));
        VERSION
    }
}

export! {
    /// The size of a delimited state block, for a binding that allocates it as bytes. It
    /// wants 8-byte alignment.
    ///
    /// # Safety
    /// None to uphold.
    fn hypertabular_delimited_state_size() -> usize {
        size_of::<State>()
    }
}

export! {
    /// Starts a delimited input: writes a fresh state to `state` for the dialect at
    /// `dialect`. [`ERR_CONTRACT`] for a null, a separator the scanner cannot honour, or
    /// an engine this CPU cannot run.
    ///
    /// # Safety
    /// `state` points to `hypertabular_delimited_state_size()` writable bytes, 8-byte
    /// aligned; `dialect` to a live `RawDialect`.
    fn hypertabular_delimited_init(state: *mut State, dialect: *const RawDialect) -> i32 {
        if state.is_null() || dialect.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        match State::init(unsafe { dialect.read_unaligned() }) {
            Some(fresh) => {
                // SAFETY: per the function contract.
                unsafe { state.write(fresh) };
                OK
            }
            None => ERR_CONTRACT,
        }
    }
}

export! {
    /// Reads the next record as a header. See [`fill::header`].
    ///
    /// # Safety
    /// `state` was initialized by `hypertabular_delimited_init`; `input` is `input_len`
    /// bytes; `names` has room for `names_cap` spans; `arena` for `arena_cap` bytes;
    /// `out` is writable.
    fn hypertabular_delimited_header(
        state: *mut State,
        input: *const u8,
        input_len: usize,
        last: u32,
        names: *mut Span,
        names_cap: usize,
        arena: *mut u8,
        arena_cap: usize,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            fill::header(
                &mut *state,
                bytes(input, input_len),
                last != 0,
                buffer(names, names_cap),
                buffer(arena, arena_cap),
                &mut *out,
            )
        }
    }
}

export! {
    /// Reads up to `max_rows` whole rows into the caller's column buffers. See
    /// [`fill::fill`].
    ///
    /// # Safety
    /// `state` was initialized by `hypertabular_delimited_init`; `input` is `input_len`
    /// bytes; `specs` and `columns` are `column_count` entries each, and each column's
    /// two arrays have room for `max_rows` elements; `cells` has room for `cells_cap`
    /// spans; `arena` for `arena_cap` bytes; `out` is writable.
    fn hypertabular_delimited_fill(
        state: *mut State,
        input: *const u8,
        input_len: usize,
        last: u32,
        specs: *const ColumnSpec,
        columns: *const ColumnBuffer,
        column_count: usize,
        max_rows: usize,
        cells: *mut Span,
        cells_cap: usize,
        arena: *mut u8,
        arena_cap: usize,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null()
            || out.is_null()
            || (column_count > 0 && (specs.is_null() || columns.is_null()))
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            fill::fill(
                &mut *state,
                bytes(input, input_len),
                last != 0,
                specs,
                columns,
                max_rows,
                buffer(cells, cells_cap),
                buffer(arena, arena_cap),
                &mut *out,
            )
        }
    }
}

export! {
    /// Unescapes one quoted cell — a cell-table entry with its flag set names one — into
    /// `out`, and returns the bytes written: never more than `len - 1`, and no more than
    /// `cap`.
    ///
    /// # Safety
    /// `cell` is `len` bytes; `out` has room for `cap`.
    fn hypertabular_delimited_unescape(
        cell: *const u8,
        len: usize,
        out: *mut u8,
        cap: usize,
    ) -> usize {
        // SAFETY: per the function contract.
        unsafe { unescape_into(bytes(cell, len), buffer(out, cap)) }
    }
}

// What the proof must refuse. Compiled only when `check-core.sh` asks, to watch it fail: a
// proof that has never rejected anything has not been shown to check anything.
#[cfg(hypertabular_canary)]
export! {
    /// Can panic, on purpose.
    ///
    /// # Safety
    /// None to uphold.
    fn hypertabular_canary(value: u32) -> u32 {
        assert!(value != 12_345, "the canary");
        value
    }
}
