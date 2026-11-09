//! `Dialect`, `Column`, `SheetInfo`, `SheetOptions` and `TabularException`: 1.0's classes with
//! constructor-promoted properties, registered as plain PHP classes (`classes.rs`), with 1.0's
//! constructors, factories and messages, and what the core has gained since.

use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::panic::AssertUnwindSafe;

use ext_php_rs::args::{Arg, ArgInfoTables};
use ext_php_rs::builders::FunctionBuilder;
use ext_php_rs::convert::IntoZval;
use ext_php_rs::exception::PhpResult;
use ext_php_rs::flags::{DataType, MethodFlags};
use ext_php_rs::types::Zval;
use ext_php_rs::zend::{ClassEntry, ExecuteData, ce, run_handler};
use ext_php_rs::zend_fastcall;

use super::args::{BatchRows, checked_args};
use super::classes::{
    Declaration, Plain, Type, assign, bool_zval, build, bytes_zval, entry, finish,
    finish_registration, instantiate, int_zval, list_zval, object_zval_ref,
    register as register_class, this,
};
use super::convert::token_strings;
use super::enums::{Door, Encoding, LongRows, ShortRows, TabularFailure};
use super::errors::{invalid_argument, pending, runtime_exception};
use super::guard;
use super::hypercast::{enum_value, invariant_format};
use crate::column::MAX_ORDINAL;

