//! This binding's values as the crate takes them and the crate's as this binding hands them out:
//! the PHP classes' properties read into `crate::Column`, `crate::Dialect`, `crate::SheetOptions`
//! and `crate::SuggestOptions`, a `crate::Suggestion` built back into PHP's, and a `crate::Error`
//! as what PHP throws. Whatever a rule says is the crate's: nothing here decides one.

use ext_php_rs::convert::IntoZval;
use ext_php_rs::exception::{PhpException, PhpResult};
use ext_php_rs::types::Zval;
use ext_php_rs::zend::ExecutorGlobals;
use hypercast::{DateOptions, DateOrder, MonthNames, NumFormat};

use super::args::instance_of;
use super::classes::{
    Plain, bool_zval, bytes_zval, entry, int_zval, list_zval, new_named, object_zval, read,
    read_own,
};
use super::enums::{Door, SuggestStatus};
use super::errors::{
    contract_violation, invalid_argument, logic, object, out_of_bounds, pending, runtime,
};
use super::hypercast::{
    comma_decimal_format, enum_value, hypercast_enum, invariant_format, packed_format,
};
use super::results::{ambiguous_column, suggested_column, suggestion};
use super::values::{ColumnFields, column, tabular_exception_of};
use crate::column::MAX_ORDINAL;
use crate::kernel::abi::ColumnSpec;
use crate::kernel::door::Door as Kernel;
use crate::kernel::suggest::{COMMA_DECIMAL, COMMA_GROUPS};
use crate::{Candidate, Encoding, LongRows, ShortRows};

/// What reading a plan-less reader or sheet is told.
const UNBOUND: &str = "has no plan to read through; bind() one first";
/// What a second `bind()` is told.
const ALREADY_BOUND: &str = "already has a plan; a plan is bound once";

/// A `crate::Error` as this binding throws it. Every size the core refuses is PHP's usual argument
/// error, in the core's words.
pub(crate) fn rust_error(error: crate::Error) -> PhpException {
    match error {
        crate::Error::Structure(failure) => match failure_zval(failure) {
            Ok(exception) => object(&exception),
            Err(failed) => failed,
        },
        crate::Error::Io(error) => runtime(format!("hypertabular: {error}")),
        crate::Error::NoSheet(which) => out_of_bounds(format!("The workbook has no sheet {which}")),
        crate::Error::Unbound => logic(format!("The reader {UNBOUND}")),
        crate::Error::AlreadyBound => logic(format!("The reader {ALREADY_BOUND}")),
        crate::Error::AmbiguousColumn {
            name,
            first,
            second,
        } => match ambiguous_column(&name, first, second) {
            Ok(exception) => object(&exception),
            Err(failed) => failed,
        },
        crate::Error::NoColumn(name) => {
            out_of_bounds(format!("The header has no column named \"{name}\""))
        }
        other => invalid_argument(other.to_string()),
    }
}

/// The `TabularException` for a failure the crate reported.
pub(crate) fn failure_zval(failure: crate::Failure) -> PhpResult<Zval> {
    tabular_exception_of(
        super::enums::TabularFailure::from_code(failure.kind as u32),
        failure.record as i64,
        i64::from(failure.line),
        failure.byte as i64,
        i64::from(failure.expected),
        i64::from(failure.found),
    )
}

/// A negative count, which is PHP's to refuse as no count at all; `0` and everything else is the
/// core's to judge.
pub(crate) fn count(value: i64, what: &str) -> PhpResult<usize> {
    usize::try_from(value).map_err(|_| invalid_argument(format!("{what}; got {value}")))
}

/// A count that a `u32` of the core's holds.
fn small_count(value: i64, what: &str) -> PhpResult<u32> {
    u32::try_from(value)
        .map_err(|_| invalid_argument(format!("{what} must fit 32 bits, unsigned; got {value}")))
}

