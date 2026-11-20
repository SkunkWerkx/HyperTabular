//! The block walker: turns a byte buffer into cell ends and rows, 64 bytes at a time.
//!
//! A cell is plain, open or after. A quote opens a quoted cell only as the cell's first byte.
//! Inside, `""` is a literal quote and a lone `"` closes the quoted part. After that, and in a
//! cell that never opened, quotes are ordinary bytes until a separator or a line ending. So
//! plain and after scan alike, and the walker only has to know whether a cell is open. These
//! rules decide boundaries only: what a cell's text reads as is [`unescape`](super::unescape)'s.
//!
//! Per block: classify (four masks), then find `hidden`, the bytes inside quoted cells, and
//! `structural = (sep | lf | cr) & !hidden`.
//!
//! - A block with no quote and no open cell hides nothing.
//! - Otherwise the block's prefix XOR of the quote mask, XORed with the parity carried in, is
//!   `inside`. When every quote that turns it on is a cell's first byte, or the second of a
//!   `""`, `inside` is the answer, which takes a few bit operations whatever the block holds
//!   (a quote that turns it off is always a close or the first of a `""`).
//! - Otherwise a stray quote is in the way, and [`walk_stray`] goes through that block's
//!   cells in order. The same prefix XOR, read from each opening quote, finds its close.
//!
//! An open cell carries across blocks. A quote at bit 63 looks at the next byte in the data
//! to see whether it's half of a `""`.
//!
//! Structural bits are popped in order with `trailing_zeros` and `x & (x - 1)`; each one ends
//! a cell, and `\n`/`\r` also end the row (a `\r` immediately followed by `\n` is one ending —
//! the `\n` is cleared from the mask, or carried into the next block when the `\r` sits at bit
//! 63). Each cell is reported with how many quotes it contained, from a popcount of the quote
//! mask between its start and end, which is the whole "does this cell need unescaping"
//! decision (Sep's `SepColInfo`).
//!
//! What the walker finds goes to a [`Sink`] rather than into arrays of its own: the walker
//! owns no memory. A sink that collects rows into vectors, and one that writes them into a
//! caller's buffers, see exactly the same calls.
//!
//! The walker is only ever entered at a row boundary — where no cell is open and no `\r` is
//! pending — so a scan that stops early (the sink is full, or the
//! data ends mid-row) can be resumed by simply calling again from the returned position.
//! An unfinished row is never delivered; its bytes are re-scanned once more of it has
//! arrived, which costs at most one row's worth of work per refill and removes every
//! rebasing step from the buffer model.
//!
//! A row longer than a refill would be scanned again on every one, and cost the square of its
//! length. So a caller that keeps a [`Resume`] between calls asks [`Scanner::row_end`] first
//! whether the bytes that are new end the row, and scans only once they do. That search is this
//! same walker, entered where it left off rather than at a row boundary, reporting to a sink
//! that stops at the first row: it is a hint about where to look, and never what a row holds.

use crate::kernel::delimited::engine::{self, Engine, Kind};

/// What a sink says after a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    More,
    /// The row is taken and no further row can be: stop after it.
    Full,
    /// The row is not taken: stop before it, so the next scan starts on it again.
    Refused,
}

/// Where the walker reports what it finds. Offsets are into the scanned data.
pub trait Sink {
    /// Whether the walker keeps, for this sink, where a search for a row's end could go on
    /// from. Only [`Scanner::row_end`]'s sink asks, so no other scan pays for it.
    const MARKS: bool = false;

    /// A cell of the row in progress: its bytes are `start..end`, of which `quotes` are
    /// `"`. The byte at `end` is what ended it — a separator, `\r`, `\n`, or the data end.
    fn cell(&mut self, start: usize, end: usize, quotes: u32);

    /// The row in progress turned out to be unfinished: forget its cells.
    fn discard(&mut self);

