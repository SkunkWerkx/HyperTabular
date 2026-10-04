//! Numbers as a workbook writes them, and every typed cell as the core says it back.
//!
//! Reading is HyperCast's: the grammar a workbook part uses for a double is the plain one
//! — sign, digits, a point, an exponent — and HyperCast's `f64` door with nothing but the
//! exponent allowed reads exactly that.
//!
//! Saying a value back is what the text door returns for a typed cell and what a faulted
//! typed cell records as its text. Each rendering is one a HyperCast door reads again:
//! integral numbers as plain digits, other numbers as the shortest decimal that names the
//! same double, wall clocks and times in the ISO profiles, spans in ISO 8601. The shortest
//! decimal is worked out exactly, in fixed-size integers on the stack (Steele and White's
//! algorithm as `core` carries it for its own fallback), because `core`'s formatting
//! machinery cannot be put under the proof this code is held to.

use core::cmp::Ordering;
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

const SIGN: u64 = 1 << 63;

/// `value` without its sign.
pub fn magnitude(value: f64) -> f64 {
    f64::from_bits(value.to_bits() & !SIGN)
}

/// True for a finite double with no fractional part.
pub fn is_integral(value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    // Every double from 2^52 up is an integer; below it the round trip through i64 is exact.
    magnitude(value) >= 4_503_599_627_370_496.0 || (value as i64) as f64 == value
}

/// A non-negative double below 2^63 rounded to the nearest integer, halves away from zero.
pub fn round_unsigned(value: f64) -> u64 {
    let whole = value as u64;
    if value - whole as f64 >= 0.5 {
        whole + 1
    } else {
        whole
    }
}

/// An integral double below 2^127 in magnitude as the integer it is, read out of its
/// bits: a conversion the compiler would otherwise hand to a runtime routine
/// (`__fixdfti`) that the static library would then have to bring along.
pub fn integral(value: f64) -> i128 {
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as u32;
    // Below one there is only zero; from 2^127 up (and for NaN and ∞) the caller's own
    // range check has already said no.
    if !(1_023..1_023 + 127).contains(&biased) {
        return 0;
    }
    let mantissa = i128::from(bits & ((1 << 52) - 1) | 1 << 52);
    let size = if biased >= 1_075 {
        mantissa << (biased - 1_075)
    } else {
        mantissa >> (1_075 - biased)
    };
    if bits & SIGN != 0 { -size } else { size }
}

