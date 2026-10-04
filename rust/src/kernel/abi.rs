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
