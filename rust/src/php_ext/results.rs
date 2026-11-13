//! The plain classes that say what a read and a suggestion found, and `Range`: `Range`, `Problem`,
//! `SuggestOptions`, `Suggestion`, `SuggestedColumn` and `AmbiguousColumn`, registered as
//! `values.rs` registers `Dialect` and `Column`.

use std::panic::AssertUnwindSafe;

use ext_php_rs::args::{Arg, ArgInfoTables};
use ext_php_rs::builders::FunctionBuilder;
use ext_php_rs::convert::IntoZval;
use ext_php_rs::exception::PhpResult;
use ext_php_rs::flags::{DataType, MethodFlags};
use ext_php_rs::types::Zval;
use ext_php_rs::zend::{ClassEntry, ExecuteData, ce, run_handler};
use ext_php_rs::zend_fastcall;

use super::args::checked_args;
use super::classes::{
    Declaration, Plain, Type, assign, build, bytes_zval, entry, finish, finish_registration,
    instantiate, int_zval, maybe_int_zval, object_of, register as register_class, this,
};
use super::convert::rust_error;
use super::enums::Origin;
use super::errors::{invalid_argument, invalid_argument_exception, pending};
use super::guard;
use super::hypercast::hypercast_enum;
use super::values::construct;
use crate::kernel::abi::{CellVerdict, RawProblem};