/// A `HyperCast\NumFormat` as the crate takes it. A stale one throws from its own `packed()`.
///
/// HyperCast's own `packed()` refuses a format holding a word it no longer makes (one forged past
/// its constructor and `unserialize`); that refusal is this one too, its exception kept as the
/// previous one, so that a stale word is refused in the same words wherever it is caught.
pub(crate) fn num_format_of(format: &Zval, what: &str) -> PhpResult<NumFormat> {
    let refused = || format!("{what} cannot be honoured");
    if !instance_of(format, b"HyperCast\\NumFormat") {
        return Err(invalid_argument(format!(
            "{what} must be a HyperCast\\NumFormat"
        )));
    }
    let format = match packed_format(format) {
        Ok(format) => format,
        Err(failure) => {
            let Some(previous) = ExecutorGlobals::take_exception() else {
                return Err(failure);
            };
            let previous = object_zval(previous.into_raw().cast());
            let wrapped = new_named(
                "InvalidArgumentException",
                &mut [bytes_zval(refused().as_bytes()), int_zval(0), previous],
            )?;
            return Err(object(&wrapped));
        }
    };
    let spec = ColumnSpec {
        ordinal: 0,
        door: Kernel::I32.code(),
        param: 0,
        format,
    };
    spec.num_format().ok_or_else(|| invalid_argument(refused()))
}

/// A list of tokens: strings, each UTF-8, as the crate's `&str`s are.
pub(crate) fn token_strings(list: &Zval, what: &str) -> PhpResult<Vec<String>> {
    let Some(table) = list.array() else {
        return Err(invalid_argument(format!(
            "{what} must be a list of strings"
        )));
    };
    let mut tokens = Vec::with_capacity(table.len());
    for token in table.values() {
        let bytes = token
            .zend_str()
            .ok_or_else(|| invalid_argument(format!("{what} must be a list of strings")))?;
        let text = std::str::from_utf8(bytes.as_bytes())
            .map_err(|_| invalid_argument(format!("{what} must be UTF-8")))?;
        tokens.push(text.to_owned());
    }
    Ok(tokens)
}

fn str_refs(tokens: &[String]) -> Vec<&str> {
    tokens.iter().map(String::as_str).collect()
}

/// One plan column as the crate takes it, checked: the door it names and everything it declares.
pub(crate) fn column_of(
    index: usize,
    fields: &ColumnFields,
    ordinal: usize,
) -> PhpResult<crate::Column> {
    let format = num_format_of(&fields.format, &format!("Plan column {index}'s format"))?;
    // `Column` refuses an ordinal past the cap itself; one that got past it is refused here.
    if ordinal > MAX_ORDINAL {
        return Err(invalid_argument(format!(
            "Plan column {index} reads source column {ordinal}; an ordinal must be below 2^20 (1,048,576)"
        )));
    }
    let door = u32::try_from(fields.door)
        .ok()
        .zip(u32::try_from(fields.declared).ok())
        .and_then(|(door, param)| Kernel::from_code(door, param))
        .ok_or_else(|| {
            invalid_argument(format!(
                "Plan column {index} names no door; build it with Column's factories"
            ))
        })?;
    let mut made = crate::Column::new(ordinal, door).format(format);
    if !fields.tokens.is_null() {
        let tokens = token_strings(&fields.tokens, &format!("Plan column {index}'s tokens"))?;
        made = made.tokens(&str_refs(&tokens));
    }
    if !fields.date_options.is_null() || !fields.months.is_null() {
        let options = if fields.date_options.is_null() {
            DateOptions::NONE
        } else {
            let flags = read_own(&fields.date_options, "flags")?;
            let pivot = read_own(&fields.date_options, "pivot")?;
            let flags = u32::try_from(flags.long().unwrap_or_default()).ok();
            let pivot = u16::try_from(pivot.long().unwrap_or_default()).ok();
            let (Some(flags), Some(pivot)) = (flags, pivot) else {
                return Err(invalid_argument(format!(
                    "Plan column {index}'s date options cannot be honoured"
                )));
            };
            DateOptions::from_flags(flags).with_pivot(pivot)
        };
        let packed = if fields.months.is_null() {
            None
        } else {
            Some(super::classes::call(
                Some(&fields.months),
                "packed",
                &mut [],
            )?)
        };
        let names = packed
            .as_ref()
            .map(|packed| {
                MonthNames::from_packed(
                    packed
                        .zend_str()
                        .map(|text| text.as_bytes())
                        .unwrap_or_default(),
                )
            })
            .transpose()
            .map_err(|_| {
                invalid_argument(format!("Plan column {index}'s months cannot be honoured"))
            })?;
        made = made.date_options(options, names);
    }
    made.spec_with(index).map_err(|_| {
        invalid_argument(format!(
            "Plan column {index}'s date options cannot be honoured"
        ))
    })?;
    Ok(made)
}

