//! The shapes that cross the C ABI. Every one is `#[repr(C)]`, holds no pointer the core
//! keeps, and is memory the caller owns: the core reads what it is handed and writes where
//! it is told, and remembers nothing between calls but what is in the caller's state block.

use core::ffi::c_void;
use hypercast::{Fault, NumFormat, RawDateOptions, RawNumFormat, Reason};
pub use hypercast::{RawCheck, RawConstraints};

/// The call did what it could; the result says how far it got.
pub const OK: i32 = 0;
/// A caller bug, not a data verdict: an undefined door, an invalid numeric format, a
/// buffer the contract rules out.
pub const ERR_CONTRACT: i32 = -1;
/// The data is structurally broken; the result's [`Failure`] says where. Final: the same
/// state block reports it again on every later call.
pub const ERR_STRUCTURE: i32 = -2;
/// The arena cannot hold what one row needs; the result says how much that is. Nothing
/// was consumed — call again with a larger arena.
pub const ERR_ARENA: i32 = -3;
/// The cell table cannot hold one row; the result says how many entries one row takes.
/// Nothing was consumed — call again with a larger table.
pub const ERR_CELLS: i32 = -4;
/// The window a workbook part is inflated through cannot hold the token being read; the
/// result says what size would be fair. See [`Buffers`] for what a workbook call that
/// returns this, [`ERR_ARENA`] or [`ERR_CELLS`] expects next.
pub const ERR_WINDOW: i32 = -5;
/// `hypertabular_batch_check` refused a read's sizes; its [`RawBatchCheck`] says why.
pub const ERR_BATCH: i32 = -6;

// The size and alignment of every shape that crosses the ABI, pinned here because the
// bindings restate them by hand (C# structs, Java's layouts, Go and PHP declarations, ctypes)
// and the C header carries each fixed size as `<Type>_SIZE`. A layout change fails to compile
// here first, and these numbers are what every binding's own declaration has to move to.
// `ColumnBuffer`, `Buffers` and the size-tagged shapes that hold pointers are pinned in pointer
// widths, and the header carries those sizes as `sizeof`;
// the delimited state is pinned beside its private fields (delimited/fill.rs).
//
// Only where a u64 is 8-aligned, which is every target a binding ships for.
macro_rules! pin_layout {
    ($($ty:ty => ($size:expr, $align:expr)),+ $(,)?) => {$(
        const _: () = assert!(
            align_of::<u64>() != 8 || (size_of::<$ty>() == $size && align_of::<$ty>() == $align)
        );
    )+};
}

const WORD: usize = size_of::<usize>();

pin_layout! {
    Span => (Span::SIZE as usize, 4),
    CellVerdict => (CellVerdict::SIZE as usize, 4),
    ColumnSpec => (ColumnSpec::SIZE as usize, 4),
    ColumnBuffer => (2 * WORD, WORD),
    Failure => (Failure::SIZE as usize, 8),
    Filled => (Filled::SIZE as usize, 8),
    Slot => (Slot::SIZE as usize, 8),
    Buffers => (14 * WORD, WORD),
    Opened => (Opened::SIZE as usize, 8),
    SheetInfo => (SheetInfo::SIZE as usize, 4),
    RawDelimitedOptions => (RawDelimitedOptions::SIZE as usize, 4),
    CellNote => (CellNote::SIZE as usize, 2),
    ColumnSpecWith => (120 + 7 * WORD, WORD),
    ColumnBufferWith => (5 * WORD, WORD),
    BuffersWith => (18 * WORD, WORD),
    RawSheetOptions => (RawSheetOptions::SIZE as usize, 4),
    RawRange => (RawRange::SIZE as usize, 4),
    RawTranscoded => (RawTranscoded::SIZE as usize, 8),
    RawProblem => (RawProblem::SIZE as usize, 4),
    RawSuggestion => (RawSuggestion::SIZE as usize, 4),
    RawBatchCheck => (RawBatchCheck::SIZE as usize, 8),
}

