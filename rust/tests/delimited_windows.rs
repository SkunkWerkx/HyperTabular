//! A slice is handed to the core in windows when it is larger than one call takes; reading
//! it in small ones proves the seams lose nothing.

use hypertabular::{Column, DelimitedReader, Dialect, Error, FailureKind};

fn rows(count: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for i in 0..count {
        data.extend_from_slice(
            format!("{i},\"name {i}, with \"\"quotes\"\"\",{i}.5\r\n").as_bytes(),
        );
    }
    data
}

fn collect(reader: &mut DelimitedReader<'_>) -> Result<Vec<(i32, String, f64)>, Error> {
    let mut out = Vec::new();
    while let Some(batch) = reader.read()? {
        for row in batch {
            let text = row.text_str(1).unwrap().into_owned();
            out.push((row.get(0).unwrap(), text, row.get(2).unwrap()));
        }
    }
    Ok(out)
}

const PLAN: [Column; 3] = [Column::i32(0), Column::text(1), Column::f64(2)];

#[test]
fn a_slice_read_in_windows_loses_nothing() {
    let data = rows(500);
    let dialect = Dialect::CSV.with_header(false);
    let options = DelimitedReader::options().batch_rows(64);
    let expected = collect(&mut options.from_slice(&data, dialect, &PLAN).unwrap()).unwrap();
    assert_eq!(expected.len(), 500);
    for window in [64, 100, 4096, data.len() - 14, data.len()] {
        let mut reader = options.from_slice(&data, dialect, &PLAN).unwrap();
        reader.limit_window(window);
        assert_eq!(collect(&mut reader).unwrap(), expected, "window {window}");
    }
}

#[test]
fn a_record_longer_than_a_window_is_too_long_and_not_a_hang() {
    let data = rows(10);
    let mut reader =
        DelimitedReader::from_slice(&data, Dialect::CSV.with_header(false), &PLAN).unwrap();
    reader.limit_window(20);
    let Err(Error::Structure(failure)) = collect(&mut reader) else {
        panic!("a record longer than the window must fail");
    };
    assert_eq!(failure.kind, FailureKind::RowTooLong);
    assert_eq!(failure.record, 0);
}
