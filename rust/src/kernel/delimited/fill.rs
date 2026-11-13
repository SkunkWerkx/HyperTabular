//! Delimited text into the caller's column buffers.
//!
//! One call takes a chunk of input and everything it will write into, and does two
//! things. It walks the chunk for whole rows, recording where each cell the plan reads
//! begins and ends in a table the caller supplied. Then it casts one column at a time —
//! one door, one loop, straight down the table — into that column's value and verdict
//! arrays. It reports how many rows it wrote and how many bytes it is finished with;
//! streaming is the caller putting what was not consumed at the front of the next chunk.
//!
//! The memory, all of it the caller's:
//!
//! - **the input** — any chunk that starts on a row boundary, which is where every
//!   `consumed` count ends. A row that does not fit a chunk is the caller's to make room
//!   for; the core only says it took nothing.
//! - **the state block** ([`State`]): where the input stands (line, record, byte offset), the
//!   width the first record fixed, and a failure once there is one, in the first 64 bytes; what
//!   [`State::init_with`] was asked for in the rest. A block from [`State::init`] is the 64 bytes
//!   alone, which is all the 1.0 surface ever said it was.
//! - **the columns** — per plan column, `max_rows` values and `max_rows` verdicts, and with
//!   [`fill_with`] optionally `max_rows` notes.
//! - **the cell table** — `max_rows * (widest ordinal + 2)` [`Span`]s. Row `r`'s entry for
//!   source column `c` is at `r * (width + 1) + c` and locates that cell in the input
//!   (flagged when the cell has `""` inside and must be unescaped to be read); the entry
//!   after a row's last column is the row's own start offset and, in `len`, its line
//!   number. It is scratch for the core and a map for the caller: the raw text of any
//!   cell, one that failed to cast included, is the input at that span.
//! - **the arena** — where an escaped cell is unescaped. A `Text` value is a span into
//!   the input, and flagged when it is a span into the arena instead. A file with no `""`
//!   in it never touches the arena. The bytes a `Text` value names are valid UTF-8: the text
//!   door gives anything else a `Malformed` verdict, without copying.
//!
//! Nothing is retained between calls except what the state block holds, and nothing here
//! allocates, blocks, or reads a clock.

use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith, ERR_ARENA,
    ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE, Failure, Filled, OK, RawDelimitedOptions, Span,
    no_format,
};
use crate::kernel::checked::{Checks, resolved_constraints, text_checked};
use crate::kernel::delimited::engine::{self, Kind};
use crate::kernel::delimited::scan::{Flow, Scanner, Sink, Stop};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use hypercast::detailed::{self, Rejection};
use hypercast::{
    Checked, CivilDateTime, Date, DateOptions, Decimal, Duration, Fault, MonthNames, NumFormat,
    RawCheck, RawDateOptions, Reason, Timestamp, ValidDateOptions, ValidNumFormat,
};

/// The dialect as the caller declares it. Nothing is sniffed.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDialect {
    /// The single-byte separator: tab, or any printable ASCII byte except `"`.
    pub separator: u8,
    /// Nonzero when `"` quotes cells (RFC 4180, `""` for a literal quote).
    pub quoting: u8,
    /// Nonzero when a completely empty line is skipped rather than read as a one-cell row.
    pub skip_blank_lines: u8,
    /// `0` for the best engine this CPU has; otherwise an engine's code, which must be one
    /// this CPU can run. For tests and benchmarks that compare them.
    pub engine: u8,
}

impl RawDialect {
    /// True for a separator the scanner can honour: tab, or printable ASCII other than
    /// the quote. (`\r`, `\n`, NUL and other control bytes are structural or padding.)
    pub const fn is_valid_separator(separator: u8) -> bool {
        separator == b'\t' || (separator >= 0x20 && separator <= 0x7E && separator != b'"')
    }
}

/// Where one input stands. The caller owns it, zero-initialized or not, and hands it to
/// [`State::init`] once before anything else. Plain integers throughout, so that no value
/// a caller could put in it is one the core cannot survive, with one exception: `extended` says
/// how large the block is, so a 64-byte block with that byte set is read and written 16 bytes past
/// its end. A block that came from `init` or `init_with`, and was left alone, says the truth.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// The dialect, as `init` copied it in. The core's own, as `started`, `extended`,
    /// `reserved` and all that follows `failure` are: a caller reads the other fields and
    /// writes none of them.
    separator: u8,
    quoting: u8,
    skip_blank_lines: u8,
    engine: u8,
    /// One-based line number of the next unread byte.
    pub line: u32,
    /// Records finished so far — the header and skipped blank lines included.
    pub records: u64,
    /// Absolute byte offset of the next unread byte.
    pub offset: u64,
    /// Cells per record, fixed by the first; `0` before it.
    pub expected: u32,
    started: u8,
    /// Nonzero when the block has the fields below: it came from [`State::init_with`]. A block
    /// from [`State::init`] may be the 64 bytes the 1.0 surface said a state is, so its reader
    /// never looks past them.
    extended: u8,
    reserved: [u8; 2],
    /// The structural failure that ended this input, if `code` is nonzero.
    pub failure: Failure,
    /// Physical lines still to pass over.
    skip: u32,
    /// Records still to discard.
    header_row: u32,
    /// Nonzero when a short record is padded.
    short_rows: u8,
    /// Nonzero when a long record is cut.
    long_rows: u8,
    tail: [u8; 6],
}

impl RawDialect {
    /// The layout's size in bytes.
    pub const SIZE: u32 = 4;
}

impl State {
    /// The layout's size in bytes, which `hypertabular_delimited_state_size` also reports.
    pub const SIZE: u32 = 80;

    /// What a block from [`State::init`] is, and all the 1.0 surface reads and writes of one.
    const FIRST_SIZE: usize = 64;

    /// Every field zero.
    const ZERO: State = State {
        separator: 0,
        quoting: 0,
        skip_blank_lines: 0,
        engine: 0,
        line: 0,
        records: 0,
        offset: 0,
        expected: 0,
        started: 0,
        extended: 0,
        reserved: [0; 2],
        failure: Failure {
            code: 0,
            line: 0,
            record: 0,
            byte: 0,
            expected: 0,
            found: 0,
        },
        skip: 0,
        header_row: 0,
        short_rows: 0,
        long_rows: 0,
        tail: [0; 6],
    };
}

// The two layouts pinned as crate::kernel::abi pins the rest, here because the state's
// private fields are visible only in this module.
const _: () = {
    use core::mem::offset_of;
    assert!(size_of::<RawDialect>() == RawDialect::SIZE as usize && align_of::<RawDialect>() == 1);
    assert!(offset_of!(RawDialect, separator) == 0);
    assert!(offset_of!(RawDialect, quoting) == 1);
    assert!(offset_of!(RawDialect, skip_blank_lines) == 2);
    assert!(offset_of!(RawDialect, engine) == 3);
    assert!(
        align_of::<u64>() != 8
            || (size_of::<State>() == State::SIZE as usize && align_of::<State>() == 8)
    );
    assert!(offset_of!(State, separator) == 0);
    assert!(offset_of!(State, quoting) == 1);
    assert!(offset_of!(State, skip_blank_lines) == 2);
    assert!(offset_of!(State, engine) == 3);
    assert!(offset_of!(State, line) == 4);
    assert!(offset_of!(State, records) == 8);
    assert!(offset_of!(State, offset) == 16);
    assert!(offset_of!(State, expected) == 24);
    assert!(offset_of!(State, started) == 28);
    assert!(offset_of!(State, extended) == 29);
    assert!(offset_of!(State, reserved) == 30);
    assert!(offset_of!(State, failure) == 32);
    assert!(offset_of!(State, skip) == State::FIRST_SIZE);
    assert!(offset_of!(State, header_row) == 68);
    assert!(offset_of!(State, short_rows) == 72);
    assert!(offset_of!(State, long_rows) == 73);
    assert!(offset_of!(State, tail) == 74);
};

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// Offsets are 31 bits: a [`Span`]'s length keeps its top bit for a flag.
const LIMIT: usize = i32::MAX as usize;

impl State {
    /// A state at the start of an input, or `None` when the dialect is one the scanner
    /// cannot honour: a separator outside its set, or an engine this CPU cannot run.
    pub fn init(dialect: RawDialect) -> Option<State> {
        if !RawDialect::is_valid_separator(dialect.separator) {
            return None;
        }
        if dialect.engine != 0 && !engine::usable(Kind::from_code(dialect.engine)?) {
            return None;
        }
        Some(State {
            separator: dialect.separator,
            quoting: dialect.quoting,
            skip_blank_lines: dialect.skip_blank_lines,
            engine: dialect.engine,
            line: 1,
            ..State::ZERO
        })
    }

