//! A column's constraints: HyperCast's declaration, judged once a fill call, bound to the
//! column's door.

use hypercast::detailed::{self, Rejection};
use hypercast::{Checked, Constraints, Decimal, RawConstraints, Text, Violation};

use crate::kernel::abi::{ColumnSpecWith, Span};
use crate::kernel::door::Door;

/// A column's declaration, judged and bound to its door: one HyperCast [`Constraints`] for each
/// door that takes one, the numeric doors and text. Borrows the caller's packed `enum`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Checks<'a> {
    I8(Constraints<'a, i8>),
    I16(Constraints<'a, i16>),
    I32(Constraints<'a, i32>),
    I64(Constraints<'a, i64>),
    U8(Constraints<'a, u8>),
    U16(Constraints<'a, u16>),
    U32(Constraints<'a, u32>),
    U64(Constraints<'a, u64>),
    F32(Constraints<'a, f32>),
    F64(Constraints<'a, f64>),
    Decimal(Constraints<'a, Decimal>),
    Text(Constraints<'a, Text>),
}

/// The column's declaration, judged at `door`: `Ok(None)` when it declares none, and `Err(())`,
/// the caller's [`ERR_CONTRACT`](crate::kernel::abi::ERR_CONTRACT), for one HyperCast's
/// `Constraints::from_raw` refuses or at a door that takes none.
///
/// # Safety
/// `spec.constraints` is null or points to a readable [`RawConstraints`], which need not be
/// aligned; `spec.enum_ptr` is null or points to `spec.enum_len` live bytes.
pub(crate) unsafe fn resolved_constraints(
    spec: &ColumnSpecWith,
    door: Door,
) -> Result<Option<Checks<'_>>, ()> {
    if spec.constraints.is_null() {
        return Ok(None);
    }
    // SAFETY: per the function contract.
    let raw = unsafe { spec.constraints.read_unaligned() };
    let entries: &[u8] = match (spec.enum_ptr.is_null(), spec.enum_len) {
        (_, 0) => &[],
        (true, _) => return Err(()),
        // SAFETY: per the function contract.
        (false, len) => unsafe { core::slice::from_raw_parts(spec.enum_ptr, len) },
    };
    macro_rules! bound {
        ($variant:ident, $ty:ty) => {
            Checks::$variant(Constraints::<$ty>::from_raw(raw, entries).map_err(|_| ())?)
        };
    }
    Ok(Some(match door {
        Door::I8 => bound!(I8, i8),
        Door::I16 => bound!(I16, i16),
        Door::I32 => bound!(I32, i32),
        Door::I64 => bound!(I64, i64),
        Door::U8 => bound!(U8, u8),
        Door::U16 => bound!(U16, u16),
        Door::U32 => bound!(U32, u32),
        Door::U64 => bound!(U64, u64),
        Door::F32 => bound!(F32, f32),
        Door::F64 => bound!(F64, f64),
        Door::Decimal => bound!(Decimal, Decimal),
        Door::Text => bound!(Text, Text),
        _ => return Err(()),
    }))
}

/// The checked text door over a cell's `text`, whose own bytes `span` says where they are: a
/// value, or a violation's, is the span of the text the door judged, moved onto the cell's, in
/// the input or in the arena, its flag kept.
#[inline(always)]
pub(crate) fn text_checked(
    text: &[u8],
    span: Span,
    constraints: &Constraints<'_, Text>,
) -> Result<Checked<Span>, Rejection<Span>> {
    let at = |offset: u32, len: u32| Span {
        offset: span.offset.wrapping_add(offset),
        len: len | (span.len & Span::FLAG),
    };
    match detailed::cast_text_checked(text, constraints) {
        Ok(passed) => {
            let trims = constraints.raw().flags & RawConstraints::TRIM != 0;
            let (offset, len) = judged_span(text, &passed, trims);
            Ok(Checked::new(
                at(offset, len),
                passed.adjusted,
                passed.enum_index,
            ))
        }
        Err(Rejection::Violation(broken)) => Err(Rejection::Violation(Violation::new(
            broken.keyword,
            at(broken.offset, broken.len),
            broken.offset,
            broken.len,
        ))),
        Err(Rejection::Failure(failure)) => Err(Rejection::Failure(failure)),
    }
}

/// Where the checked text door's judged text sits in `text`, which it passed: what the value it
/// returned spans, or, for an `enum` match, whose value is the declared entry, the text after the
/// `trim` the column declares.
#[inline(always)]
fn judged_span(text: &[u8], passed: &Checked<&str>, trims: bool) -> (u32, u32) {
    if passed.enum_index.is_none() {
        let offset = (passed.value.as_ptr() as usize).wrapping_sub(text.as_ptr() as usize);
        return (offset as u32, passed.value.len() as u32);
    }
    let Ok(whole) = core::str::from_utf8(text) else {
        return (0, text.len() as u32);
    };
    if !trims {
        return (0, whole.len() as u32);
    }
    let rest = whole.trim_start_matches(hypercast::is_white_space);
    let kept = rest.trim_end_matches(hypercast::is_white_space);
    ((whole.len() - rest.len()) as u32, kept.len() as u32)
}
