//! The C ABI — every symbol the library exports, and the only place one is declared.
//!
//! `export!` is the one way to make an export, and it does two things no export may be
//! without: it names the symbol (under the `exports` feature) and it puts the function
//! under the no-panic proof (under the `no-panic` feature). `check-core.sh` holds the
//! other end: `no_mangle` appears nowhere else in the core, and the shipped library's
//! export list is exactly the functions declared here.
//!
//! Return codes are [`crate::kernel::abi`]'s. Every pointer is the caller's memory for the
//! length of the call and no longer; a null where the contract needs memory is
//! [`ERR_CONTRACT`], and a length of `0` never dereferences its pointer.

use crate::kernel::abi::{
    Buffers, BuffersWith, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith, ERR_CONTRACT,
    Filled, OK, Opened, RawBatchCheck, RawDelimitedOptions, RawProblem, RawRange, RawSheetOptions,
    RawSuggestion, RawTranscoded, Span,
};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::faults::faults;
use crate::kernel::size::check_batch;
use crate::kernel::suggest::suggest;
use crate::kernel::transcode::transcode;
use crate::kernel::workbook::{Memory, State as Workbook, book, rows};
use core::slice;
use hypercast::RawNumFormat;

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
    /// wants 8-byte alignment. A block only ever given to `hypertabular_delimited_init` may be
    /// 64 bytes, all that this function reported before `hypertabular_delimited_init_with`;
    /// the core doesn't look past them.
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
                // SAFETY: per the function contract; this state is its first 64 bytes.
                unsafe { fresh.store(state) };
                OK
            }
            None => ERR_CONTRACT,
        }
    }
}

export! {
    /// Starts a delimited input with options the dialect alone can't say: what to pass over,
    /// and what to do about a record of the wrong width. [`ERR_CONTRACT`] for a null, a `size`
    /// the core doesn't read, what `hypertabular_delimited_init` refuses, or a policy that
    /// isn't one; nothing is written then.
    ///
    /// # Safety
    /// `state` points to `hypertabular_delimited_state_size()` writable bytes, 8-byte
    /// aligned; `options` to a live `RawDelimitedOptions`, or at least its `size`.
    fn hypertabular_delimited_init_with(
        state: *mut State,
        options: *const RawDelimitedOptions,
    ) -> i32 {
        if state.is_null() || options.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: the size first, and the rest once it says so.
        unsafe {
            if options.cast::<u32>().read_unaligned() != RawDelimitedOptions::SIZE {
                return ERR_CONTRACT;
            }
            match State::init_with(&options.read_unaligned()) {
                Some(fresh) => {
                    fresh.store(state);
                    OK
                }
                None => ERR_CONTRACT,
            }
        }
    }
}

export! {
    /// Decodes a declared encoding to UTF-8, for a binding to run ahead of the fill: `encoding`
    /// is [`RawTranscoded::WINDOWS_1252`], [`RawTranscoded::LATIN_1`] or
    /// [`RawTranscoded::UTF_16LE`]. Stateless: a code unit the input cuts off stays unconsumed,
    /// to hand over again in front of what follows it, until `last` is nonzero. A unit with no
    /// character returns [`OK`] with `failed` set in `progress`, and the caller raises
    /// `Failure::ENCODING` once every intact row ahead of it is delivered. See
    /// [`transcode`], and `docs/delimited.md` for the reader's side.
    /// [`ERR_CONTRACT`] for an `encoding` that isn't one, a null `progress`, or a null `input` or
    /// `out` with a length; nothing is written then.
    ///
    /// # Safety
    /// `input` is `input_len` bytes; `out` is `cap` writable bytes that don't overlap them;
    /// `progress` is writable, at any alignment.
    fn hypertabular_transcode(
        encoding: u32,
        input: *const u8,
        input_len: usize,
        last: u32,
        out: *mut u8,
        cap: usize,
        progress: *mut RawTranscoded,
    ) -> i32 {
        if progress.is_null() || (input.is_null() && input_len != 0) || (out.is_null() && cap != 0) {
            return ERR_CONTRACT;
        }
        let mut done = RawTranscoded::default();
        // SAFETY: per the function contract.
        unsafe {
            let code = transcode(
                encoding,
                bytes(input, input_len),
                last != 0,
                buffer(out, cap),
                &mut done,
            );
            if code == OK {
                progress.write_unaligned(done);
            }
            code
        }
    }
}

