//! Unescaping a quoted cell, appended to a `Vec`. The rule and the allocation-free
//! implementation are the core's ([`crate::kernel::delimited::unescape`]).

use crate::kernel::delimited::unescape::unescape_into;

/// Appends the unescaped content of `cell` (which must start with `"`) to `out`.
pub fn unescape(cell: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + cell.len(), 0);
    let written = unescape_into(cell, &mut out[start..]);
    out.truncate(start + written);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cell: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        unescape(cell, &mut out);
        out
    }

    #[test]
    fn the_documented_rule() {
        assert_eq!(run(b"\"a\"\"b\""), b"a\"b");
        assert_eq!(run(b"\"a\"b\""), b"a\"b");
        assert_eq!(run(b"\"abc"), b"abc");
        assert_eq!(run(b"\"a\""), b"a");
        assert_eq!(run(b"\"\""), b"");
        assert_eq!(run(b"\""), b"");
        assert_eq!(run(b"\"\"\"\""), b"\"");
        assert_eq!(run(b"\"x\"\"\"\"y\""), b"x\"\"y");
        assert_eq!(run(b"\"line1\nline2\""), b"line1\nline2");
    }
}
