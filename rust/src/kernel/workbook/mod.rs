//! The workbook core: XLSX and ODS read out of a container the caller holds, into memory
//! the caller holds.
//!
//! The container is bytes — a file the binding read or mapped. The core walks its zip
//! directory in place, inflates one part at a time through a window of the caller's, and
//! tokenizes the XML as it comes out; nothing is ever held but the caller's state block,
//! which is offsets and counters and the inflate tables, and never a pointer. A binding
//! therefore drives a workbook the way it drives delimited text — one call a batch, every
//! byte of working memory its own — with one difference the format forces: a read through
//! a compressed stream cannot be rewound, so a call that runs out of room stops where it
//! is and is called again once the caller has made room ([`Buffers`] has the contract).
//!
//! The calls, in the order a binding makes them:
//!
//! 1. [`book::open`] — is it a workbook, which kind, which date system, how large its
//!    shared strings are.
//! 2. [`book::sheets`] — the sheets' names and where each one is.
//! 3. [`book::strings`] and [`book::styles`] (XLSX) — the two tables every sheet reads
//!    its cells against, loaded once into buffers the caller then hands back, read-only.
//! 4. [`rows::sheet`] — position on one sheet; then [`rows::header`] and [`rows::fill`],
//!    which casts a batch of rows through the caller's plan into the caller's columns in
//!    the layout a delimited batch has.
//!
//! Each sheet being read wants a state block of its own; a block is plain data, so one
//! that has been opened may be copied to start another.
//!
//! [`Buffers`]: crate::kernel::abi::Buffers

pub mod book;
pub mod cell;
pub mod number;
pub mod part;
pub mod rows;
pub mod sort;
pub mod styles;
pub mod xml;
pub mod zip;

use crate::kernel::abi::{
    ERR_ARENA, ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE, ERR_WINDOW, Failure, Slot, Span,
};
use crate::kernel::inflate::Inflate;
use hypercast::ExcelEpoch;
use part::{Part, Reader, Stop};
use zip::Directory;

/// An XLSX workbook.
pub const FORMAT_XLSX: u32 = 1;
/// An ODS workbook.
pub const FORMAT_ODS: u32 = 2;

/// What a state block that [`book::open`] finished carries; anything else is not a
/// workbook's state.
const OPENED: u32 = 0x4B42_5748;

const OP_NONE: u32 = 0;
const OP_SHEETS: u32 = 1;
const OP_STRINGS: u32 = 2;
const OP_STYLES: u32 = 3;
const OP_ROWS: u32 = 4;

/// Everything the core remembers between calls: the decoder, the read position in the
/// part being read, where the workbook's parts are, and how far the call in progress has
/// got. Plain integers throughout — a block of any bits is safe to hand in, and one that
/// was not opened is refused.
#[repr(C)]
pub struct State {
    inflate: Inflate,
    part: Part,
    directory: Directory,
    opened: u32,
    format: u32,
    epoch: u32,
    /// Which call is in progress (`OP_*`), and how far into it.
    op: u32,
    phase: u32,
    depth: u32,
    flag: u32,
    reserved: u32,
    /// Central-directory offsets of the workbook's parts, plus one; zero for none.
    workbook: u64,
    rels: u64,
    styles: u64,
    strings: u64,
    content: u64,
    count: u64,
    extra: u64,
    used: u64,
    mark: u64,
    rows: rows::Sheet,
}

impl Default for State {
    fn default() -> State {
        State::new()
    }
}

impl State {
    /// A block that has opened nothing; [`book::open`] is what makes one of use. (A
    /// binding hands in bytes of its own and never calls this — `open` asks nothing of
    /// what the block held before.)
    pub fn new() -> State {
        State {
            inflate: Inflate::new(),
            part: Part::default(),
            directory: Directory::default(),
            opened: 0,
            format: 0,
            epoch: 0,
            op: OP_NONE,
            phase: 0,
            depth: 0,
            flag: 0,
            reserved: 0,
            workbook: 0,
            rels: 0,
            styles: 0,
            strings: 0,
            content: 0,
            count: 0,
            extra: 0,
            used: 0,
            mark: 0,
            rows: rows::Sheet::default(),
        }
    }

    /// The date system as HyperCast names it.
    fn system(&self) -> ExcelEpoch {
        // Only the workbook part's `date1904` sets the 1904 code; anything else is 1900.
        match ExcelEpoch::from_code(self.epoch) {
            Some(epoch) => epoch,
            None => ExcelEpoch::Y1900,
        }
    }

    /// Takes up `op`, or goes on with it if it is the call that was refused last time.
    /// Returns true when the call starts afresh.
    fn enter(&mut self, op: u32) -> bool {
        if self.op == op && self.phase != DONE {
            return false;
        }
        self.op = op;
        self.phase = 0;
        self.depth = 0;
        self.flag = 0;
        self.count = 0;
        self.extra = 0;
        self.used = 0;
        self.mark = 0;
        true
    }
}

/// The phase of a call that has run to its end.
const DONE: u32 = u32::MAX;

/// The buffers of one call, as slices.
pub struct Memory<'a> {
    pub window: &'a mut [u8],
    pub arena: &'a mut [u8],
    pub cells: &'a mut [Span],
    pub row: &'a mut [Slot],
    pub strings: &'a [u8],
    pub table: &'a [Span],
    pub kinds: &'a [u8],
}

/// The return code a stop is, with what the caller needs to know written where it looks.
fn refuse(stop: Stop, needed: &mut u64, failure: &mut Failure) -> i32 {
    match stop {
        Stop::Window(size) => {
            *needed = size;
            ERR_WINDOW
        }
        Stop::Arena(size) => {
            *needed = size;
            ERR_ARENA
        }
        Stop::Cells(size) => {
            *needed = size;
            ERR_CELLS
        }
        Stop::Fail(what) => {
            *failure = what;
            ERR_STRUCTURE
        }
        Stop::Contract => ERR_CONTRACT,
    }
}

/// A failure that is not in any part's XML.
fn failure(code: u32, part: u32, found: u32) -> Stop {
    Stop::Fail(Failure {
        code,
        line: 0,
        record: u64::from(part),
        byte: 0,
        expected: 0,
        found,
    })
}

/// Room for `more` bytes at `used` in an arena of `len`, or the stop that asks for it.
/// Text a span cannot address is a failure, not a request.
fn room(len: usize, used: u64, more: u64, part: &Part) -> Result<(), Stop> {
    let end = used.saturating_add(more);
    if end > u64::from(u32::MAX) || more >= u64::from(Span::FLAG) {
        return Err(Stop::Fail(part.failure(Failure::TOO_LARGE, 0)));
    }
    if end > len as u64 {
        // Ask for double, so a long run of small appends is a few calls and not many.
        return Err(Stop::Arena(end.max((len as u64).saturating_mul(2))));
    }
    Ok(())
}

/// Positions `reader` on the part whose central header is at `at - 1`, or on nothing if
/// `at` is zero.
fn begin(reader: &mut Reader<'_>, directory: &Directory, at: u64, id: u32) -> Result<(), Stop> {
    let Some(at) = at.checked_sub(1) else {
        reader.part.begin_empty(id);
        return Ok(());
    };
    let entry = directory
        .entry(reader.container, at)
        .ok_or(failure(Failure::CONTAINER, id, 0))?;
    let located =
        zip::locate(reader.container, &entry).map_err(|(code, found)| failure(code, id, found))?;
    reader.part.begin(reader.inflate, located, id);
    Ok(())
}
