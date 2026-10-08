//! The Ruby binding's fast backend: the gem's Fiddle crossing written again in Rust over the
//! core's own functions, and linked with the core into one Magnus extension
//! (`hypertabular_native`) — the PyO3 play, run for Ruby, as HyperCast's extension is.
//!
//! The gem keeps every native call behind three objects in `ruby/lib/hypertabular/runtime`:
//! `Runtime::Delimited` (a delimited read), `Runtime::Book::Opened` (an open workbook) and
//! `Runtime::Book::Reading` (one sheet being read), plus `Runtime.unescape`. Everything above
//! them — the readers, the workbook, `Batch` and every value and verdict it builds — is plain
//! Ruby that never sees a pointer. So this extension replaces exactly those, **in place**: on
//! require it redefines the constructors (`Delimited.start`, `Opened.new`, `Reading.new`),
//! `Delimited.version` and `Runtime.unescape` to hand back objects of its own, which answer
//! the same methods with the same bytes. The Ruby above them stays shared byte for byte, so
//! the two backends cannot disagree about what a cell means; what changes is that no Fiddle
//! library is loaded and no pointer is marshalled — which is also what lets the extension
//! run in ruby.wasm, where there is no Fiddle at all. `HYPERTABULAR_PURE=1` (checked
//! Ruby-side) keeps Fiddle.
//!
//! The buffers are this binding's own, as they are every binding's: the core fills them and
//! keeps nothing. A Ruby object one of them reads from — the text being read, the workbook's
//! bytes — is marked, which pins it where it is, as Fiddle's pointer to it does.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use magnus::typed_data::Obj;
use magnus::value::{Opaque, ReprValue};
use magnus::{
    DataTypeFunctions, Error, IntoValue, RArray, RClass, RModule, RString, Ruby, TypedData, Value,
    function, gc, method, prelude::*,
};

use crate::kernel::abi::{
    CellVerdict, ColumnBuffer, ColumnSpec, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE, ERR_WINDOW,
    Failure, Filled, OK, Opened as Found, Slot, Span,
};
use crate::kernel::delimited::fill::{self, RawDialect, State};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use crate::kernel::workbook::{FORMAT_ODS, Memory, State as Book, book, rows};
use hypercast::{CivilDateTime, Date, Decimal, Duration, RawNumFormat, Timestamp};

// The gem unpacks every array this extension hands it with little-endian directives (`L<`,
// `Q<`), which is the layout the core writes on every target the gem ships for.
#[cfg(target_endian = "big")]
compile_error!("the Ruby binding reads the core's arrays as little-endian");

/// The arena a delimited read starts with, as runtime/delimited.rb's `ARENA_BYTES`.
const ARENA_BYTES: usize = 4096;
/// The header's name table a delimited read starts with (`NAMES`).
const NAMES: usize = 64;
/// The smallest window the workbook core works in (`WINDOW_MIN`).
const WINDOW_MIN: usize = 64 * 1024;
/// The most a workbook's shared strings are given up front (`load_tables`).
const STRINGS_BOUND: u64 = 1 << 28;
/// Bytes in one packed `ColumnSpec`: ordinal, door, param, then HyperCast's 32-byte
/// `RawNumFormat` (runtime/columns.rb's `SPEC_BYTES`).
const SPEC_BYTES: usize = 44;

/// What the gem's objects need from Ruby, looked up once at load: the error a call raises
/// for a structural failure, the module that holds the specs' switches, and the message of
/// a contract violation, which is runtime/delimited.rb's own `CONTRACT`.
struct Companions {
    structure_error: Opaque<RClass>,
    book: Opaque<RModule>,
    contract: String,
}

static COMPANIONS: OnceLock<Companions> = OnceLock::new();

fn companions() -> &'static Companions {
    COMPANIONS
        .get()
        .expect("hypertabular_native used before init")
}

/// A caller bug, never a data verdict: the RuntimeError the Fiddle backend raises with
/// `raise CONTRACT`.
fn contract(ruby: &Ruby) -> Error {
    Error::new(
        ruby.exception_runtime_error(),
        companions().contract.clone(),
    )
}

/// The failure as the gem carries it: `[code, line, record, byte, expected, found]`.
fn failure_array(ruby: &Ruby, failure: &Failure) -> RArray {
    ruby.ary_from_vec(vec![
        u64::from(failure.code),
        u64::from(failure.line),
        failure.record,
        failure.byte,
        u64::from(failure.expected),
        u64::from(failure.found),
    ])
}

/// `Runtime::Book::StructureError`, carrying the failure, as `Book.settle` raises it.
fn structure_error(ruby: &Ruby, failure: &Failure) -> Error {
    let class = ruby.get_inner(companions().structure_error);
    match class.new_instance((failure_array(ruby, failure),)) {
        Ok(error) => match magnus::Exception::from_value(error) {
            Some(exception) => Error::from(exception),
            None => contract(ruby),
        },
        Err(error) => error,
    }
}

