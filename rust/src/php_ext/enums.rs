//! The int-backed enums, with 1.0's names, cases and values (the core's codes).

use std::panic::AssertUnwindSafe;

use ext_php_rs::builders::FunctionBuilder;
use ext_php_rs::class::{ClassEntryInfo, ClassMetadata, ConstructorMeta, RegisteredClass};
use ext_php_rs::convert::IntoZvalDyn;
use ext_php_rs::enum_::{Discriminant, EnumCase, RegisteredEnum};
use ext_php_rs::exception::PhpResult;
use ext_php_rs::flags::{ClassFlags, ConstantFlags, DataType, MethodFlags};
use ext_php_rs::prelude::*;
use ext_php_rs::types::Zval;
use ext_php_rs::zend::{ExecuteData, run_handler};
use ext_php_rs::zend_fastcall;

use super::args::checked_args;
use super::classes::{finish, int_zval};
use super::errors::{contract_violation, pending};
use super::guard;
use crate::kernel::door::Door as Kernel;

/// The door a column is cast through: HyperCast's, plus `Door::Text` for the bytes themselves,
/// numbered as the core numbers them.
///
/// Registered with ext-php-rs's enum builder like the others, but its traits are written out
/// here rather than derived by `#[php_enum]`: 1.0's `Door` had an instance method,
/// `valueSize()`, which a derived enum cannot carry (`#[php_impl]` reads `$this` as a Rust
/// object, which an enum case is not), so it is registered by hand beside the cases.
#[derive(Clone, Copy)]
pub enum Door {
    Bool = 1,
    I8 = 2,
    I16 = 3,
    I32 = 4,
    I64 = 5,
    U8 = 6,
    U16 = 7,
    U32 = 8,
    U64 = 9,
    F32 = 10,
    F64 = 11,
    Uuid = 12,
    Timestamp = 13,
    Unix = 14,
    Date = 15,
    Time = 16,
    Duration = 17,
    Text = 18,
    Decimal = 19,
    DateOrdered = 20,
    DateTime = 21,
    ExcelSerial = 22,
}

/// The cases, in declaration order: name, value, and 1.0's doc comment.
const DOORS: [(Door, &str, &str); 22] = [
    (Door::Bool, "Bool", " `bool`, HyperCast's boolean lexicon."),
    (Door::I8, "I8", " A signed 8-bit integer, as `int`."),
    (Door::I16, "I16", " A signed 16-bit integer, as `int`."),
    (Door::I32, "I32", " A signed 32-bit integer, as `int`."),
    (Door::I64, "I64", " A signed 64-bit integer, as `int`."),
    (Door::U8, "U8", " An unsigned 8-bit integer, as `int`."),
    (Door::U16, "U16", " An unsigned 16-bit integer, as `int`."),
    (Door::U32, "U32", " An unsigned 32-bit integer, as `int`."),
    (
        Door::U64,
        "U64",
        " An unsigned 64-bit integer, as `int`'s two's-complement bit pattern — render with sprintf('%u').",
    ),
    (
        Door::F32,
        "F32",
        " An IEEE single, as `float` (widened losslessly).",
    ),
    (Door::F64, "F64", " An IEEE double, as `float`."),
    (
        Door::Uuid,
        "Uuid",
        " A UUID, as the lowercase hyphenated string.",
    ),
    (
        Door::Timestamp,
        "Timestamp",
        " An RFC 3339 instant, as a UTC DateTimeImmutable (nanoseconds truncate to microseconds).",
    ),
    (
        Door::Unix,
        "Unix",
        " A Unix-epoch integer at a declared precision, as a UTC DateTimeImmutable.",
    ),
    (
        Door::Date,
        "Date",
        " A strict `yyyy-MM-dd` date, as a DateTimeImmutable at UTC midnight.",
    ),
    (
        Door::Time,
        "Time",
        " A 24-hour time of day, as an exact `int` of nanoseconds since midnight.",
    ),
    (
        Door::Duration,
        "Duration",
        " A duration, as HyperCast's `HyperCast\\Duration` pair.",
    ),
    (
        Door::Text,
        "Text",
        " The cell's bytes themselves, as `string` — no cast.",
    ),
    (
        Door::Decimal,
        "Decimal",
        " An exact decimal, as HyperCast's `HyperCast\\Decimal` triple; no float is ever formed.",
    ),
    (
        Door::DateOrdered,
        "DateOrdered",
        " A separated calendar date under a declared field order, as a DateTimeImmutable at UTC midnight.",
    ),
    (
        Door::DateTime,
        "DateTime",
        " A zone-less civil date-time under a declared field order, as a UTC-labeled DateTimeImmutable.",
    ),
    (
        Door::ExcelSerial,
        "ExcelSerial",
        " An Excel date serial under a declared date system, as a UTC DateTimeImmutable.",
    ),
];

