//! C-ABI exports — `libhyperworkbook`. Open a workbook over caller memory or a path,
//! pick a sheet, then `hyperworkbook_read_batch` fills a [`RawBatchView`] plus one
//! [`RawColumnView`] per plan column, all pointing into the handle's own [`Batch`] and
//! valid until the next `read_batch`, `open_sheet`, or `close`.
//!
//! Return codes follow [`crate::ffi`]: non-negative counts; `-1` contract
//! violation, `-2` structural error, `-3` I/O. After a negative return,
//! `hyperworkbook_last_error` describes it.

use crate::ffi::{
    ERR_CONTRACT, ERR_IO, ERR_STRUCTURE, RawBatchView, RawColumnSpec, RawColumnView, plan_from_raw,
};
use crate::workbook::error::Error;
use crate::workbook::sheet::{Sheet, SheetOptions};
use crate::workbook::source::{FileSource, Source};
use crate::workbook::workbook::Workbook;
use crate::{Batch, fill_batch};
use core::slice;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

/// The one source type the exports use: caller bytes or a file.
pub enum AnySource {
    Bytes(Cursor<&'static [u8]>),
    File(FileSource),
}

impl Read for AnySource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            AnySource::Bytes(cursor) => cursor.read(buf),
            AnySource::File(file) => file.read(buf),
        }
    }
}

impl Seek for AnySource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            AnySource::Bytes(cursor) => cursor.seek(pos),
            AnySource::File(file) => file.seek(pos),
        }
    }
}

impl Source for AnySource {
    fn reopen(&self) -> io::Result<Self> {
        Ok(match self {
            AnySource::Bytes(cursor) => AnySource::Bytes(cursor.reopen()?),
            AnySource::File(file) => AnySource::File(file.reopen()?),
        })
    }
}

/// Sheet options as they cross the ABI (`0`/`1` booleans).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawSheetOptions {
    pub has_header: u8,
    pub skip_empty_rows: u8,
}

impl RawSheetOptions {
    fn to_options(self) -> SheetOptions {
        SheetOptions {
            has_header: self.has_header != 0,
            skip_empty_rows: self.skip_empty_rows != 0,
        }
    }
}

/// An open workbook, its active sheet, and its batch. Opaque to callers.
pub struct Handle {
    workbook: Workbook<AnySource>,
    sheet: Option<Sheet<AnySource>>,
    batch: Batch,
    error: Option<Error>,
    message: String,
}

fn code_for(error: &Error) -> i64 {
    match error {
        Error::Io(_) => ERR_IO,
        _ => ERR_STRUCTURE,
    }
}

impl Handle {
    fn fail(&mut self, error: Error) -> i64 {
        let code = code_for(&error);
        self.message = error.to_string();
        self.error = Some(error);
        code
    }
}

/// # Safety
/// `ptr` points to `len` live bytes when `len > 0`.
unsafe fn bytes<'caller>(ptr: *const u8, len: usize) -> &'caller [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(ptr, len) }
    }
}

fn open(result: Result<Workbook<AnySource>, Error>, out: *mut *mut Handle) -> i64 {
    match result {
        Ok(workbook) => {
            let handle = Box::new(Handle {
                workbook,
                sheet: None,
                batch: Batch::new(),
                error: None,
                message: String::new(),
            });
            // SAFETY: callers check `out` for null.
            unsafe { out.write(Box::into_raw(handle)) };
            0
        }
        Err(error) => code_for(&error),
    }
}

/// Opens a workbook over `len` bytes at `ptr`, which the caller keeps alive and unchanged
/// until `hyperworkbook_close`. Writes the handle to `out` and returns `0`.
///
/// # Safety
/// `ptr`/`len` are a live byte range for the handle's lifetime; `out` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_open_bytes(
    ptr: *const u8,
    len: usize,
    out: *mut *mut Handle,
) -> i64 {
    if out.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let bytes = unsafe { bytes(ptr, len) };
    open(Workbook::open(AnySource::Bytes(Cursor::new(bytes))), out)
}

/// Opens the workbook file at the UTF-8 path in `ptr`/`len`. As
/// [`hyperworkbook_open_bytes`] otherwise.
///
/// # Safety
/// `ptr`/`len` are `len` live bytes; `out` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_open_path(
    ptr: *const u8,
    len: usize,
    out: *mut *mut Handle,
) -> i64 {
    if out.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let path = unsafe { bytes(ptr, len) };
    let Ok(path) = str::from_utf8(path) else {
        return ERR_CONTRACT;
    };
    let source = match FileSource::open(path) {
        Ok(source) => AnySource::File(source),
        Err(_) => return ERR_IO,
    };
    open(Workbook::open(source), out)
}

/// Frees a handle. A null handle is a no-op.
///
/// # Safety
/// `handle` came from an open call and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_close(handle: *mut Handle) {
    if !handle.is_null() {
        // SAFETY: per the function contract.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// `1` for XLSX, `2` for ODS.
///
/// # Safety
/// `handle` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_format(handle: *const Handle) -> i64 {
    if handle.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    unsafe { &*handle }.workbook.format().code()
}

/// The workbook's date system, as HyperCast numbers it: `1` for 1900, `2` for 1904.
///
/// # Safety
/// `handle` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_date_system(handle: *const Handle) -> i64 {
    if handle.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    unsafe { &*handle }.workbook.date_system() as i64
}

