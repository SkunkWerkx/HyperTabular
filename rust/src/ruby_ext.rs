//! The Ruby binding's fast backend: the gem's Fiddle crossing written again in Rust over the
//! core's own functions, and linked with the core into one Magnus extension
//! (`hypertabular_native`) — the PyO3 play, run for Ruby, as HyperCast's extension is.
//!
//! The gem keeps every native call behind three objects in `ruby/lib/hypertabular/runtime`:
//! `Runtime::Delimited` (a delimited read), `Runtime::Book::Opened` (an open workbook) and
//! `Runtime::Book::Reading` (one sheet being read), plus a few `Runtime` functions. Everything above
//! them — the readers, the workbook, `Batch` and every value and verdict it builds — is plain
//! Ruby that never sees a pointer. So this extension replaces exactly those: on require it
//! defines the constructors (`Delimited.start`, `Opened.new`, `Reading.new`),
//! `Delimited.version` and the `Runtime` functions (`unescape`, `refused`, `transcode`, `faults`,
//! `suggest`, `range_parse`) to hand back objects and bytes of its own, which answer the same
//! methods with the same bytes. Each goes in front of what it replaces, never over it: the Fiddle
//! definitions live in `FiddleBackend` modules their owners extend, and `new` is `Class#new`, so
//! `ruby -w` sees nothing redefined. The Ruby
//! above them stays shared byte for byte, so the two backends cannot disagree about what a
//! cell means; what changes is that no Fiddle library is loaded and no pointer is
//! marshalled — which is also what lets the extension run in ruby.wasm, where there is no
//! Fiddle at all. `HYPERTABULAR_PURE=1` (checked Ruby-side) keeps Fiddle.
//!
//! The buffers are this binding's own, as they are every binding's: the core fills them and
//! keeps nothing. A Ruby object one of them reads from — the text being read, the workbook's
//! bytes — is marked, which pins it where it is, as Fiddle's pointer to it does.

use std::alloc::{Layout, alloc_zeroed};
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use magnus::typed_data::Obj;
use magnus::value::{Opaque, ReprValue};
use magnus::{
    DataTypeFunctions, Error, IntoValue, RArray, RClass, RModule, RString, Ruby, TypedData, Value,
    function, gc, method, prelude::*, scan_args::scan_args,
};

use crate::column;
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnBufferWith, ColumnSpecWith, ERR_ARENA, ERR_CELLS, ERR_STRUCTURE,
    ERR_WINDOW, Failure, Filled, OK, Opened as Found, RawBatchCheck, RawDelimitedOptions,
    RawProblem, RawRange, RawSheetOptions, RawSuggestion, RawTranscoded, Slot, Span,
};
use crate::kernel::delimited::fill::{self, State};
use crate::kernel::delimited::unescape::unescape_into;
use crate::kernel::door::Door;
use crate::kernel::faults::faults as list_faults;
use crate::kernel::size;
use crate::kernel::suggest::suggest as judge_sample;
use crate::kernel::transcode::transcode as decode;
use crate::kernel::workbook::{FORMAT_NUMBERS, FORMAT_ODS, Memory, State as Book, book, rows};
use hypercast::{CivilDateTime, Date, Decimal, Duration, RawDateOptions, RawNumFormat, Timestamp};

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
/// Bytes in the fixed part of a `ColumnSpecWith`, column.rb's `Column#packed_with`: its size,
/// ordinal, door and param, then HyperCast's 92-byte `RawNumFormat` and 12-byte `RawDateOptions`.
const SPEC_FIXED: usize = 120;
/// RawProblems one `faults` call lists into, as runtime.rb's `FAULTS_BYTES`.
const FAULTS: usize = 256;

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
    // SAFETY: every `T` here (`u32`, `u64`, `Span`, `CellVerdict`, `CellNote`, `RawProblem`) is
    // `#[repr(C)]` integers with no padding, so each of its bytes is initialized; the length is the slice's own.
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

/// A type every field of which is an integer, so that all zero bytes are a value of it, and the
/// value `T::default()` answers. A verdict of all zeros is `CellVerdict::OK`.
///
/// # Safety
///
/// All zero bytes must be a valid `Self`.
unsafe trait AllZero: Copy + Default {}
// SAFETY: integers, and structs of nothing but integers.
unsafe impl AllZero for u8 {}
// SAFETY: as above.
unsafe impl AllZero for u32 {}
// SAFETY: as above.
unsafe impl AllZero for u64 {}
// SAFETY: as above.
unsafe impl AllZero for CellVerdict {}
// SAFETY: as above.
unsafe impl AllZero for CellNote {}
// SAFETY: as above.
unsafe impl AllZero for Span {}
// SAFETY: as above.
unsafe impl AllZero for Slot {}

/// `count` zeroed elements, or Ruby's NoMemoryError for a count no memory holds: a batch size is
/// the caller's, and nothing here may abort on one. It may not write to them either: pages nobody
/// reads or writes stay the OS's, where a `resize` would touch every one.
fn zeroed<T: AllZero>(ruby: &Ruby, count: usize) -> Result<Vec<T>, Error> {
    let refused = || Error::new(ruby.exception_no_mem_error(), "failed to allocate memory");
    let layout = Layout::array::<T>(count).map_err(|_| refused())?;
    if layout.size() == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: the layout isn't zero-sized. A block it answers holds `count` elements, all zero,
    // which `T: AllZero` says are values, and the Vec frees it with this same layout.
    unsafe {
        let block = alloc_zeroed(layout).cast::<T>();
        if block.is_null() {
            return Err(refused());
        }
        Ok(Vec::from_raw_parts(block, count, count))
    }
}