/// The bytes the core wrote, as a String of their own: binary, unfrozen, as Fiddle's
/// `Pointer#[]` hands them back.
fn binary(ruby: &Ruby, bytes: &[u8]) -> RString {
    ruby.str_from_slice(bytes)
}

/// The bytes as a frozen UTF-8 String, as the gem's `force_encoding(...).freeze` makes it.
fn utf8_frozen(ruby: &Ruby, bytes: &[u8]) -> RString {
    let text = ruby.enc_str_new(bytes, ruby.utf8_encoding());
    text.freeze();
    text
}

/// A slice of plain `#[repr(C)]` integers as the bytes it is.
fn as_bytes<T: Copy>(items: &[T]) -> &[u8] {
    // SAFETY: every `T` here (`u64`, `Span`, `CellVerdict`) is `#[repr(C)]` integers with no
    // padding, so each of its bytes is initialized; the length is the slice's own.
    unsafe { std::slice::from_raw_parts(items.as_ptr().cast::<u8>(), size_of_val(items)) }
}

/// Bytes one value of a door takes in a column's value array.
const fn value_size(door: Door) -> usize {
    match door {
        Door::Bool | Door::I8 | Door::U8 => 1,
        Door::I16 | Door::U16 => 2,
        Door::I32 | Door::U32 | Door::F32 => 4,
        Door::I64 | Door::U64 | Door::F64 | Door::Time => 8,
        Door::Decimal => size_of::<Decimal>(),
        Door::Uuid => 16,
        Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => size_of::<Timestamp>(),
        Door::Date | Door::DateOrdered(_) => size_of::<Date>(),
        Door::DateTime(_) => size_of::<CivilDateTime>(),
        Door::Duration => size_of::<Duration>(),
        Door::Text => size_of::<Span>(),
    }
}

/// A plan as the core takes it, and the arrays the core casts it into: runtime/columns.rb's
/// `Columns`, held inside the object that reads with it.
struct Columns {
    specs: Vec<ColumnSpec>,
    /// Each column's value array (8-byte words, so that every door's type is aligned in
    /// it), the bytes one of its values takes, and its verdict array.
    stores: Vec<(Vec<u64>, usize, Vec<CellVerdict>)>,
    batch_rows: usize,
}

impl Columns {
    /// +specs+ is one packed `ColumnSpec` per plan column and +sizes+ the bytes one value of
    /// each takes, as the gem's plan packs them. A value array is never smaller than the
    /// door the spec names writes, whatever +sizes+ says: the core trusts it to be that
    /// large. A spec the core refuses is refused by the core, at the first fill, as on the
    /// Fiddle backend.
    fn new(ruby: &Ruby, specs: RArray, sizes: RArray, batch_rows: usize) -> Result<Columns, Error> {
        let mut packed = Vec::with_capacity(specs.len());
        let mut stores = Vec::with_capacity(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            let spec = RString::try_convert(spec)?;
            // SAFETY: copied out before anything else runs on the Ruby side.
            let bytes: [u8; SPEC_BYTES] = unsafe { spec.as_slice() }
                .try_into()
                .map_err(|_| contract(ruby))?;
            let word = |at: usize| {
                u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
            };
            let mut format = [0u8; SPEC_BYTES - 12];
            format.copy_from_slice(&bytes[12..]);
            let spec = ColumnSpec {
                ordinal: word(0),
                door: word(4),
                param: word(8),
                format: RawNumFormat::from_le_bytes(format),
            };
            let declared: usize = sizes.entry(index as isize)?;
            let size = Door::from_code(spec.door, spec.param)
                .map_or(declared, |door| declared.max(value_size(door)));
            stores.push((
                vec![0u64; (size * batch_rows).div_ceil(8)],
                size,
                vec![CellVerdict::OK; batch_rows],
            ));
            packed.push(spec);
        }
        Ok(Columns {
            specs: packed,
            stores,
            batch_rows,
        })
    }

    /// The column buffers as the core takes them, pointing into the stores.
    fn buffers(&mut self) -> Vec<ColumnBuffer> {
        self.stores
            .iter_mut()
            .map(|(values, _, verdicts)| ColumnBuffer {
                values: values.as_mut_ptr().cast(),
                verdicts: verdicts.as_mut_ptr(),
            })
            .collect()
    }