const RANGE: Declaration = Declaration {
    name: "HyperTabular\\Range",
    properties: &[
        ("firstRow", Type::Int),
        ("firstColumn", Type::Int),
        ("lastRow", Type::Int),
        ("lastColumn", Type::Int),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

const PROBLEM: Declaration = Declaration {
    name: "HyperTabular\\Problem",
    properties: &[
        ("row", Type::Int),
        ("line", Type::Int),
        ("planColumn", Type::NullableInt),
        ("sourceColumn", Type::NullableInt),
        ("reason", Type::NullableClass("HyperCast\\CastFailure")),
        ("detail", Type::NullableClass("HyperCast\\FaultDetail")),
        ("detailCode", Type::Int),
        ("origin", Type::Class("HyperTabular\\Origin")),
        ("offset", Type::Int),
        ("length", Type::Int),
        ("expected", Type::Int),
        ("found", Type::Int),
        ("keyword", Type::NullableClass("HyperCast\\Keyword")),
        ("keywordCode", Type::Int),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

const SUGGEST_OPTIONS: Declaration = Declaration {
    name: "HyperTabular\\SuggestOptions",
    properties: &[
        ("sampleRows", Type::Int),
        ("format", Type::NullableClass("HyperCast\\NumFormat")),
        ("columnFormats", Type::Array),
        ("dateOrder", Type::NullableClass("HyperCast\\DateOrder")),
        ("columnDateOrders", Type::Array),
    ],
    constants: &[("DEFAULT_SAMPLE_ROWS", 1000)],
    readonly_class: true,
    parent: None,
};

const SUGGESTION: Declaration = Declaration {
    name: "HyperTabular\\Suggestion",
    properties: &[
        ("plan", Type::Array),
        ("report", Type::Array),
        ("truncated", Type::Bool),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

const SUGGESTED_COLUMN: Declaration = Declaration {
    name: "HyperTabular\\SuggestedColumn",
    properties: &[
        ("ordinal", Type::Int),
        ("name", Type::NullableString),
        ("door", Type::Class("HyperTabular\\Door")),
        ("declared", Type::Mixed),
        ("format", Type::NullableClass("HyperCast\\NumFormat")),
        ("candidates", Type::Array),
        ("status", Type::Class("HyperTabular\\SuggestStatus")),
        ("matched", Type::Int),
        ("sampled", Type::Int),
    ],
    constants: &[],
    readonly_class: true,
    parent: None,
};

fn invalid_argument_info() -> &'static ClassEntry {
    invalid_argument_exception()
}

const AMBIGUOUS_COLUMN: Declaration = Declaration {
    name: "HyperTabular\\AmbiguousColumn",
    properties: &[
        ("name", Type::String),
        ("first", Type::Int),
        ("second", Type::Int),
    ],
    constants: &[],
    readonly_class: false,
    parent: Some((invalid_argument_info, "InvalidArgumentException")),
};

macro_rules! keep {
    ($name:ident, $class:expr, $declaration:expr) => {
        fn $name(ce: &'static mut ClassEntry, tables: ArgInfoTables) {
            std::mem::forget(tables);
            finish_registration($class, $declaration, ce);
        }
    };
}

keep!(keep_range, Plain::Range, &RANGE);
keep!(keep_problem, Plain::Problem, &PROBLEM);
keep!(
    keep_suggest_options,
    Plain::SuggestOptions,
    &SUGGEST_OPTIONS
);
keep!(keep_suggestion, Plain::Suggestion, &SUGGESTION);
keep!(
    keep_suggested_column,
    Plain::SuggestedColumn,
    &SUGGESTED_COLUMN
);
keep!(
    keep_ambiguous_column,
    Plain::AmbiguousColumn,
    &AMBIGUOUS_COLUMN
);

/// Registers the classes. MINIT.
pub(crate) fn register() -> ext_php_rs::error::Result<()> {
    let int = DataType::Long;
    let string = DataType::String;
    let boolean = DataType::Bool;
    let array = DataType::Array;
    let mixed = DataType::Mixed;
    let statics = MethodFlags::Public | MethodFlags::Static;
    let public = MethodFlags::Public;
    let range = DataType::object("HyperTabular\\Range");
    let num_format = DataType::object("HyperCast\\NumFormat");
    let date_order = DataType::object("HyperCast\\DateOrder");

    register_class(
        &RANGE,
        vec![
            (
                FunctionBuilder::new("__construct", range_construct)
                    .not_required()
                    .arg(Arg::new("firstRow", int).default("0"))
                    .arg(Arg::new("firstColumn", int).default("0"))
                    .arg(Arg::new("lastRow", int).default("0"))
                    .arg(Arg::new("lastColumn", int).default("0")),
                public,
            ),
            (
                FunctionBuilder::new("parse", range_parse)
                    .arg(Arg::new("text", string))
                    .returns(range, false, false),
                statics,
            ),
        ],
        keep_range,
    )?;

    let mut problem = FunctionBuilder::new("__construct", problem_construct);
    for arg in [
        Arg::new("row", int),
        Arg::new("line", int),
        Arg::new("planColumn", int).allow_null(),
        Arg::new("sourceColumn", int).allow_null(),
        Arg::new("reason", DataType::object("HyperCast\\CastFailure")).allow_null(),
        Arg::new("detail", DataType::object("HyperCast\\FaultDetail")).allow_null(),
        Arg::new("detailCode", int),
        Arg::new("origin", DataType::object("HyperTabular\\Origin")),
        Arg::new("offset", int),
        Arg::new("length", int),
        Arg::new("expected", int),
        Arg::new("found", int),
    ] {
        problem = problem.arg(arg);
    }
    // A violation's keyword, which every other entry leaves out.
    problem = problem
        .not_required()
        .arg(
            Arg::new("keyword", DataType::object("HyperCast\\Keyword"))
                .allow_null()
                .default("null"),
        )
        .arg(Arg::new("keywordCode", int).default("0"));
    register_class(&PROBLEM, vec![(problem, public)], keep_problem)?;

    register_class(
        &SUGGEST_OPTIONS,
        vec![(
            FunctionBuilder::new("__construct", suggest_options_construct)
                .not_required()
                .arg(Arg::new("sampleRows", int).default("self::DEFAULT_SAMPLE_ROWS"))
                .arg(Arg::new("format", num_format).allow_null().default("null"))
                .arg(Arg::new("columnFormats", array).default("[]"))
                .arg(
                    Arg::new("dateOrder", date_order)
                        .allow_null()
                        .default("null"),
                )
                .arg(Arg::new("columnDateOrders", array).default("[]")),
            public,
        )],
        keep_suggest_options,
    )?;

    register_class(
        &SUGGESTION,
        vec![(
            FunctionBuilder::new("__construct", suggestion_construct)
                .arg(Arg::new("plan", array))
                .arg(Arg::new("report", array))
                .arg(Arg::new("truncated", boolean)),
            public,
        )],
        keep_suggestion,
    )?;

    register_class(
        &SUGGESTED_COLUMN,
        vec![(
            FunctionBuilder::new("__construct", suggested_column_construct)
                .arg(Arg::new("ordinal", int))
                .arg(Arg::new("name", string).allow_null())
                .arg(Arg::new("door", DataType::object("HyperTabular\\Door")))
                .arg(Arg::new("declared", mixed))
                .arg(Arg::new("format", num_format).allow_null())
                .arg(Arg::new("candidates", array))
                .arg(Arg::new(
                    "status",
                    DataType::object("HyperTabular\\SuggestStatus"),
                ))
                .arg(Arg::new("matched", int))
                .arg(Arg::new("sampled", int)),
            public,
        )],
        keep_suggested_column,
    )?;

    register_class(
        &AMBIGUOUS_COLUMN,
        vec![(
            FunctionBuilder::new("__construct", ambiguous_column_construct)
                .arg(Arg::new("name", string))
                .arg(Arg::new("first", int))
                .arg(Arg::new("second", int)),
            public,
        )],
        keep_ambiguous_column,
    )
}

fn range_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let zero = || int_zval(0);
    let args = construct(ex, Plain::Range, &RANGE, &[zero(), zero(), zero(), zero()])?;
    let corner = |at: usize| -> PhpResult<u32> {
        let value = args[at].long().unwrap_or_default();
        u32::try_from(value).map_err(|_| {
            invalid_argument(format!(
                "A range's corner is 1-based, with 0 for no limit; got {value}"
            ))
        })
    };
    crate::Range::new(corner(0)?, corner(1)?, corner(2)?, corner(3)?).map_err(rust_error)?;
    Ok(Zval::new())
}

fn range_from_text(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let Some(args) = checked_args(ex, 1, 1) else {
        return Err(pending("argument check"));
    };
    let text = args[0]
        .zend_str()
        .map(|text| text.as_bytes())
        .unwrap_or_default();
    let text = std::str::from_utf8(text).map_err(|_| invalid_argument("not a range"))?;
    let range = crate::Range::parse(text).map_err(rust_error)?;
    build(
        Plain::Range,
        vec![
            int_zval(i64::from(range.first_row())),
            int_zval(i64::from(range.first_column())),
            int_zval(i64::from(range.last_row())),
            int_zval(i64::from(range.last_column())),
        ],
    )
}

zend_fastcall! {
    extern "C" fn range_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| range_new(ex)), retval)));
    }
}