const DIALECT: Declaration = Declaration {
    name: "HyperTabular\\Dialect",
    properties: &[
        ("separator", Type::String),
        ("quoting", Type::Bool),
        ("hasHeader", Type::Bool),
        ("skipBlankLines", Type::Bool),
        ("skip", Type::Int),
        ("headerRow", Type::Int),
        ("shortRows", Type::Class("HyperTabular\\ShortRows")),
        ("longRows", Type::Class("HyperTabular\\LongRows")),
        ("encoding", Type::Class("HyperTabular\\Encoding")),
        ("tokens", Type::Array),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

const COLUMN: Declaration = Declaration {
    name: "HyperTabular\\Column",
    properties: &[
        ("ordinal", Type::Int),
        ("door", Type::Class("HyperTabular\\Door")),
        ("format", Type::Class("HyperCast\\NumFormat")),
        ("declared", Type::Int),
        ("tokens", Type::NullableArray),
        ("dateOptions", Type::NullableClass("HyperCast\\DateOptions")),
        ("months", Type::NullableClass("HyperCast\\MonthNames")),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

const SHEET_INFO: Declaration = Declaration {
    name: "HyperTabular\\SheetInfo",
    properties: &[
        ("name", Type::String),
        ("hidden", Type::Bool),
        ("part", Type::String),
        ("index", Type::Int),
    ],
    constants: &[],
    readonly_class: false,
    parent: None,
};

const SHEET_OPTIONS: Declaration = Declaration {
    name: "HyperTabular\\SheetOptions",
    properties: &[
        ("hasHeader", Type::Bool),
        ("skipEmptyRows", Type::Bool),
        ("batchRows", Type::Int),
        ("skip", Type::Int),
        ("headerRow", Type::Int),
        ("range", Type::NullableClass("HyperTabular\\Range")),
        ("tokens", Type::Array),
        ("notes", Type::Bool),
    ],
    constants: &[],
    readonly_class: false,
    parent: None,
};

fn runtime_exception_info() -> &'static ClassEntry {
    runtime_exception()
}

const TABULAR_EXCEPTION: Declaration = Declaration {
    name: "HyperTabular\\TabularException",
    properties: &[
        ("kind", Type::Class("HyperTabular\\TabularFailure")),
        ("record", Type::Int),
        ("recordLine", Type::Int),
        ("byte", Type::Int),
        ("expected", Type::Int),
        ("found", Type::Int),
    ],
    constants: &[],
    readonly_class: false,
    parent: Some((runtime_exception_info, "RuntimeException")),
};

fn keep_dialect(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
    std::mem::forget(tables);
    finish_registration(Plain::Dialect, &DIALECT, ce);
}

fn keep_column(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
    std::mem::forget(tables);
    finish_registration(Plain::Column, &COLUMN, ce);
}

fn keep_sheet_info(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
    std::mem::forget(tables);
    finish_registration(Plain::SheetInfo, &SHEET_INFO, ce);
}

fn keep_sheet_options(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
    std::mem::forget(tables);
    finish_registration(Plain::SheetOptions, &SHEET_OPTIONS, ce);
}

fn keep_tabular_exception(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
    std::mem::forget(tables);
    finish_registration(Plain::TabularException, &TABULAR_EXCEPTION, ce);
}

/// The factories' shapes: `(int $ordinal)`, `(int $ordinal, ?NumFormat $format = null)`, and
/// `(int $ordinal, <HyperCast enum> $declared)`.
enum Shape {
    Plain,
    Numeric,
    Declared(&'static str, &'static str),
}

/// Column's factories, in 1.0's order: name, door, shape, doc comment.
static FACTORIES: [(&str, Door, Shape, &str); 22] = [
    (
        "bool",
        Door::Bool,
        Shape::Plain,
        " A boolean column: HyperCast's boolean lexicon, as `bool`.",
    ),
    (
        "i8",
        Door::I8,
        Shape::Numeric,
        " A signed 8-bit integer column, as `int`.",
    ),
    (
        "i16",
        Door::I16,
        Shape::Numeric,
        " A signed 16-bit integer column, as `int`.",
    ),
    (
        "i32",
        Door::I32,
        Shape::Numeric,
        " A signed 32-bit integer column, as `int`.",
    ),
    (
        "i64",
        Door::I64,
        Shape::Numeric,
        " A signed 64-bit integer column, as `int`.",
    ),
    (
        "u8",
        Door::U8,
        Shape::Numeric,
        " An unsigned 8-bit integer column, as `int`.",
    ),
    (
        "u16",
        Door::U16,
        Shape::Numeric,
        " An unsigned 16-bit integer column, as `int`.",
    ),
    (
        "u32",
        Door::U32,
        Shape::Numeric,
        " An unsigned 32-bit integer column, as `int`.",
    ),
    (
        "u64",
        Door::U64,
        Shape::Numeric,
        " An unsigned 64-bit integer column, as `int`'s two's-complement bit pattern (sprintf('%u')).",
    ),
    (
        "f32",
        Door::F32,
        Shape::Numeric,
        " An IEEE single column, as `float` (widened losslessly).",
    ),
    (
        "f64",
        Door::F64,
        Shape::Numeric,
        " An IEEE double column, as `float`.",
    ),
    (
        "decimal",
        Door::Decimal,
        Shape::Numeric,
        " An exact decimal column, as HyperCast's `Decimal` triple.",
    ),
    (
        "uuid",
        Door::Uuid,
        Shape::Plain,
        " A UUID column, as the lowercase hyphenated string.",
    ),
    (
        "timestamp",
        Door::Timestamp,
        Shape::Plain,
        " An RFC 3339 instant column, as a UTC DateTimeImmutable.",
    ),
    (
        "unix",
        Door::Unix,
        Shape::Declared("precision", "HyperCast\\UnixPrecision"),
        " A Unix-epoch column at the declared precision, as a UTC DateTimeImmutable.",
    ),
    (
        "excelSerial",
        Door::ExcelSerial,
        Shape::Declared("epoch", "HyperCast\\ExcelEpoch"),
        " An Excel date-serial column under the declared date system, as a UTC DateTimeImmutable.",
    ),
    (
        "date",
        Door::Date,
        Shape::Plain,
        " A strict `yyyy-MM-dd` date column, as a DateTimeImmutable at UTC midnight.",
    ),
    (
        "dateOrdered",
        Door::DateOrdered,
        Shape::Declared("order", "HyperCast\\DateOrder"),
        " A separated-date column under the declared field order, as a DateTimeImmutable at UTC midnight.",
    ),
    (
        "datetime",
        Door::DateTime,
        Shape::Declared("order", "HyperCast\\DateOrder"),
        " A zone-less civil date-time column under the declared field order, on a UTC-labeled DateTimeImmutable.",
    ),
    (
        "time",
        Door::Time,
        Shape::Plain,
        " A 24-hour time-of-day column, as an exact `int` of nanoseconds since midnight.",
    ),
    (
        "duration",
        Door::Duration,
        Shape::Plain,
        " A duration column, as HyperCast's `Duration` pair.",
    ),
    (
        "text",
        Door::Text,
        Shape::Plain,
        " A text column: the cell's bytes themselves, untrimmed, quotes resolved.",
    ),
];

/// Registers the five classes. MINIT.
pub(crate) fn register() -> ext_php_rs::error::Result<()> {
    let int = DataType::Long;
    let string = DataType::String;
    let boolean = DataType::Bool;
    let statics = MethodFlags::Public | MethodFlags::Static;
    let dialect = DataType::object("HyperTabular\\Dialect");
    register_class(
        &DIALECT,
        vec![
            (
                FunctionBuilder::new("__construct", dialect_construct)
                    .arg(Arg::new("separator", string))
                    .not_required()
                    .arg(Arg::new("quoting", boolean).default("true"))
                    .arg(Arg::new("hasHeader", boolean).default("true"))
                    .arg(Arg::new("skipBlankLines", boolean).default("true"))
                    .arg(Arg::new("skip", int).default("0"))
                    .arg(Arg::new("headerRow", int).default("0"))
                    .arg(
                        Arg::new("shortRows", DataType::object("HyperTabular\\ShortRows"))
                            .default("\\HyperTabular\\ShortRows::Reject"),
                    )
                    .arg(
                        Arg::new("longRows", DataType::object("HyperTabular\\LongRows"))
                            .default("\\HyperTabular\\LongRows::Reject"),
                    )
                    .arg(
                        Arg::new("encoding", DataType::object("HyperTabular\\Encoding"))
                            .default("\\HyperTabular\\Encoding::Utf8"),
                    )
                    .arg(Arg::new("tokens", DataType::Array).default("[]")),
                MethodFlags::Public,
            ),
            (
                FunctionBuilder::new("csv", dialect_csv).returns(dialect, false, false),
                statics,
            ),
            (
                FunctionBuilder::new("tsv", dialect_tsv).returns(dialect, false, false),
                statics,
            ),
            (
                FunctionBuilder::new("psv", dialect_psv).returns(dialect, false, false),
                statics,
            ),
        ],
        keep_dialect,
    )?;

    let column = DataType::object("HyperTabular\\Column");
    let mut methods = vec![(
        FunctionBuilder::new("__construct", column_construct)
            .arg(Arg::new("ordinal", int))
            .arg(Arg::new("door", DataType::object("HyperTabular\\Door")))
            .arg(Arg::new("format", DataType::object("HyperCast\\NumFormat")))
            .not_required()
            .arg(Arg::new("declared", int).default("0"))
            .arg(
                Arg::new("tokens", DataType::Array)
                    .allow_null()
                    .default("null"),
            )
            .arg(
                Arg::new("dateOptions", DataType::object("HyperCast\\DateOptions"))
                    .allow_null()
                    .default("null"),
            )
            .arg(
                Arg::new("months", DataType::object("HyperCast\\MonthNames"))
                    .allow_null()
                    .default("null"),
            ),
        MethodFlags::Private,
    )];
    methods.push((
        FunctionBuilder::new("withTokens", column_with_tokens)
            .arg(Arg::new("tokens", DataType::Array))
            .returns(column, false, false),
        MethodFlags::Public,
    ));
    methods.push((
        FunctionBuilder::new("withDateOptions", column_with_date_options)
            .arg(Arg::new(
                "options",
                DataType::object("HyperCast\\DateOptions"),
            ))
            .not_required()
            .arg(
                Arg::new("months", DataType::object("HyperCast\\MonthNames"))
                    .allow_null()
                    .default("null"),
            )
            .returns(column, false, false),
        MethodFlags::Public,
    ));
    for (index, (_, _, shape, doc)) in FACTORIES.iter().enumerate() {
        let mut function = factory_builder(index).arg(Arg::new("ordinal", int));
        function = match shape {
            Shape::Plain => function,
            Shape::Numeric => function.not_required().arg(
                Arg::new("format", DataType::object("HyperCast\\NumFormat"))
                    .allow_null()
                    .default("null"),
            ),
            Shape::Declared(arg, class) => function.arg(Arg::new(*arg, DataType::object(class))),
        };
        methods.push((
            function
                .returns(column, false, false)
                .docs(std::slice::from_ref(doc)),
            statics,
        ));
    }
    register_class(&COLUMN, methods, keep_column)?;

    register_class(
        &SHEET_INFO,
        vec![(
            FunctionBuilder::new("__construct", sheet_info_construct)
                .arg(Arg::new("name", string))
                .arg(Arg::new("hidden", boolean))
                .arg(Arg::new("part", string))
                .arg(Arg::new("index", int)),
            MethodFlags::Public,
        )],
        keep_sheet_info,
    )?;

    register_class(
        &SHEET_OPTIONS,
        vec![(
            FunctionBuilder::new("__construct", sheet_options_construct)
                .not_required()
                .arg(Arg::new("hasHeader", boolean).default("true"))
                .arg(Arg::new("skipEmptyRows", boolean).default("true"))
                .arg(
                    Arg::new("batchRows", int)
                        .default("\\HyperTabular\\DelimitedReader::DEFAULT_BATCH_ROWS"),
                )
                .arg(Arg::new("skip", int).default("0"))
                .arg(Arg::new("headerRow", int).default("0"))
                .arg(
                    Arg::new("range", DataType::object("HyperTabular\\Range"))
                        .allow_null()
                        .default("null"),
                )
                .arg(Arg::new("tokens", DataType::Array).default("[]"))
                .arg(Arg::new("notes", boolean).default("true")),
            MethodFlags::Public,
        )],
        keep_sheet_options,
    )?;

    register_class(
        &TABULAR_EXCEPTION,
        vec![(
            FunctionBuilder::new("__construct", tabular_exception_construct)
                .arg(Arg::new(
                    "kind",
                    DataType::object("HyperTabular\\TabularFailure"),
                ))
                .arg(Arg::new("record", int))
                .arg(Arg::new("recordLine", int))
                .arg(Arg::new("byte", int))
                .not_required()
                .arg(Arg::new("expected", int).default("0"))
                .arg(Arg::new("found", int).default("0")),
            MethodFlags::Public,
        )],
        keep_tabular_exception,
    )
}

/// The promoted assignments of a constructor called from PHP, in declaration order, with the
/// declared defaults for the arguments left out.
pub(crate) fn construct(
    ex: &mut ExecuteData,
    class: Plain,
    declaration: &Declaration,
    defaults: &[Zval],
) -> PhpResult<Vec<Zval>> {
    let object = this(ex)?;
    let count = declaration.properties.len() as u32;
    let Some(mut args) = checked_args(ex, count - defaults.len() as u32, count) else {
        return Err(pending("argument check"));
    };
    let missing = declaration.properties.len() - args.len();
    args.extend(
        defaults[defaults.len() - missing..]
            .iter()
            .map(Zval::shallow_clone),
    );
    for ((name, _), arg) in declaration.properties.iter().zip(&args) {
        assign(entry(class), object, name, arg.shallow_clone())?;
    }
    Ok(args)
}

fn dialect_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let mut defaults = vec![bool_zval(true), bool_zval(true), bool_zval(true)];
    defaults.extend(dialect_defaults()?);
    let args = construct(ex, Plain::Dialect, &DIALECT, &defaults)?;
    // One byte is all this layer can check without repeating the core's rule; which bytes the
    // scanner honours is the core's to say, when a reader is built.
    let length = args[0].zend_str().map_or(0, |s| s.len());
    if length != 1 {
        return Err(invalid_argument(format!(
            "The separator must be a single ASCII byte; got {length} bytes"
        )));
    }
    check_line_count(&args[4], "skip")?;
    check_line_count(&args[5], "headerRow")?;
    token_strings(&args[9], "A dialect's tokens")?;
    Ok(Zval::new())
}

/// The defaults of a dialect's properties after its first four.
fn dialect_defaults() -> PhpResult<[Zval; 6]> {
    Ok([
        int_zval(0),
        int_zval(0),
        ShortRows::Reject
            .into_zval(false)
            .map_err(|_| pending("ShortRows case"))?,
        LongRows::Reject
            .into_zval(false)
            .map_err(|_| pending("LongRows case"))?,
        Encoding::Utf8
            .into_zval(false)
            .map_err(|_| pending("Encoding case"))?,
        list_zval(Vec::new())?,
    ])
}

/// A count of lines or records to pass over, which the core holds in 32 bits.
fn check_line_count(value: &Zval, what: &str) -> PhpResult<()> {
    let value = value.long().unwrap_or_default();
    if u32::try_from(value).is_err() {
        return Err(invalid_argument(format!(
            "{what} must not be negative and must fit 32 bits; got {value}"
        )));
    }
    Ok(())
}

thread_local! {
    /// `csv()`, `tsv()` and `psv()` for the current request, one instance each per request, as
    /// 1.0's function-static caches held them. Released at request shutdown
    /// ([`release_shared`]) and only there: `ManuallyDrop`, so the thread-local has no
    /// destructor to run after the engine's heap is gone.
    static SHARED: RefCell<[Option<ManuallyDrop<Zval>>; 3]> = const { RefCell::new([None, None, None]) };

    /// Whether a request is running (see `mod.rs`'s lifecycle callbacks): outside one nothing
    /// is cached, since nothing would release it before the engine frees its heap.
    static IN_REQUEST: Cell<bool> = const { Cell::new(false) };
}

const _: () = assert!(!std::mem::needs_drop::<[Option<ManuallyDrop<Zval>>; 3]>());

/// A dialect of `separator`, quoted, with a header, blank lines skipped.
fn plain_dialect(separator: u8) -> PhpResult<Zval> {
    let mut values = vec![
        bytes_zval(&[separator]),
        bool_zval(true),
        bool_zval(true),
        bool_zval(true),
    ];
    values.extend(dialect_defaults()?);
    build(Plain::Dialect, values)
}

fn shared(ex: &mut ExecuteData, slot: usize) -> PhpResult<Zval> {
    if checked_args(ex, 0, 0).is_none() {
        return Err(pending("argument check"));
    }
    let separator = b",\t|"[slot];
    if !IN_REQUEST.get() {
        return plain_dialect(separator);
    }
    SHARED.with_borrow_mut(|cache| {
        if cache[slot].is_none() {
            cache[slot] = Some(ManuallyDrop::new(plain_dialect(separator)?));
        }
        match &cache[slot] {
            Some(zv) => Ok(zv.shallow_clone()),
            None => Err(pending("shared dialect")),
        }
    })
}

/// RSHUTDOWN: stops caching and releases the request's shared dialects while the engine can
/// still free them.
pub(crate) fn release_shared() {
    IN_REQUEST.set(false);
    let released = SHARED.with_borrow_mut(std::mem::take);
    for zv in released.into_iter().flatten() {
        drop(ManuallyDrop::into_inner(zv));
    }
}

/// Post-RSHUTDOWN: ends the request for the cache, forgetting (never releasing) anything left,
/// which the engine has freed with the request when the RSHUTDOWNs were skipped.
pub(crate) fn end_request() {
    IN_REQUEST.set(false);
    forget_stale();
}

/// Forgets, without releasing, what an earlier request left cached.
pub(crate) fn forget_stale() {
    SHARED.with_borrow_mut(|cache| cache.fill_with(|| None));
}

/// RINIT, last: starts caching.
pub(crate) fn begin_request() {
    IN_REQUEST.set(true);
}

zend_fastcall! {
    extern "C" fn dialect_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| dialect_new(ex)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn dialect_csv(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| shared(ex, 0)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn dialect_tsv(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| shared(ex, 1)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn dialect_psv(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| shared(ex, 2)), retval)));
    }
}

