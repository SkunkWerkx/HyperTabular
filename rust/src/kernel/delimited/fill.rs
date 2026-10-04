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
//! - **the state block** ([`State`]) — 64 bytes: where the input stands (line, record,
//!   byte offset), the width the first record fixed, and a failure once there is one.
//! - **the columns** — per plan column, `max_rows` values and `max_rows` verdicts.
//! - **the cell table** — `max_rows * (widest ordinal + 2)` [`Span`]s. Row `r`'s entry for
//!   source column `c` is at `r * (width + 1) + c` and locates that cell in the input
//!   (flagged when the cell has `""` inside and must be unescaped to be read); the entry
//!   after a row's last column is the row's own start offset and, in `len`, its line
//!   number. It is scratch for the core and a map for the caller: the raw text of any
//!   cell, one that failed to cast included, is the input at that span.
//! - **the arena** — where an escaped cell is unescaped. A `Text` value is a span into
//!   the input, and flagged when it is a span into the arena instead. A file with no `""`
//!   in it never touches the arena.
//!
//! Nothing is retained between calls except what the state block holds, and nothing here
//! allocates, blocks, or reads a clock.

use crate::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE,
    Failure, Filled, OK, Span,
};
use crate::kernel::delimited::engine::{self, Kind};
use crate::kernel::delimited::scan::{Flow, Scanner, Sink};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use hypercast::{CivilDateTime, Date, Decimal, Duration, Fault, Reason, Timestamp};

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
/// a caller could put in it is one the core cannot survive.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
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
    reserved: [u8; 3],
    /// The structural failure that ended this input, if `code` is nonzero.
    pub failure: Failure,
}

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// Offsets are 31 bits: a [`Span`]'s length keeps its top bit for a flag.
const LIMIT: usize = i32::MAX as usize;

const EMPTY: Fault = Fault {
    reason: Reason::Empty,
    offset: 0,
    len: 0,
};

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
            records: 0,
            offset: 0,
            expected: 0,
            started: 0,
            reserved: [0; 3],
            failure: Failure {
                code: 0,
                line: 0,
                record: 0,
                byte: 0,
                expected: 0,
                found: 0,
            },
        })
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

/// The sink of a fill: writes each row's cells into the caller's table.
struct Rows<'a> {
    data: &'a [u8],
    specs: &'a [ColumnSpec],
    cells: &'a mut [Span],
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
}

impl Sink for Rows<'_> {
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
        let expected = if self.expected == 0 {
            found as u32
        } else {
            self.expected
        };
        if found as u32 != expected {
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
        // Source columns the plan reads past this record's last cell are empty.
        for ordinal in found..self.width {
            if let Some(slot) = self.cells.get_mut(base + ordinal) {
                *slot = Span::default();
            }
        }
        if escaped {
            let mut need = 0usize;
            for spec in self.specs {
                if let Some(cell) = self.cells.get(base + spec.ordinal as usize)
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
            || spec.num_format().is_none()
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
    if max_rows == 0 {
        return OK;
    }
    let Some(per_row) = width.checked_add(1) else {
        return ERR_CONTRACT;
    };
    let capacity = max_rows.min(cells.len() / per_row);
    if capacity == 0 {
        out.needed = per_row as u64;
        return ERR_CELLS;
    }
    let Some(first) = state.first_byte(input, last) else {
        return OK;
    };

    let mut sink = Rows {
        data: input,
        specs,
        cells,
        width,
        per_row,
        capacity,
        rows: 0,
        seen: 0,
        first_empty: false,
        escaped: false,
        skip_blank: state.skip_blank_lines != 0,
        expected: state.expected,
        records: state.records,
        arena_free: arena.len(),
        arena_needed: 0,
        failure: state.failure,
        offset: state.offset,
    };
    let stop = scanner.scan_into(input, first, state.line, last, &mut sink);
    let Rows {
        cells,
        rows,
        expected,
        records,
        arena_needed,
        failure,
        ..
    } = sink;

    if rows == 0 {
        if arena_needed != 0 {
            out.needed = arena_needed as u64;
            return ERR_ARENA;
        }
        state.advance(stop.next, stop.line, records);
        out.consumed = stop.next as u64;
        if failure.code != 0 {
            return state.fail(failure, out);
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
            return state.fail(failure, out);
        }
        return OK;
    }

    let mut run = Run {
        data: input,
        cells,
        per_row,
        rows,
        arena,
        used: 0,
    };
    for (spec, buffer) in specs.iter().zip(columns) {
        let (Some(door), Some(format)) =
            (Door::from_code(spec.door, spec.param), spec.num_format())
        else {
            return ERR_CONTRACT;
        };
        let format = &format;
        let ordinal = spec.ordinal as usize;
        let run = &mut run;
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
                // The bytes themselves, untrimmed; no bytes at all is the one way to fail.
                Door::Text => column(run, ordinal, buffer, Span::default(), |text, span| {
                    if text.is_empty() {
                        Err(EMPTY)
                    } else {
                        Ok(span)
                    }
                }),
            }
        }
    }

    state.expected = expected;
    state.advance(stop.next, stop.line, records);
    out.rows = rows as u64;
    out.consumed = stop.next as u64;
    out.arena_used = run.used as u64;
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
/// [`fill`].
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
    };
    let stop = scanner.scan_into(input, first, state.line, last, &mut sink);
    let Names {
        names,
        seen,
        records,
        taken,
        start,
        line,
        ..
    } = sink;

    if !taken {
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
        let (_, span) = text(input, &mut *arena, &mut used, *name);
        *name = span;
    }
    state.expected = seen as u32;
    state.advance(stop.next, stop.line, records);
    out.rows = seen as u64;
    out.consumed = stop.next as u64;
    out.arena_used = used as u64;
    OK
}