/// A `Dialect` as the crate takes it. Each distinct token list is kept for the life of the process.
pub(crate) fn dialect_of(dialect: &Zval) -> PhpResult<crate::Dialect> {
    let scope = entry(Plain::Dialect);
    let field = |name| read(scope, dialect, name);
    let separator = field("separator")?;
    let byte = separator
        .zend_str()
        .and_then(|text| text.as_bytes().first().copied())
        .unwrap_or(0);
    let short_rows = match enum_value(&field("shortRows")?) {
        Some(1) => ShortRows::Pad,
        _ => ShortRows::Reject,
    };
    let long_rows = match enum_value(&field("longRows")?) {
        Some(1) => LongRows::Report,
        Some(2) => LongRows::Truncate,
        _ => LongRows::Reject,
    };
    let encoding = match enum_value(&field("encoding")?) {
        Some(1) => Encoding::Windows1252,
        Some(2) => Encoding::Latin1,
        Some(3) => Encoding::Utf16Le,
        _ => Encoding::Utf8,
    };
    let skip = field("skip")?.long().unwrap_or_default();
    let header_row = field("headerRow")?.long().unwrap_or_default();
    let tokens = token_strings(&field("tokens")?, "A dialect's tokens")?;
    let mut made = crate::Dialect::new(byte)
        .with_quoting(field("quoting")?.bool().unwrap_or_default())
        .with_header(field("hasHeader")?.bool().unwrap_or_default())
        .with_blank_lines_skipped(field("skipBlankLines")?.bool().unwrap_or_default())
        .with_skip(small_count(skip, "A dialect's skip")?)
        .with_header_row(small_count(header_row, "A dialect's headerRow")?)
        .with_short_rows(short_rows)
        .with_long_rows(long_rows)
        .with_encoding(encoding);
    if !tokens.is_empty() {
        made = made.with_tokens(&str_refs(&tokens));
    }
    Ok(made)
}

/// A `SheetOptions` as the crate takes it. Each distinct token list is kept for the life of the
/// process.
pub(crate) fn sheet_options_of(options: &Zval) -> PhpResult<crate::SheetOptions> {
    let scope = entry(Plain::SheetOptions);
    let field = |name| read(scope, options, name);
    let batch_rows = field("batchRows")?.long().unwrap_or_default();
    let skip = field("skip")?.long().unwrap_or_default();
    let header_row = field("headerRow")?.long().unwrap_or_default();
    let tokens = token_strings(&field("tokens")?, "A sheet's tokens")?;
    let mut made = crate::SheetOptions::new()
        .with_header(field("hasHeader")?.bool().unwrap_or_default())
        .with_empty_rows_skipped(field("skipEmptyRows")?.bool().unwrap_or_default())
        .with_batch_rows(count(batch_rows, "A batch must hold at least one row")?)
        .with_skip(small_count(skip, "A sheet's skip")?)
        .with_header_row(small_count(header_row, "A sheet's headerRow")?)
        .with_notes(field("notes")?.bool().unwrap_or_default());
    let range = field("range")?;
    if !range.is_null() {
        made = made.with_range(range_of(&range)?);
    }
    if !tokens.is_empty() {
        made = made.with_tokens(&str_refs(&tokens));
    }
    Ok(made)
}