    /// A state at the start of an input with these options, or `None` for a size the core
    /// doesn't read, what [`State::init`] refuses, or a policy that isn't one.
    pub fn init_with(options: &RawDelimitedOptions) -> Option<State> {
        if options.size != RawDelimitedOptions::SIZE
            || options.short_rows > RawDelimitedOptions::SHORT_ROWS_PAD
            || options.long_rows > RawDelimitedOptions::LONG_ROWS_TRUNCATE
        {
            return None;
        }
        let mut state = State::init(RawDialect {
            separator: options.separator,
            quoting: options.quoting,
            skip_blank_lines: options.skip_blank_lines,
            engine: options.engine,
        })?;
        state.extended = 1;
        state.skip = options.skip;
        state.header_row = options.header_row;
        state.short_rows = options.short_rows as u8;
        state.long_rows = options.long_rows as u8;
        Some(state)
    }

    /// The caller's block as a whole state: its fields past the first 64 bytes default when it
    /// didn't come from [`State::init_with`], and aren't read.
    ///
    /// # Safety
    /// `block` points to a readable state, 8-byte aligned, of 64 bytes at least, and of
    /// [`State::SIZE`] when it came from [`State::init_with`].
    pub(crate) unsafe fn load(block: *const State) -> State {
        let mut state = State::ZERO;
        let into = (&raw mut state).cast::<u8>();
        // SAFETY: per the function contract, and every field is a plain integer.
        unsafe {
            core::ptr::copy_nonoverlapping(block.cast::<u8>(), into, State::FIRST_SIZE);
            if state.extended != 0 {
                core::ptr::copy_nonoverlapping(block.cast::<u8>(), into, size_of::<State>());
            }
        }
        state
    }

    /// Writes this state back to a block [`State::load`] could read, or to a new one: all of it
    /// when it came from [`State::init_with`], the first 64 bytes otherwise.
    ///
    /// # Safety
    /// `block` points to a writable state, 8-byte aligned, of as many bytes.
    pub(crate) unsafe fn store(&self, block: *mut State) {
        let bytes = if self.extended != 0 {
            size_of::<State>()
        } else {
            State::FIRST_SIZE
        };
        // SAFETY: per the function contract.
        unsafe {
            core::ptr::copy_nonoverlapping(
                core::ptr::from_ref(self).cast::<u8>(),
                block.cast::<u8>(),
                bytes,
            );
        }
    }

    /// The scanner this state asks for — checked again on every call, because the block
    /// is the caller's memory and an engine the CPU lacks must never be run on its say.
    fn scanner(&self) -> Option<Scanner> {
        if !RawDialect::is_valid_separator(self.separator) {
            return None;
        }
        let kind = match self.engine {
            0 => engine::best(),
            code => Kind::from_code(code).filter(|&kind| engine::usable(kind))?,
        };
        Some(Scanner::with_engine(
            self.separator,
            self.quoting != 0,
            kind,
        ))
    }

    /// The physical lines of `input` from `first`, up to what's left to pass over: where they
    /// end, the line there, and how many are still to come. Lines, so a scanner that doesn't
    /// know a quote from any other byte; committed only with the rest of the call.
    fn past_skipped(
        &self,
        scanner: &Scanner,
        input: &[u8],
        first: usize,
        last: bool,
    ) -> (usize, u32, u32) {
        if self.skip == 0 {
            return (first, self.line, 0);
        }
        let mut sink = SkipLines { left: self.skip };
        let stop = scanner
            .lines()
            .scan_into(input, first, self.line, last, &mut sink);
        (stop.next, stop.line, sink.left)
    }

    /// Where the scan starts in the first chunk: past a UTF-8 byte-order mark if there is
    /// one. `None` when the chunk is too short to tell and more is coming.
    fn first_byte(&self, input: &[u8], last: bool) -> Option<usize> {
        if self.started != 0 {
            return Some(0);
        }
        if input.starts_with(&BOM) {
            return Some(BOM.len());
        }
        if !last && input.len() < BOM.len() && BOM.starts_with(input) {
            return None;
        }
        Some(0)
    }

    fn advance(&mut self, consumed: usize, line: u32, records: u64) {
        self.started = 1;
        self.offset += consumed as u64;
        self.line = line;
        self.records = records;
    }

    fn fail(&mut self, failure: Failure, out: &mut Filled) -> i32 {
        self.failure = failure;
        out.failure = failure;
        ERR_STRUCTURE
    }
}

/// Where a cell's text is, given what the walker saw of it: as written; without its outer
/// quotes when those are its only two; or flagged, when it has to be unescaped.
#[inline(always)]
fn locate(data: &[u8], start: usize, end: usize, quotes: u32) -> Span {
    let raw = data.get(start..end).unwrap_or_default();
    let offset = start as u32;
    let len = raw.len() as u32;
    if quotes == 0 || raw.first() != Some(&b'"') {
        Span { offset, len }
    } else if quotes == 2 && raw.len() >= 2 && raw.last() == Some(&b'"') {
        Span {
            offset: offset + 1,
            len: len - 2,
        }
    } else {
        Span {
            offset,
            len: len | Span::FLAG,
        }
    }
}

/// The sink of the lines a read passes over: it counts them.
struct SkipLines {
    left: u32,
}

impl Sink for SkipLines {
    #[inline(always)]
    fn cell(&mut self, _start: usize, _end: usize, _quotes: u32) {}

    fn discard(&mut self) {}

    #[inline(always)]
    fn row(&mut self, _start: usize, _next: usize, _line: u32, _terminator: u8) -> Flow {
        self.left = self.left.saturating_sub(1);
        if self.left == 0 {
            Flow::Full
        } else {
            Flow::More
        }
    }
}

/// What the scan needs of a plan's column.
trait Planned {
    fn ordinal(&self) -> u32;
}

impl Planned for ColumnSpec {
    #[inline(always)]
    fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

impl Planned for ColumnSpecWith {
    #[inline(always)]
    fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

/// The sink of a fill: writes each row's cells into the caller's table.
///
/// `EXTRAS` says whether a row can meet what 1.0's sink never did: a header row still to pass
/// over, a widths array to write in, or a policy that pads a short row or cuts a long one. A fill
/// with none of them takes the sink compiled without them ([`walk`] picks), and the two agree on
/// every row such a fill can see. Both are in every build; a scan is one or the other throughout.
struct Rows<'a, 'c, P, const EXTRAS: bool> {
    data: &'a [u8],
    specs: &'a [P],
    cells: &'c mut [Span],
    width: usize,
    per_row: usize,
    capacity: usize,
    rows: usize,
    /// Cells seen so far in the row in progress.
    seen: usize,
    /// Whether that row's first cell is empty and unquoted.
    first_empty: bool,
    /// Whether any of that row's recorded cells needs unescaping.
    escaped: bool,
    skip_blank: bool,
    expected: u32,
    records: u64,
    arena_free: usize,
    /// What the first row needs of the arena, when it is more than there is.
    arena_needed: usize,
    failure: Failure,
    /// Absolute offset of `data[0]`.
    offset: u64,
    /// Records still to discard.
    header_row: u32,
    /// Whether a short record is padded.
    pad: bool,
    /// Whether a long record is cut.
    cut: bool,
    /// The cells each delivered row had, when the caller asked; empty otherwise.
    widths: &'a mut [u32],
}

impl<P: Planned, const EXTRAS: bool> Sink for Rows<'_, '_, P, EXTRAS> {
    #[inline(always)]
    fn cell(&mut self, start: usize, end: usize, quotes: u32) {
        if self.seen == 0 {
            self.first_empty = start == end;
        }
        if self.seen < self.width {
            let span = locate(self.data, start, end, quotes);
            self.escaped |= span.flagged();
            if let Some(slot) = self.cells.get_mut(self.rows * self.per_row + self.seen) {
                *slot = span;
            }
        }
        self.seen += 1;
    }

    fn discard(&mut self) {
        self.seen = 0;
        self.escaped = false;
    }

