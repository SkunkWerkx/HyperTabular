//! A hand-rolled zip reader: central directory first (local headers are trusted only for
//! the data offset), zip64 aware, stored and deflate only, no encryption. Entries stream
//! through `flate2` on the `zlib-rs` backend so a part of any size runs through a fixed
//! inflate window.

use crate::workbook::error::Error;
use std::io::{self, Read, Seek, SeekFrom, Take};

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// One central-directory entry.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The entry name as stored (forward slashes, no leading slash normalisation).
    pub name: String,
    method: u16,
    flags: u16,
    compressed: u64,
    uncompressed: u64,
    header_offset: u64,
}

impl Entry {
    /// The entry's uncompressed size per the central directory.
    pub fn size(&self) -> u64 {
        self.uncompressed
    }
}

/// The parsed central directory over a reader.
pub struct Archive<R> {
    reader: R,
    entries: Vec<Entry>,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn container(detail: impl Into<String>) -> Error {
    Error::Container(detail.into())
}

impl<R> Archive<R> {
    /// An archive over a fresh reader of the same bytes, reusing this one's directory.
    pub fn with_reader<R2: Read + Seek>(&self, reader: R2) -> Archive<R2> {
        Archive {
            reader,
            entries: self.entries.clone(),
        }
    }

    /// The underlying reader.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// Every entry, in directory order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The index of the entry named `name`, compared ASCII-case-insensitively and with a
    /// leading `/` or `\` on either side ignored (OPC part names are case-insensitive and
    /// third-party writers root them inconsistently).
    pub fn find(&self, name: &str) -> Option<usize> {
        let wanted = name.trim_start_matches(['/', '\\']);
        self.entries.iter().position(|entry| {
            entry
                .name
                .trim_start_matches(['/', '\\'])
                .eq_ignore_ascii_case(wanted)
        })
    }
}

impl<R: Read + Seek> Archive<R> {
    /// Reads the central directory.
    pub fn open(mut reader: R) -> Result<Archive<R>, Error> {
        let len = reader.seek(SeekFrom::End(0))?;
        // The EOCD is 22 bytes plus a comment of at most 65 535.
        let tail_len = len.min(22 + 65_535);
        reader.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        reader.read_exact(&mut tail)?;
        let eocd = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|&i| u32_at(&tail, i) == EOCD_SIG)
            .ok_or_else(|| container("no end-of-central-directory record: not a zip file"))?;
        let mut entry_count = u64::from(u16_at(&tail, eocd + 10));
        let mut cd_size = u64::from(u32_at(&tail, eocd + 12));
        let mut cd_offset = u64::from(u32_at(&tail, eocd + 16));
        if entry_count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
            // zip64: the locator sits immediately before the EOCD.
            let locator = eocd
                .checked_sub(20)
                .filter(|&at| u32_at(&tail, at) == EOCD64_LOCATOR_SIG);
            let Some(locator) = locator else {
                return Err(container("zip64 sizes without a zip64 locator"));
            };
            let eocd64_offset = u64_at(&tail, locator + 8);
            reader.seek(SeekFrom::Start(eocd64_offset))?;
            let mut eocd64 = [0u8; 56];
            reader.read_exact(&mut eocd64)?;
            if u32_at(&eocd64, 0) != EOCD64_SIG {
                return Err(container("bad zip64 end-of-central-directory record"));
            }
            entry_count = u64_at(&eocd64, 32);
            cd_size = u64_at(&eocd64, 40);
            cd_offset = u64_at(&eocd64, 48);
        }
        if cd_offset.checked_add(cd_size).is_none_or(|end| end > len) {
            return Err(container("central directory lies outside the file"));
        }
        reader.seek(SeekFrom::Start(cd_offset))?;
        let mut directory = vec![0u8; cd_size as usize];
        reader.read_exact(&mut directory)?;

