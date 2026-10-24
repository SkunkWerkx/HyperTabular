//! Apple Numbers: the document, its sheets and their tables, read out of the IWA objects
//! [`iwa`] hands over.
//!
//! A Numbers document is `TN.DocumentArchive` (object 1 of `Index/Document.iwa`) → its
//! `TN.SheetArchive`s → each sheet's drawables, of which the `TST.TableInfoArchive`s are
//! tables → each one's `TST.TableModelArchive`, whose data store names the tables the
//! cells index — strings, rich text, formats — and the tiles that hold the cells, 256 rows
//! each. A sheet here is one Numbers *table*: that is what has rows and columns, and what
//! Numbers itself makes a worksheet of when it exports to Excel.
//!
//! Every table a cell indexes is loaded by [`strings`] into the one strings buffer the
//! caller hands back to every sheet read, as records of two spans — a key (the table's
//! ordinal and the kind of record, then the entry's key) and a value — sorted so that a
//! cell finds its string, its rich text, its date format's kind or its tile by search.
//! `styles` has nothing to load. A listing or a loading that is refused for room starts
//! over when it is called again: nothing it writes is read before it is done, and the
//! chunks the window holds are kept by the loader's cache, so what starting over costs is
//! bounded by how often a buffer has to grow.
//!
//! Cells are the v5 storage Numbers has written since 2018: a version byte, a type byte, a
//! flag word that says which fields follow, then the fields in flag order — a 128-bit
//! decimal, a double, seconds since 2001-01-01 — and the identifiers of the entries the
//! cell names in its table's lists.

use super::cell::{Cell, parse_f64};
use super::iwa::{self, BROKEN_MESSAGE, Package, STORAGE_VERSION, resolve};
use super::part::{Part, Stop};
use super::proto::{self, Fields};
use super::rows::Run;
use super::sort::{heap_sort, partition_point};
use super::styles;
use super::{Memory, State, failure, room};
use crate::kernel::abi::{Failure, Opened, Slot, Span};
use core::ops::Range;
use hypercast::{Duration, Timestamp};

// Message types.
const DOCUMENT: u32 = 1;
const SHEET: u32 = 2;
const CUSTOM_FORMATS: u32 = 222;
const TABLE_INFO: u32 = 6000;
const TABLE_MODEL: u32 = 6001;
const TILE: u32 = 6002;

/// The widest a table is read.
const MAX_COLUMNS: usize = 16_384;
/// Rows a tile holds unless its storage says otherwise.
const DEFAULT_TILE_SIZE: u32 = 256;

// Cell types (`TST.CellType`).
const C_GENERIC: u8 = 0;
const C_SPAN: u8 = 1;
const C_NUMBER: u8 = 2;
const C_TEXT: u8 = 3;
const C_DATE: u8 = 5;
const C_BOOL: u8 = 6;
const C_DURATION: u8 = 7;
const C_ERROR: u8 = 8;
const C_RICH: u8 = 9;
const C_CURRENCY: u8 = 10;

/// Seconds from the Unix epoch to Numbers' (2001-01-01T00:00:00Z).
const EPOCH_2001: f64 = 978_307_200.0;

/// A format's `format_type` that is a date.
const FORMAT_DATE: u64 = 261;
const FORMAT_CUSTOM_DATE: u64 = 272;

// What a record in the strings table is, in the low bits of its key's `offset`; the table's
// ordinal is above them.
const KIND_BITS: u32 = 3;
const KIND_STRING: u32 = 0;
const KIND_RICH: u32 = 1;
const KIND_TILE: u32 = 2;
const KIND_FORMAT: u32 = 3;
const KIND_TABLE: u32 = 4;

/// The error every Numbers error cell is said as. Numbers records which formula failed, not
/// which error it made, in anything this core reads.
const ERROR_CODE: u8 = 0x0F;

fn ns(table: u32, kind: u32) -> u32 {
    table << KIND_BITS | kind
}

fn broken(offset: u64) -> Stop {
    let mut stop = failure(Failure::IWA, Failure::PART_IWA, BROKEN_MESSAGE);
    if let Stop::Fail(what) = &mut stop {
        what.byte = offset;
    }
    stop
}

/// The `n`th field numbered `number` of `payload`.
fn nth(payload: &[u8], number: u32, n: usize) -> Option<proto::Field<'_>> {
    Fields::of(payload)
        .filter(|field| field.number == number)
        .nth(n)
}

fn count(payload: &[u8], number: u32) -> usize {
    Fields::of(payload)
        .filter(|field| field.number == number)
        .count()
}