/// The number of sheets, hidden ones included.
///
/// # Safety
/// `handle` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_sheet_count(handle: *const Handle) -> i64 {
    if handle.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    unsafe { &*handle }.workbook.sheets().len() as i64
}

/// Writes sheet `index`'s UTF-8 name to `out_ptr`/`out_len` (valid until close) and
/// returns whether it is hidden (`1`) or not (`0`); `-1` past the end.
///
/// # Safety
/// `handle`, `out_ptr`, `out_len` are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_sheet_name(
    handle: *const Handle,
    index: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i64 {
    if handle.is_null() || out_ptr.is_null() || out_len.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let handle = unsafe { &*handle };
    match handle.workbook.sheets().get(index) {
        Some(info) => {
            // SAFETY: per the function contract.
            unsafe {
                out_ptr.write(info.name.as_ptr());
                out_len.write(info.name.len());
            }
            i64::from(info.hidden)
        }
        None => ERR_CONTRACT,
    }
}

/// Opens sheet `index` as the handle's active sheet (replacing any previous one; its
/// header record is read now if declared). Returns `0`.
///
/// # Safety
/// `handle` and `options` are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_open_sheet(
    handle: *mut Handle,
    index: usize,
    options: *const RawSheetOptions,
) -> i64 {
    if handle.is_null() || options.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let (handle, options) = unsafe { (&mut *handle, (*options).to_options()) };
    handle.sheet = None;
    handle.error = None;
    match handle.workbook.sheet(index, options) {
        Ok(sheet) => {
            handle.sheet = Some(sheet);
            0
        }
        Err(error) => handle.fail(error),
    }
}

/// The active sheet's header name count, or `0` when no header was declared.
///
/// # Safety
/// `handle` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_header_count(handle: *const Handle) -> i64 {
    if handle.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let handle = unsafe { &*handle };
    match &handle.sheet {
        Some(sheet) => sheet.header().map_or(0, |header| header.len() as i64),
        None => ERR_CONTRACT,
    }
}

/// Writes header name `index` of the active sheet to `out_ptr`/`out_len` (valid until
/// the sheet changes or close) and returns `0`; `-1` past the end.
///
/// # Safety
/// `handle`, `out_ptr`, `out_len` are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_header_name(
    handle: *const Handle,
    index: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i64 {
    if handle.is_null() || out_ptr.is_null() || out_len.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let handle = unsafe { &*handle };
    match handle
        .sheet
        .as_ref()
        .and_then(|sheet| sheet.header())
        .and_then(|header| header.name(index))
    {
        Some(name) => {
            // SAFETY: per the function contract.
            unsafe {
                out_ptr.write(name.as_ptr());
                out_len.write(name.len());
            }
            0
        }
        None => ERR_CONTRACT,
    }
}

/// Reads up to `max_rows` rows of the active sheet through the `spec_count` columns at
/// `specs` into the handle's batch, writes the batch view to `out_batch` and one column
/// view per spec to `out_columns`, and returns the row count (`0` at the end).
///
/// # Safety
/// `handle` is live and has an active sheet; `specs` points to `spec_count` specs;
/// `out_batch` is live; `out_columns` has room for `spec_count` views.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_read_batch(
    handle: *mut Handle,
    specs: *const RawColumnSpec,
    spec_count: usize,
    max_rows: u64,
    out_batch: *mut RawBatchView,
    out_columns: *mut RawColumnView,
) -> i64 {
    if handle.is_null()
        || out_batch.is_null()
        || (spec_count > 0 && (specs.is_null() || out_columns.is_null()))
    {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let handle = unsafe { &mut *handle };
    let Some(sheet) = handle.sheet.as_mut() else {
        return ERR_CONTRACT;
    };
    let specs = if spec_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(specs, spec_count) }
    };
    let Some(plan) = plan_from_raw(specs) else {
        return ERR_CONTRACT;
    };
    let rows = match fill_batch(sheet, &plan, &mut handle.batch, max_rows as usize) {
        Ok(rows) => rows,
        Err(error) => return handle.fail(error),
    };
    // SAFETY: per the function contract.
    unsafe {
        out_batch.write(handle.batch.view());
        for index in 0..spec_count {
            if let Some(view) = handle.batch.column_view(index) {
                out_columns.add(index).write(view);
            }
        }
    }
    rows as i64
}

/// Describes the last error: its kind code ([`Error::code`]) to `out_code` and the
/// message bytes (valid until the next failing call or close) to `msg_ptr`/`msg_len`.
/// Returns the kind code, or `0` when there is no error.
///
/// # Safety
/// `handle`, `out_code`, `msg_ptr`, `msg_len` are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hyperworkbook_last_error(
    handle: *const Handle,
    out_code: *mut i32,
    msg_ptr: *mut *const u8,
    msg_len: *mut usize,
) -> i64 {
    if handle.is_null() || out_code.is_null() || msg_ptr.is_null() || msg_len.is_null() {
        return ERR_CONTRACT;
    }
    // SAFETY: per the function contract.
    let handle = unsafe { &*handle };
    let (code, message): (i32, &str) = match &handle.error {
        Some(error) => (error.code(), &handle.message),
        None => (0, ""),
    };
    // SAFETY: per the function contract.
    unsafe {
        out_code.write(code);
        msg_ptr.write(message.as_ptr());
        msg_len.write(message.len());
    }
    i64::from(code)
}