/// `first * second`, or Ruby's NoMemoryError where that overflows.
fn product(ruby: &Ruby, first: usize, second: usize) -> Result<usize, Error> {
    first
        .checked_mul(second)
        .ok_or_else(|| Error::new(ruby.exception_no_mem_error(), "failed to allocate memory"))
}

/// One plan column as the gem hands it over, column.rb's `Column.specs` entry: the fixed part of
/// its `ColumnSpecWith`, and the lists the rest of it points at, owned here for as long as the
/// plan is. Nothing is interned, so a list lasts as long as its reader does.
struct Spec {
    fixed: [u8; SPEC_FIXED],
    months: Option<Vec<u8>>,
    tokens: Option<Vec<u8>>,
}

impl Spec {
    fn read(ruby: &Ruby, entry: Value) -> Result<Spec, Error> {
        let entry = RArray::try_convert(entry)?;
        let fixed: RString = entry.entry(0)?;
        // SAFETY: copied out at once.
        let fixed: [u8; SPEC_FIXED] = unsafe { fixed.as_slice() }
            .try_into()
            .map_err(|_| contract(ruby))?;
        let list = |at: isize| -> Result<Option<Vec<u8>>, Error> {
            let list: Option<RString> = entry.entry(at)?;
            // SAFETY: copied out at once.
            Ok(list.map(|list| unsafe { list.as_slice() }.to_vec()))
        };
        Ok(Spec {
            fixed,
            months: list(1)?,
            tokens: list(2)?,
        })
    }

    /// The door the spec names, as the fixed part says.
    fn door(&self) -> Option<Door> {
        Door::from_code(self.word(8), self.word(12))
    }

    fn word(&self, at: usize) -> u32 {
        u32::from_le_bytes([
            self.fixed[at],
            self.fixed[at + 1],
            self.fixed[at + 2],
            self.fixed[at + 3],
        ])
    }

    /// The whole `ColumnSpecWith`, whose pointers are good as long as this is.
    fn with(&self) -> ColumnSpecWith {
        let mut format = [0u8; 92];
        format.copy_from_slice(&self.fixed[16..108]);
        let mut spec = ColumnSpecWith::new(self.word(4), self.word(8), self.word(12));
        spec.size = self.word(0);
        spec.format = RawNumFormat::from_le_bytes(format);
        spec.date = RawDateOptions {
            size: self.word(108),
            flags: self.word(112),
            pivot: self.word(116),
        };
        if let Some(months) = &self.months {
            spec.months = months.as_ptr();
            spec.months_len = months.len();
        }
        if let Some(tokens) = &self.tokens {
            spec.tokens = tokens.as_ptr();
            spec.tokens_len = tokens.len();
        }
        spec
    }
}

/// Each column's value array (8-byte words, so that every door's type is aligned in it), the
/// bytes one of its values takes, its verdict array and its notes, empty with notes off.
struct Store {
    values: Vec<u64>,
    size: usize,
    verdicts: Vec<CellVerdict>,
    notes: Vec<CellNote>,
}

/// A plan as the core takes it, and the arrays the core casts it into: runtime/columns.rb's
/// `Columns`, held inside the object that reads with it.
struct Columns {
    specs: Vec<Spec>,
    stores: Vec<Store>,
    /// Each row's width, empty unless asked for.
    widths: Vec<u32>,
    batch_rows: usize,
}

impl Columns {
    /// +specs+ is one `Column.specs` entry per plan column and +sizes+ the bytes one value of
    /// each takes, as the gem's plan packs them. A value array is never smaller than the
    /// door the spec names writes, whatever +sizes+ says: the core trusts it to be that
    /// large. A spec the core refuses is refused by the core, at the first fill, as on the
    /// Fiddle backend. +notes+ and +widths+ say whether those arrays are kept.
    fn new(
        ruby: &Ruby,
        specs: RArray,
        sizes: RArray,
        batch_rows: usize,
        notes: bool,
        widths: bool,
    ) -> Result<Columns, Error> {
        let mut packed = Vec::with_capacity(specs.len());
        let mut stores = Vec::with_capacity(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            let spec = Spec::read(ruby, spec)?;
            let declared: usize = sizes.entry(index as isize)?;
            let size = spec
                .door()
                .map_or(declared, |door| declared.max(value_size(door)));
            stores.push(Store {
                values: zeroed::<u64>(ruby, product(ruby, size, batch_rows)?.div_ceil(8))?,
                size,
                verdicts: zeroed::<CellVerdict>(ruby, batch_rows)?,
                notes: zeroed::<CellNote>(ruby, if notes { batch_rows } else { 0 })?,
            });
            packed.push(spec);
        }
        Ok(Columns {
            specs: packed,
            stores,
            widths: zeroed::<u32>(ruby, if widths { batch_rows } else { 0 })?,
            batch_rows,
        })
    }

    /// The specs as the core takes them, pointing into `self.specs`.
    fn specs(&self) -> Vec<ColumnSpecWith> {
        self.specs.iter().map(Spec::with).collect()
    }