export! {
    /// Reads the next record as a header. See [`fill::header`].
    ///
    /// # Safety
    /// `state` was initialized by `hypertabular_delimited_init` or
    /// `hypertabular_delimited_init_with`; `input` is `input_len` bytes; `names` has room for
    /// `names_cap` spans; `arena` for `arena_cap` bytes; `out` is writable.
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
            let mut held = State::load(state);
            let code = fill::header(
                &mut held,
                bytes(input, input_len),
                last != 0,
                buffer(names, names_cap),
                buffer(arena, arena_cap),
                &mut *out,
            );
            held.store(state);
            code
        }
    }
}

export! {
    /// Reads up to `max_rows` whole rows into the caller's column buffers. See
    /// [`fill::fill`]; a format's flags are read as 1.0 read them, ignoring every bit from 8 up, so
    /// every format
    /// reads Western.
    ///
    /// # Safety
    /// `state` was initialized by `hypertabular_delimited_init` or
    /// `hypertabular_delimited_init_with`; `input` is `input_len` bytes; `specs` and `columns`
    /// are `column_count` entries each, and each column's two arrays have room for `max_rows`
    /// elements; `buffers` is readable, and its `cells` and `arena` hold `cells_cap` spans and `arena_cap` bytes (the rest of it is
    /// not read); `out` is writable.
    ///
    /// The two buffers come in a [`Buffers`] rather than as four arguments of their own,
    /// as the workbook fill takes them: Mono's interpreter, which runs .NET in the browser
    /// and in an iOS debug build, passes no more than twelve integer arguments to a native
    /// function, and this one had thirteen.
    fn hypertabular_delimited_fill(
        state: *mut State,
        input: *const u8,
        input_len: usize,
        last: u32,
        specs: *const ColumnSpec,
        columns: *const ColumnBuffer,
        column_count: usize,
        max_rows: usize,
        buffers: *const Buffers,
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
            if buffers.is_null() {
                return ERR_CONTRACT;
            }
            let buffers = buffers.read_unaligned();
            if !buffers.cells.is_aligned() {
                return ERR_CONTRACT;
            }
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            let mut held = State::load(state);
            let code = fill::fill_1_0(
                &mut held,
                bytes(input, input_len),
                last != 0,
                specs,
                columns,
                max_rows,
                buffer(buffers.cells, buffers.cells_cap),
                buffer(buffers.arena, buffers.arena_cap),
                &mut *out,
            );
            held.store(state);
            code
        }
    }
}

export! {
    /// [`hypertabular_delimited_fill`] with the column options, tokens, notes and row widths the
    /// size-tagged shapes hold. See [`fill::fill_with`]. Every `size` is read first and must be
    /// the one this core knows; [`ERR_CONTRACT`] writes nothing, `out` and `state` included.
    ///
    /// # Safety
    /// `state` was initialized by `hypertabular_delimited_init` or
    /// `hypertabular_delimited_init_with`; `input` is `input_len` bytes; `specs` and `columns`
    /// are `column_count` aligned entries each (or as far as the first wrong `size`), and each
    /// column's arrays have room for `max_rows` elements; `buffers` is readable, or its `size`
    /// is, and its `cells`, `arena` and `tokens` hold what their lengths say, and `row_widths`
    /// `max_rows` counts, unless they're null (the rest of it is not read); `out` is writable.
    fn hypertabular_delimited_fill_with(
        state: *mut State,
        input: *const u8,
        input_len: usize,
        last: u32,
        specs: *const ColumnSpecWith,
        columns: *const ColumnBufferWith,
        column_count: usize,
        max_rows: usize,
        buffers: *const BuffersWith,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null()
            || out.is_null()
            || buffers.is_null()
            || (column_count > 0 && (specs.is_null() || columns.is_null()))
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: each size before what it says the layout is.
        unsafe {
            if buffers.cast::<u32>().read_unaligned() != BuffersWith::SIZE {
                return ERR_CONTRACT;
            }
            let buffers = buffers.read_unaligned();
            if !buffers.cells.is_aligned()
                || !buffers.row_widths.is_aligned()
                || (column_count > 0 && (!specs.is_aligned() || !columns.is_aligned()))
                || (buffers.tokens.is_null() && buffers.tokens_len != 0)
            {
                return ERR_CONTRACT;
            }
            let mut at = 0;
            while at < column_count {
                if specs.add(at).cast::<u32>().read() != ColumnSpecWith::SIZE
                    || columns.add(at).cast::<u32>().read() != ColumnBufferWith::SIZE
                {
                    return ERR_CONTRACT;
                }
                at += 1;
            }
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            let widths_cap = if buffers.row_widths.is_null() { 0 } else { max_rows };
            let mut held = State::load(state);
            let code = fill::fill_with(
                &mut held,
                bytes(input, input_len),
                last != 0,
                specs,
                columns,
                max_rows,
                buffer(buffers.cells, buffers.cells_cap),
                buffer(buffers.arena, buffers.arena_cap),
                bytes(buffers.tokens, buffers.tokens_len),
                buffer(buffers.row_widths, widths_cap),
                &mut *out,
            );
            if code != ERR_CONTRACT {
                held.store(state);
            }
            code
        }
    }
}