/// 1.0's ordinal check.
fn check_ordinal(ordinal: i64) -> PhpResult<()> {
    if !(0..=MAX_ORDINAL as i64).contains(&ordinal) {
        return Err(invalid_argument(format!(
            "A column's ordinal must be below 2^20 (1,048,576); got {ordinal}"
        )));
    }
    Ok(())
}

fn column_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let defaults = [int_zval(0), Zval::new(), Zval::new(), Zval::new()];
    let args = construct(ex, Plain::Column, &COLUMN, &defaults)?;
    check_ordinal(args[0].long().unwrap_or_default())?;
    if !args[4].is_null() {
        token_strings(&args[4], "A column's tokens")?;
    }
    Ok(Zval::new())
}

/// A copy of the running `Column` with `tokens`, `dateOptions` and `months` as given, each `None`
/// left as it is.
fn column_changed(
    ex: &mut ExecuteData,
    tokens: Option<Zval>,
    declared: Option<(Zval, Zval)>,
) -> PhpResult<Zval> {
    let this = object_zval_ref(this(ex)?);
    let scope = entry(Plain::Column);
    let field = |name| super::classes::read(scope, &this, name);
    let (options, months) = match declared {
        Some(both) => both,
        None => (field("dateOptions")?, field("months")?),
    };
    build(
        Plain::Column,
        vec![
            field("ordinal")?,
            field("door")?,
            field("format")?,
            field("declared")?,
            match tokens {
                Some(tokens) => tokens,
                None => field("tokens")?,
            },
            options,
            months,
        ],
    )
}