// The offsets, pinned the same way.
const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(RawTranscoded, consumed) == 0);
    assert!(offset_of!(RawTranscoded, written) == 8);
    assert!(offset_of!(RawTranscoded, failure_at) == 16);
    assert!(offset_of!(RawTranscoded, failed) == 24);
    assert!(offset_of!(RawTranscoded, reserved) == 28);
    assert!(offset_of!(RawBatchCheck, needed) == 0);
    assert!(offset_of!(RawBatchCheck, limit) == 8);
    assert!(offset_of!(RawBatchCheck, buffer_bytes) == 16);
    assert!(offset_of!(RawBatchCheck, sample_rows) == 24);
    assert!(offset_of!(RawBatchCheck, width) == 32);
    assert!(offset_of!(RawBatchCheck, refused) == 40);
    assert!(offset_of!(RawBatchCheck, reserved) == 44);
    assert!(offset_of!(RawProblem, row) == 0);
    assert!(offset_of!(RawProblem, plan_column) == 4);
    assert!(offset_of!(RawProblem, reason) == 8);
    assert!(offset_of!(RawProblem, detail) == 12);
    assert!(offset_of!(RawProblem, origin) == 14);
    assert!(offset_of!(RawProblem, offset) == 16);
    assert!(offset_of!(RawProblem, len) == 20);
    assert!(offset_of!(RawProblem, expected) == 24);
    assert!(offset_of!(RawProblem, found) == 28);
    assert!(offset_of!(RawProblem, keyword) == 32);
    assert!(offset_of!(RawSuggestion, door) == 0);
    assert!(offset_of!(RawSuggestion, param) == 4);
    assert!(offset_of!(RawSuggestion, status) == 8);
    assert!(offset_of!(RawSuggestion, matched) == 12);
    assert!(offset_of!(RawSuggestion, sampled) == 16);
    assert!(offset_of!(Span, offset) == 0 && offset_of!(Span, len) == 4);
    assert!(offset_of!(CellVerdict, offset) == 0);
    assert!(offset_of!(CellVerdict, len) == 4);
    assert!(offset_of!(CellVerdict, reason) == 8);
    assert!(offset_of!(ColumnSpec, ordinal) == 0);
    assert!(offset_of!(ColumnSpec, door) == 4);
    assert!(offset_of!(ColumnSpec, param) == 8);
    assert!(offset_of!(ColumnSpec, format) == 12);
    assert!(offset_of!(ColumnBuffer, values) == 0 && offset_of!(ColumnBuffer, verdicts) == WORD);
    assert!(offset_of!(Failure, code) == 0);
    assert!(offset_of!(Failure, line) == 4);
    assert!(offset_of!(Failure, record) == 8);
    assert!(offset_of!(Failure, byte) == 16);
    assert!(offset_of!(Failure, expected) == 24);
    assert!(offset_of!(Failure, found) == 28);
    assert!(offset_of!(Filled, rows) == 0);
    assert!(offset_of!(Filled, consumed) == 8);
    assert!(offset_of!(Filled, arena_used) == 16);
    assert!(offset_of!(Filled, needed) == 24);
    assert!(offset_of!(Filled, failure) == 32);
    assert!(offset_of!(Buffers, window) == 0);
    assert!(offset_of!(Buffers, window_cap) == WORD);
    assert!(offset_of!(Buffers, arena) == 2 * WORD);
    assert!(offset_of!(Buffers, arena_cap) == 3 * WORD);
    assert!(offset_of!(Buffers, cells) == 4 * WORD);
    assert!(offset_of!(Buffers, cells_cap) == 5 * WORD);
    assert!(offset_of!(Buffers, row) == 6 * WORD);
    assert!(offset_of!(Buffers, row_cap) == 7 * WORD);
    assert!(offset_of!(Buffers, strings) == 8 * WORD);
    assert!(offset_of!(Buffers, strings_len) == 9 * WORD);
    assert!(offset_of!(Buffers, table) == 10 * WORD);
    assert!(offset_of!(Buffers, table_len) == 11 * WORD);
    assert!(offset_of!(Buffers, kinds) == 12 * WORD);
    assert!(offset_of!(Buffers, kinds_len) == 13 * WORD);
    assert!(offset_of!(Opened, format) == 0);
    assert!(offset_of!(Opened, epoch) == 4);
    assert!(offset_of!(Opened, strings_bytes) == 8);
    assert!(offset_of!(Opened, strings_count) == 16);
    assert!(offset_of!(Opened, needed) == 24);
    assert!(offset_of!(Opened, failure) == 32);
    assert!(offset_of!(SheetInfo, name) == 0);
    assert!(offset_of!(SheetInfo, part) == 8);
    assert!(offset_of!(SheetInfo, flags) == 16);
    assert!(offset_of!(SheetInfo, index) == 20);
    assert!(offset_of!(RawDelimitedOptions, size) == 0);
    assert!(offset_of!(RawDelimitedOptions, separator) == 4);
    assert!(offset_of!(RawDelimitedOptions, quoting) == 5);
    assert!(offset_of!(RawDelimitedOptions, skip_blank_lines) == 6);
    assert!(offset_of!(RawDelimitedOptions, engine) == 7);
    assert!(offset_of!(RawDelimitedOptions, skip) == 8);
    assert!(offset_of!(RawDelimitedOptions, header_row) == 12);
    assert!(offset_of!(RawDelimitedOptions, short_rows) == 16);
    assert!(offset_of!(RawDelimitedOptions, long_rows) == 20);
    assert!(offset_of!(CellNote, detail) == 0 && offset_of!(CellNote, origin) == 2);
    assert!(offset_of!(ColumnSpecWith, size) == 0);
    assert!(offset_of!(ColumnSpecWith, ordinal) == 4);
    assert!(offset_of!(ColumnSpecWith, door) == 8);
    assert!(offset_of!(ColumnSpecWith, param) == 12);
    assert!(offset_of!(ColumnSpecWith, format) == 16);
    assert!(offset_of!(ColumnSpecWith, date) == 108);
    assert!(offset_of!(ColumnSpecWith, months) == 120);
    assert!(offset_of!(ColumnSpecWith, months_len) == 120 + WORD);
    assert!(offset_of!(ColumnSpecWith, tokens) == 120 + 2 * WORD);
    assert!(offset_of!(ColumnSpecWith, tokens_len) == 120 + 3 * WORD);
    assert!(offset_of!(ColumnSpecWith, constraints) == 120 + 4 * WORD);
    assert!(offset_of!(ColumnSpecWith, enum_ptr) == 120 + 5 * WORD);
    assert!(offset_of!(ColumnSpecWith, enum_len) == 120 + 6 * WORD);
    assert!(offset_of!(ColumnBufferWith, size) == 0);
    assert!(offset_of!(ColumnBufferWith, values) == WORD);
    assert!(offset_of!(ColumnBufferWith, verdicts) == 2 * WORD);
    assert!(offset_of!(ColumnBufferWith, notes) == 3 * WORD);
    assert!(offset_of!(ColumnBufferWith, checks) == 4 * WORD);
    assert!(offset_of!(BuffersWith, size) == 0);
    assert!(offset_of!(BuffersWith, window) == WORD);
    assert!(offset_of!(BuffersWith, window_cap) == 2 * WORD);
    assert!(offset_of!(BuffersWith, arena) == 3 * WORD);
    assert!(offset_of!(BuffersWith, arena_cap) == 4 * WORD);
    assert!(offset_of!(BuffersWith, cells) == 5 * WORD);
    assert!(offset_of!(BuffersWith, cells_cap) == 6 * WORD);
    assert!(offset_of!(BuffersWith, row) == 7 * WORD);
    assert!(offset_of!(BuffersWith, row_cap) == 8 * WORD);
    assert!(offset_of!(BuffersWith, strings) == 9 * WORD);
    assert!(offset_of!(BuffersWith, strings_len) == 10 * WORD);
    assert!(offset_of!(BuffersWith, table) == 11 * WORD);
    assert!(offset_of!(BuffersWith, table_len) == 12 * WORD);
    assert!(offset_of!(BuffersWith, kinds) == 13 * WORD);
    assert!(offset_of!(BuffersWith, kinds_len) == 14 * WORD);
    assert!(offset_of!(BuffersWith, tokens) == 15 * WORD);
    assert!(offset_of!(BuffersWith, tokens_len) == 16 * WORD);
    assert!(offset_of!(BuffersWith, row_widths) == 17 * WORD);
    assert!(offset_of!(RawSheetOptions, size) == 0);
    assert!(offset_of!(RawSheetOptions, has_header) == 4);
    assert!(offset_of!(RawSheetOptions, skip_empty_rows) == 8);
    assert!(offset_of!(RawSheetOptions, skip) == 12);
    assert!(offset_of!(RawSheetOptions, header_row) == 16);
    assert!(offset_of!(RawSheetOptions, first_row) == 20);
    assert!(offset_of!(RawSheetOptions, first_column) == 24);
    assert!(offset_of!(RawSheetOptions, last_row) == 28);
    assert!(offset_of!(RawSheetOptions, last_column) == 32);
    assert!(offset_of!(RawRange, first_row) == 0);
    assert!(offset_of!(RawRange, first_column) == 4);
    assert!(offset_of!(RawRange, last_row) == 8);
    assert!(offset_of!(RawRange, last_column) == 12);
};

