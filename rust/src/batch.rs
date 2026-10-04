//! The column-major batch: per plan column a typed value vector and a parallel
//! [`CellVerdict`] array, `Text` values in one shared byte arena, and a fault table that
//! records the raw text of every `Malformed`/`OutOfRange` cell so the error story stays
//! data after the provider's buffer has moved on. Reused across fills — vectors are
//! cleared, never reallocated once warm — and crossed once per fill by the FFI layer.

use crate::cast::{
    cast_bool, cast_date, cast_date_ordered, cast_datetime, cast_decimal, cast_duration,
    cast_excel_serial, cast_f32, cast_f64, cast_i8, cast_i16, cast_i32, cast_i64, cast_text,
    cast_time, cast_timestamp, cast_u8, cast_u16, cast_u32, cast_u64, cast_unix, cast_uuid,
};
use crate::cell::Cell;
use crate::kernel::abi::{CellVerdict, Span};
use crate::plan::{Column, Door, Plan};
use crate::render::render;
use crate::source::{Row, TabularSource};
use core::ffi::c_void;
use hypercast::{CivilDateTime, Date, Decimal, Duration, ExcelEpoch, Fault, Reason, Timestamp};

/// One entry of the fault table: which cell faulted and where its raw text sits in the
/// arena. Only `Malformed` and `OutOfRange` verdicts are recorded — `Empty` has no text.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRaw {
    /// Row index within the batch.
    pub row: u32,
    /// Plan column index.
    pub column: u32,
    /// The cell's raw text (or a typed cell's canonical rendering) in the arena.
    pub raw: Span,
}

/// The typed values of one column. Every vector is `#[repr(C)]`-friendly: `bool` is one
/// byte of `0`/`1`, HyperCast's temporal structs are `#[repr(C)]`, and `Text` holds spans
/// into the arena.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    Bool(Vec<bool>),
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Uuid(Vec<[u8; 16]>),
    Timestamp(Vec<Timestamp>),
    Date(Vec<Date>),
    /// Nanoseconds since midnight.
    Time(Vec<u64>),
    Duration(Vec<Duration>),
    Text(Vec<Span>),
    Decimal(Vec<Decimal>),
    DateTime(Vec<CivilDateTime>),
}

impl Values {
    fn for_door(door: Door) -> Values {
        match door {
            Door::Bool => Values::Bool(Vec::new()),
            Door::I8 => Values::I8(Vec::new()),
            Door::I16 => Values::I16(Vec::new()),
            Door::I32 => Values::I32(Vec::new()),
            Door::I64 => Values::I64(Vec::new()),
            Door::U8 => Values::U8(Vec::new()),
            Door::U16 => Values::U16(Vec::new()),
            Door::U32 => Values::U32(Vec::new()),
            Door::U64 => Values::U64(Vec::new()),
            Door::F32 => Values::F32(Vec::new()),
            Door::F64 => Values::F64(Vec::new()),
            Door::Uuid => Values::Uuid(Vec::new()),
            Door::Timestamp | Door::Unix(_) | Door::ExcelSerial(_) => Values::Timestamp(Vec::new()),
            Door::Date | Door::DateOrdered(_) => Values::Date(Vec::new()),
            Door::Decimal => Values::Decimal(Vec::new()),
            Door::DateTime(_) => Values::DateTime(Vec::new()),
            Door::Time => Values::Time(Vec::new()),
            Door::Duration => Values::Duration(Vec::new()),
            Door::Text => Values::Text(Vec::new()),
        }
    }

    fn clear(&mut self) {
        match self {
            Values::Bool(v) => v.clear(),
            Values::I8(v) => v.clear(),
            Values::I16(v) => v.clear(),
            Values::I32(v) => v.clear(),
            Values::I64(v) => v.clear(),
            Values::U8(v) => v.clear(),
            Values::U16(v) => v.clear(),
            Values::U32(v) => v.clear(),
            Values::U64(v) => v.clear(),
            Values::F32(v) => v.clear(),
            Values::F64(v) => v.clear(),
            Values::Uuid(v) => v.clear(),
            Values::Timestamp(v) => v.clear(),
            Values::Date(v) => v.clear(),
            Values::Time(v) => v.clear(),
            Values::Duration(v) => v.clear(),
            Values::Text(v) => v.clear(),
            Values::Decimal(v) => v.clear(),
            Values::DateTime(v) => v.clear(),
        }
    }