/// # Safety
/// `ptr` points to `len` live `u32`s when `len > 0`, aligned.
unsafe fn counts<'caller>(ptr: *const u32, len: usize) -> &'caller [u32] {
    if len == 0 || ptr.is_null() {
        &[]
    } else {
        // SAFETY: per the function contract.
        unsafe { slice::from_raw_parts(ptr, len) }
    }
}

export! {
    /// Lists what went wrong in a batch `hypertabular_delimited_fill_with` or
    /// `hypertabular_workbook_fill_with` wrote. See [`faults`] for what's listed, and in what order.
    ///
    /// Resumable: start `cursor` at `0`, and each call writes up to `cap` entries to `out`, says how
    /// many in `written`, and moves `cursor` past what it looked at. A call that writes fewer than
    /// `cap` has reached the end, and `cursor` is then `rows * (column_count + 1)`. Asked from there,
    /// it writes none. A `cap` of `0` looks at nothing: it says `0` and leaves `cursor` where it is,
    /// so a loop that waits for fewer than `cap` must not pass one.
    ///
    /// `long_rows` is the policy the read declared, [`RawDelimitedOptions`]' codes, and a workbook
    /// passes [`RawDelimitedOptions::LONG_ROWS_REJECT`]. Under
    /// [`RawDelimitedOptions::LONG_ROWS_REPORT`] `row_widths` is required, as the row widths of the
    /// fill, and `expected_width` is the width the first record fixed: `expected` in the state block.
    /// Otherwise neither is read.
    ///
    /// [`ERR_CONTRACT`] writes nothing, `cursor` and `written` included, for: a null `cursor` or
    /// `written`; a null `columns` with a `column_count`, or a null `out` with a `cap`; any of those
    /// misaligned; a column whose `size` this core doesn't know, or whose verdicts are null; a `cursor`
    /// past the end; `rows` or `column_count` past `u32::MAX`; a `long_rows` that isn't one of the
    /// codes; or, under a report, `row_widths` misaligned, a `rows` no array could hold, or no
    /// `row_widths`, which would be a list that leaves out what it was asked to name.
    ///
    /// A column whose notes are null still lists every refused cell, at `detail` `0`, but can't tell
    /// a formula with no cached value from a blank, so lists none. A workbook row that the empty-row
    /// filter left out isn't in the batch, so nothing here can name it: `skip_empty_rows` says when
    /// that happens.
    ///
    /// # Safety
    /// `columns` is `column_count` aligned entries (or as far as the first wrong `size`), each
    /// column's `verdicts` and `notes`, unless they're null, hold `rows` elements; `row_widths`, when
    /// required, holds `rows` counts; `out` is `cap` writable entries; `cursor` and `written` are
    /// writable and don't overlap them.
    fn hypertabular_faults(
        columns: *const ColumnBufferWith,
        column_count: usize,
        rows: usize,
        row_widths: *const u32,
        expected_width: u32,
        long_rows: u32,
        cursor: *mut u64,
        out: *mut RawProblem,
        cap: usize,
        written: *mut usize,
    ) -> i32 {
        let reported = long_rows == RawDelimitedOptions::LONG_ROWS_REPORT;
        if cursor.is_null()
            || !cursor.is_aligned()
            || written.is_null()
            || !written.is_aligned()
            || (out.is_null() && cap != 0)
            || !out.is_aligned()
            || (columns.is_null() && column_count != 0)
            || !columns.is_aligned()
            || u32::try_from(rows).is_err()
            || (reported && (!row_widths.is_aligned() || rows > isize::MAX as usize / 4))
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: each size before what it says the layout is.
        unsafe {
            let mut at = 0;
            while at < column_count {
                if columns.add(at).cast::<u32>().read() != ColumnBufferWith::SIZE {
                    return ERR_CONTRACT;
                }
                at += 1;
            }
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            let widths = if reported { counts(row_widths, rows) } else { &[] };
            let mut held = cursor.read();
            let mut count = 0;
            let code = faults(
                columns,
                rows,
                widths,
                expected_width,
                long_rows,
                &mut held,
                buffer(out, cap),
                &mut count,
            );
            if code == OK {
                cursor.write(held);
                written.write(count);
            }
            code
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

/// The buffers of a workbook call as the core takes them, or `None` for a null or for a
/// table that is not aligned for what it holds.
///
/// # Safety
/// `buffers` is null or a live [`Buffers`], and each of its pointers is null or points to
/// as many live elements as the size beside it says — writable ones, and no two of them
/// overlapping, for `window`, `arena`, `cells` and `row`.
unsafe fn memory<'caller>(buffers: *const Buffers) -> Option<Memory<'caller>> {
    if buffers.is_null() {
        return None;
    }
    // SAFETY: per the function contract.
    unsafe {
        let buffers = buffers.read_unaligned();
        if !buffers.cells.is_aligned() || !buffers.row.is_aligned() || !buffers.table.is_aligned() {
            return None;
        }
        Some(Memory {
            window: buffer(buffers.window, buffers.window_cap),
            arena: buffer(buffers.arena, buffers.arena_cap),
            cells: buffer(buffers.cells, buffers.cells_cap),
            row: buffer(buffers.row, buffers.row_cap),
            strings: bytes(buffers.strings, buffers.strings_len),
            table: if buffers.table_len == 0 || buffers.table.is_null() {
                &[]
            } else {
                slice::from_raw_parts(buffers.table, buffers.table_len)
            },
            kinds: bytes(buffers.kinds, buffers.kinds_len),
        })
    }
}

export! {
    /// The size of a workbook state block, for a binding that allocates it as bytes. It
    /// wants 8-byte alignment. One block reads one sheet at a time; a block that has been
    /// opened may be copied, bytes and all, to read another.
    ///
    /// # Safety
    /// None to uphold.
    fn hypertabular_workbook_state_size() -> usize {
        size_of::<Workbook>()
    }
}

export! {
    /// Opens the workbook in `container`: writes a fresh state and says what it found. See
    /// [`book::open`]. Uses `window` and `arena`.
    ///
    /// # Safety
    /// `state` points to `hypertabular_workbook_state_size()` writable bytes, 8-byte
    /// aligned; `container` is `container_len` bytes; `buffers` is a live [`Buffers`]
    /// whose pointers each hold what their sizes say; `out` is writable.
    fn hypertabular_workbook_open(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        buffers: *const Buffers,
        out: *mut Opened,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            book::open(&mut *state, bytes(container, container_len), &mut memory, &mut *out)
        }
    }
}

export! {
    /// Lists the workbook's sheets. See [`book::sheets`]. Uses `window`, `arena`, `cells`.
    ///
    /// # Safety
    /// `state` was opened by `hypertabular_workbook_open` over these same `container`
    /// bytes; `buffers` and `out` as there.
    fn hypertabular_workbook_sheets(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        buffers: *const Buffers,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            book::sheets(&mut *state, bytes(container, container_len), &mut memory, &mut *out)
        }
    }
}

export! {
    /// Loads the workbook's shared strings. See [`book::strings`]. Uses `window`, `arena`,
    /// `cells`.
    ///
    /// # Safety
    /// As `hypertabular_workbook_sheets`.
    fn hypertabular_workbook_strings(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        buffers: *const Buffers,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            book::strings(&mut *state, bytes(container, container_len), &mut memory, &mut *out)
        }
    }
}

