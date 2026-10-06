//! What a workbook is before any sheet is read: which format, where its parts are, the
//! sheets' names, and (XLSX) the two tables every sheet reads its cells against — the
//! shared strings and the number-format kind of each cell format.
//!
//! The part-resolution order is OPC's: `_rels/.rels` names the workbook part, whose own
//! relationships name the sheets, the style table and the shared strings. ODS has one
//! part, `content.xml`, and its sheets are that part's tables.

use super::part::{Reader, Stop};
use super::sort::{heap_sort, partition_point};
use super::xml::{self, Kind, Tag, parse_u32, slice, unescape_into};
use super::zip::{self, Directory, Entry, unrooted};
use super::{
    DONE, FORMAT_ODS, FORMAT_XLSX, Memory, OP_NONE, OP_SHEETS, OP_STRINGS, OP_STYLES, OPENED,
    State, begin, failure, refuse, room, styles,
};
use crate::kernel::abi::{ERR_CONTRACT, Failure, Filled, OK, Opened, Span};
use hypercast::ExcelEpoch;

const MIMETYPE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const CONTENT: &[u8] = b"content.xml";
const DEFAULT_WORKBOOK: &[u8] = b"xl/workbook.xml";

/// A part name split at its last slash: the directory (slash included) and the file.
fn dir_of(part: &[u8]) -> (&[u8], &[u8]) {
    match part.iter().rposition(|&b| b == b'/') {
        Some(slash) => part.split_at_checked(slash + 1).unwrap_or((b"", part)),
        None => (b"", part),
    }
}

/// A relationship target against the directory of the part that names it, as OPC says: a
/// rooted target stands alone, `.` and empty segments vanish, `..` drops one. Written to
/// `out`, which holds `dir.len() + target.len()` bytes; returns the length.
fn resolve(dir: &[u8], target: &[u8], out: &mut [u8]) -> usize {
    let (dir, target) = match target.strip_prefix(b"/") {
        Some(rooted) => (&b""[..], rooted),
        None => (dir, target),
    };
    let mut len = 0usize;
    let segments = dir
        .split(|&b| b == b'/')
        .chain(target.split(|&b| b == b'/'));
    for segment in segments {
        match segment {
            b"" | b"." => {}
            b".." => {
                len = slice(out, 0, len)
                    .iter()
                    .rposition(|&b| b == b'/')
                    .unwrap_or(0);
            }
            name => {
                if len > 0
                    && let Some(slot) = out.get_mut(len)
                {
                    *slot = b'/';
                    len += 1;
                }
                if let Some(room) = out.get_mut(len..len + name.len()) {
                    room.copy_from_slice(name);
                    len += name.len();
                }
            }
        }
    }
    len
}

/// The three attributes of an internal relationship: its id, its type and its target.
fn relationship<'a>(tag: &Tag<'a>) -> Option<(&'a [u8], &'a [u8], &'a [u8])> {
    if tag.local() != b"Relationship"
        || tag
            .attr(b"TargetMode")
            .is_some_and(|mode| mode.eq_ignore_ascii_case(b"External"))
    {
        return None;
    }
    Some((tag.attr(b"Id")?, tag.attr(b"Type")?, tag.attr(b"Target")?))
}

/// Text being put together in the caller's arena.
struct Scratch<'a> {
    bytes: &'a mut [u8],
    used: usize,
}

/// A run of the scratch: where it starts and how long it is.
type Run = (usize, usize);

