//! A pull tokenizer over bytes the caller holds: start, empty and end tags with lazy
//! attributes, text, CDATA; comments, processing instructions and doctypes are recognised
//! and stepped over. Namespace prefixes are ignored by every consumer ([`Tag::local`]):
//! `<x:c>` and `<c>` are the same cell.
//!
//! It keeps nothing. [`scan`] is asked for the token at a position in a buffer and answers
//! with offsets into that buffer, or says the token runs past what the buffer holds — and
//! whoever holds the buffer brings more and asks again. That is what lets a part be read
//! through a window that slides, and a read be picked up after the window had to grow.

/// What [`scan`] found at a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `<name attrs…>`: the name is `a..b`, the raw attribute bytes `b..c`.
    Start,
    /// `<name attrs…/>`, the same offsets.
    Empty,
    /// `</name>`: the name is `a..b`.
    End,
    /// Character data between tags, entities not yet decoded: `a..b`.
    Text,
    /// `<![CDATA[…]]>` content, literal: `a..b`.
    CData,
    /// The end of the input.
    Eof,
}

/// One token, as offsets into the buffer it was scanned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub a: usize,
    pub b: usize,
    pub c: usize,
    /// Where the token after this one starts.
    pub next: usize,
}

/// The answer to a [`scan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    /// A whole token.
    Token(Token),
    /// The token at the position given runs past the buffer, and more input exists.
    /// Everything before the position carried here has been stepped over for good.
    More(usize),
    /// The input ended inside a construct; the value says which (see `UNTERMINATED_*`).
    Malformed(u32),
}

/// The input ended inside a comment.
pub const UNTERMINATED_COMMENT: u32 = 1;
/// The input ended inside a CDATA section.
pub const UNTERMINATED_CDATA: u32 = 2;
/// The input ended inside a processing instruction.
pub const UNTERMINATED_INSTRUCTION: u32 = 3;
/// The input ended inside a declaration.
pub const UNTERMINATED_DECLARATION: u32 = 4;
/// The input ended inside a tag.
pub const UNTERMINATED_TAG: u32 = 5;
/// The input ended inside an element that had to be read to its end.
pub const UNTERMINATED_ELEMENT: u32 = 6;

/// `bytes[from..to]`, or nothing if that is not a range of it.
#[inline(always)]
pub fn slice(bytes: &[u8], from: usize, to: usize) -> &[u8] {
    bytes.get(from..to).unwrap_or_default()
}

/// `bytes[from..]`, or nothing.
#[inline(always)]
pub fn tail(bytes: &[u8], from: usize) -> &[u8] {
    bytes.get(from..).unwrap_or_default()
}

/// Finds `needle` in `hay[from..]`, eight bytes at a time.
pub fn find_byte(hay: &[u8], from: usize, needle: u8) -> Option<usize> {
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    let needle_word = u64::from(needle).wrapping_mul(0x0101_0101_0101_0101);
    let mut at = from;
    let mut rest = hay.get(from..)?;
    while let Some((chunk, after)) = rest.split_first_chunk::<8>() {
        let word = u64::from_le_bytes(*chunk) ^ needle_word;
        let zero = !((word & LOW7).wrapping_add(LOW7) | word | LOW7);
        if zero != 0 {
            return Some(at + (zero.trailing_zeros() / 8) as usize);
        }
        at += 8;
        rest = after;
    }
    rest.iter().position(|&b| b == needle).map(|i| at + i)
}

/// Finds the first `>`, `"` or `'` in `hay[from..]` — what ends a tag, and what opens a
/// quoted value inside one — eight bytes at a time, as [`find_byte`] finds one byte.
fn find_tag_byte(hay: &[u8], from: usize) -> Option<usize> {
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    const GT: u64 = 0x3E3E_3E3E_3E3E_3E3E;
    const DOUBLE: u64 = 0x2222_2222_2222_2222;
    const SINGLE: u64 = 0x2727_2727_2727_2727;
    // A byte of `word` that is zero sets that byte's high bit here; no other byte does.
    let zeros = |word: u64| !((word & LOW7).wrapping_add(LOW7) | word | LOW7);
    let mut at = from;
    let mut rest = hay.get(from..)?;
    while let Some((chunk, after)) = rest.split_first_chunk::<8>() {
        let word = u64::from_le_bytes(*chunk);
        let hit = zeros(word ^ GT) | zeros(word ^ DOUBLE) | zeros(word ^ SINGLE);
        if hit != 0 {
            return Some(at + (hit.trailing_zeros() / 8) as usize);
        }
        at += 8;
        rest = after;
    }
    rest.iter()
        .position(|&b| b == b'>' || b == b'"' || b == b'\'')
        .map(|i| at + i)
}