/// The lookups the walks make, each a fetch of one object and a read of one thing in it.
impl Package<'_> {
    /// The object's type.
    fn kind_at(&mut self, at: u64, offset: u64) -> Result<u32, Stop> {
        Ok(self.object(at, offset)?.kind)
    }

    /// How many fields numbered `number` the object's message has.
    fn count_at(&mut self, at: u64, offset: u64, number: u32) -> Result<usize, Stop> {
        let object = self.object(at, offset)?;
        if !proto::well_formed(object.payload) {
            return Err(broken(offset));
        }
        Ok(count(object.payload, number))
    }

    /// The `n`th field numbered `number`, as a reference's identifier.
    fn ref_at(&mut self, at: u64, offset: u64, number: u32, n: usize) -> Result<Option<u64>, Stop> {
        let object = self.object(at, offset)?;
        Ok(nth(object.payload, number, n).and_then(|field| field.reference()))
    }

    /// The first field numbered `number`, as an integer.
    fn int_at(&mut self, at: u64, offset: u64, number: u32, default: u64) -> Result<u64, Stop> {
        let object = self.object(at, offset)?;
        Ok(proto::integer(object.payload, number, default))
    }

    /// A reference inside the table model's data store (field 4 of the model).
    fn store_ref(&mut self, at: u64, offset: u64, number: u32) -> Result<Option<u64>, Stop> {
        let object = self.object(at, offset)?;
        Ok(proto::reference(proto::bytes(object.payload, 4), number))
    }

    /// The tile storage of a table model: how many tiles, and the rows a tile holds.
    fn tile_storage(&mut self, at: u64, offset: u64) -> Result<(usize, u32), Stop> {
        let object = self.object(at, offset)?;
        let tiles = proto::bytes(proto::bytes(object.payload, 4), 3);
        if !proto::well_formed(tiles) {
            return Err(broken(offset));
        }
        let size = proto::integer(tiles, 2, u64::from(DEFAULT_TILE_SIZE));
        Ok((count(tiles, 1), size.clamp(1, u64::from(u32::MAX)) as u32))
    }

    /// The `n`th tile of a table model: its id and the identifier of its object.
    fn tile_at(&mut self, at: u64, offset: u64, n: usize) -> Result<Option<(u32, u64)>, Stop> {
        let object = self.object(at, offset)?;
        let tiles = proto::bytes(proto::bytes(object.payload, 4), 3);
        let Some(tile) = nth(tiles, 1, n) else {
            return Ok(None);
        };
        let id = proto::integer(tile.bytes, 1, 0) as u32;
        Ok(proto::reference(tile.bytes, 2).map(|reference| (id, reference)))
    }

    /// The next entry of a data list (or a segment of one) from byte `pos` of its message
    /// on, and where that entry's bytes lie in the message; `pos` moves past it, so that a
    /// walk over a list of three hundred thousand strings reads each byte once rather than
    /// scanning from the start for every entry. An entry is its key, the rich-text payload
    /// it names, the kind of its format if that is a date format, and the uid of the custom
    /// format its format names, if it names one.
    fn entry_at(
        &mut self,
        at: u64,
        offset: u64,
        pos: &mut usize,
    ) -> Result<(Entry, Range<usize>), Stop> {
        let object = self.object(at, offset)?;
        let mut fields = Fields::from(object.payload, *pos);
        let Some(entry) = fields.find(|field| field.number == 3) else {
            return Err(broken(offset));
        };
        *pos = fields.position();
        if !proto::well_formed(entry.bytes) {
            return Err(broken(offset));
        }
        let format = proto::bytes(entry.bytes, 6);
        Ok((
            Entry {
                id: proto::integer(entry.bytes, 1, 0) as u32,
                payload: proto::reference(entry.bytes, 9),
                date: date_kind(format),
                custom: proto::first(format, 41).map(|uid| uuid(uid.bytes)),
            },
            *pos - entry.bytes.len()..*pos,
        ))
    }

    /// Where the document keeps its custom formats: `TN.DocumentArchive` field 8 is its
    /// `TSA.DocumentArchive`, whose field 12 names the `TSK.CustomFormatListArchive`.
    fn custom_formats(&mut self, at: u64, offset: u64) -> Result<Option<u64>, Stop> {
        let object = self.object(at, offset)?;
        Ok(proto::reference(proto::bytes(object.payload, 8), 12))
    }

    /// The kind of the custom format `uid` names in the list at `(at, offset)`: the uuids
    /// (field 1) and the formats (field 2) are two lists matched by position, and the
    /// format's default (field 3) is the struct that carries its pattern. `None` for a uid
    /// the list does not have.
    fn custom_kind(&mut self, at: u64, offset: u64, uid: (u64, u64)) -> Result<Option<u8>, Stop> {
        let object = self.object(at, offset)?;
        if object.kind != CUSTOM_FORMATS || !proto::well_formed(object.payload) {
            return Err(broken(offset));
        }
        let Some(n) = Fields::of(object.payload)
            .filter(|field| field.number == 1)
            .position(|field| uuid(field.bytes) == uid)
        else {
            return Ok(None);
        };
        Ok(nth(object.payload, 2, n).map(|format| date_kind(proto::bytes(format.bytes, 3))))
    }

    /// Appends to the arena every field numbered `number` of the object at `place` (a
    /// part and a stream offset) — of the list entry whose bytes `entry` locates in the
    /// message, when it names one — and returns the run appended.
    fn copy_at(
        &mut self,
        place: (u64, u64),
        entry: Option<Range<usize>>,
        number: u32,
        arena: &mut [u8],
        used: &mut u64,
        part: &Part,
    ) -> Result<Span, Stop> {
        let object = self.object(place.0, place.1)?;
        let message = match entry {
            Some(range) => object.payload.get(range).unwrap_or(&[]),
            None => object.payload,
        };
        let start = *used;
        for field in Fields::of(message).filter(|field| field.number == number) {
            room(arena.len(), *used, field.bytes.len() as u64, part)?;
            if let Some(slot) = arena.get_mut(*used as usize..*used as usize + field.bytes.len()) {
                slot.copy_from_slice(field.bytes);
            }
            *used += field.bytes.len() as u64;
        }
        Ok(Span {
            offset: start as u32,
            len: (*used - start) as u32,
        })
    }
}