impl Scratch<'_> {
    fn ensure(&self, more: usize) -> Result<(), Stop> {
        let end = self.used.saturating_add(more);
        if end > self.bytes.len() {
            return Err(Stop::Arena(
                (end as u64).max((self.bytes.len() as u64).saturating_mul(2)),
            ));
        }
        Ok(())
    }

    fn at(&self, run: Run) -> &[u8] {
        slice(self.bytes, run.0, run.0 + run.1)
    }

    fn push(&mut self, pieces: &[&[u8]]) -> Result<Run, Stop> {
        let total: usize = pieces.iter().map(|piece| piece.len()).sum();
        self.ensure(total)?;
        let start = self.used;
        for piece in pieces {
            if let Some(room) = self.bytes.get_mut(self.used..self.used + piece.len()) {
                room.copy_from_slice(piece);
                self.used += piece.len();
            }
        }
        Ok((start, self.used - start))
    }

    /// Appends a copy of an earlier run.
    fn repeat(&mut self, run: Run) -> Result<Run, Stop> {
        self.ensure(run.1)?;
        let start = self.used;
        if let Some((head, room)) = self.bytes.split_at_mut_checked(start)
            && let (Some(source), Some(room)) =
                (head.get(run.0..run.0 + run.1), room.get_mut(..run.1))
        {
            room.copy_from_slice(source);
            self.used += run.1;
        }
        Ok((start, self.used - start))
    }

    fn push_unescaped(&mut self, raw: &[u8]) -> Result<Run, Stop> {
        self.ensure(raw.len())?;
        let start = self.used;
        let written = unescape_into(raw, tail_mut(self.bytes, start));
        self.used += written;
        Ok((start, written))
    }

    /// `target` (escaped, as its attribute holds it) resolved against `dir`, which is
    /// either outside the scratch or a run of it.
    fn resolve(&mut self, dir: Result<&[u8], Run>, target: &[u8]) -> Result<Run, Stop> {
        let target = self.push_unescaped(target)?;
        let dir_len = match dir {
            Ok(bytes) => bytes.len(),
            Err(run) => run.1,
        };
        self.ensure(dir_len + target.1)?;
        let start = self.used;
        let Some((head, out)) = self.bytes.split_at_mut_checked(start) else {
            return Ok((start, 0));
        };
        let dir = match dir {
            Ok(bytes) => bytes,
            Err(run) => slice(head, run.0, run.0 + run.1),
        };
        let len = resolve(dir, slice(head, target.0, target.0 + target.1), out);
        self.used += len;
        Ok((start, len))
    }
}

fn tail_mut(bytes: &mut [u8], from: usize) -> &mut [u8] {
    bytes.get_mut(from..).unwrap_or_default()
}

/// Positions `reader` at the start of `entry`.
fn start(reader: &mut Reader<'_>, entry: &Entry<'_>, id: u32) -> Result<(), Stop> {
    let located =
        zip::locate(reader.container, entry).map_err(|(code, found)| failure(code, id, found))?;
    reader.part.begin(reader.inflate, located, id);
    Ok(())
}

/// What [`open`] learns about an XLSX package.
#[derive(Default)]
struct Package {
    workbook: u64,
    rels: u64,
    styles: u64,
    strings: u64,
    content: u64,
    epoch: u32,
}

/// Opens a workbook: checks the container, tells XLSX from ODS by what the container holds
/// (never by a file name), and finds the parts every later call reads. Writes a fresh
/// state; any earlier one in the block is gone.
///
/// Uses `window` and, as scratch, `arena`. On [`ERR_WINDOW`](crate::kernel::abi::ERR_WINDOW)
/// or [`ERR_ARENA`](crate::kernel::abi::ERR_ARENA) the call is made again with that buffer
/// larger, and starts over.
pub fn open(state: &mut State, container: &[u8], memory: &mut Memory<'_>, out: &mut Opened) -> i32 {
    *out = Opened::default();
    state.opened = 0;
    state.op = OP_NONE;
    state.phase = DONE;
    state.workbook = 0;
    state.rels = 0;
    state.styles = 0;
    state.strings = 0;
    state.content = 0;
    state.epoch = ExcelEpoch::Y1900 as u32;
    let directory = match zip::directory(container) {
        Ok(directory) => directory,
        Err(code) => return refuse(failure(code, 0, 0), &mut out.needed, &mut out.failure),
    };
    state.directory = directory;
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: memory.window,
    };
    let mut scratch = Scratch {
        bytes: memory.arena,
        used: 0,
    };
    let is_xlsx = directory.find(container, b"[Content_Types].xml").is_some()
        || directory.find(container, DEFAULT_WORKBOOK).is_some();
    let opened = if is_xlsx {
        open_xlsx(&mut reader, &directory, &mut scratch, out)
    } else {
        open_ods(&mut reader, &directory).map(|content| Package {
            content,
            epoch: ExcelEpoch::Y1900 as u32,
            ..Package::default()
        })
    };
    match opened {
        Ok(package) => {
            state.workbook = package.workbook;
            state.rels = package.rels;
            state.styles = package.styles;
            state.strings = package.strings;
            state.content = package.content;
            state.epoch = package.epoch;
            state.format = if is_xlsx { FORMAT_XLSX } else { FORMAT_ODS };
            state.opened = OPENED;
            out.format = state.format;
            out.epoch = state.epoch;
            OK
        }
        Err(stop) => refuse(stop, &mut out.needed, &mut out.failure),
    }
}

