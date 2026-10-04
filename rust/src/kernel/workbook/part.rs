//! One part of the container being read: where its bytes are, how far the read has got,
//! and — for a deflated part — the window of inflated bytes the tokenizer looks through.
//!
//! The window is the caller's. Inflate writes into it and reads its own back-references
//! out of it, so the last 32 KiB written always stay in front of the write position; what
//! lies before both that and the token being read is slid out when room runs short. A
//! token that would not fit even then is the caller's cue to bring a larger window, with
//! what the old one held copied into it: every position kept here is an offset, so the
//! read goes on from where it stopped. A stored part needs no window at all — it is
//! tokenized where it lies in the container.

use super::xml::{self, Kind, Scan, Token, slice};
use super::zip::{Located, METHOD_DEFLATE};
use crate::kernel::abi::Failure;
use crate::kernel::inflate::{Inflate, Status};

/// The smallest window a deflated part can be read through: the 32 KiB inflate looks back
/// over, and as much again to write into.
pub const WINDOW_MIN: usize = 64 * 1024;
/// How far back a deflate match may reach.
const HISTORY: u64 = 32 * 1024;
/// The longest thing inflate writes in one step; less room than this and it may not move.
const MATCH_MAX: u64 = 258;

/// Why a read stopped short of what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The window cannot hold the token being read; this many bytes would be a fair size.
    Window(u64),
    /// The arena is full; it needs at least this many bytes.
    Arena(u64),
    /// The span table is full; it needs at least this many entries.
    Cells(u64),
    /// The data is broken.
    Fail(Failure),
    /// The caller broke the contract.
    Contract,
}

/// The read position in one part. Plain integers: any bit pattern is a state the code
/// below can be handed without harm.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// Where the part's bytes start in the container.
    data: u64,
    /// How many of them the container holds.
    len: u64,
    /// Inflated bytes still to come before the size the directory declared is reached.
    limit: u64,
    /// Compressed bytes consumed.
    fed: u64,
    /// Inflated bytes that have been slid out of the window.
    base: u64,
    /// The tokenizer's position: in the window, or in the stored bytes.
    pos: u64,
    /// Bytes inflate has written into the window.
    end: u64,
    /// How many of those count: the part ends at its declared size.
    visible: u64,
    /// Depth inside an element that is being skipped to its end.
    skip: u32,
    /// Which part this is, for a failure to name (`Failure::PART_*`).
    id: u32,
    deflated: u32,
    eof: u32,
}

impl Part {
    /// Positions a read at the start of `located`.
    pub fn begin(&mut self, inflate: &mut Inflate, located: Located, id: u32) {
        inflate.reset();
        let deflated = located.method == METHOD_DEFLATE;
        *self = Part {
            data: located.data,
            len: if deflated {
                located.len
            } else {
                located.len.min(located.limit)
            },
            limit: located.limit,
            fed: 0,
            base: 0,
            pos: 0,
            end: 0,
            visible: 0,
            skip: 0,
            id,
            deflated: u32::from(deflated),
            eof: u32::from(!deflated || located.limit == 0),
        };
    }

    /// A read with nothing in it: the part was absent, and its end is where it starts.
    pub fn begin_empty(&mut self, id: u32) {
        *self = Part {
            id,
            eof: 1,
            ..Part::default()
        };
    }

    /// How far into the part's inflated bytes the read has got.
    pub fn offset(&self) -> u64 {
        self.base.wrapping_add(self.pos)
    }

    /// A failure in this part, at this position.
    pub fn failure(&self, code: u32, found: u32) -> Failure {
        Failure {
            code,
            line: 0,
            record: u64::from(self.id),
            byte: self.offset(),
            expected: 0,
            found,
        }
    }
}

/// A part being tokenized: the position, the decoder, and the two buffers it reads from.
pub struct Reader<'a> {
    pub part: &'a mut Part,
    pub inflate: &'a mut Inflate,
    pub container: &'a [u8],
    pub window: &'a mut [u8],
}