/// One entry of a data list, as [`Package::entry_at`] reads it.
struct Entry {
    id: u32,
    payload: Option<u64>,
    date: u8,
    custom: Option<(u64, u64)>,
}

/// A `TSP.UUID`: its two halves.
fn uuid(message: &[u8]) -> (u64, u64) {
    (proto::integer(message, 1, 0), proto::integer(message, 2, 0))
}

/// The kind a cell with this format reads its seconds as: a time of day when the format
/// shows no date, a wall clock otherwise; a plain number for a format that is not a date's.
/// A format that names a custom format by uid carries no pattern of its own and reads as
/// a date here; the caller looks the pattern up in the document's list.
fn date_kind(format: &[u8]) -> u8 {
    let kind = proto::integer(format, 1, 0);
    if kind != FORMAT_DATE && kind != FORMAT_CUSTOM_DATE {
        return styles::NUMBER;
    }
    if proto::integer(format, 12, 0) != 0 {
        return styles::TIME;
    }
    // The pattern is Unicode's (`yyyy-MM-dd H:mm:ss`), in field 14 for a built-in format
    // and field 18 for a custom one: any date letter outside a quoted literal shows a date;
    // only time letters, a time; neither, a date by default.
    let mut pattern = proto::bytes(format, 14);
    if pattern.is_empty() {
        pattern = proto::bytes(format, 18);
    }
    let mut quoted = false;
    let mut timed = false;
    for &byte in pattern {
        if byte == b'\'' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if b"GyYuUQqMLwWdDFgEec".contains(&byte) {
            return styles::DATE_TIME;
        }
        if b"ahHkKmsSAzZvVOXx".contains(&byte) {
            timed = true;
        }
    }
    if timed {
        styles::TIME
    } else {
        styles::DATE_TIME
    }
}

/// Decides whether the container is a Numbers document: `Index/Document.iwa` holds an
/// object 1 that is a `TN.DocumentArchive` whose sheets are `TN.SheetArchive`s. Fills
/// in what `open` reports and returns the document part's central-header offset plus one.
pub fn open(package: &mut Package<'_>, out: &mut Opened) -> Result<u64, Stop> {
    let not_a_workbook = failure(Failure::NOT_A_WORKBOOK, 0, 0);
    let Some(document) = package
        .directory
        .find(package.container, b"Index/Document.iwa")
    else {
        return Err(not_a_workbook);
    };
    let at = document.at as u64;
    let Some((_, offset)) = package.find(at, 1)? else {
        return Err(not_a_workbook);
    };
    if package.kind_at(at, offset)? != DOCUMENT {
        return Err(not_a_workbook);
    }
    let sheets = package.count_at(at, offset, 1)?;
    for n in 0..sheets {
        let Some(identifier) = package.ref_at(at, offset, 1, n)? else {
            return Err(broken(offset));
        };
        let Some((sheet_at, sheet_offset)) = package.find(at, identifier)? else {
            return Err(iwa::missing(identifier));
        };
        if package.kind_at(sheet_at, sheet_offset)? != SHEET {
            return Err(not_a_workbook);
        }
    }
    // The strings live in the data lists: what those parts decode to bounds them.
    let mut ordinal = 0usize;
    while let Some(part) = package.part(ordinal)? {
        ordinal += 1;
        let entry = package.entry(part)?;
        if entry.name.starts_with(b"Index/Tables/DataList") {
            let bytes = package.stream_len(part)?;
            out.strings_bytes = out.strings_bytes.saturating_add(bytes);
        }
    }
    Ok(at + 1)
}

/// One table of the document, as the walks visit them in order: the sheet it is on and
/// where its model is.
struct Table {
    sheet_at: u64,
    sheet_offset: u64,
    /// How many tables the sheet has.
    siblings: usize,
    model_at: u64,
    model_offset: u64,
}