    #[inline(always)]
    fn row(&mut self, start: usize, _next: usize, line: u32, terminator: u8) -> Flow {
        let found = self.seen;
        let escaped = self.escaped;
        self.seen = 0;
        self.escaped = false;
        if found == 1 && self.first_empty && self.skip_blank && terminator != 0 {
            self.records += 1;
            return Flow::More;
        }
        if EXTRAS && self.header_row > 0 {
            self.header_row -= 1;
            self.records += 1;
            return Flow::More;
        }
        let expected = if self.expected == 0 {
            found as u32
        } else {
            self.expected
        };
        let width = found as u32;
        if width != expected && !(EXTRAS && if width < expected { self.pad } else { self.cut }) {
            // Every intact row before it is delivered first; the failure is raised by the
            // call that finds it at the front.
            if self.rows == 0 {
                self.failure = Failure {
                    code: Failure::COLUMN_COUNT,
                    line,
                    record: self.records,
                    byte: self.offset + start as u64,
                    expected,
                    found: found as u32,
                };
            }
            return Flow::Refused;
        }
        let base = self.rows * self.per_row;
        // Source columns the plan reads past this record's last cell are empty, as are those
        // past the width in a row that's cut.
        let from = if EXTRAS {
            found.min(expected as usize)
        } else {
            found
        };
        for ordinal in from..self.width {
            if let Some(slot) = self.cells.get_mut(base + ordinal) {
                *slot = Span::default();
            }
        }
        if escaped {
            let mut need = 0usize;
            for spec in self.specs {
                if let Some(cell) = self.cells.get(base + spec.ordinal() as usize)
                    && cell.flagged()
                {
                    need += cell.len();
                }
            }
            if need > self.arena_free {
                if self.rows == 0 {
                    self.arena_needed = need;
                }
                return Flow::Refused;
            }
            self.arena_free -= need;
        }
        if let Some(slot) = self.cells.get_mut(base + self.width) {
            *slot = Span {
                offset: start as u32,
                len: line,
            };
        }
        if EXTRAS && let Some(slot) = self.widths.get_mut(self.rows) {
            *slot = width;
        }
        self.expected = expected;
        self.records += 1;
        self.rows += 1;
        if self.rows == self.capacity {
            Flow::Full
        } else {
            Flow::More
        }
    }
}

/// A cell's text, and the span a `Text` value carries for it: the input as written, or —
/// for a flagged cell — its unescaped bytes, written to the arena at `used`.
#[inline(always)]
fn text<'t>(data: &'t [u8], arena: &'t mut [u8], used: &mut usize, cell: Span) -> (&'t [u8], Span) {
    let start = cell.offset as usize;
    let raw = data.get(start..start + cell.len()).unwrap_or_default();
    if !cell.flagged() {
        return (raw, cell);
    }
    let room = arena.get_mut(*used..).unwrap_or_default();
    let written = unescape_into(raw, room);
    let span = Span {
        offset: *used as u32,
        len: written as u32 | Span::FLAG,
    };
    *used += written;
    let room: &'t [u8] = room;
    (room.get(..written).unwrap_or_default(), span)
}

/// What the column loops share.
struct Run<'a> {
    data: &'a [u8],
    cells: &'a [Span],
    per_row: usize,
    rows: usize,
    arena: &'a mut [u8],
    used: usize,
}

/// One column: `door` over every row's cell at `ordinal`, values and verdicts written
/// where the caller pointed. A cell that does not cast leaves `zero` as its value.
///
/// # Safety
/// `buffer`'s two arrays each have room for `run.rows` elements.
#[inline(always)]
unsafe fn column<T: Copy>(
    run: &mut Run<'_>,
    ordinal: usize,
    buffer: &ColumnBuffer,
    zero: T,
    door: impl Fn(&[u8], Span) -> Result<T, Fault>,
) {
    let values = buffer.values.cast::<T>();
    for row in 0..run.rows {
        let cell = run
            .cells
            .get(row * run.per_row + ordinal)
            .copied()
            .unwrap_or_default();
        let (text, span) = text(run.data, &mut *run.arena, &mut run.used, cell);
        let (value, verdict) = match door(text, span) {
            Ok(value) => (value, CellVerdict::OK),
            Err(fault) => (zero, CellVerdict::from_fault(fault)),
        };
        // SAFETY: per the function contract; neither array need be aligned.
        unsafe {
            values.add(row).write_unaligned(value);
            buffer.verdicts.add(row).write_unaligned(verdict);
        }
    }
}

/// One column as `hypertabular_delimited_fill` has always cast it: HyperCast's doors, a fault
/// and nothing more. `valid` says a text cell with bytes in it is taken as valid UTF-8 here:
/// [`vouch`] answered for its row, or [`rejudge`] asks the door for it afterwards.
///
/// # Safety
/// `buffer`'s two arrays each have room for `run.rows` elements of the door's types.
#[inline(always)]
unsafe fn cast_plain(
    run: &mut Run<'_>,
    door: Door,
    ordinal: usize,
    format: &NumFormat,
    valid: bool,
    buffer: &ColumnBuffer,
) {
    const INSTANT: Timestamp = Timestamp {
        seconds: 0,
        nanos: 0,
    };
    const DAY: Date = Date {
        year: 0,
        month: 0,
        day: 0,
    };
    // SAFETY: the caller's contract, passed on a column at a time.
    unsafe {
        match door {
            Door::Bool => column(run, ordinal, buffer, 0u8, |text, _| {
                hypercast::cast_bool(text).map(u8::from)
            }),
            Door::I8 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_i8(text, format)
            }),
            Door::I16 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_i16(text, format)
            }),
            Door::I32 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_i32(text, format)
            }),
            Door::I64 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_i64(text, format)
            }),
            Door::U8 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_u8(text, format)
            }),
            Door::U16 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_u16(text, format)
            }),
            Door::U32 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_u32(text, format)
            }),
            Door::U64 => column(run, ordinal, buffer, 0, |text, _| {
                hypercast::cast_u64(text, format)
            }),
            Door::F32 => column(run, ordinal, buffer, 0.0, |text, _| {
                hypercast::cast_f32(text, format)
            }),
            Door::F64 => column(run, ordinal, buffer, 0.0, |text, _| {
                hypercast::cast_f64(text, format)
            }),
            Door::Decimal => {
                let zero = Decimal {
                    lo: 0,
                    hi: 0,
                    scale: 0,
                    negative: false,
                };
                column(run, ordinal, buffer, zero, |text, _| {
                    hypercast::cast_decimal(text, format)
                })
            }
            Door::Uuid => column(run, ordinal, buffer, [0u8; 16], |text, _| {
                hypercast::cast_uuid(text)
            }),
            Door::Timestamp => column(run, ordinal, buffer, INSTANT, |text, _| {
                hypercast::cast_timestamp(text)
            }),
            Door::Unix(precision) => column(run, ordinal, buffer, INSTANT, |text, _| {
                hypercast::cast_unix(text, precision)
            }),
            Door::ExcelSerial(epoch) => column(run, ordinal, buffer, INSTANT, |text, _| {
                hypercast::cast_excel_serial(text, epoch)
            }),
            Door::Date => column(run, ordinal, buffer, DAY, |text, _| {
                hypercast::cast_date(text)
            }),
            Door::DateOrdered(order) => column(run, ordinal, buffer, DAY, |text, _| {
                hypercast::cast_date_ordered(text, order)
            }),
            Door::DateTime(order) => {
                let zero = CivilDateTime {
                    date: DAY,
                    nanos_of_day: 0,
                };
                column(run, ordinal, buffer, zero, |text, _| {
                    hypercast::cast_datetime(text, order)
                })
            }
            Door::Time => column(run, ordinal, buffer, 0u64, |text, _| {
                hypercast::cast_time(text)
            }),
            Door::Duration => {
                let zero = Duration {
                    seconds: 0,
                    nanos: 0,
                };
                column(run, ordinal, buffer, zero, |text, _| {
                    hypercast::cast_duration(text)
                })
            }
            // The bytes themselves, untrimmed, as HyperCast's door judges them: no bytes at
            // all is `Empty`, and bytes that aren't UTF-8 are `Malformed`. The region check
            // leaves only the first of those to ask here.
            Door::Text => column(run, ordinal, buffer, Span::default(), |text, span| {
                if valid && !text.is_empty() {
                    Ok(span)
                } else {
                    hypercast::cast_text(text).map(|_| span)
                }
            }),
        }
    }
}

/// The verdict no door gave: what a padded cell or a token is.
const EMPTY: CellVerdict = CellVerdict {
    offset: 0,
    len: 0,
    reason: Reason::Empty as u32,
};

/// Whether `list` is whole: a little-endian `u32` length and that many bytes, to its end.
pub(crate) fn tokens_are_whole(mut list: &[u8]) -> bool {
    while !list.is_empty() {
        match next_token(list) {
            Some((_, rest)) => list = rest,
            None => return false,
        }
    }
    true
}