/// A `Range` as the crate takes it.
fn range_of(range: &Zval) -> PhpResult<crate::Range> {
    let scope = entry(Plain::Range);
    let corner = |name: &str| -> PhpResult<u32> {
        small_count(read(scope, range, name)?.long().unwrap_or_default(), name)
    };
    crate::Range::new(
        corner("firstRow")?,
        corner("firstColumn")?,
        corner("lastRow")?,
        corner("lastColumn")?,
    )
    .map_err(rust_error)
}

/// A date order case as the core numbers it.
fn date_order_of(order: &Zval) -> PhpResult<DateOrder> {
    if !instance_of(order, b"HyperCast\\DateOrder") {
        return Err(invalid_argument(
            "A date order must be a HyperCast\\DateOrder",
        ));
    }
    let code = enum_value(order).and_then(|code| u32::try_from(code).ok());
    match code.and_then(|code| {
        Kernel::from_code(Kernel::DateOrdered(DateOrder::YearMonthDay).code(), code)
    }) {
        Some(Kernel::DateOrdered(order)) => Ok(order),
        _ => Err(invalid_argument(
            "A date order must be a HyperCast\\DateOrder",
        )),
    }
}

/// A `SuggestOptions` as the crate takes it: every format it holds judged, first of anything, as
/// the crate's own suggest would before it opened the source.
pub(crate) fn suggest_options_of(options: Option<&Zval>) -> PhpResult<crate::SuggestOptions> {
    let mut made = crate::SuggestOptions::new();
    let Some(options) = options else {
        return Ok(made);
    };
    let scope = entry(Plain::SuggestOptions);
    let field = |name| read(scope, options, name);
    let sample_rows = field("sampleRows")?.long().unwrap_or_default();
    made = made.sample_rows(count(sample_rows, "A sample can't be negative")?);
    let format = field("format")?;
    if !format.is_null() {
        made = made.format(num_format_of(&format, "SuggestOptions' format")?);
    }
    let column_formats = field("columnFormats")?;
    for (ordinal, format) in ordinal_entries(&column_formats, "columnFormats")? {
        let what = format!("SuggestOptions' columnFormats[{ordinal}]");
        made = made.column_format(ordinal, num_format_of(&format, &what)?);
    }
    let order = field("dateOrder")?;
    if !order.is_null() {
        made = made.date_order(date_order_of(&order)?);
    }
    let column_orders = field("columnDateOrders")?;
    for (ordinal, order) in ordinal_entries(&column_orders, "columnDateOrders")? {
        made = made.column_date_order(ordinal, date_order_of(&order)?);
    }
    Ok(made)
}

/// A map from source column to a declaration: its entries, the keys checked.
fn ordinal_entries(map: &Zval, what: &str) -> PhpResult<Vec<(usize, Zval)>> {
    let mut entries = Vec::new();
    if let Some(table) = map.array() {
        for (key, value) in table {
            let ordinal = match key {
                ext_php_rs::types::ArrayKey::Long(ordinal) => usize::try_from(ordinal).ok(),
                _ => None,
            }
            .ok_or_else(|| {
                invalid_argument(format!("SuggestOptions' {what} is keyed by source column"))
            })?;
            entries.push((ordinal, value.shallow_clone()));
        }
    }
    Ok(entries)
}

/// What a door declares beside itself, as the core numbers it.
fn door_param(door: Kernel) -> i64 {
    i64::from(match door {
        Kernel::Unix(precision) => precision as u32,
        Kernel::DateOrdered(order) | Kernel::DateTime(order) => order as u32,
        Kernel::ExcelSerial(epoch) => epoch as u32,
        _ => 0,
    })
}

