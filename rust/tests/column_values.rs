//! Every typed door lends its whole column through the same checked generic accessor.

use hypertabular::{
    CivilDateTime, Column, Date, DateOrder, Decimal, DelimitedReader, Dialect, Duration,
    ExcelEpoch, Reason, Timestamp, UnixPrecision, Value,
};

fn check<T: Value + PartialEq + std::fmt::Debug>(column: Column, text: &str, expected: T) {
    let input = format!("value\n{text}\nnot-a-value\n\"\"\n");
    let mut reader =
        DelimitedReader::from_slice(input.as_bytes(), Dialect::CSV, &[column]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let values: &[T] = batch.values(0);
    assert_eq!(values.len(), 3);
    assert_eq!(values[0], expected);
    assert_eq!(batch.get::<T>(0, 0), Ok(expected));
    assert!(batch.verdicts(0)[0].is_ok());
    assert_eq!(
        batch.verdicts(0)[1].fault().unwrap().reason,
        Reason::Malformed
    );
    assert_eq!(batch.verdicts(0)[2].fault().unwrap().reason, Reason::Empty);
}

#[test]
fn every_value_type_has_a_borrowed_column_with_verdicts() {
    check(Column::bool(0), "true", true);
    check(Column::i8(0), "-12", -12i8);
    check(Column::i16(0), "-1234", -1234i16);
    check(Column::i32(0), "-123456", -123456i32);
    check(Column::i64(0), "-12345678901", -12345678901i64);
    check(Column::u8(0), "12", 12u8);
    check(Column::u16(0), "1234", 1234u16);
    check(Column::u32(0), "123456", 123456u32);
    check(Column::u64(0), "12345678901", 12345678901u64);
    check(Column::f32(0), "1.25", 1.25f32);
    check(Column::f64(0), "1.25", 1.25f64);
    check(
        Column::decimal(0),
        "12.5",
        Decimal {
            lo: 125,
            hi: 0,
            scale: 1,
            negative: false,
        },
    );
    check(
        Column::uuid(0),
        "01020304-0506-0708-090a-0b0c0d0e0f10",
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    );
    let instant = Timestamp {
        seconds: 1_704_067_200,
        nanos: 0,
    };
    check(Column::timestamp(0), "2024-01-01T00:00:00Z", instant);
    check(
        Column::unix(0, UnixPrecision::Seconds),
        "1704067200",
        instant,
    );
    check(Column::excel_serial(0, ExcelEpoch::Y1900), "45292", instant);
    let date = Date {
        year: 2024,
        month: 1,
        day: 1,
    };
    check(Column::date(0), "2024-01-01", date);
    check(
        Column::date_ordered(0, DateOrder::DayMonthYear),
        "01/01/2024",
        date,
    );
    check(
        Column::datetime(0, DateOrder::YearMonthDay),
        "2024-01-01 12:30:00",
        CivilDateTime {
            date,
            nanos_of_day: 45_000_000_000_000,
        },
    );
    check(Column::time(0), "00:00:01", 1_000_000_000u64);
    check(
        Column::duration(0),
        "PT1S",
        Duration {
            seconds: 1,
            nanos: 0,
        },
    );
}

#[test]
#[should_panic(expected = "which does not make a f64")]
fn a_generic_column_checks_its_type() {
    let mut reader =
        DelimitedReader::from_slice(b"n\n1\n", Dialect::CSV, &[Column::i32(0)]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let _ = batch.values::<f64>(0);
}

#[test]
#[should_panic(expected = "column 1")]
fn a_generic_column_checks_its_index() {
    let mut reader =
        DelimitedReader::from_slice(b"n\n1\n", Dialect::CSV, &[Column::i32(0)]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    let _ = batch.values::<i32>(1);
}
