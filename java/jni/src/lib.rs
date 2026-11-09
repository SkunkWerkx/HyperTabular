//! The JNI library the Java binding loads below JDK 25. It sits beside the core in one
//! process: the core's C exports are linked in (so they remain exported), and `JNI_OnLoad`
//! registers one table of natives on `Native`, one per C export. No `alloc`, no `std`, and no
//! panic path: every native hands its arguments to the export of the same name and returns
//! what it returns.
//!
//! Every pointer crosses as a `long`, the address of native memory the binding allocated
//! (`MemorySegment.address()`), and every `usize` as a `long`: the buffers a reader works in
//! are native memory it holds for its whole life, so there is no Java array to pin or copy,
//! and no JNI function is called on any path but registration. The Java side checks each
//! segment is alive and its own before taking its address, as an FFM downcall does.
//!
//! The natives are `pub unsafe extern "system" fn`: their safety contract is the JNI one (a
//! live `JNIEnv`) plus the C ABI's (each address is live memory of the size beside it),
//! hence the crate-wide allow.
#![cfg_attr(not(test), no_std)]
#![allow(clippy::missing_safety_doc)]

extern crate hypertabular;

use core::ffi::{c_char, c_void};
use core::ptr::{null_mut, with_exposed_provenance, with_exposed_provenance_mut};
use jni_sys::{
    JNI_ERR, JNI_OK, JNI_VERSION_1_8, JNIEnv, JNINativeMethod, JavaVM, jclass, jint, jlong,
};

