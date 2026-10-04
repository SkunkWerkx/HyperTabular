//! The contract every Hyper* tabular provider speaks. A provider (HyperDelimited,
//! HyperWorkbook) turns its format into a forward-only stream of rows whose cells are the
//! format-neutral [`Cell`]; this crate turns those cells into HyperCast verdicts under a
//! caller-declared [`Plan`] of doors, and packs the results column-major into a [`Batch`]
//! that crosses the FFI boundary once per chunk instead of once per cell.
//!
//! - [`Cell`] — bytes from delimited text, or the typed value a workbook stores
//! - [`Door`] / [`Column`] / [`Plan`] — which HyperCast door each output column casts through
//! - [`cast_i32`]-style doors — one function per door, `Result<T, Fault>` per cell, the
//!   same verdict vocabulary as HyperCast (`Empty` / `Malformed` / `OutOfRange`)
//! - [`Batch`] / [`fill_batch`] — typed value vectors plus parallel [`CellVerdict`] arrays
//! - [`TabularSource`] / [`Row`] / [`Header`] — the provider trait
//! - [`serial`] — Excel's two date systems, with their leap-year lie
//! - [`ffi`] — the `#[repr(C)]` shapes every provider export and every binding share
//!
//! Nothing here sniffs, infers, or guesses: the caller declares the doors, the number
//! formats, the header, and the dialect. The design record is `docs/design.md`.

#![cfg_attr(not(feature = "std"), no_std)]

// The panic handler for the no_std libraries this crate links itself (`cargo staticlib`,
// `cargo cdylib`), built with `panic = "abort"`. No export can reach it — that is what the
// no-panic proof says — but a no_std artifact has to name one. HyperCast's lib.rs has the
// whole account of this and of the two blocks after it; the mechanics are identical.
#[cfg(all(feature = "staticlib", not(feature = "std")))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch = "wasm32"))]
    {
        unsafe extern "C" {
            safe fn abort() -> !;
        }
        abort()
    }
}

// A no_std shared library is its own final link, so it has to define the personality
// routine the precompiled `core` names — hidden, so that it is not one more export.
#[cfg(all(feature = "cdylib", not(feature = "std"), target_vendor = "apple"))]
core::arch::global_asm!(
    ".globl _rust_eh_personality",
    ".private_extern _rust_eh_personality",
    "_rust_eh_personality:",
    "ret",
);
#[cfg(all(
    feature = "cdylib",
    not(feature = "std"),
    not(target_vendor = "apple"),
    not(target_os = "windows"),
    not(target_arch = "wasm32"),
))]
core::arch::global_asm!(
    ".globl rust_eh_personality",
    ".hidden rust_eh_personality",
    ".type rust_eh_personality, %function",
    "rust_eh_personality:",
    "ret",
);

// And it has to name the C runtime std would have named for it.
#[cfg(all(feature = "cdylib", not(feature = "std"), unix))]
#[link(name = "c")]
unsafe extern "C" {}
#[cfg(all(feature = "cdylib", not(feature = "std"), target_env = "msvc"))]
#[cfg_attr(target_feature = "crt-static", link(name = "libcmt"))]
#[cfg_attr(not(target_feature = "crt-static"), link(name = "msvcrt"))]
unsafe extern "C" {}

pub mod kernel;

#[cfg(feature = "std")]
mod batch;
#[cfg(feature = "std")]
mod cast;
#[cfg(feature = "std")]
mod cell;
#[cfg(feature = "std")]
pub mod delimited;
#[cfg(feature = "std")]
pub mod ffi;
#[cfg(feature = "php")]
mod php_ext;
#[cfg(feature = "std")]
mod plan;
#[cfg(feature = "std")]
mod render;
#[cfg(feature = "std")]
pub mod serial;
#[cfg(feature = "std")]
mod source;
#[cfg(feature = "std")]
pub mod workbook;

// The Python binding over the core. A binding, not part of the core: it allocates, it
// uses std, and it is compiled only into the extension module.
#[cfg(feature = "python")]
mod python_ext;

#[cfg(feature = "std")]
pub use hypercast::{
    Date, Duration, ExcelEpoch, Fault, NumFormat, Reason, Timestamp, UnixPrecision,
};

pub use kernel::abi::{CellVerdict, Span};
pub use kernel::door::Door;