    /// A column's value array for the first +rows+ rows, copied out as the core wrote it.
    fn values(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<RString, Error> {
        let (values, size, _) = self.column(ruby, column, rows)?;
        Ok(binary(ruby, &as_bytes(values)[..size * rows]))
    }

    /// A column's verdict array for the first +rows+ rows — offset, len, reason per row.
    fn verdicts(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<RString, Error> {
        let (_, _, verdicts) = self.column(ruby, column, rows)?;
        Ok(binary(ruby, as_bytes(&verdicts[..rows])))
    }

    #[allow(clippy::type_complexity)]
    fn column(
        &self,
        ruby: &Ruby,
        column: usize,
        rows: usize,
    ) -> Result<&(Vec<u64>, usize, Vec<CellVerdict>), Error> {
        if rows > self.batch_rows {
            return Err(contract(ruby));
        }
        self.stores.get(column).ok_or_else(|| {
            Error::new(
                ruby.exception_index_error(),
                format!("column {column} is outside the plan"),
            )
        })
    }
}

/// The text a read is attached to, pinned for as long as it is.
fn attached(ruby: &Ruby, input: &Cell<Option<Opaque<RString>>>) -> Result<RString, Error> {
    input
        .get()
        .map(|text| ruby.get_inner(text))
        .ok_or_else(|| contract(ruby))
}

/// `length` bytes of +text+ from +start+, or a contract violation for a range outside it.
///
/// # Safety
/// The slice is good until the String is next modified or Ruby next runs code that could
/// modify it; the caller hands it to the core and lets go of it before either.
unsafe fn window(
    ruby: &Ruby,
    text: &RString,
    start: usize,
    length: usize,
) -> Result<&'static [u8], Error> {
    // SAFETY: per the function contract; the 'static is the caller's to keep short.
    let bytes: &'static [u8] =
        unsafe { std::mem::transmute::<&[u8], &'static [u8]>(text.as_slice()) };
    start
        .checked_add(length)
        .and_then(|end| bytes.get(start..end))
        .ok_or_else(|| contract(ruby))
}

// --- delimited text

/// runtime/delimited.rb's `Delimited`: the core's delimited reader as one object, every
/// byte of memory a read needs and every call it makes. `columns` answers with the object
/// itself, which also answers the `Columns` methods the reader asks of it.
#[derive(TypedData)]
#[magnus(
    class = "HyperTabular::Runtime::Native::Delimited",
    free_immediately,
    mark
)]
struct Delimited {
    input: Cell<Option<Opaque<RString>>>,
    read: RefCell<DelimitedRead>,
}

struct DelimitedRead {
    state: State,
    columns: Columns,
    per_row: usize,
    cramped: bool,
    cells: Vec<Span>,
    names: Vec<Span>,
    arena: Vec<u8>,
    out: Filled,
}

impl DataTypeFunctions for Delimited {
    fn mark(&self, marker: &gc::Marker) {
        if let Some(input) = self.input.get() {
            marker.mark(input);
        }
    }
}

impl DelimitedRead {
    fn grow_arena(&mut self, needed: usize) {
        let capacity = needed.max(self.arena.len() * 2);
        self.arena = vec![0; capacity];
    }
}

impl Delimited {
    /// `Delimited.start`: nil when the core refuses the dialect.
    fn start(
        ruby: &Ruby,
        dialect: RString,
        specs: RArray,
        sizes: RArray,
        batch_rows: usize,
        per_row: usize,
    ) -> Result<Option<Obj<Delimited>>, Error> {
        // SAFETY: four bytes copied out at once.
        let packed: [u8; 4] = unsafe { dialect.as_slice() }
            .try_into()
            .map_err(|_| contract(ruby))?;
        let raw = RawDialect {
            separator: packed[0],
            quoting: packed[1],
            skip_blank_lines: packed[2],
            engine: packed[3],
        };
        let Some(state) = State::init(raw) else {
            return Ok(None);
        };
        let columns = Columns::new(ruby, specs, sizes, batch_rows)?;
        let read = DelimitedRead {
            state,
            columns,
            per_row,
            cramped: false,
            cells: vec![Span::default(); per_row * batch_rows],
            names: Vec::new(),
            arena: vec![0; ARENA_BYTES],
            out: Filled::default(),
        };
        Ok(Some(ruby.obj_wrap(Delimited {
            input: Cell::new(None),
            read: RefCell::new(read),
        })))
    }

    /// Names the String the calls that follow read, held where it is.
    fn attach(&self, input: RString) {
        self.input.set(Some(Opaque::from(input)));
    }

    /// Reads the next record of the attached input as a header: OK or ERR_STRUCTURE.
    fn header(
        ruby: &Ruby,
        rb_self: &Self,
        start: usize,
        length: usize,
        last: bool,
    ) -> Result<i32, Error> {
        let text = attached(ruby, &rb_self.input)?;
        let mut guard = rb_self.read.borrow_mut();
        let read = &mut *guard;
        if read.names.is_empty() {
            read.names = vec![Span::default(); NAMES];
        }
        loop {
            // SAFETY: handed to the core and let go of before anything Ruby runs.
            let input = unsafe { window(ruby, &text, start, length)? };
            let code = fill::header(
                &mut read.state,
                input,
                last,
                &mut read.names,
                &mut read.arena,
                &mut read.out,
            );
            let needed = read.out.needed as usize;
            match code {
                OK | ERR_STRUCTURE => return Ok(code),
                ERR_CELLS => read.names = vec![Span::default(); needed],
                ERR_ARENA => read.grow_arena(needed),
                _ => return Err(contract(ruby)),
            }
        }
    }