const fn case(door: (Door, &'static str, &'static str)) -> EnumCase {
    EnumCase {
        name: door.1,
        discriminant: Some(Discriminant::Int(door.0 as i64)),
        docs: &[],
    }
}

impl Door {
    /// The case with the core's code `code`.
    pub(crate) fn from_code(code: i64) -> Option<Door> {
        DOORS
            .iter()
            .find(|(door, _, _)| *door as i64 == code)
            .map(|(door, _, _)| *door)
    }

    /// Bytes one value of this door takes in a column buffer: the core's layout
    /// (`kernel::abi`'s `ColumnBuffer`), as 1.0's `valueSize()` said it.
    pub(crate) const fn value_size(self) -> usize {
        match self {
            Door::Bool | Door::I8 | Door::U8 => 1,
            Door::I16 | Door::U16 => 2,
            Door::I32 | Door::U32 | Door::F32 | Door::Date | Door::DateOrdered => 4,
            Door::I64 | Door::U64 | Door::F64 | Door::Time | Door::Text => 8,
            Door::Decimal
            | Door::Uuid
            | Door::Timestamp
            | Door::Unix
            | Door::ExcelSerial
            | Door::DateTime
            | Door::Duration => 16,
        }
    }
}

// A compile-time check that 1.0's sizes are the core's.
const _: () = {
    use crate::kernel::abi::Span;
    use hypercast::{CivilDateTime, Date, Decimal, Duration, Timestamp};
    assert!(Door::Text.value_size() == size_of::<Span>());
    assert!(Door::Date.value_size() == size_of::<Date>());
    assert!(Door::Decimal.value_size() == size_of::<Decimal>());
    assert!(Door::Timestamp.value_size() == size_of::<Timestamp>());
    assert!(Door::DateTime.value_size() == size_of::<CivilDateTime>());
    assert!(Door::Duration.value_size() == size_of::<Duration>());
    let _ = Kernel::Bool;
};

impl RegisteredClass for Door {
    const CLASS_NAME: &'static str = "HyperTabular\\Door";
    const BUILDER_MODIFIER: Option<
        fn(ext_php_rs::builders::ClassBuilder) -> ext_php_rs::builders::ClassBuilder,
    > = None;
    const EXTENDS: Option<ClassEntryInfo> = None;
    const IMPLEMENTS: &'static [ClassEntryInfo] = &[];
    const FLAGS: ClassFlags = ClassFlags::Enum;
    const DOC_COMMENTS: &'static [&'static str] = &[
        " The door a column is cast through: HyperCast's, plus `Door::Text` for the bytes",
        " themselves. Named as HyperCast's PHP binding names its doors, and numbered as the core",
        " numbers them. Each case says what a cell of that door is in PHP — the same carrier",
        " HyperCast's own `Cast` returns for it.",
    ];

    fn get_metadata() -> &'static ClassMetadata<Self> {
        static METADATA: ClassMetadata<Door> = ClassMetadata::new(&[]);
        &METADATA
    }

    fn method_builders() -> Vec<(FunctionBuilder<'static>, MethodFlags)> {
        vec![(
            FunctionBuilder::new("valueSize", value_size_handler)
                .returns(DataType::Long, false, false)
                .docs(&[
                    " Bytes one value of this door takes in a column buffer — the core's layout",
                    " (rust/src/kernel/abi.rs, `ColumnBuffer`).",
                    "",
                    " @internal the reader's allocation arithmetic",
                ]),
            MethodFlags::Public,
        )]
    }

    fn constructor() -> Option<ConstructorMeta<Self>> {
        None
    }

    fn constants() -> &'static [(
        &'static str,
        &'static dyn IntoZvalDyn,
        &'static [&'static str],
        ConstantFlags,
    )] {
        &[]
    }
}

impl RegisteredEnum for Door {
    const CASES: &'static [EnumCase] = &[
        case(DOORS[0]),
        case(DOORS[1]),
        case(DOORS[2]),
        case(DOORS[3]),
        case(DOORS[4]),
        case(DOORS[5]),
        case(DOORS[6]),
        case(DOORS[7]),
        case(DOORS[8]),
        case(DOORS[9]),
        case(DOORS[10]),
        case(DOORS[11]),
        case(DOORS[12]),
        case(DOORS[13]),
        case(DOORS[14]),
        case(DOORS[15]),
        case(DOORS[16]),
        case(DOORS[17]),
        case(DOORS[18]),
        case(DOORS[19]),
        case(DOORS[20]),
        case(DOORS[21]),
    ];

    fn from_name(name: &str) -> ext_php_rs::error::Result<Self> {
        DOORS
            .iter()
            .find(|(_, case, _)| *case == name)
            .map(|(door, _, _)| *door)
            .ok_or_else(|| ext_php_rs::error::Error::InvalidEnumCase {
                case: name.to_string(),
            })
    }

    fn to_name(&self) -> &'static str {
        DOORS[*self as usize - 1].1
    }
}