/// Visits every table of every sheet, in the document's order, calling `f` with each and
/// its ordinal. `cells` is the object index, of `objects` entries.
fn each_table(
    package: &mut Package<'_>,
    cells: &[Span],
    objects: usize,
    mut f: impl FnMut(&mut Package<'_>, usize, &Table) -> Result<(), Stop>,
) -> Result<(), Stop> {
    let (at, offset) = resolve(cells, objects, 1).ok_or(iwa::missing(1))?;
    let sheets = package.count_at(at, offset, 1)?;
    let mut ordinal = 0usize;
    for sheet in 0..sheets {
        let Some(identifier) = package.ref_at(at, offset, 1, sheet)? else {
            return Err(broken(offset));
        };
        let (sheet_at, sheet_offset) =
            resolve(cells, objects, identifier).ok_or(iwa::missing(identifier))?;
        if package.kind_at(sheet_at, sheet_offset)? != SHEET {
            return Err(broken(sheet_offset));
        }
        let drawables = package.count_at(sheet_at, sheet_offset, 2)?;
        // Which drawables are tables, and where their models are: twice over, so that each
        // table knows how many it shares the sheet with.
        let mut siblings = 0usize;
        for pass in 0..2 {
            for drawable in 0..drawables {
                let Some(identifier) = package.ref_at(sheet_at, sheet_offset, 2, drawable)? else {
                    return Err(broken(sheet_offset));
                };
                let Some((info_at, info_offset)) = resolve(cells, objects, identifier) else {
                    // A drawable the package lost is not a table.
                    continue;
                };
                if package.kind_at(info_at, info_offset)? != TABLE_INFO {
                    continue;
                }
                if pass == 0 {
                    siblings += 1;
                    continue;
                }
                let Some(model) = package.ref_at(info_at, info_offset, 2, 0)? else {
                    return Err(broken(info_offset));
                };
                let (model_at, model_offset) =
                    resolve(cells, objects, model).ok_or(iwa::missing(model))?;
                if package.kind_at(model_at, model_offset)? != TABLE_MODEL {
                    return Err(broken(model_offset));
                }
                let table = Table {
                    sheet_at,
                    sheet_offset,
                    siblings,
                    model_at,
                    model_offset,
                };
                f(package, ordinal, &table)?;
                ordinal += 1;
            }
        }
    }
    Ok(())
}

/// The listing: one record of three spans per table — its name (the sheet's, and the
/// table's after a dash when the sheet has more than one), an empty part, and the hidden
/// flag with the table's ordinal. Starts over when called again.
pub fn sheets(state: &mut State, container: &[u8], memory: &mut Memory<'_>) -> Result<(), Stop> {
    state.count = 0;
    state.used = 0;
    let part = state.part;
    let mut package = Package {
        container,
        directory: state.directory,
        inflate: &mut state.inflate,
        window: &mut *memory.window,
        cache: &mut state.cache,
    };
    let cells = &mut *memory.cells;
    let arena = &mut *memory.arena;
    let objects = package.index(cells)?;
    let base = objects * 2;
    let (mut count, mut used) = (0usize, 0u64);
    let (index_cells, records) = cells.split_at_mut(base.min(cells.len()));
    each_table(
        &mut package,
        index_cells,
        objects,
        |package, ordinal, table| {
            let Some(slot) = records
                .get_mut(count * 3..)
                .and_then(|rest| rest.first_chunk_mut::<3>())
            else {
                return Err(Stop::Cells(
                    (base as u64 + (count as u64 + 1) * 3).max(cells_len(base, records) * 2),
                ));
            };
            let start = used;
            package.copy_at(
                (table.sheet_at, table.sheet_offset),
                None,
                1,
                arena,
                &mut used,
                &part,
            )?;
            if table.siblings > 1 {
                room(arena.len(), used, 3, &part)?;
                if let Some(room) = arena.get_mut(used as usize..used as usize + 3) {
                    room.copy_from_slice(b" - ");
                }
                used += 3;
                package.copy_at(
                    (table.model_at, table.model_offset),
                    None,
                    8,
                    arena,
                    &mut used,
                    &part,
                )?;
            }
            let hidden = package.int_at(table.sheet_at, table.sheet_offset, 25, 0)? != 0;
            *slot = [
                Span {
                    offset: start as u32,
                    len: (used - start) as u32,
                },
                Span::default(),
                Span {
                    offset: u32::from(hidden),
                    len: ordinal as u32,
                },
            ];
            count += 1;
            Ok(())
        },
    )?;
    // The records were listed after the index; move them to the front.
    for index in 0..count * 3 {
        let span = cells.get(base + index).copied().unwrap_or_default();
        if let Some(slot) = cells.get_mut(index) {
            *slot = span;
        }
    }
    state.count = count as u64;
    state.used = used;
    Ok(())
}

fn cells_len(base: usize, records: &[Span]) -> u64 {
    (base + records.len()) as u64
}

/// Writes record `count` — a key and a value — after the index.
fn emit(
    records: &mut [Span],
    base: usize,
    count: &mut usize,
    key: Span,
    value: Span,
) -> Result<(), Stop> {
    let Some(slot) = records
        .get_mut(*count * 2..)
        .and_then(|rest| rest.first_chunk_mut::<2>())
    else {
        return Err(Stop::Cells(
            (base as u64 + (*count as u64 + 1) * 2).max(cells_len(base, records) * 2),
        ));
    };
    *slot = [key, value];
    *count += 1;
    Ok(())
}

/// Loads every table a cell indexes, for every table of the document, as sorted records of
/// two spans in `cells` with the text in `arena`: strings and rich text (the value is the
/// text's span), the tiles (the value is where the tile's object is), each date format
/// that shows no date (the value is [`styles::TIME`]), and each table's tile size. Starts
/// over when called again.
pub fn strings(state: &mut State, container: &[u8], memory: &mut Memory<'_>) -> Result<(), Stop> {
    state.count = 0;
    state.used = 0;
    let part = state.part;
    let mut package = Package {
        container,
        directory: state.directory,
        inflate: &mut state.inflate,
        window: &mut *memory.window,
        cache: &mut state.cache,
    };
    let cells = &mut *memory.cells;
    let arena = &mut *memory.arena;
    let objects = package.index(cells)?;
    let base = objects * 2;
    let (mut count, mut used) = (0usize, 0u64);
    let (index_cells, records) = cells.split_at_mut(base.min(cells.len()));
    let document = resolve(index_cells, objects, 1).ok_or(iwa::missing(1))?;
    each_table(
        &mut package,
        index_cells,
        objects,
        |package, ordinal, table| {
            let table_ns = ordinal as u32;
            let (model_at, model_offset) = (table.model_at, table.model_offset);
            let key = |kind: u32, id: u32| Span {
                offset: ns(table_ns, kind),
                len: id,
            };

            // The tiles, and how many rows each holds.
            let (tiles, tile_size) = package.tile_storage(model_at, model_offset)?;
            emit(
                records,
                base,
                &mut count,
                key(KIND_TABLE, 0),
                Span {
                    offset: tile_size,
                    len: 0,
                },
            )?;
            for n in 0..tiles {
                let Some((id, reference)) = package.tile_at(model_at, model_offset, n)? else {
                    return Err(broken(model_offset));
                };
                let (at, offset) =
                    resolve(index_cells, objects, reference).ok_or(iwa::missing(reference))?;
                emit(
                    records,
                    base,
                    &mut count,
                    key(KIND_TILE, id),
                    Span {
                        offset: at as u32,
                        len: offset as u32,
                    },
                )?;
            }

            // The three lists: strings (field 4 of the store), rich text (17), formats (22).
            for (field, kind) in [(4, KIND_STRING), (17, KIND_RICH), (22, KIND_FORMAT)] {
                let Some(list) = package.store_ref(model_at, model_offset, field)? else {
                    continue;
                };
                let (list_at, list_offset) =
                    resolve(index_cells, objects, list).ok_or(iwa::missing(list))?;
                // The list's own entries, then those of each segment.
                let segments = package.count_at(list_at, list_offset, 4)?;
                for segment in 0..=segments {
                    let (at, offset) = if segment == 0 {
                        (list_at, list_offset)
                    } else {
                        let Some(identifier) =
                            package.ref_at(list_at, list_offset, 4, segment - 1)?
                        else {
                            return Err(broken(list_offset));
                        };
                        resolve(index_cells, objects, identifier).ok_or(iwa::missing(identifier))?
                    };
                    let entries = package.count_at(at, offset, 3)?;
                    let mut pos = 0usize;
                    for _ in 0..entries {
                        let (
                            Entry {
                                id,
                                payload,
                                mut date,
                                custom,
                            },
                            range,
                        ) = package.entry_at(at, offset, &mut pos)?;
                        // A custom format's pattern lives in the document's list, under
                        // the uid the format names; a uid the list does not have, or a
                        // document without a list, leaves the format read as it stands.
                        if kind == KIND_FORMAT
                            && let Some(uid) = custom
                            && let Some(list) = package.custom_formats(document.0, document.1)?
                        {
                            let (list_at, list_offset) =
                                resolve(index_cells, objects, list).ok_or(iwa::missing(list))?;
                            if let Some(found) = package.custom_kind(list_at, list_offset, uid)? {
                                date = found;
                            }
                        }
                        let value = match kind {
                            KIND_STRING => package.copy_at(
                                (at, offset),
                                Some(range),
                                3,
                                arena,
                                &mut used,
                                &part,
                            )?,
                            KIND_RICH => {
                                // The payload names a storage, whose text runs are the string.
                                let Some(payload) = payload else {
                                    continue;
                                };
                                let (payload_at, payload_offset) =
                                    resolve(index_cells, objects, payload)
                                        .ok_or(iwa::missing(payload))?;
                                let Some(storage) =
                                    package.ref_at(payload_at, payload_offset, 1, 0)?
                                else {
                                    return Err(broken(payload_offset));
                                };
                                let (storage_at, storage_offset) =
                                    resolve(index_cells, objects, storage)
                                        .ok_or(iwa::missing(storage))?;
                                package.copy_at(
                                    (storage_at, storage_offset),
                                    None,
                                    3,
                                    arena,
                                    &mut used,
                                    &part,
                                )?
                            }
                            _ => {
                                if date != styles::TIME {
                                    continue;
                                }
                                Span {
                                    offset: u32::from(date),
                                    len: 0,
                                }
                            }
                        };
                        emit(records, base, &mut count, key(kind, id), value)?;
                    }
                }
            }
            Ok(())
        },
    )?;
    // Sort the records by key and move them to the front.
    let key = |cells: &[Span], index: usize| {
        let span = cells.get(index * 2).copied().unwrap_or_default();
        (span.offset, span.len)
    };
    heap_sort(
        records,
        count,
        |cells, a, b| key(cells, a) < key(cells, b),
        |cells, a, b| {
            let left = cells
                .get(a * 2..)
                .and_then(|rest| rest.first_chunk::<2>())
                .copied()
                .unwrap_or_default();
            let right = cells
                .get(b * 2..)
                .and_then(|rest| rest.first_chunk::<2>())
                .copied()
                .unwrap_or_default();
            if let Some(slot) = cells
                .get_mut(a * 2..)
                .and_then(|rest| rest.first_chunk_mut::<2>())
            {
                *slot = right;
            }
            if let Some(slot) = cells
                .get_mut(b * 2..)
                .and_then(|rest| rest.first_chunk_mut::<2>())
            {
                *slot = left;
            }
        },
    );
    for index in 0..count * 2 {
        let span = cells.get(base + index).copied().unwrap_or_default();
        if let Some(slot) = cells.get_mut(index) {
            *slot = span;
        }
    }
    state.count = count as u64 * 2;
    state.used = used;
    Ok(())
}

/// The value of the record keyed `(ns, id)` in a strings table, if there is one.
fn lookup(table: &[Span], ns: u32, id: u32) -> Option<Span> {
    let records = table.len() / 2;
    let key = |index: usize| {
        let span = table.get(index * 2).copied().unwrap_or_default();
        (span.offset, span.len)
    };
    let at = partition_point(records, |index| key(index) < (ns, id));
    if at >= records || key(at) != (ns, id) {
        return None;
    }
    table.get(at * 2 + 1).copied()
}

/// The `n`th tile record of table `table`: its id, and where its object is.
fn tile(table_spans: &[Span], table: u32, n: usize) -> Option<(u32, u64, u64)> {
    let records = table_spans.len() / 2;
    let wanted = ns(table, KIND_TILE);
    let key = |index: usize| {
        let span = table_spans.get(index * 2).copied().unwrap_or_default();
        (span.offset, span.len)
    };
    let first = partition_point(records, |index| key(index) < (wanted, 0));
    let at = first.checked_add(n)?;
    if at >= records || key(at).0 != wanted {
        return None;
    }
    let value = table_spans.get(at * 2 + 1).copied()?;
    Some((key(at).1, u64::from(value.offset), u64::from(value.len)))
}

/// A 128-bit decimal (IEEE 754 binary integer decimal) as the double nearest it: the
/// mantissa's digits and the exponent, read back through the one grammar the core reads
/// numbers in.
fn decimal128(bytes: &[u8; 16]) -> f64 {
    let high = bytes[15];
    if high & 0x78 == 0x78 {
        // An infinity or a NaN.
        return f64::NAN;
    }
    let exponent = (i32::from(high & 0x7F) << 7 | i32::from(bytes[14] >> 1)) - 0x1820;
    let mut mantissa = u128::from(bytes[14] & 1);
    for &byte in bytes.iter().take(14).rev() {
        mantissa = mantissa << 8 | u128::from(byte);
    }
    if mantissa == 0 {
        return 0.0;
    }
    // A sign, up to 39 digits, `E`, a sign and up to 4 digits; the digits of each number
    // come out backwards and are written last first.
    let mut text = [0u8; 48];
    let mut len = 0usize;
    let mut push = |byte: u8| {
        if let Some(slot) = text.get_mut(len) {
            *slot = byte;
            len += 1;
        }
    };
    if high & 0x80 != 0 {
        push(b'-');
    }
    let mut digits = [0u8; 40];
    let mut n = 0usize;
    while mantissa > 0 {
        if let Some(slot) = digits.get_mut(n) {
            *slot = b'0' + (mantissa % 10) as u8;
        }
        mantissa /= 10;
        n += 1;
    }
    for &digit in digits.iter().take(n).rev() {
        push(digit);
    }
    if exponent != 0 {
        push(b'E');
        if exponent < 0 {
            push(b'-');
        }
        let mut magnitude = exponent.unsigned_abs();
        let mut digits = [0u8; 10];
        let mut n = 0usize;
        while magnitude > 0 {
            if let Some(slot) = digits.get_mut(n) {
                *slot = b'0' + (magnitude % 10) as u8;
            }
            magnitude /= 10;
            n += 1;
        }
        for &digit in digits.iter().take(n).rev() {
            push(digit);
        }
    }
    parse_f64(text.get(..len).unwrap_or_default()).unwrap_or(f64::NAN)
}

/// `value` split into whole seconds and nanoseconds, the whole part rounded toward
/// negative infinity (`floor`), as `core` alone can do it. `None` for a value that is not
/// finite or does not fit.
fn split_seconds(value: f64) -> Option<(i64, i32)> {
    if !value.is_finite() || value >= 9.2e18 || value <= -9.2e18 {
        return None;
    }
    let mut whole = value as i64;
    let mut fraction = value - whole as f64;
    if fraction < 0.0 {
        whole -= 1;
        fraction += 1.0;
    }
    let mut nanos = (fraction * 1e9 + 0.5) as i64;
    if nanos >= 1_000_000_000 {
        nanos -= 1_000_000_000;
        whole = whole.checked_add(1)?;
    }
    Some((whole, nanos as i32))
}

/// A span of `seconds`, or `None` for one HyperCast's duration cannot hold.
fn duration(seconds: f64) -> Option<Duration> {
    const LIMIT: f64 = 315_576_000_000.0;
    if !(-LIMIT..=LIMIT).contains(&seconds) {
        return None;
    }
    let (whole, nanos) = split_seconds(seconds)?;
    // A duration keeps the sign on both parts.
    if whole < 0 && nanos > 0 {
        return Some(Duration {
            seconds: whole + 1,
            nanos: nanos - 1_000_000_000,
        });
    }
    Some(Duration {
        seconds: whole,
        nanos,
    })
}

/// The wall clock `seconds` since 2001 is, in UTC, or `None` for an instant with no date.
fn wall(seconds: f64) -> Option<Cell<'static>> {
    let (whole, nanos) = split_seconds(seconds + EPOCH_2001)?;
    let civil = Timestamp {
        seconds: whole,
        nanos,
    }
    .utc_civil()?;
    Some(Cell::Wall {
        date: civil.date,
        nanos: civil.nanos_of_day,
    })
}