export! {
    /// Loads the number-format kind of each cell format. See [`book::styles`]. Uses
    /// `window`, `arena`, `cells`.
    ///
    /// # Safety
    /// As `hypertabular_workbook_sheets`.
    fn hypertabular_workbook_styles(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        buffers: *const Buffers,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            book::styles(&mut *state, bytes(container, container_len), &mut memory, &mut *out)
        }
    }
}

export! {
    /// Positions the state on one sheet: the XLSX part named by the `part_len` bytes at
    /// `part`, or the ODS or Numbers table at `index` — both as `hypertabular_workbook_sheets`
    /// listed them. See [`rows::sheet`].
    ///
    /// # Safety
    /// `state` was opened by `hypertabular_workbook_open` over these same `container`
    /// bytes; `part` is `part_len` bytes; `out` is writable.
    fn hypertabular_workbook_sheet(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        part: *const u8,
        part_len: usize,
        index: u32,
        has_header: u32,
        skip_empty_rows: u32,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            rows::sheet(
                &mut *state,
                bytes(container, container_len),
                bytes(part, part_len),
                index,
                has_header != 0,
                skip_empty_rows != 0,
                &mut *out,
            )
        }
    }
}

export! {
    /// Reads the sheet's header row. See [`rows::header`]. Uses `window`, `arena`, `cells`,
    /// `row`, and the workbook's tables.
    ///
    /// # Safety
    /// `state` was positioned by `hypertabular_workbook_sheet` or
    /// `hypertabular_workbook_sheet_with` over these same `container` bytes; `buffers` and `out` as
    /// for `hypertabular_workbook_open`.
    fn hypertabular_workbook_header(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        buffers: *const Buffers,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            rows::header(&mut *state, bytes(container, container_len), &mut memory, &mut *out)
        }
    }
}