// The core's C exports, reached by symbol the way every binding reaches them. The shapes
// the pointers point at are the core's (rust/src/kernel/abi.rs); nothing here reads them.
unsafe extern "C" {
    fn hypertabular_version() -> u32;
    fn hypertabular_delimited_state_size() -> usize;
    fn hypertabular_delimited_init(state: *mut c_void, dialect: *const c_void) -> i32;
    fn hypertabular_delimited_header(
        state: *mut c_void,
        input: *const u8,
        input_len: usize,
        last: u32,
        names: *mut c_void,
        names_cap: usize,
        arena: *mut u8,
        arena_cap: usize,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_delimited_fill(
        state: *mut c_void,
        input: *const u8,
        input_len: usize,
        last: u32,
        specs: *const c_void,
        columns: *const c_void,
        column_count: usize,
        max_rows: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_delimited_unescape(
        cell: *const u8,
        len: usize,
        out: *mut u8,
        cap: usize,
    ) -> usize;
    fn hypertabular_workbook_state_size() -> usize;
    fn hypertabular_workbook_open(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_sheets(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_strings(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_styles(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_sheet(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        part: *const u8,
        part_len: usize,
        index: u32,
        has_header: u32,
        skip_empty_rows: u32,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_header(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_fill(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        specs: *const c_void,
        columns: *const c_void,
        column_count: usize,
        max_rows: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_delimited_init_with(state: *mut c_void, options: *const c_void) -> i32;
    fn hypertabular_delimited_fill_with(
        state: *mut c_void,
        input: *const u8,
        input_len: usize,
        last: u32,
        specs: *const c_void,
        columns: *const c_void,
        column_count: usize,
        max_rows: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_transcode(
        encoding: u32,
        input: *const u8,
        input_len: usize,
        last: u32,
        out: *mut u8,
        cap: usize,
        progress: *mut c_void,
    ) -> i32;
    fn hypertabular_faults(
        columns: *const c_void,
        column_count: usize,
        rows: usize,
        row_widths: *const u32,
        expected_width: u32,
        long_rows: u32,
        cursor: *mut u64,
        out: *mut c_void,
        cap: usize,
        written: *mut usize,
    ) -> i32;
    fn hypertabular_suggest(
        input: *const u8,
        input_len: usize,
        arena: *const u8,
        arena_len: usize,
        cells: *const c_void,
        typed: *const u8,
        rows: usize,
        columns: usize,
        formats: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_sheet_with(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        part: *const u8,
        part_len: usize,
        index: u32,
        options: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_workbook_fill_with(
        state: *mut c_void,
        container: *const u8,
        container_len: usize,
        specs: *const c_void,
        columns: *const c_void,
        column_count: usize,
        max_rows: usize,
        buffers: *const c_void,
        out: *mut c_void,
    ) -> i32;
    fn hypertabular_range_parse(text: *const u8, len: usize, out: *mut c_void) -> i32;
    fn hypertabular_batch_check(
        specs: *const c_void,
        column_count: usize,
        batch_rows: u64,
        buffer_bytes: u64,
        flags: u32,
        out: *mut c_void,
    ) -> i32;
}

const CLASS: &[u8] = b"io/github/skunkwerkx/hypertabular/Native\0";

/// One registered native: NUL-terminated name and descriptor, and the function.
pub struct Native {
    name: &'static [u8],
    descriptor: &'static [u8],
    function: *const c_void,
}

macro_rules! native {
    ($name:literal, $descriptor:literal, $function:path) => {
        Native {
            name: concat!($name, "\0").as_bytes(),
            descriptor: concat!($descriptor, "\0").as_bytes(),
            function: $function as *const c_void,
        }
    };
}

/// One native per C export, named after it, and `jniNatives`, the count the Java side holds
/// its own declarations to. Every address and `usize` is a `J`, every `u32` an `I`.
const NATIVES: [Native; 24] = [
    native!("hypertabularVersion", "()I", native_hypertabular_version),
    native!(
        "hypertabularDelimitedStateSize",
        "()J",
        native_hypertabular_delimited_state_size
    ),
    native!(
        "hypertabularDelimitedInit",
        "(JJ)I",
        native_hypertabular_delimited_init
    ),
    native!(
        "hypertabularDelimitedHeader",
        "(JJJIJJJJJ)I",
        native_hypertabular_delimited_header
    ),
    native!(
        "hypertabularDelimitedFill",
        "(JJJIJJJJJJ)I",
        native_hypertabular_delimited_fill
    ),
    native!(
        "hypertabularDelimitedUnescape",
        "(JJJJ)J",
        native_hypertabular_delimited_unescape
    ),
    native!(
        "hypertabularWorkbookStateSize",
        "()J",
        native_hypertabular_workbook_state_size
    ),
    native!(
        "hypertabularWorkbookOpen",
        "(JJJJJ)I",
        native_hypertabular_workbook_open
    ),
    native!(
        "hypertabularWorkbookSheets",
        "(JJJJJ)I",
        native_hypertabular_workbook_sheets
    ),
    native!(
        "hypertabularWorkbookStrings",
        "(JJJJJ)I",
        native_hypertabular_workbook_strings
    ),
    native!(
        "hypertabularWorkbookStyles",
        "(JJJJJ)I",
        native_hypertabular_workbook_styles
    ),
    native!(
        "hypertabularWorkbookSheet",
        "(JJJJJIIIJ)I",
        native_hypertabular_workbook_sheet
    ),
    native!(
        "hypertabularWorkbookHeader",
        "(JJJJJ)I",
        native_hypertabular_workbook_header
    ),
    native!(
        "hypertabularWorkbookFill",
        "(JJJJJJJJJ)I",
        native_hypertabular_workbook_fill
    ),
    native!(
        "hypertabularDelimitedInitWith",
        "(JJ)I",
        native_hypertabular_delimited_init_with
    ),
    native!(
        "hypertabularDelimitedFillWith",
        "(JJJIJJJJJJ)I",
        native_hypertabular_delimited_fill_with
    ),
    native!(
        "hypertabularTranscode",
        "(IJJIJJJ)I",
        native_hypertabular_transcode
    ),
    native!(
        "hypertabularFaults",
        "(JJJJIIJJJJ)I",
        native_hypertabular_faults
    ),
    native!(
        "hypertabularSuggest",
        "(JJJJJJJJJJ)I",
        native_hypertabular_suggest
    ),
    native!(
        "hypertabularWorkbookSheetWith",
        "(JJJJJIJJ)I",
        native_hypertabular_workbook_sheet_with
    ),
    native!(
        "hypertabularWorkbookFillWith",
        "(JJJJJJJJJ)I",
        native_hypertabular_workbook_fill_with
    ),
    native!(
        "hypertabularRangeParse",
        "(JJJ)I",
        native_hypertabular_range_parse
    ),
    native!(
        "hypertabularBatchCheck",
        "(JJJJIJ)I",
        native_hypertabular_batch_check
    ),
    native!("jniNatives", "()I", native_jni_natives),
];

/// True when no two entries share a name. A `const fn` so a repeat fails the build.
const fn names_unique(natives: &[Native]) -> bool {
    let mut i = 0;
    while i < natives.len() {
        let mut j = i + 1;
        while j < natives.len() {
            if bytes_eq(natives[i].name, natives[j].name) {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut k = 0;
    while k < a.len() {
        if a[k] != b[k] {
            return false;
        }
        k += 1;
    }
    true
}

const _: () = assert!(names_unique(&NATIVES));

/// The pointer a `long` address names: memory Java's `MemorySegment.address()` exposed.
#[inline(always)]
fn ptr<T>(address: jlong) -> *const T {
    with_exposed_provenance(address as usize)
}

/// The same, writable.
#[inline(always)]
fn ptr_mut<T>(address: jlong) -> *mut T {
    with_exposed_provenance_mut(address as usize)
}

/// A size, which Java holds as a non-negative `long`.
#[inline(always)]
fn size(value: jlong) -> usize {
    value as usize
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_version(
    _env: *mut JNIEnv,
    _class: jclass,
) -> jint {
    // SAFETY: takes no arguments.
    unsafe { hypertabular_version() as jint }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_delimited_state_size(
    _env: *mut JNIEnv,
    _class: jclass,
) -> jlong {
    // SAFETY: takes no arguments.
    unsafe { hypertabular_delimited_state_size() as jlong }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_delimited_init(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    dialect: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe { hypertabular_delimited_init(ptr_mut(state), ptr(dialect)) }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_delimited_header(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    input: jlong,
    input_len: jlong,
    last: jint,
    names: jlong,
    names_cap: jlong,
    arena: jlong,
    arena_cap: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_delimited_header(
            ptr_mut(state),
            ptr(input),
            size(input_len),
            last as u32,
            ptr_mut(names),
            size(names_cap),
            ptr_mut(arena),
            size(arena_cap),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_delimited_fill(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    input: jlong,
    input_len: jlong,
    last: jint,
    specs: jlong,
    columns: jlong,
    column_count: jlong,
    max_rows: jlong,
    buffers: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_delimited_fill(
            ptr_mut(state),
            ptr(input),
            size(input_len),
            last as u32,
            ptr(specs),
            ptr(columns),
            size(column_count),
            size(max_rows),
            ptr(buffers),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_delimited_unescape(
    _env: *mut JNIEnv,
    _class: jclass,
    cell: jlong,
    len: jlong,
    out: jlong,
    cap: jlong,
) -> jlong {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_delimited_unescape(ptr(cell), size(len), ptr_mut(out), size(cap)) as jlong
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_workbook_state_size(
    _env: *mut JNIEnv,
    _class: jclass,
) -> jlong {
    // SAFETY: takes no arguments.
    unsafe { hypertabular_workbook_state_size() as jlong }
}

/// The five workbook calls that share one shape.
macro_rules! book {
    ($native:ident, $export:ident) => {
        #[cfg_attr(feature = "no-panic", no_panic::no_panic)]
        pub unsafe extern "system" fn $native(
            _env: *mut JNIEnv,
            _class: jclass,
            state: jlong,
            container: jlong,
            container_len: jlong,
            buffers: jlong,
            out: jlong,
        ) -> jint {
            // SAFETY: the export's contract, which the caller upholds.
            unsafe {
                $export(
                    ptr_mut(state),
                    ptr(container),
                    size(container_len),
                    ptr(buffers),
                    ptr_mut(out),
                )
            }
        }
    };
}

book!(
    native_hypertabular_workbook_open,
    hypertabular_workbook_open
);
book!(
    native_hypertabular_workbook_sheets,
    hypertabular_workbook_sheets
);
book!(
    native_hypertabular_workbook_strings,
    hypertabular_workbook_strings
);
book!(
    native_hypertabular_workbook_styles,
    hypertabular_workbook_styles
);
book!(
    native_hypertabular_workbook_header,
    hypertabular_workbook_header
);

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_workbook_sheet(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    container: jlong,
    container_len: jlong,
    part: jlong,
    part_len: jlong,
    index: jint,
    has_header: jint,
    skip_empty_rows: jint,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_workbook_sheet(
            ptr_mut(state),
            ptr(container),
            size(container_len),
            ptr(part),
            size(part_len),
            index as u32,
            has_header as u32,
            skip_empty_rows as u32,
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_workbook_fill(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    container: jlong,
    container_len: jlong,
    specs: jlong,
    columns: jlong,
    column_count: jlong,
    max_rows: jlong,
    buffers: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_workbook_fill(
            ptr_mut(state),
            ptr(container),
            size(container_len),
            ptr(specs),
            ptr(columns),
            size(column_count),
            size(max_rows),
            ptr(buffers),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_delimited_init_with(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    options: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe { hypertabular_delimited_init_with(ptr_mut(state), ptr(options)) }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_delimited_fill_with(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    input: jlong,
    input_len: jlong,
    last: jint,
    specs: jlong,
    columns: jlong,
    column_count: jlong,
    max_rows: jlong,
    buffers: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_delimited_fill_with(
            ptr_mut(state),
            ptr(input),
            size(input_len),
            last as u32,
            ptr(specs),
            ptr(columns),
            size(column_count),
            size(max_rows),
            ptr(buffers),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_transcode(
    _env: *mut JNIEnv,
    _class: jclass,
    encoding: jint,
    input: jlong,
    input_len: jlong,
    last: jint,
    out: jlong,
    cap: jlong,
    progress: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_transcode(
            encoding as u32,
            ptr(input),
            size(input_len),
            last as u32,
            ptr_mut(out),
            size(cap),
            ptr_mut(progress),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_faults(
    _env: *mut JNIEnv,
    _class: jclass,
    columns: jlong,
    column_count: jlong,
    rows: jlong,
    row_widths: jlong,
    expected_width: jint,
    long_rows: jint,
    cursor: jlong,
    out: jlong,
    cap: jlong,
    written: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_faults(
            ptr(columns),
            size(column_count),
            size(rows),
            ptr(row_widths),
            expected_width as u32,
            long_rows as u32,
            ptr_mut(cursor),
            ptr_mut(out),
            size(cap),
            ptr_mut(written),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_suggest(
    _env: *mut JNIEnv,
    _class: jclass,
    input: jlong,
    input_len: jlong,
    arena: jlong,
    arena_len: jlong,
    cells: jlong,
    typed: jlong,
    rows: jlong,
    columns: jlong,
    formats: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_suggest(
            ptr(input),
            size(input_len),
            ptr(arena),
            size(arena_len),
            ptr(cells),
            ptr(typed),
            size(rows),
            size(columns),
            ptr(formats),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_workbook_sheet_with(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    container: jlong,
    container_len: jlong,
    part: jlong,
    part_len: jlong,
    index: jint,
    options: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_workbook_sheet_with(
            ptr_mut(state),
            ptr(container),
            size(container_len),
            ptr(part),
            size(part_len),
            index as u32,
            ptr(options),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn native_hypertabular_workbook_fill_with(
    _env: *mut JNIEnv,
    _class: jclass,
    state: jlong,
    container: jlong,
    container_len: jlong,
    specs: jlong,
    columns: jlong,
    column_count: jlong,
    max_rows: jlong,
    buffers: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_workbook_fill_with(
            ptr_mut(state),
            ptr(container),
            size(container_len),
            ptr(specs),
            ptr(columns),
            size(column_count),
            size(max_rows),
            ptr(buffers),
            ptr_mut(out),
        )
    }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_range_parse(
    _env: *mut JNIEnv,
    _class: jclass,
    text: jlong,
    len: jlong,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe { hypertabular_range_parse(ptr(text), size(len), ptr_mut(out)) }
}

#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_hypertabular_batch_check(
    _env: *mut JNIEnv,
    _class: jclass,
    specs: jlong,
    column_count: jlong,
    batch_rows: jlong,
    buffer_bytes: jlong,
    flags: jint,
    out: jlong,
) -> jint {
    // SAFETY: the export's contract, which the caller upholds.
    unsafe {
        hypertabular_batch_check(
            ptr(specs),
            size(column_count),
            batch_rows as u64,
            buffer_bytes as u64,
            flags as u32,
            ptr_mut(out),
        )
    }
}

/// The length of the registration table, never a literal.
#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn native_jni_natives(_env: *mut JNIEnv, _class: jclass) -> jint {
    NATIVES.len() as jint
}

/// Registers every native on `Native`, eagerly, so a table that does not match the Java
/// declarations fails `System.load` (with the pending exception) instead of a later call.
#[unsafe(no_mangle)]
#[cfg_attr(feature = "no-panic", no_panic::no_panic)]
pub unsafe extern "system" fn JNI_OnLoad(vm: *mut JavaVM, _reserved: *mut c_void) -> jint {
    let mut penv: *mut c_void = null_mut();
    // SAFETY: the JVM passes a live `JavaVM`.
    let got = unsafe { ((**vm).v1_2.GetEnv)(vm, &mut penv, JNI_VERSION_1_8) };
    // The null check is on `env`, the pointer dereferenced below, not on `penv`: CodeQL's
    // rust/access-invalid-pointer query takes `is_null()` as a barrier for its receiver only.
    let env = penv.cast::<*const jni_sys::JNINativeInterface_>();
    if got != JNI_OK || env.is_null() {
        return JNI_ERR;
    }
    // SAFETY: `env` came from `GetEnv` above and is not null; the class name is NUL-terminated.
    let class = unsafe { ((**env).v1_2.FindClass)(env, CLASS.as_ptr().cast::<c_char>()) };
    if class.is_null() {
        return JNI_ERR;
    }
    let empty = JNINativeMethod {
        name: null_mut(),
        signature: null_mut(),
        fnPtr: null_mut(),
    };
    let mut methods = [empty; NATIVES.len()];
    let mut i = 0;
    while i < NATIVES.len() {
        methods[i] = JNINativeMethod {
            name: NATIVES[i].name.as_ptr().cast_mut().cast::<c_char>(),
            signature: NATIVES[i].descriptor.as_ptr().cast_mut().cast::<c_char>(),
            fnPtr: NATIVES[i].function.cast_mut(),
        };
        i += 1;
    }
    // SAFETY: `methods` holds `NATIVES.len()` entries of NUL-terminated strings.
    let rc = unsafe {
        ((**env).v1_2.RegisterNatives)(env, class, methods.as_ptr(), NATIVES.len() as jint)
    };
    if rc != JNI_OK {
        return JNI_ERR;
    }
    JNI_VERSION_1_8
}

#[cfg(test)]
mod tests;
