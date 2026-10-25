//! Apple's IWA packaging, as a Numbers document stores its objects: every `.iwa` part is a
//! sequence of Snappy-compressed chunks (a zero byte, a three-byte little-endian compressed
//! length, the block), and the stream they decode to is a sequence of objects, each a
//! varint-prefixed `TSP.ArchiveInfo` header — the object's identifier and the type and
//! length of the messages that follow — and then those messages, Protocol Buffers both.
//!
//! Nothing here knows what a table is. This module finds an object by its identifier and
//! hands its first message over as bytes, keeping in the caller's window only the chunks
//! that hold the object asked for last — a tile's chunks while a tile is read, never a
//! whole part — and in the caller's state block where those chunks came from, so that a
//! call refused for room asks for the object again and finds it in place.
//!
//! A part that was deflated into the zip (Apple stores them; a package rewritten by another
//! tool may not) is inflated whole into the window first and parked at its far end, moved
//! along whenever the window grows, and read from there as if from the container.

use super::part::Stop;
use super::proto::{self, varint};
use super::snappy;
use super::sort::{heap_sort, partition_point};
use super::zip::{self, Directory, Entry, Located, METHOD_DEFLATE};
use super::{
    failure,
    part::{DEFLATE_RATIO, WINDOW_MIN},
};
use crate::kernel::abi::{Failure, Span};
use crate::kernel::inflate::{Inflate, Status};

/// A chunk header: a zero byte and a three-byte length.
const CHUNK_HEADER: usize = 4;

/// The most a Snappy block can grow by: a three-byte copy yields 64 bytes, so a block of
/// `n` bytes decodes to under `22n`; a chunk that declares more than this many times its
/// block is not one. Deflate's own ceiling is [`DEFLATE_RATIO`].
const SNAPPY_RATIO: u64 = 32;

/// What [`Failure::IWA`]'s `found` says.
/// A chunk header that is not one, a block cut short, or a Snappy block that does not
/// decode to what it declared.
pub const BROKEN_CHUNK: u32 = 1;
/// A Protocol Buffers message that is malformed, or an object header that is not an
/// `ArchiveInfo`.
pub const BROKEN_MESSAGE: u32 = 2;
/// An object a reference names is not in the package (`expected` is the identifier).
pub const MISSING_OBJECT: u32 = 3;
/// A tile's cell storage is of a version the core does not read (`expected` is the
/// version), or was saved before the layout the core reads.
pub const STORAGE_VERSION: u32 = 4;

/// What the loader keeps between calls. Plain integers: any bits are a cache that holds
/// nothing, at worst.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cache {
    /// The part (its central-header offset plus one) whose stream the window's head holds,
    /// or zero.
    entry: u64,
    /// The stream offset the head's first byte stands at.
    from: u64,
    /// Decoded bytes in the head.
    len: u64,
    /// The chunk decoded last — its offset in the part's bytes and where in the head it
    /// begins — which may be decoded only as far as it was needed.
    last_chunk: u64,
    last_at: u64,
    /// The part's offset of the chunk after it.
    next_chunk: u64,
    /// The deflated part (plus one) inflated into the window, where it starts, how long.
    inflated: u64,
    inflated_at: u64,
    inflated_len: u64,
}

/// One object's first message, with what its header said.
#[derive(Clone, Copy, Debug)]
pub struct Object<'a> {
    /// The message's type (`TSP.MessageInfo.type`), or zero for an object with none.
    pub kind: u32,
    pub payload: &'a [u8],
}

/// What an object's header says: its identifier, its first message's type and length, and
/// how long all its messages are together.
struct Header {
    identifier: u64,
    kind: u32,
    first: u64,
    total: u64,
}

/// One chunk, as its header places it.
#[derive(Clone, Copy)]
struct Chunk {
    /// Where its block starts in the part's bytes, and how long the block is.
    block: usize,
    len: usize,
    /// How many bytes it decodes to.
    size: u64,
    /// The part's offset of the chunk after it.
    next: usize,
}

/// A Numbers package being read: the container, and the caller's decoder, window and
/// cache.
pub struct Package<'a> {
    pub container: &'a [u8],
    pub directory: Directory,
    pub inflate: &'a mut Inflate,
    pub window: &'a mut [u8],
    pub cache: &'a mut Cache,
}

fn broken(found: u32, offset: u64) -> Stop {
    let mut what = failure(Failure::IWA, Failure::PART_IWA, found);
    if let Stop::Fail(failure) = &mut what {
        failure.byte = offset;
    }
    what
}