    /// The number of values.
    pub fn len(&self) -> usize {
        match self {
            Values::Bool(v) => v.len(),
            Values::I8(v) => v.len(),
            Values::I16(v) => v.len(),
            Values::I32(v) => v.len(),
            Values::I64(v) => v.len(),
            Values::U8(v) => v.len(),
            Values::U16(v) => v.len(),
            Values::U32(v) => v.len(),
            Values::U64(v) => v.len(),
            Values::F32(v) => v.len(),
            Values::F64(v) => v.len(),
            Values::Uuid(v) => v.len(),
            Values::Timestamp(v) => v.len(),
            Values::Date(v) => v.len(),
            Values::Time(v) => v.len(),
            Values::Duration(v) => v.len(),
            Values::Text(v) => v.len(),
            Values::Decimal(v) => v.len(),
            Values::DateTime(v) => v.len(),
        }
    }

    /// True when there are no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The start of the value array as it crosses the ABI: `uint8_t` for `Bool`, the
    /// matching C integer/float for the numerics, 16 bytes per UUID, HyperCast's
    /// `#[repr(C)]` structs for temporals, and [`Span`] pairs for `Text`.
    pub fn as_ptr(&self) -> *const c_void {
        match self {
            Values::Bool(v) => v.as_ptr().cast(),
            Values::I8(v) => v.as_ptr().cast(),
            Values::I16(v) => v.as_ptr().cast(),
            Values::I32(v) => v.as_ptr().cast(),
            Values::I64(v) => v.as_ptr().cast(),
            Values::U8(v) => v.as_ptr().cast(),
            Values::U16(v) => v.as_ptr().cast(),
            Values::U32(v) => v.as_ptr().cast(),
            Values::U64(v) => v.as_ptr().cast(),
            Values::F32(v) => v.as_ptr().cast(),
            Values::F64(v) => v.as_ptr().cast(),
            Values::Uuid(v) => v.as_ptr().cast(),
            Values::Timestamp(v) => v.as_ptr().cast(),
            Values::Date(v) => v.as_ptr().cast(),
            Values::Time(v) => v.as_ptr().cast(),
            Values::Duration(v) => v.as_ptr().cast(),
            Values::Text(v) => v.as_ptr().cast(),
            Values::Decimal(v) => v.as_ptr().cast(),
            Values::DateTime(v) => v.as_ptr().cast(),
        }
    }
}

/// One plan column's output: values plus parallel verdicts, both `rows` long.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnData {
    column: Column,
    values: Values,
    verdicts: Vec<CellVerdict>,
}

macro_rules! typed_accessors {
    ($($name:ident => $variant:ident $ty:ty),+ $(,)?) => {$(
        /// The values as a typed slice, when this column's door produces that type.
        pub fn $name(&self) -> Option<&[$ty]> {
            match &self.values {
                Values::$variant(v) => Some(v),
                _ => None,
            }
        }
    )+};
}

impl ColumnData {
    /// The plan column this data was produced for.
    pub fn column(&self) -> &Column {
        &self.column
    }

    /// The typed values.
    pub fn values(&self) -> &Values {
        &self.values
    }

    /// The verdicts, parallel to the values.
    pub fn verdicts(&self) -> &[CellVerdict] {
        &self.verdicts
    }

    typed_accessors! {
        bools => Bool bool,
        i8s => I8 i8,
        i16s => I16 i16,
        i32s => I32 i32,
        i64s => I64 i64,
        u8s => U8 u8,
        u16s => U16 u16,
        u32s => U32 u32,
        u64s => U64 u64,
        f32s => F32 f32,
        f64s => F64 f64,
        uuids => Uuid [u8; 16],
        timestamps => Timestamp Timestamp,
        dates => Date Date,
        times => Time u64,
        durations => Duration Duration,
        spans => Text Span,
        decimals => Decimal Decimal,
        datetimes => DateTime CivilDateTime,
    }
}

/// The batch. See the module doc.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    rows: usize,
    columns: Vec<ColumnData>,
    arena: Vec<u8>,
    faults: Vec<FaultRaw>,
}