impl Reader<'_> {
    /// The bytes token offsets index: the stored part itself, or the window.
    pub fn buf(&self) -> &[u8] {
        if self.part.deflated != 0 {
            let visible = usize::try_from(self.part.visible).unwrap_or(usize::MAX);
            return self.window.get(..visible).unwrap_or_default();
        }
        let (Ok(from), Ok(len)) = (
            usize::try_from(self.part.data),
            usize::try_from(self.part.len),
        ) else {
            return &[];
        };
        slice(self.container, from, from.saturating_add(len))
    }

    /// The next token, not yet consumed: [`Reader::take`] moves past it, and until then a
    /// second `peek` finds it again — which is how a read that stopped for want of room
    /// is picked up. Elements being skipped are skipped here.
    pub fn peek(&mut self) -> Result<Token, Stop> {
        loop {
            let pos = usize::try_from(self.part.pos).unwrap_or(usize::MAX);
            match xml::scan(self.buf(), pos, self.part.eof != 0) {
                Scan::Token(token) => {
                    if self.part.skip == 0 {
                        return Ok(token);
                    }
                    match token.kind {
                        Kind::Start => self.part.skip = self.part.skip.saturating_add(1),
                        Kind::End => self.part.skip -= 1,
                        Kind::Eof => {
                            return Err(Stop::Fail(
                                self.part.failure(Failure::XML, xml::UNTERMINATED_ELEMENT),
                            ));
                        }
                        _ => {}
                    }
                    self.part.pos = token.next as u64;
                }
                Scan::More(resume) => {
                    self.part.pos = resume as u64;
                    self.more()?;
                }
                Scan::Malformed(detail) => {
                    return Err(Stop::Fail(self.part.failure(Failure::XML, detail)));
                }
            }
        }
    }

    /// True once the whole part is in view.
    pub fn at_end(&self) -> bool {
        self.part.eof != 0
    }

    /// Where in [`Reader::buf`] the read stands.
    pub fn position(&self) -> usize {
        usize::try_from(self.part.pos).unwrap_or(usize::MAX)
    }

    /// Moves the read to `position` in [`Reader::buf`]: what lies before it may be slid
    /// out of the window.
    pub fn seek(&mut self, position: usize) {
        self.part.pos = position as u64;
    }

    /// Moves past a token [`Reader::peek`] returned.
    pub fn take(&mut self, token: &Token) {
        self.part.pos = token.next as u64;
    }

    /// Moves past a start tag and everything up to the end tag that closes it.
    pub fn skip_element(&mut self, token: &Token) {
        self.part.pos = token.next as u64;
        self.part.skip = 1;
    }

    /// Inflates more of the part into the window, sliding it first if room is short.
    pub fn more(&mut self) -> Result<(), Stop> {
        let part = &mut *self.part;
        if part.deflated == 0 || part.eof != 0 {
            part.eof = 1;
            return Ok(());
        }
        if self.window.len() < WINDOW_MIN {
            return Err(Stop::Window(WINDOW_MIN as u64));
        }
        let cap = self.window.len() as u64;
        if part.end > cap || part.pos > part.visible || part.visible > part.end {
            // Not a state this code leaves behind: the caller's block was disturbed.
            return Err(Stop::Contract);
        }
        if part.end + MATCH_MAX > cap {
            // Everything before both the token being read and inflate's history can go.
            // Too little to be worth the move means the token is most of the window.
            let keep = part.pos.min(part.end.saturating_sub(HISTORY));
            if keep < cap / 4 {
                return Err(Stop::Window(cap.saturating_mul(2)));
            }
            if let Some(held) = self.window.get_mut(..part.end as usize)
                && keep as usize <= held.len()
            {
                held.copy_within(keep as usize.., 0);
            }
            part.base += keep;
            part.pos -= keep;
            part.end -= keep;
            part.visible -= keep;
        }
        let from = usize::try_from(part.data.saturating_add(part.fed)).unwrap_or(usize::MAX);
        let to = usize::try_from(part.data.saturating_add(part.len)).unwrap_or(usize::MAX);
        let input = slice(self.container, from, to);
        let progress = self.inflate.run(input, self.window, part.end as usize);
        part.fed += progress.consumed as u64;
        part.end += progress.written as u64;
        let counted = (progress.written as u64).min(part.limit);
        part.visible += counted;
        part.limit -= counted;
        if part.limit == 0 {
            part.eof = 1;
        }
        match progress.status {
            Status::Done => part.eof = 1,
            Status::Invalid => return Err(Stop::Fail(part.failure(Failure::DEFLATE, 0))),
            // The container ran out before the stream did.
            Status::NeedsInput if part.fed >= part.len && part.eof == 0 => {
                return Err(Stop::Fail(part.failure(Failure::DEFLATE, 1)));
            }
            _ => {}
        }
        Ok(())
    }
}