#[cfg(feature = "std")]
pub use batch::{Batch, ColumnData, FaultRaw, Values, fill_batch};
#[cfg(feature = "std")]
pub use cast::{
    Value, cast_bool, cast_date, cast_date_ordered, cast_datetime, cast_decimal, cast_duration,
    cast_excel_serial, cast_f32, cast_f64, cast_i8, cast_i16, cast_i32, cast_i64, cast_text,
    cast_time, cast_timestamp, cast_u8, cast_u16, cast_u32, cast_u64, cast_unix, cast_uuid,
};
#[cfg(feature = "std")]
pub use cell::{Cell, CellError};
#[cfg(feature = "std")]
pub use plan::{Column, Plan};
#[cfg(feature = "std")]
pub use render::render;
#[cfg(feature = "std")]
pub use source::{Header, Row, TabularSource};

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::convert::Infallible;

    const INVARIANT: NumFormat = NumFormat::INVARIANT;

    fn reason<T: core::fmt::Debug>(verdict: Result<T, Fault>) -> Reason {
        verdict.unwrap_err().reason
    }

    // --- the cast matrix, row by row ---

    #[test]
    fn empty_is_empty_for_every_door() {
        let cell = Cell::Empty;
        assert_eq!(reason(cast_bool(&cell)), Reason::Empty);
        assert_eq!(reason(cast_i32(&cell, &INVARIANT)), Reason::Empty);
        assert_eq!(reason(cast_f64(&cell, &INVARIANT)), Reason::Empty);
        assert_eq!(reason(cast_uuid(&cell)), Reason::Empty);
        assert_eq!(
            reason(cast_timestamp(&cell, ExcelEpoch::Y1900)),
            Reason::Empty
        );
        assert_eq!(
            reason(cast_unix(&cell, UnixPrecision::Seconds)),
            Reason::Empty
        );
        assert_eq!(reason(cast_date(&cell, ExcelEpoch::Y1900)), Reason::Empty);
        assert_eq!(reason(cast_time(&cell)), Reason::Empty);
        assert_eq!(reason(cast_duration(&cell)), Reason::Empty);
        assert_eq!(reason(cast_text(&cell)), Reason::Empty);
    }

    #[test]
    fn text_goes_through_the_hypercast_door_verbatim() {
        assert_eq!(cast_i32(&Cell::Text(b"(1,234)"), &INVARIANT), Ok(-1234));
        assert_eq!(cast_bool(&Cell::Text(b" enabled ")), Ok(true));
        assert_eq!(cast_f64(&Cell::Text(b"50%"), &INVARIANT), Ok(0.5));
        assert_eq!(
            cast_timestamp(&Cell::Text(b"2022-02-22T19:22:22Z"), ExcelEpoch::Y1900),
            Ok(Timestamp {
                seconds: 1_645_557_742,
                nanos: 0
            })
        );
        // The fault span is HyperCast's own, indexed into the cell's bytes.
        let fault = cast_i32(&Cell::Text(b"  12x4"), &INVARIANT).unwrap_err();
        assert_eq!(
            (fault.reason, fault.offset, fault.len),
            (Reason::Malformed, 4, 1)
        );
        // The text door borrows; an empty text cell is Empty.
        assert_eq!(cast_text(&Cell::Text(b"abc")).unwrap().as_ref(), b"abc");
        assert_eq!(reason(cast_text(&Cell::Text(b""))), Reason::Empty);
        assert_eq!(cast_text(&Cell::Text(b"  ")).unwrap().as_ref(), b"  ");
    }

    #[test]
    fn number_into_integers_demands_integrality_and_range() {
        assert_eq!(cast_i32(&Cell::Number(42.0), &INVARIANT), Ok(42));
        assert_eq!(cast_i32(&Cell::Number(-0.0), &INVARIANT), Ok(0));
        assert_eq!(cast_u8(&Cell::Number(255.0), &INVARIANT), Ok(255));
        assert_eq!(
            reason(cast_u8(&Cell::Number(256.0), &INVARIANT)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_u8(&Cell::Number(-1.0), &INVARIANT)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_i32(&Cell::Number(1.5), &INVARIANT)),
            Reason::Malformed
        );
        assert_eq!(
            reason(cast_i64(&Cell::Number(1e300), &INVARIANT)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_i64(&Cell::Number(f64::NAN), &INVARIANT)),
            Reason::Malformed
        );
        assert_eq!(
            cast_u64(&Cell::Number(9_007_199_254_740_992.0), &INVARIANT),
            Ok(1 << 53)
        );
        assert_eq!(
            cast_i64(&Cell::Number(-9_223_372_036_854_775_808.0), &INVARIANT),
            Ok(i64::MIN)
        );
        assert_eq!(
            reason(cast_i64(
                &Cell::Number(9_223_372_036_854_775_808.0),
                &INVARIANT
            )),
            Reason::OutOfRange
        );
    }

    #[test]
    fn number_into_reals_checks_only_the_narrower_range() {
        assert_eq!(cast_f64(&Cell::Number(1.5), &INVARIANT), Ok(1.5));
        assert_eq!(cast_f32(&Cell::Number(1.5), &INVARIANT), Ok(1.5f32));
        assert_eq!(
            reason(cast_f32(&Cell::Number(1e39), &INVARIANT)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_f64(&Cell::Number(f64::INFINITY), &INVARIANT)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_f64(&Cell::Number(f64::NAN), &INVARIANT)),
            Reason::Malformed
        );
    }

    #[test]
    fn number_into_bool_is_zero_or_one_only() {
        assert_eq!(cast_bool(&Cell::Number(1.0)), Ok(true));
        assert_eq!(cast_bool(&Cell::Number(0.0)), Ok(false));
        assert_eq!(reason(cast_bool(&Cell::Number(2.0))), Reason::Malformed);
        assert_eq!(reason(cast_bool(&Cell::Number(0.5))), Reason::Malformed);
    }

    #[test]
    fn number_into_temporal_doors_reads_an_excel_serial() {
        // 45000 is 2023-03-15 in the 1900 system.
        let date = Date {
            year: 2023,
            month: 3,
            day: 15,
        };
        assert_eq!(
            cast_date(&Cell::Number(45_000.0), ExcelEpoch::Y1900),
            Ok(date)
        );
        assert_eq!(
            cast_timestamp(&Cell::Number(45_000.5), ExcelEpoch::Y1900),
            Ok(Timestamp {
                seconds: 1_678_881_600,
                nanos: 0
            })
        );
        // The same serial is 1462 days later in the 1904 system.
        assert_eq!(
            cast_date(&Cell::Number(45_000.0), ExcelEpoch::Y1904),
            Ok(Date {
                year: 2027,
                month: 3,
                day: 16
            })
        );
        // Time is the fraction; in the 1900 system a serial under 1 has no date.
        assert_eq!(cast_time(&Cell::Number(0.5)), Ok(43_200_000_000_000));
        assert_eq!(cast_time(&Cell::Number(45_000.25)), Ok(21_600_000_000_000));
        assert_eq!(
            reason(cast_date(&Cell::Number(0.5), ExcelEpoch::Y1900)),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_timestamp(&Cell::Number(0.5), ExcelEpoch::Y1900)),
            Reason::OutOfRange
        );
        // The phantom 1900-02-29 is out of range, as it is for HyperCast's text door.
        assert_eq!(
            reason(cast_date(&Cell::Number(60.0), ExcelEpoch::Y1900)),
            Reason::OutOfRange
        );
        // Duration is days.
        assert_eq!(
            cast_duration(&Cell::Number(1.5)),
            Ok(Duration {
                seconds: 129_600,
                nanos: 0
            })
        );
        assert_eq!(
            cast_duration(&Cell::Number(-0.5)),
            Ok(Duration {
                seconds: -43_200,
                nanos: 0
            })
        );
        // Unix is the integer at the declared precision.
        assert_eq!(
            cast_unix(&Cell::Number(1_700_000_000_123.0), UnixPrecision::Millis),
            Ok(Timestamp {
                seconds: 1_700_000_000,
                nanos: 123_000_000
            })
        );
        assert_eq!(
            cast_unix(&Cell::Number(-1.0), UnixPrecision::Millis),
            Ok(Timestamp {
                seconds: -1,
                nanos: 999_000_000
            })
        );
        assert_eq!(
            reason(cast_unix(&Cell::Number(1.5), UnixPrecision::Seconds)),
            Reason::Malformed
        );
        assert_eq!(
            reason(cast_unix(
                &Cell::Number(253_402_300_800.0),
                UnixPrecision::Seconds
            )),
            Reason::OutOfRange
        );
    }

    #[test]
    fn wall_clock_span_and_bool_cells_land_where_the_matrix_says() {
        let wall = Cell::Wall {
            date: Date {
                year: 2026,
                month: 1,
                day: 2,
            },
            nanos: 54_245_123_000_000,
        };
        assert_eq!(
            cast_timestamp(&wall, ExcelEpoch::Y1900),
            Ok(Timestamp {
                seconds: 1_767_366_245,
                nanos: 123_000_000
            })
        );
        assert_eq!(
            cast_unix(&wall, UnixPrecision::Nanos).unwrap().seconds,
            1_767_366_245
        );
        assert_eq!(
            cast_date(&wall, ExcelEpoch::Y1900),
            Ok(Date {
                year: 2026,
                month: 1,
                day: 2
            })
        );
        assert_eq!(cast_time(&wall), Ok(54_245_123_000_000));
        assert_eq!(reason(cast_i32(&wall, &INVARIANT)), Reason::Malformed);
        assert_eq!(reason(cast_duration(&wall)), Reason::Malformed);
        assert_eq!(
            cast_text(&wall).unwrap().as_ref(),
            b"2026-01-02T15:04:05.123"
        );

        let clock = Cell::Clock(54_245_000_000_000);
        assert_eq!(cast_time(&clock), Ok(54_245_000_000_000));
        assert_eq!(
            cast_duration(&clock),
            Ok(Duration {
                seconds: 54_245,
                nanos: 0
            })
        );
        assert_eq!(
            reason(cast_date(&clock, ExcelEpoch::Y1900)),
            Reason::Malformed
        );
        assert_eq!(cast_text(&clock).unwrap().as_ref(), b"15:04:05");

        let span = Cell::Span(Duration {
            seconds: 5_400,
            nanos: 500_000_000,
        });
        assert_eq!(
            cast_duration(&span),
            Ok(Duration {
                seconds: 5_400,
                nanos: 500_000_000
            })
        );
        assert_eq!(cast_time(&span), Ok(5_400_500_000_000));
        assert_eq!(
            reason(cast_time(&Cell::Span(Duration {
                seconds: 86_400,
                nanos: 0
            }))),
            Reason::OutOfRange
        );
        assert_eq!(
            reason(cast_time(&Cell::Span(Duration {
                seconds: -1,
                nanos: 0
            }))),
            Reason::OutOfRange
        );
        assert_eq!(cast_text(&span).unwrap().as_ref(), b"PT1H30M0.5S");
        assert_eq!(
            cast_text(&Cell::Span(Duration {
                seconds: -108_000,
                nanos: 0
            }))
            .unwrap()
            .as_ref(),
            b"-P1DT6H"
        );
        assert_eq!(
            cast_text(&Cell::Span(Duration {
                seconds: 0,
                nanos: 0
            }))
            .unwrap()
            .as_ref(),
            b"PT0S"
        );

        assert_eq!(cast_bool(&Cell::Bool(true)), Ok(true));
        assert_eq!(
            reason(cast_i32(&Cell::Bool(true), &INVARIANT)),
            Reason::Malformed
        );
        assert_eq!(cast_text(&Cell::Bool(false)).unwrap().as_ref(), b"false");

        let error = Cell::Error(CellError::NotAvailable);
        assert_eq!(reason(cast_f64(&error, &INVARIANT)), Reason::Malformed);
        assert_eq!(cast_text(&error).unwrap().as_ref(), b"#N/A");
        assert_eq!(cast_text(&Cell::Number(42.0)).unwrap().as_ref(), b"42");
        assert_eq!(cast_text(&Cell::Number(0.1)).unwrap().as_ref(), b"0.1");
        assert_eq!(cast_text(&Cell::Number(1e300)).unwrap().as_ref(), b"1e300");
    }

    #[test]
    fn typed_cell_faults_carry_no_span_until_the_batch_renders_them() {
        let fault = cast_i32(&Cell::Bool(true), &INVARIANT).unwrap_err();
        assert_eq!((fault.offset, fault.len), (0, 0));
    }

    #[test]
    fn column_cast_dispatches_dynamically() {
        let column = Column::new(0, Door::I16);
        assert_eq!(
            column.cast(&Cell::Text(b"7"), ExcelEpoch::Y1900),
            Ok(Value::I16(7))
        );
        let column = Column::new(0, Door::Unix(UnixPrecision::Seconds));
        assert_eq!(
            column.cast(&Cell::Text(b"0"), ExcelEpoch::Y1900),
            Ok(Value::Timestamp(Timestamp {
                seconds: 0,
                nanos: 0
            }))
        );
        let column = Column::new(0, Door::Text);
        assert!(
            matches!(column.cast(&Cell::Number(1.0), ExcelEpoch::Y1900), Ok(Value::Text(text)) if text.as_ref() == b"1")
        );
    }

    // --- the batch ---

    struct Rows<'c> {
        rows: Vec<Vec<Cell<'c>>>,
        next: usize,
        header: Option<Header>,
    }

    impl<'c> TabularSource for Rows<'c> {
        type Row<'a>
            = &'a [Cell<'c>]
        where
            Self: 'a;
        type Error = Infallible;

        fn header(&self) -> Option<&Header> {
            self.header.as_ref()
        }

        fn next_row(&mut self) -> Result<Option<Self::Row<'_>>, Infallible> {
            let row = self.rows.get(self.next).map(Vec::as_slice);
            self.next += 1;
            Ok(row)
        }
    }

    #[test]
    fn batch_is_column_major_with_parallel_verdicts_and_a_fault_table() {
        let mut source = Rows {
            rows: vec![
                vec![Cell::Text(b"1"), Cell::Text(b"hello"), Cell::Number(2.5)],
                vec![Cell::Text(b"x9"), Cell::Text(b""), Cell::Bool(true)],
                vec![Cell::Text(b"3")],
            ],
            next: 0,
            header: Some(Header::from_names([&b"id"[..], b"name", b"score"])),
        };
        let plan = Plan::new(vec![
            Column::new(0, Door::I32),
            Column::new(1, Door::Text),
            Column::new(2, Door::F64),
        ]);
        let mut batch = Batch::new();
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 10), Ok(3));
        assert_eq!(batch.rows(), 3);

        assert_eq!(batch.column(0).i32s().unwrap(), &[1, 0, 3]);
        let verdicts = batch.column(0).verdicts();
        assert!(verdicts[0].is_ok());
        // HyperCast's integer door spans the offending byte, not the token.
        assert_eq!(
            verdicts[1],
            CellVerdict {
                offset: 0,
                len: 1,
                reason: Reason::Malformed as u32
            }
        );
        assert!(verdicts[2].is_ok());

        assert_eq!(batch.text(1, 0), Some(&b"hello"[..]));
        assert_eq!(batch.column(1).verdicts()[1].reason, Reason::Empty as u32);
        assert_eq!(batch.text(1, 1), None);
        assert_eq!(batch.column(1).verdicts()[2].reason, Reason::Empty as u32);

        assert_eq!(batch.column(2).f64s().unwrap(), &[2.5, 0.0, 0.0]);
        let bool_fault = batch.column(2).verdicts()[1];
        assert_eq!(bool_fault.reason, Reason::Malformed as u32);
        assert_eq!((bool_fault.offset, bool_fault.len), (0, 4));

        // Only Malformed/OutOfRange cells land in the fault table, with their raw text.
        let faults = batch.faults();
        assert_eq!(faults.len(), 2);
        assert_eq!((faults[0].row, faults[0].column), (1, 0));
        assert_eq!(batch.raw(&faults[0]), b"x9");
        assert_eq!((faults[1].row, faults[1].column), (1, 2));
        assert_eq!(batch.raw(&faults[1]), b"true");

        // A second fill on an exhausted source reuses the batch and reports zero rows.
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 10), Ok(0));
        assert_eq!(batch.rows(), 0);
        assert_eq!(source.header().unwrap().ordinal(b"score"), Some(2));
    }

    #[test]
    fn batch_honours_max_rows_and_the_date_system() {
        let mut source = Rows {
            rows: vec![vec![Cell::Number(45_000.0)]; 5],
            next: 0,
            header: None,
        };
        let plan = Plan::new(vec![Column::new(0, Door::Date)]);
        let mut batch = Batch::new();
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 2), Ok(2));
        assert_eq!(
            batch.column(0).dates().unwrap()[1],
            Date {
                year: 2023,
                month: 3,
                day: 15
            }
        );
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 2), Ok(2));
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 2), Ok(1));
        assert_eq!(fill_batch(&mut source, &plan, &mut batch, 2), Ok(0));
    }
}