    /// The header's names as the core located them: offset and flagged length, a pair per
    /// name.
    fn names(ruby: &Ruby, rb_self: &Self) -> RArray {
        let read = rb_self.read.borrow();
        let count = (read.out.rows as usize).min(read.names.len());
        ruby.ary_from_iter(
            read.names[..count]
                .iter()
                .flat_map(|span| [span.offset, span.len]),
        )
    }

    /// Fills every column from the attached input — the one call a batch makes. OK or
    /// ERR_STRUCTURE. A batch that came back short with the arena half used or more has the
    /// next one start with it doubled, as on the Fiddle backend.
    fn fill(
        ruby: &Ruby,
        rb_self: &Self,
        start: usize,
        length: usize,
        last: bool,
    ) -> Result<i32, Error> {
        let text = attached(ruby, &rb_self.input)?;
        let mut guard = rb_self.read.borrow_mut();
        let read = &mut *guard;
        if read.cramped {
            let doubled = read.arena.len() * 2;
            read.grow_arena(doubled);
        }
        read.cramped = false;
        let batch_rows = read.columns.batch_rows;
        loop {
            let buffers = read.columns.buffers();
            // SAFETY: handed to the core and let go of before anything Ruby runs.
            let input = unsafe { window(ruby, &text, start, length)? };
            // SAFETY: each column's two arrays have room for `batch_rows` values of its
            // door's type (`Columns::new`) and `batch_rows` verdicts.
            let code = unsafe {
                fill::fill(
                    &mut read.state,
                    input,
                    last,
                    &read.columns.specs,
                    &buffers,
                    batch_rows,
                    &mut read.cells,
                    &mut read.arena,
                    &mut read.out,
                )
            };
            let needed = read.out.needed as usize;
            match code {
                OK => {
                    let rows = read.out.rows as usize;
                    read.cramped = rows > 0
                        && rows < batch_rows
                        && read.out.arena_used as usize * 2 >= read.arena.len();
                    return Ok(code);
                }
                ERR_STRUCTURE => return Ok(code),
                ERR_CELLS => {
                    read.per_row = read.per_row.max(needed);
                    read.cells = vec![Span::default(); read.per_row * batch_rows];
                }
                ERR_ARENA => read.grow_arena(needed),
                _ => return Err(contract(ruby)),
            }
        }
    }

    /// The cell table for the batch in hand — `per_row` spans a row — copied out.
    fn cells(ruby: &Ruby, rb_self: &Self) -> RString {
        let read = rb_self.read.borrow();
        let count = (read.per_row * read.out.rows as usize).min(read.cells.len());
        binary(ruby, as_bytes(&read.cells[..count]))
    }

    /// The arena as the last call left it.
    fn arena(ruby: &Ruby, rb_self: &Self) -> RString {
        let read = rb_self.read.borrow();
        binary(
            ruby,
            &read.arena[..(read.out.arena_used as usize).min(read.arena.len())],
        )
    }

    fn records(&self) -> u64 {
        self.read.borrow().state.records
    }

    fn line(&self) -> u32 {
        self.read.borrow().state.line
    }

    fn offset(&self) -> u64 {
        self.read.borrow().state.offset
    }

    fn rows(&self) -> u64 {
        self.read.borrow().out.rows
    }

    fn consumed(&self) -> u64 {
        self.read.borrow().out.consumed
    }

    fn arena_used(&self) -> u64 {
        self.read.borrow().out.arena_used
    }

    fn failure(ruby: &Ruby, rb_self: &Self) -> RArray {
        failure_array(ruby, &rb_self.read.borrow().out.failure)
    }

    fn per_row(&self) -> usize {
        self.read.borrow().per_row
    }

    fn count(&self) -> usize {
        self.read.borrow().columns.specs.len()
    }

    fn batch_rows(&self) -> usize {
        self.read.borrow().columns.batch_rows
    }

    fn values(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns.values(ruby, column, rows)
    }

    fn verdicts(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns.verdicts(ruby, column, rows)
    }
}

/// `Delimited.version`: the core's version word, `major << 16 | minor << 8 | patch`.
fn version() -> u32 {
    // SAFETY: no contract to uphold; the export is `unsafe` only because every one is.
    unsafe { crate::kernel::exports::hypertabular_version() }
}

/// `Runtime.unescape`: a quoted delimited cell with its quotes resolved, as the core cast it.
fn unescape(ruby: &Ruby, quoted: RString) -> RString {
    // SAFETY: copied out at once.
    let cell = unsafe { quoted.as_slice() }.to_vec();
    let mut out = vec![0u8; cell.len().max(1)];
    let written = unescape_into(&cell, &mut out);
    binary(ruby, &out[..written])
}

// --- workbooks

/// The buffers a workbook call may ask to have grown: runtime/workbook.rb's `Scratch`.
struct Scratch {
    window: Vec<u8>,
    arena: Vec<u8>,
    cells: Vec<Span>,
    row: Vec<Slot>,
}

