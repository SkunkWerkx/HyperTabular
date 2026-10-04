//! A small pull tokenizer over any `Read`: start/empty/end tags with lazy attributes,
//! text, CDATA; comments, processing instructions and doctypes are recognised and
//! skipped. Namespace prefixes are ignored by every consumer ([`Tag::local`]) — `<x:c>`
//! and `<c>` are the same cell. Entities are decoded on demand ([`unescape`]), so text
//! without `&` is never copied. The same tokenizer serves the tiny parts and the
//! hundred-megabyte ones; no DOM anywhere.
//!
//! Events borrow the tokenizer's buffer and are valid until the next [`Reader::next`].

use std::io::Read;

/// One event.
#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// `<name attrs…>`
    Start(Tag<'a>),
    /// `<name attrs…/>`
    Empty(Tag<'a>),
    /// `</name>`
    End(&'a [u8]),
    /// Character data between tags, raw (entities not yet decoded).
    Text(&'a [u8]),
    /// `<![CDATA[…]]>` content, literal.
    CData(&'a [u8]),
    /// The end of the input.
    Eof,
}

/// A start or empty tag: its qualified name and the raw attribute bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Tag<'a> {
    /// The name as written, prefix included.
    pub name: &'a [u8],
    attrs: &'a [u8],
}

/// The part of a qualified name after the last `:`.
pub fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(colon) => &name[colon + 1..],
        None => name,
    }
}

impl<'a> Tag<'a> {
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
        let rest = trim_start(self.rest);
        let name_end = rest
            .iter()
            .position(|&b| b == b'=' || b.is_ascii_whitespace())?;
        let name = &rest[..name_end];
        let after_name = trim_start(&rest[name_end..]);
        let after_eq = trim_start(after_name.strip_prefix(b"=")?);
        let quote = *after_eq.first()?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_end = after_eq[1..].iter().position(|&b| b == quote)?;
        let value = &after_eq[1..1 + value_end];
        self.rest = &after_eq[value_end + 2..];
        Some((name, value))
    }
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let n = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    &bytes[n..]
}