fn column_tokens_call(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let Some(args) = checked_args(ex, 1, 1) else {
        return Err(pending("argument check"));
    };
    let tokens = token_strings(&args[0], "A column's tokens")?;
    let list = list_zval(
        tokens
            .iter()
            .map(|token| bytes_zval(token.as_bytes()))
            .collect(),
    )?;
    column_changed(ex, Some(list), None)
}

fn column_date_options_call(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let Some(args) = checked_args(ex, 1, 2) else {
        return Err(pending("argument check"));
    };
    let months = args.get(1).map_or_else(Zval::new, Zval::shallow_clone);
    column_changed(ex, None, Some((args[0].shallow_clone(), months)))
}

zend_fastcall! {
    extern "C" fn column_with_tokens(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| column_tokens_call(ex)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn column_with_date_options(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| column_date_options_call(ex)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn column_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| column_new(ex)), retval)));
    }
}

/// A column of `door` reading `ordinal`, in `format` (the invariant notation when null), with
/// `declared` beside the door.
pub(crate) fn column(
    ordinal: i64,
    door: Door,
    format: Option<Zval>,
    declared: i64,
) -> PhpResult<Zval> {
    check_ordinal(ordinal)?;
    let format = match format {
        Some(format) if !format.is_null() => format,
        _ => invariant_format()?,
    };
    let door = door.into_zval(false).map_err(|_| pending("Door case"))?;
    build(
        Plain::Column,
        vec![
            int_zval(ordinal),
            door,
            format,
            int_zval(declared),
            Zval::new(),
            Zval::new(),
            Zval::new(),
        ],
    )
}

