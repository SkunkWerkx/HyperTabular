//! Inflate (RFC 1951), in the caller's memory and unable to panic.
//!
//! A workbook is a zip of deflated XML, so the core needs an inflate, and no inflate that
//! exists can be put under the proof the rest of the core is held to: the ones written in
//! safe Rust index their tables, and the ones behind a C ABI turn a panic into an abort
//! the proof cannot see. So this is the core's own.
//!
//! It owns nothing. [`Inflate`] is a fixed-size block of plain integers the caller keeps —
//! the bit buffer, where in a block it is, and the decoding tables for the block in
//! progress. The output is a buffer of the caller's that doubles as the dictionary: bytes
//! are written at `at`, and a back-reference reads the bytes before it, so whoever calls
//! keeps at least the last 32 KiB of what was written in front of where it writes next.
//!
//! Every step either completes or leaves the state exactly as it found it: a symbol is
//! decoded from a copy of the bit buffer and committed only if the whole of it — code,
//! extra bits, the distance after a length — was there, and only if what it writes fits.
//! That is what makes it resumable at any byte of input and any byte of output without a
//! state for every place it might have stopped.
//!
//! The tables are two-level: a first lookup on the next [`LITLEN_ROOT`] bits of input
//! settles any code that short, and a longer one indexes a second table by the bits that
//! follow. Their sizes are bounded for any complete code (each second-level table is
//! covered by at least two codes), which is what lets them be fixed arrays.

/// What a call achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Every byte of input was used and the stream is not finished: it wants more.
    NeedsInput,
    /// The next thing to write does not fit the output.
    OutputFull,
    /// The stream's final block has ended.
    Done,
    /// The input is not a deflate stream.
    Invalid,
}

/// What a call did: how much input it used, how much output it wrote, and why it stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Bytes of the input consumed.
    pub consumed: usize,
    /// Bytes written to the output, starting at the `at` the call was given.
    pub written: usize,
    /// Why the call returned.
    pub status: Status,
}

/// Bits a first-level lookup of a literal/length code takes.
const LITLEN_ROOT: u32 = 11;
/// Bits a first-level lookup of a distance code takes.
const DIST_ROOT: u32 = 8;
/// Bits a lookup of a code-length code takes; no such code is longer.
const PRE_ROOT: u32 = 7;
/// First-level entries plus the most second-level entries a complete code can need: at
/// most half its symbols start a second-level table, each of at most `2^(15 - root)`.
const LITLEN_SIZE: usize = (1 << LITLEN_ROOT) + (288 / 2) * (1 << (15 - LITLEN_ROOT));
const DIST_SIZE: usize = (1 << DIST_ROOT) + (32 / 2) * (1 << (15 - DIST_ROOT));
const PRE_SIZE: usize = 1 << PRE_ROOT;

// A table entry: `value << 16 | kind << 8 | extra << 4 | bits`, where `bits` is how many
// bits of input the lookup uses up.
const KIND_LITERAL: u32 = 0;
const KIND_LENGTH: u32 = 1;
const KIND_END: u32 = 2;
const KIND_SUBTABLE: u32 = 3;
const KIND_INVALID: u32 = 4;
const KIND_DISTANCE: u32 = 5;
const KIND_SYMBOL: u32 = 6;
const INVALID: u32 = KIND_INVALID << 8;