/// A byte range. Where it points is the field's to say; the top bit of `len` is a flag
/// each use defines, and [`Span::len`] is the length without it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the first byte.
    pub offset: u32,
    /// Byte length, with the flag in its top bit.
    pub len: u32,
}

impl Span {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 8;

    /// The flag bit of `len`. On a cell-table entry: the cell is quoted with `""` inside
    /// and has to be unescaped to be read. On a `Text` value: the bytes are in the arena
    /// rather than in the input.
    pub const FLAG: u32 = 0x8000_0000;

    /// The length, without the flag.
    pub const fn len(self) -> usize {
        (self.len & !Span::FLAG) as usize
    }

    /// True when the span is empty.
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// True when the flag is set.
    pub const fn flagged(self) -> bool {
        self.len & Span::FLAG != 0
    }
}

/// HyperCast's fault as it sits in a verdict array, with `reason == 0` meaning the cell
/// cast. The span indexes the cell's own text.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellVerdict {
    /// Byte offset of the offending span within the cell's text.
    pub offset: u32,
    /// Byte length of the offending span.
    pub len: u32,
    /// `0` ok, `1` empty, `2` malformed, `3` out of range — HyperCast's ABI codes — or, from a
    /// `_with` fill on a column that declares constraints, [`CellVerdict::VIOLATION`].
    pub reason: u32,
}

impl CellVerdict {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 12;

    /// `reason`: the cell cast, and broke a constraint its column declares, HyperCast's
    /// `cast_checked` code. The value slot holds the value exactly as cast, never rounded, the span
    /// is the judged text's, and the column's [`ColumnBufferWith::checks`] names the keyword.
    /// [`CellVerdict::fault`] reads it as no fault.
    pub const VIOLATION: u32 = 4;

    /// The success verdict.
    pub const OK: CellVerdict = CellVerdict {
        offset: 0,
        len: 0,
        reason: 0,
    };

    /// True when the cell cast successfully.
    pub const fn is_ok(&self) -> bool {
        self.reason == 0
    }

    /// The fault, when there is one.
    pub const fn fault(&self) -> Option<Fault> {
        let Some(reason) = Reason::from_code(self.reason) else {
            return None;
        };
        Some(Fault {
            reason,
            offset: self.offset,
            len: self.len,
        })
    }

    /// A verdict from a fault.
    pub const fn from_fault(fault: Fault) -> CellVerdict {
        CellVerdict {
            offset: fault.offset,
            len: fault.len,
            reason: fault.reason as u32,
        }
    }
}

/// One output column as the caller declares it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSpec {
    /// Zero-based ordinal of the source column it reads. Past a row's end reads as empty.
    pub ordinal: u32,
    /// The door's code ([`crate::kernel::door::Door::code`]): `1` bool, `2`–`5` i8 to i64,
    /// `6`–`9` u8 to u64, `10` f32, `11` f64, `12` UUID, `13` timestamp, `14` Unix time,
    /// `15` date, `16` time of day, `17` duration, `18` text, `19` decimal, `20` a date
    /// under a declared order, `21` a civil date-time under a declared order, `22` an Excel
    /// serial.
    pub door: u32,
    /// What the door declares, where it declares something, numbered as HyperCast numbers
    /// it: for `14` the Unix precision (`1` seconds, `2` milliseconds, `3` microseconds, `4`
    /// nanoseconds), for `20` and `21` the date order (`1` year-month-day, `2`
    /// month-day-year, `3` day-month-year), for `22` the Excel date system (`1` 1900, `2`
    /// 1904). `0` otherwise.
    pub param: u32,
    /// The numeric notation, read by the numeric doors: HyperCast's size-tagged layout,
    /// built whole (its `size` is `RawNumFormat::SIZE`, 92). All zeros — a `size` of `0` —
    /// is no format, HyperCast's `NumFormat::INVARIANT`, which groups as Western does.
    /// `hypertabular_delimited_fill` reads only the low 8 bits of its `flags`, as 1.0 did, so it
    /// reads every format as Western; `hypertabular_delimited_fill_with` reads every one, and
    /// refuses a retired grouping bit (`256`, so the words `351` and `863`) before any cell.
    pub format: RawNumFormat,
}

impl ColumnSpec {
    /// The layout's size in bytes: three words, then HyperCast's 92-byte `RawNumFormat`.
    pub const SIZE: u32 = 104;

    /// A column over `ordinal` through the door `door`/`param`, invariant notation.
    pub const fn new(ordinal: u32, door: u32, param: u32) -> ColumnSpec {
        ColumnSpec {
            ordinal,
            door,
            param,
            format: RawNumFormat {
                size: 0,
                ..RawNumFormat::ZERO
            },
        }
    }

    /// The declared notation, or `None` for a contract violation: a format HyperCast's
    /// own doors would refuse, or a layout whose size the core does not read.
    pub fn num_format(&self) -> Option<NumFormat> {
        if self.format.size == 0 {
            return no_format(&self.format).then_some(NumFormat::INVARIANT);
        }
        self.format.resolve().copied()
    }

    /// [`ColumnSpec::num_format`] with `flags` as a 1.0 export read them: bits 8 and up, which 1.0
    /// ignored and this version gives meaning to or refuses, are ignored, so such an export reads
    /// Western grouping, whatever the policy bits say. As HyperCast's own 1.0 exports do. What
    /// Western grouping answers is HyperCast's, and has moved since 1.0: `1,23,456`, `12,34` and
    /// `0,125` are refused.
    pub(crate) fn num_format_1_0(&self) -> Option<NumFormat> {
        if self.format.size == 0 {
            return Some(NumFormat::INVARIANT);
        }
        let mut raw = self.format;
        raw.format.flags &= FLAGS_1_0;
        raw.resolve().copied()
    }
}

/// Whether a format with a `size` of `0` declares nothing, as its doc says. A grouping policy beside
/// that `size` is a declaration that would be lost, and a stale word that would never be refused,
/// so it's refused here. No 1.0 export calls this: those read no bit from 8 up.
pub(crate) const fn no_format(format: &RawNumFormat) -> bool {
    format.format.flags & NumFormat::GROUPING_POLICY == 0
}

/// The `flags` bits a 1.0 export reads.
const FLAGS_1_0: u32 = 0xFF;