/// The workbook's tables as a sheet's calls are handed them.
#[derive(Clone, Copy)]
struct Tables<'a> {
    strings: &'a [u8],
    table: &'a [Span],
    kinds: &'a [u8],
}

const NO_TABLES: Tables<'static> = Tables {
    strings: &[],
    table: &[],
    kinds: &[],
};

/// Whether the specs asked for every buffer to start with room for one element.
fn stingy(ruby: &Ruby) -> Result<bool, Error> {
    let book = ruby.get_inner(companions().book);
    Ok(book.funcall::<_, _, Value>("stingy", ())?.to_bool())
}

/// Counts one growth of buffer +which+ (window, arena, cells) in `Book.grown`, for the specs.
fn grown(ruby: &Ruby, which: isize) -> Result<(), Error> {
    let book = ruby.get_inner(companions().book);
    let grown: RArray = book.funcall("grown", ())?;
    let count: i64 = grown.entry(which)?;
    grown.store(which, count + 1)
}

impl Scratch {
    fn new(window: usize, arena: usize, cells: usize, row: usize) -> Scratch {
        Scratch {
            window: vec![0; window],
            arena: vec![0; arena],
            cells: vec![Span::default(); cells],
            row: vec![Slot::default(); row],
        }
    }

    /// The buffer at least +needed+ long, what it held kept.
    fn grow<T: Copy + Default>(buffer: &mut Vec<T>, needed: u64) {
        let length = (needed as usize).max(buffer.len() + 1);
        buffer.resize(length, T::default());
    }

    fn memory<'a>(&'a mut self, tables: Tables<'a>) -> Memory<'a> {
        Memory {
            window: &mut self.window,
            arena: &mut self.arena,
            cells: &mut self.cells,
            row: &mut self.row,
            strings: tables.strings,
            table: tables.table,
            kinds: tables.kinds,
        }
    }

    /// Makes +call+ until it stops asking for room, growing the buffer it names each time.
    /// Returns the code it ended on, with what it wrote in +out+.
    fn drive(
        &mut self,
        ruby: &Ruby,
        tables: Tables<'_>,
        out: &mut Filled,
        mut call: impl FnMut(&mut Memory<'_>, &mut Filled) -> i32,
    ) -> Result<i32, Error> {
        loop {
            let code = call(&mut self.memory(tables), out);
            match code {
                ERR_WINDOW => {
                    Scratch::grow(&mut self.window, out.needed);
                    grown(ruby, 0)?;
                }
                ERR_ARENA => {
                    Scratch::grow(&mut self.arena, out.needed);
                    grown(ruby, 1)?;
                }
                ERR_CELLS => {
                    Scratch::grow(&mut self.cells, out.needed);
                    grown(ruby, 2)?;
                }
                _ => return Ok(code),
            }
        }
    }
}

/// What a call's code means: nothing more to do, or a structural failure. Anything else is
/// this binding's bug.
fn settle(ruby: &Ruby, code: i32, out: &Filled) -> Result<(), Error> {
    match code {
        OK => Ok(()),
        ERR_STRUCTURE => Err(structure_error(ruby, &out.failure)),
        _ => Err(contract(ruby)),
    }
}

/// runtime/workbook.rb's `Opened`: a workbook opened in memory — its state, its sheets, its
/// tables. Nothing in it changes once it is open.
#[derive(TypedData)]
#[magnus(
    class = "HyperTabular::Runtime::Native::Opened",
    free_immediately,
    mark
)]
struct Opened {
    container: Opaque<RString>,
    state: Box<Book>,
    ods: bool,
    epoch: u32,
    sheets: Opaque<RArray>,
    strings: Opaque<RString>,
    strings_bytes: Vec<u8>,
    table: Vec<Span>,
    kinds: Vec<u8>,
}

impl DataTypeFunctions for Opened {
    fn mark(&self, marker: &gc::Marker) {
        marker.mark(self.container);
        marker.mark(self.sheets);
        marker.mark(self.strings);
    }
}

