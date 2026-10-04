//! Unescaping a quoted cell into caller memory — the one copying path, paid only by cells whose
//! quote count says they need it (a cell whose only quotes are its outer pair is sliced,
//! never copied).
//!
//! The rule, applied to a cell whose first byte is `"`:
//! - the opening quote is dropped;
//! - `""` becomes `"`;
//! - a lone `"` that is the cell's last byte is the closing quote and is dropped;
//! - any other lone `"` is kept literally — it is malformed input, and delivering it
//!   verbatim lets the HyperCast door say `Malformed` at the exact byte instead of this
//!   layer silently repairing it.
//!
//! So `"a""b"` → `a"b`, `"a"b"` → `a"b`, `"abc` → `abc`, `"a"` → `a`.

/// Writes the unescaped content of `cell` (which starts with `"`) to the front of `out`
/// and returns how many bytes that was — never more than `cell.len() - 1`. Stops, and
/// returns what fit, if `out` is shorter than that.
pub fn unescape_into(cell: &[u8], out: &mut [u8]) -> usize {
    let body = cell.get(1..).unwrap_or_default();
    let mut written = 0;
    let mut rest = body;
    while let Some((&byte, after)) = rest.split_first() {
        let mut next = after;
        if byte == b'"' {
            match after.split_first() {
                // `""` is one literal quote.
                Some((b'"', beyond)) => next = beyond,
                // A lone quote that ends the cell is the closing quote.
                None => break,
                // Any other lone quote is kept: malformed input, delivered verbatim.
                Some(_) => {}
            }
        }
        let Some(slot) = out.get_mut(written) else {
            break;
        };
        *slot = byte;
        written += 1;
        rest = next;
    }
    written
}