/// Where one column's output goes: `max_rows` values of the door's type and `max_rows`
/// verdicts, both the caller's memory. Neither has to be aligned.
///
/// The value type per door: `u8` (0/1) for `Bool`; the matching integer or float for the
/// numeric doors; HyperCast's `Decimal` (16 bytes); 16 bytes for `Uuid`; HyperCast's
/// `Timestamp` for `Timestamp`, `Unix` and `ExcelSerial`; its `Date` for `Date` and
/// `DateOrdered`; its `CivilDateTime` for `DateTime`; `u64` nanoseconds since midnight for
/// `Time`; its `Duration`; and a [`Span`] for `Text`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ColumnBuffer {
    /// The value array.
    pub values: *mut c_void,
    /// The verdict array.
    pub verdicts: *mut CellVerdict,
}

/// Why the data could not be read. `code` `0` is no failure.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Failure {
    /// `1` the input ended inside a quoted cell; `2` a record's cell count disagrees with
    /// the first record's; `16`–`26` a workbook's ([`Failure::NOT_A_ZIP`] and on); `27` text
    /// that isn't valid UTF-8 where no cell's verdict can carry it, or that a declared encoding
    /// has no character for.
    pub code: u32,
    /// One-based line the offending record starts on.
    pub line: u32,
    /// Zero-based index of the offending record, header and skipped blank lines included.
    pub record: u64,
    /// Absolute byte offset of the offending record's start, in the decoded text if an encoding
    /// was declared, except for [`Failure::ENCODING`]'s there (it says what).
    pub byte: u64,
    /// Cells in the first record (code `2`).
    pub expected: u32,
    /// Cells in this record (code `2`).
    pub found: u32,
}

impl Failure {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 32;

    /// The input ended inside a quoted cell.
    pub const UNCLOSED_QUOTE: u32 = 1;
    /// A record's cell count disagrees with the first record's.
    pub const COLUMN_COUNT: u32 = 2;

    // A workbook's failures. `record` is the part being read ([`Failure::PART_ROOT_RELS`]
    // and the rest), `byte` how far into its inflated bytes the read had got, and `line`
    // the sheet row being read, where there is one.

    /// The container has no end-of-central-directory record: it is not a zip file.
    pub const NOT_A_ZIP: u32 = 16;
    /// The zip's own structure is broken: a directory that lies outside the file or is
    /// cut short, a local header that is not one.
    pub const CONTAINER: u32 = 17;
    /// The part, or the document, is encrypted.
    pub const ENCRYPTED: u32 = 18;
    /// The part is compressed by a method that is neither stored nor deflate; `found` is
    /// the method.
    pub const METHOD: u32 = 19;
    /// A part the workbook cannot be read without is not in the container.
    pub const MISSING_PART: u32 = 20;
    /// The part's XML ends inside a construct; `found` says which (`1` a comment, `2` a
    /// CDATA section, `3` a processing instruction, `4` a declaration, `5` a tag, `6` an
    /// element, `7` a row, `8` a cell, `9` a value, `10` a string).
    pub const XML: u32 = 21;
    /// The part's bytes are not a deflate stream (`found` `0`), or stop before the stream
    /// does (`found` `1`).
    pub const DEFLATE: u32 = 22;
    /// The container is a zip but not an XLSX, ODS or Numbers workbook.
    pub const NOT_A_WORKBOOK: u32 = 23;
    /// A cell names a shared string by something that is not a number (`found` `0`,
    /// `expected` `0`), or by an index past the table (`found` the index, `expected` the
    /// table's length).
    pub const SHARED_STRING: u32 = 24;
    /// More text than a span can address: over 4 GiB in one batch or in the shared
    /// strings, or 2 GiB in one cell.
    pub const TOO_LARGE: u32 = 25;
    /// A Numbers part is not what the IWA packaging says: `found` is `1` for a chunk or
    /// Snappy block that is broken, `2` for a Protocol Buffers message that is malformed,
    /// `3` for an object a reference names that is not in the package (`expected` is its
    /// identifier), `4` for cell storage of a version the core does not read (`expected`
    /// is the version). `byte` is the object's offset in the part's decoded stream.
    pub const IWA: u32 = 26;
    /// Text that isn't valid UTF-8 where a read can't carry it as a cell's verdict: a header
    /// name. `line`, `record` and `byte` locate the record as for any delimited failure, or as
    /// for any workbook failure in a workbook; `expected` and `found` are `0`. Or, raised by a
    /// binding after `hypertabular_transcode`, a unit a declared encoding has no character for:
    /// `line` and `record` locate the record the failure cut, as ever, and `byte` is the unit's
    /// offset in the original input, not the decoded text.
    pub const ENCODING: u32 = 27;

    /// `_rels/.rels`.
    pub const PART_ROOT_RELS: u32 = 1;
    /// The workbook part.
    pub const PART_WORKBOOK: u32 = 2;
    /// The workbook part's relationships.
    pub const PART_WORKBOOK_RELS: u32 = 3;
    /// The style table.
    pub const PART_STYLES: u32 = 4;
    /// The shared strings.
    pub const PART_STRINGS: u32 = 5;
    /// A worksheet.
    pub const PART_SHEET: u32 = 6;
    /// An ODS document's `content.xml`.
    pub const PART_CONTENT: u32 = 7;
    /// An ODS document's `mimetype`.
    pub const PART_MIMETYPE: u32 = 8;
    /// An ODS document's `META-INF/manifest.xml`.
    pub const PART_MANIFEST: u32 = 9;
    /// A Numbers document's `.iwa` parts, as one.
    pub const PART_IWA: u32 = 10;
}

/// What a call did.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filled {
    /// Rows written to every column (or, for a header, names written).
    pub rows: u64,
    /// Bytes of the input that are finished with. What follows them has to be at the
    /// front of the next call's input.
    pub consumed: u64,
    /// Bytes of the arena written.
    pub arena_used: u64,
    /// On [`ERR_ARENA`], the arena bytes one row needs; on [`ERR_CELLS`], the table
    /// entries one row needs.
    pub needed: u64,
    /// On [`ERR_STRUCTURE`], what is wrong and where.
    pub failure: Failure,
}

impl Filled {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 64;
}

/// One cell of the row a workbook read is assembling: a tag and a payload, plain integers.
/// The caller supplies the scratch these live in ([`Buffers::row`]) and never reads them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub(crate) tag: u32,
    pub(crate) aux: u32,
    pub(crate) bits: u64,
}

impl Slot {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 16;
}

