//! One sheet, read forward: the row readers of the two formats, the cursor that turns
//! what they read into deliveries (empty rows, gaps in the row numbers, ODS's repeated
//! rows), the header, and the fill that casts a batch of rows through the caller's plan.
//!
//! **XLSX** is `<sheetData>` → `<row>` → `<c>`. `r` gives the column (missing ⇒ the one
//! after the last, per ISO 29500), `s` the cell format, `t` the type; `<v>` is read by
//! type and — for numbers — by the format's kind; `<is>` is collected like a shared
//! string; `<f>` is skipped: the cached value is the value.
//!
//! **ODS** types a cell by `office:value-type` and the value attribute paired with it —
//! no style lookup — and the repeat discipline is calamine's: repeated *empty* cells and
//! rows are never materialised beyond what the cursor is asked to deliver.
//!
//! Both readers are written a token at a time, with everything they know between tokens
//! in the state block: a token is looked at, acted on, and only then stepped past, so a
//! call that has to stop for room — in the window, the arena, the span table — stops with
//! nothing half-done and goes on from the same token when called again.

use super::book::is_table;
use super::cell::{self, Cell, Stores, cast, error_code, parse_f64, render, wall_from_iso};
use super::part::{Part, Reader, Stop};
use super::xml::{self, Kind, Tag, Token, parse_u32, slice, unescape_into};
use super::zip;
use super::{
    FORMAT_ODS, FORMAT_XLSX, Memory, OP_ROWS, OPENED, State, begin, failure, refuse, room, styles,
};
use crate::kernel::abi::{
    ColumnBuffer, ColumnSpec, ERR_CELLS, ERR_CONTRACT, ERR_STRUCTURE, Failure, Filled, OK, Slot,
    Span,
};
use crate::kernel::door::Door;
use hypercast::ExcelEpoch;

/// The widest sheet either format allows; an ODS repeat count is believed no further.
const MAX_COLUMNS: u64 = 16_384;

/// A header name that is not there: a column the header row has no cell in.
const ABSENT: Span = Span {
    offset: u32::MAX,
    len: u32::MAX,
};

// Where a row reader is. XLSX:
const X_PREAMBLE: u32 = 0;
const X_ROWS: u32 = 1;
const X_CELLS: u32 = 2;
const X_BODY: u32 = 3;
const X_VALUE: u32 = 4;
const X_RICH: u32 = 5;
// ODS:
const O_SEEK: u32 = 10;
const O_ROWS: u32 = 11;
const O_CELLS: u32 = 12;
const O_TEXT: u32 = 13;

// XLSX cell types (`t`).
const T_NUMBER: u32 = 0;
const T_SHARED: u32 = 1;
const T_STRING: u32 = 2;
const T_BOOL: u32 = 3;
const T_ERROR: u32 = 4;
const T_DATE: u32 = 5;

// ODS value types (`office:value-type`).
const V_NONE: u32 = 0;
const V_FLOAT: u32 = 1;
const V_BOOL: u32 = 2;
const V_DATE: u32 = 3;
const V_TIME: u32 = 4;
const V_STRING: u32 = 5;
const V_ERROR: u32 = 6;

// What the XML ended inside (`Failure::XML`'s `found`, after the tokenizer's own).
const IN_ROW: u32 = 7;
const IN_CELL: u32 = 8;
const IN_VALUE: u32 = 9;
const IN_STRING: u32 = 10;

/// Everything a sheet read keeps between calls. Plain integers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Sheet {
    /// Where the row reader is (`X_*`, `O_*`), and how deep in the element it is reading.
    mode: u32,
    depth: u32,
    /// ODS: which table this sheet is, and how many have been passed.
    table: u32,
    seen: u32,
    skip_empty: u32,
    header_pending: u32,
    done: u32,
    failed: u32,
    /// A fill was refused part-way through a batch, and through writing a row.
    mid: u32,
    writing: u32,
    /// The delivery in hand is an empty row.
    empty_delivery: u32,
    /// A cell lies past the slots the caller gave.
    beyond: u32,
    // Row numbering, as the cursor keeps it.
    next_number: u32,
    number: u32,
    gap: u32,
    repeat: u32,
    gap_left: u32,
    repeat_left: u32,
    current: u32,
    // The cell being read.
    cell_type: u32,
    kind: u32,
    value_type: u32,
    value_present: u32,
    paragraphs: u32,
    col: u64,
    next_col: u64,
    cell_repeat: u64,
    pending: Slot,
    /// The cell's text so far, past the committed end of the arena: a value, then (ODS)
    /// the paragraphs.
    val_start: u64,
    val_len: u64,
    text_len: u64,
    // The row being assembled: slots in use, slots holding a cell, header names in use.
    extent: u64,
    occupied: u64,
    names: u64,
    // The arena: bytes committed, where the stored row's text lies, where a row being
    // written began.
    used: u64,
    row_start: u64,
    row_end: u64,
    mark: u64,
    /// Rows written in the batch in progress.
    rows: u64,
    failure: Failure,
}