/// One of Column's factories, by its place in [`FACTORIES`].
fn factory(ex: &mut ExecuteData, index: usize) -> PhpResult<Zval> {
    let (_, door, shape, _) = &FACTORIES[index];
    let (min, max) = match shape {
        Shape::Plain => (1, 1),
        Shape::Numeric => (1, 2),
        Shape::Declared(..) => (2, 2),
    };
    let Some(args) = checked_args(ex, min, max) else {
        return Err(pending("argument check"));
    };
    let ordinal = args[0].long().unwrap_or_default();
    match shape {
        Shape::Plain => column(ordinal, *door, None, 0),
        Shape::Numeric => column(ordinal, *door, args.get(1).map(Zval::shallow_clone), 0),
        Shape::Declared(..) => {
            let declared = enum_value(&args[1]).ok_or_else(|| pending("enum value"))?;
            column(ordinal, *door, None, declared)
        }
    }
}

zend_fastcall! {
    extern "C" fn factory_bool(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 0)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_i8(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 1)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_i16(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 2)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_i32(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 3)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_i64(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 4)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_u8(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 5)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_u16(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 6)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_u32(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 7)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_u64(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 8)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_f32(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 9)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_f64(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 10)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_decimal(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 11)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_uuid(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 12)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_timestamp(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 13)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_unix(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 14)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_excel_serial(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 15)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_date(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 16)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_date_ordered(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 17)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_datetime(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 18)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_time(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 19)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_duration(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 20)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn factory_text(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| factory(ex, 21)), retval)));
    }
}