/// The memory a workbook call works in — all of it the caller's, handed over again on
/// every call — and the delimited fill's `cells` and `arena`. A call uses the buffers its
/// own documentation names and ignores the rest (null with a zero size is fine for those).
///
/// **Growing a buffer.** A workbook part is a compressed stream, and a read of one cannot
/// be rolled back to where the call started the way a read of delimited text can. So when
/// a workbook call returns [`ERR_WINDOW`], [`ERR_ARENA`] or [`ERR_CELLS`], nothing is
/// undone: the state block holds exactly where the read stopped, as offsets into these
/// buffers. The caller makes the buffer the code names at least as large as the result's
/// `needed`, **keeping what the old one held** (a `realloc`, or allocate-and-copy), leaves
/// every other buffer as it was, and makes the same call again; the read goes on from the
/// token it stopped at. Calling again without having grown the buffer returns the same
/// code. (Opening a workbook is the one exception: it starts over, and keeps nothing.)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Buffers {
    /// Where a deflated part is inflated and tokenized: at least 64 KiB. Grown on
    /// [`ERR_WINDOW`].
    pub window: *mut u8,
    /// Bytes at `window`.
    pub window_cap: usize,
    /// Text the call produces. Grown on [`ERR_ARENA`].
    pub arena: *mut u8,
    /// Bytes at `arena`.
    pub arena_cap: usize,
    /// Spans the call produces. Grown on [`ERR_CELLS`].
    pub cells: *mut Span,
    /// Spans at `cells`.
    pub cells_cap: usize,
    /// The row being assembled, one [`Slot`] per source column the plan reaches: the
    /// widest ordinal in the plan, plus one. Never grown.
    pub row: *mut Slot,
    /// Slots at `row`.
    pub row_cap: usize,
    /// The workbook's shared strings as `hypertabular_workbook_strings` wrote them: the
    /// bytes, and (in `table`) the span of each. Read, never written.
    pub strings: *const u8,
    /// Bytes at `strings`.
    pub strings_len: usize,
    /// The span of each shared string.
    pub table: *const Span,
    /// Spans at `table`: how many shared strings there are.
    pub table_len: usize,
    /// The kind of each cell format as `hypertabular_workbook_styles` wrote them. Read.
    pub kinds: *const u8,
    /// Bytes at `kinds`: how many cell formats there are.
    pub kinds_len: usize,
}

/// The options of `hypertabular_delimited_init_with`: the dialect, and what a read does before
/// and around its rows. `size` first, as every shape of that surface: a size the core doesn't
/// read is [`ERR_CONTRACT`], so a later core can tell a caller built against this one.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDelimitedOptions {
    /// The bytes the caller built: [`RawDelimitedOptions::SIZE`].
    pub size: u32,
    /// The single-byte separator: tab, or any printable ASCII byte except `"`.
    pub separator: u8,
    /// Nonzero when `"` quotes cells.
    pub quoting: u8,
    /// Nonzero when a completely empty line is skipped rather than read as a one-cell row.
    pub skip_blank_lines: u8,
    /// `0` for the best engine this CPU has; otherwise an engine's code.
    pub engine: u8,
    /// Physical lines to pass over after a byte-order mark. Nothing in them is read, so a quote
    /// in one means nothing.
    pub skip: u32,
    /// Records to discard after that, ahead of the header if there is one. A blank line the
    /// dialect skips isn't a record.
    pub header_row: u32,
    /// A record with fewer cells than the width: [`RawDelimitedOptions::SHORT_ROWS_REJECT`] or
    /// [`RawDelimitedOptions::SHORT_ROWS_PAD`].
    pub short_rows: u32,
    /// A record with more cells than the width: [`RawDelimitedOptions::LONG_ROWS_REJECT`],
    /// [`RawDelimitedOptions::LONG_ROWS_REPORT`] or [`RawDelimitedOptions::LONG_ROWS_TRUNCATE`].
    pub long_rows: u32,
}

impl RawDelimitedOptions {
    /// The layout's size in bytes, which `size` carries.
    pub const SIZE: u32 = 24;

    /// A short record is a structural failure.
    pub const SHORT_ROWS_REJECT: u32 = 0;
    /// A short record is padded: the cells it lacks are `Empty`, at [`CellNote::ORIGIN_PADDED`].
    pub const SHORT_ROWS_PAD: u32 = 1;
    /// A long record is a structural failure.
    pub const LONG_ROWS_REJECT: u32 = 0;
    /// A long record is cut to the width, and the caller means to say so: `hypertabular_faults`
    /// lists the rows `row_widths` shows were cut. To the fill it is
    /// [`RawDelimitedOptions::LONG_ROWS_TRUNCATE`].
    pub const LONG_ROWS_REPORT: u32 = 1;
    /// A long record is cut to the width.
    pub const LONG_ROWS_TRUNCATE: u32 = 2;
}

/// How far `hypertabular_transcode` got. Every field counts this call's input and output, not a
/// whole read's.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawTranscoded {
    /// Bytes of the input decoded: what the next call starts after.
    pub consumed: u64,
    /// Bytes of UTF-8 written, whole characters.
    pub written: u64,
    /// With `failed`, the offset in this call's input of the unit with no character.
    pub failure_at: u64,
    /// `1` when the input holds a unit with no character at `failure_at`, otherwise `0`.
    pub failed: u32,
    /// `0`. It keeps the layout one size on every target.
    pub reserved: u32,
}

impl RawTranscoded {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 32;

    /// `encoding`: Windows-1252, its undefined bytes their C1 controls, as WHATWG has it.
    pub const WINDOWS_1252: u32 = 1;
    /// `encoding`: Latin-1, every byte the code point of its value.
    pub const LATIN_1: u32 = 2;
    /// `encoding`: UTF-16, little-endian.
    pub const UTF_16LE: u32 = 3;
}

/// What `hypertabular_batch_check` found of a read's sizes. See [`crate::kernel::size`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawBatchCheck {
    /// The bytes the batch asks for, or `u64::MAX` for an ask too big to count. `0` for
    /// `batch_rows` of `0`.
    pub needed: u64,
    /// The most bytes a batch may ask for.
    pub limit: u64,
    /// The size to start the input buffer at: what was asked, up to the longest record a reader
    /// will buffer. `0` for a sheet, which has no input buffer.
    pub buffer_bytes: u64,
    /// The rows of a batch for a sampler that wants `batch_rows` rows, however many batches that
    /// then takes. The core works this out so that no binding does. `0` where one row is past
    /// `limit` or `batch_rows` is `0`.
    pub sample_rows: u64,
    /// The columns one row of the read holds: the source columns the plan reaches, or a sheet's
    /// plan columns where it has more, whichever drives the row's count.
    pub width: u64,
    /// Why the sizes were refused: one of the `REFUSED_` codes.
    pub refused: u32,
    /// `0`. It keeps the layout one size on every target.
    pub reserved: u32,
}

impl RawBatchCheck {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 48;

