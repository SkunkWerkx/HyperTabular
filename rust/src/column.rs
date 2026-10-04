//! What the caller declares: which source column each output column reads, the door it
//! casts through, and — for the numeric doors — the notation the text is written in. A
//! plan is a slice of these, and a projection: nothing about the source's width is
//! inferred from it, and one source column may be read through several doors.

use crate::Error;
use crate::kernel::abi::ColumnSpec;
use crate::kernel::door::Door;
use hypercast::{DateOrder, ExcelEpoch, NumFormat, RawNumFormat, UnixPrecision};

/// One output column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// The zero-based column of the source this reads.
    pub ordinal: usize,
    /// The door its cells are cast through.
    pub door: Door,
    /// The numeric notation, for the doors that read numbers.
    pub format: NumFormat,
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
    /// Source column `ordinal` through `door`, in the invariant numeric notation.
    pub const fn new(ordinal: usize, door: Door) -> Column {
        Column {
            ordinal,
            door,
            format: NumFormat::INVARIANT,
        }
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

    /// The same column reading its numbers in `format`. Only the numeric doors (the
    /// integers, the floats and the decimal) read it.
    pub const fn format(self, format: NumFormat) -> Column {
        Column { format, ..self }
    }

    /// The column as the core takes it; `index` is its place in the plan, for the error.
    pub(crate) fn spec(&self, index: usize) -> Result<ColumnSpec, Error> {
        let ordinal = u32::try_from(self.ordinal).map_err(|_| Error::Plan { column: index })?;
        let param = match self.door {
            Door::Unix(precision) => precision as u32,
            Door::DateOrdered(order) | Door::DateTime(order) => order as u32,
            Door::ExcelSerial(epoch) => epoch as u32,
            _ => 0,
        };
        let mut currency = [0u8; 16];
        let symbol = self.format.currency.as_bytes();
        currency
            .get_mut(..symbol.len())
            .ok_or(Error::Plan { column: index })?
            .copy_from_slice(symbol);
        let spec = ColumnSpec {
            ordinal,
            door: self.door.code(),
            param,
            format: RawNumFormat {
                decimal_sep: u32::from(self.format.decimal_sep),
                group_sep: u32::from(self.format.group_sep),
                flags: self.format.flags,
                currency_len: symbol.len() as u32,
                currency,
            },
        };
        // A format the core would refuse is the caller's mistake, said before any read —
        // as is U+0000 for a decimal separator, which the core reads as "none declared".
        if self.format.decimal_sep == '\0' || spec.num_format().is_none() {
            return Err(Error::Plan { column: index });
        }
        Ok(spec)
    }
}

/// A plan as the core takes it, and the number of row slots it reaches.
pub(crate) fn specs(plan: &[Column]) -> Result<(Vec<ColumnSpec>, usize), Error> {
    let specs = plan
        .iter()
        .enumerate()
        .map(|(index, column)| column.spec(index))
        .collect::<Result<Vec<_>, _>>()?;
    let width = plan
        .iter()
        .map(|column| column.ordinal + 1)
        .max()
        .unwrap_or(0);
    Ok((specs, width))
}