impl Batch {
    /// An empty batch; the first fill sizes it, later fills reuse it.
    pub fn new() -> Batch {
        Batch::default()
    }

    /// Rows filled by the last fill.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The columns, in plan order.
    pub fn columns(&self) -> &[ColumnData] {
        &self.columns
    }

    /// One column by plan index.
    pub fn column(&self, index: usize) -> &ColumnData {
        &self.columns[index]
    }

    /// The byte arena `Text` spans and fault raws index into.
    pub fn arena(&self) -> &[u8] {
        &self.arena
    }

    /// The fault table: every `Malformed`/`OutOfRange` cell with its raw text.
    pub fn faults(&self) -> &[FaultRaw] {
        &self.faults
    }

    /// The raw text of a faulted cell.
    pub fn raw(&self, fault: &FaultRaw) -> &[u8] {
        self.slice(fault.raw)
    }

    /// A `Text` column's value at `row`, or `None` if the column is not a text column or
    /// the cell did not cast.
    pub fn text(&self, column: usize, row: usize) -> Option<&[u8]> {
        let data = self.columns.get(column)?;
        if !data.verdicts.get(row)?.is_ok() {
            return None;
        }
        match &data.values {
            Values::Text(spans) => Some(self.slice(spans[row])),
            _ => None,
        }
    }

    fn slice(&self, span: Span) -> &[u8] {
        &self.arena[span.offset as usize..(span.offset + span.len) as usize]
    }

    /// Resets the batch for `plan`: rebuilds the column set only if the plan changed,
    /// otherwise just clears.
    pub fn prepare(&mut self, plan: &Plan) {
        let same = self.columns.len() == plan.len()
            && self
                .columns
                .iter()
                .zip(plan.columns())
                .all(|(data, column)| data.column == *column);
        if !same {
            self.columns = plan
                .columns()
                .iter()
                .map(|&column| ColumnData {
                    column,
                    values: Values::for_door(column.door),
                    verdicts: Vec::new(),
                })
                .collect();
        }
        for data in &mut self.columns {
            data.values.clear();
            data.verdicts.clear();
        }
        self.arena.clear();
        self.faults.clear();
        self.rows = 0;
    }

    fn push_arena(&mut self, bytes: &[u8]) -> Span {
        let offset = self.arena.len() as u32;
        self.arena.extend_from_slice(bytes);
        Span {
            offset,
            len: bytes.len() as u32,
        }
    }