const fn entry(value: u32, kind: u32, extra: u32) -> u32 {
    value << 16 | kind << 8 | extra << 4
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code-length code lengths are sent in.
const PRE_ORDER: [u8; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn litlen_entry(symbol: usize) -> u32 {
    match symbol {
        0..=255 => entry(symbol as u32, KIND_LITERAL, 0),
        256 => entry(0, KIND_END, 0),
        _ => match (
            LENGTH_BASE.get(symbol - 257),
            LENGTH_EXTRA.get(symbol - 257),
        ) {
            (Some(&base), Some(&extra)) => entry(u32::from(base), KIND_LENGTH, u32::from(extra)),
            // 286 and 287 have codes in the fixed tree and mean nothing.
            _ => INVALID,
        },
    }
}

fn dist_entry(symbol: usize) -> u32 {
    match (DIST_BASE.get(symbol), DIST_EXTRA.get(symbol)) {
        (Some(&base), Some(&extra)) => entry(u32::from(base), KIND_DISTANCE, u32::from(extra)),
        _ => INVALID,
    }
}

fn pre_entry(symbol: usize) -> u32 {
    entry(symbol as u32, KIND_SYMBOL, 0)
}

/// Which of a block's three codes a table is for — what its symbols mean. A value rather
/// than a function to call: a call through a pointer is one the compiler has to assume
/// can unwind, and the no-panic proof would have to assume it with it.
#[derive(Clone, Copy)]
enum Code {
    Lengths,
    Literals,
    Distances,
}

impl Code {
    #[inline(always)]
    fn entry(self, symbol: usize) -> u32 {
        match self {
            Code::Lengths => pre_entry(symbol),
            Code::Literals => litlen_entry(symbol),
            Code::Distances => dist_entry(symbol),
        }
    }
}

/// Builds the decoding table of the canonical code whose lengths are `lens` — one length
/// per symbol, `0` for a symbol with no code — into `table`, whose first `1 << root`
/// entries are the first level. `None` for lengths that are no prefix code: more codes
/// than fit, or fewer — except, where `lone` allows it, a code with nothing longer than
/// one bit, which deflate permits for the two codes of a block's body.
fn build(lens: &[u8], root: u32, table: &mut [u32], kind: Code, lone: bool) -> Option<()> {
    let mut count = [0u16; 16];
    for &len in lens {
        *count.get_mut(usize::from(len))? += 1;
    }
    let first = 1usize << root;
    for slot in table.get_mut(..first)? {
        *slot = INVALID;
    }
    let longest = (1..16usize)
        .rev()
        .find(|&len| count.get(len).is_some_and(|&n| n != 0));
    let Some(longest) = longest else {
        // No codes at all: a table that decodes nothing, which is what a block of
        // literals alone sends for distances.
        return Some(());
    };
    // Kraft: each length halves what is left of the code space.
    let mut left = 1i32;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for len in 1..16usize {
        let codes = *count.get(len)?;
        left = (left << 1) - i32::from(codes);
        if left < 0 {
            return None;
        }
        *next.get_mut(len)? = code;
        code = code.wrapping_add(codes) << 1;
    }
    if left > 0 && !(lone && longest == 1) {
        return None;
    }

    let mask = first - 1;
    // How many bits past the root the longest code under each first-level prefix has.
    let mut beyond = [0u8; 1 << LITLEN_ROOT];
    if longest > root as usize {
        let mut codes = next;
        for &len in lens {
            let len = usize::from(len);
            if len == 0 {
                continue;
            }
            let slot = codes.get_mut(len)?;
            let reversed = usize::from(slot.reverse_bits() >> (16 - len));
            *slot = slot.wrapping_add(1);
            if len > root as usize {
                let depth = beyond.get_mut(reversed & mask)?;
                *depth = (*depth).max((len - root as usize) as u8);
            }
        }
        let mut free = first;
        for (prefix, &depth) in beyond.iter().enumerate().take(first) {
            if depth == 0 {
                continue;
            }
            let size = 1usize << depth;
            for slot in table.get_mut(free..free + size)? {
                *slot = INVALID;
            }
            *table.get_mut(prefix)? = entry(free as u32, KIND_SUBTABLE, u32::from(depth)) | root;
            free += size;
        }
    }

    for (symbol, &len) in lens.iter().enumerate() {
        let len = usize::from(len);
        if len == 0 {
            continue;
        }
        let slot = next.get_mut(len)?;
        let reversed = usize::from(slot.reverse_bits() >> (16 - len));
        *slot = slot.wrapping_add(1);
        let symbol = kind.entry(symbol);
        if len <= root as usize {
            let mut index = reversed;
            while index < first {
                *table.get_mut(index)? = symbol | len as u32;
                index += 1 << len;
            }
        } else {
            let pointer = *table.get(reversed & mask)?;
            if pointer >> 8 & 0xFF != KIND_SUBTABLE {
                return None;
            }
            let (start, depth) = ((pointer >> 16) as usize, (pointer >> 4 & 15) as usize);
            let short = len - root as usize;
            let mut index = reversed >> root;
            while index < 1 << depth {
                *table.get_mut(start + index)? = symbol | short as u32;
                index += 1 << short;
            }
        }
    }
    Some(())
}

/// Looks `bits` up in a two-level table: the entry, and how many bits it used in all.
#[inline(always)]
fn lookup(table: &[u32], root: u32, bits: u64) -> (u32, u32) {
    let first = table
        .get((bits & ((1 << root) - 1)) as usize)
        .copied()
        .unwrap_or(INVALID);
    if first >> 8 & 0xFF != KIND_SUBTABLE {
        return (first, first & 15);
    }
    let depth = first >> 4 & 15;
    let index = (first >> 16) as usize + ((bits >> root) & ((1 << depth) - 1)) as usize;
    let second = table.get(index).copied().unwrap_or(INVALID);
    (second, root + (second & 15))
}

const PHASE_HEADER: u8 = 0;
const PHASE_STORED_LENGTH: u8 = 1;
const PHASE_STORED: u8 = 2;
const PHASE_COUNTS: u8 = 3;
const PHASE_PRECODE: u8 = 4;
const PHASE_LENGTHS: u8 = 5;
const PHASE_CODES: u8 = 6;
const PHASE_DONE: u8 = 7;
const PHASE_INVALID: u8 = 8;

/// The decoder. About 27 KiB of plain integers the caller keeps for the length of one
/// stream; see the module doc.
#[repr(C)]
pub struct Inflate {
    bits: u64,
    count: u32,
    phase: u8,
    last: u8,
    /// Bytes left in a stored block.
    stored: u16,
    literal_codes: u16,
    distance_codes: u16,
    precode_codes: u16,
    /// How many lengths (of the precode, then of the two codes) have been read.
    have: u16,
    lens: [u8; 320],
    precode: [u32; PRE_SIZE],
    litlen: [u32; LITLEN_SIZE],
    dist: [u32; DIST_SIZE],
}

impl Default for Inflate {
    fn default() -> Inflate {
        Inflate::new()
    }
}

impl Inflate {
    /// A decoder at the start of a stream.
    pub const fn new() -> Inflate {
        Inflate {
            bits: 0,
            count: 0,
            phase: PHASE_HEADER,
            last: 0,
            stored: 0,
            literal_codes: 0,
            distance_codes: 0,
            precode_codes: 0,
            have: 0,
            lens: [0; 320],
            precode: [0; PRE_SIZE],
            litlen: [0; LITLEN_SIZE],
            dist: [0; DIST_SIZE],
        }
    }

    /// Back to the start of a stream, tables and all.
    pub fn reset(&mut self) {
        self.bits = 0;
        self.count = 0;
        self.phase = PHASE_HEADER;
        self.last = 0;
        self.stored = 0;
        self.have = 0;
    }

    /// Tops the bit buffer up from `input` at `*pos`: a byte at a time until 56 bits or
    /// more are held, or the input runs out.
    #[inline(always)]
    fn refill(&mut self, input: &[u8], pos: &mut usize) {
        if self.count <= 56
            && let Some(chunk) = input.get(*pos..).and_then(|rest| rest.first_chunk::<8>())
        {
            // Eight bytes are there: take as many whole ones as fit, in one read.
            self.bits |= u64::from_le_bytes(*chunk) << self.count;
            let taken = (63 - self.count) >> 3;
            *pos += taken as usize;
            self.count += taken * 8;
            // Part of one more byte came along in the shift. It is not held: a stored
            // block copies its bytes straight from the input, past the buffer.
            self.bits &= (1u64 << self.count) - 1;
            return;
        }
        while self.count <= 56 {
            let Some(&byte) = input.get(*pos) else {
                return;
            };
            self.bits |= u64::from(byte) << self.count;
            self.count += 8;
            *pos += 1;
        }
    }

    #[inline(always)]
    fn take(&mut self, bits: u32) -> u32 {
        let value = (self.bits & ((1u64 << bits) - 1)) as u32;
        self.bits >>= bits;
        self.count -= bits;
        value
    }

    /// Inflates from `input` into `out`, writing from `at`; `out[..at]` is what was
    /// written before and is where back-references read. Returns when the input is used
    /// up, the output is full, the stream ends, or the input is not deflate — and can be
    /// called again with more of whichever it wanted. A stream that has ended or failed
    /// says so again on every later call.
    pub fn run(&mut self, input: &[u8], out: &mut [u8], at: usize) -> Progress {
        let mut pos = 0usize;
        let mut to = at.min(out.len());
        let status = self.step(input, &mut pos, out, &mut to);
        if status == Status::Invalid {
            self.phase = PHASE_INVALID;
        }
        Progress {
            consumed: pos,
            written: to - at.min(out.len()),
            status,
        }
    }

    fn step(&mut self, input: &[u8], pos: &mut usize, out: &mut [u8], to: &mut usize) -> Status {
        loop {
            self.refill(input, pos);
            match self.phase {
                PHASE_HEADER => {
                    if self.last != 0 {
                        self.phase = PHASE_DONE;
                        continue;
                    }
                    if self.count < 3 {
                        return Status::NeedsInput;
                    }
                    self.last = self.take(1) as u8;
                    match self.take(2) {
                        0 => {
                            // A stored block starts on the next byte boundary.
                            let padding = self.count & 7;
                            self.take(padding);
                            self.phase = PHASE_STORED_LENGTH;
                        }
                        1 => {
                            let mut lens = [8u8; 288];
                            for (symbol, len) in lens.iter_mut().enumerate() {
                                *len = match symbol {
                                    0..=143 => 8,
                                    144..=255 => 9,
                                    256..=279 => 7,
                                    _ => 8,
                                };
                            }
                            // Thirty-two distance codes of five bits: the last two have
                            // codes and no meaning.
                            let built =
                                build(&lens, LITLEN_ROOT, &mut self.litlen, Code::Literals, true)
                                    .and_then(|()| {
                                        build(
                                            &[5u8; 32],
                                            DIST_ROOT,
                                            &mut self.dist,
                                            Code::Distances,
                                            true,
                                        )
                                    });
                            if built.is_none() {
                                return Status::Invalid;
                            }
                            self.phase = PHASE_CODES;
                        }
                        2 => self.phase = PHASE_COUNTS,
                        _ => return Status::Invalid,
                    }
                }
                PHASE_STORED_LENGTH => {
                    if self.count < 32 {
                        return Status::NeedsInput;
                    }
                    let len = self.take(16);
                    let complement = self.take(16);
                    if len != !complement & 0xFFFF {
                        return Status::Invalid;
                    }
                    self.stored = len as u16;
                    self.phase = PHASE_STORED;
                }
                PHASE_STORED => {
                    // Whole bytes still in the bit buffer first, then straight across.
                    while self.stored > 0 && self.count >= 8 {
                        let Some(slot) = out.get_mut(*to) else {
                            return Status::OutputFull;
                        };
                        *slot = self.take(8) as u8;
                        *to += 1;
                        self.stored -= 1;
                    }
                    if self.stored > 0 {
                        let from = input.get(*pos..).unwrap_or_default();
                        let room = out.get_mut(*to..).unwrap_or_default();
                        let moved = usize::from(self.stored).min(from.len()).min(room.len());
                        for (slot, &byte) in room.iter_mut().zip(from).take(moved) {
                            *slot = byte;
                        }
                        *pos += moved;
                        *to += moved;
                        self.stored -= moved as u16;
                        if self.stored > 0 {
                            return if from.len() == moved {
                                Status::NeedsInput
                            } else {
                                Status::OutputFull
                            };
                        }
                    }
                    self.phase = PHASE_HEADER;
                }
                PHASE_COUNTS => {
                    if self.count < 14 {
                        return Status::NeedsInput;
                    }
                    self.literal_codes = self.take(5) as u16 + 257;
                    self.distance_codes = self.take(5) as u16 + 1;
                    self.precode_codes = self.take(4) as u16 + 4;
                    if self.literal_codes > 286 || self.distance_codes > 30 {
                        return Status::Invalid;
                    }
                    self.lens = [0; 320];
                    self.have = 0;
                    self.phase = PHASE_PRECODE;
                }
                PHASE_PRECODE => {
                    while self.have < self.precode_codes {
                        if self.count < 3 {
                            return Status::NeedsInput;
                        }
                        let len = self.take(3) as u8;
                        let symbol = PRE_ORDER.get(usize::from(self.have)).copied().unwrap_or(0);
                        if let Some(slot) = self.lens.get_mut(usize::from(symbol)) {
                            *slot = len;
                        }
                        self.have += 1;
                        self.refill(input, pos);
                    }
                    let lens = self.lens.get(..19).unwrap_or_default();
                    if build(lens, PRE_ROOT, &mut self.precode, Code::Lengths, false).is_none() {
                        return Status::Invalid;
                    }
                    self.lens = [0; 320];
                    self.have = 0;
                    self.phase = PHASE_LENGTHS;
                }
                PHASE_LENGTHS => {
                    let total = self.literal_codes + self.distance_codes;
                    while self.have < total {
                        let (found, used) = lookup(&self.precode, PRE_ROOT, self.bits);
                        if found >> 8 & 0xFF != KIND_SYMBOL {
                            // An unassigned pattern, or not enough bits yet to say.
                            return if self.count < PRE_ROOT && *pos >= input.len() {
                                Status::NeedsInput
                            } else {
                                Status::Invalid
                            };
                        }
                        let symbol = found >> 16;
                        let (extra, base) = match symbol {
                            0..=15 => (0, 1),
                            16 => (2, 3),
                            17 => (3, 3),
                            _ => (7, 11),
                        };
                        // Nothing is judged until the whole of it has arrived: a code cut
                        // short by the end of the input can look like any other.
                        if self.count < used + extra {
                            return Status::NeedsInput;
                        }
                        let repeated = match symbol {
                            0..=15 => symbol as u8,
                            16 => {
                                let previous = usize::from(self.have).checked_sub(1);
                                let Some(&previous) = previous.and_then(|at| self.lens.get(at))
                                else {
                                    return Status::Invalid;
                                };
                                previous
                            }
                            _ => 0,
                        };
                        self.take(used);
                        let run = base + self.take(extra) as u16;
                        if self.have + run > total {
                            return Status::Invalid;
                        }
                        let start = usize::from(self.have);
                        for slot in self
                            .lens
                            .get_mut(start..start + usize::from(run))
                            .unwrap_or_default()
                        {
                            *slot = repeated;
                        }
                        self.have += run;
                        self.refill(input, pos);
                    }
                    let literals = usize::from(self.literal_codes);
                    // A block with no end-of-block code could never end.
                    if self.lens.get(256).copied().unwrap_or(0) == 0 {
                        return Status::Invalid;
                    }
                    let (lit, rest) = self.lens.split_at_checked(literals).unwrap_or((&[], &[]));
                    let dist = rest
                        .get(..usize::from(self.distance_codes))
                        .unwrap_or_default();
                    let built = build(lit, LITLEN_ROOT, &mut self.litlen, Code::Literals, true)
                        .and_then(|()| {
                            build(dist, DIST_ROOT, &mut self.dist, Code::Distances, true)
                        });
                    if built.is_none() {
                        return Status::Invalid;
                    }
                    self.phase = PHASE_CODES;
                }
                PHASE_CODES => {
                    if let Some(status) = self.codes(input, pos, out, to) {
                        return status;
                    }
                }
                PHASE_DONE => return Status::Done,
                _ => return Status::Invalid,
            }
        }
    }

    /// The body of a compressed block: literals and matches until the end-of-block code,
    /// which returns `None` with the phase moved on. Anything else that stops it is the
    /// status to return, with nothing half-done.
    #[inline(always)]
    fn codes(
        &mut self,
        input: &[u8],
        pos: &mut usize,
        out: &mut [u8],
        to: &mut usize,
    ) -> Option<Status> {
        loop {
            self.refill(input, pos);
            // Decode from a copy; commit only a whole symbol that fits.
            let (found, used) = lookup(&self.litlen, LITLEN_ROOT, self.bits);
            let starved = |held: u32, needed: u32| held < needed && *pos >= input.len();
            match found >> 8 & 0xFF {
                KIND_LITERAL => {
                    if self.count < used {
                        return Some(Status::NeedsInput);
                    }
                    let Some(slot) = out.get_mut(*to) else {
                        return Some(Status::OutputFull);
                    };
                    *slot = (found >> 16) as u8;
                    *to += 1;
                    self.take(used);
                }
                KIND_LENGTH => {
                    let length_extra = found >> 4 & 15;
                    let mut bits = self.bits >> used;
                    let mut total = used + length_extra;
                    let length =
                        (found >> 16) as usize + (bits & ((1 << length_extra) - 1)) as usize;
                    bits >>= length_extra;
                    let (far, far_used) = lookup(&self.dist, DIST_ROOT, bits);
                    if far >> 8 & 0xFF != KIND_DISTANCE {
                        return Some(if starved(self.count, total + 15) {
                            Status::NeedsInput
                        } else {
                            Status::Invalid
                        });
                    }
                    let distance_extra = far >> 4 & 15;
                    bits >>= far_used;
                    total += far_used + distance_extra;
                    let distance =
                        (far >> 16) as usize + (bits & ((1 << distance_extra) - 1)) as usize;
                    if self.count < total {
                        return Some(Status::NeedsInput);
                    }
                    if distance > *to {
                        return Some(Status::Invalid);
                    }
                    if out.len() - *to < length {
                        return Some(Status::OutputFull);
                    }
                    copy_match(out, *to, distance, length);
                    *to += length;
                    self.take(total);
                }
                KIND_END => {
                    if self.count < used {
                        return Some(Status::NeedsInput);
                    }
                    self.take(used);
                    self.phase = PHASE_HEADER;
                    return None;
                }
                _ => {
                    return Some(if starved(self.count, 15) {
                        Status::NeedsInput
                    } else {
                        Status::Invalid
                    });
                }
            }
        }
    }
}

/// Copies `length` bytes to `out[to..]` from `distance` bytes behind it, a byte being
/// readable as soon as it is written — a distance shorter than the length repeats.
/// The caller has checked `distance <= to` and `to + length <= out.len()`.
#[inline(always)]
fn copy_match(out: &mut [u8], to: usize, distance: usize, length: usize) {
    let mut done = 0;
    while done < length {
        // Each pass copies a stretch that ends before its own destination begins.
        let Some((behind, ahead)) = out.split_at_mut_checked(to + done) else {
            return;
        };
        let Some(source) = behind
            .len()
            .checked_sub(distance)
            .and_then(|start| behind.get(start..))
        else {
            return;
        };
        let stretch = source.len().min(length - done);
        for (slot, &byte) in ahead.iter_mut().zip(source).take(stretch) {
            *slot = byte;
        }
        if stretch == 0 {
            return;
        }
        done += stretch;
    }
}