    /// Flag: each column holds a note beside each verdict.
    pub const NOTES: u32 = 1;
    /// Flag: the read keeps each row's width, as a delimited read reporting long rows does.
    pub const WIDTHS: u32 = 2;
    /// Flag: a sheet, whose cell table is a whole batch and which has no input buffer.
    pub const SHEET: u32 = 4;
    /// `refused`: nothing was refused.
    pub const REFUSED_NONE: u32 = 0;
    /// `refused`: `batch_rows` is `0`.
    pub const REFUSED_BATCH_ROWS_ZERO: u32 = 1;
    /// `refused`: `buffer_bytes` is `0`.
    pub const REFUSED_BUFFER_BYTES_ZERO: u32 = 2;
    /// `refused`: `needed` is past `limit`.
    pub const REFUSED_OVER_LIMIT: u32 = 3;
    /// `refused`: one row alone is past `limit`, at any `batch_rows`; `needed` is that row's.
    pub const REFUSED_ROW_TOO_WIDE: u32 = 4;

    /// Every flag this core defines.
    pub(crate) const FLAGS: u32 = Self::NOTES | Self::WIDTHS | Self::SHEET;
}

/// What a verdict doesn't say about one cell: why, in HyperCast's terms, and where the verdict
/// came from.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellNote {
    /// HyperCast's `Detail` code for the fault; `0` where no door refused the cell: a cell that
    /// cast, one at [`CellNote::ORIGIN_TOKEN`] or [`CellNote::ORIGIN_PADDED`], or a workbook's typed
    /// cell of a kind the door has no twin for, a boolean through an integer door, say.
    pub detail: u16,
    /// Where the verdict came from: one of the `ORIGIN_` codes.
    pub origin: u16,
}

impl CellNote {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 4;

    /// A door judged the cell's own text.
    pub const ORIGIN_CELL: u16 = 0;
    /// The cell's exact unquoted bytes are a declared token, so it is `Empty` and no door ran. A
    /// workbook's text cells alone can be one: a number whose text would match is a number.
    pub const ORIGIN_TOKEN: u16 = 1;
    /// The row has no such cell: padding for a short row, or an ordinal past the row's end,
    /// which in a workbook is past the last cell it holds, every ordinal of an empty row
    /// included. It is `Empty` and no door ran.
    pub const ORIGIN_PADDED: u16 = 2;
    /// A workbook's formula cell with no cached value: XLSX's `<f>` with no `<v>`, or an empty one
    /// unless the cell is declared a string, and ODS's `table:formula` with no value. It is `Empty`
    /// and no door ran, and through the 1.0 fill it's simply `Empty`.
    pub const ORIGIN_UNCACHED: u16 = 3;
}

/// One thing `hypertabular_faults` lists: a cell a door refused, a formula with no cached value, or
/// a long row the read declared it would report.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawProblem {
    /// The row in the batch, from zero.
    pub row: u32,
    /// The column of the plan, or [`RawProblem::ROW_LEVEL`] for an entry about the row as a whole.
    pub plan_column: u32,
    /// The verdict's reason, HyperCast's code: `2` malformed, `3` out of range,
    /// [`CellVerdict::VIOLATION`] for a constraint broken, or `1` empty for a formula with no
    /// cached value. `0` on a row-level entry.
    pub reason: u32,
    /// The note's [`CellNote::detail`]: `0` on a row-level entry, or where the column had no notes.
    pub detail: u16,
    /// The note's [`CellNote::origin`]: `0` on a row-level entry, or where the column had no notes.
    pub origin: u16,
    /// The verdict's span within the cell's text; `0` on a row-level entry.
    pub offset: u32,
    /// The verdict's span's length; `0` on a row-level entry.
    pub len: u32,
    /// On a row-level entry, the width the first record fixed; otherwise `0`.
    pub expected: u32,
    /// On a row-level entry, the cells the row had; otherwise `0`.
    pub found: u32,
    /// On a [`CellVerdict::VIOLATION`], the keyword broken, as the column's
    /// [`ColumnBufferWith::checks`] names it (HyperCast's codes, `1` minimum … `8` enum); otherwise
    /// `0`.
    pub keyword: u32,
}

impl RawProblem {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 36;

    /// What [`RawProblem::plan_column`] says for an entry about the row as a whole.
    pub const ROW_LEVEL: u32 = u32::MAX;
}

/// What `hypertabular_suggest` says of one column of a sample. Nothing reads it but its caller:
/// a suggestion is an answer to look at, and is never applied.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawSuggestion {
    /// The door, as [`crate::kernel::door::Door::code`] numbers it: bool `1`, `i32` `4`, `i64` `5`,
    /// `f64` `11`, UUID `12`, timestamp `13`, ISO date `15`, text `18` or a date under a declared
    /// order `20`. Under [`RawSuggestion::UNRESOLVED`] it's text. Under
    /// [`RawSuggestion::AMBIGUOUS`] it's the door under the first candidate.
    pub door: u32,
    /// What the door declares. Under [`RawSuggestion::AMBIGUOUS`] it's a set of candidates to
    /// choose from, bit `n` for code `n`, and no door can read it as it stands; otherwise it's one
    /// code, which says what a column spec for `door` takes.
    ///
    /// - A date under a declared order: the order's code, `1` year-month-day, `2` month-day-year,
    ///   `3` day-month-year. Candidates `12` are the last two.
    /// - A numeric door: resolved, `0` for the format the call declared for the column or else the
    ///   first notation, [`RawSuggestion::NOTATION_COMMA_GROUPS`] where a `,` in the sample proved
    ///   that notation, or [`RawSuggestion::NOTATION_COMMA_DECIMAL`] where only that one reads the
    ///   column, each for the spec's `format` and not its `param`. A binding's own plan reads the
    ///   second where it's named and otherwise, with nothing declared, the first, under
    ///   [`RawSuggestion::NOTATION_FLAGS`], so that a later `1,23,456` is refused. Candidates are
    ///   [`RawSuggestion::NOTATION_COMMA_GROUPS`] and [`RawSuggestion::NOTATION_COMMA_DECIMAL`].
    /// - Any other door: `0`.
    pub param: u32,
    /// [`RawSuggestion::RESOLVED`], [`RawSuggestion::AMBIGUOUS`] or
    /// [`RawSuggestion::UNRESOLVED`].
    pub status: u32,
    /// The cells the door took. Resolved or ambiguous, that's every cell `sampled` counts.
    /// Unresolved, it's the most that any one door but the text door took, `0` when none did.
    pub matched: u32,
    /// The column's cells that aren't blank: a cell with nothing but white space in it is blank.
    pub sampled: u32,
}

impl RawSuggestion {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 20;