impl Opened {
    /// `Opened.new`: opens +container+, a String the workbook holds unmodified for its whole
    /// life. A structural failure raises `StructureError`.
    fn open(ruby: &Ruby, container: RString) -> Result<Obj<Opened>, Error> {
        let stingy = stingy(ruby)?;
        let mut scratch = if stingy {
            Scratch::new(1, 1, 1, 0)
        } else {
            Scratch::new(WINDOW_MIN, 1024, 64, 0)
        };
        let mut state = Box::new(Book::new());
        // SAFETY: the container is frozen (Workbook.new freezes what it is handed), and the
        // slice is handed to the core and let go of before anything Ruby runs.
        let bytes = || unsafe { std::mem::transmute::<&[u8], &'static [u8]>(container.as_slice()) };

        let mut found = Found::default();
        loop {
            let code = book::open(
                &mut state,
                bytes(),
                &mut scratch.memory(NO_TABLES),
                &mut found,
            );
            match code {
                OK => break,
                ERR_WINDOW => Scratch::grow(&mut scratch.window, found.needed),
                ERR_ARENA => Scratch::grow(&mut scratch.arena, found.needed),
                ERR_STRUCTURE => return Err(structure_error(ruby, &found.failure)),
                _ => return Err(contract(ruby)),
            }
        }

        // The sheets: three spans each — name, part, then one whose offset's low bit says
        // hidden and whose length is the sheet's index.
        let mut out = Filled::default();
        let code = scratch.drive(ruby, NO_TABLES, &mut out, |memory, out| {
            book::sheets(&mut state, bytes(), memory, out)
        })?;
        settle(ruby, code, &out)?;
        let listed = (out.rows as usize * 3).min(scratch.cells.len());
        let slice = |span: Span| {
            scratch
                .arena
                .get(span.offset as usize..span.offset as usize + span.len())
        };
        let sheets = ruby.ary_new_capa(listed / 3);
        for entry in scratch.cells[..listed].as_chunks::<3>().0 {
            let (name, part) = slice(entry[0])
                .zip(slice(entry[1]))
                .ok_or_else(|| contract(ruby))?;
            sheets.push(ruby.ary_new_from_values(&[
                utf8_frozen(ruby, name).as_value(),
                (entry[2].offset & 1 == 1).into_value_with(ruby),
                binary(ruby, part).as_value(),
                entry[2].len.into_value_with(ruby),
            ]))?;
        }
        sheets.freeze();

        // The shared strings take no more room than their part inflates to; asking for it
        // once saves growing into it.
        let bound = found.strings_bytes.min(STRINGS_BOUND) as usize;
        if !stingy && scratch.arena.len() < bound {
            Scratch::grow(&mut scratch.arena, bound as u64);
        }
        let code = scratch.drive(ruby, NO_TABLES, &mut out, |memory, out| {
            book::strings(&mut state, bytes(), memory, out)
        })?;
        settle(ruby, code, &out)?;
        let used = (out.arena_used as usize).min(scratch.arena.len());
        let strings_bytes = scratch.arena[..used].to_vec();
        let table = scratch.cells[..(out.rows as usize).min(scratch.cells.len())].to_vec();
        let code = scratch.drive(ruby, NO_TABLES, &mut out, |memory, out| {
            book::styles(&mut state, bytes(), memory, out)
        })?;
        settle(ruby, code, &out)?;
        let kinds = scratch.arena[..(out.rows as usize).min(scratch.arena.len())].to_vec();

        Ok(ruby.obj_wrap(Opened {
            container: Opaque::from(container),
            state,
            ods: found.format == FORMAT_ODS,
            epoch: found.epoch,
            sheets: Opaque::from(sheets),
            strings: Opaque::from(utf8_frozen(ruby, &strings_bytes)),
            strings_bytes,
            table,
            kinds,
        }))
    }

    fn tables(&self) -> Tables<'_> {
        Tables {
            strings: &self.strings_bytes,
            table: &self.table,
            kinds: &self.kinds,
        }
    }

    /// The workbook's bytes, for a sheet's call.
    ///
    /// # Safety
    /// As [`window`]: the container is frozen, and the caller lets go of the slice before
    /// anything Ruby runs.
    unsafe fn container(&self, ruby: &Ruby) -> &[u8] {
        let container = ruby.get_inner(self.container);
        // SAFETY: per the function contract. The bytes belong to the String, not to the
        // handle to it this function holds, and the String lives as long as `self` marks it.
        unsafe { std::mem::transmute::<&[u8], &[u8]>(container.as_slice()) }
    }

    fn format(ruby: &Ruby, rb_self: &Self) -> magnus::Symbol {
        ruby.to_symbol(if rb_self.ods { "ods" } else { "xlsx" })
    }

    fn epoch(&self) -> u32 {
        self.epoch
    }

    fn sheets(ruby: &Ruby, rb_self: &Self) -> RArray {
        ruby.get_inner(rb_self.sheets)
    }

    fn strings(ruby: &Ruby, rb_self: &Self) -> RString {
        ruby.get_inner(rb_self.strings)
    }
}

/// runtime/workbook.rb's `Reading`: one sheet being read — its own copy of the state, its
/// plan's arrays, its scratch. `columns` answers with the object itself, as `Delimited`'s
/// does.
#[derive(TypedData)]
#[magnus(
    class = "HyperTabular::Runtime::Native::Reading",
    free_immediately,
    mark
)]
struct Reading {
    book: Opaque<Obj<Opened>>,
    header: Opaque<Value>,
    read: RefCell<SheetRead>,
}

struct SheetRead {
    state: Box<Book>,
    columns: Columns,
    per_row: usize,
    scratch: Scratch,
    out: Filled,
}