fn trim(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

/// Finds `needle` in `hay[from..]` eight bytes at a time.
pub fn find_byte(hay: &[u8], from: usize, needle: u8) -> Option<usize> {
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    let needle_word = u64::from(needle) * 0x0101_0101_0101_0101;
    let mut at = from;
    while at + 8 <= hay.len() {
        let word = u64::from_le_bytes(hay[at..at + 8].try_into().unwrap()) ^ needle_word;
        let zero = !((word & LOW7).wrapping_add(LOW7) | word | LOW7);
        if zero != 0 {
            return Some(at + (zero.trailing_zeros() / 8) as usize);
        }
        at += 8;
    }
    hay[at..].iter().position(|&b| b == needle).map(|i| at + i)
}

/// Finds the byte sequence `pattern` in `hay[from..]`.
pub fn find_seq(hay: &[u8], from: usize, pattern: &[u8]) -> Option<usize> {
    let mut at = from;
    while let Some(hit) = find_byte(hay, at, pattern[0]) {
        if hay[hit..].starts_with(pattern) {
            return Some(hit);
        }
        at = hit + 1;
    }
    None
}

/// Decodes XML entities. Returns `raw` untouched when it contains no `&`; otherwise
/// decodes into `scratch` (cleared first) and returns that. Unknown entities are kept
/// literally.
pub fn unescape<'a>(raw: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
    let Some(first) = find_byte(raw, 0, b'&') else {
        return raw;
    };
    scratch.clear();
    scratch.extend_from_slice(&raw[..first]);
    let mut at = first;
    while at < raw.len() {
        if raw[at] != b'&' {
            let next = find_byte(raw, at, b'&').unwrap_or(raw.len());
            scratch.extend_from_slice(&raw[at..next]);
            at = next;
            continue;
        }
        let Some(semi) = find_byte(raw, at, b';') else {
            scratch.extend_from_slice(&raw[at..]);
            break;
        };
        let entity = &raw[at + 1..semi];
        let decoded: Option<char> = match entity {
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"amp" => Some('&'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ if entity.first() == Some(&b'#') => {
                let (digits, radix) = match entity.get(1) {
                    Some(b'x') | Some(b'X') => (&entity[2..], 16),
                    _ => (&entity[1..], 10),
                };
                str::from_utf8(digits)
                    .ok()
                    .and_then(|s| u32::from_str_radix(s, radix).ok())
                    .and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(ch) => {
                let mut utf8 = [0u8; 4];
                scratch.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
                at = semi + 1;
            }
            None => {
                scratch.push(b'&');
                at += 1;
            }
        }
    }
    scratch
}

/// The tokenizer. See the module doc.
pub struct Reader<R> {
    source: R,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
    eof: bool,
}

/// What a truncated or malformed token looks like to callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl<R: Read> Reader<R> {
    /// The initial buffer; it grows to hold the largest single token.
    pub const INITIAL_CAPACITY: usize = 64 * 1024;

    /// A tokenizer over `source`.
    pub fn new(source: R) -> Reader<R> {
        Reader::with_capacity(source, Reader::<R>::INITIAL_CAPACITY)
    }

    /// A tokenizer with a chosen initial buffer (tests use tiny ones).
    pub fn with_capacity(source: R, capacity: usize) -> Reader<R> {
        Reader {
            source,
            buf: vec![0; capacity.max(16)],
            pos: 0,
            end: 0,
            eof: false,
        }
    }

    /// Compacts and reads more; returns false at end of input.
    fn refill(&mut self) -> Result<bool, Malformed> {
        if self.eof {
            return Ok(false);
        }
        if self.pos > 0 {
            self.buf.copy_within(self.pos..self.end, 0);
            self.end -= self.pos;
            self.pos = 0;
        }
        if self.end == self.buf.len() {
            self.buf.resize(self.buf.len() * 2, 0);
        }
        loop {
            match self.source.read(&mut self.buf[self.end..]) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.end += n;
                    return Ok(true);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Malformed("read failed")),
            }
        }
    }

    /// Finds `pattern` at or after `self.pos + skip`, refilling until it is in the buffer.
    fn ensure_seq(&mut self, skip: usize, pattern: &[u8]) -> Result<Option<usize>, Malformed> {
        let mut from = self.pos + skip;
        loop {
            if let Some(hit) = find_seq(&self.buf[..self.end], from.min(self.end), pattern) {
                return Ok(Some(hit));
            }
            // Keep a pattern's worth of already-scanned bytes so a split match is found.
            let scanned = self
                .end
                .saturating_sub(pattern.len() - 1)
                .max(self.pos + skip);
            let before = self.pos;
            if !self.refill()? {
                return Ok(None);
            }
            from = scanned - before;
        }
    }

    /// The next event. (Not an `Iterator`: events borrow the reader.)
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Event<'_>, Malformed> {
        loop {
            if self.pos == self.end && !self.refill()? {
                return Ok(Event::Eof);
            }
            if self.buf[self.pos] != b'<' {
                // Text up to the next tag.
                let start = self.pos;
                let lt = loop {
                    if let Some(lt) = find_byte(&self.buf[..self.end], self.pos, b'<') {
                        break lt;
                    }
                    let scanned = self.end - self.pos;
                    if !self.refill()? {
                        break self.end;
                    }
                    // pos is now 0; skip what was already scanned.
                    if let Some(lt) = find_byte(&self.buf[..self.end], scanned, b'<') {
                        break lt;
                    }
                    continue;
                };
                let text_start = if self.pos == start { start } else { 0 };
                self.pos = lt;
                return Ok(Event::Text(&self.buf[text_start..lt]));
            }
            // A markup construct; make sure enough of it is buffered to classify.
            while self.end - self.pos < 9 && self.refill()? {}
            let head = &self.buf[self.pos..self.end];
            if head.starts_with(b"<!--") {
                let Some(close) = self.ensure_seq(4, b"-->")? else {
                    return Err(Malformed("unterminated comment"));
                };
                self.pos = close + 3;
                continue;
            }
            if head.starts_with(b"<![CDATA[") {
                let Some(close) = self.ensure_seq(9, b"]]>")? else {
                    return Err(Malformed("unterminated CDATA section"));
                };
                let start = self.pos + 9;
                self.pos = close + 3;
                return Ok(Event::CData(&self.buf[start..close]));
            }
            if head.starts_with(b"<?") {
                let Some(close) = self.ensure_seq(2, b"?>")? else {
                    return Err(Malformed("unterminated processing instruction"));
                };
                self.pos = close + 2;
                continue;
            }
            if head.starts_with(b"<!") {
                let Some(close) = self.ensure_seq(2, b">")? else {
                    return Err(Malformed("unterminated declaration"));
                };
                self.pos = close + 1;
                continue;
            }
            // An ordinary tag: find the `>` outside quotes.
            let gt = self.find_tag_end()?;
            let start = self.pos + 1;
            self.pos = gt + 1;
            let body = trim(&self.buf[start..gt]);
            if let Some(name) = body.strip_prefix(b"/") {
                return Ok(Event::End(trim(name)));
            }
            let (body, empty) = match body.strip_suffix(b"/") {
                Some(inner) => (trim(inner), true),
                None => (body, false),
            };
            let name_end = body
                .iter()
                .position(|b| b.is_ascii_whitespace())
                .unwrap_or(body.len());
            let tag = Tag {
                name: &body[..name_end],
                attrs: &body[name_end..],
            };
            return Ok(if empty {
                Event::Empty(tag)
            } else {
                Event::Start(tag)
            });
        }
    }

    /// The index of the `>` that closes the tag at `self.pos`, quote-aware, refilling as
    /// needed.
    fn find_tag_end(&mut self) -> Result<usize, Malformed> {
        let mut scanned = 1usize;
        loop {
            let mut i = self.pos + scanned;
            let mut quote: u8 = 0;
            while i < self.end {
                let b = self.buf[i];
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    return Ok(i);
                }
                i += 1;
            }
            // Rescan from the tag start after a refill: a quote may have been open.
            let before = self.pos;
            if !self.refill()? {
                return Err(Malformed("unterminated tag"));
            }
            scanned = 1;
            let _ = before;
        }
    }

    /// Skips everything up to and including the end tag matching an already-consumed
    /// start tag (nesting-aware by depth, not by name).
    pub fn skip_subtree(&mut self) -> Result<(), Malformed> {
        let mut depth = 1usize;
        loop {
            match self.next()? {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Event::Eof => return Err(Malformed("unexpected end of input inside an element")),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn events(text: &[u8], capacity: usize) -> Vec<String> {
        let mut reader = Reader::with_capacity(Cursor::new(text), capacity);
        let mut out = Vec::new();
        loop {
            let event = reader.next().unwrap();
            let rendered = match &event {
                Event::Start(tag) => format!(
                    "start {} [{}]",
                    String::from_utf8_lossy(tag.name),
                    tag.attrs()
                        .map(|(k, v)| format!(
                            "{}={}",
                            String::from_utf8_lossy(k),
                            String::from_utf8_lossy(v)
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Event::Empty(tag) => format!(
                    "empty {} [{}]",
                    String::from_utf8_lossy(tag.name),
                    tag.attrs().count()
                ),
                Event::End(name) => format!("end {}", String::from_utf8_lossy(name)),
                Event::Text(text) => format!("text {:?}", String::from_utf8_lossy(text)),
                Event::CData(text) => format!("cdata {:?}", String::from_utf8_lossy(text)),
                Event::Eof => break,
            };
            out.push(rendered);
        }
        out
    }

    #[test]
    fn tokenizes_the_sheet_shapes_at_every_buffer_size() {
        let text = b"<?xml version=\"1.0\"?><!DOCTYPE x><x:c r=\"A1\" t='s'><v>12</v><!-- c --><is><t xml:space=\"preserve\"> a&amp;b </t></is><f/><![CDATA[<raw>]]></x:c>";
        let expected = vec![
            "start x:c [r=A1,t=s]",
            "start v []",
            "text \"12\"",
            "end v",
            "start is []",
            "start t [xml:space=preserve]",
            "text \" a&amp;b \"",
            "end t",
            "end is",
            "empty f [0]",
            "cdata \"<raw>\"",
            "end x:c",
        ];
        for capacity in [16, 17, 23, 64, 4096] {
            assert_eq!(events(text, capacity), expected, "capacity {capacity}");
        }
    }

    #[test]
    fn attributes_names_and_entities() {
        let mut reader = Reader::new(Cursor::new(
            &b"<a:b x:y=\"1\" z='q>r' w=\"&lt;&#65;&#x42;&bogus;\"/>"[..],
        ));
        let Event::Empty(tag) = reader.next().unwrap() else {
            panic!()
        };
        assert_eq!(tag.local(), b"b");
        assert_eq!(tag.attr(b"y"), Some(&b"1"[..]));
        assert_eq!(tag.attr_qualified(b"x:y"), Some(&b"1"[..]));
        assert_eq!(tag.attr(b"z"), Some(&b"q>r"[..]));
        let mut scratch = Vec::new();
        assert_eq!(
            unescape(tag.attr(b"w").unwrap(), &mut scratch),
            b"<AB&bogus;"
        );
        assert_eq!(unescape(b"plain", &mut scratch), b"plain");
        assert_eq!(local_name(b"ns:x:name"), b"name");
        assert_eq!(find_byte(b"0123456789abcdef<", 0, b'<'), Some(16));
        assert_eq!(find_seq(b"aa--->", 0, b"-->"), Some(3));
    }

    #[test]
    fn skip_subtree_and_errors() {
        let mut reader = Reader::new(Cursor::new(&b"<a><b><c/></b><d>x</d></a>tail"[..]));
        assert!(matches!(reader.next().unwrap(), Event::Start(_)));
        assert!(matches!(reader.next().unwrap(), Event::Start(tag) if tag.name == b"b"));
        reader.skip_subtree().unwrap();
        assert!(matches!(reader.next().unwrap(), Event::Start(tag) if tag.name == b"d"));
        let mut reader = Reader::new(Cursor::new(&b"<a x=\"unterminated"[..]));
        assert_eq!(reader.next().unwrap_err(), Malformed("unterminated tag"));
    }
}
