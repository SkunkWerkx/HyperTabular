//! A batch kept by its caller, its faults list worked out from the copies: the same list
//! `Batch::faults` gives, for every kind of entry, after the reader has moved on.

use hypertabular::{
    CellNote, CellVerdict, Column, DelimitedOptions, Dialect, KeptBatch, KeptColumn, KeptFaults,
    LongRows, Problem, Range, SheetOptions, ShortRows, Workbook,
};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook");

/// What a binding keeps of a batch, owned.
struct Copy {
    rows: usize,
    lines: Vec<u32>,
    verdicts: Vec<Vec<CellVerdict>>,
    notes: Vec<Vec<CellNote>>,
    sources: Vec<usize>,
    widths: Option<(u32, Vec<u32>)>,
    /// What the batch itself listed, and said of each cell, while it was there.
    faults: Vec<Problem>,
    cells: Vec<Vec<Cell>>,
}

/// What a batch said of one cell: its verdict, detail and origin.
type Cell = (
    Result<(), hypertabular::Fault>,
    Option<hypertabular::Detail>,
    hypertabular::Origin,
);

fn keep(batch: &hypertabular::Batch<'_>) -> Copy {
    let columns = batch.columns().len();
    Copy {
        rows: batch.rows(),
        lines: (0..batch.rows()).map(|row| batch.line(row)).collect(),
        verdicts: (0..columns).map(|c| batch.verdicts(c).to_vec()).collect(),
        notes: (0..columns).map(|c| batch.notes(c).to_vec()).collect(),
        sources: (0..columns).map(|c| batch.source_column(c)).collect(),
        widths: batch
            .reported_widths()
            .map(|(width, rows)| (width, rows[..batch.rows()].to_vec())),
        faults: batch.faults().collect(),
        cells: (0..columns)
            .map(|c| {
                (0..batch.rows())
                    .map(|row| {
                        let verdict = batch.verdicts(c)[row];
                        let verdict = verdict.fault().map_or(Ok(()), Err);
                        (verdict, batch.detail(c, row), batch.origin(c, row))
                    })
                    .collect()
            })
            .collect(),
    }
}

/// The kept copy says what the batch said: its list, and each cell's verdict, detail and origin.
fn check(copy: &Copy) {
    let columns: Vec<KeptColumn> = (0..copy.verdicts.len())
        .map(|c| KeptColumn::new(&copy.verdicts[c], &copy.notes[c], copy.sources[c]))
        .collect();
    let long_rows = copy.widths.as_ref().map(|(w, rows)| (*w, rows.as_slice()));
    let kept = KeptBatch::new(&copy.lines, &columns, long_rows).unwrap();
    assert_eq!(kept.rows(), copy.rows);
    assert_eq!(kept.faults().collect::<Vec<_>>(), copy.faults);
    // Asked twice, the same list.
    assert_eq!(kept.faults().collect::<Vec<_>>(), copy.faults);
    for (c, column) in columns.iter().enumerate() {
        for (row, (verdict, detail, origin)) in copy.cells[c].iter().enumerate() {
            assert_eq!(column.verdict(row), Some(*verdict));
            assert_eq!(column.detail(row), *detail);
            assert_eq!(column.origin(row), *origin);
        }
        assert_eq!(column.verdict(copy.rows), None);
        assert_eq!(
            column.fault_count(),
            copy.cells[c].iter().filter(|cell| cell.0.is_err()).count()
        );
    }
}

fn delimited(
    text: &[u8],
    dialect: Dialect,
    plan: &[Column],
    options: DelimitedOptions,
) -> Vec<Copy> {
    let mut reader = options.from_slice(text, dialect, plan).unwrap();
    let mut copies = Vec::new();
    while let Some(batch) = reader.read().unwrap() {
        copies.push(keep(&batch));
    }
    copies
}

#[test]
fn a_kept_delimited_batch_lists_what_the_batch_listed() {
    let text = b"a,b,c\n1,x,2\n,3,9999999999\n\"q\",NA,1,extra\n4,5\n";
    let plan = [Column::i32(0), Column::i32(1), Column::i32(2)];
    let dialect = Dialect::CSV
        .with_long_rows(LongRows::Report)
        .with_short_rows(ShortRows::Pad)
        .with_tokens(&["NA"]);
    for batch_rows in [1, 2, 4096] {
        let copies = delimited(
            text,
            dialect,
            &plan,
            DelimitedOptions::new().batch_rows(batch_rows),
        );
        let listed: usize = copies.iter().map(|copy| copy.faults.len()).sum();
        // The malformed `x` and `q`, the out-of-range `9999999999`, and the long row; not the
        // blank, the token or the padding.
        assert_eq!(listed, 4, "batches of {batch_rows}");
        assert!(copies.iter().any(|copy| copy.widths.is_some()));
        copies.iter().for_each(check);
    }
    // Notes off: the same reasons, no details, every origin a cell.
    let copies = delimited(text, dialect, &plan, DelimitedOptions::new().notes(false));
    assert!(
        copies
            .iter()
            .all(|copy| copy.notes.iter().all(Vec::is_empty))
    );
    copies.iter().for_each(check);
}