fn open_xlsx(
    reader: &mut Reader<'_>,
    directory: &Directory,
    scratch: &mut Scratch<'_>,
    out: &mut Opened,
) -> Result<Package, Stop> {
    let container = reader.container;
    // The workbook part is whatever the package's own relationships call the office
    // document, and `xl/workbook.xml` if they do not say.
    let mut workbook: Option<Run> = None;
    if let Some(entry) = directory.find(container, b"_rels/.rels") {
        start(reader, &entry, Failure::PART_ROOT_RELS)?;
        loop {
            let token = reader.peek()?;
            if token.kind == Kind::Eof {
                break;
            }
            if matches!(token.kind, Kind::Start | Kind::Empty) && workbook.is_none() {
                let tag = Tag::of(reader.buf(), &token);
                if let Some((_, kind, target)) = relationship(&tag)
                    && kind.ends_with(b"/officeDocument")
                {
                    workbook = Some(scratch.resolve(Ok(b""), target)?);
                }
            }
            reader.take(&token);
        }
    }
    let workbook = match workbook {
        Some(run) => run,
        None => scratch.push(&[DEFAULT_WORKBOOK])?,
    };
    let (dir_len, base_len) = {
        let (dir, base) = dir_of(scratch.at(workbook));
        (dir.len(), base.len())
    };
    let dir: Run = (workbook.0, dir_len);
    let base: Run = (workbook.0 + dir_len, base_len);

    // Its relationships: where the styles and the shared strings are.
    let mut package = Package {
        epoch: ExcelEpoch::Y1900 as u32,
        ..Package::default()
    };
    let (mut styles, mut strings): (Option<Run>, Option<Run>) = (None, None);
    let name = {
        let start = scratch.used;
        scratch.repeat(dir)?;
        scratch.push(&[b"_rels/"])?;
        scratch.repeat(base)?;
        scratch.push(&[b".rels"])?;
        (start, scratch.used - start)
    };
    if let Some(entry) = directory.find(container, scratch.at(name)) {
        package.rels = entry.at as u64 + 1;
        start(reader, &entry, Failure::PART_WORKBOOK_RELS)?;
        loop {
            let token = reader.peek()?;
            if token.kind == Kind::Eof {
                break;
            }
            if matches!(token.kind, Kind::Start | Kind::Empty) {
                let tag = Tag::of(reader.buf(), &token);
                if let Some((_, kind, target)) = relationship(&tag) {
                    if styles.is_none() && kind.ends_with(b"/styles") {
                        styles = Some(scratch.resolve(Err(dir), target)?);
                    } else if strings.is_none() && kind.ends_with(b"/sharedStrings") {
                        strings = Some(scratch.resolve(Err(dir), target)?);
                    }
                }
            }
            reader.take(&token);
        }
    }

    // The workbook part itself: the date system. (Its sheets are `sheets`' to list.)
    let entry = directory
        .find(container, scratch.at(workbook))
        .ok_or(failure(Failure::MISSING_PART, Failure::PART_WORKBOOK, 0))?;
    package.workbook = entry.at as u64 + 1;
    start(reader, &entry, Failure::PART_WORKBOOK)?;
    loop {
        let token = reader.peek()?;
        if token.kind == Kind::Eof {
            break;
        }
        if matches!(token.kind, Kind::Start | Kind::Empty) {
            let tag = Tag::of(reader.buf(), &token);
            if tag.local() == b"workbookPr"
                && tag
                    .attr(b"date1904")
                    .is_some_and(|v| v == b"1" || v.eq_ignore_ascii_case(b"true"))
            {
                package.epoch = ExcelEpoch::Y1904 as u32;
            }
        }
        reader.take(&token);
    }

    // The two tables, where the relationships said or where they usually are. A table
    // that is named and not there is a workbook without one.
    let styles = match styles {
        Some(run) => run,
        None => {
            let start = scratch.used;
            scratch.repeat(dir)?;
            scratch.push(&[b"styles.xml"])?;
            (start, scratch.used - start)
        }
    };
    if let Some(entry) = directory.find(container, scratch.at(styles)) {
        package.styles = entry.at as u64 + 1;
    }
    let strings = match strings {
        Some(run) => run,
        None => {
            let start = scratch.used;
            scratch.repeat(dir)?;
            scratch.push(&[b"sharedStrings.xml"])?;
            (start, scratch.used - start)
        }
    };
    if let Some(entry) = directory.find(container, scratch.at(strings)) {
        package.strings = entry.at as u64 + 1;
        out.strings_bytes = entry.uncompressed;
        // The root element says how many strings there are, when it says.
        start(reader, &entry, Failure::PART_STRINGS)?;
        loop {
            let token = reader.peek()?;
            if matches!(token.kind, Kind::Start | Kind::Empty) {
                let tag = Tag::of(reader.buf(), &token);
                let count = tag.attr(b"uniqueCount").or_else(|| tag.attr(b"count"));
                out.strings_count = u64::from(count.and_then(parse_u32).unwrap_or(0));
                break;
            }
            if token.kind == Kind::Eof {
                break;
            }
            reader.take(&token);
        }
    }
    Ok(package)
}

