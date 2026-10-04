//! The shapes that cross the C ABI. Every one is `#[repr(C)]`, holds no pointer the core
//! keeps, and is memory the caller owns: the core reads what it is handed and writes where
//! it is told, and remembers nothing between calls but what is in the caller's state block.

use core::ffi::c_void;
use hypercast::{Fault, NumFormat, RawNumFormat, Reason};

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
    /// The flag bit of `len`. On a cell-table entry: the cell is quoted with `""` inside
    /// and has to be unescaped to be read. On a `Text` value: the bytes are in the arena
    /// rather than in the input.
    pub const FLAG: u32 = 1 << 31;

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
    /// `0` ok, `1` empty, `2` malformed, `3` out of range — HyperCast's ABI codes.
    pub reason: u32,
}

impl CellVerdict {
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
        let reason = match self.reason {
            1 => Reason::Empty,
            2 => Reason::Malformed,
            3 => Reason::OutOfRange,
            _ => return None,
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
    /// The door's code ([`crate::kernel::door::Door::code`]).
    pub door: u32,
    /// What the door declares, where it declares something: the Unix precision, a date
    /// order, an Excel date system — numbered as HyperCast numbers them. `0` otherwise.
    pub param: u32,
    /// The numeric notation, read by the numeric doors. All zeros is the invariant
    /// notation.
    pub format: RawNumFormat,
}

impl ColumnSpec {
    /// A column over `ordinal` through the door `door`/`param`, invariant notation.
    pub const fn new(ordinal: u32, door: u32, param: u32) -> ColumnSpec {
        ColumnSpec {
            ordinal,
            door,
            param,
            format: RawNumFormat {
                decimal_sep: 0,
                group_sep: 0,
                flags: 0,
                currency_len: 0,
                currency: [0; 16],
            },
        }
    }

    /// The declared notation, or `None` for a contract violation.
    pub fn num_format(&self) -> Option<NumFormat> {
        if self.format.decimal_sep == 0 {
            return Some(NumFormat::INVARIANT);
        }
        self.format.resolve()
    }
}

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
    /// the first record's.
    pub code: u32,
    /// One-based line the offending record starts on.
    pub line: u32,
    /// Zero-based index of the offending record, header and skipped blank lines included.
    pub record: u64,
    /// Absolute byte offset of the offending record's start.
    pub byte: u64,
    /// Cells in the first record (code `2`).
    pub expected: u32,
    /// Cells in this record (code `2`).
    pub found: u32,
}

impl Failure {
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
    /// The container is a zip but neither an XLSX nor an ODS workbook.
    pub const NOT_A_WORKBOOK: u32 = 23;
    /// A cell names a shared string by something that is not a number (`found` `0`,
    /// `expected` `0`), or by an index past the table (`found` the index, `expected` the
    /// table's length).
    pub const SHARED_STRING: u32 = 24;
    /// More text than a span can address: over 4 GiB in one batch or in the shared
    /// strings, or 2 GiB in one cell.
    pub const TOO_LARGE: u32 = 25;

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

/// One cell of the row a workbook read is assembling: a tag and a payload, plain integers.
/// The caller supplies the scratch these live in ([`Buffers::row`]) and never reads them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub(crate) tag: u32,
    pub(crate) aux: u32,
    pub(crate) bits: u64,
}

/// The memory a workbook call works in — all of it the caller's, handed over again on
/// every call. A call uses the buffers its own documentation names and ignores the rest
/// (null with a zero size is fine for those).
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

/// What `hypertabular_workbook_open` found.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    /// `1` XLSX, `2` ODS.
    pub format: u32,
    /// The workbook's date system: HyperCast's `ExcelEpoch` code (`1` 1900, `2` 1904).
    pub epoch: u32,
    /// An upper bound on the bytes the shared strings take: the size their part inflates
    /// to. Zero if the workbook has none.
    pub strings_bytes: u64,
    /// How many shared strings the part says it has. A hint: the file's word, unchecked.
    pub strings_count: u64,
    /// On [`ERR_WINDOW`] or [`ERR_ARENA`], the size that buffer needs.
    pub needed: u64,
    /// On [`ERR_STRUCTURE`], what is wrong and where.
    pub failure: Failure,
}

/// One sheet as `hypertabular_workbook_sheets` lists it: three spans' worth of the table.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SheetInfo {
    /// The sheet's name, in the arena.
    pub name: Span,
    /// The part that holds it, in the arena: what `hypertabular_workbook_sheet` is given
    /// to open it. Empty for ODS, where every sheet is in one part.
    pub part: Span,
    /// Bit 0: the sheet is hidden.
    pub flags: u32,
    /// The sheet's position among the document's tables (ODS), given back likewise.
    pub index: u32,
}