    /// `status`: one door and one declaration take every cell.
    pub const RESOLVED: u32 = 1;
    /// `status`: every cell is read, but the sample can't say which declaration it's written in.
    pub const AMBIGUOUS: u32 = 2;
    /// `status`: there's nothing to go on, or a cell no door took beside cells one did, or a cell
    /// that isn't text at all. Read it as text.
    pub const UNRESOLVED: u32 = 3;

    /// A notation in `param`: `,` groups digits in threes and `.` is the decimal separator,
    /// HyperCast's `NumFormat::INVARIANT`, the same format as a plan with none.
    pub const NOTATION_COMMA_GROUPS: u32 = 1 << 1;
    /// A notation in `param`: `,` is the decimal separator and `.` groups digits in threes, every
    /// default lenience on.
    pub const NOTATION_COMMA_DECIMAL: u32 = 1 << 2;
    /// The `flags` of a `RawNumFormat` for either notation, which a binding building one from
    /// `param` needs: every default lenience, and Western groups of three, whose policy is no bits.
    pub const NOTATION_FLAGS: u32 = 95;
}

/// One output column as `hypertabular_delimited_fill_with` and `hypertabular_workbook_fill_with`
/// take it: [`ColumnSpec`], with date options, a month vocabulary and tokens.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSpecWith {
    /// The bytes the caller built: [`ColumnSpecWith::SIZE`].
    pub size: u32,
    /// As [`ColumnSpec::ordinal`].
    pub ordinal: u32,
    /// As [`ColumnSpec::door`].
    pub door: u32,
    /// As [`ColumnSpec::param`].
    pub param: u32,
    /// As [`ColumnSpec::format`], and judged as HyperCast's `NumFormat::validate` judges it: a
    /// flag bit it doesn't define is [`ERR_CONTRACT`] here.
    pub format: RawNumFormat,
    /// What the date doors read: HyperCast's size-tagged layout. All zeros, a `size` of `0`, is
    /// no options.
    pub date: RawDateOptions,
    /// A month vocabulary for the date doors, packed as HyperCast's `MonthNames` packs it; null
    /// for none. Judged on every call.
    pub months: *const u8,
    /// Bytes at `months`.
    pub months_len: usize,
    /// This column's tokens: each a little-endian `u32` length, then its bytes. Null inherits
    /// the list in [`BuffersWith::tokens`]; anything else replaces it, so an empty list means
    /// none.
    pub tokens: *const u8,
    /// Bytes at `tokens`.
    pub tokens_len: usize,
    /// What the column's cells must also meet: HyperCast's 104-byte `RawConstraints`, judged as
    /// its `Constraints::from_raw` judges it, at a numeric door or text alone, and with
    /// [`ColumnBufferWith::checks`] set. Null declares none, and the column casts as it would
    /// without this field. A declaration refused, at any other door, or without `checks` is
    /// [`ERR_CONTRACT`] before any cell is read, as is a decimal in it whose sign byte is other
    /// than `0` or `1`, which is read as a byte before the layout is.
    pub constraints: *const RawConstraints,
    /// The `enum` list, packed as HyperCast packs it, read only when `constraints` sets
    /// `HAS_ENUM`; null and `0` otherwise.
    pub enum_ptr: *const u8,
    /// Bytes at `enum_ptr`.
    pub enum_len: usize,
}

impl ColumnSpecWith {
    /// The layout's size in bytes: four words, HyperCast's 92-byte `RawNumFormat` and 12-byte
    /// `RawDateOptions`, then three pointers and their lengths, the constraints between them.
    pub const SIZE: u32 = size_of::<ColumnSpecWith>() as u32;

    /// A column over `ordinal` through the door `door`/`param`: invariant notation, no options,
    /// the tokens the fill was given.
    pub const fn new(ordinal: u32, door: u32, param: u32) -> ColumnSpecWith {
        ColumnSpecWith {
            size: ColumnSpecWith::SIZE,
            ordinal,
            door,
            param,
            format: RawNumFormat {
                size: 0,
                ..RawNumFormat::ZERO
            },
            date: RawDateOptions {
                size: 0,
                flags: 0,
                pivot: 0,
            },
            months: core::ptr::null(),
            months_len: 0,
            tokens: core::ptr::null(),
            tokens_len: 0,
            constraints: core::ptr::null(),
            enum_ptr: core::ptr::null(),
            enum_len: 0,
        }
    }
}

/// Where one column's output goes in `hypertabular_delimited_fill_with` and
/// `hypertabular_workbook_fill_with`: [`ColumnBuffer`], with a note per row.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ColumnBufferWith {
    /// The bytes the caller built: [`ColumnBufferWith::SIZE`].
    pub size: u32,
    /// The value array, as [`ColumnBuffer::values`].
    pub values: *mut c_void,
    /// The verdict array.
    pub verdicts: *mut CellVerdict,
    /// `max_rows` notes, not aligned, or null to write none. In delimited text, a column with no
    /// notes, no tokens and no date options runs the same loop `hypertabular_delimited_fill` does.
    pub notes: *mut CellNote,
    /// `max_rows` checks, aligned, HyperCast's `RawCheck` as its `cast_checked` writes one: on a
    /// [`CellVerdict::VIOLATION`] the keyword, on a cell that cast whether rounding adjusted it and
    /// the `enum` entry a text matched. Required when the column declares constraints; never read
    /// or written when it declares none, so null there. A row whose verdict isn't from a door (a
    /// token, padding, an uncached formula) or is a fault is left as it was.
    pub checks: *mut RawCheck,
}

impl ColumnBufferWith {
    /// The layout's size in bytes: five words.
    pub const SIZE: u32 = size_of::<ColumnBufferWith>() as u32;
}

/// The memory `hypertabular_delimited_fill_with` and `hypertabular_workbook_fill_with` work in:
/// the fields of [`Buffers`], of which the delimited fill reads `cells` and `arena` and the workbook
/// fill all, then what only they read.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BuffersWith {
    /// The bytes the caller built: [`BuffersWith::SIZE`].
    pub size: u32,
    /// As [`Buffers::window`].
    pub window: *mut u8,
    /// As [`Buffers::window_cap`].
    pub window_cap: usize,
    /// As [`Buffers::arena`].
    pub arena: *mut u8,
    /// As [`Buffers::arena_cap`].
    pub arena_cap: usize,
    /// As [`Buffers::cells`].
    pub cells: *mut Span,
    /// As [`Buffers::cells_cap`].
    pub cells_cap: usize,
    /// As [`Buffers::row`].
    pub row: *mut Slot,
    /// As [`Buffers::row_cap`].
    pub row_cap: usize,
    /// As [`Buffers::strings`].
    pub strings: *const u8,
    /// As [`Buffers::strings_len`].
    pub strings_len: usize,
    /// As [`Buffers::table`].
    pub table: *const Span,
    /// As [`Buffers::table_len`].
    pub table_len: usize,
    /// As [`Buffers::kinds`].
    pub kinds: *const u8,
    /// As [`Buffers::kinds_len`].
    pub kinds_len: usize,
    /// The tokens every column inherits: each a little-endian `u32` length, then its bytes. Null
    /// for none.
    pub tokens: *const u8,
    /// Bytes at `tokens`.
    pub tokens_len: usize,
    /// `max_rows` counts, aligned, of the cells each delivered row had before padding or
    /// cutting; null to write none. A workbook row's count runs to the last column holding a cell
    /// inside the range, whatever the plan reads, and is `0` for an empty row. A cell its row names
    /// again as empty still counts toward it.
    pub row_widths: *mut u32,
}