fn open_ods(reader: &mut Reader<'_>, directory: &Directory) -> Result<u64, Stop> {
    let container = reader.container;
    let not_a_workbook = failure(Failure::NOT_A_WORKBOOK, 0, 0);
    let Some(entry) = directory.find(container, b"mimetype") else {
        return Err(not_a_workbook);
    };
    start(reader, &entry, Failure::PART_MIMETYPE)?;
    while !reader.at_end() {
        reader.more()?;
    }
    if reader.buf().trim_ascii() != MIMETYPE {
        return Err(not_a_workbook);
    }
    if let Some(entry) = directory.find(container, b"META-INF/manifest.xml") {
        // An encrypted document says so in its manifest, anywhere in it.
        const MARK: &[u8] = b"encryption-data";
        start(reader, &entry, Failure::PART_MANIFEST)?;
        loop {
            let buf = reader.buf();
            let from = reader.position();
            if xml::find_seq(buf, from, MARK).is_some() {
                return Err(failure(Failure::ENCRYPTED, Failure::PART_MANIFEST, 0));
            }
            if reader.at_end() {
                break;
            }
            // Everything but a mark's worth of the end has been searched for good.
            let searched = buf.len().saturating_sub(MARK.len() - 1).max(from);
            reader.seek(searched);
            reader.more()?;
        }
    }
    let entry = directory.find(container, CONTENT).ok_or(failure(
        Failure::MISSING_PART,
        Failure::PART_CONTENT,
        0,
    ))?;
    Ok(entry.at as u64 + 1)
}

