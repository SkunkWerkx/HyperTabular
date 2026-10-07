//! Number-format kinds: what a cell format says a numeric cell *means* — a plain number,
//! a date (with or without a time), a time of day, an elapsed-time serial, or text.
//! Built-in ids per ISO 29500 §18.8.30 (plus the locale ranges every reader honours),
//! custom codes by the scan Sylvan, calamine, xlrd and POI agree on (see
//! `docs/prior-art.md`), with the spec's month-versus-minutes rule on top.

use super::xml::tail;

/// A plain number.
pub const NUMBER: u8 = 0;
/// A date, with or without a time of day: the serial is a wall clock, and in the 1904
/// system a date at every serial from `0` (1904-01-01) up.
pub const DATE_TIME: u8 = 1;
/// An elapsed duration (`[h]:mm:ss` and friends): the serial is a span.
pub const ELAPSED: u8 = 2;
/// The `@` format: still a number, but meant to display as text.
pub const TEXT: u8 = 3;
/// A time of day with no date shown, or a built-in that is one in some locales and a date
/// in others: under one day the serial is a time of day, past it a wall clock — so no date
/// is read that the format may not show.
pub const TIME: u8 = 4;

/// The kind of a built-in `numFmtId`. ISO 29500 §18.8.30 lists the locale ids per UI
/// language, which the file does not record: 34, 35, 52, 53, 55 and 56 are times in
/// Chinese and dates in Japanese or Korean, so they are [`TIME`], which reads no date
/// either way. Thai 79 (`[ช]:นน:ทท`) is `[h]:mm:ss`.
pub fn builtin(id: u32) -> u8 {
    match id {
        14..=17 | 22 | 27..=31 | 36 | 50 | 51 | 54 | 57 | 58 | 71..=74 | 77 | 81 => DATE_TIME,
        18..=21 | 32..=35 | 45 | 47 | 52 | 53 | 55 | 56 | 75 | 76 | 78 | 80 => TIME,
        46 | 79 => ELAPSED,
        49 => TEXT,
        _ => NUMBER,
    }
}

/// The kind of a custom `formatCode`. Only the first `;` section counts (positive
/// numbers); `"…"` literals and `\`/`_`/`*` escapes are skipped; `[h]`/`[m]`/`[s]` is
/// elapsed; any other `[…]` (colours, conditions, `[$-409]` locales) is ignored; `AM/PM`
/// is skipped; then `y` or `d`, or an `m` that is a month, means a date, and `h`, `s` or an
/// `m` that is minutes alone means a time. An `m` run is minutes when the date/time code
/// before it is an `h` or the one after it an `s` (§18.8.31, "month versus minutes"), the
/// literals and separators between them not counting; otherwise it is the month.
pub fn classify(code: &[u8]) -> u8 {
    let mut in_quote = false;
    let mut temporal = false;
    let mut dated = false;
    let mut elapsed = false;
    // The last date/time code seen (`y m d h s`, lowercased), and whether an `m` run waits
    // on the next code to say if it was the month.
    let mut previous = 0u8;
    let mut month_pending = false;
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
                let code_letter = c.to_ascii_lowercase();
                if matches!(code_letter, b'y' | b'm' | b'd' | b'h' | b's') {
                    temporal = true;
                    // One code is the whole run of its letter: `mmm` is one month.
                    let run = rest
                        .iter()
                        .take_while(|b| b.to_ascii_lowercase() == code_letter);
                    i += run.count().saturating_sub(1);
                    if month_pending {
                        dated |= code_letter != b's';
                        month_pending = false;
                    }
                    match code_letter {
                        b'm' => month_pending = previous != b'h',
                        b'y' | b'd' => dated = true,
                        _ => {}
                    }
                    previous = code_letter;
                }
            }
        }
        i += 1;
    }
    if elapsed {
        ELAPSED
    } else if dated || month_pending {
        DATE_TIME
    } else if temporal {
        TIME
    } else if code.get(..section_end).unwrap_or_default().trim_ascii() == b"@" {
        TEXT
    } else {
        NUMBER
    }
}