fn next_token(list: &[u8]) -> Option<(&[u8], &[u8])> {
    let (head, rest) = list.split_first_chunk::<4>()?;
    rest.split_at_checked(u32::from_le_bytes(*head) as usize)
}

/// Whether `text` is, byte for byte, one of a whole list's tokens.
#[inline]
pub(crate) fn is_token(mut list: &[u8], text: &[u8]) -> bool {
    while let Some((token, rest)) = next_token(list) {
        if token == text {
            return true;
        }
        list = rest;
    }
    false
}

/// The cell at `ordinal` of `row`, as a door is asked of it: its text and the span a `Text`
/// value carries, or the origin of the `Empty` it is without asking one, a cell the row doesn't
/// have or a cell that's one of `tokens`.
#[inline(always)]
fn routed<'t>(
    run: &'t mut Run<'_>,
    row: usize,
    ordinal: usize,
    tokens: &[u8],
) -> Result<(&'t [u8], Span), u16> {
    let cell = run
        .cells
        .get(row * run.per_row + ordinal)
        .copied()
        .unwrap_or_default();
    // A cell the row has, past its first, starts after the separator before it, so only a
    // blanked table entry says offset 0 there.
    if ordinal > 0 && cell == Span::default() {
        return Err(CellNote::ORIGIN_PADDED);
    }
    // The byte before an unquoted cell is a separator or a line ending, or there is none.
    let quoted = !tokens.is_empty()
        && (cell.flagged()
            || (cell.offset as usize)
                .checked_sub(1)
                .and_then(|before| run.data.get(before))
                == Some(&b'"'));
    let (text, span) = text(run.data, &mut *run.arena, &mut run.used, cell);
    if !tokens.is_empty() && !quoted && is_token(tokens, text) {
        return Err(CellNote::ORIGIN_TOKEN);
    }
    Ok((text, span))
}

/// One column with what a verdict doesn't say: `door` over every row's cell, but a cell the row
/// doesn't have and a cell that's a token are `Empty` without asking it. A note goes beside each
/// verdict when `buffer` has somewhere for it.
///
/// # Safety
/// `buffer`'s arrays each have room for `run.rows` elements, its notes unless they're null.
#[inline(always)]
unsafe fn noted_column<T: Copy>(
    run: &mut Run<'_>,
    ordinal: usize,
    buffer: &ColumnBufferWith,
    tokens: &[u8],
    zero: T,
    door: impl Fn(&[u8], Span) -> Result<T, hypercast::Failure>,
) {
    let values = buffer.values.cast::<T>();
    let notes = buffer.notes;
    // Most cells have nothing to note: every note starts as the default, a zero, and only the rest
    // are written. Asked of each cell, this is most of what notes cost.
    if !notes.is_null() {
        // SAFETY: per the function contract; as bytes, since the array need not be aligned.
        unsafe {
            notes
                .cast::<u8>()
                .write_bytes(0, run.rows * size_of::<CellNote>());
        }
    }
    let note_at = |row: usize, detail: u16, origin: u16| {
        if !notes.is_null() {
            // SAFETY: per the function contract; the array need not be aligned.
            unsafe { notes.add(row).write_unaligned(CellNote { detail, origin }) };
        }
    };
    for row in 0..run.rows {
        let (value, verdict) = match routed(run, row, ordinal, tokens) {
            Err(origin) => {
                note_at(row, 0, origin);
                (zero, EMPTY)
            }
            Ok((text, span)) => match door(text, span) {
                Ok(value) => (value, CellVerdict::OK),
                Err(failure) => {
                    note_at(row, failure.detail.code() as u16, CellNote::ORIGIN_CELL);
                    (zero, CellVerdict::from_fault(failure.fault()))
                }
            },
        };
        // SAFETY: per the function contract; neither array need be aligned.
        unsafe {
            values.add(row).write_unaligned(value);
            buffer.verdicts.add(row).write_unaligned(verdict);
        }
    }
}

/// [`cast_plain`] through HyperCast's detailed doors, with options, tokens and notes.
///
/// # Safety
/// `buffer`'s arrays each have room for `run.rows` elements of the door's types, its notes
/// unless they're null.
#[inline(always)]
unsafe fn cast_noted(
    run: &mut Run<'_>,
    door: Door,
    ordinal: usize,
    column: &Resolved<'_>,
    valid: bool,
    buffer: &ColumnBufferWith,
) {
    const INSTANT: Timestamp = Timestamp {
        seconds: 0,
        nanos: 0,
    };
    const DAY: Date = Date {
        year: 0,
        month: 0,
        day: 0,
    };
    let format = &column.format;
    let date = column.date.as_ref();
    let tokens = column.tokens;
    // SAFETY: the caller's contract, passed on.
    unsafe {
        match door {
            Door::Bool => noted_column(run, ordinal, buffer, tokens, 0u8, |text, _| {
                detailed::cast_bool(text).map(u8::from)
            }),
            Door::I8 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_i8(text, format)
            }),
            Door::I16 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_i16(text, format)
            }),
            Door::I32 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_i32(text, format)
            }),
            Door::I64 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_i64(text, format)
            }),
            Door::U8 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_u8(text, format)
            }),
            Door::U16 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_u16(text, format)
            }),
            Door::U32 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_u32(text, format)
            }),
            Door::U64 => noted_column(run, ordinal, buffer, tokens, 0, |text, _| {
                detailed::cast_u64(text, format)
            }),
            Door::F32 => noted_column(run, ordinal, buffer, tokens, 0.0, |text, _| {
                detailed::cast_f32(text, format)
            }),
            Door::F64 => noted_column(run, ordinal, buffer, tokens, 0.0, |text, _| {
                detailed::cast_f64(text, format)
            }),
            Door::Decimal => {
                let zero = Decimal {
                    lo: 0,
                    hi: 0,
                    scale: 0,
                    negative: false,
                };
                noted_column(run, ordinal, buffer, tokens, zero, |text, _| {
                    detailed::cast_decimal(text, format)
                })
            }
            Door::Uuid => noted_column(run, ordinal, buffer, tokens, [0u8; 16], |text, _| {
                detailed::cast_uuid(text)
            }),
            Door::Timestamp => noted_column(run, ordinal, buffer, tokens, INSTANT, |text, _| {
                detailed::cast_timestamp(text, date)
            }),
            Door::Unix(precision) => {
                noted_column(run, ordinal, buffer, tokens, INSTANT, |text, _| {
                    detailed::cast_unix(text, precision)
                })
            }
            Door::ExcelSerial(epoch) => {
                noted_column(run, ordinal, buffer, tokens, INSTANT, |text, _| {
                    detailed::cast_excel_serial(text, epoch)
                })
            }
            Door::Date => noted_column(run, ordinal, buffer, tokens, DAY, |text, _| {
                detailed::cast_date(text, date)
            }),
            Door::DateOrdered(order) => {
                noted_column(run, ordinal, buffer, tokens, DAY, |text, _| {
                    detailed::cast_date_ordered(text, order, date)
                })
            }
            Door::DateTime(order) => {
                let zero = CivilDateTime {
                    date: DAY,
                    nanos_of_day: 0,
                };
                noted_column(run, ordinal, buffer, tokens, zero, |text, _| {
                    detailed::cast_datetime(text, order, date)
                })
            }
            Door::Time => noted_column(run, ordinal, buffer, tokens, 0u64, |text, _| {
                detailed::cast_time(text)
            }),
            Door::Duration => {
                let zero = Duration {
                    seconds: 0,
                    nanos: 0,
                };
                noted_column(run, ordinal, buffer, tokens, zero, |text, _| {
                    detailed::cast_duration(text)
                })
            }
            // As `cast_plain`'s text door, tokens having had their turn on the bytes.
            Door::Text => noted_column(
                run,
                ordinal,
                buffer,
                tokens,
                Span::default(),
                |text, span| {
                    if valid && !text.is_empty() {
                        Ok(span)
                    } else {
                        detailed::cast_text(text).map(|_| span)
                    }
                },
            ),
        }
    }
}