/// What to do with the token just handled.
enum Step {
    /// Move past it.
    Take,
    /// Move past it and the element it opens.
    Skip,
    /// Move past it: a row is complete.
    Row,
    /// The sheet has no more rows.
    End,
}

/// A sheet read in progress: the state, and the caller's memory for this call.
struct Run<'a> {
    sheet: &'a mut Sheet,
    arena: &'a mut [u8],
    row: &'a mut [Slot],
    /// The header's names, when a header is what is being read.
    names: Option<&'a mut [Span]>,
    strings: &'a [u8],
    table: &'a [Span],
    kinds: &'a [u8],
    system: ExcelEpoch,
    xlsx: bool,
}

fn broken(part: &Part, inside: u32) -> Stop {
    Stop::Fail(part.failure(Failure::XML, inside))
}

impl Run<'_> {
    /// Starts a row: no cells, and its text begins where the arena now ends.
    fn clear_row(&mut self) {
        let extent = usize::try_from(self.sheet.extent).unwrap_or(usize::MAX);
        for slot in self.row.iter_mut().take(extent) {
            *slot = Slot::EMPTY;
        }
        self.sheet.extent = 0;
        self.sheet.occupied = 0;
        self.sheet.beyond = 0;
        self.sheet.names = 0;
        self.sheet.row_start = self.sheet.used;
    }

    /// Puts `slot` in `count` columns from `col`: in the row's slots as far as the caller
    /// gave any, and — when a header is being read — as text in the name table.
    fn place(&mut self, col: u64, count: u64, slot: Slot, part: &Part) -> Result<(), Stop> {
        let end = col.saturating_add(count);
        let empty = slot.is_empty();
        if let Some(names) = self.names.as_deref_mut() {
            let held = self.sheet.names;
            if empty {
                // Overwriting a cell with none; past the names in use there is nothing to undo.
                for name in names
                    .iter_mut()
                    .take(end.min(held) as usize)
                    .skip(col.min(held) as usize)
                {
                    *name = ABSENT;
                }
            } else {
                if (names.len() as u64) < end {
                    return Err(Stop::Cells(end.max(names.len() as u64 * 2)));
                }
                let name = match slot.span() {
                    Some(span) => span,
                    None => {
                        let text = render(&slot.cell(&[], &[]));
                        let text = text.as_bytes();
                        room(self.arena.len(), self.sheet.used, text.len() as u64, part)?;
                        let mut used = self.sheet.used as usize;
                        let span =
                            cell::append(self.arena, &mut used, text).map_err(Stop::Arena)?;
                        self.sheet.used = used as u64;
                        span
                    }
                };
                // The names from the last one in use to this cell's: nothing up to it,
                // then the cell.
                let from = held.min(col);
                for (index, entry) in names
                    .iter_mut()
                    .enumerate()
                    .take(end as usize)
                    .skip(from as usize)
                {
                    *entry = if index as u64 >= col { name } else { ABSENT };
                }
                self.sheet.names = held.max(end);
            }
        }
        let slots = self.row.len() as u64;
        if !empty && end > slots {
            self.sheet.beyond = 1;
        }
        for at in col..end.min(slots) {
            let extent = self.sheet.extent;
            if at >= extent {
                if empty {
                    break;
                }
                for gap in self.row.iter_mut().take(at as usize).skip(extent as usize) {
                    *gap = Slot::EMPTY;
                }
                self.sheet.extent = at + 1;
            }
            let Some(held) = self.row.get_mut(at as usize) else {
                break;
            };
            let was = at < extent && !held.is_empty();
            *held = slot;
            self.sheet.occupied = self
                .sheet
                .occupied
                .wrapping_add(u64::from(!empty))
                .wrapping_sub(u64::from(was));
        }
        Ok(())
    }

    /// Appends to the text of the cell being read: `raw` with its entities decoded, or as
    /// it is.
    fn gather(
        &mut self,
        raw: &[u8],
        decode: bool,
        paragraphs: bool,
        part: &Part,
    ) -> Result<(), Stop> {
        let sheet = &mut *self.sheet;
        let at = sheet.val_start + sheet.val_len + sheet.text_len;
        room(self.arena.len(), at, raw.len() as u64, part)?;
        if sheet.val_len + sheet.text_len + raw.len() as u64 >= u64::from(Span::FLAG) {
            return Err(Stop::Fail(part.failure(Failure::TOO_LARGE, 0)));
        }
        let out = self.arena.get_mut(at as usize..).unwrap_or_default();
        let written = if decode {
            unescape_into(raw, out)
        } else {
            out.get_mut(..raw.len()).map_or(0, |room| {
                room.copy_from_slice(raw);
                raw.len()
            })
        } as u64;
        if paragraphs {
            sheet.text_len += written;
        } else {
            sheet.val_len += written;
        }
        Ok(())
    }

    /// Appends `count` copies of `byte` to the paragraphs of the cell being read.
    fn pad(&mut self, byte: u8, count: u64, part: &Part) -> Result<(), Stop> {
        let sheet = &mut *self.sheet;
        let at = sheet.val_start + sheet.val_len + sheet.text_len;
        room(self.arena.len(), at, count, part)?;
        if sheet.val_len + sheet.text_len + count >= u64::from(Span::FLAG) {
            return Err(Stop::Fail(part.failure(Failure::TOO_LARGE, 0)));
        }
        for slot in self.arena.iter_mut().skip(at as usize).take(count as usize) {
            *slot = byte;
        }
        sheet.text_len += count;
        Ok(())
    }

    /// Starts gathering a cell's text at the arena's committed end.
    fn begin_text(&mut self) {
        self.sheet.val_start = self.sheet.used;
        self.sheet.val_len = 0;
        self.sheet.text_len = 0;
    }

    /// The gathered value, and the gathered paragraphs.
    fn gathered(&self) -> (&[u8], &[u8]) {
        let start = self.sheet.val_start as usize;
        let middle = start + self.sheet.val_len as usize;
        let end = middle + self.sheet.text_len as usize;
        (
            slice(self.arena, start, middle),
            slice(self.arena, middle, end),
        )
    }

    /// A text cell of `len` gathered bytes starting `from` bytes into what was gathered,
    /// and the arena end that keeps them.
    fn kept(&self, from: u64, len: u64) -> (Slot, u64) {
        let sheet = &*self.sheet;
        let span = Span {
            offset: (sheet.val_start + from) as u32,
            len: len as u32 | Span::FLAG,
        };
        (
            Slot::text(span),
            sheet.val_start + sheet.val_len + sheet.text_len,
        )
    }

    /// What an XLSX `<v>` holds, by the cell's type and its format's kind. Text that is
    /// kept moves the arena's end past it.
    fn value(&mut self, part: &Part) -> Result<Slot, Stop> {
        let (value, _) = self.gathered();
        let lead = (value.len() - value.trim_ascii_start().len()) as u64;
        let value = value.trim_ascii();
        let text = self.kept(lead, value.len() as u64);
        let typed = match self.sheet.cell_type {
            T_SHARED => {
                let Some(index) = parse_u32(value) else {
                    return Err(Stop::Fail(part.failure(Failure::SHARED_STRING, 0)));
                };
                let Some(span) = self.table.get(index as usize) else {
                    let mut failure = part.failure(Failure::SHARED_STRING, index);
                    failure.expected = self.table.len() as u32;
                    return Err(Stop::Fail(failure));
                };
                Some(Slot::text(Span {
                    offset: span.offset,
                    len: span.len & !Span::FLAG,
                }))
            }
            T_STRING => None,
            T_BOOL => match value {
                b"1" => Some(Slot::of(&Cell::Bool(true))),
                b"0" => Some(Slot::of(&Cell::Bool(false))),
                _ => None,
            },
            T_ERROR => error_code(value).map(|code| Slot::of(&Cell::Error(code))),
            T_DATE => wall_from_iso(value).map(|cell| Slot::of(&cell)),
            _ => parse_f64(value).map(|number| {
                let read = match self.sheet.kind as u8 {
                    styles::DATE_TIME => cell::serial(number, self.system, false),
                    styles::ELAPSED => cell::serial(number, self.system, true),
                    _ => None,
                };
                // A non-temporal format, or a serial the rules refuse: keep the number
                // and let the door say what it thinks.
                Slot::of(&read.unwrap_or(Cell::Number(number)))
            }),
        };
        Ok(match typed {
            Some(slot) => slot,
            None => {
                self.sheet.used = text.1;
                text.0
            }
        })
    }

    /// The column, type and format kind an XLSX `<c>` declares.
    fn cell_attrs(&mut self, tag: &Tag<'_>) {
        let sheet = &mut *self.sheet;
        sheet.col = tag.attr(b"r").and_then(column_of).unwrap_or(sheet.next_col);
        sheet.next_col = sheet.col.saturating_add(1);
        sheet.kind = tag
            .attr(b"s")
            .and_then(parse_u32)
            .and_then(|s| self.kinds.get(s as usize))
            .map_or(u32::from(styles::NUMBER), |&kind| u32::from(kind));
        sheet.cell_type = match tag.attr(b"t") {
            None | Some(b"n") => T_NUMBER,
            Some(b"s") => T_SHARED,
            Some(b"b") => T_BOOL,
            Some(b"e") => T_ERROR,
            Some(b"d") => T_DATE,
            // `str`, `inlineStr`, and anything unknown: the text is the value.
            Some(_) => T_STRING,
        };
    }

    /// One token of an XLSX sheet.
    fn xlsx(&mut self, token: &Token, buf: &[u8], part: &Part) -> Result<Step, Stop> {
        let name = slice(buf, token.a, token.b);
        let local = xml::local_name(name);
        let opens = matches!(token.kind, Kind::Start | Kind::Empty);
        match self.sheet.mode {
            X_PREAMBLE => match token.kind {
                Kind::Start if local == b"sheetData" => self.sheet.mode = X_ROWS,
                Kind::Empty if local == b"sheetData" => return Ok(Step::End),
                Kind::Eof => return Ok(Step::End),
                _ => {}
            },
            X_ROWS => match token.kind {
                _ if opens && local == b"row" => {
                    let tag = Tag::of(buf, token);
                    let sheet = &mut *self.sheet;
                    let number = tag
                        .attr(b"r")
                        .and_then(parse_u32)
                        .filter(|&number| number >= sheet.next_number)
                        .unwrap_or(sheet.next_number);
                    sheet.number = number;
                    sheet.gap = number - sheet.next_number;
                    sheet.repeat = 1;
                    sheet.next_number = number.wrapping_add(1);
                    sheet.next_col = 0;
                    self.clear_row();
                    if token.kind == Kind::Empty {
                        return Ok(Step::Row);
                    }
                    self.sheet.mode = X_CELLS;
                }
                Kind::End if local == b"sheetData" => return Ok(Step::End),
                Kind::Eof => return Ok(Step::End),
                // Something other than a row inside sheetData: skip it whole.
                Kind::Start => return Ok(Step::Skip),
                _ => {}
            },
            X_CELLS => match token.kind {
                Kind::Start if local == b"c" => {
                    self.cell_attrs(&Tag::of(buf, token));
                    self.sheet.pending = Slot::EMPTY;
                    self.sheet.mode = X_BODY;
                }
                Kind::Empty if local == b"c" => {
                    let before = (self.sheet.col, self.sheet.next_col);
                    self.cell_attrs(&Tag::of(buf, token));
                    if let Err(stop) = self.place(self.sheet.col, 1, Slot::EMPTY, part) {
                        (self.sheet.col, self.sheet.next_col) = before;
                        return Err(stop);
                    }
                }
                Kind::Start => return Ok(Step::Skip),
                Kind::End if local == b"row" => {
                    self.sheet.mode = X_ROWS;
                    return Ok(Step::Row);
                }
                Kind::Eof => return Err(broken(part, IN_ROW)),
                _ => {}
            },
            X_BODY => match token.kind {
                Kind::Start if local == b"v" || local == b"is" => {
                    self.begin_text();
                    self.sheet.depth = 1;
                    self.sheet.mode = if local == b"v" { X_VALUE } else { X_RICH };
                }
                Kind::Start => return Ok(Step::Skip),
                Kind::End if local == b"c" => {
                    self.place(self.sheet.col, 1, self.sheet.pending, part)?;
                    self.sheet.mode = X_CELLS;
                }
                Kind::Eof => return Err(broken(part, IN_CELL)),
                _ => {}
            },
            // `<v>` and `<is>`: the text inside, whatever elements wrap it — but a
            // phonetic run in a rich string is not part of it.
            _ => match token.kind {
                Kind::Text => self.gather(name, true, false, part)?,
                Kind::CData => self.gather(name, false, false, part)?,
                Kind::Start if self.sheet.mode == X_RICH && local == b"rPh" => {
                    return Ok(Step::Skip);
                }
                Kind::Start => self.sheet.depth = self.sheet.depth.saturating_add(1),
                Kind::End if self.sheet.depth > 1 => self.sheet.depth -= 1,
                Kind::End => {
                    self.sheet.pending = if self.sheet.mode == X_VALUE {
                        self.value(part)?
                    } else {
                        let (slot, used) = self.kept(0, self.sheet.val_len);
                        self.sheet.used = used;
                        slot
                    };
                    self.sheet.mode = X_BODY;
                }
                Kind::Empty => {}
                Kind::Eof => {
                    let inside = if self.sheet.mode == X_VALUE {
                        IN_VALUE
                    } else {
                        IN_STRING
                    };
                    return Err(broken(part, inside));
                }
            },
        }
        Ok(Step::Take)
    }

    /// The repeat count, type and value an ODS cell's tag declares; the value is gathered.
    fn ods_attrs(&mut self, tag: &Tag<'_>, part: &Part) -> Result<(), Stop> {
        let repeat = tag
            .attr(b"number-columns-repeated")
            .and_then(parse_u32)
            .unwrap_or(1)
            .max(1);
        let declared = tag
            .attr_qualified(b"office:value-type")
            .or_else(|| tag.attr(b"value-type"));
        // LibreOffice marks an error cell `office:value-type="string"` *and*
        // `calcext:value-type="error"`; the extension is the truth there.
        let value_type = if tag.attr_qualified(b"calcext:value-type") == Some(b"error") {
            V_ERROR
        } else {
            match declared {
                Some(b"float") | Some(b"percentage") | Some(b"currency") => V_FLOAT,
                Some(b"boolean") => V_BOOL,
                Some(b"date") => V_DATE,
                Some(b"time") => V_TIME,
                Some(b"error") => V_ERROR,
                Some(_) => V_STRING,
                None => V_NONE,
            }
        };
        let raw = match value_type {
            V_FLOAT => tag.attr(b"value"),
            V_BOOL => tag.attr(b"boolean-value"),
            V_DATE => tag.attr(b"date-value"),
            V_TIME => tag.attr(b"time-value"),
            V_STRING => tag.attr(b"string-value"),
            _ => None,
        };
        self.begin_text();
        if let Some(raw) = raw {
            self.gather(raw, true, false, part)?;
        }
        let sheet = &mut *self.sheet;
        sheet.cell_repeat = u64::from(repeat);
        sheet.value_type = value_type;
        sheet.value_present = u32::from(raw.is_some());
        sheet.paragraphs = 0;
        Ok(())
    }

    /// Ends an ODS cell: what it holds, by its declared type, placed in every column it
    /// repeats over.
    fn ods_finish(&mut self, part: &Part) -> Result<(), Stop> {
        let (value, text) = self.gathered();
        let value_len = value.len() as u64;
        let keep_value = self.kept(0, value_len);
        let keep_text = self.kept(value_len, text.len() as u64);
        let typed = |cell: Option<Cell<'_>>| cell.map(|cell| Slot::of(&cell));
        let (slot, used) = match self.sheet.value_type {
            V_FLOAT => (typed(parse_f64(value).map(Cell::Number)), keep_value),
            V_BOOL => (
                match value.trim_ascii() {
                    b"true" => typed(Some(Cell::Bool(true))),
                    b"false" => typed(Some(Cell::Bool(false))),
                    _ => None,
                },
                keep_value,
            ),
            V_DATE => (typed(wall_from_iso(value)), keep_value),
            V_TIME => (
                typed(hypercast::cast_duration(value).ok().map(Cell::Span)),
                keep_value,
            ),
            V_STRING if self.sheet.value_present != 0 => (None, keep_value),
            V_ERROR => (
                typed(error_code(text.trim_ascii()).map(Cell::Error)),
                keep_text,
            ),
            V_NONE if text.is_empty() => (Some(Slot::EMPTY), keep_text),
            _ => (None, keep_text),
        };
        let (slot, used) = match slot {
            Some(slot) => (slot, self.sheet.used),
            None => used,
        };
        let col = self.sheet.col;
        if !slot.is_empty() && col < MAX_COLUMNS {
            let count = self.sheet.cell_repeat.min(MAX_COLUMNS - col);
            let before = self.sheet.used;
            self.sheet.used = used;
            if let Err(stop) = self.place(col, count, slot, part) {
                self.sheet.used = before;
                return Err(stop);
            }
        }
        self.sheet.col = col.saturating_add(self.sheet.cell_repeat);
        Ok(())
    }

    /// One token of an ODS sheet.
    fn ods(&mut self, token: &Token, buf: &[u8], part: &Part) -> Result<Step, Stop> {
        let name = slice(buf, token.a, token.b);
        let local = xml::local_name(name);
        let opens = matches!(token.kind, Kind::Start | Kind::Empty);
        let table = token.kind == Kind::Start && is_table(&Tag::of(buf, token));
        match self.sheet.mode {
            O_SEEK => match token.kind {
                Kind::Start if table => {
                    if self.sheet.seen != self.sheet.table {
                        self.sheet.seen += 1;
                        return Ok(Step::Skip);
                    }
                    self.sheet.mode = O_ROWS;
                }
                Kind::Eof => return Ok(Step::End),
                _ => {}
            },
            O_ROWS => match token.kind {
                _ if opens && local == b"table-row" => {
                    let repeat = Tag::of(buf, token)
                        .attr(b"number-rows-repeated")
                        .and_then(parse_u32)
                        .unwrap_or(1)
                        .max(1);
                    let sheet = &mut *self.sheet;
                    sheet.number = sheet.next_number;
                    sheet.gap = 0;
                    sheet.repeat = repeat;
                    sheet.next_number = sheet.next_number.saturating_add(repeat);
                    sheet.col = 0;
                    self.clear_row();
                    if token.kind == Kind::Empty {
                        return Ok(Step::Row);
                    }
                    self.sheet.mode = O_CELLS;
                }
                // A table inside this one is not rows of it.
                Kind::Start if table => return Ok(Step::Skip),
                // Row groups are read through; a page break holds nothing.
                Kind::Start
                    if matches!(
                        local,
                        b"table-header-rows"
                            | b"table-rows"
                            | b"table-row-group"
                            | b"soft-page-break"
                    ) => {}
                Kind::Start => return Ok(Step::Skip),
                Kind::End if local == b"table" => return Ok(Step::End),
                Kind::Eof => return Ok(Step::End),
                _ => {}
            },
            O_CELLS => match token.kind {
                _ if opens && matches!(local, b"table-cell" | b"covered-table-cell") => {
                    self.ods_attrs(&Tag::of(buf, token), part)?;
                    if token.kind == Kind::Empty {
                        self.ods_finish(part)?;
                    } else {
                        self.sheet.depth = 1;
                        self.sheet.mode = O_TEXT;
                    }
                }
                Kind::Start => return Ok(Step::Skip),
                Kind::End if local == b"table-row" => {
                    self.sheet.mode = O_ROWS;
                    return Ok(Step::Row);
                }
                Kind::Eof => return Err(broken(part, IN_ROW)),
                _ => {}
            },
            // A cell's content: its paragraphs' text, a line feed between two of them.
            _ => match token.kind {
                Kind::Start | Kind::Empty => {
                    if token.kind == Kind::Start
                        && (name.starts_with(b"draw:")
                            || name.starts_with(b"office:annotation")
                            || table)
                    {
                        return Ok(Step::Skip);
                    }
                    match local {
                        b"p" => {
                            if self.sheet.paragraphs > 0 {
                                self.pad(b'\n', 1, part)?;
                            }
                            self.sheet.paragraphs = self.sheet.paragraphs.saturating_add(1);
                        }
                        b"s" => {
                            let count = Tag::of(buf, token)
                                .attr(b"c")
                                .and_then(parse_u32)
                                .unwrap_or(1);
                            self.pad(b' ', u64::from(count), part)?;
                        }
                        b"tab" => self.pad(b'\t', 1, part)?,
                        b"line-break" => self.pad(b'\n', 1, part)?,
                        _ => {}
                    }
                    if token.kind == Kind::Start {
                        self.sheet.depth = self.sheet.depth.saturating_add(1);
                    }
                }
                Kind::End if self.sheet.depth > 1 => self.sheet.depth -= 1,
                Kind::End => {
                    self.ods_finish(part)?;
                    self.sheet.mode = O_CELLS;
                }
                Kind::Text => self.gather(name, true, true, part)?,
                Kind::CData => self.gather(name, false, true, part)?,
                Kind::Eof => return Err(broken(part, IN_CELL)),
            },
        }
        Ok(Step::Take)
    }

    /// Reads the next row into the slots. False when the sheet has no more.
    fn read_row(&mut self, reader: &mut Reader<'_>) -> Result<bool, Stop> {
        loop {
            let token = reader.peek()?;
            let step = if self.xlsx {
                self.xlsx(&token, reader.buf(), reader.part)?
            } else {
                self.ods(&token, reader.buf(), reader.part)?
            };
            match step {
                Step::Take => reader.take(&token),
                Step::Skip => reader.skip_element(&token),
                Step::Row => {
                    reader.take(&token);
                    return Ok(true);
                }
                Step::End => return Ok(false),
            }
        }
    }

    /// Moves to the next row to deliver: an empty one out of a gap, the stored row once
    /// more out of a repeat, or a row newly read. False at the end of the sheet.
    fn deliver(&mut self, reader: &mut Reader<'_>) -> Result<bool, Stop> {
        loop {
            let sheet = &mut *self.sheet;
            if sheet.gap_left > 0 || sheet.repeat_left > 0 {
                sheet.empty_delivery = u32::from(sheet.gap_left > 0);
                if sheet.gap_left > 0 {
                    sheet.gap_left -= 1;
                } else {
                    sheet.repeat_left -= 1;
                }
                sheet.current = sheet.current.wrapping_add(1);
                return Ok(true);
            }
            if sheet.done != 0 {
                return Ok(false);
            }
            if !self.read_row(reader)? {
                self.sheet.done = 1;
                return Ok(false);
            }
            let sheet = &mut *self.sheet;
            let blank = sheet.occupied == 0 && sheet.beyond == 0;
            let mut gap = sheet.gap;
            let mut repeat = sheet.repeat.max(1);
            if blank {
                // An explicit empty row is just more gap.
                gap = gap.wrapping_add(repeat);
                repeat = 0;
            }
            let skip = sheet.skip_empty != 0;
            if skip {
                gap = 0;
            }
            // The first delivery is row `number - gap` when gaps are delivered; `current`
            // is stepped on each delivery.
            sheet.current = if skip {
                sheet.number.wrapping_sub(1)
            } else {
                sheet.number.wrapping_sub(sheet.gap).wrapping_sub(1)
            };
            sheet.gap_left = gap;
            sheet.repeat_left = repeat;
            sheet.row_end = sheet.used;
        }
    }

    /// Casts the delivery in hand through the plan into batch row `sheet.rows`.
    ///
    /// # Safety
    /// Every column's arrays have room for `sheet.rows + 1` elements.
    unsafe fn write(
        &mut self,
        specs: &[ColumnSpec],
        columns: &[ColumnBuffer],
        cells: &mut [Span],
        part: &Part,
    ) -> Result<(), Stop> {
        let at = self.sheet.rows as usize;
        let base = at.saturating_mul(specs.len() + 1);
        let mut used = self.sheet.used as usize;
        let empty = self.sheet.empty_delivery != 0;
        for (index, (spec, buffer)) in specs.iter().zip(columns).enumerate() {
            let Some(door) = Door::from_code(spec.door, spec.param) else {
                return Err(Stop::Contract);
            };
            let ordinal = u64::from(spec.ordinal);
            let slot = if empty || ordinal >= self.sheet.extent {
                Slot::EMPTY
            } else {
                self.row
                    .get(ordinal as usize)
                    .copied()
                    .unwrap_or(Slot::EMPTY)
            };
            let mut stores = Stores {
                arena: &mut *self.arena,
                used: &mut used,
                strings: self.strings,
            };
            // SAFETY: the caller's contract.
            let raw = unsafe { cast(&slot, door, spec, self.system, buffer, at, &mut stores) }
                .map_err(|needed| {
                    if needed > u64::from(u32::MAX) {
                        Stop::Fail(part.failure(Failure::TOO_LARGE, 0))
                    } else {
                        Stop::Arena(needed.max(self.arena.len() as u64 * 2))
                    }
                })?;
            if let Some(entry) = cells.get_mut(base + index) {
                *entry = raw;
            }
        }
        if let Some(entry) = cells.get_mut(base + specs.len()) {
            *entry = Span {
                offset: self.sheet.current,
                len: 0,
            };
        }
        self.sheet.used = used as u64;
        Ok(())
    }
}