    /// The row whose cells were just reported is complete. It began at `start` on the
    /// one-based `line`; the next row begins at `next`; `terminator` is the length of
    /// what ended it (0 at the data end, 1 for `\n` or `\r`, 2 for `\r\n`).
    fn row(&mut self, start: usize, next: usize, line: u32, terminator: u8) -> Flow;
}

/// Where a scan stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    /// Offset to resume from: the byte after the last row the sink took.
    pub next: usize,
    /// True when the data ended mid-row (only when `eof` was false); the unfinished
    /// row starts at `next` and nothing of it was delivered.
    pub partial: bool,
    /// One-based line number at `next`.
    pub line: u32,
    /// True when `eof` was set and the data ended inside a quoted cell.
    pub unclosed: bool,
}

/// Where the search for an unfinished row's end stands: a place in the row and what the walker
/// knew there. `at` counts from the row's first byte, and no byte before it ends the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The offset to go on from.
    pub at: usize,
    /// A quoted cell is open there.
    pub open: bool,
    /// The quote parity carried into the block there.
    pub parity: bool,
    /// A cell starts there.
    pub cell: bool,
}

impl Resume {
    /// The start of a row.
    pub const START: Resume = Resume {
        at: 0,
        open: false,
        parity: false,
        cell: true,
    };
}

/// The sink of a search for a row's end: it wants no cell, and stops at the first row.
struct FirstRow;

impl Sink for FirstRow {
    const MARKS: bool = true;

    #[inline(always)]
    fn cell(&mut self, _start: usize, _end: usize, _quotes: u32) {}

    fn discard(&mut self) {}

    #[inline(always)]
    fn row(&mut self, _start: usize, _next: usize, _line: u32, _terminator: u8) -> Flow {
        Flow::Full
    }
}

/// The scanner: a separator, whether quotes count, and the engine to run.
#[derive(Clone, Copy, Debug)]
pub struct Scanner {
    sep: u8,
    quoting: bool,
    kind: Kind,
}

/// Everything one scan needs, bundled so the per-engine entry points share a signature.
struct Args<'a, S> {
    sep: u8,
    quote_all: u64,
    data: &'a [u8],
    pos: usize,
    line: u32,
    eof: bool,
    sink: &'a mut S,
    /// What the walker knows on entry: at a row boundary, nothing is open and a cell starts.
    entry: Resume,
}

impl Scanner {
    /// A scanner on a specific engine (the conformance suite runs every available one).
    pub fn with_engine(sep: u8, quoting: bool, kind: Kind) -> Scanner {
        Scanner { sep, quoting, kind }
    }

    /// True on the portable engine, which reads eight bytes at a time.
    pub(crate) fn is_portable(&self) -> bool {
        matches!(self.kind, Kind::Swar)
    }

    /// The same scanner for reading physical lines: a quote is a byte like another.
    pub fn lines(&self) -> Scanner {
        Scanner {
            sep: self.sep,
            quoting: false,
            kind: self.kind,
        }
    }

    /// Scans `data[pos..]`, which must start at a row boundary, reporting every complete
    /// row to `sink` until it stops the scan or the data runs out. `line` is the
    /// one-based line number at `pos`; `eof` says whether `data` is all there is (so a
    /// final row without a terminator can be closed at the data end).
    pub fn scan_into<S: Sink>(
        &self,
        data: &[u8],
        pos: usize,
        line: u32,
        eof: bool,
        sink: &mut S,
    ) -> Stop {
        let args = Args {
            sep: self.sep,
            quote_all: if self.quoting { u64::MAX } else { 0 },
            data,
            pos,
            line,
            eof,
            sink,
            entry: Resume::START,
        };
        self.walk(args).0
    }