/// One column under its constraints: as [`noted_column`], each cell asked of a checked door,
/// whose verdict the column's `checks` also takes. A value that passed is written with
/// [`CellVerdict::OK`] and `adjusted` and `enum_index` beside it; one that broke a constraint is
/// written exactly as cast, with [`CellVerdict::VIOLATION`] spanning it and the keyword beside it.
/// A cell that did not cast, and one that's `Empty` without asking, leave its `checks` as they
/// were: there is nothing to judge.
///
/// # Safety
/// `buffer`'s arrays each have room for `run.rows` elements, its notes unless they're null, and
/// its `checks` is aligned and not null.
#[inline(always)]
unsafe fn checked_column<T: Copy>(
    run: &mut Run<'_>,
    ordinal: usize,
    buffer: &ColumnBufferWith,
    tokens: &[u8],
    zero: T,
    door: impl Fn(&[u8], Span) -> Result<Checked<T>, Rejection<T>>,
) {
    let values = buffer.values.cast::<T>();
    let notes = buffer.notes;
    if !notes.is_null() {
        // SAFETY: per the function contract; as bytes, since the array need not be aligned.
        unsafe {
            notes
                .cast::<u8>()
                .write_bytes(0, run.rows * size_of::<CellNote>());
        }
    }
    let note_at = |row: usize, detail: u16, origin: u16| {
        if !notes.is_null() {
            // SAFETY: per the function contract; the array need not be aligned.
            unsafe { notes.add(row).write_unaligned(CellNote { detail, origin }) };
        }
    };
    let check_at = |row: usize, check: RawCheck| {
        // SAFETY: per the function contract.
        unsafe { buffer.checks.add(row).write(check) };
    };
    for row in 0..run.rows {
        let (value, verdict) = match routed(run, row, ordinal, tokens) {
            Err(origin) => {
                note_at(row, 0, origin);
                (zero, EMPTY)
            }
            Ok((text, span)) => match door(text, span) {
                Ok(passed) => {
                    check_at(
                        row,
                        RawCheck {
                            keyword: 0,
                            adjusted: u32::from(passed.adjusted),
                            enum_index: passed.enum_index.unwrap_or(u32::MAX),
                        },
                    );
                    (passed.value, CellVerdict::OK)
                }
                // A violation's note is the default, a detail of 0 at the cell, as written.
                Err(Rejection::Violation(broken)) => {
                    check_at(
                        row,
                        RawCheck {
                            keyword: broken.keyword.code(),
                            adjusted: 0,
                            enum_index: u32::MAX,
                        },
                    );
                    let verdict = CellVerdict {
                        offset: broken.offset,
                        len: broken.len,
                        reason: CellVerdict::VIOLATION,
                    };
                    (broken.value, verdict)
                }
                Err(Rejection::Failure(failure)) => {
                    note_at(row, failure.detail.code() as u16, CellNote::ORIGIN_CELL);
                    (zero, CellVerdict::from_fault(failure.fault()))
                }
            },
        };
        // SAFETY: per the function contract; neither array need be aligned.
        unsafe {
            values.add(row).write_unaligned(value);
            buffer.verdicts.add(row).write_unaligned(verdict);
        }
    }
}

/// [`cast_noted`] for a column that declares constraints: HyperCast's checked doors, numeric and
/// text, with tokens, notes and `checks`.
///
/// # Safety
/// As [`checked_column`], for the door's types.
#[inline(always)]
unsafe fn cast_checked_column(
    run: &mut Run<'_>,
    ordinal: usize,
    column: &Resolved<'_>,
    checks: Checks<'_>,
    buffer: &ColumnBufferWith,
) {
    let format = &column.format;
    let tokens = column.tokens;
    macro_rules! numeric {
        ($constraints:expr, $zero:expr) => {
            checked_column(run, ordinal, buffer, tokens, $zero, |text, _| {
                detailed::cast_checked(text, format, &$constraints)
            })
        };
    }
    // SAFETY: the caller's contract, passed on.
    unsafe {
        match checks {
            Checks::I8(c) => numeric!(c, 0),
            Checks::I16(c) => numeric!(c, 0),
            Checks::I32(c) => numeric!(c, 0),
            Checks::I64(c) => numeric!(c, 0),
            Checks::U8(c) => numeric!(c, 0),
            Checks::U16(c) => numeric!(c, 0),
            Checks::U32(c) => numeric!(c, 0),
            Checks::U64(c) => numeric!(c, 0),
            Checks::F32(c) => numeric!(c, 0.0),
            Checks::F64(c) => numeric!(c, 0.0),
            Checks::Decimal(c) => numeric!(
                c,
                Decimal {
                    lo: 0,
                    hi: 0,
                    scale: 0,
                    negative: false,
                }
            ),
            Checks::Text(c) => checked_column(
                run,
                ordinal,
                buffer,
                tokens,
                Span::default(),
                |text, span| text_checked(text, span, &c),
            ),
        }
    }
}

/// What the scan of a call found, for its columns to be cast from and the state moved to.
struct Walked<'c> {
    cells: &'c [Span],
    per_row: usize,
    rows: usize,
    expected: u32,
    records: u64,
    header_row: u32,
    next: usize,
    line: u32,
}

/// What [`gather`] scans, and the room it has for the rows.
struct Begin<'a, 'c, P> {
    scanner: &'a Scanner,
    input: &'a [u8],
    first: usize,
    line: u32,
    last: bool,
    specs: &'a [P],
    cells: &'c mut [Span],
    width: usize,
    per_row: usize,
    capacity: usize,
    arena_cap: usize,
    widths: &'a mut [u32],
}

/// What a scan left in its sink, and where it stopped.
struct Gathered<'c> {
    stop: Stop,
    cells: &'c mut [Span],
    rows: usize,
    expected: u32,
    records: u64,
    arena_needed: usize,
    failure: Failure,
    header_row: u32,
}

/// One scan's rows into the cell table, through the sink `EXTRAS` names: see [`Rows`].
#[inline(always)]
fn gather<'c, P: Planned, const EXTRAS: bool>(
    state: &State,
    begin: Begin<'_, 'c, P>,
) -> Gathered<'c> {
    let mut sink: Rows<'_, 'c, P, EXTRAS> = Rows {
        data: begin.input,
        specs: begin.specs,
        cells: begin.cells,
        width: begin.width,
        per_row: begin.per_row,
        capacity: begin.capacity,
        rows: 0,
        seen: 0,
        first_empty: false,
        escaped: false,
        skip_blank: state.skip_blank_lines != 0,
        expected: state.expected,
        records: state.records,
        arena_free: begin.arena_cap,
        arena_needed: 0,
        failure: state.failure,
        offset: state.offset,
        header_row: state.header_row,
        pad: state.short_rows != 0,
        cut: state.long_rows != 0,
        widths: begin.widths,
    };
    let stop = begin
        .scanner
        .scan_into(begin.input, begin.first, begin.line, begin.last, &mut sink);
    Gathered {
        stop,
        cells: sink.cells,
        rows: sink.rows,
        expected: sink.expected,
        records: sink.records,
        arena_needed: sink.arena_needed,
        failure: sink.failure,
        header_row: sink.header_row,
    }
}

/// The part of a fill the plan's types don't enter: past what's skipped, then rows into the cell
/// table. `Err` holds what the call returns, its work done: no rows came of it.
#[allow(clippy::too_many_arguments)]
fn walk<'c, P: Planned>(
    state: &mut State,
    scanner: &Scanner,
    input: &[u8],
    last: bool,
    specs: &[P],
    width: usize,
    max_rows: usize,
    cells: &'c mut [Span],
    arena_cap: usize,
    widths: &mut [u32],
    out: &mut Filled,
) -> Result<Walked<'c>, i32> {
    if max_rows == 0 {
        return Err(OK);
    }
    let Some(per_row) = width.checked_add(1) else {
        return Err(ERR_CONTRACT);
    };
    let capacity = max_rows.min(cells.len() / per_row);
    if capacity == 0 {
        out.needed = per_row as u64;
        return Err(ERR_CELLS);
    }
    let Some(first) = state.first_byte(input, last) else {
        return Err(OK);
    };
    let (first, line, skip) = state.past_skipped(scanner, input, first, last);
    if skip != 0 {
        state.skip = skip;
        state.advance(first, line, state.records);
        out.consumed = first as u64;
        return Err(OK);
    }

    // A row asks more of the sink than 1.0's did only when there are header rows to pass over, widths
    // to report or a policy for a row of the wrong width. Without any of them the scan runs through
    // the sink that leaves those out, which is most of what they cost a plain read on a SIMD
    // engine. The portable engine keeps the full sink: it measured 3 to 4% slower through the lean
    // one (Apple M6, 2026-11-09, `docs/delimited.md`).
    let extras = scanner.is_portable()
        || state.header_row != 0
        || !widths.is_empty()
        || state.short_rows != 0
        || state.long_rows != 0;
    let begin = Begin {
        scanner,
        input,
        first,
        line,
        last,
        specs,
        cells,
        width,
        per_row,
        capacity,
        arena_cap,
        widths,
    };
    let Gathered {
        stop,
        cells,
        rows,
        expected,
        records,
        arena_needed,
        failure,
        header_row,
    } = if extras {
        gather::<P, true>(state, begin)
    } else {
        gather::<P, false>(state, begin)
    };

    if rows == 0 {
        if arena_needed != 0 {
            out.needed = arena_needed as u64;
            return Err(ERR_ARENA);
        }
        state.skip = 0;
        state.header_row = header_row;
        state.advance(stop.next, stop.line, records);
        out.consumed = stop.next as u64;
        if failure.code != 0 {
            return Err(state.fail(failure, out));
        }
        if stop.unclosed {
            let failure = Failure {
                code: Failure::UNCLOSED_QUOTE,
                line: stop.line,
                record: records,
                byte: state.offset,
                expected: 0,
                found: 0,
            };
            return Err(state.fail(failure, out));
        }
        return Err(OK);
    }
    Ok(Walked {
        cells,
        per_row,
        rows,
        expected,
        records,
        header_row,
        next: stop.next,
        line: stop.line,
    })
}