/// Lists the sheets: one [`SheetInfo`](crate::kernel::abi::SheetInfo) each, as three spans
/// of `cells`, names and part names in `arena`; `out.rows` is how many. XLSX sheets that
/// hold no cells (chart sheets, macro sheets) are left out.
///
/// Uses `window`, `arena` and `cells`, the last two also as scratch while it works.
/// Resumes after a refusal.
pub fn sheets(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED {
        return ERR_CONTRACT;
    }
    let fresh = state.enter(OP_SHEETS);
    let result = sheets_inner(state, container, memory, fresh);
    out.arena_used = state.used;
    match result {
        Ok(()) => {
            state.phase = DONE;
            out.rows = state.count;
            OK
        }
        Err(stop) => {
            if matches!(stop, Stop::Fail(_) | Stop::Contract) {
                state.phase = DONE;
            }
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}

/// The three spans of record `index`.
fn record(cells: &[Span], index: usize) -> [Span; 3] {
    cells
        .get(index.saturating_mul(3)..)
        .and_then(|rest| rest.first_chunk::<3>())
        .copied()
        .unwrap_or_default()
}

fn set_record(cells: &mut [Span], index: usize, value: [Span; 3]) {
    if let Some(slot) = cells
        .get_mut(index.saturating_mul(3)..)
        .and_then(|rest| rest.first_chunk_mut::<3>())
    {
        *slot = value;
    }
}

/// The bytes of a span this module wrote into the arena.
fn text(arena: &[u8], span: Span) -> &[u8] {
    let from = span.offset as usize;
    slice(arena, from, from + span.len())
}

const PHASE_START: u32 = 0;
const PHASE_RELS: u32 = 1;
const PHASE_WORKBOOK: u32 = 2;
const PHASE_CONTENT: u32 = 3;

fn sheets_inner(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    fresh: bool,
) -> Result<(), Stop> {
    let directory = state.directory;
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: memory.window,
    };
    let arena = &mut *memory.arena;
    let cells = &mut *memory.cells;
    if fresh || state.phase == PHASE_START {
        if state.format == FORMAT_ODS {
            begin(
                &mut reader,
                &directory,
                state.content,
                Failure::PART_CONTENT,
            )?;
            state.phase = PHASE_CONTENT;
        } else {
            begin(
                &mut reader,
                &directory,
                state.rels,
                Failure::PART_WORKBOOK_RELS,
            )?;
            state.phase = PHASE_RELS;
        }
    }

    if state.phase == PHASE_CONTENT {
        // ODS: every table of the document is a sheet; a table inside a cell is not.
        //
        // A sheet is hidden by its style, not by an attribute of its own: the automatic
        // styles, which come before the body, hold `style:style style:family="table"`
        // elements, and one whose `style:table-properties` says `table:display="false"`
        // hides every table that names it. So each table style met before the first table
        // is a record ahead of the sheets' — its name, and whether it hides — and the
        // records are sorted by name when the first table arrives, so that a table finds
        // its style by search and not by a walk. The sheets are listed after them and
        // moved to the front at the end, as the XLSX listing moves its own.
        loop {
            let token = reader.peek()?;
            match token.kind {
                Kind::Eof => break,
                Kind::Start | Kind::Empty => {}
                _ => {
                    reader.take(&token);
                    continue;
                }
            }
            let tag = Tag::of(reader.buf(), &token);
            let styles = state.extra as usize;
            if state.count == 0
                && token.kind == Kind::Start
                && tag.local() == b"style"
                && tag.attr(b"family") == Some(&b"table"[..])
            {
                let raw = tag.attr(b"name").unwrap_or_default();
                if cells.len() < (styles + 1) * 3 {
                    return Err(Stop::Cells(
                        ((styles as u64 + 1) * 3).max(cells.len() as u64 * 2),
                    ));
                }
                room(arena.len(), state.used, raw.len() as u64, reader.part)?;
                let mut scratch = Scratch {
                    bytes: &mut *arena,
                    used: state.used as usize,
                };
                let run = scratch.push(&[raw])?;
                let name = Span {
                    offset: run.0 as u32,
                    len: run.1 as u32,
                };
                set_record(cells, styles, [name, Span::default(), Span::default()]);
                state.used = scratch.used as u64;
                state.extra += 1;
                reader.take(&token);
                continue;
            }
            if state.count == 0
                && styles > 0
                && tag.local() == b"table-properties"
                && tag.attr(b"display") == Some(&b"false"[..])
            {
                // The properties of the style listed last: the one this element is inside.
                let [name, part, _] = record(cells, styles - 1);
                let hides = Span { offset: 1, len: 0 };
                set_record(cells, styles - 1, [name, part, hides]);
                reader.take(&token);
                continue;
            }
            if token.kind != Kind::Start || !is_table(&tag) {
                reader.take(&token);
                continue;
            }
            let hidden = {
                let arena = &*arena;
                if state.count == 0 {
                    heap_sort(
                        cells,
                        styles,
                        |cells, a, b| {
                            text(arena, record(cells, a)[0]) < text(arena, record(cells, b)[0])
                        },
                        |cells, a, b| {
                            let (left, right) = (record(cells, a), record(cells, b));
                            set_record(cells, a, right);
                            set_record(cells, b, left);
                        },
                    );
                }
                tag.attr(b"style-name").is_some_and(|wanted| {
                    let at = partition_point(styles, |index| {
                        text(arena, record(cells, index)[0]) < wanted
                    });
                    let [name, _, hides] = record(cells, at);
                    at < styles && text(arena, name) == wanted && hides.offset != 0
                })
            };
            let raw = tag.attr(b"name").unwrap_or_default();
            let count = state.count as usize;
            let slot = styles + count;
            if cells.len() < (slot + 1) * 3 {
                return Err(Stop::Cells(
                    ((slot as u64 + 1) * 3).max(cells.len() as u64 * 2),
                ));
            }
            room(arena.len(), state.used, raw.len() as u64, reader.part)?;
            let written = unescape_into(raw, tail_mut(arena, state.used as usize));
            let name = Span {
                offset: state.used as u32,
                len: written as u32,
            };
            let index = Span {
                offset: u32::from(hidden),
                len: count as u32,
            };
            set_record(cells, slot, [name, Span::default(), index]);
            state.used += written as u64;
            state.count += 1;
            reader.skip_element(&token);
        }
        let styles = state.extra as usize;
        for index in 0..state.count as usize {
            let sheet = record(cells, styles + index);
            set_record(cells, index, sheet);
        }
        return Ok(());
    }

    // XLSX. A sheet names its part by a relationship id, so the relationships come first:
    // one record each (the id, the part it resolves to, its position and whether it is a
    // worksheet), then sorted by id so that finding one is a search and not a walk.
    let workbook = directory.entry(container, state.workbook.wrapping_sub(1));
    let dir = workbook.map_or(&b""[..], |entry| dir_of(unrooted(entry.name)).0);
    if state.phase == PHASE_RELS {
        loop {
            let token = reader.peek()?;
            if token.kind == Kind::Eof {
                break;
            }
            if matches!(token.kind, Kind::Start | Kind::Empty) {
                let tag = Tag::of(reader.buf(), &token);
                if let Some((id, kind, target)) = relationship(&tag) {
                    let count = state.extra as usize;
                    if cells.len() < (count + 1) * 3 {
                        return Err(Stop::Cells(
                            ((count as u64 + 1) * 3).max(cells.len() as u64 * 2),
                        ));
                    }
                    let worksheet = kind.ends_with(b"/worksheet");
                    let need = id.len()
                        + if worksheet {
                            dir.len() + 2 * target.len()
                        } else {
                            0
                        };
                    room(arena.len(), state.used, need as u64, reader.part)?;
                    let mut scratch = Scratch {
                        bytes: &mut *arena,
                        used: state.used as usize,
                    };
                    let id = scratch.push(&[id])?;
                    let part = if worksheet {
                        scratch.resolve(Ok(dir), target)?
                    } else {
                        (0, 0)
                    };
                    let span = |run: (usize, usize)| Span {
                        offset: run.0 as u32,
                        len: run.1 as u32,
                    };
                    let order = Span {
                        offset: count as u32,
                        len: u32::from(worksheet),
                    };
                    set_record(cells, count, [span(id), span(part), order]);
                    state.used = scratch.used as u64;
                    state.extra += 1;
                }
            }
            reader.take(&token);
        }
        let key = |cells: &[Span], index: usize| {
            let [id, _, order] = record(cells, index);
            (id, order.offset)
        };
        let arena = &*arena;
        heap_sort(
            cells,
            state.extra as usize,
            |cells, a, b| {
                let ((left, first), (right, second)) = (key(cells, a), key(cells, b));
                (text(arena, left), first) < (text(arena, right), second)
            },
            |cells, a, b| {
                let (left, right) = (record(cells, a), record(cells, b));
                set_record(cells, a, right);
                set_record(cells, b, left);
            },
        );
        begin(
            &mut reader,
            &directory,
            state.workbook,
            Failure::PART_WORKBOOK,
        )?;
        state.phase = PHASE_WORKBOOK;
    }

    let relationships = state.extra as usize;
    loop {
        let token = reader.peek()?;
        if token.kind == Kind::Eof {
            break;
        }
        if matches!(token.kind, Kind::Start | Kind::Empty) {
            let tag = Tag::of(reader.buf(), &token);
            if tag.local() == b"sheet"
                && let Some(wanted) = tag.attr(b"id")
            {
                // The first relationship with this id, as the file orders them.
                let at = partition_point(relationships, |index| {
                    text(arena, record(cells, index)[0]) < wanted
                });
                let [id, part, order] = record(cells, at);
                if at < relationships && text(arena, id) == wanted && order.len != 0 {
                    let raw = tag.attr(b"name").unwrap_or_default();
                    let hidden = tag
                        .attr(b"state")
                        .is_some_and(|state| state == b"hidden" || state == b"veryHidden");
                    let slot = relationships + state.count as usize;
                    if cells.len() < (slot + 1) * 3 {
                        return Err(Stop::Cells(
                            ((slot as u64 + 1) * 3).max(cells.len() as u64 * 2),
                        ));
                    }
                    room(arena.len(), state.used, raw.len() as u64, reader.part)?;
                    let written = unescape_into(raw, tail_mut(arena, state.used as usize));
                    let name = Span {
                        offset: state.used as u32,
                        len: written as u32,
                    };
                    let flags = Span {
                        offset: u32::from(hidden),
                        len: 0,
                    };
                    set_record(cells, slot, [name, part, flags]);
                    state.used += written as u64;
                    state.count += 1;
                }
            }
        }
        reader.take(&token);
    }
    // The sheets were listed after the relationships; move them to the front.
    for index in 0..state.count as usize {
        let sheet = record(cells, relationships + index);
        set_record(cells, index, sheet);
    }
    Ok(())
}

/// True for a `table:table` start tag.
pub fn is_table(tag: &Tag<'_>) -> bool {
    tag.local() == b"table" && tag.name.ends_with(b"table:table")
}

/// Loads the shared strings (XLSX): each one's text into `arena`, entities decoded and
/// rich-text runs joined (phonetic runs left out), and its span into `cells`; `out.rows`
/// is how many. A workbook without any — every ODS, some XLSX — has zero.
///
/// Uses `window`, `arena` and `cells`. Resumes after a refusal.
pub fn strings(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED {
        return ERR_CONTRACT;
    }
    let fresh = state.enter(OP_STRINGS);
    let result = strings_inner(state, container, memory, fresh);
    out.arena_used = state.used;
    match result {
        Ok(()) => {
            state.phase = DONE;
            out.rows = state.count;
            OK
        }
        Err(stop) => {
            if matches!(stop, Stop::Fail(_) | Stop::Contract) {
                state.phase = DONE;
            }
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}

fn strings_inner(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    fresh: bool,
) -> Result<(), Stop> {
    let directory = state.directory;
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: memory.window,
    };
    let arena = &mut *memory.arena;
    let cells = &mut *memory.cells;
    if fresh {
        begin(
            &mut reader,
            &directory,
            state.strings,
            Failure::PART_STRINGS,
        )?;
        state.phase = 1;
    }
    loop {
        let token = reader.peek()?;
        let buf = reader.buf();
        let too_large = Stop::Fail(reader.part.failure(Failure::TOO_LARGE, 0));
        let mut skip = false;
        // `depth` is zero between strings and counts open elements inside one.
        match (token.kind, state.depth) {
            (Kind::Eof, 0) => return Ok(()),
            (Kind::Eof, _) => return Err(Stop::Fail(reader.part.failure(Failure::XML, 10))),
            (Kind::Start, 0) => {
                if Tag::of(buf, &token).local() == b"si" {
                    state.mark = state.used;
                    state.depth = 1;
                }
            }
            (Kind::Empty, 0) => {
                if Tag::of(buf, &token).local() == b"si" {
                    finish_string(state.count, state.used, state.used, cells, too_large)?;
                    state.count += 1;
                }
            }
            (Kind::Start, _) => {
                if Tag::of(buf, &token).local() == b"rPh" {
                    skip = true;
                } else {
                    state.depth = state.depth.saturating_add(1);
                }
            }
            (Kind::End, 1) => {
                finish_string(state.count, state.mark, state.used, cells, too_large)?;
                state.count += 1;
                state.depth = 0;
            }
            (Kind::End, depth) => state.depth = depth.saturating_sub(1),
            (Kind::Text | Kind::CData, depth) if depth > 0 => {
                let raw = slice(buf, token.a, token.b);
                room(arena.len(), state.used, raw.len() as u64, reader.part)?;
                let out = tail_mut(arena, state.used as usize);
                let written = if token.kind == Kind::Text {
                    unescape_into(raw, out)
                } else {
                    out.get_mut(..raw.len()).map_or(0, |room| {
                        room.copy_from_slice(raw);
                        raw.len()
                    })
                };
                state.used += written as u64;
            }
            _ => {}
        }
        if skip {
            reader.skip_element(&token);
        } else {
            reader.take(&token);
        }
    }
}

/// Records string `count` as `mark..used` of the arena.
fn finish_string(
    count: u64,
    mark: u64,
    used: u64,
    cells: &mut [Span],
    too_large: Stop,
) -> Result<(), Stop> {
    if used - mark >= u64::from(Span::FLAG) {
        return Err(too_large);
    }
    let Some(slot) = cells.get_mut(count as usize) else {
        return Err(Stop::Cells((count + 1).max(cells.len() as u64 * 2)));
    };
    *slot = Span {
        offset: mark as u32,
        len: (used - mark) as u32,
    };
    Ok(())
}

/// Loads the number-format kind of each cell format (XLSX): one byte each into `arena`
/// (`0` a plain number, `1` a date or time, `2` an elapsed time, `3` text); `out.rows` is
/// how many. A workbook without a style table has zero, and every numeric cell in it is
/// a plain number.
///
/// Uses `window`, `arena`, and `cells` as scratch for the custom formats. Resumes after a
/// refusal.
pub fn styles(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED {
        return ERR_CONTRACT;
    }
    let fresh = state.enter(OP_STYLES);
    let result = styles_inner(state, container, memory, fresh);
    match result {
        Ok(()) => {
            state.phase = DONE;
            out.rows = state.count;
            out.arena_used = state.count;
            OK
        }
        Err(stop) => {
            if matches!(stop, Stop::Fail(_) | Stop::Contract) {
                state.phase = DONE;
            }
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}

/// A `numFmt` tag that declares a format: its id and its code.
fn custom_format<'a>(tag: &Tag<'a>) -> Option<(u32, &'a [u8])> {
    if tag.local() != b"numFmt" {
        return None;
    }
    Some((
        tag.attr(b"numFmtId").and_then(parse_u32)?,
        tag.attr(b"formatCode")?,
    ))
}

fn styles_inner(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    fresh: bool,
) -> Result<(), Stop> {
    const COLLECT: u32 = 1;
    const APPLY: u32 = 2;
    let directory = state.directory;
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: memory.window,
    };
    let arena = &mut *memory.arena;
    let cells = &mut *memory.cells;
    if fresh {
        begin(&mut reader, &directory, state.styles, Failure::PART_STYLES)?;
        state.phase = COLLECT;
    }
    // A cell format names its number format by id, and a custom id means what the latest
    // `numFmt` before it said. So the custom formats are gathered first — one record each:
    // the id, and the format's position and kind — and sorted by id and position; then the
    // part is read again, and each cell format finds its number format by search.
    if state.phase == COLLECT {
        loop {
            let token = reader.peek()?;
            if token.kind == Kind::Eof {
                break;
            }
            if matches!(token.kind, Kind::Start | Kind::Empty)
                && let Some((id, code)) = custom_format(&Tag::of(reader.buf(), &token))
            {
                let count = state.extra;
                if count >= 1 << 30 {
                    return Err(Stop::Fail(reader.part.failure(Failure::TOO_LARGE, 0)));
                }
                let Some(slot) = cells.get_mut(count as usize) else {
                    return Err(Stop::Cells((count + 1).max(cells.len() as u64 * 2)));
                };
                room(arena.len(), 0, code.len() as u64, reader.part)?;
                let written = unescape_into(code, arena);
                let kind = styles::classify(slice(arena, 0, written));
                *slot = Span {
                    offset: id,
                    len: (count as u32) << 2 | u32::from(kind),
                };
                state.extra += 1;
            }
            reader.take(&token);
        }
        heap_sort(
            cells,
            state.extra as usize,
            |cells, a, b| {
                let key = |index: usize| {
                    let span = cells.get(index).copied().unwrap_or_default();
                    (span.offset, span.len)
                };
                key(a) < key(b)
            },
            |cells, a, b| {
                if let (Some(&left), Some(&right)) = (cells.get(a), cells.get(b)) {
                    if let Some(slot) = cells.get_mut(a) {
                        *slot = right;
                    }
                    if let Some(slot) = cells.get_mut(b) {
                        *slot = left;
                    }
                }
            },
        );
        begin(&mut reader, &directory, state.styles, Failure::PART_STYLES)?;
        state.phase = APPLY;
    }

    let formats = state.extra as usize;
    loop {
        let token = reader.peek()?;
        let buf = reader.buf();
        match token.kind {
            Kind::Eof => return Ok(()),
            Kind::End => {
                if xml::local_name(slice(buf, token.a, token.b)) == b"cellXfs" {
                    state.flag = 0;
                }
            }
            Kind::Start | Kind::Empty => {
                let tag = Tag::of(buf, &token);
                if custom_format(&tag).is_some() {
                    // `mark` counts the custom formats declared so far.
                    state.mark += 1;
                } else if tag.local() == b"cellXfs" {
                    state.flag = 1;
                } else if tag.local() == b"xf" && state.flag != 0 {
                    let id = tag.attr(b"numFmtId").and_then(parse_u32).unwrap_or(0);
                    let Some(slot) = arena.get_mut(state.count as usize) else {
                        return Err(Stop::Arena((state.count + 1).max(arena.len() as u64 * 2)));
                    };
                    // The last record for this id declared before here.
                    let before = (id, (state.mark.min(1 << 30) as u32) << 2);
                    let at = partition_point(formats, |index| {
                        let span = cells.get(index).copied().unwrap_or_default();
                        (span.offset, span.len) < before
                    });
                    let custom = at
                        .checked_sub(1)
                        .and_then(|index| cells.get(index))
                        .filter(|span| span.offset == id);
                    *slot = match custom {
                        Some(span) => (span.len & 3) as u8,
                        None => styles::builtin(id),
                    };
                    state.count += 1;
                }
            }
            _ => {}
        }
        reader.take(&token);
    }
}