#[test]
fn a_kept_batch_lists_more_problems_than_it_works_out_at_once() {
    // Far past the few entries a list holds at a time: every one of them, in order.
    let mut text = b"a,b\n".to_vec();
    for row in 0..100 {
        text.extend_from_slice(format!("x{row},{row}\n").as_bytes());
    }
    let plan = [Column::i32(0), Column::i32(1)];
    let copies = delimited(&text, Dialect::CSV, &plan, DelimitedOptions::new());
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].faults.len(), 100);
    copies.iter().for_each(check);
}

#[test]
fn a_kept_sheet_lists_a_formula_with_no_cached_value_and_its_own_columns() {
    let book = Workbook::open(format!("{CORPUS}/uncached.xlsx")).unwrap();
    let plan: Vec<Column> = (0..3).map(Column::i32).collect();
    let options = SheetOptions::new()
        .with_empty_rows_skipped(false)
        .with_batch_rows(2);
    let mut sheet = book.sheet(0, options, &plan).unwrap();
    let mut uncached = 0;
    while let Some(batch) = sheet.read().unwrap() {
        let copy = keep(&batch);
        uncached += copy
            .faults
            .iter()
            .filter(|problem| problem.origin == hypertabular::Origin::Uncached)
            .count();
        check(&copy);
    }
    assert!(uncached > 0);

    // A range's columns are the sheet's own in the list.
    let book = Workbook::open(format!("{CORPUS}/ranges.xlsx")).unwrap();
    let range = Range::parse("B2:D40").unwrap();
    let mut sheet = book
        .sheet(
            0,
            SheetOptions::new().with_range(range),
            &[Column::i32(0), Column::i32(1)],
        )
        .unwrap();
    while let Some(batch) = sheet.read().unwrap() {
        let copy = keep(&batch);
        assert_eq!(copy.sources, [1, 2]);
        check(&copy);
    }
}

#[test]
fn a_kept_batch_refuses_copies_of_the_wrong_length() {
    let verdicts = [CellVerdict::OK; 2];
    let notes = [CellNote::default(); 2];
    let column = [KeptColumn::new(&verdicts, &notes, 0)];
    assert!(KeptBatch::new(&[1, 2], &column, None).is_some());
    assert!(KeptBatch::new(&[1], &column, None).is_none());
    assert!(KeptBatch::new(&[1, 2, 3], &column, None).is_none());
    assert!(KeptBatch::new(&[1, 2], &column, Some((1, &[1]))).is_none());
    let short_notes = [KeptColumn::new(&verdicts, &notes[..1], 0)];
    assert!(KeptBatch::new(&[1, 2], &short_notes, None).is_none());
    // No notes is a read with notes off, and is whole.
    let none = [KeptColumn::new(&verdicts, &[], 0)];
    assert!(KeptBatch::new(&[1, 2], &none, None).is_some());
    // A reason code no read writes is no verdict at all.
    let odd = [CellVerdict {
        offset: 0,
        len: 0,
        reason: 9,
    }];
    assert_eq!(KeptColumn::new(&odd, &[], 0).verdict(0), None);
}

#[test]
fn a_kept_list_can_go_to_another_thread() {
    fn send_and_share<T: Send + Sync>(_: &T) {}
    let verdicts = [CellVerdict::OK];
    let column = [KeptColumn::new(&verdicts, &[], 0)];
    let kept = KeptBatch::new(&[1], &column, None).unwrap();
    let faults: KeptFaults<'_> = kept.faults();
    send_and_share(&faults);
    send_and_share(&kept);
    // wasm32-wasip1 has no threads: the bounds above still prove Send + Sync there.
    #[cfg(not(target_family = "wasm"))]
    assert_eq!(
        std::thread::scope(|scope| scope.spawn(|| faults.count()).join().unwrap()),
        0
    );
}