/// The builder of the factory at `index` in [`FACTORIES`], its handler named.
fn factory_builder(index: usize) -> FunctionBuilder<'static> {
    match index {
        0 => FunctionBuilder::new("bool", factory_bool),
        1 => FunctionBuilder::new("i8", factory_i8),
        2 => FunctionBuilder::new("i16", factory_i16),
        3 => FunctionBuilder::new("i32", factory_i32),
        4 => FunctionBuilder::new("i64", factory_i64),
        5 => FunctionBuilder::new("u8", factory_u8),
        6 => FunctionBuilder::new("u16", factory_u16),
        7 => FunctionBuilder::new("u32", factory_u32),
        8 => FunctionBuilder::new("u64", factory_u64),
        9 => FunctionBuilder::new("f32", factory_f32),
        10 => FunctionBuilder::new("f64", factory_f64),
        11 => FunctionBuilder::new("decimal", factory_decimal),
        12 => FunctionBuilder::new("uuid", factory_uuid),
        13 => FunctionBuilder::new("timestamp", factory_timestamp),
        14 => FunctionBuilder::new("unix", factory_unix),
        15 => FunctionBuilder::new("excelSerial", factory_excel_serial),
        16 => FunctionBuilder::new("date", factory_date),
        17 => FunctionBuilder::new("dateOrdered", factory_date_ordered),
        18 => FunctionBuilder::new("datetime", factory_datetime),
        19 => FunctionBuilder::new("time", factory_time),
        20 => FunctionBuilder::new("duration", factory_duration),
        21 => FunctionBuilder::new("text", factory_text),
        _ => FunctionBuilder::new("text", factory_text),
    }
}