    /// Casts one row under `plan` (which must be the plan [`Batch::prepare`] saw) and
    /// appends it.
    pub fn push_row<R: Row + ?Sized>(&mut self, row: &R, plan: &Plan, system: ExcelEpoch) {
        let row_index = self.rows as u32;
        for (index, column) in plan.columns().iter().enumerate() {
            let cell = row.cell(column.ordinal);
            let format = &column.format;
            let outcome: Result<(), Fault> = match (&mut self.columns[index].values, column.door) {
                (Values::Bool(v), _) => push_or(v, cast_bool(&cell), false),
                (Values::I8(v), _) => push_or(v, cast_i8(&cell, format), 0),
                (Values::I16(v), _) => push_or(v, cast_i16(&cell, format), 0),
                (Values::I32(v), _) => push_or(v, cast_i32(&cell, format), 0),
                (Values::I64(v), _) => push_or(v, cast_i64(&cell, format), 0),
                (Values::U8(v), _) => push_or(v, cast_u8(&cell, format), 0),
                (Values::U16(v), _) => push_or(v, cast_u16(&cell, format), 0),
                (Values::U32(v), _) => push_or(v, cast_u32(&cell, format), 0),
                (Values::U64(v), _) => push_or(v, cast_u64(&cell, format), 0),
                (Values::F32(v), _) => push_or(v, cast_f32(&cell, format), 0.0),
                (Values::F64(v), _) => push_or(v, cast_f64(&cell, format), 0.0),
                (Values::Uuid(v), _) => push_or(v, cast_uuid(&cell), [0; 16]),
                (Values::Timestamp(v), Door::ExcelSerial(epoch)) => push_or(
                    v,
                    cast_excel_serial(&cell, epoch),
                    Timestamp {
                        seconds: 0,
                        nanos: 0,
                    },
                ),
                (Values::Date(v), Door::DateOrdered(order)) => push_or(
                    v,
                    cast_date_ordered(&cell, order, system),
                    Date {
                        year: 1,
                        month: 1,
                        day: 1,
                    },
                ),
                (Values::DateTime(v), door) => {
                    let order = match door {
                        Door::DateTime(order) => order,
                        _ => hypercast::DateOrder::YearMonthDay,
                    };
                    push_or(
                        v,
                        cast_datetime(&cell, order, system),
                        CivilDateTime {
                            date: Date {
                                year: 1,
                                month: 1,
                                day: 1,
                            },
                            nanos_of_day: 0,
                        },
                    )
                }
                (Values::Decimal(v), _) => push_or(
                    v,
                    cast_decimal(&cell, format),
                    Decimal {
                        lo: 0,
                        hi: 0,
                        scale: 0,
                        negative: false,
                    },
                ),
                (Values::Timestamp(v), Door::Unix(precision)) => push_or(
                    v,
                    cast_unix(&cell, precision),
                    Timestamp {
                        seconds: 0,
                        nanos: 0,
                    },
                ),
                (Values::Timestamp(v), _) => push_or(
                    v,
                    cast_timestamp(&cell, system),
                    Timestamp {
                        seconds: 0,
                        nanos: 0,
                    },
                ),
                (Values::Date(v), _) => push_or(
                    v,
                    cast_date(&cell, system),
                    Date {
                        year: 1,
                        month: 1,
                        day: 1,
                    },
                ),
                (Values::Time(v), _) => push_or(v, cast_time(&cell), 0),
                (Values::Duration(v), _) => push_or(
                    v,
                    cast_duration(&cell),
                    Duration {
                        seconds: 0,
                        nanos: 0,
                    },
                ),
                (Values::Text(_), _) => match cast_text(&cell) {
                    Ok(text) => {
                        let span = self.push_arena(&text);
                        if let Values::Text(v) = &mut self.columns[index].values {
                            v.push(span);
                        }
                        Ok(())
                    }
                    Err(fault) => {
                        if let Values::Text(v) = &mut self.columns[index].values {
                            v.push(Span::default());
                        }
                        Err(fault)
                    }
                },
            };
            let verdict = match outcome {
                Ok(()) => CellVerdict::OK,
                Err(fault) if fault.reason == Reason::Empty => CellVerdict::from_fault(fault),
                Err(fault) => {
                    let raw = self.render_raw(&cell);
                    self.faults.push(FaultRaw {
                        row: row_index,
                        column: index as u32,
                        raw,
                    });
                    let mut verdict = CellVerdict::from_fault(fault);
                    if !matches!(cell, Cell::Text(_)) {
                        // A typed cell's fault spans its whole rendering.
                        verdict.len = raw.len;
                    }
                    verdict
                }
            };
            self.columns[index].verdicts.push(verdict);
        }
        self.rows += 1;
    }

    fn render_raw(&mut self, cell: &Cell<'_>) -> Span {
        let offset = self.arena.len() as u32;
        render(cell, &mut self.arena);
        Span {
            offset,
            len: self.arena.len() as u32 - offset,
        }
    }
}

fn push_or<T: Copy>(values: &mut Vec<T>, verdict: Result<T, Fault>, zero: T) -> Result<(), Fault> {
    match verdict {
        Ok(value) => {
            values.push(value);
            Ok(())
        }
        Err(fault) => {
            values.push(zero);
            Err(fault)
        }
    }
}

/// Pulls up to `max_rows` rows from `source` through `plan` into `batch`, returning how
/// many landed (`0` once the source is exhausted). A structural error from the provider
/// is returned as-is; the rows already in the batch stay readable.
pub fn fill_batch<S: TabularSource + ?Sized>(
    source: &mut S,
    plan: &Plan,
    batch: &mut Batch,
    max_rows: usize,
) -> Result<usize, S::Error> {
    batch.prepare(plan);
    let system = source.date_system();
    while batch.rows < max_rows {
        match source.next_row()? {
            Some(row) => batch.push_row(&row, plan, system),
            None => break,
        }
    }
    Ok(batch.rows)
}
