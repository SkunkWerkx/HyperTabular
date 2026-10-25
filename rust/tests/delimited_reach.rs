//! What a plan's reach costs: the cell table holds a batch of rows for an ordinary plan,
//! and one row's worth, not a batch of them, for a plan reaching far past anything a
//! record holds.

use crate::column::CELL_BUDGET;
use hypertabular::{Column, DelimitedReader, Dialect, Reason};

#[test]
fn a_column_far_past_a_headerless_source_costs_a_row_not_a_batch() {
    let ordinal = (1 << 20) - 1;
    let plan = [Column::i64(0), Column::i64(ordinal)];
    let dialect = Dialect::CSV.with_header(false);
    let mut reader = DelimitedReader::from_slice(b"a,b\n1,2\n", dialect, &plan).unwrap();
    // One row of `ordinal + 2` entries: a batch of them would be 34 GB.
    assert_eq!(reader.cell_entries(), ordinal + 2);
    let mut empty = 0;
    let mut values = Vec::new();
    while let Some(batch) = reader.read().unwrap() {
        for row in batch {
            values.push(row.get::<i64>(0).ok());
            assert_eq!(row.get::<i64>(1).unwrap_err().reason, Reason::Empty);
            empty += 1;
        }
    }
    assert_eq!((values, empty), (vec![None, Some(1)], 2));
    assert_eq!(reader.cell_entries(), ordinal + 2);
}

#[test]
fn an_ordinary_plan_keeps_full_batches() {
    // The widest plan the budget holds a default batch of: 256 source columns.
    let mut text = Vec::new();
    for row in 0..5000 {
        let cells: Vec<String> = (0..256).map(|column| (row + column).to_string()).collect();
        text.extend_from_slice(cells.join(",").as_bytes());
        text.push(b'\n');
    }
    let plan = [Column::i64(0), Column::i64(255)];
    let dialect = Dialect::CSV.with_header(false);
    let mut reader = DelimitedReader::from_slice(&text, dialect, &plan).unwrap();
    assert_eq!(reader.cell_entries(), 4096 * 257);
    assert!(reader.cell_entries() <= CELL_BUDGET);
    let sizes: Vec<usize> =
        std::iter::from_fn(|| reader.read().unwrap().map(|b| b.rows())).collect();
    assert_eq!(sizes, [4096, 904]);

    // One column further, and the batch is what the budget holds.
    let plan = [Column::i64(0), Column::i64(256)];
    let reader = DelimitedReader::from_slice(&text, dialect, &plan).unwrap();
    assert_eq!(reader.cell_entries(), CELL_BUDGET / 258 * 258);
}