/// What a door declares beside itself, as HyperCast's case.
fn door_declared(door: Kernel) -> PhpResult<Zval> {
    match door {
        Kernel::Unix(precision) => hypercast_enum("UnixPrecision", precision as u32),
        Kernel::DateOrdered(order) | Kernel::DateTime(order) => {
            hypercast_enum("DateOrder", order as u32)
        }
        Kernel::ExcelSerial(epoch) => hypercast_enum("ExcelEpoch", epoch as u32),
        _ => Ok(Zval::new()),
    }
}

/// A format a suggestion names, as PHP's: the caller's own object when it's the one declared for
/// that column, or else one of the two a sample can name.
fn format_zval(format: NumFormat, wanted: Option<&Zval>) -> PhpResult<Zval> {
    if let Some(wanted) = wanted
        && num_format_of(wanted, "A declared format")? == format
    {
        return Ok(wanted.shallow_clone());
    }
    if format == COMMA_GROUPS {
        invariant_format()
    } else if format == COMMA_DECIMAL {
        comma_decimal_format()
    } else {
        Err(contract_violation())
    }
}

/// The format `options` declares for source column `ordinal`, if it declares one.
fn wanted_format(options: Option<&Zval>, ordinal: usize) -> PhpResult<Option<Zval>> {
    let Some(options) = options else {
        return Ok(None);
    };
    let scope = entry(Plain::SuggestOptions);
    let columns = read(scope, options, "columnFormats")?;
    if let Some(format) = columns
        .array()
        .and_then(|table| table.get_index(ordinal as i64))
    {
        return Ok(Some(format.shallow_clone()));
    }
    let format = read(scope, options, "format")?;
    Ok((!format.is_null()).then_some(format))
}

/// PHP's `Suggestion` for what the crate found.
pub(crate) fn suggestion_of(found: &crate::Suggestion, options: Option<&Zval>) -> PhpResult<Zval> {
    let mut plan = Vec::with_capacity(found.plan.len());
    let mut report = Vec::with_capacity(found.report.len());
    for (column_found, planned) in found.report.iter().zip(&found.plan) {
        let wanted = wanted_format(options, column_found.ordinal)?;
        let door =
            |door: Kernel| Door::from_code(i64::from(door.code())).ok_or_else(contract_violation);
        plan.push(column(
            column_found.ordinal as i64,
            door(planned.door)?,
            Some(format_zval(planned.format, wanted.as_ref())?),
            door_param(planned.door),
        )?);
        let mut candidates = Vec::with_capacity(column_found.candidates.len());
        for candidate in &column_found.candidates {
            candidates.push(match candidate {
                Candidate::DateOrder(order) => hypercast_enum("DateOrder", *order as u32)?,
                Candidate::NumFormat(format) => format_zval(*format, wanted.as_ref())?,
            });
        }
        let status = match column_found.status {
            crate::SuggestStatus::Resolved => SuggestStatus::Resolved,
            crate::SuggestStatus::Ambiguous => SuggestStatus::Ambiguous,
            crate::SuggestStatus::Unresolved => SuggestStatus::Unresolved,
        };
        let format = match column_found.format {
            Some(format) => format_zval(format, wanted.as_ref())?,
            None => Zval::new(),
        };
        report.push(suggested_column(vec![
            int_zval(column_found.ordinal as i64),
            column_found
                .name
                .as_deref()
                .map_or_else(Zval::new, super::classes::bytes_zval),
            door(column_found.door)?
                .into_zval(false)
                .map_err(|_| pending("Door case"))?,
            door_declared(column_found.door)?,
            format,
            list_zval(candidates)?,
            status
                .into_zval(false)
                .map_err(|_| pending("SuggestStatus case"))?,
            int_zval(column_found.matched as i64),
            int_zval(column_found.sampled as i64),
        ])?);
    }
    suggestion(vec![
        list_zval(plan)?,
        list_zval(report)?,
        bool_zval(found.truncated),
    ])
}