/// A v5 cell storage, read into a slot: the typed value, or a text cell's span into the
/// strings. Empty for a cell with nothing in it, and for the cells a merged cell spans.
fn cell(bytes: &[u8], table: &[Span], table_ns: u32, part: &Part) -> Result<Slot, Stop> {
    let Some(&version) = bytes.first() else {
        return Ok(Slot::EMPTY);
    };
    if version != 5 {
        let mut failure = part.failure(Failure::IWA, STORAGE_VERSION);
        failure.expected = u32::from(version);
        return Err(Stop::Fail(failure));
    }
    let cut = || Stop::Fail(part.failure(Failure::IWA, BROKEN_MESSAGE));
    let kind = *bytes.get(1).ok_or_else(cut)?;
    let flags = u32::from_le_bytes(
        *bytes
            .get(8..12)
            .and_then(|b| b.first_chunk::<4>())
            .ok_or_else(cut)?,
    );
    let mut pos = 12usize;
    let mut decimal: Option<f64> = None;
    let mut double: Option<f64> = None;
    let mut seconds: Option<f64> = None;
    if flags & 0x1 != 0 {
        let raw = bytes
            .get(pos..pos + 16)
            .and_then(|b| b.first_chunk::<16>())
            .ok_or_else(cut)?;
        decimal = Some(decimal128(raw));
        pos += 16;
    }
    for (flag, slot) in [(0x2u32, &mut double), (0x4, &mut seconds)] {
        if flags & flag != 0 {
            let raw = bytes
                .get(pos..pos + 8)
                .and_then(|b| b.first_chunk::<8>())
                .ok_or_else(cut)?;
            *slot = Some(f64::from_le_bytes(*raw));
            pos += 8;
        }
    }
    // The identifiers, four bytes each, in flag order from 0x8 up: the string, the rich
    // text, four styles, the formula, the control, the error, the suggestion, then the
    // formats — number, currency, date, duration, text, boolean — and the comment.
    let mut ids = [None::<u32>; 17];
    for (index, slot) in ids.iter_mut().enumerate() {
        if flags & (0x8 << index) != 0 {
            let raw = bytes
                .get(pos..pos + 4)
                .and_then(|b| b.first_chunk::<4>())
                .ok_or_else(cut)?;
            *slot = Some(u32::from_le_bytes(*raw));
            pos += 4;
        }
    }
    let string_id = ids[0];
    let rich_id = ids[1];
    let date_format = ids[12];
    let text = |kind: u32, id: Option<u32>| -> Result<Slot, Stop> {
        let Some(id) = id else {
            return Ok(Slot::text(Span::default()));
        };
        match lookup(table, ns(table_ns, kind), id) {
            Some(span) => Ok(Slot::text(Span {
                offset: span.offset,
                len: span.len & !Span::FLAG,
            })),
            None => Err(Stop::Fail(part.failure(Failure::SHARED_STRING, id))),
        }
    };
    let number = |value: Option<f64>| Slot::of(&value.map_or(Cell::Empty, Cell::Number));
    Ok(match kind {
        C_GENERIC | C_SPAN => Slot::EMPTY,
        C_NUMBER | C_CURRENCY => number(decimal.or(double)),
        C_TEXT => text(KIND_STRING, string_id)?,
        C_RICH => text(KIND_RICH, rich_id)?,
        C_DATE => {
            let Some(seconds) = seconds else {
                return Ok(Slot::EMPTY);
            };
            let timed = date_format
                .and_then(|id| lookup(table, ns(table_ns, KIND_FORMAT), id))
                .is_some_and(|value| value.offset == u32::from(styles::TIME));
            match wall(seconds) {
                Some(Cell::Wall { nanos, .. }) if timed => Slot::of(&Cell::Clock(nanos)),
                Some(cell) => Slot::of(&cell),
                None => Slot::of(&Cell::Number(seconds)),
            }
        }
        C_BOOL => Slot::of(&Cell::Bool(double.is_some_and(|value| value > 0.0))),
        C_DURATION => {
            let Some(seconds) = double else {
                return Ok(Slot::EMPTY);
            };
            Slot::of(&duration(seconds).map_or(Cell::Number(seconds), Cell::Span))
        }
        C_ERROR => Slot::of(&Cell::Error(ERROR_CODE)),
        // A type this core does not know: whatever value it carries.
        _ => {
            if string_id.is_some() {
                text(KIND_STRING, string_id)?
            } else if rich_id.is_some() {
                text(KIND_RICH, rich_id)?
            } else if let Some(seconds) = seconds {
                Slot::of(&wall(seconds).unwrap_or(Cell::Number(seconds)))
            } else {
                number(decimal.or(double))
            }
        }
    })
}

