//! The workbook targets' oracle: one package read twice — with buffers that start empty and
//! grow only as the core asks, and with room to spare and batches of another size — and
//! the two readings held to each other. The harness (rust/tests/support/workbook.rs, the
//! one the crate's own tests use) panics on anything the contract rules out: a code no call
//! may return, a refusal that asks for less than it has, a failure that names nothing, a
//! window past 256 MiB, a core that keeps asking.
//!
//! Each target's input carries [`EXTRAS`] bytes for what `hypertabular_workbook_sheet_with` and
//! `_fill_with` take: a range, a skip, a header row, and whether the read goes through `fill_with`,
//! tokens, notes and row widths included. All zero is a plain read.

use crate::support::{Harness, Sizing, every_door};
use hypertabular::Range;
use hypertabular::workbook::SheetOptions;

/// Rows taken from one sheet before a reading stops: a few bytes of ODS can repeat a row a
/// million times, which is the core's to read and not the fuzzer's to wait for.
const ROW_LIMIT: usize = 2_000;

/// The bytes [`extras`] reads: whether to go through `fill_with`, the skip, the header row, then
/// the range's first row, first column, last row and last column.
pub const EXTRAS: usize = 7;

/// A count from one byte: small ones as they are, since that's where the edges of a small sheet
/// are, and the top of the byte for the huge ones a repeat can reach.
fn count(byte: u8) -> u32 {
    if byte < 0xF0 {
        u32::from(byte)
    } else {
        u32::MAX >> (byte & 0x0F)
    }
}

/// `options` with what `bytes` says, and the token list to read through `fill_with` with, if it
/// says to.
pub fn extras(options: SheetOptions, bytes: &[u8; EXTRAS]) -> (SheetOptions, Option<Vec<u8>>) {
    let [
        through,
        skip,
        header_row,
        first_row,
        first_column,
        last_row,
        last_column,
    ] = *bytes;
    let mut options = options
        .with_skip(count(skip))
        .with_header_row(count(header_row));
    let ordered = |first: u8, last: u8| {
        let (first, last) = (count(first), count(last));
        if first != 0 && last != 0 {
            (first.min(last), first.max(last))
        } else {
            (first, last)
        }
    };
    let (first_row, last_row) = ordered(first_row, last_row);
    let (first_column, last_column) = ordered(first_column, last_column);
    if first_row != 0 || first_column != 0 || last_row != 0 || last_column != 0 {
        let range = Range::new(first_row, first_column, last_row, last_column)
            .expect("an ordered range is a range");
        options = options.with_range(range);
    }
    // "NA", "-" and the empty text.
    let tokens = b"\x02\0\0\0NA\x01\0\0\0-\0\0\0\0".to_vec();
    (options, (through & 1 != 0).then_some(tokens))
}

pub fn compare(
    container: &[u8],
    options: SheetOptions,
    through_with: Option<Vec<u8>>,
    width: u32,
    max_rows: usize,
) {
    let specs = every_door(width);
    let mut stingy = Harness::new(container, Sizing::Stingy);
    let mut roomy = Harness::new(container, Sizing::Roomy);
    stingy.row_limit = ROW_LIMIT;
    roomy.row_limit = ROW_LIMIT;
    stingy.through_with = through_with.clone();
    roomy.through_with = through_with;
    let open = |harness: &mut Harness<'_>| {
        let opened = harness.open().map_err(|failure| failure.code)?;
        let sheets = harness.sheets().map_err(|failure| failure.code)?;
        harness.load().map_err(|failure| failure.code)?;
        Ok::<_, u32>((opened, sheets))
    };
    let (opened, sheets) = match (open(&mut stingy), open(&mut roomy)) {
        (Ok(ours), Ok(theirs)) => {
            assert_eq!(ours, theirs);
            ours
        }
        (Err(ours), Err(theirs)) => {
            assert_eq!(ours, theirs);
            return;
        }
        (ours, theirs) => panic!("stingy says {ours:?} and roomy says {theirs:?}"),
    };
    assert!((1..=3).contains(&opened.format), "format {}", opened.format);
    assert_eq!(
        (&stingy.strings, &stingy.table, &stingy.kinds),
        (&roomy.strings, &roomy.table, &roomy.kinds)
    );
    for (index, listed) in sheets.iter().enumerate() {
        let ours = stingy.read(listed, options, &specs, max_rows);
        let theirs = roomy.read(listed, options, &specs, max_rows * 3 + 1);
        assert_eq!(ours.header, theirs.header, "the header of sheet {index}");
        // A reading cut short at the row limit stops at a batch boundary of its own, so
        // the two are compared over the rows both have.
        let common = ours.rows.len().min(theirs.rows.len());
        if !ours.truncated && !theirs.truncated {
            assert_eq!(
                ours.rows.len(),
                theirs.rows.len(),
                "the rows of sheet {index}"
            );
        }
        assert_eq!(
            ours.rows[..common],
            theirs.rows[..common],
            "the rows of sheet {index}"
        );
        // Empty both, off `fill_with`.
        let notes = ours.notes.len().min(theirs.notes.len());
        assert_eq!(
            (&ours.notes[..notes], &ours.widths[..notes]),
            (&theirs.notes[..notes], &theirs.widths[..notes]),
            "the notes and widths of sheet {index}"
        );
        if !ours.truncated && !theirs.truncated {
            assert_eq!(ours.failed, theirs.failed, "whether sheet {index} failed");
            if ours.failed {
                assert_eq!(stingy.last_failure, roomy.last_failure);
            }
        }
        if ours.failed || theirs.failed || ours.truncated || theirs.truncated {
            // A sheet that failed leaves its state block failed; the next sheet is read
            // only by a fresh pair, so the comparison ends here.
            return;
        }
    }
}