/// `Door::valueSize()`: `$this->value`'s size.
fn value_size_call(ex: &mut ExecuteData) -> PhpResult<Zval> {
    if checked_args(ex, 0, 0).is_none() {
        return Err(pending("argument check"));
    }
    let code = ex
        .This
        .object()
        .and_then(|this| this.get_property::<i64>("value").ok())
        .ok_or_else(contract_violation)?;
    let door = Door::from_code(code).ok_or_else(contract_violation)?;
    Ok(int_zval(door.value_size() as i64))
}

zend_fastcall! {
    extern "C" fn value_size_handler(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| value_size_call(ex)), retval)));
    }
}

/// Why an input could not be read as rows at all — the kinds of `TabularException`. The values
/// are the core's codes.
#[php_enum]
#[php(name = "HyperTabular\\TabularFailure")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabularFailure {
    /// The input ended inside a quoted cell.
    #[php(value = 1)]
    UnclosedQuote,
    /// A record's cell count disagrees with the first record's.
    #[php(value = 2)]
    ColumnCount,
    /// A single record is larger than `DelimitedReader::MAX_ROW_BYTES`.
    #[php(value = 3)]
    RowTooLong,
    /// The workbook's container is not a zip file.
    #[php(value = 16)]
    NotAZip,
    /// The zip's own structure is broken.
    #[php(value = 17)]
    Container,
    /// The workbook is encrypted.
    #[php(value = 18)]
    Encrypted,
    /// A part is compressed by a method other than stored or deflate.
    #[php(value = 19)]
    Method,
    /// A part the workbook cannot be read without is missing.
    #[php(value = 20)]
    MissingPart,
    /// A part's XML ends inside a construct.
    #[php(value = 21)]
    Xml,
    /// A part's bytes are not a deflate stream, or stop before the stream does.
    #[php(value = 22)]
    Deflate,
    /// The zip is not an XLSX, ODS or Numbers workbook.
    #[php(value = 23)]
    NotAWorkbook,
    /// A cell names a shared string the table does not have.
    #[php(value = 24)]
    SharedString,
    /// More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2
    /// GiB in a cell.
    #[php(value = 25)]
    TooLarge,
    /// A Numbers part is not what the IWA packaging says: found is 1 for a broken chunk or
    /// Snappy block, 2 for a malformed message, 3 for an object a reference names that is not
    /// there (expected is its identifier), 4 for cell storage of a version the core does not
    /// read (expected is the version).
    #[php(value = 26)]
    Iwa,
}

impl TabularFailure {
    /// The kind for the core's code; a code this binding does not know is the container's,
    /// the most general refusal, as 1.0's `TabularException::from` said.
    pub(crate) fn from_code(code: u32) -> TabularFailure {
        match code {
            1 => TabularFailure::UnclosedQuote,
            2 => TabularFailure::ColumnCount,
            3 => TabularFailure::RowTooLong,
            16 => TabularFailure::NotAZip,
            18 => TabularFailure::Encrypted,
            19 => TabularFailure::Method,
            20 => TabularFailure::MissingPart,
            21 => TabularFailure::Xml,
            22 => TabularFailure::Deflate,
            23 => TabularFailure::NotAWorkbook,
            24 => TabularFailure::SharedString,
            25 => TabularFailure::TooLarge,
            26 => TabularFailure::Iwa,
            _ => TabularFailure::Container,
        }
    }
}

/// Which kind of workbook a `Workbook` is. The values are the core's codes.
#[php_enum]
#[php(name = "HyperTabular\\WorkbookFormat")]
#[derive(Clone, Copy)]
pub enum WorkbookFormat {
    /// Office Open XML: .xlsx, .xlsm.
    #[php(value = 1)]
    Xlsx,
    /// OpenDocument: .ods.
    #[php(value = 2)]
    Ods,
    /// Apple Numbers: .numbers, each table a sheet.
    #[php(value = 3)]
    Numbers,
}

impl WorkbookFormat {
    /// The format the core reported.
    pub(crate) fn from_code(code: u32) -> Option<WorkbookFormat> {
        match code {
            1 => Some(WorkbookFormat::Xlsx),
            2 => Some(WorkbookFormat::Ods),
            3 => Some(WorkbookFormat::Numbers),
            _ => None,
        }
    }
}