zend_fastcall! {
    extern "C" fn range_parse(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| range_from_text(ex)), retval)));
    }
}

fn problem_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    construct(ex, Plain::Problem, &PROBLEM, &[Zval::new(), int_zval(0)])?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn problem_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| problem_new(ex)), retval)));
    }
}

/// HyperCast's `Keyword` for a violated constraint's code, or the bare code for one this
/// HyperCast doesn't name.
pub(crate) fn keyword(code: u32) -> PhpResult<Zval> {
    let named = keyword_case(code)?;
    Ok(if named.is_null() {
        int_zval(i64::from(code))
    } else {
        named
    })
}

/// HyperCast's `Keyword` for `code`, or null: none for a cell that broke nothing.
fn keyword_case(code: u32) -> PhpResult<Zval> {
    if code == 0 {
        return Ok(Zval::new());
    }
    super::classes::call(
        None,
        "HyperCast\\Keyword::tryFrom",
        &mut [int_zval(i64::from(code))],
    )
}

/// A list entry of `Batch::faults()`, from the core's.
pub(crate) fn problem(raw: &RawProblem, line: u32, source: Option<usize>) -> PhpResult<Zval> {
    let row_level = raw.plan_column == RawProblem::ROW_LEVEL;
    // A violation cast: it has its keyword, and no reason.
    let reason = if raw.reason == 0 || raw.reason == CellVerdict::VIOLATION {
        Zval::new()
    } else {
        hypercast_enum("CastFailure", raw.reason)?
    };
    let detail = if raw.detail == 0 {
        Zval::new()
    } else {
        super::classes::call(
            None,
            "HyperCast\\FaultDetail::tryFrom",
            &mut [int_zval(i64::from(raw.detail))],
        )?
    };
    build(
        Plain::Problem,
        vec![
            int_zval(i64::from(raw.row)),
            int_zval(i64::from(line)),
            maybe_int_zval((!row_level).then_some(i64::from(raw.plan_column))),
            maybe_int_zval(source.filter(|_| !row_level).map(|source| source as i64)),
            reason,
            detail,
            int_zval(i64::from(raw.detail)),
            Origin::from_code(raw.origin)
                .into_zval(false)
                .map_err(|_| pending("Origin case"))?,
            int_zval(i64::from(raw.offset)),
            int_zval(i64::from(raw.len)),
            int_zval(i64::from(raw.expected)),
            int_zval(i64::from(raw.found)),
            keyword_case(raw.keyword)?,
            int_zval(i64::from(raw.keyword)),
        ],
    )
}