/// The failure for an object that is not there.
pub fn missing(identifier: u64) -> Stop {
    let mut what = failure(Failure::IWA, Failure::PART_IWA, MISSING_OBJECT);
    if let Stop::Fail(failure) = &mut what {
        failure.expected = identifier as u32;
    }
    what
}

/// The chunk whose header is at `at` in `source`, or `None` at the end.
fn chunk(source: &[u8], at: usize) -> Result<Option<Chunk>, Stop> {
    if at >= source.len() {
        return Ok(None);
    }
    let header = source
        .get(at..)
        .and_then(|rest| rest.first_chunk::<CHUNK_HEADER>())
        .ok_or(broken(BROKEN_CHUNK, at as u64))?;
    if header[0] != 0 {
        return Err(broken(BROKEN_CHUNK, at as u64));
    }
    let len = usize::from(header[1]) | usize::from(header[2]) << 8 | usize::from(header[3]) << 16;
    let block = at + CHUNK_HEADER;
    let bytes = source
        .get(block..block + len)
        .ok_or(broken(BROKEN_CHUNK, at as u64))?;
    let (size, _) = snappy::declared(bytes).ok_or(broken(BROKEN_CHUNK, at as u64))?;
    if size > (len as u64).saturating_mul(SNAPPY_RATIO).saturating_add(64) {
        return Err(broken(BROKEN_CHUNK, at as u64));
    }
    Ok(Some(Chunk {
        block,
        len,
        size,
        next: block + len,
    }))
}

/// The head of the window and the part's bytes, split so both can be held at once: the
/// bytes lie in the container, or past the head in the window.
fn split<'w>(
    window: &'w mut [u8],
    container: &'w [u8],
    located: Located,
    cache: &Cache,
) -> (&'w mut [u8], &'w [u8]) {
    if located.method == METHOD_DEFLATE {
        let at = usize::try_from(cache.inflated_at).unwrap_or(usize::MAX);
        let len = usize::try_from(cache.inflated_len).unwrap_or(usize::MAX);
        let (head, tail) = window.split_at_mut(at.min(window.len()));
        (head, tail.get(..len).unwrap_or_default())
    } else {
        let from = usize::try_from(located.data).unwrap_or(usize::MAX);
        let len = usize::try_from(located.len).unwrap_or(usize::MAX);
        let bytes = container
            .get(from..from.saturating_add(len))
            .unwrap_or_default();
        (window, bytes)
    }
}

