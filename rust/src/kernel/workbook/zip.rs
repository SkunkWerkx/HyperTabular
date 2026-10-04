//! The zip container, read where it lies: the end-of-central-directory record (zip64
//! included), the central directory walked in place, and the local header that says where
//! an entry's bytes start. Nothing is copied out and no list of entries is kept — a part
//! is found by walking the directory again, which is a handful of walks per workbook.
//!
//! Every offset and length here comes from the file and is believed only as far as the
//! container's own length allows.

use super::xml::{slice, tail};
use crate::kernel::abi::Failure;

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// Stored: the entry's bytes are its content.
pub const METHOD_STORED: u16 = 0;
/// Deflate.
pub const METHOD_DEFLATE: u16 = 8;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    tail(bytes, at)
        .first_chunk::<2>()
        .map(|b| u16::from_le_bytes(*b))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    tail(bytes, at)
        .first_chunk::<4>()
        .map(|b| u32::from_le_bytes(*b))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    tail(bytes, at)
        .first_chunk::<8>()
        .map(|b| u64::from_le_bytes(*b))
}

/// Where the central directory is and how many entries it says it has.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Directory {
    pub offset: u64,
    pub size: u64,
    pub count: u64,
}

/// One central-directory entry.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    /// The name as the archive spells it.
    pub name: &'a [u8],
    pub method: u16,
    pub flags: u16,
    pub compressed: u64,
    pub uncompressed: u64,
    /// Where the entry's local header is, from the start of the container.
    pub header: u64,
    /// Where this entry's central header is, from the start of the directory.
    pub at: usize,
    /// Where the next entry's central header is.
    pub next: usize,
}

/// An entry's content: where it starts, how much of it the container holds, and how much
/// it claims to inflate to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Located {
    pub data: u64,
    pub len: u64,
    pub limit: u64,
    pub method: u16,
}

/// Finds and checks the central directory: every entry it claims must be there.
pub fn directory(container: &[u8]) -> Result<Directory, u32> {
    // The record is 22 bytes plus a comment of at most 65 535.
    let tail_start = container.len().saturating_sub(22 + 65_535);
    let end = tail(container, tail_start);
    let mut eocd = None;
    let mut at = end.len().saturating_sub(21);
    while at > 0 {
        at -= 1;
        if u32_at(end, at) == Some(EOCD_SIG) {
            eocd = Some(at);
            break;
        }
    }
    let eocd = eocd.ok_or(Failure::NOT_A_ZIP)?;
    let field = |at: usize| u32_at(end, eocd + at).map(u64::from);
    let mut count = u64::from(u16_at(end, eocd + 10).ok_or(Failure::NOT_A_ZIP)?);
    let mut size = field(12).ok_or(Failure::NOT_A_ZIP)?;
    let mut offset = field(16).ok_or(Failure::NOT_A_ZIP)?;
    if count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
        // zip64: the locator sits immediately before the record.
        let locator = eocd
            .checked_sub(20)
            .filter(|&at| u32_at(end, at) == Some(EOCD64_LOCATOR_SIG))
            .ok_or(Failure::CONTAINER)?;
        let record = u64_at(end, locator + 8).ok_or(Failure::CONTAINER)?;
        let record = usize::try_from(record).map_err(|_| Failure::CONTAINER)?;
        let eocd64 = tail(container, record);
        if eocd64.len() < 56 || u32_at(eocd64, 0) != Some(EOCD64_SIG) {
            return Err(Failure::CONTAINER);
        }
        count = u64_at(eocd64, 32).ok_or(Failure::CONTAINER)?;
        size = u64_at(eocd64, 40).ok_or(Failure::CONTAINER)?;
        offset = u64_at(eocd64, 48).ok_or(Failure::CONTAINER)?;
    }
    if offset
        .checked_add(size)
        .is_none_or(|end| end > container.len() as u64)
    {
        return Err(Failure::CONTAINER);
    }
    let directory = Directory {
        offset,
        size,
        count,
    };
    let bytes = directory.bytes(container);
    let mut at = 0usize;
    let mut seen = 0u64;
    while seen < count {
        at = entry(bytes, at).ok_or(Failure::CONTAINER)?.next;
        seen += 1;
    }
    Ok(directory)
}