impl DataTypeFunctions for Reading {
    fn mark(&self, marker: &gc::Marker) {
        marker.mark(self.book);
        marker.mark(self.header);
    }
}

impl Reading {
    /// `Reading.new`: positions a copy of the workbook's state on +sheet+ (an entry of
    /// `Opened#sheets`) and, with a header declared, reads it.
    #[allow(clippy::too_many_arguments)]
    fn new(
        ruby: &Ruby,
        book: Obj<Opened>,
        sheet: RArray,
        has_header: bool,
        skip_empty_rows: bool,
        specs: RArray,
        sizes: RArray,
        batch_rows: usize,
        width: usize,
    ) -> Result<Obj<Reading>, Error> {
        let part: RString = sheet.entry(2)?;
        // SAFETY: copied out at once.
        let part = unsafe { part.as_slice() }.to_vec();
        let index: u32 = sheet.entry(3)?;
        let columns = Columns::new(ruby, specs, sizes, batch_rows)?;
        let per_row = columns.specs.len() + 1;
        let scratch = if stingy(ruby)? {
            Scratch::new(1, 1, 1, width)
        } else {
            Scratch::new(0, 4096, batch_rows * per_row, width)
        };
        let mut state = Box::new(Book::new());
        // SAFETY: a state block is plain integers with no destructor, the two do not
        // overlap, and the core documents an opened block as copyable.
        unsafe { std::ptr::copy_nonoverlapping(&*book.state, &mut *state, 1) };
        let mut read = SheetRead {
            state,
            columns,
            per_row,
            scratch,
            out: Filled::default(),
        };

        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let code = rows::sheet(
            &mut read.state,
            container,
            &part,
            index,
            has_header,
            skip_empty_rows,
            &mut read.out,
        );
        settle(ruby, code, &read.out)?;
        let header = if has_header {
            read.header(ruby, &book)?.as_value()
        } else {
            ruby.qnil().as_value()
        };
        Ok(ruby.obj_wrap(Reading {
            book: Opaque::from(book),
            header: Opaque::from(header),
            read: RefCell::new(read),
        }))
    }

    /// The next batch: `[rows, cells, arena, failure]` — its rows, its cell table and the
    /// arena as far as the core wrote into it, copied, and the failure that came with them
    /// or nil.
    fn fill(ruby: &Ruby, rb_self: &Self) -> Result<RArray, Error> {
        let book = ruby.get_inner(rb_self.book);
        let mut guard = rb_self.read.borrow_mut();
        let read = &mut *guard;
        let buffers = read.columns.buffers();
        let (state, specs, batch_rows) = (
            &mut read.state,
            &read.columns.specs,
            read.columns.batch_rows,
        );
        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let code = read
            .scratch
            .drive(ruby, book.tables(), &mut read.out, |memory, out| {
                // SAFETY: each column's two arrays have room for `batch_rows` values of its
                // door's type (`Columns::new`) and `batch_rows` verdicts.
                unsafe { rows::fill(state, container, specs, &buffers, batch_rows, memory, out) }
            })?;
        if code != OK && code != ERR_STRUCTURE {
            return Err(contract(ruby));
        }
        let rows = read.out.rows as usize;
        let cells = (read.per_row * rows).min(read.scratch.cells.len());
        let used = (read.out.arena_used as usize).min(read.scratch.arena.len());
        let failure = if code == OK {
            ruby.qnil().as_value()
        } else {
            failure_array(ruby, &read.out.failure).as_value()
        };
        Ok(ruby.ary_new_from_values(&[
            rows.into_value_with(ruby),
            binary(ruby, as_bytes(&read.scratch.cells[..cells])).as_value(),
            binary(ruby, &read.scratch.arena[..used]).as_value(),
            failure,
        ]))
    }

    fn header(ruby: &Ruby, rb_self: &Self) -> Value {
        ruby.get_inner(rb_self.header)
    }

    fn per_row(&self) -> usize {
        self.read.borrow().per_row
    }

    fn count(&self) -> usize {
        self.read.borrow().columns.specs.len()
    }

    fn batch_rows(&self) -> usize {
        self.read.borrow().columns.batch_rows
    }

    fn values(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns.values(ruby, column, rows)
    }

    fn verdicts(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns.verdicts(ruby, column, rows)
    }
}

impl SheetRead {
    /// The header row's names: in the shared strings as written, or — flagged — in the
    /// arena. Frozen UTF-8 Strings in a frozen Array.
    fn header(&mut self, ruby: &Ruby, book: &Opened) -> Result<RArray, Error> {
        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let state = &mut self.state;
        let code = self
            .scratch
            .drive(ruby, book.tables(), &mut self.out, |memory, out| {
                rows::header(state, container, memory, out)
            })?;
        settle(ruby, code, &self.out)?;
        let count = (self.out.rows as usize).min(self.scratch.cells.len());
        let names = ruby.ary_new_capa(count);
        for span in &self.scratch.cells[..count] {
            let (from, offset) = if span.flagged() {
                (&self.scratch.arena[..], span.offset as usize)
            } else {
                (&book.strings_bytes[..], span.offset as usize)
            };
            let name = from
                .get(offset..offset + span.len())
                .ok_or_else(|| contract(ruby))?;
            names.push(utf8_frozen(ruby, name))?;
        }
        names.freeze();
        Ok(names)
    }
}