    /// Searches `row`, which starts at a row boundary and has more to come, for the row's end.
    /// `None` when it ends there, and it's for a scan to read; otherwise the row is unfinished,
    /// and this is where the next search goes on from. `from` is [`Resume::START`], or what an
    /// earlier search of a shorter `row` gave: the bytes before `from.at` are not looked at. A
    /// `from` that can't be one for this `row` is `None` too, so the scan decides.
    pub fn row_end(&self, row: &[u8], from: Resume) -> Option<Resume> {
        if from.at > row.len() || (from.at == 0 && from != Resume::START) {
            return None;
        }
        let args = Args {
            sep: self.sep,
            quote_all: if self.quoting { u64::MAX } else { 0 },
            data: row,
            pos: from.at,
            line: 1,
            eof: false,
            sink: &mut FirstRow,
            entry: from,
        };
        let (stop, mark) = self.walk(args);
        // The walker stops with nothing taken only when the data ran out inside the row.
        (stop.next == from.at).then_some(mark)
    }

    /// The walk on this scanner's engine: where it stopped, and where it marked.
    #[inline(always)]
    fn walk<S: Sink>(&self, args: Args<'_, S>) -> (Stop, Resume) {
        // SAFETY: a `Kind` other than `Swar` is only ever made by `engine::detect`, on a
        // CPU that has what it names.
        match self.kind {
            #[cfg(target_arch = "aarch64")]
            Kind::Neon => unsafe { walk_neon(args) },
            #[cfg(target_arch = "aarch64")]
            Kind::NeonPmull => unsafe { walk_neon_pmull(args) },
            #[cfg(target_arch = "x86_64")]
            Kind::Sse2 => unsafe { walk_sse2(args) },
            #[cfg(target_arch = "x86_64")]
            Kind::Sse2Clmul => unsafe { walk_sse2_clmul(args) },
            #[cfg(target_arch = "x86_64")]
            Kind::Avx2Clmul => unsafe { walk_avx2_clmul(args) },
            // The portable engine, and the answer for an engine this target lacks.
            _ => unsafe { walk::<engine::Swar, S>(args) },
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn walk_neon<S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    unsafe { walk::<engine::Neon, S>(args) }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,aes")]
unsafe fn walk_neon_pmull<S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    unsafe { walk::<engine::NeonPmull, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn walk_sse2<S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    unsafe { walk::<engine::Sse2, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2,pclmulqdq")]
unsafe fn walk_sse2_clmul<S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    unsafe { walk::<engine::Sse2Clmul, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,pclmulqdq")]
unsafe fn walk_avx2_clmul<S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    unsafe { walk::<engine::Avx2Clmul, S>(args) }
}

/// Bits `[from, to)`.
#[inline(always)]
fn between(from: u32, to: u32) -> u64 {
    low(to) & !low(from)
}

/// Bits `[0, n)`.
#[inline(always)]
fn low(n: u32) -> u64 {
    if n >= 64 { u64::MAX } else { (1u64 << n) - 1 }
}

/// Bits `[n, 64)`.
#[inline(always)]
fn from(n: u32) -> u64 {
    if n >= 64 { 0 } else { u64::MAX << n }
}

/// Bit `n`, or nothing when `n` is past the block.
#[inline(always)]
fn bit(n: u32) -> u64 {
    if n >= 64 { 0 } else { 1u64 << n }
}

/// The top bit of `x`, as every bit: what carries a parity into the next block.
#[inline(always)]
fn top(x: u64) -> u64 {
    ((x as i64) >> 63) as u64
}

/// A block's quote masks, for the stray-quote walk.
#[derive(Clone, Copy)]
struct Quotes {
    /// The quotes.
    quote: u64,
    /// The inclusive prefix XOR of `quote`.
    toggled: u64,
    /// The quotes with another quote right after them.
    doubled: u64,
}

/// A cell's quoted state across a block edge. When `open` is false, `parity` is 0. When it's
/// true, `parity` is what makes the next block's prefix XOR read 1 inside the quotes: 0 only
/// when that block starts with the second quote of a `""`.
#[derive(Clone, Copy)]
struct Carry {
    open: bool,
    parity: u64,
}

/// The bytes a block's quoted cells hide, cell by cell, and the state at the block's end.
/// `first` is the bit where the first cell starting in this block starts, or 64 for none, and
/// `ends` is every byte that can end a cell.
#[inline(always)]
fn walk_stray(quotes: Quotes, mut ends: u64, first: u32, carry: Carry) -> (u64, Carry) {
    let Quotes {
        quote,
        toggled,
        doubled,
    } = quotes;
    let mut hidden = 0;
    let mut open = carry.open;
    let mut parity = carry.parity;
    let mut start = first;
    loop {
        if open || quote & bit(start) != 0 {
            // Bit `from_bit` is where the quotes begin in this block, and `inside` reads 1
            // from there until each quote that turns it off.
            let from_bit = if open { 0 } else { start };
            if !open {
                parity = if toggled & bit(start) != 0 {
                    0
                } else {
                    u64::MAX
                };
            }
            let inside = toggled ^ parity;
            let closes = quote & !inside & !doubled & from(from_bit);
            if closes == 0 {
                hidden |= from(from_bit);
                return (
                    hidden,
                    Carry {
                        open: true,
                        parity: top(inside),
                    },
                );
            }
            let close = closes.trailing_zeros() & 63;
            hidden |= between(from_bit, close);
            ends &= from(close + 1);
            open = false;
        }
        if ends == 0 {
            return (
                hidden,
                Carry {
                    open: false,
                    parity: 0,
                },
            );
        }
        start = (ends.trailing_zeros() & 63) + 1;
        ends &= ends - 1;
    }
}

/// The walker proper. See the module doc.
///
/// # Safety
/// `E`'s CPU features were detected.
#[inline(always)]
unsafe fn walk<E: Engine, S: Sink>(args: Args<'_, S>) -> (Stop, Resume) {
    let Args {
        sep,
        quote_all,
        data,
        pos,
        mut line,
        eof,
        sink,
        entry,
    } = args;
    let len = data.len();
    let mut row_start = pos;
    // A cell that started before `pos` starts anywhere before it, as far as the walker asks.
    let mut cell_start = if entry.cell {
        pos
    } else {
        pos.saturating_sub(1)
    };
    let mut row_line = line;
    let mut carry = Carry {
        open: entry.open,
        parity: if entry.parity { u64::MAX } else { 0 },
    };
    // Where a search could go on from: the top of the last block entered. What the walker knows
    // there came from whole blocks with a byte after them, so it's what any longer data says too;
    // what it knows after a last block isn't, a quote at the data's end being a close or half of
    // a `""` by the byte that isn't there yet.
    let mut mark = entry;
    let mut cr_carry = false;
    let mut quote_acc: u32 = 0;
    let mut block_pos = pos;
    let mut tail: [u8; 64];

    while block_pos < len {
        if S::MARKS {
            mark = Resume {
                at: block_pos,
                open: carry.open,
                parity: carry.parity != 0,
                cell: cell_start == block_pos,
            };
        }
        let rest = data.get(block_pos..).unwrap_or_default();
        let block: &[u8; 64] = match rest.first_chunk::<64>() {
            Some(block) => block,
            None => {
                // The last, short block: padded with zeros, which are never structural.
                tail = [0u8; 64];
                for (slot, &byte) in tail.iter_mut().zip(rest) {
                    *slot = byte;
                }
                &tail
            }
        };
        // SAFETY: the caller detected E's features.
        let masks = unsafe { E::classify(block, sep) };
        let quote = masks.quote & quote_all;
        let ends = masks.sep | masks.lf | masks.cr;
        let hidden = if quote == 0 && !carry.open {
            0
        } else {
            // SAFETY: as above.
            let toggled = unsafe { E::prefix_xor(quote) };
            // The padding of a last, short block holds no quote.
            let over_edge = quote >> 63 != 0 && data.get(block_pos + 64) == Some(&b'"');
            let doubled = (quote >> 1) | (u64::from(over_edge) << 63);
            let inside = toggled ^ carry.parity;
            let closing = quote & !inside;
            // Bit 0 opens a cell only if a cell starts there; bit 0 of an open cell is the
            // second quote of a `""`.
            let first = u64::from(carry.open || cell_start == block_pos);
            let opening = quote & inside & !((ends | quote) << 1 | first);
            if opening == 0 {
                carry = Carry {
                    open: top(inside | (closing & doubled)) != 0,
                    parity: top(inside),
                };
                inside
            } else {
                let first = if cell_start >= block_pos {
                    (cell_start - block_pos) as u32
                } else {
                    64
                };
                let quotes = Quotes {
                    quote,
                    toggled,
                    doubled,
                };
                let (hidden, next) = walk_stray(quotes, ends, first, carry);
                carry = next;
                hidden
            }
        };
        let mut structural = ends & !hidden;
        if cr_carry {
            // The previous block ended a row with `\r` and this one starts with its `\n`.
            structural &= !1;
            cr_carry = false;
        }
        let mut cell_bit = cell_start.saturating_sub(block_pos) as u32;
        let mut row_bit = row_start.saturating_sub(block_pos) as u32;

        while structural != 0 {
            let p = structural.trailing_zeros() & 63;
            structural &= structural - 1;
            let abs = block_pos + p as usize;
            let byte = block[p as usize];
            let quotes = quote_acc + (quote & between(cell_bit, p)).count_ones();
            quote_acc = 0;
            sink.cell(cell_start, abs, quotes);
            if byte == sep {
                cell_start = abs + 1;
                cell_bit = p + 1;
                continue;
            }
            // A row ends here.
            let terminator: u8 = if byte == b'\r' {
                match data.get(abs + 1) {
                    Some(b'\n') => {
                        if p < 63 {
                            structural &= !(1u64 << (p + 1));
                        } else {
                            cr_carry = true;
                        }
                        2
                    }
                    Some(_) => 1,
                    None if eof => 1,
                    None => {
                        // A `\r` at the very end of a non-final buffer: its `\n` may be
                        // in the next read. Leave the row unfinished.
                        sink.discard();
                        return (
                            Stop {
                                next: row_start,
                                partial: true,
                                line: row_line,
                                unclosed: false,
                            },
                            mark,
                        );
                    }
                }
            } else {
                1
            };
            // Newlines inside this row's quoted cells advance the line count first.
            line += (masks.lf & hidden & between(row_bit, p)).count_ones();
            let next = abs + terminator as usize;
            let flow = sink.row(row_start, next, row_line, terminator);
            if flow == Flow::Refused {
                return (
                    Stop {
                        next: row_start,
                        partial: false,
                        line: row_line,
                        unclosed: false,
                    },
                    mark,
                );
            }
            line += 1;
            row_start = next;
            cell_start = row_start;
            cell_bit = p + terminator as u32;
            row_bit = cell_bit;
            row_line = line;
            if flow == Flow::Full {
                return (
                    Stop {
                        next: row_start,
                        partial: false,
                        line,
                        unclosed: false,
                    },
                    mark,
                );
            }
        }

        quote_acc += (quote & from(cell_bit)).count_ones();
        // Newlines inside a quoted cell of the unfinished row advance the line count.
        line += (masks.lf & hidden & from(row_bit)).count_ones();
        block_pos += 64;
    }

    if !eof {
        // The data ended mid-row (or exactly at a row boundary with nothing pending).
        sink.discard();
        return (
            Stop {
                next: row_start,
                partial: row_start < len,
                line: row_line,
                unclosed: false,
            },
            mark,
        );
    }
    if carry.open {
        sink.discard();
        return (
            Stop {
                next: row_start,
                partial: false,
                line: row_line,
                unclosed: true,
            },
            mark,
        );
    }
    if row_start < len {
        // A final row with no terminator.
        sink.cell(cell_start, len, quote_acc);
        if sink.row(row_start, len, row_line, 0) == Flow::Refused {
            return (
                Stop {
                    next: row_start,
                    partial: false,
                    line: row_line,
                    unclosed: false,
                },
                mark,
            );
        }
        line += 1;
    }
    (
        Stop {
            next: len,
            partial: false,
            line,
            unclosed: false,
        },
        mark,
    )
}