fn suggest_options_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let defaults = [
        int_zval(1000),
        Zval::new(),
        super::classes::list_zval(Vec::new())?,
        Zval::new(),
        super::classes::list_zval(Vec::new())?,
    ];
    let args = construct(ex, Plain::SuggestOptions, &SUGGEST_OPTIONS, &defaults)?;
    let rows = args[0].long().unwrap_or_default();
    if rows < 0 {
        return Err(invalid_argument(format!(
            "A sample can't be negative; got {rows}"
        )));
    }
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn suggest_options_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| suggest_options_new(ex)), retval)));
    }
}

fn suggestion_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    construct(ex, Plain::Suggestion, &SUGGESTION, &[])?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn suggestion_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| suggestion_new(ex)), retval)));
    }
}

/// A `Suggestion` from its properties, in declaration order.
pub(crate) fn suggestion(values: Vec<Zval>) -> PhpResult<Zval> {
    build(Plain::Suggestion, values)
}

fn suggested_column_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    construct(ex, Plain::SuggestedColumn, &SUGGESTED_COLUMN, &[])?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn suggested_column_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| suggested_column_new(ex)), retval)));
    }
}

/// A `SuggestedColumn` from its properties, in declaration order.
pub(crate) fn suggested_column(values: Vec<Zval>) -> PhpResult<Zval> {
    build(Plain::SuggestedColumn, values)
}

/// Writes an `AmbiguousColumn`'s properties and its message, the core's words.
fn fill_ambiguous(
    raw: *mut ext_php_rs::ffi::zend_object,
    name: &str,
    first: i64,
    second: i64,
) -> PhpResult<()> {
    let scope = entry(Plain::AmbiguousColumn);
    assign(scope, raw, "name", bytes_zval(name.as_bytes()))?;
    assign(scope, raw, "first", int_zval(first))?;
    assign(scope, raw, "second", int_zval(second))?;
    let text = crate::Error::AmbiguousColumn {
        name: name.to_owned(),
        first: first as usize,
        second: second as usize,
    }
    .to_string();
    let exception = std::ptr::from_ref(ce::exception()).cast_mut().cast();
    assign(exception, raw, "message", bytes_zval(text.as_bytes()))
}

fn ambiguous_column_new(ex: &mut ExecuteData) -> PhpResult<Zval> {
    let raw = this(ex)?;
    let Some(args) = checked_args(ex, 3, 3) else {
        return Err(pending("argument check"));
    };
    let name = args[0]
        .zend_str()
        .map(|text| String::from_utf8_lossy(text.as_bytes()).into_owned())
        .unwrap_or_default();
    fill_ambiguous(
        raw,
        &name,
        args[1].long().unwrap_or_default(),
        args[2].long().unwrap_or_default(),
    )?;
    Ok(Zval::new())
}

zend_fastcall! {
    extern "C" fn ambiguous_column_construct(ex: &mut ExecuteData, retval: &mut Zval) {
        run_handler(AssertUnwindSafe(|| finish(guard(|| ambiguous_column_new(ex)), retval)));
    }
}

/// The `AmbiguousColumn` for a name the crate wouldn't choose between.
pub(crate) fn ambiguous_column(name: &str, first: usize, second: usize) -> PhpResult<Zval> {
    let object = instantiate(Plain::AmbiguousColumn)?;
    let raw = object_of(&object).ok_or_else(|| pending("exception"))?;
    fill_ambiguous(raw, name, first as i64, second as i64)?;
    Ok(object)
}