fn sheet_info_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    construct(ex, Plain::SheetInfo, &SHEET_INFO, &[])?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn sheet_info_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| sheet_info_new(ex)), retval)));
    }
}

/// A `SheetInfo` the binding builds as it lists a workbook's sheets.
pub(crate) fn sheet_info(name: &[u8], hidden: bool, part: &[u8], index: u32) -> PhpResult<Zval> {
    build(
        Plain::SheetInfo,
        vec![
            bytes_zval(name),
            bool_zval(hidden),
            bytes_zval(part),
            int_zval(i64::from(index)),
        ],
    )
}

fn sheet_options_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let defaults = [
        bool_zval(true),
        bool_zval(true),
        int_zval(BatchRows::DEFAULT),
        int_zval(0),
        int_zval(0),
        Zval::new(),
        list_zval(Vec::new())?,
        bool_zval(true),
    ];
    let args = construct(ex, Plain::SheetOptions, &SHEET_OPTIONS, &defaults)?;
    // Zero, and every size past what the core allows, the core refuses at opening the sheet.
    let batch_rows = args[2].long().unwrap_or_default();
    if batch_rows < 0 {
        return Err(invalid_argument(format!(
            "A batch must hold at least one row; got {batch_rows}"
        )));
    }
    check_line_count(&args[3], "skip")?;
    check_line_count(&args[4], "headerRow")?;
    token_strings(&args[6], "A sheet's tokens")?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn sheet_options_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| sheet_options_new(ex)), retval)));
    }
}

/// 1.0's message for each kind of structural failure.
fn message(
    kind: TabularFailure,
    record: i64,
    line: i64,
    byte: i64,
    expected: i64,
    found: i64,
) -> String {
    match kind {
        TabularFailure::ColumnCount => format!(
            "Record {record} (line {line}, byte {byte}) has {found} cells; the first record had {expected}."
        ),
        TabularFailure::UnclosedQuote => format!(
            "The input ended inside a quoted cell in record {record} (line {line}, byte {byte})."
        ),
        TabularFailure::RowTooLong => format!(
            "Record {record} (line {line}, byte {byte}) exceeds the {}-byte row ceiling.",
            crate::DelimitedReader::MAX_ROW_BYTES
        ),
        TabularFailure::NotAZip => "The workbook is not a zip file.".to_owned(),
        TabularFailure::Container => "The workbook's zip structure is broken.".to_owned(),
        TabularFailure::Encrypted => "The workbook is encrypted.".to_owned(),
        TabularFailure::Method => format!(
            "Part {record} of the workbook is compressed by method {found}, neither stored nor deflate."
        ),
        TabularFailure::MissingPart => {
            format!("Part {record}, which the workbook cannot be read without, is missing.")
        }
        TabularFailure::Xml => {
            format!("Part {record} of the workbook ends inside an XML construct (byte {byte}).")
        }
        TabularFailure::Deflate => {
            format!("Part {record} of the workbook is not a whole deflate stream (byte {byte}).")
        }
        TabularFailure::NotAWorkbook => {
            "The zip is not an XLSX, ODS or Numbers workbook.".to_owned()
        }
        TabularFailure::SharedString => {
            format!("Row {line} names shared string {found}; the table has {expected}.")
        }
        TabularFailure::TooLarge => {
            "The workbook holds more text than a batch can address.".to_owned()
        }
        TabularFailure::Iwa => format!(
            "Part {record} of the Numbers document is not what the IWA packaging says (byte {byte}, detail {found})."
        ),
        TabularFailure::Encoding => format!(
            "Record {record} (line {line}, byte {byte}) has text its encoding has no character for."
        ),
    }
}

