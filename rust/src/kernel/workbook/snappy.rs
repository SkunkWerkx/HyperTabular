//! Snappy's block format — a varint uncompressed length, then literals and copies — as the
//! chunks of an Apple IWA part carry it (no stream framing, no checksums). A block is
//! decoded whole into the caller's buffer, every offset and length believed only as far as
//! the block and the buffer allow, and nothing is kept between calls.
//!
//! The format: each element starts with a tag byte whose low two bits say what it is. `00`
//! is a literal of `tag >> 2` + 1 bytes, or, for a field of 60 and up, of the one to four
//! little-endian bytes that follow, plus one. `01` is a copy of 4 to 11 bytes from an
//! 11-bit offset split across the tag and one more byte; `10` and `11` are copies of up to
//! 64 bytes from a two- or four-byte offset. A copy may reach into what it is writing.

use super::proto::varint;

/// The uncompressed length a block declares, and where its first element starts. `None`
/// for a block that does not start with a varint.
pub fn declared(block: &[u8]) -> Option<(u64, usize)> {
    varint(block, 0)
}

/// Decodes `block` whole into `out`, which must hold the declared length, and returns how
/// many bytes were written — the declared length, or `None` for a block that is not
/// Snappy: an element cut short, a copy that reaches before the start, a block that writes
/// more or less than it declared, or output too small for it.
pub fn decode(block: &[u8], out: &mut [u8]) -> Option<usize> {
    let (declared, _) = declared(block)?;
    let written = prefix(block, out, usize::MAX)?;
    if written as u64 == declared {
        Some(written)
    } else {
        None
    }
}

/// Decodes `block` into `out` until at least `need` bytes have been written or the block
/// ends, and returns how many were — all of them when the block ends first, and then as
/// many as it declared. `out` must hold what is decoded; a block that does not fit, or is
/// not Snappy as far as it is read, is `None`. What a prefix does not reach is not checked.
pub fn prefix(block: &[u8], out: &mut [u8], need: usize) -> Option<usize> {
    let (declared, mut pos) = declared(block)?;
    let declared = usize::try_from(declared).ok()?;
    let mut written = 0usize;
    while written < need {
        let Some(&tag) = block.get(pos) else {
            // The block has ended: it must have written exactly what it declared.
            return if written == declared {
                Some(written)
            } else {
                None
            };
        };
        pos += 1;
        let (length, offset) = match tag & 3 {
            0 => {
                let mut length = usize::from(tag >> 2);
                if length >= 60 {
                    let extra = length - 59;
                    let bytes = block.get(pos..pos + extra)?;
                    length = 0;
                    for (index, &byte) in bytes.iter().enumerate() {
                        length |= usize::from(byte) << (index * 8);
                    }
                    pos += extra;
                }
                let length = length.checked_add(1)?;
                let end = written.checked_add(length)?;
                if end > declared {
                    return None;
                }
                let source = block.get(pos..pos + length)?;
                out.get_mut(written..end)?.copy_from_slice(source);
                pos += length;
                written = end;
                continue;
            }
            1 => {
                let byte = *block.get(pos)?;
                pos += 1;
                (
                    usize::from((tag >> 2) & 7) + 4,
                    usize::from(tag >> 5) << 8 | usize::from(byte),
                )
            }
            2 => {
                let bytes = block.get(pos..pos + 2)?;
                pos += 2;
                (
                    usize::from(tag >> 2) + 1,
                    usize::from(u16::from_le_bytes([bytes[0], bytes[1]])),
                )
            }
            _ => {
                let bytes = block.get(pos..pos + 4)?;
                pos += 4;
                let offset = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                (usize::from(tag >> 2) + 1, usize::try_from(offset).ok()?)
            }
        };
        // A copy reads from `offset` bytes back, which must lie inside what was written.
        if offset == 0 || offset > written {
            return None;
        }
        let from = written - offset;
        let end = written.checked_add(length)?;
        if end > declared {
            return None;
        }
        // Byte by byte, as the format says: a copy may overlap what it writes (a run), and
        // no copy is longer than 64 bytes.
        for index in 0..length {
            let byte = *out.get(from + index)?;
            *out.get_mut(written + index)? = byte;
        }
        written = end;
    }
    Some(written)
}
