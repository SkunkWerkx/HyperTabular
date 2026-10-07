//! The workbook targets' oracle: one package read twice — with buffers that start empty and
//! grow only as the core asks, and with room to spare and batches of another size — and
//! the two readings held to each other. The harness (rust/tests/support/workbook.rs, the
//! one the crate's own tests use) panics on anything the contract rules out: a code no call
//! may return, a refusal that asks for less than it has, a failure that names nothing, a
//! window past 256 MiB, a core that keeps asking.

use crate::support::{Harness, Sizing, every_door};
use hypertabular::workbook::SheetOptions;

/// Rows taken from one sheet before a reading stops: a few bytes of ODS can repeat a row a
/// million times, which is the core's to read and not the fuzzer's to wait for.
const ROW_LIMIT: usize = 2_000;

pub fn compare(container: &[u8], options: SheetOptions, width: u32, max_rows: usize) {
    let specs = every_door(width);
    let mut stingy = Harness::new(container, Sizing::Stingy);
    let mut roomy = Harness::new(container, Sizing::Roomy);
    stingy.row_limit = ROW_LIMIT;
    roomy.row_limit = ROW_LIMIT;
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
    assert!(opened.format == 1 || opened.format == 2);
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