/// The zero-based column of a cell reference (`BC12` ⇒ 54): its leading letters, three at
/// most.
fn column_of(reference: &[u8]) -> Option<u64> {
    let mut column = 0u64;
    let mut letters = 0;
    for &b in reference {
        if !b.is_ascii_alphabetic() {
            break;
        }
        column = column * 26 + u64::from(b.to_ascii_uppercase() - b'A' + 1);
        letters += 1;
        if letters > 3 {
            return None;
        }
    }
    column.checked_sub(1)
}

/// A structural failure, with the row it was found in; the sheet keeps it.
fn fail(sheet: &mut Sheet, stop: Stop) -> Stop {
    if let Stop::Fail(mut failure) = stop {
        failure.line = sheet.number;
        sheet.failed = 1;
        sheet.failure = failure;
        return Stop::Fail(failure);
    }
    stop
}

/// Positions a read at the start of a sheet: for XLSX the part `sheets` named it by, for
/// ODS the table at `index`. `has_header` makes the first row delivered the header, which
/// [`header`] then has to be asked for before any [`fill`]; `skip_empty_rows` leaves out
/// rows with no cell in them, and the rows a gap in the numbering stands for.
///
/// Uses no buffer. This state block now reads this sheet; one that is reading another
/// sheet is a different block.
pub fn sheet(
    state: &mut State,
    container: &[u8],
    part: &[u8],
    index: u32,
    has_header: bool,
    skip_empty_rows: bool,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED {
        return ERR_CONTRACT;
    }
    state.op = OP_ROWS;
    state.phase = 0;
    let xlsx = state.format == FORMAT_XLSX;
    state.rows = Sheet {
        mode: if xlsx { X_PREAMBLE } else { O_SEEK },
        table: index,
        skip_empty: u32::from(skip_empty_rows),
        header_pending: u32::from(has_header),
        next_number: 1,
        ..Sheet::default()
    };
    let directory = state.directory;
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: &mut [],
    };
    let positioned = if state.format == FORMAT_ODS {
        begin(
            &mut reader,
            &directory,
            state.content,
            Failure::PART_CONTENT,
        )
    } else {
        match directory.find(container, part) {
            Some(entry) => zip::locate(container, &entry)
                .map(|located| {
                    reader
                        .part
                        .begin(reader.inflate, located, Failure::PART_SHEET)
                })
                .map_err(|(code, found)| failure(code, Failure::PART_SHEET, found)),
            None => Err(failure(Failure::MISSING_PART, Failure::PART_SHEET, 0)),
        }
    };
    match positioned {
        Ok(()) => OK,
        Err(stop) => {
            let stop = fail(&mut state.rows, stop);
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}

/// Reads the header: the first row delivered, each cell as text — a typed cell said the
/// way the text door says it — one span of `cells` per column up to the last that holds a
/// cell; `out.rows` is how many. A span with its flag set is in `arena`, one without in
/// the shared strings. A sheet opened without a header has none, and neither has an empty
/// sheet.
///
/// Uses `window`, `arena`, `cells`, `row` and the workbook's tables. Resumes after a
/// refusal.
pub fn header(
    state: &mut State,
    container: &[u8],
    memory: &mut Memory<'_>,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED || state.op != OP_ROWS {
        return ERR_CONTRACT;
    }
    if state.rows.failed != 0 {
        out.failure = state.rows.failure;
        return ERR_STRUCTURE;
    }
    if state.rows.header_pending == 0 {
        return OK;
    }
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: &mut *memory.window,
    };
    let mut run = Run {
        sheet: &mut state.rows,
        arena: &mut *memory.arena,
        row: &mut *memory.row,
        names: Some(&mut *memory.cells),
        strings: memory.strings,
        table: memory.table,
        kinds: memory.kinds,
        system: if state.epoch == ExcelEpoch::Y1904 as u32 {
            ExcelEpoch::Y1904
        } else {
            ExcelEpoch::Y1900
        },
        xlsx: state.format == FORMAT_XLSX,
    };
    let delivered = run.deliver(&mut reader);
    let sheet = &mut state.rows;
    out.arena_used = sheet.used;
    match delivered {
        Ok(delivered) => {
            let mut count = 0usize;
            if delivered && sheet.empty_delivery == 0 {
                count = (sheet.names as usize).min(memory.cells.len());
                while count > 0 && memory.cells.get(count - 1) == Some(&ABSENT) {
                    count -= 1;
                }
                for name in memory.cells.iter_mut().take(count) {
                    if *name == ABSENT {
                        *name = Span::default();
                    }
                }
            }
            sheet.header_pending = 0;
            out.rows = count as u64;
            OK
        }
        Err(stop) => {
            let stop = fail(sheet, stop);
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}

/// Reads up to `max_rows` rows through `specs` into `columns`: values and verdicts in the
/// layout a delimited batch has, so one reader of a batch reads both.
///
/// `cells` holds, for batch row `r`, one span per plan column at `r * (count + 1)` — the
/// cell's text, where it has any: a text cell's own bytes, and what a typed cell was said
/// as if it failed its door or went through the text door — and after them an entry whose
/// `offset` is the row's number in the sheet. A text value and a cell's span say where
/// their bytes are by the flag: set, in `arena`; clear, in the shared strings.
///
/// `out.rows` short of `max_rows` is the end of the sheet. Broken data is
/// [`ERR_STRUCTURE`] with the rows read before it in `out.rows`, and again on every call
/// after. Uses every buffer; `cells` wants `max_rows * (count + 1)` spans from the start,
/// and `row` a slot for every ordinal in the plan. Resumes after a refusal, given the
/// same plan and columns.
///
/// # Safety
/// For every column, `values` has room for `max_rows` values of its door's type and
/// `verdicts` for `max_rows` verdicts.
pub unsafe fn fill(
    state: &mut State,
    container: &[u8],
    specs: &[ColumnSpec],
    columns: &[ColumnBuffer],
    max_rows: usize,
    memory: &mut Memory<'_>,
    out: &mut Filled,
) -> i32 {
    *out = Filled::default();
    if state.opened != OPENED
        || state.op != OP_ROWS
        || specs.len() != columns.len()
        || state.rows.header_pending != 0
    {
        return ERR_CONTRACT;
    }
    for (spec, buffer) in specs.iter().zip(columns) {
        if Door::from_code(spec.door, spec.param).is_none()
            || spec.num_format().is_none()
            || spec.ordinal as usize >= memory.row.len()
            || (max_rows > 0 && (buffer.values.is_null() || buffer.verdicts.is_null()))
        {
            return ERR_CONTRACT;
        }
    }
    if state.rows.failed != 0 {
        out.failure = state.rows.failure;
        return ERR_STRUCTURE;
    }
    let wanted = max_rows.saturating_mul(specs.len() + 1);
    if memory.cells.len() < wanted {
        out.needed = wanted as u64;
        return ERR_CELLS;
    }
    let system = state.system();
    let mut reader = Reader {
        part: &mut state.part,
        inflate: &mut state.inflate,
        container,
        window: &mut *memory.window,
    };
    let sheet = &mut state.rows;
    if sheet.mid == 0 {
        // A new batch: the arena starts over, but for the text of a row that is still to
        // be delivered again, which moves to the front.
        let (from, to) = (sheet.row_start as usize, sheet.row_end as usize);
        if sheet.repeat_left > 0 && from < to && to <= memory.arena.len() {
            for at in from..to {
                if let Some(&byte) = memory.arena.get(at)
                    && let Some(slot) = memory.arena.get_mut(at - from)
                {
                    *slot = byte;
                }
            }
            for slot in memory.row.iter_mut().take(sheet.extent as usize) {
                if let Some(span) = slot.span().filter(|span| span.flagged()) {
                    *slot = Slot::text(Span {
                        offset: span.offset.wrapping_sub(from as u32),
                        len: span.len,
                    });
                }
            }
            sheet.used = (to - from) as u64;
        } else {
            sheet.used = 0;
        }
        sheet.row_start = 0;
        sheet.row_end = sheet.used;
        sheet.rows = 0;
        sheet.mid = 1;
    }
    let mut run = Run {
        sheet,
        arena: &mut *memory.arena,
        row: &mut *memory.row,
        names: None,
        strings: memory.strings,
        table: memory.table,
        kinds: memory.kinds,
        system,
        xlsx: state.format == FORMAT_XLSX,
    };
    let result = loop {
        if run.sheet.rows as usize >= max_rows {
            break Ok(());
        }
        if run.sheet.writing == 0 {
            match run.deliver(&mut reader) {
                Ok(true) => {
                    run.sheet.writing = 1;
                    run.sheet.mark = run.sheet.used;
                }
                Ok(false) => break Ok(()),
                Err(stop) => break Err(stop),
            }
        } else {
            // A row whose write was refused is written again from its first column.
            run.sheet.used = run.sheet.mark;
        }
        // SAFETY: the caller's contract; `rows` is below `max_rows`.
        match unsafe { run.write(specs, columns, memory.cells, reader.part) } {
            Ok(()) => {
                run.sheet.writing = 0;
                run.sheet.rows += 1;
            }
            Err(stop) => break Err(stop),
        }
    };
    let sheet = &mut state.rows;
    out.arena_used = sheet.used;
    out.consumed = state.part.offset();
    match result {
        Ok(()) => {
            sheet.mid = 0;
            out.rows = sheet.rows;
            OK
        }
        Err(stop) => {
            let stop = fail(sheet, stop);
            if matches!(stop, Stop::Fail(_) | Stop::Contract) {
                // The rows before the break are whole, and the batch is over.
                sheet.mid = 0;
                out.rows = sheet.rows;
            }
            refuse(stop, &mut out.needed, &mut out.failure)
        }
    }
}