/// Reads the next row of the table into the slots: the next row of the tile in hand, or
/// the first of the next tile. False when the table has no more. Re-entrant at a cell:
/// a row half placed when a refusal stops it is finished from the cell it stopped at.
pub(super) fn read_row(
    run: &mut Run<'_>,
    package: &mut Package<'_>,
    part: &Part,
) -> Result<bool, Stop> {
    loop {
        let sheet = &mut *run.sheet;
        if sheet.n_located == 0 {
            let Some((id, at, offset)) = tile(run.table, sheet.table, sheet.n_tile as usize) else {
                return Ok(false);
            };
            if sheet.n_tile == 0 {
                sheet.n_tile_size = lookup(run.table, ns(sheet.table, KIND_TABLE), 0)
                    .map_or(DEFAULT_TILE_SIZE, |value| value.offset.max(1));
            }
            sheet.n_tileid = id;
            sheet.n_at = at;
            sheet.n_offset = offset;
            sheet.n_pos = 0;
            sheet.n_located = 1;
        }
        let object = package.object(sheet.n_at, sheet.n_offset)?;
        if object.kind != TILE {
            return Err(broken(sheet.n_offset));
        }
        let mut fields = Fields::from(object.payload, sheet.n_pos as usize);
        let Some(info) = fields.find(|field| field.number == 5) else {
            if fields.broken() {
                return Err(broken(sheet.n_offset));
            }
            // The tile is read: on to the next.
            sheet.n_tile = sheet.n_tile.saturating_add(1);
            sheet.n_located = 0;
            sheet.n_mid = 0;
            continue;
        };
        let after = fields.position() as u64;
        if !proto::well_formed(info.bytes) {
            return Err(broken(sheet.n_offset));
        }
        let offsets = proto::bytes(info.bytes, 7);
        let storage = proto::bytes(info.bytes, 6);
        if offsets.is_empty() && storage.is_empty() && proto::first(info.bytes, 4).is_some() {
            // Saved before the layout this core reads.
            let mut failure = part.failure(Failure::IWA, STORAGE_VERSION);
            failure.expected = proto::integer(info.bytes, 5, 0) as u32;
            return Err(Stop::Fail(failure));
        }
        let wide = proto::integer(info.bytes, 8, 0) != 0;
        if sheet.n_mid == 0 {
            let index = proto::integer(info.bytes, 1, 0);
            let number = u64::from(sheet.n_tileid)
                .saturating_mul(u64::from(sheet.n_tile_size))
                .saturating_add(index)
                .saturating_add(1)
                .min(u64::from(u32::MAX)) as u32;
            let number = number.max(sheet.next_number);
            sheet.number = number;
            sheet.gap = number - sheet.next_number;
            sheet.repeat = 1;
            sheet.next_number = number.wrapping_add(1);
            sheet.col = 0;
            run.clear_row();
            run.sheet.n_mid = 1;
        }
        let columns = (offsets.len() / 2).min(MAX_COLUMNS);
        while (run.sheet.col as usize) < columns {
            let col = run.sheet.col as usize;
            let raw = offsets
                .get(col * 2..col * 2 + 2)
                .and_then(|b| b.first_chunk::<2>())
                .map_or(-1, |b| i16::from_le_bytes(*b));
            if raw >= 0 {
                let start = usize::from(raw as u16) * if wide { 4 } else { 1 };
                let bytes = storage.get(start..).unwrap_or_default();
                let slot = cell(bytes, run.table, run.sheet.table, part)?;
                if !slot.is_empty() {
                    run.place(col as u64, 1, slot, part)?;
                }
            }
            run.sheet.col = col as u64 + 1;
        }
        run.sheet.n_pos = after;
        run.sheet.n_mid = 0;
        return Ok(true);
    }
}