        let mut entries = Vec::with_capacity(entry_count.min(4096) as usize);
        let mut at = 0usize;
        for _ in 0..entry_count {
            if at + 46 > directory.len() || u32_at(&directory, at) != CENTRAL_SIG {
                return Err(container("truncated central directory"));
            }
            let flags = u16_at(&directory, at + 8);
            let method = u16_at(&directory, at + 10);
            let mut compressed = u64::from(u32_at(&directory, at + 20));
            let mut uncompressed = u64::from(u32_at(&directory, at + 24));
            let name_len = usize::from(u16_at(&directory, at + 28));
            let extra_len = usize::from(u16_at(&directory, at + 30));
            let comment_len = usize::from(u16_at(&directory, at + 32));
            let mut header_offset = u64::from(u32_at(&directory, at + 42));
            let name_start = at + 46;
            let extra_start = name_start + name_len;
            let end = extra_start + extra_len + comment_len;
            if end > directory.len() {
                return Err(container("truncated central directory entry"));
            }
            let name = String::from_utf8_lossy(&directory[name_start..extra_start]).into_owned();
            // zip64 extra field: only the fields that were saturated are present, in order.
            let mut extra = extra_start;
            while extra + 4 <= extra_start + extra_len {
                let id = u16_at(&directory, extra);
                let size = usize::from(u16_at(&directory, extra + 2));
                let body = extra + 4;
                if id == 0x0001 {
                    let mut cursor = body;
                    for field in [&mut uncompressed, &mut compressed, &mut header_offset] {
                        if *field == 0xFFFF_FFFF && cursor + 8 <= body + size {
                            *field = u64_at(&directory, cursor);
                            cursor += 8;
                        }
                    }
                }
                extra = body + size;
            }
            entries.push(Entry {
                name,
                method,
                flags,
                compressed,
                uncompressed,
                header_offset,
            });
            at = end;
        }
        Ok(Archive { reader, entries })
    }

    /// Positions the reader at the entry's data and returns the entry's method and sizes.
    fn locate(&mut self, index: usize) -> Result<(u16, u64, u64), Error> {
        let entry = &self.entries[index];
        if entry.flags & 0x1 != 0 {
            return Err(container(format!("{} is encrypted", entry.name)));
        }
        if entry.method != METHOD_STORED && entry.method != METHOD_DEFLATE {
            return Err(container(format!(
                "{} uses compression method {}",
                entry.name, entry.method
            )));
        }
        self.reader.seek(SeekFrom::Start(entry.header_offset))?;
        let mut header = [0u8; 30];
        self.reader.read_exact(&mut header)?;
        if u32_at(&header, 0) != LOCAL_SIG {
            return Err(container(format!("{}: bad local header", entry.name)));
        }
        let skip = u64::from(u16_at(&header, 26)) + u64::from(u16_at(&header, 28));
        self.reader.seek(SeekFrom::Current(skip as i64))?;
        Ok((entry.method, entry.compressed, entry.uncompressed))
    }

    /// A streaming reader over one entry's uncompressed bytes, borrowing the archive.
    pub fn stream(&mut self, index: usize) -> Result<EntryReader<&mut R>, Error> {
        let (method, compressed, uncompressed) = self.locate(index)?;
        Ok(EntryReader::new(
            &mut self.reader,
            method,
            compressed,
            uncompressed,
        ))
    }

    /// A streaming reader that takes the archive's reader with it — for a cursor that
    /// must outlive the archive (a [`crate::workbook::Sheet`]).
    pub fn into_stream(mut self, index: usize) -> Result<EntryReader<R>, Error> {
        let (method, compressed, uncompressed) = self.locate(index)?;
        Ok(EntryReader::new(
            self.reader,
            method,
            compressed,
            uncompressed,
        ))
    }

    /// One entry's whole content.
    pub fn read(&mut self, index: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::with_capacity(self.entries[index].uncompressed.min(1 << 26) as usize);
        self.stream(index)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

enum Inner<R> {
    Stored(Take<R>),
    Deflate(flate2::read::DeflateDecoder<Take<R>>),
}

/// A `Read` over one entry's uncompressed bytes.
pub struct EntryReader<R> {
    inner: Inner<R>,
    remaining: u64,
}

impl<R: Read> EntryReader<R> {
    fn new(reader: R, method: u16, compressed: u64, uncompressed: u64) -> EntryReader<R> {
        let take = reader.take(compressed);
        let inner = if method == METHOD_DEFLATE {
            Inner::Deflate(flate2::read::DeflateDecoder::new(take))
        } else {
            Inner::Stored(take)
        };
        EntryReader {
            inner,
            remaining: uncompressed,
        }
    }
}

impl<R: Read> Read for EntryReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = buf
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let n = match &mut self.inner {
            Inner::Stored(take) => take.read(&mut buf[..limit])?,
            Inner::Deflate(decoder) => decoder.read(&mut buf[..limit])?,
        };
        self.remaining -= n as u64;
        Ok(n)
    }
}

pub mod write {
    //! A minimal zip *writer* — stored or deflated entries, no zip64 — for this crate's
    //! tests and benchmarks, which build their fixtures in memory. Not a writing feature:
    //! it exists so the reader can be exercised without committing large binaries.

    use std::io::Write;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Builds a zip with the given `(name, content, deflate?)` entries, in order.
    pub fn build(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for &(name, content, deflate) in entries {
            let data = if deflate {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(content).unwrap();
                encoder.finish().unwrap()
            } else {
                content.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            let crc = crc32(content);
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(content.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(content.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_stored_and_deflated_entries_by_name() {
        let big: Vec<u8> = (0..200_000u32)
            .map(|i| b"abcdefgh"[(i % 8) as usize])
            .collect();
        let zip = write::build(&[
            ("mimetype", b"application/x", false),
            ("xl/workbook.xml", b"<w/>", true),
            ("Big/Part.bin", &big, true),
        ]);
        let mut archive = Archive::open(Cursor::new(&zip[..])).unwrap();
        assert_eq!(archive.entries().len(), 3);
        let index = archive.find("/XL/WORKBOOK.XML").unwrap();
        assert_eq!(archive.read(index).unwrap(), b"<w/>");
        assert_eq!(archive.read(0).unwrap(), b"application/x");
        let index = archive.find("big/part.bin").unwrap();
        assert_eq!(archive.entries()[index].size(), big.len() as u64);
        let mut streamed = Vec::new();
        archive
            .stream(index)
            .unwrap()
            .read_to_end(&mut streamed)
            .unwrap();
        assert_eq!(streamed, big);
        assert!(archive.find("nope").is_none());
        // Taking the reader along.
        let mut owned = archive
            .with_reader(Cursor::new(&zip[..]))
            .into_stream(index)
            .unwrap();
        let mut again = Vec::new();
        owned.read_to_end(&mut again).unwrap();
        assert_eq!(again, big);
    }

    #[test]
    fn rejects_non_zips() {
        assert!(matches!(
            Archive::open(Cursor::new(&b"not a zip at all"[..])),
            Err(Error::Container(_))
        ));
        assert!(matches!(
            Archive::open(Cursor::new(&b""[..])),
            Err(Error::Container(_))
        ));
    }
}