/// Finds the byte sequence `pattern` in `hay[from..]`.
pub fn find_seq(hay: &[u8], from: usize, pattern: &[u8]) -> Option<usize> {
    let first = *pattern.first()?;
    let mut at = from;
    while let Some(hit) = find_byte(hay, at, first) {
        if tail(hay, hit).starts_with(pattern) {
            return Some(hit);
        }
        at = hit + 1;
    }
    None
}

/// The token at `pos` in `buf`. `eof` says no input follows `buf`.
pub fn scan(buf: &[u8], pos: usize, eof: bool) -> Scan {
    let mut pos = pos.min(buf.len());
    loop {
        let rest = tail(buf, pos);
        let Some(&first) = rest.first() else {
            return if eof {
                Scan::Token(Token {
                    kind: Kind::Eof,
                    a: pos,
                    b: pos,
                    c: pos,
                    next: pos,
                })
            } else {
                Scan::More(pos)
            };
        };
        if first != b'<' {
            // Text up to the next tag, or to the end of the input.
            let end = match find_byte(buf, pos, b'<') {
                Some(lt) => lt,
                None if eof => buf.len(),
                None => return Scan::More(pos),
            };
            return Scan::Token(Token {
                kind: Kind::Text,
                a: pos,
                b: end,
                c: end,
                next: end,
            });
        }
        // A markup construct; nine bytes classify it.
        if rest.len() < 9 && !eof {
            return Scan::More(pos);
        }
        // `<!` first: an ordinary tag, which is almost every construct, then never meets
        // the longer comparisons.
        let (skip, close, detail): (usize, &[u8], u32) = if rest.starts_with(b"<!") {
            if rest.starts_with(b"<!--") {
                (4, b"-->", UNTERMINATED_COMMENT)
            } else if rest.starts_with(b"<![CDATA[") {
                (9, b"]]>", UNTERMINATED_CDATA)
            } else {
                (2, b">", UNTERMINATED_DECLARATION)
            }
        } else if rest.starts_with(b"<?") {
            (2, b"?>", UNTERMINATED_INSTRUCTION)
        } else {
            return tag(buf, pos, eof);
        };
        let Some(found) = find_seq(buf, pos + skip, close) else {
            return if eof {
                Scan::Malformed(detail)
            } else {
                Scan::More(pos)
            };
        };
        let next = found + close.len();
        if detail == UNTERMINATED_CDATA {
            return Scan::Token(Token {
                kind: Kind::CData,
                a: pos + 9,
                b: found,
                c: found,
                next,
            });
        }
        pos = next;
    }
}

/// An ordinary tag at `pos`: finds the `>` outside quotes and splits name from attributes.
fn tag(buf: &[u8], pos: usize, eof: bool) -> Scan {
    // From one `>` or quote to the next: a quote is jumped to the one that closes it.
    let mut at = pos + 1;
    let gt = loop {
        let Some(found) = find_tag_byte(buf, at) else {
            break None;
        };
        match buf.get(found) {
            Some(&b'>') => break Some(found),
            Some(&quote) => match find_byte(buf, found + 1, quote) {
                Some(close) => at = close + 1,
                None => break None,
            },
            None => break None,
        }
    };
    let Some(gt) = gt else {
        return if eof {
            Scan::Malformed(UNTERMINATED_TAG)
        } else {
            Scan::More(pos)
        };
    };
    let (mut from, mut to) = trimmed(buf, pos + 1, gt);
    if buf.get(from) == Some(&b'/') && from < to {
        let (a, b) = trimmed(buf, from + 1, to);
        return Scan::Token(Token {
            kind: Kind::End,
            a,
            b,
            c: b,
            next: gt + 1,
        });
    }
    let mut kind = Kind::Start;
    if to > from && buf.get(to - 1) == Some(&b'/') {
        kind = Kind::Empty;
        (from, to) = trimmed(buf, from, to - 1);
    }
    let name_end = slice(buf, from, to)
        .iter()
        .position(|b| b.is_ascii_whitespace())
        .map_or(to, |at| from + at);
    Scan::Token(Token {
        kind,
        a: from,
        b: name_end,
        c: to,
        next: gt + 1,
    })
}

/// `from..to` with ASCII whitespace taken off both ends.
fn trimmed(buf: &[u8], from: usize, to: usize) -> (usize, usize) {
    let bytes = slice(buf, from, to);
    let lead = bytes.len() - bytes.trim_ascii_start().len();
    let kept = bytes.trim_ascii().len();
    (from + lead, from + lead + kept)
}

/// The part of a qualified name after the last `:`.
pub fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(colon) => tail(name, colon + 1),
        None => name,
    }
}