#[magnus::init(name = "hypertabular_native")]
fn init(ruby: &Ruby) -> Result<(), Error> {
    // lib/hypertabular.rb has defined the Fiddle backend before requiring this; these
    // redefinitions replace its entry points in place.
    let hypertabular = ruby.define_module("HyperTabular")?;
    let runtime: RModule = hypertabular.const_get("Runtime")?;
    let fiddle_delimited: RClass = runtime.const_get("Delimited")?;
    let book_module: RModule = runtime.const_get("Book")?;
    let contract_message: RString = fiddle_delimited.const_get("CONTRACT")?;
    let structure_error: RClass = book_module.const_get("StructureError")?;
    // A constant keeps these from being collected but not from being moved: a compacting GC
    // (the reader spec runs GC.compact) relocates classes and modules and rewrites every
    // reference it can see, which a Rust static is not. Registering them pins them, so
    // the VALUEs below stay theirs. Unpinned, `Book` once moved under a Book.stingy call
    // and the call landed on an Array (HyperTabular run 37718484155, osx-arm64).
    ruby.gc_register_mark_object(structure_error);
    ruby.gc_register_mark_object(book_module);
    let _ = COMPANIONS.set(Companions {
        structure_error: Opaque::from(structure_error),
        book: Opaque::from(book_module),
        contract: contract_message.to_string()?,
    });

    let native = runtime.define_module("Native")?;

    let delimited = native.define_class("Delimited", ruby.class_object())?;
    delimited.undef_default_alloc_func();
    delimited.define_method("attach", method!(Delimited::attach, 1))?;
    delimited.define_method("header", method!(Delimited::header, 3))?;
    delimited.define_method("names", method!(Delimited::names, 0))?;
    delimited.define_method("fill", method!(Delimited::fill, 3))?;
    delimited.define_method("cells", method!(Delimited::cells, 0))?;
    delimited.define_method("arena", method!(Delimited::arena, 0))?;
    delimited.define_method("records", method!(Delimited::records, 0))?;
    delimited.define_method("line", method!(Delimited::line, 0))?;
    delimited.define_method("offset", method!(Delimited::offset, 0))?;
    delimited.define_method("rows", method!(Delimited::rows, 0))?;
    delimited.define_method("consumed", method!(Delimited::consumed, 0))?;
    delimited.define_method("arena_used", method!(Delimited::arena_used, 0))?;
    delimited.define_method("failure", method!(Delimited::failure, 0))?;
    delimited.define_method("per_row", method!(Delimited::per_row, 0))?;
    delimited.define_method("columns", method!(|rb_self: Obj<Delimited>| rb_self, 0))?;
    delimited.define_method("count", method!(Delimited::count, 0))?;
    delimited.define_method("batch_rows", method!(Delimited::batch_rows, 0))?;
    delimited.define_method("values", method!(Delimited::values, 2))?;
    delimited.define_method("verdicts", method!(Delimited::verdicts, 2))?;

    let opened = native.define_class("Opened", ruby.class_object())?;
    opened.undef_default_alloc_func();
    opened.define_method("format", method!(Opened::format, 0))?;
    opened.define_method("epoch", method!(Opened::epoch, 0))?;
    opened.define_method("sheets", method!(Opened::sheets, 0))?;
    opened.define_method("strings", method!(Opened::strings, 0))?;

    let reading = native.define_class("Reading", ruby.class_object())?;
    reading.undef_default_alloc_func();
    reading.define_method("fill", method!(Reading::fill, 0))?;
    reading.define_method("header", method!(Reading::header, 0))?;
    reading.define_method("per_row", method!(Reading::per_row, 0))?;
    reading.define_method("columns", method!(|rb_self: Obj<Reading>| rb_self, 0))?;
    reading.define_method("count", method!(Reading::count, 0))?;
    reading.define_method("batch_rows", method!(Reading::batch_rows, 0))?;
    reading.define_method("values", method!(Reading::values, 2))?;
    reading.define_method("verdicts", method!(Reading::verdicts, 2))?;

    // The entry points the Ruby above calls, now handing out the objects above.
    fiddle_delimited.define_singleton_method("start", function!(Delimited::start, 5))?;
    fiddle_delimited.define_singleton_method("version", function!(version, 0))?;
    runtime.define_singleton_method("unescape", function!(unescape, 1))?;
    book_module
        .const_get::<_, RClass>("Opened")?
        .define_singleton_method("new", function!(Opened::open, 1))?;
    book_module
        .const_get::<_, RClass>("Reading")?
        .define_singleton_method("new", function!(Reading::new, 8))?;
    Ok(())
}
