//! Decoding a declared encoding to UTF-8, ahead of a delimited fill: Windows-1252, Latin-1 or
//! UTF-16LE. Stateless: everything a call needs is in its arguments, so a code unit a chunk cut
//! in two is simply left unconsumed, for the caller to hand over again in front of the next
//! chunk, until `last` says nothing follows it.
//!
//! The output is always whole characters of valid UTF-8, so that the fill can start on any of
//! it. Nothing is ever replaced: a unit that has no character is a failure, for the caller to
//! raise after the rows ahead of it.

use crate::kernel::abi::{ERR_CONTRACT, OK, RawTranscoded};

/// What 0x80 to 0x9F are in Windows-1252, WHATWG's table: the five bytes the code page leaves
/// undefined are their C1 controls, as they are in Latin-1.
const WINDOWS_1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decodes as much of `input` as `out` holds whole characters of, and says how far it got in
/// `progress`: `input[consumed..]` is what to hand over next, in front of whatever follows, and
/// `out[..written]` is valid UTF-8. A call stops short for one of three reasons:
///
/// - `out` has no room for the next character. No character takes more than 4 bytes, so with 4
///   bytes of room a call that wrote nothing and didn't fail wants more input, not more room.
/// - Without `last`, a code unit isn't all there: half a unit, or a high surrogate whose low
///   surrogate isn't. With `last`, that's a failure.
/// - A failure: `failed` is `1`, and `failure_at` is the offset in this call's `input` of the
///   unit that has no character, which is also `consumed`. That needs no room in `out`, and the
///   same input fails the same way if it's asked again.
///
/// A byte-order mark is a character like any other: `FF FE` is U+FEFF wherever it falls, and it's
/// for whoever reads the output to say what a U+FEFF at byte 0 is. [`ERR_CONTRACT`] for an
/// `encoding` that isn't one, and nothing is written then.
pub fn transcode(
    encoding: u32,
    input: &[u8],
    last: bool,
    out: &mut [u8],
    progress: &mut RawTranscoded,
) -> i32 {
    *progress = match encoding {
        RawTranscoded::WINDOWS_1252 => single_byte(input, out, |byte| match byte {
            0x80..=0x9F => u32::from(WINDOWS_1252_HIGH[usize::from(byte & 0x1F)]),
            other => u32::from(other),
        }),
        RawTranscoded::LATIN_1 => single_byte(input, out, u32::from),
        RawTranscoded::UTF_16LE => utf16le(input, last, out),
        _ => return ERR_CONTRACT,
    };
    OK
}

#[inline(always)]
fn single_byte(input: &[u8], out: &mut [u8], decode: impl Fn(u8) -> u32) -> RawTranscoded {
    let mut consumed = 0;
    let mut written = 0;
    for &byte in input {
        if !put(out, &mut written, decode(byte)) {
            break;
        }
        consumed += 1;
    }
    RawTranscoded {
        consumed,
        written: written as u64,
        ..RawTranscoded::default()
    }
}

#[inline(always)]
fn utf16le(input: &[u8], last: bool, out: &mut [u8]) -> RawTranscoded {
    let mut rest = input;
    let mut written = 0;
    let failed = loop {
        let [a, b, after @ ..] = rest else {
            // Nothing, or half a unit.
            break last && !rest.is_empty();
        };
        let unit = u16::from_le_bytes([*a, *b]);
        let (code, next) = match unit {
            0xD800..=0xDBFF => {
                let [c, d, beyond @ ..] = after else {
                    break last;
                };
                let low = u16::from_le_bytes([*c, *d]);
                if !matches!(low, 0xDC00..=0xDFFF) {
                    break true;
                }
                let high = u32::from(unit - 0xD800);
                (0x10000 + (high << 10 | u32::from(low - 0xDC00)), beyond)
            }
            0xDC00..=0xDFFF => break true,
            _ => (u32::from(unit), after),
        };
        if !put(out, &mut written, code) {
            break false;
        }
        rest = next;
    };
    let consumed = (input.len() - rest.len()) as u64;
    RawTranscoded {
        consumed,
        written: written as u64,
        failure_at: if failed { consumed } else { 0 },
        failed: u32::from(failed),
        reserved: 0,
    }
}

/// Writes `code` at `written`, or nothing at all if it doesn't fit. `code` is a character.
#[inline(always)]
fn put(out: &mut [u8], written: &mut usize, code: u32) -> bool {
    let tail = |shift: u32| 0x80 | (code >> shift & 0x3F) as u8;
    let size = match out.get_mut(*written..) {
        Some([a, ..]) if code < 0x80 => {
            *a = code as u8;
            1
        }
        Some([a, b, ..]) if code < 0x800 => {
            *a = 0xC0 | (code >> 6) as u8;
            *b = tail(0);
            2
        }
        Some([a, b, c, ..]) if code < 0x10000 => {
            *a = 0xE0 | (code >> 12) as u8;
            *b = tail(6);
            *c = tail(0);
            3
        }
        Some([a, b, c, d, ..]) if code >= 0x10000 => {
            *a = 0xF0 | (code >> 18) as u8;
            *b = tail(12);
            *c = tail(6);
            *d = tail(0);
            4
        }
        _ => return false,
    };
    *written += size;
    true
}