/// How many rows a refused region can set aside before the rest are asked cell by cell.
const SUSPECTS: usize = 16;

/// What the region check found: every row below `upto` is valid UTF-8 but the first `count` of
/// `suspects`, which each hold a byte the door refused.
struct Vouched {
    suspects: [usize; SUSPECTS],
    count: usize,
    upto: usize,
}

impl Vouched {
    /// Whether any row is left for [`rejudge`].
    fn all(&self, rows: usize) -> bool {
        self.count == 0 && self.upto >= rows
    }
}

/// Which of the rows found are valid UTF-8, asked of the whole region they came from: from the
/// first row's start, so what was passed over or discarded is no part of it, to where the scan
/// stopped. If the region is valid, so is every cell of it: a cell is cut out at ASCII bytes,
/// which no multi-byte character holds, and unescaping only takes ASCII quotes out. Asked once,
/// this is most of what validating costs.
///
/// A region the door refuses says where its first bad byte is. The row holding it is set aside, a
/// suspect, and the rest is asked again from the row after: of `core`, which is what the door
/// itself asks once its fast check has refused, so a fill reads its bytes once fast and at most
/// once more however many rows are bad. A suspect is only that: the byte may be in a cell the plan
/// doesn't read, in one a cut row dropped, or in a line passed over after the row. Past
/// [`SUSPECTS`] of them the rows left are all for [`rejudge`].
fn vouch(input: &[u8], walked: &Walked<'_>) -> Vouched {
    let rows = walked.rows;
    let start = |row: usize| {
        walked
            .cells
            .get(row * walked.per_row + walked.per_row - 1)
            .map_or(0, |start| start.offset as usize)
    };
    let mut vouched = Vouched {
        suspects: [0; SUSPECTS],
        count: 0,
        upto: rows,
    };
    // Rows below `settled` are answered for; the region left starts at `from`.
    let mut settled = 0usize;
    let mut from = start(0);
    loop {
        let region = input.get(from..walked.next).unwrap_or_default();
        let bad = if settled == 0 {
            match hypercast::cast_text(region) {
                Ok(_) => return vouched,
                Err(fault) if fault.reason == Reason::Malformed => fault.offset as usize,
                // No bytes at all: every cell is empty, which the door says of each.
                Err(_) => return vouched,
            }
        } else {
            match core::str::from_utf8(region) {
                Ok(_) => return vouched,
                Err(error) => error.valid_up_to(),
            }
        };
        // The last row that starts at or before the bad byte: rows are in input order.
        let at = from.saturating_add(bad);
        let (mut low, mut high) = (settled, rows);
        while high.saturating_sub(low) > 1 {
            let middle = low + (high - low) / 2;
            if start(middle) <= at {
                low = middle;
            } else {
                high = middle;
            }
        }
        if let Some(slot) = vouched.suspects.get_mut(vouched.count) {
            *slot = low;
            vouched.count += 1;
        }
        settled = low + 1;
        if settled >= rows {
            return vouched;
        }
        if vouched.count == SUSPECTS {
            vouched.upto = settled;
            return vouched;
        }
        from = start(settled);
    }
}

/// A text column's cells in the rows [`vouch`] did not answer for, each asked of the door for
/// itself: its verdict, and its note when there is somewhere for it, are then what the door says
/// of that cell alone. The column was cast as if every row were valid, so a cell with a value has
/// its text where the value says, and a cell without one (empty, padded or a token) was never
/// taken as valid.
///
/// # Safety
/// `values` has room for `rows` spans and `verdicts` for as many verdicts, `notes` for as many
/// notes unless it's null, and the column has been cast through the text door.
#[cold]
unsafe fn rejudge(
    vouched: &Vouched,
    rows: usize,
    data: &[u8],
    arena: &[u8],
    values: *mut Span,
    verdicts: *mut CellVerdict,
    notes: *mut CellNote,
) {
    let cell = |row: usize| {
        // SAFETY: per the function contract; no array need be aligned.
        unsafe {
            if verdicts.add(row).read_unaligned() != CellVerdict::OK {
                return;
            }
            let span = values.add(row).read_unaligned();
            let store = if span.flagged() { arena } else { data };
            let from = span.offset as usize;
            let text = store
                .get(from..from.saturating_add(span.len()))
                .unwrap_or_default();
            if let Err(failure) = detailed::cast_text(text) {
                values.add(row).write_unaligned(Span::default());
                verdicts
                    .add(row)
                    .write_unaligned(CellVerdict::from_fault(failure.fault()));
                if !notes.is_null() {
                    notes.add(row).write_unaligned(CellNote {
                        detail: failure.detail.code() as u16,
                        origin: CellNote::ORIGIN_CELL,
                    });
                }
            }
        }
    };
    for &row in vouched.suspects.get(..vouched.count).unwrap_or_default() {
        if row < rows {
            cell(row);
        }
    }
    for row in vouched.upto..rows {
        cell(row);
    }
}

impl State {
    /// A fill's rows delivered: the state moves past them, and `out` says so.
    fn deliver(&mut self, walked: &Walked<'_>, arena_used: usize, out: &mut Filled) {
        self.expected = walked.expected;
        self.skip = 0;
        self.header_row = walked.header_row;
        self.advance(walked.next, walked.line, walked.records);
        out.rows = walked.rows as u64;
        out.consumed = walked.next as u64;
        out.arena_used = arena_used as u64;
    }
}

/// Reads up to `max_rows` whole rows of `input` through `specs` into `columns`.
///
/// `input` starts where the last call's `consumed` ended (or at the start of the data);
/// `last` says no more follows it, so a final row without a terminator is a row. `out`
/// says how many rows were written and how many bytes are finished with. With rows short
/// of `max_rows` and input left over, the leftover is an unfinished row: it wants more
/// input behind it, unless `last` — then the input is done.
///
/// Returns [`OK`], or [`ERR_CONTRACT`] for a caller bug, [`ERR_STRUCTURE`] for broken data
/// (once every intact row before it has been delivered), [`ERR_ARENA`] or [`ERR_CELLS`]
/// when one row does not fit the arena or the cell table — see each for what `out` holds.
///
/// # Safety
/// For every column, `values` has room for `max_rows` values of its door's type and
/// `verdicts` for `max_rows` verdicts.
#[allow(clippy::too_many_arguments)]
pub unsafe fn fill(
    state: &mut State,
    input: &[u8],
    last: bool,
    specs: &[ColumnSpec],
    columns: &[ColumnBuffer],
    max_rows: usize,
    cells: &mut [Span],
    arena: &mut [u8],
    out: &mut Filled,
) -> i32 {
    // SAFETY: the caller's contract, passed on.
    unsafe {
        fill_read(
            state,
            input,
            last,
            specs,
            columns,
            max_rows,
            cells,
            arena,
            out,
            ColumnSpec::num_format,
        )
    }
}

/// [`fill`] as `hypertabular_delimited_fill` promises it: a format's flags as 1.0 read them
/// ([`ColumnSpec::num_format_1_0`]), so a stale policy bit is ignored and the column reads Western.
/// The extensions and the Rust API, whose callers can only have meant the flags they set
/// in this version's terms, call [`fill`].
///
/// # Safety
/// As [`fill`].
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn fill_1_0(
    state: &mut State,
    input: &[u8],
    last: bool,
    specs: &[ColumnSpec],
    columns: &[ColumnBuffer],
    max_rows: usize,
    cells: &mut [Span],
    arena: &mut [u8],
    out: &mut Filled,
) -> i32 {
    // SAFETY: the caller's contract, passed on.
    unsafe {
        fill_read(
            state,
            input,
            last,
            specs,
            columns,
            max_rows,
            cells,
            arena,
            out,
            ColumnSpec::num_format_1_0,
        )
    }
}

