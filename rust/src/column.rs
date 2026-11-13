//! What the caller declares: which source column each output column reads, the door it
//! casts through, and — for the numeric doors — the notation the text is written in. A
//! plan is a slice of these, and a projection: nothing about the source's width is
//! inferred from it, and one source column may be read through several doors.

use crate::Error;
use crate::kernel::abi::{
    CellNote, CellVerdict, ColumnSpec, ColumnSpecWith, ERR_BATCH, OK, RawBatchCheck, Slot, Span,
};
use crate::kernel::delimited::fill;
use crate::kernel::door::Door;
use crate::kernel::size;
use hypercast::{
    Constraints, DateOptions, DateOrder, Decimal, ExcelEpoch, MonthNames, NumFormat,
    RawConstraints, RawDateOptions, RawNumFormat, Text, UnixPrecision,
};
use std::alloc::{Layout, alloc_zeroed};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// A list of bytes for the life of the program, one copy of each distinct list: what lets a
/// [`Column`] or a [`Dialect`](crate::Dialect), which are `Copy` and have no lifetime, hold a list
/// of tokens or month names. So what this keeps is bounded by the distinct lists a program
/// declares, however many readers use them.
pub(crate) fn intern(list: Vec<u8>) -> &'static [u8] {
    static LISTS: Mutex<BTreeSet<&'static [u8]>> = Mutex::new(BTreeSet::new());
    // A panic with this held can't leave the set anything but a set.
    let mut lists = LISTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = lists.get(list.as_slice()) {
        return known;
    }
    let kept: &'static [u8] = Box::leak(list.into_boxed_slice());
    lists.insert(kept);
    kept
}

/// Tokens as the core takes them: each a little-endian `u32` length, then its bytes. A token too
/// long for the length word can't be in a cell, whose offsets are 31 bits, so it is left out.
pub(crate) fn packed_tokens(tokens: &[&str]) -> &'static [u8] {
    let mut list = Vec::new();
    for token in tokens {
        if let Ok(len) = u32::try_from(token.len()) {
            list.extend_from_slice(&len.to_le_bytes());
            list.extend_from_slice(token.as_bytes());
        }
    }
    intern(list)
}

/// One copy of each distinct declaration for the life of the program, as [`intern`] keeps lists:
/// what lets a [`Column`], `Copy` and without a lifetime, carry a declaration a fill can point to.
fn intern_constraints(raw: RawConstraints) -> &'static RawConstraints {
    static KEPT: Mutex<Vec<&'static RawConstraints>> = Mutex::new(Vec::new());
    // A panic with this held can't leave the list anything but a list.
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = kept.iter().find(|known| ***known == raw) {
        return known;
    }
    let leaked: &'static RawConstraints = Box::leak(Box::new(raw));
    kept.push(leaked);
    leaked
}

mod sealed {
    pub trait Sealed {}
}

/// A type whose door takes HyperCast's [`Constraints`]: the integers, the floats, [`Decimal`] and
/// HyperCast's [`Text`] marker, each naming its door. Sealed: it says which doors a
/// [`Column::checked`] can be, so a door that takes none is a compile error.
///
/// ```compile_fail
/// use hypertabular::{Column, Constraints, NumericRules};
/// // A UUID column takes no constraints: there is no `Constraints::<[u8; 16]>`.
/// let column = Column::checked(0, Constraints::<[u8; 16]>::numeric(NumericRules::new(), None).unwrap());
/// ```
pub trait CheckedDoor: sealed::Sealed {
    /// The door a column of this type reads through.
    const DOOR: Door;
}

macro_rules! checked_doors {
    ($($ty:ty => $door:ident),+ $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl CheckedDoor for $ty {
            const DOOR: Door = Door::$door;
        }
    )+};
}

checked_doors! {
    i8 => I8, i16 => I16, i32 => I32, i64 => I64,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64,
    f32 => F32, f64 => F64, Decimal => Decimal, Text => Text,
}

/// A column's constraints as a fill takes them, kept for the life of the program, and the door
/// they were declared for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) raw: &'static RawConstraints,
    /// The packed `enum`; empty for none.
    pub(crate) entries: &'static [u8],
    pub(crate) door: Door,
}

