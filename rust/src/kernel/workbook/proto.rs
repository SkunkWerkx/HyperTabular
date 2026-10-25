//! Protocol Buffers' wire format, read in place: varints, and the fields of a message as
//! `(number, wire type, value)` triples over the bytes that hold them. Nothing is decoded
//! that is not asked for — a message is walked when a field of it is wanted, and a nested
//! message is a slice to walk in turn — and nothing is held but a position.
//!
//! The wire types read are the four a current writer emits: `0` varint, `1` 64-bit, `2`
//! length-delimited, `5` 32-bit. The deprecated group markers end a walk as malformed.

/// A varint field.
pub const VARINT: u8 = 0;
/// A fixed 64-bit field (a double, or `fixed64`).
pub const FIXED64: u8 = 1;
/// A length-delimited field: bytes, a string, a nested message, a packed array.
pub const BYTES: u8 = 2;
/// A fixed 32-bit field (a float, or `fixed32`).
pub const FIXED32: u8 = 5;

/// The varint at `pos`: its value and the position after it. `None` for one that runs
/// past the bytes or past ten bytes.
pub fn varint(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut at = pos;
    loop {
        let byte = *buf.get(at)?;
        at += 1;
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, at));
        }
        shift += 7;
    }
}

/// One field as the wire carries it.
#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    /// The field number.
    pub number: u32,
    /// A varint's value, or a fixed field's bits (the low 32 for a 32-bit one).
    pub bits: u64,
    /// A length-delimited field's bytes; empty for the other wire types.
    pub bytes: &'a [u8],
}

impl<'a> Field<'a> {
    /// A varint or fixed field as an integer.
    pub const fn u64(&self) -> u64 {
        self.bits
    }

    /// A `TSP.Reference` message's identifier: its field 1.
    pub fn reference(&self) -> Option<u64> {
        first(self.bytes, 1).map(|field| field.u64())
    }
}

/// The field at `pos`, and the position after it. `None` for a key that runs out, an
/// unknown or deprecated wire type, or a value cut short.
pub fn field(buf: &[u8], pos: usize) -> Option<(Field<'_>, usize)> {
    let (key, at) = varint(buf, pos)?;
    let wire = (key & 7) as u8;
    let number = u32::try_from(key >> 3).ok()?;
    if number == 0 {
        return None;
    }
    match wire {
        VARINT => {
            let (bits, next) = varint(buf, at)?;
            Some((
                Field {
                    number,
                    bits,
                    bytes: &[],
                },
                next,
            ))
        }
        FIXED64 => {
            let bytes = buf.get(at..at + 8)?.first_chunk::<8>()?;
            Some((
                Field {
                    number,
                    bits: u64::from_le_bytes(*bytes),
                    bytes: &[],
                },
                at + 8,
            ))
        }
        FIXED32 => {
            let bytes = buf.get(at..at + 4)?.first_chunk::<4>()?;
            Some((
                Field {
                    number,
                    bits: u64::from(u32::from_le_bytes(*bytes)),
                    bytes: &[],
                },
                at + 4,
            ))
        }
        BYTES => {
            let (len, start) = varint(buf, at)?;
            let len = usize::try_from(len).ok()?;
            let end = start.checked_add(len)?;
            let bytes = buf.get(start..end)?;
            Some((
                Field {
                    number,
                    bits: 0,
                    bytes,
                },
                end,
            ))
        }
        _ => None,
    }
}

/// The fields of a message, in order. Ends at the message's end, or at the first field
/// that cannot be read — [`Fields::broken`] says which.
pub struct Fields<'a> {
    buf: &'a [u8],
    pos: usize,
    broken: bool,
}

impl<'a> Fields<'a> {
    /// The fields of `buf`.
    pub const fn of(buf: &'a [u8]) -> Fields<'a> {
        Fields {
            buf,
            pos: 0,
            broken: false,
        }
    }

    /// The fields of `buf` from `pos` on.
    pub const fn from(buf: &'a [u8], pos: usize) -> Fields<'a> {
        Fields {
            buf,
            pos,
            broken: false,
        }
    }

    /// True once a field could not be read: the message is not one.
    pub const fn broken(&self) -> bool {
        self.broken
    }

    /// Where the walk stands.
    pub const fn position(&self) -> usize {
        self.pos
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Field<'a>> {
        if self.pos >= self.buf.len() || self.broken {
            return None;
        }
        match field(self.buf, self.pos) {
            Some((found, next)) => {
                self.pos = next;
                Some(found)
            }
            None => {
                self.broken = true;
                None
            }
        }
    }
}

/// The first field numbered `number` in `buf`, if the message holds one.
pub fn first(buf: &[u8], number: u32) -> Option<Field<'_>> {
    Fields::of(buf).find(|field| field.number == number)
}

/// The first field numbered `number` as an integer, or `default`.
pub fn integer(buf: &[u8], number: u32, default: u64) -> u64 {
    first(buf, number).map_or(default, |field| field.u64())
}

/// The first field numbered `number` as a reference's identifier.
pub fn reference(buf: &[u8], number: u32) -> Option<u64> {
    first(buf, number).and_then(|field| field.reference())
}

/// The first field numbered `number` as bytes, or nothing.
pub fn bytes(buf: &[u8], number: u32) -> &[u8] {
    first(buf, number).map_or(&[], |field| field.bytes)
}

/// Whether `buf` holds every field a well-formed message of its kind would: a walk that
/// reaches the end without a malformed field.
pub fn well_formed(buf: &[u8]) -> bool {
    let mut fields = Fields::of(buf);
    for _ in fields.by_ref() {}
    !fields.broken()
}