/// Writes a `TabularException`'s properties and its message, as 1.0's constructor did.
fn fill_exception(object: &Zval, args: &[Zval]) -> PhpResult<()> {
    let raw = super::classes::object_of(object).ok_or_else(|| pending("exception"))?;
    let scope = entry(Plain::TabularException);
    for ((name, _), arg) in TABULAR_EXCEPTION.properties.iter().zip(args) {
        assign(scope, raw, name, arg.shallow_clone())?;
    }
    let kind = enum_value(&args[0])
        .and_then(|value| u32::try_from(value).ok())
        .map(TabularFailure::from_code)
        .ok_or_else(|| pending("TabularFailure case"))?;
    let int = |i: usize| args[i].long().unwrap_or_default();
    let text = message(kind, int(1), int(2), int(3), int(4), int(5));
    let exception = std::ptr::from_ref(ce::exception()).cast_mut().cast();
    assign(exception, raw, "message", bytes_zval(text.as_bytes()))
}

fn tabular_exception_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let object = this(ex)?;
    let Some(mut args) = checked_args(ex, 4, 6) else {
        return Err(pending("argument check"));
    };
    while args.len() < 6 {
        args.push(int_zval(0));
    }
    fill_exception(&super::classes::object_zval_ref(object), &args)?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn tabular_exception_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| tabular_exception_new(ex)), retval)));
    }
}

/// A `TabularException` of `kind`, as `new TabularException(...)` makes one.
pub(crate) fn tabular_exception_of(
    kind: TabularFailure,
    record: i64,
    line: i64,
    byte: i64,
    expected: i64,
    found: i64,
) -> PhpResult<Zval> {
    let object = instantiate(Plain::TabularException)?;
    let kind = kind
        .into_zval(false)
        .map_err(|_| pending("TabularFailure case"))?;
    let args = [
        kind,
        int_zval(record),
        int_zval(line),
        int_zval(byte),
        int_zval(expected),
        int_zval(found),
    ];
    fill_exception(&object, &args)?;
    Ok(object)
}

/// A `Column`'s properties, as a plan reads them.
pub(crate) struct ColumnFields {
    pub(crate) ordinal: i64,
    pub(crate) door: i64,
    pub(crate) format: Zval,
    pub(crate) declared: i64,
    pub(crate) tokens: Zval,
    pub(crate) date_options: Zval,
    pub(crate) months: Zval,
}

/// Reads a plan entry, which must be a `Column`; `None` for anything else.
pub(crate) fn column_fields(column: &Zval) -> PhpResult<Option<ColumnFields>> {
    if !super::classes::is_a(column, Plain::Column) {
        return Ok(None);
    }
    let scope = entry(Plain::Column);
    let read = |name| super::classes::read(scope, column, name);
    let ordinal = read("ordinal")?.long().unwrap_or_default();
    let door = enum_value(&read("door")?).ok_or_else(|| pending("door"))?;
    let format = read("format")?;
    let declared = read("declared")?.long().unwrap_or_default();
    Ok(Some(ColumnFields {
        ordinal,
        door,
        format,
        declared,
        tokens: read("tokens")?,
        date_options: read("dateOptions")?,
        months: read("months")?,
    }))
}

/// Registers every plain class. MINIT.
pub(crate) fn register_all() -> ext_php_rs::error::Result<()> {
    register()?;
    super::results::register()
}