/// One output column.
///
/// Build columns with [`Column::new`] or a typed constructor, then set their numeric
/// notation with [`Column::format`], what stands for no value with [`Column::tokens`], and what
/// the date doors accept with [`Column::date_options`]. Additional settings may be added in a
/// minor release.
///
/// ```compile_fail,E0639
/// use hypertabular::{Column, Door, NumFormat};
/// let column = Column { ordinal: 0, door: Door::I32, format: NumFormat::INVARIANT };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Column {
    /// The zero-based column of the source this reads.
    pub ordinal: usize,
    /// The door its cells are cast through.
    pub door: Door,
    /// The numeric notation, for the doors that read numbers.
    pub format: NumFormat,
    /// The tokens this column answers to, in place of its dialect's.
    tokens: Option<&'static [u8]>,
    /// What the date doors accept beyond their own shapes.
    date: DateOptions,
    /// The month vocabulary, packed.
    months: Option<&'static [u8]>,
    /// The constraints its cells must also meet: [`Column::checked`].
    constraints: Option<Declared>,
}

macro_rules! plain_doors {
    ($($(#[$doc:meta])* $name:ident => $door:ident),+ $(,)?) => {$(
        $(#[$doc])*
        pub const fn $name(ordinal: usize) -> Column {
            Column::new(ordinal, Door::$door)
        }
    )+};
}

impl Column {
    /// Source column `ordinal` through `door`, with no numeric format declared:
    /// [`NumFormat::INVARIANT`], which is `,` groups under Western grouping, as a plan with no
    /// format reads in every binding. `1,234,567` is 1234567 there and `1,23,456` is
    /// [`Reason::Malformed`](crate::Reason::Malformed). Declare Indian grouping on the column to
    /// read that shape: `NumFormat::INVARIANT.with_grouping(Grouping::Indian)`.
    pub const fn new(ordinal: usize, door: Door) -> Column {
        Column {
            ordinal,
            door,
            format: NumFormat::INVARIANT,
            tokens: None,
            date: DateOptions::NONE,
            months: None,
            constraints: None,
        }
    }

    /// Source column `ordinal` through `T`'s door, whose cells must also meet `constraints`,
    /// HyperCast's declaration: read through [`Batch::get_checked`](crate::Batch::get_checked) or
    /// [`Batch::text_checked`](crate::Batch::text_checked), a cell that casts and breaks a declared
    /// rule is a [`Violation`](crate::Violation) carrying the value as cast, and one that casts and
    /// meets them all is [`Checked`](crate::Checked), flagged when `multipleOf`'s rounding adjusted
    /// it. A cell that doesn't cast, and one no door is asked about, is its fault, as with
    /// [`Batch::get`](crate::Batch::get), whose getters a constrained column refuses.
    ///
    /// The door is `T`'s, and the column's `door` must stay it: a column whose `door` was changed
    /// after this is [`Error::Plan`] when the plan is bound. The numeric notation, tokens and date
    /// options are set as on any column. Each distinct declaration is kept for the life of the
    /// process, as tokens are.
    ///
    /// ```
    /// use hypertabular::{Column, Constraints, DelimitedReader, Dialect, Keyword, NumericRules, Rejection};
    /// let at_most = NumericRules::new().maximum(hypercast::decimal_for_declaration("10").unwrap());
    /// let plan = [Column::checked(0, Constraints::<i32>::numeric(at_most, None).unwrap())];
    /// let mut reader = DelimitedReader::from_slice(b"n\n7\n11\n", Dialect::CSV, &plan)?;
    /// let batch = reader.read()?.unwrap();
    /// assert_eq!(batch.get_checked::<i32>(0, 0).map(|ok| ok.value), Ok(7));
    /// let Err(Rejection::Violation(broken)) = batch.get_checked::<i32>(0, 1) else { panic!() };
    /// assert_eq!((broken.keyword, broken.value), (Keyword::Maximum, 11));
    /// # Ok::<(), hypertabular::Error>(())
    /// ```
    pub fn checked<T: CheckedDoor>(ordinal: usize, constraints: Constraints<'_, T>) -> Column {
        // A packed `enum` too long for the length word can't be declared: HyperCast's
        // `pack_enum` says `None`, and the fill refuses the stand-in, a list that isn't whole.
        let entries = match constraints.pack_enum(&mut []) {
            Some(0) => &[][..],
            Some(len) => {
                let mut packed = vec![0; len];
                match constraints.pack_enum(&mut packed) {
                    Some(_) => intern(packed),
                    None => &[0],
                }
            }
            None => &[0],
        };
        Column {
            constraints: Some(Declared {
                raw: intern_constraints(constraints.raw()),
                entries,
                door: T::DOOR,
            }),
            ..Column::new(ordinal, T::DOOR)
        }
    }

    /// Whether the column declares constraints: [`Column::checked`].
    pub const fn is_checked(&self) -> bool {
        self.constraints.is_some()
    }

    /// The column's declaration, as its door's type takes it, when it is text.
    pub(crate) fn text_constraints(&self) -> Option<Constraints<'static, Text>> {
        let declared = self.constraints?;
        Constraints::<Text>::from_raw(*declared.raw, declared.entries).ok()
    }

    plain_doors! {
        /// A boolean.
        bool => Bool,
        /// A signed 8-bit integer.
        i8 => I8,
        /// A signed 16-bit integer.
        i16 => I16,
        /// A signed 32-bit integer.
        i32 => I32,
        /// A signed 64-bit integer.
        i64 => I64,
        /// An unsigned 8-bit integer.
        u8 => U8,
        /// An unsigned 16-bit integer.
        u16 => U16,
        /// An unsigned 32-bit integer.
        u32 => U32,
        /// An unsigned 64-bit integer.
        u64 => U64,
        /// A 32-bit float.
        f32 => F32,
        /// A 64-bit float.
        f64 => F64,
        /// An exact decimal.
        decimal => Decimal,
        /// A UUID.
        uuid => Uuid,
        /// An RFC 3339 instant.
        timestamp => Timestamp,
        /// A `yyyy-MM-dd` date.
        date => Date,
        /// A 24-hour time of day.
        time => Time,
        /// An ISO 8601 duration.
        duration => Duration,
        /// The cell's text itself.
        text => Text,
    }

    /// A count of seconds, milliseconds, microseconds or nanoseconds since the Unix epoch.
    pub const fn unix(ordinal: usize, precision: UnixPrecision) -> Column {
        Column::new(ordinal, Door::Unix(precision))
    }

    /// A spreadsheet date serial, in the declared date system.
    pub const fn excel_serial(ordinal: usize, epoch: ExcelEpoch) -> Column {
        Column::new(ordinal, Door::ExcelSerial(epoch))
    }

    /// A date whose fields come in the declared order.
    pub const fn date_ordered(ordinal: usize, order: DateOrder) -> Column {
        Column::new(ordinal, Door::DateOrdered(order))
    }

    /// A zoneless date and time whose date fields come in the declared order.
    pub const fn datetime(ordinal: usize, order: DateOrder) -> Column {
        Column::new(ordinal, Door::DateTime(order))
    }

    /// The same column reading its numbers in `format`, this column's alone, so one plan can read
    /// an Indian column beside a Western one. Only the numeric doors (the integers, the floats and
    /// the decimal) read it. A format HyperCast refuses, such as one holding the retired grouping
    /// bit (`256`, so the words `351` and `863`), is [`Error::Plan`] when the plan is bound, whatever
    /// the door and before any cell.
    pub const fn format(self, format: NumFormat) -> Column {
        Column { format, ..self }
    }

    /// The same column answering to these tokens rather than its dialect's
    /// ([`Dialect::with_tokens`](crate::Dialect::with_tokens)): a cell whose exact, unquoted bytes
    /// are one of them is `Empty`, whatever its door, and [`Batch::origin`](crate::Batch::origin)
    /// says [`Origin::Token`](crate::Origin::Token). An empty list means none, so a column can opt
    /// out of its dialect's. On a sheet, only a text cell can be one
    /// ([`SheetOptions::with_tokens`](crate::SheetOptions::with_tokens)).
    ///
    /// Each distinct list is kept for the life of the process, so that a column can stay `Copy`: a
    /// program building lists from request data grows without bound.
    pub fn tokens(self, tokens: &[&str]) -> Column {
        Column {
            tokens: Some(packed_tokens(tokens)),
            ..self
        }
    }

    /// The same column reading dates under these options and this month vocabulary: the
    /// `Timestamp`, `Date`, `DateOrdered` and `DateTime` doors read them, as HyperCast's own do. A
    /// declaration HyperCast refuses is [`Error::Plan`] when the plan is bound. On a sheet they reach
    /// only a text cell, since a typed date cell has no text for them to read.
    ///
    /// Each distinct vocabulary is kept for the life of the process, so that a column can stay
    /// `Copy`: a program building vocabularies from request data grows without bound.
    pub fn date_options(self, options: DateOptions, months: Option<MonthNames<'_>>) -> Column {
        let months = months.map(|names| {
            // A vocabulary too long to pack can't be read back, and the core refuses this list.
            let mut packed = vec![0; names.pack(&mut []).unwrap_or_default()];
            match names.pack(&mut packed) {
                Some(_) => intern(packed),
                None => &[0],
            }
        });
        Column {
            date: options,
            months,
            ..self
        }
    }

    /// The column as a fill takes it; `index` is its place in the plan, for the error.
    pub(crate) fn spec_with(&self, index: usize) -> Result<ColumnSpecWith, Error> {
        let spec = self.spec(index)?;
        let mut with = ColumnSpecWith::new(spec.ordinal, spec.door, spec.param);
        with.format = spec.format;
        if self.date != DateOptions::NONE {
            with.date = RawDateOptions {
                size: RawDateOptions::SIZE,
                flags: self.date.flags(),
                pivot: u32::from(self.date.pivot()),
            };
        }
        if let Some(months) = self.months {
            with.months = months.as_ptr();
            with.months_len = months.len();
        }
        if let Some(tokens) = self.tokens {
            with.tokens = tokens.as_ptr();
            with.tokens_len = tokens.len();
        }
        if let Some(declared) = self.constraints {
            // A door changed after the declaration: the constraints are another door's.
            if declared.door != self.door {
                return Err(Error::Plan { column: index });
            }
            with.constraints = declared.raw;
            if !declared.entries.is_empty() {
                with.enum_ptr = declared.entries.as_ptr();
                with.enum_len = declared.entries.len();
            }
        }
        // Everything the fill would refuse, said before any read.
        // SAFETY: the lists are interned, so they live as long as the program.
        if !unsafe { fill::honours(&with) } {
            return Err(Error::Plan { column: index });
        }
        Ok(with)
    }

    /// The column as the core takes it; `index` is its place in the plan, for the error.
    pub(crate) fn spec(&self, index: usize) -> Result<ColumnSpec, Error> {
        if self.ordinal > MAX_ORDINAL {
            return Err(Error::Plan { column: index });
        }
        let ordinal = self.ordinal as u32;
        let param = match self.door {
            Door::Unix(precision) => precision as u32,
            Door::DateOrdered(order) | Door::DateTime(order) => order as u32,
            Door::ExcelSerial(epoch) => epoch as u32,
            _ => 0,
        };
        let spec = ColumnSpec {
            ordinal,
            door: self.door.code(),
            param,
            format: RawNumFormat::from(self.format),
        };
        // A format the core would refuse — equal separators, a symbol the format's own
        // constructors would not have built, a retired grouping bit, which `NumFormat::new` takes
        // without a word — is the caller's mistake, said before any read.
        if spec.num_format().is_none() {
            return Err(Error::Plan { column: index });
        }
        Ok(spec)
    }
}

/// The widest ordinal a plan may name: an ordinal must be below 2^20 (1,048,576). No
/// workbook format has that many columns (XLSX and ODS stop at 16,384, Numbers at 1,000),
/// and at the cap one row of cell table is at most 8 MiB, so a headerless plan cannot ask
/// for the gigabytes a row reaching 2³¹ would take.
pub(crate) const MAX_ORDINAL: usize = (1 << 20) - 1;

/// A plan as the core takes it, the number of row slots it reaches, and the place in the
/// plan of the column that reaches furthest (the first, if several do).
pub(crate) fn specs(plan: &[Column]) -> Result<(Vec<ColumnSpec>, usize, usize), Error> {
    let specs = plan
        .iter()
        .enumerate()
        .map(|(index, column)| column.spec(index))
        .collect::<Result<Vec<_>, _>>()?;
    // Every ordinal is at most `MAX_ORDINAL`, so one more fits a `usize` on any target.
    let (mut width, mut widest) = (0, 0);
    for (index, column) in plan.iter().enumerate() {
        if column.ordinal + 1 > width {
            (width, widest) = (column.ordinal + 1, index);
        }
    }
    Ok((specs, width, widest))
}

/// The first plan column reading at or past `width`, the source's width once a header has
/// fixed it, as [`Error::Plan`]: every record has exactly that many cells, so such a column
/// could only ever be empty.
pub(crate) fn within(plan: &[Column], width: Option<usize>) -> Result<(), Error> {
    match width.and_then(|width| plan.iter().position(|column| column.ordinal >= width)) {
        Some(column) => Err(Error::Plan { column }),
        None => Ok(()),
    }
}

/// The plan as a fill takes it.
pub(crate) fn specs_with(plan: &[Column]) -> Result<Vec<ColumnSpecWith>, Error> {
    plan.iter()
        .enumerate()
        .map(|(index, column)| column.spec_with(index))
        .collect()
}

/// The rows of `per_row` entries a delimited cell table holds: the core's rule,
/// [`size::cell_rows`].
pub(crate) fn cell_rows(per_row: usize, batch_rows: usize) -> usize {
    size::cell_rows(per_row as u64, batch_rows as u64) as usize
}

/// A type whose all-zero bytes are a value of it, so that it can be had from `alloc_zeroed`.
///
/// # Safety
/// Every byte of `Self` being zero must be a valid `Self`.
pub(crate) unsafe trait AllZero: Copy {}
// SAFETY: integers and structs of integers, all-zero.
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
// SAFETY: as above.
unsafe impl AllZero for hypercast::RawCheck {}

/// `count` zeroed elements, or `None` if memory for them can't be had. Nothing is written: the
/// pages stay untouched until a read reaches them, so a batch costs what it uses.
pub(crate) fn zeroed<T: AllZero>(count: usize) -> Option<Vec<T>> {
    let layout = Layout::array::<T>(count).ok()?;
    if layout.size() == 0 {
        return Some(Vec::new());
    }
    // SAFETY: `layout` has a nonzero size. The block is that layout's, all zero, which
    // `T: AllZero` says are values, and the Vec frees it with this same layout.
    unsafe {
        let block = alloc_zeroed(layout).cast::<T>();
        (!block.is_null()).then(|| Vec::from_raw_parts(block, count, count))
    }
}

/// The core's answer on a read's sizes, as this API's errors.
pub(crate) fn check_sizes(
    specs: &[ColumnSpecWith],
    batch_rows: usize,
    buffer_bytes: usize,
    flags: u32,
) -> Result<RawBatchCheck, Error> {
    let mut found = RawBatchCheck::default();
    let code = size::check_batch(
        specs,
        batch_rows as u64,
        buffer_bytes as u64,
        flags,
        &mut found,
    );
    match (code, found.refused) {
        (OK, _) => Ok(found),
        (ERR_BATCH, RawBatchCheck::REFUSED_BATCH_ROWS_ZERO) => Err(Error::ZeroBatchRows),
        (ERR_BATCH, RawBatchCheck::REFUSED_BUFFER_BYTES_ZERO) => Err(Error::ZeroBufferBytes),
        (ERR_BATCH, RawBatchCheck::REFUSED_ROW_TOO_WIDE) => Err(Error::RowTooWide {
            width: found.width,
            needed: found.needed,
            limit: found.limit,
        }),
        (ERR_BATCH, _) => Err(Error::BatchTooLarge {
            needed: found.needed,
            limit: found.limit,
        }),
        // A spec this crate built names a door that is one.
        _ => Err(Error::Plan { column: 0 }),
    }
}

/// The rows of a sampler's batch for a sampler wanting `wanted` rows: the core's answer, which only
/// a row past the limit refuses. `wanted` being past the limit is why it asks.
pub(crate) fn sample_rows(
    specs: &[ColumnSpecWith],
    wanted: usize,
    flags: u32,
) -> Result<usize, Error> {
    let mut found = RawBatchCheck::default();
    size::check_batch(specs, wanted as u64, 1, flags, &mut found);
    match found.refused {
        RawBatchCheck::REFUSED_NONE | RawBatchCheck::REFUSED_OVER_LIMIT => {
            Ok(found.sample_rows as usize)
        }
        _ => check_sizes(specs, wanted, 1, flags).map(|_| 1),
    }
}

/// `len` entries, or [`Error::Plan`] naming plan column `column`, the one whose reach takes them,
/// when that many can't be counted, or [`Error::NoMemory`] when they can't be had.
pub(crate) fn table<T: AllZero>(len: Option<usize>, column: usize) -> Result<Vec<T>, Error> {
    let len = len.ok_or(Error::Plan { column })?;
    zeroed(len).ok_or(Error::NoMemory {
        bytes: (len as u64).saturating_mul(size_of::<T>() as u64),
    })
}

/// A declaration as the packages HyperCast's extensions build lay out a `Constraints`' `packed`
/// bytes, native-endian: six words, then the five decimals, each its magnitude's low 64 bits and
/// high 32, its scale and its sign. `None` for bytes of another length.
#[cfg(any(feature = "python", feature = "ruby", feature = "php"))]
pub(crate) fn raw_constraints(packed: &[u8]) -> Option<RawConstraints> {
    if packed.len() != RawConstraints::SIZE as usize {
        return None;
    }
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(packed.get(at..at + 4)?.try_into().ok()?))
    };
    let decimal = |at: usize| -> Option<Decimal> {
        Some(Decimal {
            lo: u64::from_ne_bytes(packed.get(at..at + 8)?.try_into().ok()?),
            hi: word(at + 8)?,
            scale: *packed.get(at + 12)?,
            negative: *packed.get(at + 13)? != 0,
        })
    };
    Some(RawConstraints {
        size: word(0)?,
        flags: word(4)?,
        rounding: word(8)?,
        min_length: word(12)?,
        max_length: word(16)?,
        _reserved: word(20)?,
        minimum: decimal(24)?,
        maximum: decimal(40)?,
        multiple_of: decimal(56)?,
        exclusive_minimum: decimal(72)?,
        exclusive_maximum: decimal(88)?,
    })
}