impl<'a> Package<'a> {
    /// The entry whose central header is at `at`.
    pub fn entry(&self, at: u64) -> Result<Entry<'a>, Stop> {
        self.directory.entry(self.container, at).ok_or(failure(
            Failure::CONTAINER,
            Failure::PART_IWA,
            0,
        ))
    }

    fn located(&self, entry: &Entry<'_>) -> Result<Located, Stop> {
        zip::locate(self.container, entry)
            .map_err(|(code, found)| failure(code, Failure::PART_IWA, found))
    }

    /// The central-header offset of the package's `ordinal`th `.iwa` entry, in directory
    /// order, or `None` past the last.
    pub fn part(&self, ordinal: usize) -> Result<Option<u64>, Stop> {
        let mut at = 0usize;
        let mut seen = 0u64;
        let mut parts = 0usize;
        while seen < self.directory.count {
            let entry = self.entry(at as u64)?;
            let name = entry.name;
            let iwa = name.len().checked_sub(4).is_some_and(|dot| {
                name.get(dot..)
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(b".iwa"))
            });
            if iwa {
                if parts == ordinal {
                    return Ok(Some(at as u64));
                }
                parts += 1;
            }
            at = entry.next;
            seen += 1;
        }
        Ok(None)
    }

    /// Moves the inflated part to the window's far end, where the head cannot reach it.
    fn park_inflated(&mut self) {
        let cap = self.window.len();
        let len = usize::try_from(self.cache.inflated_len).unwrap_or(usize::MAX);
        let old = usize::try_from(self.cache.inflated_at).unwrap_or(usize::MAX);
        let new = cap.saturating_sub(len);
        if old == new || old.saturating_add(len) > cap {
            return;
        }
        // Last byte first: the two runs may overlap, and the move is forward.
        for index in (0..len).rev() {
            let byte = self.window.get(old + index).copied().unwrap_or(0);
            if let Some(slot) = self.window.get_mut(new + index) {
                *slot = byte;
            }
        }
        self.cache.inflated_at = new as u64;
    }

    /// Makes the part at `at` readable: a deflated one is inflated whole into the window
    /// and parked at its far end, where it stays — moved along when the window grows —
    /// until another deflated part is wanted. Returns where its bytes are.
    ///
    /// The size the directory declares is not believed: the stream is inflated into every
    /// byte the window has and the window asked to double until the stream ends, so the
    /// window is only ever as large as the part decodes to, which deflate bounds at 1032
    /// times the part's length.
    fn prepare(&mut self, at: u64) -> Result<Located, Stop> {
        let entry = self.entry(at)?;
        let located = self.located(&entry)?;
        if self.cache.entry != at + 1 {
            // Another part's chunks are in the head: forget them.
            self.cache.entry = 0;
            self.cache.len = 0;
        }
        if located.method != METHOD_DEFLATE {
            self.cache.inflated = 0;
            return Ok(located);
        }
        if self.cache.inflated == at + 1 {
            self.park_inflated();
            return Ok(located);
        }
        let cap = self.window.len();
        if cap < WINDOW_MIN {
            return Err(Stop::Window(WINDOW_MIN as u64));
        }
        let from = usize::try_from(located.data).unwrap_or(usize::MAX);
        let to = usize::try_from(located.data.saturating_add(located.len)).unwrap_or(usize::MAX);
        let input = self.container.get(from..to).unwrap_or_default();
        let ceiling = (input.len() as u64)
            .saturating_mul(DEFLATE_RATIO)
            .saturating_add(64);
        self.inflate.reset();
        let mut fed = 0usize;
        let mut written = 0usize;
        loop {
            let progress = self.inflate.run(
                input.get(fed..).unwrap_or_default(),
                &mut *self.window,
                written,
            );
            fed += progress.consumed;
            written += progress.written;
            match progress.status {
                Status::Done => break,
                Status::OutputFull if (cap as u64) >= ceiling => {
                    // No deflate stream of this length decodes to more: the part is not one.
                    return Err(failure(Failure::DEFLATE, Failure::PART_IWA, 0));
                }
                Status::OutputFull => {
                    if cap as u64 >= u64::from(u32::MAX) {
                        return Err(failure(Failure::TOO_LARGE, Failure::PART_IWA, 0));
                    }
                    // Start over with twice the room; nothing decoded so far is kept.
                    self.cache.inflated = 0;
                    return Err(Stop::Window(
                        (cap as u64).saturating_mul(2).max(WINDOW_MIN as u64),
                    ));
                }
                Status::Invalid => {
                    return Err(failure(Failure::DEFLATE, Failure::PART_IWA, 0));
                }
                Status::NeedsInput if fed >= input.len() => {
                    return Err(failure(Failure::DEFLATE, Failure::PART_IWA, 1));
                }
                Status::NeedsInput => {}
            }
        }
        self.cache.inflated = at + 1;
        self.cache.inflated_at = 0;
        self.cache.inflated_len = written as u64;
        self.cache.entry = 0;
        self.cache.len = 0;
        self.park_inflated();
        Ok(located)
    }

    /// How many bytes the part at `at` decodes to, by its chunk headers alone.
    pub fn stream_len(&mut self, at: u64) -> Result<u64, Stop> {
        let located = self.prepare(at)?;
        let (_, source) = split(&mut *self.window, self.container, located, self.cache);
        let mut total = 0u64;
        let mut at_chunk = 0usize;
        while let Some(found) = chunk(source, at_chunk)? {
            total = total.saturating_add(found.size);
            at_chunk = found.next;
        }
        Ok(total)
    }

    /// Makes the head hold stream bytes `offset..end` of the part at `at` (or as far as
    /// the stream goes), decoding chunks as needed, and returns the head and where
    /// `offset` is in it.
    fn ensure(&mut self, at: u64, offset: u64, end: u64) -> Result<(&[u8], usize), Stop> {
        let located = self.prepare(at)?;
        let cap = self.window.len() as u64;
        let cache = &mut *self.cache;
        let (head, source) = split(&mut *self.window, self.container, located, cache);
        if cache.entry != at + 1 || offset < cache.from || offset > cache.from + cache.len {
            // Start over at the chunk that holds `offset`.
            let mut stream = 0u64;
            let mut at_chunk = 0usize;
            loop {
                let Some(found) = chunk(source, at_chunk)? else {
                    // Past the end of the stream: nothing to hold, at its end.
                    cache.entry = at + 1;
                    cache.from = stream;
                    cache.len = 0;
                    cache.last_chunk = at_chunk as u64;
                    cache.last_at = 0;
                    cache.next_chunk = at_chunk as u64;
                    return Ok((&[], 0));
                };
                if offset < stream + found.size {
                    break;
                }
                stream += found.size;
                at_chunk = found.next;
            }
            cache.entry = at + 1;
            cache.from = stream;
            cache.len = 0;
            cache.last_chunk = at_chunk as u64;
            cache.last_at = 0;
            cache.next_chunk = at_chunk as u64;
        }
        // Extend the head until `end` is in it or the stream ends. The chunk decoded last
        // may have been decoded only as far as it was needed, and is decoded again, further.
        loop {
            if cache.from + cache.len >= end {
                break;
            }
            let last_at = usize::try_from(cache.last_at).unwrap_or(usize::MAX);
            let last = if cache.len > 0 {
                chunk(source, cache.last_chunk as usize)?
            } else {
                None
            };
            let (which, where_) = match last {
                Some(found) if cache.last_at + found.size > cache.len => (found, last_at),
                _ => {
                    let Some(found) = chunk(source, cache.next_chunk as usize)? else {
                        break;
                    };
                    (found, cache.len as usize)
                }
            };
            let need_in_chunk = (end - cache.from - where_ as u64).min(which.size) as usize;
            // Room for the whole chunk, decoded only as far as needed: a literal may run
            // past what is needed, and the decoder checks nothing it does not write.
            if (head.len() as u64) < where_ as u64 + which.size {
                // Ask for every chunk up to the one that holds `end`, whole.
                let mut total = where_ as u64 + which.size;
                let mut next = which.next;
                while cache.from + total < end {
                    let Some(found) = chunk(source, next)? else {
                        break;
                    };
                    total += found.size;
                    next = found.next;
                }
                let needed = total.saturating_add(cache.inflated_len);
                return Err(Stop::Window(
                    needed.max(cap.saturating_mul(2)).max(WINDOW_MIN as u64),
                ));
            }
            let block = source
                .get(which.block..which.block + which.len)
                .ok_or(broken(BROKEN_CHUNK, which.block as u64))?;
            let to = (where_ + which.size as usize).min(head.len());
            let room = head.get_mut(where_..to).unwrap_or_default();
            let written = snappy::prefix(block, room, need_in_chunk)
                .ok_or(broken(BROKEN_CHUNK, which.block as u64))?;
            cache.last_chunk = (which.block - CHUNK_HEADER) as u64;
            cache.last_at = where_ as u64;
            cache.next_chunk = which.next as u64;
            cache.len = (where_ + written) as u64;
        }
        let have = usize::try_from(cache.len).unwrap_or(usize::MAX);
        let start = usize::try_from(offset - cache.from).unwrap_or(usize::MAX);
        let head = head.get(..have.min(head.len())).unwrap_or_default();
        Ok((head, start.min(head.len())))
    }

    /// The header of the object at stream `offset` of the part at `at`, and where its
    /// messages begin. `None` at the end of the stream.
    fn header(&mut self, at: u64, offset: u64) -> Result<Option<(Header, u64)>, Stop> {
        // The length varint and then the header itself.
        let (head, start) = self.ensure(at, offset, offset + 16)?;
        if start >= head.len() {
            return Ok(None);
        }
        let (len, after) = varint(head, start).ok_or(broken(BROKEN_MESSAGE, offset))?;
        let varint_len = (after - start) as u64;
        let (head, start) = self.ensure(at, offset, offset + varint_len + len)?;
        let bytes = head
            .get(start + varint_len as usize..start + (varint_len + len) as usize)
            .ok_or(broken(BROKEN_MESSAGE, offset))?;
        if !proto::well_formed(bytes) {
            return Err(broken(BROKEN_MESSAGE, offset));
        }
        let mut header = Header {
            identifier: proto::integer(bytes, 1, 0),
            kind: 0,
            first: 0,
            total: 0,
        };
        let mut seen = false;
        for info in proto::Fields::of(bytes).filter(|field| field.number == 2) {
            if !proto::well_formed(info.bytes) {
                return Err(broken(BROKEN_MESSAGE, offset));
            }
            let length = proto::integer(info.bytes, 3, 0);
            if !seen {
                header.kind = proto::integer(info.bytes, 1, 0) as u32;
                header.first = length;
                seen = true;
            }
            header.total = header.total.saturating_add(length);
        }
        Ok(Some((header, offset + varint_len + len)))
    }

    /// The object at stream `offset` of the part at `at`.
    pub fn object(&mut self, at: u64, offset: u64) -> Result<Object<'_>, Stop> {
        let Some((header, payload)) = self.header(at, offset)? else {
            return Err(broken(BROKEN_MESSAGE, offset));
        };
        let end = payload.saturating_add(header.total);
        let (head, start) = self.ensure(at, offset, end)?;
        let from = start + (payload - offset) as usize;
        let bytes = head
            .get(from..from + header.first as usize)
            .ok_or(broken(BROKEN_MESSAGE, offset))?;
        Ok(Object {
            kind: header.kind,
            payload: bytes,
        })
    }

    /// Calls `f` with the identifier, type and stream offset of every object of the part at
    /// `at`, until it says to stop.
    pub fn scan(
        &mut self,
        at: u64,
        mut f: impl FnMut(u64, u32, u64) -> Result<bool, Stop>,
    ) -> Result<(), Stop> {
        let mut offset = 0u64;
        while let Some((header, payload)) = self.header(at, offset)? {
            if !f(header.identifier, header.kind, offset)? {
                return Ok(());
            }
            let next = payload.saturating_add(header.total);
            if next <= offset {
                return Err(broken(BROKEN_MESSAGE, offset));
            }
            offset = next;
        }
        Ok(())
    }

    /// Where object `identifier` is, by a walk of every part — the one at `first` before
    /// the rest. For the handful of lookups opening makes; a listing builds an [`index`].
    ///
    /// [`index`]: Package::index
    pub fn find(&mut self, first: u64, identifier: u64) -> Result<Option<(u64, u64)>, Stop> {
        let mut ordinal = 0usize;
        let mut at = Some(first);
        while let Some(part) = at {
            let mut found = None;
            self.scan(part, |id, _, offset| {
                if id == identifier {
                    found = Some((part, offset));
                    return Ok(false);
                }
                Ok(true)
            })?;
            if found.is_some() {
                return Ok(found);
            }
            at = loop {
                let next = self.part(ordinal)?;
                ordinal += 1;
                if next != Some(first) {
                    break next;
                }
            };
        }
        Ok(None)
    }

    /// Lists every object of the package into `cells`, two spans each — the identifier,
    /// then where it is (the part's central-header offset, its stream offset) — sorted by
    /// identifier, and returns how many. A package too large to index that way is too large.
    pub fn index(&mut self, cells: &mut [Span]) -> Result<usize, Stop> {
        let mut count = 0usize;
        let mut parts = 0usize;
        while let Some(at) = self.part(parts)? {
            parts += 1;
            if at > u64::from(u32::MAX) {
                return Err(failure(Failure::TOO_LARGE, Failure::PART_IWA, 0));
            }
            let mut stop: Option<Stop> = None;
            self.scan(at, |identifier, kind, offset| {
                if kind == 0 {
                    return Ok(true);
                }
                if offset > u64::from(u32::MAX) {
                    stop = Some(failure(Failure::TOO_LARGE, Failure::PART_IWA, 0));
                    return Ok(false);
                }
                let Some(slot) = cells
                    .get_mut(count * 2..)
                    .and_then(|rest| rest.first_chunk_mut::<2>())
                else {
                    stop = Some(Stop::Cells(
                        ((count as u64 + 1) * 2).max(cells.len() as u64 * 2),
                    ));
                    return Ok(false);
                };
                *slot = [
                    Span {
                        offset: identifier as u32,
                        len: (identifier >> 32) as u32,
                    },
                    Span {
                        offset: at as u32,
                        len: offset as u32,
                    },
                ];
                count += 1;
                Ok(true)
            })?;
            if let Some(stop) = stop {
                return Err(stop);
            }
        }
        heap_sort(
            cells,
            count,
            |cells, a, b| key(cells, a) < key(cells, b),
            |cells, a, b| {
                let (left, right) = (record(cells, a), record(cells, b));
                set_record(cells, a, right);
                set_record(cells, b, left);
            },
        );
        Ok(count)
    }
}

fn record(cells: &[Span], index: usize) -> [Span; 2] {
    cells
        .get(index * 2..)
        .and_then(|rest| rest.first_chunk::<2>())
        .copied()
        .unwrap_or_default()
}

fn set_record(cells: &mut [Span], index: usize, value: [Span; 2]) {
    if let Some(slot) = cells
        .get_mut(index * 2..)
        .and_then(|rest| rest.first_chunk_mut::<2>())
    {
        *slot = value;
    }
}

fn key(cells: &[Span], index: usize) -> u64 {
    let [id, _] = record(cells, index);
    u64::from(id.offset) | u64::from(id.len) << 32
}

/// Where object `identifier` is, in an index of `count` objects: the part's central-header
/// offset and the object's stream offset.
pub fn resolve(cells: &[Span], count: usize, identifier: u64) -> Option<(u64, u64)> {
    let at = partition_point(count, |index| key(cells, index) < identifier);
    if at >= count || key(cells, at) != identifier {
        return None;
    }
    let [_, place] = record(cells, at);
    Some((u64::from(place.offset), u64::from(place.len)))
}
