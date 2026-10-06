//! Numbers as a workbook writes them, and every typed cell as the core says it back.
//!
//! Reading is HyperCast's: the grammar a workbook part uses for a double is the plain one
//! — sign, digits, a point, an exponent — and HyperCast's `f64` door with nothing but the
//! exponent allowed reads exactly that.
//!
//! Saying a value back is what the text door returns for a typed cell and what a faulted
//! typed cell records as its text. Each rendering is one a HyperCast door reads again:
//! integral numbers as plain digits, other numbers as the shortest decimal that names the
//! same double (HyperCast's `shortest_digits`, the number its decimal door reads), wall
//! clocks and times in the ISO profiles, spans in ISO 8601.

use hypercast::{CurrencySymbol, Date, Duration, NumFormat};

/// The one notation a workbook part writes a double in.
const PLAIN: NumFormat = NumFormat {
    decimal_sep: '.',
    group_sep: ',',
    flags: NumFormat::EXPONENT,
    currency: CurrencySymbol::NONE,
};

/// A finite double from the text of a numeric cell, or `None` if the text is not one.
pub fn parse_f64(bytes: &[u8]) -> Option<f64> {
    // The door trims; this grammar does not.
    let (first, last) = (*bytes.first()?, *bytes.last()?);
    if !(first.is_ascii_digit() || matches!(first, b'+' | b'-' | b'.'))
        || !(last.is_ascii_digit() || last == b'.')
    {
        return None;
    }
    hypercast::cast_f64(bytes, &PLAIN).ok()
}

/// A short run of text built on the stack.
pub struct Text {
    bytes: [u8; Text::CAPACITY],
    len: usize,
}

impl Text {
    /// More than the longest rendering: a span of days, hours, minutes and nanoseconds.
    const CAPACITY: usize = 64;

    /// Nothing yet.
    pub const fn new() -> Text {
        Text {
            bytes: [0; Text::CAPACITY],
            len: 0,
        }
    }

    /// What has been written.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    /// `value` in decimal, left-padded with zeros to `width` digits.
    fn padded(&mut self, value: u64, width: usize) {
        let mut digits = [b'0'; 20];
        let mut cursor = digits.len();
        let mut remaining = value;
        loop {
            cursor -= 1;
            if let Some(slot) = digits.get_mut(cursor) {
                *slot = b'0' + (remaining % 10) as u8;
            }
            remaining /= 10;
            if remaining == 0 || cursor == 0 {
                break;
            }
        }
        cursor = cursor.min(digits.len().saturating_sub(width));
        self.extend(digits.get(cursor..).unwrap_or_default());
    }

    /// `.fffffffff` without its trailing zeros, or nothing for no fraction at all.
    fn fraction(&mut self, nanos: u64) {
        if nanos == 0 {
            return;
        }
        self.push(b'.');
        self.padded(nanos, 9);
        while self.as_bytes().last() == Some(&b'0') {
            self.len -= 1;
        }
    }

    /// A number: integral values as integers, everything else as the shortest decimal
    /// that reads back as the same double, in exponent form outside `1e-4..1e16`.
    pub fn number(&mut self, value: f64) {
        let size = value.abs();
        if size < 1e16
            && let Ok(whole) = hypercast::i64_from_f64(value)
        {
            if whole < 0 {
                self.push(b'-');
            }
            self.padded(whole.unsigned_abs(), 1);
            return;
        }
        if value.is_nan() {
            self.extend(b"NaN");
            return;
        }
        if value.is_sign_negative() {
            self.push(b'-');
        }
        let Some(shortest) = hypercast::shortest_digits(value) else {
            // Zero has gone out as an integer above, so this is an infinity.
            self.extend(b"inf");
            return;
        };
        let digits = shortest.digits();
        let exponent = i32::from(shortest.exponent());
        if !(1e-4..1e16).contains(&size) {
            let (first, rest) = digits.split_first().unwrap_or((&b'0', &[]));
            self.push(*first);
            if !rest.is_empty() {
                self.push(b'.');
                self.extend(rest);
            }
            self.push(b'e');
            let shown = exponent - 1;
            if shown < 0 {
                self.push(b'-');
            }
            self.padded(u64::from(shown.unsigned_abs()), 1);
        } else if exponent <= 0 {
            self.extend(b"0.");
            for _ in 0..exponent.unsigned_abs() {
                self.push(b'0');
            }
            self.extend(digits);
        } else if let Some((whole, fraction)) = digits.split_at_checked(exponent as usize)
            && !fraction.is_empty()
        {
            self.extend(whole);
            self.push(b'.');
            self.extend(fraction);
        } else {
            self.extend(digits);
            for _ in digits.len()..exponent as usize {
                self.push(b'0');
            }
            self.extend(b".0");
        }
    }

    /// A workbook boolean.
    pub fn boolean(&mut self, value: bool) {
        self.extend(if value { b"true" } else { b"false" });
    }

    /// `yyyy-MM-ddTHH:mm:ss[.f…]`.
    pub fn wall(&mut self, date: Date, nanos: u64) {
        self.padded(u64::from(date.year), 4);
        self.push(b'-');
        self.padded(u64::from(date.month), 2);
        self.push(b'-');
        self.padded(u64::from(date.day), 2);
        self.push(b'T');
        self.clock(nanos);
    }

    /// `HH:mm:ss`, with a fraction only when there is one.
    pub fn clock(&mut self, nanos: u64) {
        let seconds = nanos / 1_000_000_000;
        self.padded(seconds / 3_600, 2);
        self.push(b':');
        self.padded(seconds % 3_600 / 60, 2);
        self.push(b':');
        self.padded(seconds % 60, 2);
        self.fraction(nanos % 1_000_000_000);
    }

    /// ISO 8601 with fixed components only — `-P1DT6H30M15.5S` — the profile HyperCast's
    /// duration door reads; zero components are left out, and no time at all is `PT0S`.
    pub fn span(&mut self, span: Duration) {
        if span.seconds < 0 || span.nanos < 0 {
            self.push(b'-');
        }
        let seconds = span.seconds.unsigned_abs();
        let nanos = u64::from(span.nanos.unsigned_abs());
        self.push(b'P');
        let days = seconds / 86_400;
        if days > 0 {
            self.padded(days, 1);
            self.push(b'D');
        }
        let hours = seconds % 86_400 / 3_600;
        let minutes = seconds % 3_600 / 60;
        let whole = seconds % 60;
        if hours == 0 && minutes == 0 && whole == 0 && nanos == 0 {
            if days == 0 {
                self.extend(b"T0S");
            }
            return;
        }
        self.push(b'T');
        if hours > 0 {
            self.padded(hours, 1);
            self.push(b'H');
        }
        if minutes > 0 {
            self.padded(minutes, 1);
            self.push(b'M');
        }
        if whole > 0 || nanos > 0 {
            self.padded(whole, 1);
            self.fraction(nanos);
            self.push(b'S');
        }
    }

    /// Bytes as they are (an error cell's spelling).
    pub fn literal(&mut self, bytes: &[u8]) {
        self.extend(bytes);
    }
}

impl Default for Text {
    fn default() -> Text {
        Text::new()
    }
}
