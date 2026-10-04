//! Number-format kinds: what a `cellXfs` style says a numeric cell *means* — a plain
//! number, a date/time serial, an elapsed-time serial, or text. Built-in ids per ISO
//! 29500 §18.8.30 (plus the locale ranges every reader honours), custom codes by the
//! scan Sylvan, calamine, xlrd, and POI agree on (see `docs/prior-art.md`).

/// What a number format makes of a numeric cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumberKind {
    /// A plain number.
    #[default]
    Number,
    /// A date and/or time of day (serial → wall clock).
    DateTime,
    /// An elapsed duration (`[h]:mm:ss` and friends; serial → span).
    Elapsed,
    /// The `@` format: the number is still a number, but was meant to display as text.
    Text,
}

/// The kind of a built-in `numFmtId`.
pub fn builtin(id: u32) -> NumberKind {
    match id {
        14..=22 | 27..=36 | 45 | 47 | 50..=58 | 71..=81 => NumberKind::DateTime,
        46 => NumberKind::Elapsed,
        49 => NumberKind::Text,
        _ => NumberKind::Number,
    }
}

/// The kind of a custom `formatCode`. Only the first `;` section counts (positive
/// numbers); `"…"` literals and `\`/`_`/`*` escapes are skipped; `[h]`/`[m]`/`[s]` is
/// elapsed; any other `[…]` (colours, conditions, `[$-409]` locales) is ignored; `AM/PM`
/// is skipped; then any of `y m d h s` means date/time.
pub fn classify(code: &[u8]) -> NumberKind {
    let mut in_quote = false;
    let mut temporal = false;
    let mut elapsed = false;
    let mut i = 0;
    let mut section_end = code.len();
    while i < code.len() {
        let c = code[i];
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
                let close = code[i..]
                    .iter()
                    .position(|&b| b == b']')
                    .map_or(code.len(), |at| i + at);
                let content = &code[i + 1..close.min(code.len())];
                if !content.is_empty()
                    && content
                        .iter()
                        .all(|b| matches!(b.to_ascii_lowercase(), b'h' | b'm' | b's'))
                {
                    elapsed = true;
                }
                i = close;
            }
            _ => {
                let rest = &code[i..];
                if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"am/pm") {
                    i += 5;
                    continue;
                }
                if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"a/p") {
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
        NumberKind::Elapsed
    } else if temporal {
        NumberKind::DateTime
    } else if code[..section_end].trim_ascii() == b"@" {
        NumberKind::Text
    } else {
        NumberKind::Number
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins() {
        assert_eq!(builtin(0), NumberKind::Number);
        assert_eq!(builtin(4), NumberKind::Number);
        assert_eq!(builtin(14), NumberKind::DateTime);
        assert_eq!(builtin(22), NumberKind::DateTime);
        assert_eq!(builtin(45), NumberKind::DateTime);
        assert_eq!(builtin(46), NumberKind::Elapsed);
        assert_eq!(builtin(47), NumberKind::DateTime);
        assert_eq!(builtin(49), NumberKind::Text);
        assert_eq!(builtin(164), NumberKind::Number);
    }

    #[test]
    fn custom_codes() {
        assert_eq!(classify(b"yyyy-mm-dd"), NumberKind::DateTime);
        assert_eq!(classify(b"m/d/yyyy h:mm"), NumberKind::DateTime);
        assert_eq!(classify(b"h:mm AM/PM"), NumberKind::DateTime);
        assert_eq!(classify(b"[h]:mm:ss"), NumberKind::Elapsed);
        assert_eq!(classify(b"[mm]:ss"), NumberKind::Elapsed);
        assert_eq!(classify(b"0.00"), NumberKind::Number);
        assert_eq!(classify(b"#,##0.00;[Red]-#,##0.00"), NumberKind::Number);
        assert_eq!(classify(b"General"), NumberKind::Number);
        assert_eq!(classify(b"0.00E+00"), NumberKind::Number);
        assert_eq!(classify(b"\"Total: \"0.00"), NumberKind::Number);
        assert_eq!(classify(b"\"Days: \"d"), NumberKind::DateTime);
        assert_eq!(classify(b"0\\d"), NumberKind::Number);
        assert_eq!(classify(b"[$-409]d-mmm-yy"), NumberKind::DateTime);
        assert_eq!(classify(b"[$USD] #,##0"), NumberKind::Number);
        assert_eq!(classify(b"@"), NumberKind::Text);
        assert_eq!(classify(b"0.0;yyyy"), NumberKind::Number);
        assert_eq!(classify(b"AM/PM"), NumberKind::Number);
    }
}