/// Why a declaration an extension was handed can't make a plan column.
#[cfg(any(feature = "python", feature = "php"))]
pub(crate) enum Undeclarable {
    /// The packed bytes aren't HyperCast's layout: a binding bug.
    NotPacked,
    /// The declaration is for another door, or the column's door takes none.
    Mismatched,
    /// HyperCast's core refuses the declaration.
    Refused,
}

/// A column of `door` reading `ordinal` that declares the constraints HyperCast's extension packed
/// (`packed` and its `enum`, `entries`) for the door whose code is `declared`.
#[cfg(any(feature = "python", feature = "php"))]
pub(crate) fn checked_from_packed(
    ordinal: usize,
    door: Door,
    declared: u32,
    packed: &[u8],
    entries: &[u8],
) -> Result<Column, Undeclarable> {
    let raw = raw_constraints(packed).ok_or(Undeclarable::NotPacked)?;
    macro_rules! typed {
        ($ty:ty, $door:ident) => {{
            if declared != hypercast::Door::$door.code() {
                return Err(Undeclarable::Mismatched);
            }
            let typed =
                Constraints::<$ty>::from_raw(raw, entries).map_err(|_| Undeclarable::Refused)?;
            Column::checked(ordinal, typed)
        }};
    }
    Ok(match door {
        Door::I8 => typed!(i8, I8),
        Door::I16 => typed!(i16, I16),
        Door::I32 => typed!(i32, I32),
        Door::I64 => typed!(i64, I64),
        Door::U8 => typed!(u8, U8),
        Door::U16 => typed!(u16, U16),
        Door::U32 => typed!(u32, U32),
        Door::U64 => typed!(u64, U64),
        Door::F32 => typed!(f32, F32),
        Door::F64 => typed!(f64, F64),
        Door::Decimal => typed!(Decimal, Decimal),
        Door::Text => typed!(Text, Text),
        // A door that takes no constraints has none for its own door.
        _ => return Err(Undeclarable::Mismatched),
    })
}