export! {
    /// Reads up to `max_rows` rows of the sheet into the caller's column buffers. See
    /// [`rows::fill`]; a format's flags are read as 1.0 read them, ignoring every bit from 8 up, so
    /// every format
    /// reads Western.
    /// Uses every buffer.
    ///
    /// # Safety
    /// `state` was positioned by `hypertabular_workbook_sheet` or
    /// `hypertabular_workbook_sheet_with` over these same `container` bytes; `specs` and `columns`
    /// are `column_count` entries each, and each column's two arrays have room for `max_rows` elements; `buffers` and `out` as for
    /// `hypertabular_workbook_open`.
    fn hypertabular_workbook_fill(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        specs: *const ColumnSpec,
        columns: *const ColumnBuffer,
        column_count: usize,
        max_rows: usize,
        buffers: *const Buffers,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null()
            || !state.is_aligned()
            || out.is_null()
            || (column_count > 0 && (specs.is_null() || columns.is_null()))
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            let Some(mut memory) = memory(buffers) else {
                return ERR_CONTRACT;
            };
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            rows::fill_1_0(
                &mut *state,
                bytes(container, container_len),
                specs,
                columns,
                max_rows,
                &mut memory,
                &mut *out,
            )
        }
    }
}

export! {
    /// [`hypertabular_workbook_sheet`] with the skip, header row and range [`RawSheetOptions`]
    /// holds. See [`rows::sheet_with`]. [`ERR_CONTRACT`] for a null, a `size` the core doesn't read
    /// or an inverted range writes nothing, `out` and `state` included.
    ///
    /// # Safety
    /// As `hypertabular_workbook_sheet`; `options` is readable, or its `size` is, at any alignment.
    fn hypertabular_workbook_sheet_with(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        part: *const u8,
        part_len: usize,
        index: u32,
        options: *const RawSheetOptions,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null() || !state.is_aligned() || out.is_null() || options.is_null() {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: the size first, and the rest once it says so.
        unsafe {
            if options.cast::<u32>().read_unaligned() != RawSheetOptions::SIZE {
                return ERR_CONTRACT;
            }
            rows::sheet_with(
                &mut *state,
                bytes(container, container_len),
                bytes(part, part_len),
                index,
                &options.read_unaligned(),
                &mut *out,
            )
        }
    }
}

export! {
    /// [`hypertabular_workbook_fill`] with the column options, tokens, notes and row widths the
    /// size-tagged shapes hold. See [`rows::fill_with`]. Every `size` is read first and must be
    /// the one this core knows; [`ERR_CONTRACT`] writes nothing, `out` and `state` included.
    ///
    /// # Safety
    /// `state` was positioned by `hypertabular_workbook_sheet` or
    /// `hypertabular_workbook_sheet_with` over these same `container` bytes; `specs` and `columns`
    /// are `column_count` aligned entries each (or as far as the first wrong `size`), and each
    /// column's arrays have room for `max_rows` elements; `buffers` is readable, or its `size` is,
    /// and each of its pointers holds what its size says, and `row_widths` `max_rows` counts,
    /// unless it's null; `out` is writable.
    fn hypertabular_workbook_fill_with(
        state: *mut Workbook,
        container: *const u8,
        container_len: usize,
        specs: *const ColumnSpecWith,
        columns: *const ColumnBufferWith,
        column_count: usize,
        max_rows: usize,
        buffers: *const BuffersWith,
        out: *mut Filled,
    ) -> i32 {
        if state.is_null()
            || !state.is_aligned()
            || out.is_null()
            || buffers.is_null()
            || (column_count > 0 && (specs.is_null() || columns.is_null()))
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: each size before what it says the layout is.
        unsafe {
            if buffers.cast::<u32>().read_unaligned() != BuffersWith::SIZE {
                return ERR_CONTRACT;
            }
            let buffers = buffers.read_unaligned();
            if !buffers.row_widths.is_aligned()
                || (column_count > 0 && (!specs.is_aligned() || !columns.is_aligned()))
                || (buffers.tokens.is_null() && buffers.tokens_len != 0)
            {
                return ERR_CONTRACT;
            }
            let mut at = 0;
            while at < column_count {
                if specs.add(at).cast::<u32>().read() != ColumnSpecWith::SIZE
                    || columns.add(at).cast::<u32>().read() != ColumnBufferWith::SIZE
                {
                    return ERR_CONTRACT;
                }
                at += 1;
            }
            let plain = buffers.buffers();
            let Some(mut memory) = memory(&plain) else {
                return ERR_CONTRACT;
            };
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let columns =
                if column_count == 0 { &[] } else { slice::from_raw_parts(columns, column_count) };
            let widths_cap = if buffers.row_widths.is_null() { 0 } else { max_rows };
            rows::fill_with(
                &mut *state,
                bytes(container, container_len),
                specs,
                columns,
                max_rows,
                &mut memory,
                bytes(buffers.tokens, buffers.tokens_len),
                buffer(buffers.row_widths, widths_cap),
                &mut *out,
            )
        }
    }
}

export! {
    /// Tries each column of a sample against the doors and says what it finds, in `out`, one entry
    /// a column. Nothing here touches a read: a suggestion is an answer, for the caller to look at
    /// and pass back in a plan of its own. See [`suggest`] for how a column is judged.
    ///
    /// `cells` is the sample by column: `rows` spans of the first column, then `rows` of the next,
    /// so `rows * columns` in all. A span's flag says its bytes are in `arena` and not in `input`,
    /// as a text value's does, and a blank span is a blank cell, the way to pass a cell whose fill
    /// verdict said empty: a declared token, padding or a formula with no cached value.
    ///
    /// `typed` is `rows * columns` bytes in the order of `cells`, or null for none. A byte that
    /// isn't `0` says the cell was a workbook's typed cell, which `cells` holds as the text door
    /// said it, so a typed number is judged by its value and asks nothing about notation. Delimited
    /// text passes null, and so can a workbook's caller who has no way to know, whose typed numbers
    /// are then judged as text. A sheet read says which: through a [`Door::Uuid`] beside the text
    /// door, with notes, a typed cell is `Malformed` with a detail of `0`.
    ///
    /// `formats` is `columns` entries, or null. Null, and an entry whose `size` is `0`, declare
    /// nothing: the invariant notation is read, with `,` left to [`suggest`]'s two candidates. Any
    /// other entry settles that column's notation.
    ///
    /// [`ERR_CONTRACT`] writes nothing, for: `rows * columns` past what a slice can be; `rows` past
    /// `u32::MAX`; a null `cells` or `out` with anything to hold, or a null `input` or `arena` with
    /// a length; `cells`, `formats` or `out` misaligned; a format HyperCast's doors would refuse; or
    /// a span reaching past the end of what it points into.
    ///
    /// # Safety
    /// `input` is `input_len` bytes and `arena` `arena_len`; `cells` is `rows * columns` spans;
    /// `typed`, unless it's null, is `rows * columns` bytes; `formats`, unless it's null, is `columns` formats; `out` is `columns` writable entries that
    /// don't overlap any of them.
    fn hypertabular_suggest(
        input: *const u8,
        input_len: usize,
        arena: *const u8,
        arena_len: usize,
        cells: *const Span,
        typed: *const u8,
        rows: usize,
        columns: usize,
        formats: *const RawNumFormat,
        out: *mut RawSuggestion,
    ) -> i32 {
        let Some(count) = rows.checked_mul(columns) else {
            return ERR_CONTRACT;
        };
        let room = |size: u32| columns <= isize::MAX as usize / size as usize;
        if u32::try_from(rows).is_err()
            || (input.is_null() && input_len != 0)
            || (arena.is_null() && arena_len != 0)
            || (cells.is_null() && count != 0)
            || !cells.is_aligned()
            || count > isize::MAX as usize / Span::SIZE as usize
            || !formats.is_aligned()
            || !room(RawNumFormat::SIZE)
            || (out.is_null() && columns != 0)
            || !out.is_aligned()
            || !room(RawSuggestion::SIZE)
        {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract, each count held to what a slice can be just above.
        unsafe {
            let cells = if count == 0 { &[] } else { slice::from_raw_parts(cells, count) };
            let typed = if typed.is_null() {
                None
            } else if count == 0 {
                Some(&[][..])
            } else {
                Some(slice::from_raw_parts(typed, count))
            };
            let formats = if formats.is_null() {
                None
            } else if columns == 0 {
                Some(&[][..])
            } else {
                Some(slice::from_raw_parts(formats, columns))
            };
            suggest(
                bytes(input, input_len),
                bytes(arena, arena_len),
                cells,
                typed,
                rows,
                formats,
                buffer(out, columns),
            )
        }
    }
}

export! {
    /// Reads `A5:F200` as its four numbers, so a binding doesn't parse a range itself: both corners
    /// in capitals, the second no less than the first in either direction. Anything else, `A5`
    /// alone, `a5:f200` or `F200:A5`, is [`ERR_CONTRACT`], as is a null, and nothing is written
    /// then. See [`rows::parse_range`].
    ///
    /// # Safety
    /// `text` is `len` bytes; `out` is writable, at any alignment.
    fn hypertabular_range_parse(text: *const u8, len: usize, out: *mut RawRange) -> i32 {
        if out.is_null() || (text.is_null() && len != 0) {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract.
        unsafe {
            match rows::parse_range(bytes(text, len)) {
                Some(range) => {
                    out.write_unaligned(range);
                    OK
                }
                None => ERR_CONTRACT,
            }
        }
    }
}

export! {
    /// Judges a read's sizes before the caller allocates anything: the bytes `batch_rows` rows of
    /// the plan ask for, against the one limit, and `batch_rows` and `buffer_bytes` of `0`. Call
    /// it when a reader or a sheet is opened, with `column_count` of `0` when the plan comes later,
    /// and again when a plan is bound. It writes `out` and returns [`OK`] or
    /// [`crate::kernel::abi::ERR_BATCH`]; `out` names the bytes, the limit and the reason, and
    /// says what size to start the input buffer at. See [`check_batch`].
    ///
    /// [`ERR_CONTRACT`] for a null `out`, a null or misaligned `specs` with a count, flags this core
    /// doesn't define, a spec of the wrong `size` or a door that isn't one; nothing is written then.
    ///
    /// # Safety
    /// `specs` is `column_count` aligned entries, or as far as the first wrong `size`; `out` is
    /// writable, at any alignment.
    fn hypertabular_batch_check(
        specs: *const ColumnSpecWith,
        column_count: usize,
        batch_rows: u64,
        buffer_bytes: u64,
        flags: u32,
        out: *mut RawBatchCheck,
    ) -> i32 {
        if out.is_null() || (column_count > 0 && (specs.is_null() || !specs.is_aligned())) {
            return ERR_CONTRACT;
        }
        // SAFETY: per the function contract: each size before what it says the layout is.
        unsafe {
            let mut at = 0;
            while at < column_count {
                if specs.add(at).cast::<u32>().read() != ColumnSpecWith::SIZE {
                    return ERR_CONTRACT;
                }
                at += 1;
            }
            let specs = if column_count == 0 { &[] } else { slice::from_raw_parts(specs, column_count) };
            let mut found = RawBatchCheck::default();
            let code = check_batch(specs, batch_rows, buffer_bytes, flags, &mut found);
            if code != ERR_CONTRACT {
                out.write_unaligned(found);
            }
            code
        }
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