impl Directory {
    /// The directory's own bytes.
    pub fn bytes<'a>(&self, container: &'a [u8]) -> &'a [u8] {
        let (Ok(from), Ok(size)) = (usize::try_from(self.offset), usize::try_from(self.size))
        else {
            return &[];
        };
        slice(container, from, from.saturating_add(size))
    }

    /// The first entry whose name is `wanted`, leading slashes and ASCII case aside.
    pub fn find<'a>(&self, container: &'a [u8], wanted: &[u8]) -> Option<Entry<'a>> {
        let wanted = unrooted(wanted);
        let bytes = self.bytes(container);
        let mut at = 0usize;
        let mut seen = 0u64;
        while seen < self.count {
            let found = entry(bytes, at)?;
            if unrooted(found.name).eq_ignore_ascii_case(wanted) {
                return Some(found);
            }
            at = found.next;
            seen += 1;
        }
        None
    }

    /// The entry whose central header is at `at` in the directory.
    pub fn entry<'a>(&self, container: &'a [u8], at: u64) -> Option<Entry<'a>> {
        entry(self.bytes(container), usize::try_from(at).ok()?)
    }
}

/// A name without the slashes some writers put in front of it.
pub fn unrooted(name: &[u8]) -> &[u8] {
    let lead = name
        .iter()
        .take_while(|&&b| b == b'/' || b == b'\\')
        .count();
    tail(name, lead)
}

/// The central header at `at` in the directory's bytes.
fn entry(directory: &[u8], at: usize) -> Option<Entry<'_>> {
    let header = tail(directory, at);
    if header.len() < 46 || u32_at(header, 0) != Some(CENTRAL_SIG) {
        return None;
    }
    let flags = u16_at(header, 8)?;
    let method = u16_at(header, 10)?;
    let mut compressed = u64::from(u32_at(header, 20)?);
    let mut uncompressed = u64::from(u32_at(header, 24)?);
    let name_len = usize::from(u16_at(header, 28)?);
    let extra_len = usize::from(u16_at(header, 30)?);
    let comment_len = usize::from(u16_at(header, 32)?);
    let mut local = u64::from(u32_at(header, 42)?);
    let extra_start = 46 + name_len;
    let end = extra_start + extra_len + comment_len;
    if end > header.len() {
        return None;
    }
    // The zip64 extra field: only the fields that were saturated are present, in order.
    let mut extra = extra_start;
    while extra + 4 <= extra_start + extra_len {
        let id = u16_at(header, extra)?;
        let size = usize::from(u16_at(header, extra + 2)?);
        let body = extra + 4;
        if id == 0x0001 {
            let mut cursor = body;
            for field in [&mut uncompressed, &mut compressed, &mut local] {
                if *field == 0xFFFF_FFFF && cursor + 8 <= body + size {
                    if let Some(wide) = u64_at(header, cursor) {
                        *field = wide;
                    }
                    cursor += 8;
                }
            }
        }
        extra = body + size;
    }
    Some(Entry {
        name: slice(header, 46, extra_start),
        method,
        flags,
        compressed,
        uncompressed,
        header: local,
        at,
        next: at + end,
    })
}

/// Where an entry's content is. An encrypted entry, a compression method that is neither
/// stored nor deflate, and a local header that is not one are each refused, as the
/// failure code and what was found.
pub fn locate(container: &[u8], entry: &Entry<'_>) -> Result<Located, (u32, u32)> {
    if entry.flags & 1 != 0 {
        return Err((Failure::ENCRYPTED, 0));
    }
    if entry.method != METHOD_STORED && entry.method != METHOD_DEFLATE {
        return Err((Failure::METHOD, u32::from(entry.method)));
    }
    let at = usize::try_from(entry.header).map_err(|_| (Failure::CONTAINER, 0))?;
    let header = tail(container, at);
    if header.len() < 30 || u32_at(header, 0) != Some(LOCAL_SIG) {
        return Err((Failure::CONTAINER, 0));
    }
    let skip =
        u64::from(u16_at(header, 26).unwrap_or(0)) + u64::from(u16_at(header, 28).unwrap_or(0));
    let data = entry.header.saturating_add(30 + skip);
    let held = (container.len() as u64).saturating_sub(data);
    Ok(Located {
        data,
        len: entry.compressed.min(held),
        limit: entry.uncompressed,
        method: entry.method,
    })
}