/// [`fill`], with what a column's format is taken to be.
///
/// # Safety
/// As [`fill`].
#[allow(clippy::too_many_arguments)]
#[inline(always)]
unsafe fn fill_read(
    state: &mut State,
    input: &[u8],
    last: bool,
    specs: &[ColumnSpec],
    columns: &[ColumnBuffer],
    max_rows: usize,
    cells: &mut [Span],
    arena: &mut [u8],
    out: &mut Filled,
    format_of: impl Fn(&ColumnSpec) -> Option<NumFormat>,
) -> i32 {
    *out = Filled::default();
    if state.failure.code != 0 {
        out.failure = state.failure;
        return ERR_STRUCTURE;
    }
    if specs.len() != columns.len() || input.len() > LIMIT || arena.len() > LIMIT {
        return ERR_CONTRACT;
    }
    let Some(scanner) = state.scanner() else {
        return ERR_CONTRACT;
    };
    let mut width = 0usize;
    for (spec, buffer) in specs.iter().zip(columns) {
        if Door::from_code(spec.door, spec.param).is_none()
            || format_of(spec).is_none()
            || buffer.values.is_null()
            || buffer.verdicts.is_null()
        {
            return ERR_CONTRACT;
        }
        let Some(reach) = (spec.ordinal as usize).checked_add(1) else {
            return ERR_CONTRACT;
        };
        width = width.max(reach);
    }
    let walked = match walk(
        state,
        &scanner,
        input,
        last,
        specs,
        width,
        max_rows,
        cells,
        arena.len(),
        &mut [],
        out,
    ) {
        Ok(walked) => walked,
        Err(code) => return code,
    };

    let valid = specs.iter().any(|spec| spec.door == Door::Text.code());
    let vouched = valid.then(|| vouch(input, &walked));
    let mut run = Run {
        data: input,
        cells: walked.cells,
        per_row: walked.per_row,
        rows: walked.rows,
        arena,
        used: 0,
    };
    for (spec, buffer) in specs.iter().zip(columns) {
        let (Some(door), Some(format)) = (Door::from_code(spec.door, spec.param), format_of(spec))
        else {
            return ERR_CONTRACT;
        };
        // SAFETY: the caller's contract, passed on a column at a time.
        unsafe {
            cast_plain(
                &mut run,
                door,
                spec.ordinal as usize,
                &format,
                valid,
                buffer,
            )
        };
    }

    if let Some(vouched) = vouched.filter(|vouched| !vouched.all(walked.rows)) {
        for (spec, buffer) in specs.iter().zip(columns) {
            if spec.door == Door::Text.code() {
                // SAFETY: the caller's contract, passed on; the loop above cast the column.
                unsafe {
                    rejudge(
                        &vouched,
                        walked.rows,
                        run.data,
                        run.arena,
                        buffer.values.cast(),
                        buffer.verdicts,
                        core::ptr::null_mut(),
                    )
                };
            }
        }
    }

    let used = run.used;
    state.deliver(&walked, used, out);
    OK
}

/// A [`ColumnSpecWith`], judged: what a fill reads of it, or `None` for what it can't honour.
struct Resolved<'a> {
    door: Door,
    format: ValidNumFormat,
    /// The options and vocabulary, when the column declared either.
    date: Option<ValidDateOptions<'a>>,
    /// The tokens this column answers to; empty for none.
    tokens: &'a [u8],
    /// The constraints its cells must also meet, when it declares any.
    checks: Option<Checks<'a>>,
}

/// A column's format, judged.
pub(crate) fn resolved_format(spec: &ColumnSpecWith) -> Option<ValidNumFormat> {
    match spec.format.size {
        0 => no_format(&spec.format).then_some(ValidNumFormat::INVARIANT),
        _ => spec.format.resolve().copied()?.validate().ok(),
    }
}

/// A column's date options and vocabulary, judged: `Some(None)` when it declared neither.
///
/// # Safety
/// `spec`'s `months` points to as many live bytes as the length beside it says, unless it's null.
pub(crate) unsafe fn resolved_date(spec: &ColumnSpecWith) -> Option<Option<ValidDateOptions<'_>>> {
    let months = match (spec.months.is_null(), spec.months_len) {
        (true, 0) => None,
        (true, _) => return None,
        // SAFETY: per the function contract.
        (false, len) => Some(
            MonthNames::from_packed(unsafe { core::slice::from_raw_parts(spec.months, len) })
                .ok()?,
        ),
    };
    let raw = spec.date;
    let declared = raw.size != 0;
    if declared
        && ((raw.size != RawDateOptions::SIZE && raw.size != RawDateOptions::MIN_SIZE)
            || raw.pivot > 9999)
    {
        return None;
    }
    let pivot = if raw.size == RawDateOptions::SIZE {
        raw.pivot as u16
    } else {
        0
    };
    let flags = if declared { raw.flags } else { 0 };
    if flags != 0 || pivot != 0 || months.is_some() {
        Some(Some(
            DateOptions::from_flags(flags)
                .with_pivot(pivot)
                .validate(months)
                .ok()?,
        ))
    } else {
        Some(None)
    }
}

/// The tokens a column answers to: `inherited` unless it has its own, whole, or `None` for a list
/// that isn't.
///
/// # Safety
/// `spec`'s `tokens` points to as many live bytes as the length beside it says, unless it's null.
pub(crate) unsafe fn resolved_tokens<'a>(
    spec: &'a ColumnSpecWith,
    inherited: &'a [u8],
) -> Option<&'a [u8]> {
    match (spec.tokens.is_null(), spec.tokens_len) {
        (true, 0) => Some(inherited),
        (true, _) => None,
        (false, 0) => Some(&[]),
        // SAFETY: per the function contract.
        (false, len) => {
            let list = unsafe { core::slice::from_raw_parts(spec.tokens, len) };
            tokens_are_whole(list).then_some(list)
        }
    }
}

impl<'a> Resolved<'a> {
    /// # Safety
    /// `spec`'s `months`, `tokens` and `enum_ptr` point to as many live bytes as the length beside
    /// each says, when they aren't null, and its `constraints` is null or readable. `inherited` is
    /// a list [`tokens_are_whole`] accepted.
    unsafe fn of(spec: &'a ColumnSpecWith, inherited: &'a [u8]) -> Option<Resolved<'a>> {
        let door = Door::from_code(spec.door, spec.param)?;
        Some(Resolved {
            door,
            format: resolved_format(spec)?,
            // SAFETY: per the function contract.
            date: unsafe { resolved_date(spec) }?,
            // SAFETY: per the function contract.
            tokens: unsafe { resolved_tokens(spec, inherited) }?,
            // SAFETY: per the function contract.
            checks: unsafe { resolved_constraints(spec, door) }.ok()?,
        })
    }
}

/// Whether [`fill_with`] would take this column, given no list of its dialect's to inherit: what
/// a caller can ask before any read. A column that declares constraints also needs `checks` in
/// its buffer, which this can't see.
///
/// # Safety
/// `spec`'s `months`, `tokens` and `enum_ptr` point to as many live bytes as the length beside
/// each says, when they aren't null, and its `constraints` is null or readable.
pub unsafe fn honours(spec: &ColumnSpecWith) -> bool {
    // SAFETY: per the function contract, and an empty list is whole.
    unsafe { Resolved::of(spec, &[]) }.is_some()
}