/// A double below 2^127 in magnitude rounded to the nearest integer, halves away from
/// zero.
pub fn round_signed(value: f64) -> i128 {
    // Every double from 2^52 up is an integer already.
    if value.is_nan() || magnitude(value) >= 4_503_599_627_370_496.0 {
        return integral(value);
    }
    let whole = value as i64;
    let rest = value - whole as f64;
    i128::from(if rest >= 0.5 {
        whole + 1
    } else if rest <= -0.5 {
        whole - 1
    } else {
        whole
    })
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
        let size = magnitude(value);
        if is_integral(value) && size < 1e16 {
            let whole = value as i64;
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
        if value.to_bits() & SIGN != 0 {
            self.push(b'-');
        }
        if value.is_infinite() {
            self.extend(b"inf");
            return;
        }
        let mut digits = [b'0'; 20];
        let (count, exponent) = shortest(size, &mut digits);
        let digits = digits.get(..count).unwrap_or_default();
        let exponent = i32::from(exponent);
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

/// An unsigned integer of 1280 bits in 32-bit digits, least significant first — room for
/// the largest power of two and of ten a double's shortest decimal is worked out against.
#[derive(Clone, Copy)]
struct Big {
    size: usize,
    base: [u32; Big::DIGITS],
}

impl Big {
    const DIGITS: usize = 40;

    fn from_u64(value: u64) -> Big {
        let mut base = [0u32; Big::DIGITS];
        if let Some(pair) = base.first_chunk_mut::<2>() {
            *pair = [value as u32, (value >> 32) as u32];
        }
        let size = match value {
            0 => 0,
            v if v >> 32 == 0 => 1,
            _ => 2,
        };
        Big { size, base }
    }

    fn mul_small(&mut self, factor: u32) {
        let mut carry = 0u64;
        for digit in self.base.iter_mut().take(self.size) {
            let wide = u64::from(*digit) * u64::from(factor) + carry;
            *digit = wide as u32;
            carry = wide >> 32;
        }
        if carry > 0
            && let Some(top) = self.base.get_mut(self.size)
        {
            *top = carry as u32;
            self.size += 1;
        }
    }

    fn mul_pow2(&mut self, mut bits: u32) {
        while bits >= 31 {
            self.mul_small(1 << 31);
            bits -= 31;
        }
        self.mul_small(1 << bits);
    }

    fn mul_pow10(&mut self, mut power: u32) {
        while power >= 9 {
            self.mul_small(1_000_000_000);
            power -= 9;
        }
        self.mul_small(10u32.pow(power));
    }

    fn add(&mut self, other: &Big) {
        let size = self.size.max(other.size);
        let mut carry = false;
        for (digit, &more) in self.base.iter_mut().zip(other.base.iter()).take(size) {
            let (sum, first) = digit.overflowing_add(more);
            let (sum, second) = sum.overflowing_add(u32::from(carry));
            *digit = sum;
            carry = first || second;
        }
        self.size = size;
        if carry && let Some(top) = self.base.get_mut(size) {
            *top = 1;
            self.size += 1;
        }
    }

    /// `self -= other`, for `other` no greater than `self`.
    fn sub(&mut self, other: &Big) {
        let mut borrow = false;
        for (digit, &less) in self.base.iter_mut().zip(other.base.iter()).take(self.size) {
            let (rest, first) = digit.overflowing_sub(less);
            let (rest, second) = rest.overflowing_sub(u32::from(borrow));
            *digit = rest;
            borrow = first || second;
        }
        while self.size > 0 && self.base.get(self.size - 1) == Some(&0) {
            self.size -= 1;
        }
    }

    fn cmp(&self, other: &Big) -> Ordering {
        let size = self.size.max(other.size);
        for (mine, theirs) in self.base.iter().zip(other.base.iter()).take(size).rev() {
            match mine.cmp(theirs) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

/// The shortest run of decimal digits that names the positive finite double `value` and
/// no other, nearest to it where several are as short. Returns how many digits were
/// written and the exponent `k` in `0.d₁d₂… × 10ᵏ`.
fn shortest(value: f64, digits: &mut [u8; 20]) -> (usize, i16) {
    let bits = value.to_bits();
    let fraction = bits & ((1 << 52) - 1);
    let biased = ((bits >> 52) & 0x7FF) as i16;
    // The double is `mant × 2^exp`, and its neighbours are `minus` below and `plus` above
    // in the same unit. A power of two has a nearer neighbour below than above.
    let (mant, plus, exp) = if biased == 0 {
        (fraction << 1, 1u64, -1075i16)
    } else if fraction == 0 {
        (1u64 << 54, 2, biased - 1077)
    } else {
        ((fraction | 1 << 52) << 1, 1, biased - 1076)
    };
    // (`core` reads a subnormal's mantissa already doubled, and so always even.)
    let even = biased == 0 || (fraction & 1) == 0;
    // With an even mantissa the bounds themselves round to this double.
    let within = |order: Ordering| {
        if even {
            order != Ordering::Greater
        } else {
            order == Ordering::Less
        }
    };

    let width = 64 - i64::from((mant + plus - 1).leading_zeros());
    let mut k = (((width + i64::from(exp)) * 1_292_913_986) >> 32) as i16;

    let mut mant = Big::from_u64(mant);
    let mut minus = Big::from_u64(1);
    let mut plus = Big::from_u64(plus);
    let mut scale = Big::from_u64(1);
    if exp < 0 {
        scale.mul_pow2(u32::from(exp.unsigned_abs()));
    } else {
        mant.mul_pow2(u32::from(exp.unsigned_abs()));
        minus.mul_pow2(u32::from(exp.unsigned_abs()));
        plus.mul_pow2(u32::from(exp.unsigned_abs()));
    }
    if k >= 0 {
        scale.mul_pow10(u32::from(k.unsigned_abs()));
    } else {
        mant.mul_pow10(u32::from(k.unsigned_abs()));
        minus.mul_pow10(u32::from(k.unsigned_abs()));
        plus.mul_pow10(u32::from(k.unsigned_abs()));
    }

    // The estimate of `k` is at most one short.
    let mut high = mant;
    high.add(&plus);
    if within(scale.cmp(&high)) {
        k += 1;
    } else {
        mant.mul_small(10);
        minus.mul_small(10);
        plus.mul_small(10);
    }

    let mut scale2 = scale;
    scale2.mul_small(2);
    let mut scale4 = scale;
    scale4.mul_small(4);
    let mut scale8 = scale;
    scale8.mul_small(8);

    let mut count = 0usize;
    let (mut down, mut up) = (false, false);
    while count < 18 {
        // One digit: `mant / scale`, which is below ten.
        let mut digit = 0u8;
        for (step, weight) in [(&scale8, 8u8), (&scale4, 4), (&scale2, 2), (&scale, 1)] {
            if mant.cmp(step) != Ordering::Less {
                mant.sub(step);
                digit += weight;
            }
        }
        if let Some(slot) = digits.get_mut(count) {
            *slot = b'0' + digit;
        }
        count += 1;
        // Stop as soon as what is left can no longer be told from a neighbour.
        down = within(mant.cmp(&minus));
        let mut high = mant;
        high.add(&plus);
        up = within(scale.cmp(&high));
        if down || up {
            break;
        }
        mant.mul_small(10);
        minus.mul_small(10);
        plus.mul_small(10);
    }

    // Stopped between two candidates: the upper one, unless the lower is nearer.
    if up
        && (!down || {
            mant.mul_small(2);
            mant.cmp(&scale) != Ordering::Less
        })
    {
        let written = digits.get_mut(..count).unwrap_or_default();
        match written.iter().rposition(|&d| d != b'9') {
            Some(at) => {
                for (index, digit) in written.iter_mut().enumerate().skip(at) {
                    *digit = if index == at { *digit + 1 } else { b'0' };
                }
            }
            None => {
                // All nines: one more digit, and one more power of ten.
                for (index, digit) in written.iter_mut().enumerate() {
                    *digit = if index == 0 { b'1' } else { b'0' };
                }
                if let Some(slot) = digits.get_mut(count) {
                    *slot = b'0';
                }
                count += 1;
                k += 1;
            }
        }
    }
    (count, k)
}