impl BuffersWith {
    /// The layout's size in bytes: eighteen words.
    pub const SIZE: u32 = size_of::<BuffersWith>() as u32;

    /// Every buffer null, every length `0`, with `size` set: what to start a call's from.
    pub const fn empty() -> BuffersWith {
        BuffersWith {
            size: BuffersWith::SIZE,
            window: core::ptr::null_mut(),
            window_cap: 0,
            arena: core::ptr::null_mut(),
            arena_cap: 0,
            cells: core::ptr::null_mut(),
            cells_cap: 0,
            row: core::ptr::null_mut(),
            row_cap: 0,
            strings: core::ptr::null(),
            strings_len: 0,
            table: core::ptr::null(),
            table_len: 0,
            kinds: core::ptr::null(),
            kinds_len: 0,
            tokens: core::ptr::null(),
            tokens_len: 0,
            row_widths: core::ptr::null_mut(),
        }
    }

    /// The fields [`Buffers`] has.
    pub const fn buffers(&self) -> Buffers {
        Buffers {
            window: self.window,
            window_cap: self.window_cap,
            arena: self.arena,
            arena_cap: self.arena_cap,
            cells: self.cells,
            cells_cap: self.cells_cap,
            row: self.row,
            row_cap: self.row_cap,
            strings: self.strings,
            strings_len: self.strings_len,
            table: self.table,
            table_len: self.table_len,
            kinds: self.kinds,
            kinds_len: self.kinds_len,
        }
    }
}

/// How `hypertabular_workbook_sheet_with` reads a sheet: `hypertabular_workbook_sheet`'s two
/// switches, then what's passed over and the range. The order is fixed: the range selects rows and
/// columns, `skip` passes over rows inside it, `header_row` discards rows after those, then comes
/// the header, and the filter holds throughout. See `docs/workbook.md`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawSheetOptions {
    /// The bytes the caller built: [`RawSheetOptions::SIZE`].
    pub size: u32,
    /// Nonzero when the first row delivered is the header.
    pub has_header: u32,
    /// Nonzero when rows with no cell in them, and the rows a gap in the numbering stands for, are
    /// left out. Such a row still counts toward `skip`, and doesn't toward `header_row`. A row whose
    /// only cells are formulas with nothing spelled where a value goes counts as one, and nothing
    /// says it was left out. That depends on the writer: `<f>` alone or with `<v />` counts as empty,
    /// and with `<v></v>` holds a cell, so a table of nothing but formulas from the first kind of
    /// writer can read as no rows. With this zero those rows are delivered, their cells at
    /// [`CellNote::ORIGIN_UNCACHED`].
    pub skip_empty_rows: u32,
    /// Rows of the sheet to pass over at the start of the range, empty ones included. Their cells
    /// aren't judged, so a bad cell there doesn't matter; the XML is still tokenized, or for Numbers
    /// stepped over by its offsets.
    pub skip: u32,
    /// Rows to read and discard after those, as a filtered read delivers them.
    pub header_row: u32,
    /// The range's first row, 1-based like `A5:F200`; `0` for no limit on that side.
    pub first_row: u32,
    /// Its first column, 1-based, so `A` is `1`; `0` for no limit. Ordinal `0` of a plan is this
    /// column.
    pub first_column: u32,
    /// Its last row, 1-based and included; `0` for no limit. A read ends here: no row past it is
    /// read, so a failure in one isn't met. What opening a workbook reads is met whatever the
    /// range: all of an ODS's `content.xml`, and every chunk of a Numbers document's parts.
    pub last_row: u32,
    /// Its last column, 1-based and included; `0` for no limit.
    pub last_column: u32,
}

impl RawSheetOptions {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 36;
}

/// What `hypertabular_range_parse` makes of `A5:F200`: each 1-based, `5`, `1`, `200`, `6`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawRange {
    /// The first row.
    pub first_row: u32,
    /// The first column, `A` being `1`.
    pub first_column: u32,
    /// The last row, included.
    pub last_row: u32,
    /// The last column, included.
    pub last_column: u32,
}

impl RawRange {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 16;
}

/// What `hypertabular_workbook_open` found.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    /// `1` XLSX, `2` ODS, `3` Numbers.
    pub format: u32,
    /// The workbook's date system: HyperCast's `ExcelEpoch` code (`1` 1900, `2` 1904).
    pub epoch: u32,
    /// An upper bound on the bytes the shared strings take: the size their part inflates
    /// to (Numbers: the size the parts that hold its tables' strings decode to). The
    /// directory's figure, held to what the part's compressed bytes can inflate to (1032
    /// to one), so a buffer sized by it is never larger than the part could fill. Zero if
    /// the workbook has none.
    pub strings_bytes: u64,
    /// How many shared strings the part says it has. A hint: the file's word, unchecked;
    /// Numbers says nothing, and it is zero.
    pub strings_count: u64,
    /// On [`ERR_WINDOW`] or [`ERR_ARENA`], the size that buffer needs.
    pub needed: u64,
    /// On [`ERR_STRUCTURE`], what is wrong and where.
    pub failure: Failure,
}

impl Opened {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 64;
}

/// One sheet as `hypertabular_workbook_sheets` lists it: three spans' worth of the table.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SheetInfo {
    /// The sheet's name, in the arena.
    pub name: Span,
    /// The part that holds it, in the arena: what `hypertabular_workbook_sheet` is given
    /// to open it. Empty for ODS and Numbers, where every sheet is reached another way.
    pub part: Span,
    /// Bit 0: the sheet is hidden.
    pub flags: u32,
    /// The sheet's position among the document's tables (ODS, Numbers), given back
    /// likewise.
    pub index: u32,
}

impl SheetInfo {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 24;
}