/// [`fill`], with the column options [`ColumnSpecWith`] declares and the notes
/// [`ColumnBufferWith`] takes: `tokens` is the list a column without its own inherits, and
/// `row_widths`, unless it's empty, gets the cells each delivered row had.
///
/// Everything is judged before anything is written: [`ERR_CONTRACT`] leaves `state`, `out` and
/// every buffer as they were. A column asking for nothing new runs [`fill`]'s loop, and asks no
/// door why.
///
/// # Safety
/// As [`fill`], `notes` as `verdicts` unless it's null, and `specs`' pointers as
/// [`ColumnSpecWith`] says.
#[allow(clippy::too_many_arguments)]
pub unsafe fn fill_with(
    state: &mut State,
    input: &[u8],
    last: bool,
    specs: &[ColumnSpecWith],
    columns: &[ColumnBufferWith],
    max_rows: usize,
    cells: &mut [Span],
    arena: &mut [u8],
    tokens: &[u8],
    row_widths: &mut [u32],
    out: &mut Filled,
) -> i32 {
    if specs.len() != columns.len()
        || input.len() > LIMIT
        || arena.len() > LIMIT
        || !tokens_are_whole(tokens)
    {
        return ERR_CONTRACT;
    }
    let Some(scanner) = state.scanner() else {
        return ERR_CONTRACT;
    };
    let mut width = 0usize;
    for (spec, buffer) in specs.iter().zip(columns) {
        // SAFETY: the caller's contract, and the inherited list was judged above.
        let Some(column) = (unsafe { Resolved::of(spec, tokens) }) else {
            return ERR_CONTRACT;
        };
        if buffer.values.is_null()
            || buffer.verdicts.is_null()
            || (column.checks.is_some() && (buffer.checks.is_null() || !buffer.checks.is_aligned()))
        {
            return ERR_CONTRACT;
        }
        let Some(reach) = (spec.ordinal as usize).checked_add(1) else {
            return ERR_CONTRACT;
        };
        width = width.max(reach);
    }

    *out = Filled::default();
    if state.failure.code != 0 {
        out.failure = state.failure;
        return ERR_STRUCTURE;
    }
    let widths = row_widths;
    let walked = match walk(
        state,
        &scanner,
        input,
        last,
        specs,
        width,
        max_rows,
        cells,
        arena.len(),
        widths,
        out,
    ) {
        Ok(walked) => walked,
        Err(code) => return code,
    };

    let valid = specs.iter().any(|spec| spec.door == Door::Text.code());
    let vouched = valid.then(|| vouch(input, &walked));
    let mut run = Run {
        data: input,
        cells: walked.cells,
        per_row: walked.per_row,
        rows: walked.rows,
        arena,
        used: 0,
    };
    for (spec, buffer) in specs.iter().zip(columns) {
        // SAFETY: as above.
        let Some(column) = (unsafe { Resolved::of(spec, tokens) }) else {
            return ERR_CONTRACT;
        };
        let ordinal = spec.ordinal as usize;
        // SAFETY: the caller's contract, passed on a column at a time.
        unsafe {
            if let Some(checks) = column.checks {
                cast_checked_column(&mut run, ordinal, &column, checks, buffer);
            } else if buffer.notes.is_null() && column.tokens.is_empty() && column.date.is_none() {
                let plain = ColumnBuffer {
                    values: buffer.values,
                    verdicts: buffer.verdicts,
                };
                cast_plain(
                    &mut run,
                    column.door,
                    ordinal,
                    column.format.format(),
                    valid,
                    &plain,
                );
            } else {
                cast_noted(&mut run, column.door, ordinal, &column, valid, buffer);
            }
        }
    }

    if let Some(vouched) = vouched.filter(|vouched| !vouched.all(walked.rows)) {
        for (spec, buffer) in specs.iter().zip(columns) {
            // A checked text column's door asked for every cell already.
            if spec.door == Door::Text.code() && spec.constraints.is_null() {
                // SAFETY: the caller's contract, passed on; the loop above cast the column.
                unsafe {
                    rejudge(
                        &vouched,
                        walked.rows,
                        run.data,
                        run.arena,
                        buffer.values.cast(),
                        buffer.verdicts,
                        buffer.notes,
                    )
                };
            }
        }
    }

    let used = run.used;
    state.deliver(&walked, used, out);
    OK
}

/// The sink of a header read: one record's cells into the caller's name table.
struct Names<'a> {
    data: &'a [u8],
    names: &'a mut [Span],
    seen: usize,
    first_empty: bool,
    skip_blank: bool,
    records: u64,
    taken: bool,
    /// Where the record that was taken starts, and its line.
    start: usize,
    line: u32,
    /// Records still to discard.
    header_row: u32,
}

impl Sink for Names<'_> {
    fn cell(&mut self, start: usize, end: usize, quotes: u32) {
        if self.seen == 0 {
            self.first_empty = start == end;
        }
        if let Some(slot) = self.names.get_mut(self.seen) {
            *slot = locate(self.data, start, end, quotes);
        }
        self.seen += 1;
    }

    fn discard(&mut self) {
        self.seen = 0;
    }

    fn row(&mut self, start: usize, _next: usize, line: u32, terminator: u8) -> Flow {
        self.records += 1;
        if self.seen == 1 && self.first_empty && self.skip_blank && terminator != 0 {
            self.seen = 0;
            return Flow::More;
        }
        if self.header_row > 0 {
            self.header_row -= 1;
            self.seen = 0;
            return Flow::More;
        }
        self.taken = true;
        self.start = start;
        self.line = line;
        Flow::Full
    }
}

/// Reads the next record as a header: each cell's text located in `names`, as a `Text`
/// value is — a span into the input, flagged when it is in the arena instead. Fixes the
/// width every later record must have. `out.rows` is the number of names; `0` with
/// nothing left over and `last` set means the input had no record at all.
///
/// Returns [`OK`]; [`ERR_CELLS`] when the record has more cells than `names` holds
/// (`out.needed` says how many) and [`ERR_ARENA`] when its escaped names need more arena
/// than there is — nothing consumed either way; [`ERR_STRUCTURE`] or [`ERR_CONTRACT`] as
/// [`fill`], and [`ERR_STRUCTURE`] with [`Failure::ENCODING`] for a name that isn't valid
/// UTF-8.
pub fn header(
    state: &mut State,
    input: &[u8],
    last: bool,
    names: &mut [Span],
    arena: &mut [u8],
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.failure.code != 0 {
        out.failure = state.failure;
        return ERR_STRUCTURE;
    }
    if input.len() > LIMIT || arena.len() > LIMIT {
        return ERR_CONTRACT;
    }
    let Some(scanner) = state.scanner() else {
        return ERR_CONTRACT;
    };
    let Some(first) = state.first_byte(input, last) else {
        return OK;
    };
    let (first, at_line, skip) = state.past_skipped(&scanner, input, first, last);
    if skip != 0 {
        state.skip = skip;
        state.advance(first, at_line, state.records);
        out.consumed = first as u64;
        return OK;
    }
    let mut sink = Names {
        data: input,
        names,
        seen: 0,
        first_empty: false,
        skip_blank: state.skip_blank_lines != 0,
        records: state.records,
        taken: false,
        start: 0,
        line: 0,
        header_row: state.header_row,
    };
    let stop = scanner.scan_into(input, first, at_line, last, &mut sink);
    let Names {
        names,
        seen,
        records,
        taken,
        start,
        line,
        header_row,
        ..
    } = sink;

    if !taken {
        state.skip = 0;
        state.header_row = header_row;
        state.advance(stop.next, stop.line, records);
        out.consumed = stop.next as u64;
        if stop.unclosed {
            let failure = Failure {
                code: Failure::UNCLOSED_QUOTE,
                line: stop.line,
                record: records,
                byte: state.offset,
                expected: 0,
                found: 0,
            };
            return state.fail(failure, out);
        }
        return OK;
    }
    if seen > names.len() {
        out.needed = seen as u64;
        return ERR_CELLS;
    }
    let names = names.get_mut(..seen).unwrap_or_default();
    let need: usize = names
        .iter()
        .filter(|name| name.flagged())
        .map(|name| name.len())
        .sum();
    if need > arena.len() {
        out.needed = need as u64;
        return ERR_ARENA;
    }
    if state.expected != 0 && state.expected != seen as u32 {
        let failure = Failure {
            code: Failure::COLUMN_COUNT,
            line,
            record: records - 1,
            byte: state.offset + start as u64,
            expected: state.expected,
            found: seen as u32,
        };
        return state.fail(failure, out);
    }
    let mut used = 0usize;
    for name in names {
        let (bytes, span) = text(input, &mut *arena, &mut used, *name);
        // A name has no verdict to carry its fault, and a name the door refuses can't be
        // asked for. An empty name is a name.
        if matches!(hypercast::cast_text(bytes), Err(fault) if fault.reason == Reason::Malformed) {
            let failure = Failure {
                code: Failure::ENCODING,
                line,
                record: records - 1,
                byte: state.offset + start as u64,
                expected: 0,
                found: 0,
            };
            return state.fail(failure, out);
        }
        *name = span;
    }
    state.expected = seen as u32;
    state.skip = 0;
    state.header_row = header_row;
    state.advance(stop.next, stop.line, records);
    out.rows = seen as u64;
    out.consumed = stop.next as u64;
    out.arena_used = used as u64;
    OK
}