    /// The column buffers as the core takes them, pointing into the stores.
    fn buffers(&mut self) -> Vec<ColumnBufferWith> {
        self.stores
            .iter_mut()
            .map(|store| ColumnBufferWith {
                size: ColumnBufferWith::SIZE,
                values: store.values.as_mut_ptr().cast(),
                verdicts: store.verdicts.as_mut_ptr(),
                notes: if store.notes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    store.notes.as_mut_ptr()
                },
            })
            .collect()
    }

    /// A column's value array for the first +rows+ rows, copied out as the core wrote it.
    fn values(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<RString, Error> {
        let store = self.column(ruby, column, rows)?;
        Ok(binary(ruby, &as_bytes(&store.values)[..store.size * rows]))
    }

    /// A column's verdict array for the first +rows+ rows — offset, len, reason per row.
    fn verdicts(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<RString, Error> {
        let store = self.column(ruby, column, rows)?;
        Ok(binary(ruby, as_bytes(&store.verdicts[..rows])))
    }

    /// A column's notes for the first +rows+ rows.
    fn notes(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<RString, Error> {
        let store = self.column(ruby, column, rows)?;
        let notes = store.notes.get(..rows).ok_or_else(|| contract(ruby))?;
        Ok(binary(ruby, as_bytes(notes)))
    }

    /// The widths of the first +rows+ rows.
    fn widths(&self, ruby: &Ruby, rows: usize) -> Result<RString, Error> {
        let widths = self.widths.get(..rows).ok_or_else(|| contract(ruby))?;
        Ok(binary(ruby, as_bytes(widths)))
    }

    fn column(&self, ruby: &Ruby, column: usize, rows: usize) -> Result<&Store, Error> {
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
    /// The plan's arrays, once one is bound: the header is read before there is one.
    columns: Option<Columns>,
    per_row: usize,
    cramped: bool,
    cells: Vec<Span>,
    names: Vec<Span>,
    arena: Vec<u8>,
    /// The dialect's tokens, as the core takes them.
    tokens: Vec<u8>,
    out: Filled,
}

impl DataTypeFunctions for Delimited {
    fn mark(&self, marker: &gc::Marker) {
        if let Some(input) = self.input.get() {
            marker.mark(input);
        }
    }
}

/// A delimited read's arena, at least +needed+ bytes and at least doubled: the core rewrites
/// what it needs, so nothing is kept.
fn grow_arena(ruby: &Ruby, arena: &mut Vec<u8>, needed: usize) -> Result<(), Error> {
    *arena = zeroed(ruby, needed.max(arena.len() * 2))?;
    Ok(())
}

impl DelimitedRead {
    /// The plan's arrays, or a contract violation: the reader above never fills before
    /// it has bound a plan.
    fn columns(&self, ruby: &Ruby) -> Result<&Columns, Error> {
        self.columns.as_ref().ok_or_else(|| contract(ruby))
    }
}

impl Delimited {
    /// `Delimited.start`: nil when the core refuses the options, the bytes of a
    /// `RawDelimitedOptions`. +tokens+ is the dialect's, packed, or nil. The plan comes later, by
    /// `bind`, so that the header can be read first.
    fn start(
        ruby: &Ruby,
        options: RString,
        tokens: Option<RString>,
    ) -> Result<Option<Obj<Delimited>>, Error> {
        // SAFETY: copied out at once.
        let packed: [u8; 24] = unsafe { options.as_slice() }
            .try_into()
            .map_err(|_| contract(ruby))?;
        let word = |at: usize| {
            u32::from_le_bytes([packed[at], packed[at + 1], packed[at + 2], packed[at + 3]])
        };
        let raw = RawDelimitedOptions {
            size: word(0),
            separator: packed[4],
            quoting: packed[5],
            skip_blank_lines: packed[6],
            engine: packed[7],
            skip: word(8),
            header_row: word(12),
            short_rows: word(16),
            long_rows: word(20),
        };
        let Some(state) = State::init_with(&raw) else {
            return Ok(None);
        };
        let read = DelimitedRead {
            state,
            columns: None,
            per_row: 0,
            cramped: false,
            cells: Vec::new(),
            names: Vec::new(),
            arena: vec![0; ARENA_BYTES],
            // SAFETY: copied out at once.
            tokens: tokens.map_or(Vec::new(), |tokens| unsafe { tokens.as_slice() }.to_vec()),
            out: Filled::default(),
        };
        Ok(Some(ruby.obj_wrap(Delimited {
            input: Cell::new(None),
            read: RefCell::new(read),
        })))
    }

    /// Takes the plan: +specs+ is one `Column.specs` entry per plan column and +sizes+ the
    /// bytes one value of each takes, +batch_rows+ the most rows a fill writes, +per_row+ the
    /// cell-table entries one row takes, +notes+ whether each column keeps its notes and +widths+
    /// whether each row's width is kept. Once, before the first fill.
    #[allow(clippy::too_many_arguments)] // Magnus takes what the Ruby method takes, one by one.
    fn bind(
        ruby: &Ruby,
        rb_self: &Self,
        specs: RArray,
        sizes: RArray,
        batch_rows: usize,
        per_row: usize,
        notes: bool,
        widths: bool,
    ) -> Result<(), Error> {
        let columns = Columns::new(ruby, specs, sizes, batch_rows, notes, widths)?;
        let mut read = rb_self.read.borrow_mut();
        // As many rows as the budget allows, at least one and at most a batch.
        let rows = column::cell_rows(per_row, batch_rows);
        read.cells = zeroed(ruby, per_row * rows)?;
        read.per_row = per_row;
        read.columns = Some(columns);
        Ok(())
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
                ERR_CELLS => read.names = zeroed(ruby, needed)?,
                ERR_ARENA => grow_arena(ruby, &mut read.arena, needed)?,
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
        let Some(columns) = read.columns.as_mut() else {
            return Err(contract(ruby));
        };
        if read.cramped {
            let doubled = read.arena.len() * 2;
            grow_arena(ruby, &mut read.arena, doubled)?;
        }
        read.cramped = false;
        let batch_rows = columns.batch_rows;
        let specs = columns.specs();
        loop {
            let buffers = columns.buffers();
            // SAFETY: handed to the core and let go of before anything Ruby runs.
            let input = unsafe { window(ruby, &text, start, length)? };
            // SAFETY: each column's arrays have room for `batch_rows` values of its door's type
            // (`Columns::new`), `batch_rows` verdicts and, unless null, as many notes; `specs`
            // points into `columns.specs`, which nothing here changes.
            let code = unsafe {
                fill::fill_with(
                    &mut read.state,
                    input,
                    last,
                    &specs,
                    &buffers,
                    batch_rows,
                    &mut read.cells,
                    &mut read.arena,
                    &read.tokens,
                    &mut columns.widths,
                    &mut read.out,
                )
            };
            let needed = read.out.needed as usize;
            match code {
                OK => {
                    let rows = read.out.rows as usize;
                    // A batch the cell table ends early (see `size::CELL_BUDGET`) is no
                    // reason to grow the arena.
                    let held = batch_rows.min(read.cells.len() / read.per_row.max(1));
                    read.cramped = rows > 0
                        && rows < held
                        && read.out.arena_used as usize * 2 >= read.arena.len();
                    return Ok(code);
                }
                ERR_STRUCTURE => return Ok(code),
                ERR_CELLS => {
                    let per_row = read.per_row.max(needed);
                    let rows = column::cell_rows(per_row, batch_rows);
                    read.cells = zeroed(ruby, per_row * rows)?;
                    read.per_row = per_row;
                }
                ERR_ARENA => grow_arena(ruby, &mut read.arena, needed)?,
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

    /// Cells per record, fixed by the header or the first record; 0 before either.
    fn expected(&self) -> u32 {
        self.read.borrow().state.expected
    }

    fn per_row(&self) -> usize {
        self.read.borrow().per_row
    }

    /// The cell-table entries the reader holds.
    fn cell_entries(&self) -> usize {
        self.read.borrow().cells.len()
    }

    fn count(ruby: &Ruby, rb_self: &Self) -> Result<usize, Error> {
        Ok(rb_self.read.borrow().columns(ruby)?.specs.len())
    }

    fn batch_rows(ruby: &Ruby, rb_self: &Self) -> Result<usize, Error> {
        Ok(rb_self.read.borrow().columns(ruby)?.batch_rows)
    }

    fn values(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .values(ruby, column, rows)
    }

    fn verdicts(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .verdicts(ruby, column, rows)
    }

    fn notes(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .notes(ruby, column, rows)
    }

    fn widths(ruby: &Ruby, rb_self: &Self, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns(ruby)?.widths(ruby, rows)
    }
}

/// `Delimited.start`, whose tokens are optional, as on the Fiddle backend.
fn start_delimited(ruby: &Ruby, args: &[Value]) -> Result<Option<Obj<Delimited>>, Error> {
    let parsed = scan_args::<(RString,), (Option<Option<RString>>,), (), (), (), ()>(args)?;
    Delimited::start(ruby, parsed.required.0, parsed.optional.0.flatten())
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

/// `Runtime.refused`: the index of the first of +specs+ (`Column.specs` entries) the core refuses,
/// or nil.
fn refused(ruby: &Ruby, specs: RArray) -> Result<Option<usize>, Error> {
    for (index, spec) in specs.into_iter().enumerate() {
        let spec = Spec::read(ruby, spec)?;
        // SAFETY: `with` points into `spec`, which outlives the call.
        if !unsafe { fill::honours(&spec.with()) } {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

/// `Runtime.transcode`: the next step of a declared encoding's decode. Answers `[consumed,
/// written, failed, failure_at, out]`.
fn transcode(
    ruby: &Ruby,
    encoding: u32,
    input: RString,
    start: usize,
    length: usize,
    last: bool,
    capacity: usize,
) -> Result<RArray, Error> {
    let mut out = zeroed::<u8>(ruby, capacity)?;
    let mut done = RawTranscoded::default();
    // SAFETY: handed to the core and let go of before anything Ruby runs.
    let bytes = unsafe { window(ruby, &input, start, length)? };
    if decode(encoding, bytes, last, &mut out, &mut done) != OK {
        return Err(contract(ruby));
    }
    let written = (done.written as usize).min(out.len());
    Ok(ruby.ary_new_from_values(&[
        done.consumed.into_value_with(ruby),
        done.written.into_value_with(ruby),
        done.failed.into_value_with(ruby),
        done.failure_at.into_value_with(ruby),
        binary(ruby, &out[..written]).as_value(),
    ]))
}

/// A String's bytes as `count` of a `#[repr(C)]` integer type, aligned as the core wants them.
/// A contract violation for a String of another length.
fn aligned<T: AllZero>(ruby: &Ruby, text: RString, count: usize) -> Result<Vec<T>, Error> {
    let size = product(ruby, count, size_of::<T>())?;
    let mut items = zeroed::<T>(ruby, count)?;
    // SAFETY: copied out at once.
    let bytes = unsafe { text.as_slice() };
    if bytes.len() != size {
        return Err(contract(ruby));
    }
    // SAFETY: `items` has room for `size` bytes, the two do not overlap, and every `T` here is
    // `#[repr(C)]` integers, for which any bytes are a value.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), items.as_mut_ptr().cast(), size) };
    Ok(items)
}

/// `Runtime.faults`: every `RawProblem` of a batch as one binary String, from the columns'
/// verdicts as the core wrote them, their notes or nil, and the row widths or nil.
fn faults(
    ruby: &Ruby,
    verdicts: RArray,
    notes: Option<RArray>,
    rows: usize,
    widths: Option<RString>,
    expected: u32,
    long_rows: u32,
) -> Result<RString, Error> {
    let mut verdict_arrays = Vec::with_capacity(verdicts.len());
    let mut note_arrays = Vec::with_capacity(verdicts.len());
    for column in 0..verdicts.len() as isize {
        verdict_arrays.push(aligned::<CellVerdict>(ruby, verdicts.entry(column)?, rows)?);
        note_arrays.push(match notes {
            Some(notes) => aligned::<CellNote>(ruby, notes.entry(column)?, rows)?,
            None => Vec::new(),
        });
    }
    let row_widths = match widths {
        Some(widths) => aligned::<u32>(ruby, widths, rows)?,
        None => Vec::new(),
    };
    let buffers: Vec<ColumnBufferWith> = verdict_arrays
        .iter_mut()
        .zip(note_arrays.iter_mut())
        .map(|(verdicts, notes)| ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: std::ptr::null_mut(),
            verdicts: verdicts.as_mut_ptr(),
            notes: if notes.is_empty() {
                std::ptr::null_mut()
            } else {
                notes.as_mut_ptr()
            },
        })
        .collect();
    let mut cursor = 0u64;
    let mut listed = Vec::new();
    let mut out = [RawProblem::default(); FAULTS];
    loop {
        let mut written = 0usize;
        // SAFETY: each column's arrays hold `rows` elements, and the widths too when asked for
        // (`aligned`).
        let code = unsafe {
            list_faults(
                &buffers,
                rows,
                &row_widths,
                expected,
                long_rows,
                &mut cursor,
                &mut out,
                &mut written,
            )
        };
        if code != OK {
            return Err(contract(ruby));
        }
        listed.extend_from_slice(&out[..written]);
        if written < FAULTS {
            return Ok(binary(ruby, as_bytes(&listed)));
        }
    }
}

/// `Runtime.suggest`: door, param, status, matched and sampled for each of +columns+, flat, or nil
/// if the core refuses.
fn suggest(
    ruby: &Ruby,
    input: RString,
    cells: RString,
    typed: Option<RString>,
    rows: usize,
    columns: usize,
    formats: RString,
) -> Result<Option<RArray>, Error> {
    let spans = aligned::<Span>(ruby, cells, product(ruby, rows, columns)?)?;
    // SAFETY: copied out at once.
    let marks = typed.map(|typed| unsafe { typed.as_slice() }.to_vec());
    // SAFETY: copied out at once.
    let packed = unsafe { formats.as_slice() }.to_vec();
    if packed.len() != product(ruby, columns, 92)? {
        return Err(contract(ruby));
    }
    let declared: Vec<RawNumFormat> = packed
        .as_chunks::<92>()
        .0
        .iter()
        .map(|format| RawNumFormat::from_le_bytes(*format))
        .collect();
    let mut out = vec![RawSuggestion::default(); columns];
    // SAFETY: copied out at once.
    let bytes = unsafe { input.as_slice() }.to_vec();
    let code = judge_sample(
        &bytes,
        &[],
        &spans,
        marks.as_deref(),
        rows,
        Some(&declared),
        &mut out,
    );
    if code != OK {
        return Ok(None);
    }
    Ok(Some(ruby.ary_from_iter(out.iter().flat_map(|found| {
        [
            found.door,
            found.param,
            found.status,
            found.matched,
            found.sampled,
        ]
    }))))
}

/// `Runtime.range_parse`: a range's four corners as the core reads them, or nil.
fn range_parse(ruby: &Ruby, text: RString) -> Option<RArray> {
    // SAFETY: copied out at once.
    let text = unsafe { text.as_slice() }.to_vec();
    rows::parse_range(&text).map(|range: RawRange| {
        ruby.ary_from_vec(vec![
            range.first_row,
            range.first_column,
            range.last_row,
            range.last_column,
        ])
    })
}

/// `[code, needed, limit, buffer_bytes, refused, sample_rows, width]`.
type BatchCheckTuple = (i32, u64, u64, u64, u32, u64, u64);

/// `Runtime.batch_check`: what the core says of a batch's sizes, `[code, needed, limit,
/// buffer_bytes, refused, sample_rows, width]`.
fn batch_check(
    ruby: &Ruby,
    specs: RArray,
    batch_rows: u64,
    buffer_bytes: u64,
    flags: u32,
) -> Result<BatchCheckTuple, Error> {
    let specs = specs
        .into_iter()
        .map(|spec| Spec::read(ruby, spec))
        .collect::<Result<Vec<_>, _>>()?;
    let specs: Vec<ColumnSpecWith> = specs.iter().map(Spec::with).collect();
    let mut out = RawBatchCheck::default();
    let code = size::check_batch(&specs, batch_rows, buffer_bytes, flags, &mut out);
    Ok((
        code,
        out.needed,
        out.limit,
        out.buffer_bytes,
        out.refused,
        out.sample_rows,
        out.width,
    ))
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

    /// The buffer at least +needed+ long, what it held kept, or Ruby's NoMemoryError. The cell
    /// table a fill asks for is a batch size's, the caller's, so nothing past what it held is
    /// written to.
    fn grow<T: AllZero>(ruby: &Ruby, buffer: &mut Vec<T>, needed: u64) -> Result<(), Error> {
        let mut larger = zeroed::<T>(ruby, (needed as usize).max(buffer.len() + 1))?;
        larger[..buffer.len()].copy_from_slice(buffer);
        *buffer = larger;
        Ok(())
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

    /// The buffer at least +needed+ long if it is shorter, what it held kept, or Ruby's
    /// NoMemoryError.
    fn reserve<T: AllZero>(ruby: &Ruby, buffer: &mut Vec<T>, needed: usize) -> Result<(), Error> {
        if buffer.len() < needed {
            Scratch::grow(ruby, buffer, needed as u64)?;
        }
        Ok(())
    }

    /// Makes +call+ until it stops asking for room, growing the buffer it names each time.
    /// Returns the code it ended on, with what it wrote in +out+.
    ///
    /// With +row_follows_cells+, the row's slots are grown with the cell table, so that
    /// there is a slot for every cell a header names: what a sheet whose plan is not yet
    /// known reads its header with.
    fn drive(
        &mut self,
        ruby: &Ruby,
        tables: Tables<'_>,
        row_follows_cells: bool,
        out: &mut Filled,
        mut call: impl FnMut(&mut Memory<'_>, &mut Filled) -> i32,
    ) -> Result<i32, Error> {
        loop {
            let code = call(&mut self.memory(tables), out);
            match code {
                ERR_WINDOW => {
                    Scratch::grow(ruby, &mut self.window, out.needed)?;
                    grown(ruby, 0)?;
                }
                ERR_ARENA => {
                    Scratch::grow(ruby, &mut self.arena, out.needed)?;
                    grown(ruby, 1)?;
                }
                ERR_CELLS => {
                    Scratch::grow(ruby, &mut self.cells, out.needed)?;
                    if row_follows_cells {
                        Scratch::reserve(ruby, &mut self.row, self.cells.len())?;
                    }
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
    format: u32,
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
                ERR_WINDOW => Scratch::grow(ruby, &mut scratch.window, found.needed)?,
                ERR_ARENA => Scratch::grow(ruby, &mut scratch.arena, found.needed)?,
                ERR_STRUCTURE => return Err(structure_error(ruby, &found.failure)),
                _ => return Err(contract(ruby)),
            }
        }

        // The sheets: three spans each — name, part, then one whose offset's low bit says
        // hidden and whose length is the sheet's index.
        let mut out = Filled::default();
        let code = scratch.drive(ruby, NO_TABLES, false, &mut out, |memory, out| {
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
            Scratch::grow(ruby, &mut scratch.arena, bound as u64)?;
        }
        let code = scratch.drive(ruby, NO_TABLES, false, &mut out, |memory, out| {
            book::strings(&mut state, bytes(), memory, out)
        })?;
        settle(ruby, code, &out)?;
        let used = (out.arena_used as usize).min(scratch.arena.len());
        let strings_bytes = scratch.arena[..used].to_vec();
        let table = scratch.cells[..(out.rows as usize).min(scratch.cells.len())].to_vec();
        let code = scratch.drive(ruby, NO_TABLES, false, &mut out, |memory, out| {
            book::styles(&mut state, bytes(), memory, out)
        })?;
        settle(ruby, code, &out)?;
        let kinds = scratch.arena[..(out.rows as usize).min(scratch.arena.len())].to_vec();

        Ok(ruby.obj_wrap(Opened {
            container: Opaque::from(container),
            state,
            format: found.format,
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
        ruby.to_symbol(match rb_self.format {
            FORMAT_ODS => "ods",
            FORMAT_NUMBERS => "numbers",
            _ => "xlsx",
        })
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
/// does. The plan's arrays wait for `bind`, so that the header can be read first.
#[derive(TypedData)]
#[magnus(
    class = "HyperTabular::Runtime::Native::Reading",
    free_immediately,
    mark
)]
struct Reading {
    book: Opaque<Obj<Opened>>,
    read: RefCell<SheetRead>,
}

struct SheetRead {
    state: Box<Book>,
    /// The plan's arrays, once one is bound.
    columns: Option<Columns>,
    /// The sheet's tokens, as the core takes them.
    tokens: Vec<u8>,
    batch_rows: usize,
    per_row: usize,
    scratch: Scratch,
    out: Filled,
}

impl DataTypeFunctions for Reading {
    fn mark(&self, marker: &gc::Marker) {
        marker.mark(self.book);
    }
}

impl Reading {
    /// `Reading.new`: positions a copy of the workbook's state on +sheet+ (an entry of
    /// `Opened#sheets`), as +options+, the bytes of a `RawSheetOptions`, say. +tokens+ is the
    /// sheet's, packed, or nil.
    fn new(
        ruby: &Ruby,
        book: Obj<Opened>,
        sheet: RArray,
        options: RString,
        batch_rows: usize,
        tokens: Option<RString>,
    ) -> Result<Obj<Reading>, Error> {
        let part: RString = sheet.entry(2)?;
        // SAFETY: copied out at once.
        let part = unsafe { part.as_slice() }.to_vec();
        let index: u32 = sheet.entry(3)?;
        let scratch = if stingy(ruby)? {
            Scratch::new(1, 1, 1, 0)
        } else {
            Scratch::new(0, 4096, 64, 0)
        };
        let mut state = Box::new(Book::new());
        // SAFETY: a state block is plain integers with no destructor, the two do not
        // overlap, and the core documents an opened block as copyable.
        unsafe { std::ptr::copy_nonoverlapping(&*book.state, &mut *state, 1) };
        let options = aligned::<u32>(ruby, options, 9)?;
        let options = RawSheetOptions {
            size: options[0],
            has_header: options[1],
            skip_empty_rows: options[2],
            skip: options[3],
            header_row: options[4],
            first_row: options[5],
            first_column: options[6],
            last_row: options[7],
            last_column: options[8],
        };
        let mut read = SheetRead {
            state,
            columns: None,
            // SAFETY: copied out at once.
            tokens: tokens.map_or(Vec::new(), |tokens| unsafe { tokens.as_slice() }.to_vec()),
            batch_rows,
            per_row: 0,
            scratch,
            out: Filled::default(),
        };

        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let code = rows::sheet_with(
            &mut read.state,
            container,
            &part,
            index,
            &options,
            &mut read.out,
        );
        settle(ruby, code, &read.out)?;
        Ok(ruby.obj_wrap(Reading {
            book: Opaque::from(book),
            read: RefCell::new(read),
        }))
    }

    /// Takes the plan: +specs+ is one `Column.specs` entry per plan column, +sizes+ the bytes
    /// one value of each takes, +width+ the slots a row needs to reach every source
    /// column the plan reads, +notes+ whether each column keeps its notes and +widths+ whether
    /// each row's width is kept. Once, before the first fill. The core keeps a header row's
    /// cells in the row's slots — a row the sheet repeats (an ODS `number-rows-repeated`) is
    /// delivered again from them — so the slots are only ever grown here, never replaced.
    fn bind(
        ruby: &Ruby,
        rb_self: &Self,
        specs: RArray,
        sizes: RArray,
        width: usize,
        notes: bool,
        widths: bool,
    ) -> Result<(), Error> {
        let stingy = stingy(ruby)?;
        let mut guard = rb_self.read.borrow_mut();
        let read = &mut *guard;
        let columns = Columns::new(ruby, specs, sizes, read.batch_rows, notes, widths)?;
        read.per_row = columns.specs.len() + 1;
        // A stingy scratch is left to grow into the cell table the hard way, mid-read.
        if !stingy {
            let wanted = product(ruby, read.batch_rows, read.per_row)?;
            Scratch::reserve(ruby, &mut read.scratch.cells, wanted)?;
        }
        Scratch::reserve(ruby, &mut read.scratch.row, width)?;
        read.columns = Some(columns);
        Ok(())
    }

    /// Reads the header row: its names, as frozen UTF-8 Strings in a frozen Array.
    fn read_header(ruby: &Ruby, rb_self: &Self) -> Result<RArray, Error> {
        let book = ruby.get_inner(rb_self.book);
        rb_self.read.borrow_mut().header(ruby, &book)
    }

    /// The next batch: `[rows, cells, arena, failure]` — its rows, its cell table and the
    /// arena as far as the core wrote into it, copied, and the failure that came with them
    /// or nil.
    fn fill(ruby: &Ruby, rb_self: &Self) -> Result<RArray, Error> {
        let book = ruby.get_inner(rb_self.book);
        let mut guard = rb_self.read.borrow_mut();
        let read = &mut *guard;
        let Some(columns) = read.columns.as_mut() else {
            return Err(contract(ruby));
        };
        let buffers = columns.buffers();
        let specs = columns.specs();
        let batch_rows = columns.batch_rows;
        let state = &mut read.state;
        let tokens = &read.tokens;
        let widths = &mut columns.widths;
        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let code =
            read.scratch
                .drive(ruby, book.tables(), false, &mut read.out, |memory, out| {
                    // SAFETY: each column's arrays have room for `batch_rows` values of its door's
                    // type (`Columns::new`), `batch_rows` verdicts and, unless null, as many notes;
                    // `specs` points into `columns.specs`, which nothing here changes.
                    unsafe {
                        rows::fill_with(
                            state, container, &specs, &buffers, batch_rows, memory, tokens, widths,
                            out,
                        )
                    }
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

    fn per_row(&self) -> usize {
        self.read.borrow().per_row
    }

    fn count(ruby: &Ruby, rb_self: &Self) -> Result<usize, Error> {
        Ok(rb_self.read.borrow().columns(ruby)?.specs.len())
    }

    fn batch_rows(&self) -> usize {
        self.read.borrow().batch_rows
    }

    fn values(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .values(ruby, column, rows)
    }

    fn verdicts(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .verdicts(ruby, column, rows)
    }

    fn notes(ruby: &Ruby, rb_self: &Self, column: usize, rows: usize) -> Result<RString, Error> {
        rb_self
            .read
            .borrow()
            .columns(ruby)?
            .notes(ruby, column, rows)
    }

    fn widths(ruby: &Ruby, rb_self: &Self, rows: usize) -> Result<RString, Error> {
        rb_self.read.borrow().columns(ruby)?.widths(ruby, rows)
    }
}

impl SheetRead {
    /// The plan's arrays, or a contract violation: the sheet above never fills before it
    /// has bound a plan.
    fn columns(&self, ruby: &Ruby) -> Result<&Columns, Error> {
        self.columns.as_ref().ok_or_else(|| contract(ruby))
    }

    /// The header row's names: in the shared strings as written, or — flagged — in the
    /// arena. Frozen UTF-8 Strings in a frozen Array.
    ///
    /// Read before a plan is bound, the row's slots are made — and kept — as many as the
    /// cell table has spans, a slot for every name: the core keeps the header row's cells
    /// there as well as naming them, and a row the sheet repeats is delivered from them.
    fn header(&mut self, ruby: &Ruby, book: &Opened) -> Result<RArray, Error> {
        // SAFETY: the container is frozen, and the slice is let go of before Ruby runs.
        let container = unsafe { book.container(ruby) };
        let unbound = self.columns.is_none();
        if unbound {
            Scratch::reserve(ruby, &mut self.scratch.row, self.scratch.cells.len())?;
        }
        let state = &mut self.state;
        let code = self.scratch.drive(
            ruby,
            book.tables(),
            unbound,
            &mut self.out,
            |memory, out| rows::header(state, container, memory, out),
        )?;
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
    // definitions go in front of its FiddleBackend entry points.
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
    delimited.define_method("bind", method!(Delimited::bind, 6))?;
    delimited.define_method("attach", method!(Delimited::attach, 1))?;
    delimited.define_method("header", method!(Delimited::header, 3))?;
    delimited.define_method("names", method!(Delimited::names, 0))?;
    delimited.define_method("fill", method!(Delimited::fill, 3))?;
    delimited.define_method("cells", method!(Delimited::cells, 0))?;
    delimited.define_method("arena", method!(Delimited::arena, 0))?;
    delimited.define_method("records", method!(Delimited::records, 0))?;
    delimited.define_method("line", method!(Delimited::line, 0))?;
    delimited.define_method("offset", method!(Delimited::offset, 0))?;
    delimited.define_method("expected", method!(Delimited::expected, 0))?;
    delimited.define_method("rows", method!(Delimited::rows, 0))?;
    delimited.define_method("consumed", method!(Delimited::consumed, 0))?;
    delimited.define_method("arena_used", method!(Delimited::arena_used, 0))?;
    delimited.define_method("failure", method!(Delimited::failure, 0))?;
    delimited.define_method("per_row", method!(Delimited::per_row, 0))?;
    delimited.define_method("cell_entries", method!(Delimited::cell_entries, 0))?;
    delimited.define_method("columns", method!(|rb_self: Obj<Delimited>| rb_self, 0))?;
    delimited.define_method("count", method!(Delimited::count, 0))?;
    delimited.define_method("batch_rows", method!(Delimited::batch_rows, 0))?;
    delimited.define_method("values", method!(Delimited::values, 2))?;
    delimited.define_method("verdicts", method!(Delimited::verdicts, 2))?;
    delimited.define_method("notes", method!(Delimited::notes, 2))?;
    delimited.define_method("widths", method!(Delimited::widths, 1))?;

    let opened = native.define_class("Opened", ruby.class_object())?;
    opened.undef_default_alloc_func();
    opened.define_method("format", method!(Opened::format, 0))?;
    opened.define_method("epoch", method!(Opened::epoch, 0))?;
    opened.define_method("sheets", method!(Opened::sheets, 0))?;
    opened.define_method("strings", method!(Opened::strings, 0))?;

    let reading = native.define_class("Reading", ruby.class_object())?;
    reading.undef_default_alloc_func();
    reading.define_method("bind", method!(Reading::bind, 5))?;
    reading.define_method("read_header", method!(Reading::read_header, 0))?;
    reading.define_method("fill", method!(Reading::fill, 0))?;
    reading.define_method("per_row", method!(Reading::per_row, 0))?;
    reading.define_method("columns", method!(|rb_self: Obj<Reading>| rb_self, 0))?;
    reading.define_method("count", method!(Reading::count, 0))?;
    reading.define_method("batch_rows", method!(Reading::batch_rows, 0))?;
    reading.define_method("values", method!(Reading::values, 2))?;
    reading.define_method("verdicts", method!(Reading::verdicts, 2))?;
    reading.define_method("notes", method!(Reading::notes, 2))?;
    reading.define_method("widths", method!(Reading::widths, 1))?;

    // The entry points the Ruby above calls, now handing out the objects above.
    fiddle_delimited.define_singleton_method("start", function!(start_delimited, -1))?;
    fiddle_delimited.define_singleton_method("version", function!(version, 0))?;
    runtime.define_singleton_method("unescape", function!(unescape, 1))?;
    runtime.define_singleton_method("refused", function!(refused, 1))?;
    runtime.define_singleton_method("transcode", function!(transcode, 6))?;
    runtime.define_singleton_method("faults", function!(faults, 6))?;
    runtime.define_singleton_method("suggest", function!(suggest, 6))?;
    runtime.define_singleton_method("range_parse", function!(range_parse, 1))?;
    runtime.define_singleton_method("batch_check", function!(batch_check, 4))?;
    book_module
        .const_get::<_, RClass>("Opened")?
        .define_singleton_method("new", function!(Opened::open, 1))?;
    book_module
        .const_get::<_, RClass>("Reading")?
        .define_singleton_method("new", function!(Reading::new, 5))?;
    Ok(())
}
