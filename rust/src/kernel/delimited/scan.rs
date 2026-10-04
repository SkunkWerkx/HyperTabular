//! The block walker: turns a byte buffer into cell ends and rows, 64 bytes at a time.
//!
//! Per block: classify (four masks), take the prefix XOR of the quote mask and XOR in the
//! carried parity — that is the inside-quotes mask — then `structural = (sep | lf | cr) &
//! !inside`. Structural bits are popped in order with `trailing_zeros` and `x & (x - 1)`;
//! each one ends a cell, and `\n`/`\r` also end the row (a `\r` immediately followed by
//! `\n` is one ending — the `\n` is cleared from the mask, or carried into the next block
//! when the `\r` sits at bit 63). Each cell is reported with how many quotes it contained,
//! from a popcount of the quote mask between its start and end, which is the whole
//! "does this cell need unescaping" decision (Sep's `SepColInfo`).
//!
//! What the walker finds goes to a [`Sink`] rather than into arrays of its own: the walker
//! owns no memory. A sink that collects rows into vectors, and one that writes them into a
//! caller's buffers, see exactly the same calls.
//!
//! The walker is only ever entered at a row boundary — where the quote parity is known to
//! be zero and no `\r` is pending — so a scan that stops early (the sink is full, or the
//! data ends mid-row) can be resumed by simply calling again from the returned position.
//! An unfinished row is never delivered; its bytes are re-scanned once more of it has
//! arrived, which costs at most one row's worth of work per refill and removes every
//! rebasing step from the buffer model.

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
}

impl Scanner {
    /// A scanner with the best engine this CPU offers.
    pub fn new(sep: u8, quoting: bool) -> Scanner {
        Scanner::with_engine(sep, quoting, engine::detect())
    }

    /// A scanner on a specific engine (the conformance suite runs every available one).
    pub fn with_engine(sep: u8, quoting: bool, kind: Kind) -> Scanner {
        Scanner { sep, quoting, kind }
    }

    /// The engine in use.
    pub fn engine(&self) -> Kind {
        self.kind
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
        };
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
unsafe fn walk_neon<S: Sink>(args: Args<'_, S>) -> Stop {
    unsafe { walk::<engine::Neon, S>(args) }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,aes")]
unsafe fn walk_neon_pmull<S: Sink>(args: Args<'_, S>) -> Stop {
    unsafe { walk::<engine::NeonPmull, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn walk_sse2<S: Sink>(args: Args<'_, S>) -> Stop {
    unsafe { walk::<engine::Sse2, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2,pclmulqdq")]
unsafe fn walk_sse2_clmul<S: Sink>(args: Args<'_, S>) -> Stop {
    unsafe { walk::<engine::Sse2Clmul, S>(args) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,pclmulqdq")]
unsafe fn walk_avx2_clmul<S: Sink>(args: Args<'_, S>) -> Stop {
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

/// The walker proper. See the module doc.
///
/// # Safety
/// `E`'s CPU features were detected.
#[inline(always)]
unsafe fn walk<E: Engine, S: Sink>(args: Args<'_, S>) -> Stop {
    let Args {
        sep,
        quote_all,
        data,
        pos,
        mut line,
        eof,
        sink,
    } = args;
    let len = data.len();
    let mut row_start = pos;
    let mut cell_start = pos;
    let mut row_line = line;
    let mut inside: u64 = 0;
    let mut cr_carry = false;
    let mut quote_acc: u32 = 0;
    let mut block_pos = pos;
    let mut tail: [u8; 64];

    while block_pos < len {
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
        // SAFETY: as above.
        let parity = unsafe { E::prefix_xor(quote) } ^ inside;
        inside = ((parity as i64) >> 63) as u64;
        let mut structural = (masks.sep | masks.lf | masks.cr) & !parity;
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
                        return Stop {
                            next: row_start,
                            partial: true,
                            line: row_line,
                            unclosed: false,
                        };
                    }
                }
            } else {
                1
            };
            // Newlines inside this row's quoted cells advance the line count first.
            line += (masks.lf & parity & between(row_bit, p)).count_ones();
            let next = abs + terminator as usize;
            let flow = sink.row(row_start, next, row_line, terminator);
            if flow == Flow::Refused {
                return Stop {
                    next: row_start,
                    partial: false,
                    line: row_line,
                    unclosed: false,
                };
            }
            line += 1;
            row_start = next;
            cell_start = row_start;
            cell_bit = p + terminator as u32;
            row_bit = cell_bit;
            row_line = line;
            if flow == Flow::Full {
                return Stop {
                    next: row_start,
                    partial: false,
                    line,
                    unclosed: false,
                };
            }
        }

        quote_acc += (quote & from(cell_bit)).count_ones();
        // Newlines inside a quoted cell of the unfinished row advance the line count.
        line += (masks.lf & parity & from(row_bit)).count_ones();
        block_pos += 64;
    }

    if !eof {
        // The data ended mid-row (or exactly at a row boundary with nothing pending).
        sink.discard();
        return Stop {
            next: row_start,
            partial: row_start < len,
            line: row_line,
            unclosed: false,
        };
    }
    if inside != 0 {
        sink.discard();
        return Stop {
            next: row_start,
            partial: false,
            line: row_line,
            unclosed: true,
        };
    }
    if row_start < len {
        // A final row with no terminator.
        sink.cell(cell_start, len, quote_acc);
        if sink.row(row_start, len, row_line, 0) == Flow::Refused {
            return Stop {
                next: row_start,
                partial: false,
                line: row_line,
                unclosed: false,
            };
        }
        line += 1;
    }
    Stop {
        next: len,
        partial: false,
        line,
        unclosed: false,
    }
}