/// A start or empty tag: its qualified name and its raw attribute bytes.
#[derive(Clone, Copy, Debug)]
pub struct Tag<'a> {
    /// The name as written, prefix included.
    pub name: &'a [u8],
    attrs: &'a [u8],
}

impl<'a> Tag<'a> {
    /// The tag a [`Kind::Start`] or [`Kind::Empty`] token describes.
    pub fn of(buf: &'a [u8], token: &Token) -> Tag<'a> {
        Tag {
            name: slice(buf, token.a, token.b),
            attrs: slice(buf, token.b, token.c),
        }
    }

    /// The name without its prefix.
    pub fn local(&self) -> &'a [u8] {
        local_name(self.name)
    }

    /// The raw value of the first attribute whose local name is `local`.
    pub fn attr(&self, local: &[u8]) -> Option<&'a [u8]> {
        self.attrs()
            .find(|(name, _)| local_name(name) == local)
            .map(|(_, value)| value)
    }

    /// The raw value of the attribute with exactly this qualified name.
    pub fn attr_qualified(&self, name: &[u8]) -> Option<&'a [u8]> {
        self.attrs()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value)
    }

    /// Every `(qualified name, raw value)` pair.
    pub fn attrs(&self) -> Attrs<'a> {
        Attrs { rest: self.attrs }
    }
}

/// The attribute iterator. Lenient: stops at the first thing that is not `name="value"`.
pub struct Attrs<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Attrs<'a> {
    type Item = (&'a [u8], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest.trim_ascii_start();
        let name_end = rest
            .iter()
            .position(|&b| b == b'=' || b.is_ascii_whitespace())?;
        let (name, after_name) = rest.split_at_checked(name_end)?;
        let after_eq = after_name
            .trim_ascii_start()
            .strip_prefix(b"=")?
            .trim_ascii_start();
        let (&quote, body) = after_eq.split_first()?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_end = body.iter().position(|&b| b == quote)?;
        let (value, after) = body.split_at_checked(value_end)?;
        self.rest = tail(after, 1);
        Some((name, value))
    }
}

/// Decodes XML entities from `raw` into `out`, which holds at least `raw.len()` bytes — a
/// decoded entity is never longer than its reference. Unknown entities are kept literally.
/// Returns how many bytes were written (as many as fit, should `out` be shorter).
pub fn unescape_into(raw: &[u8], out: &mut [u8]) -> usize {
    let mut written = 0usize;
    let mut at = 0usize;
    let mut put = |bytes: &[u8], written: &mut usize| {
        if let Some(room) = out.get_mut(*written..*written + bytes.len()) {
            room.copy_from_slice(bytes);
            *written += bytes.len();
        }
    };
    while at < raw.len() {
        let amp = find_byte(raw, at, b'&').unwrap_or(raw.len());
        put(slice(raw, at, amp), &mut written);
        at = amp;
        if at >= raw.len() {
            break;
        }
        let Some(semi) = find_byte(raw, at, b';') else {
            put(tail(raw, at), &mut written);
            break;
        };
        let entity = slice(raw, at + 1, semi);
        let decoded: Option<char> = match entity {
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"amp" => Some('&'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ if entity.first() == Some(&b'#') => match entity.get(1) {
                Some(b'x') | Some(b'X') => code_point(tail(entity, 2), 16),
                _ => code_point(tail(entity, 1), 10),
            },
            _ => None,
        };
        match decoded {
            Some(ch) => {
                let mut utf8 = [0u8; 4];
                put(ch.encode_utf8(&mut utf8).as_bytes(), &mut written);
                at = semi + 1;
            }
            None => {
                put(b"&", &mut written);
                at += 1;
            }
        }
    }
    written
}

/// A character reference's digits as the character, by the rules `u32::from_str_radix`
/// applies: one optional `+`, then digits of the radix, the value within `u32`.
fn code_point(digits: &[u8], radix: u32) -> Option<char> {
    let digits = digits.strip_prefix(b"+").unwrap_or(digits);
    if digits.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &b in digits {
        let digit = (b as char).to_digit(radix)?;
        value = value * u64::from(radix) + u64::from(digit);
        if value > u64::from(u32::MAX) {
            return None;
        }
    }
    char::from_u32(value as u32)
}

/// An unsigned decimal as the workbook parts write counts and indices: ASCII-trimmed, one
/// to ten digits, no sign, within `u32`.
pub fn parse_u32(bytes: &[u8]) -> Option<u32> {
    let bytes = bytes.trim_ascii();
    if bytes.is_empty() || bytes.len() > 10 {
        return None;
    }
    let mut value = 0u64;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u64::from(b - b'0');
    }
    u32::try_from(value).ok()
}
