//! Number-format kinds: what a cell format says a numeric cell *means* — a plain number,
//! a date/time serial, an elapsed-time serial, or text. Built-in ids per ISO 29500
//! §18.8.30 (plus the locale ranges every reader honours), custom codes by the scan
//! Sylvan, calamine, xlrd and POI agree on (see `docs/prior-art.md`).

use super::xml::tail;

/// A plain number.
pub const NUMBER: u8 = 0;
/// A date and/or time of day: the serial is a wall clock.
pub const DATE_TIME: u8 = 1;
/// An elapsed duration (`[h]:mm:ss` and friends): the serial is a span.
pub const ELAPSED: u8 = 2;
/// The `@` format: still a number, but meant to display as text.
pub const TEXT: u8 = 3;

/// The kind of a built-in `numFmtId`.
pub fn builtin(id: u32) -> u8 {
    match id {
        14..=22 | 27..=36 | 45 | 47 | 50..=58 | 71..=81 => DATE_TIME,
        46 => ELAPSED,
        49 => TEXT,
        _ => NUMBER,
    }
}

/// The kind of a custom `formatCode`. Only the first `;` section counts (positive
/// numbers); `"…"` literals and `\`/`_`/`*` escapes are skipped; `[h]`/`[m]`/`[s]` is
/// elapsed; any other `[…]` (colours, conditions, `[$-409]` locales) is ignored; `AM/PM`
/// is skipped; then any of `y m d h s` means date/time.
pub fn classify(code: &[u8]) -> u8 {
    let mut in_quote = false;
    let mut temporal = false;
    let mut elapsed = false;
    let mut i = 0usize;
    let mut section_end = code.len();
    while let Some(&c) = code.get(i) {
        if in_quote {
            in_quote = c != b'"';
            i += 1;
            continue;
        }
        match c {
            b'"' => in_quote = true,
            b';' => {
                section_end = i;
                break;
            }
            b'\\' | b'_' | b'*' => i += 1,
            b'[' => {
                let rest = tail(code, i);
                let close = rest.iter().position(|&b| b == b']').unwrap_or(rest.len());
                let content = rest.get(1..close).unwrap_or_default();
                if !content.is_empty()
                    && content
                        .iter()
                        .all(|b| matches!(b.to_ascii_lowercase(), b'h' | b'm' | b's'))
                {
                    elapsed = true;
                }
                i += close;
            }
            _ => {
                let rest = tail(code, i);
                if rest
                    .get(..5)
                    .is_some_and(|head| head.eq_ignore_ascii_case(b"am/pm"))
                {
                    i += 5;
                    continue;
                }
                if rest
                    .get(..3)
                    .is_some_and(|head| head.eq_ignore_ascii_case(b"a/p"))
                {
                    i += 3;
                    continue;
                }
                if matches!(c.to_ascii_lowercase(), b'y' | b'm' | b'd' | b'h' | b's') {
                    temporal = true;
                }
            }
        }
        i += 1;
    }
    if elapsed {
        ELAPSED
    } else if temporal {
        DATE_TIME
    } else if code.get(..section_end).unwrap_or_default().trim_ascii() == b"@" {
        TEXT
    } else {
        NUMBER
    }
}
