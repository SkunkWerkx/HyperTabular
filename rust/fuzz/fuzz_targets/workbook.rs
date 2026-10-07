//! A workbook container, any bytes: XLSX or ODS, through the zip directory, inflate, the
//! package's relationships, the shared strings, the styles and every sheet, every door
//! over every source column. The core must never panic, never stall, never ask for room
//! it does not need, and must read the same with buffers that start empty as with room to
//! spare (see compare.rs). The leading two bytes choose the sheet options, the batch size
//! and the plan's width; the rest is the container, seeded from corpus/workbook/.

#![no_main]

mod compare;
#[path = "../../tests/support/workbook.rs"]
mod support;

use hypertabular::workbook::SheetOptions;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [options_sel, sizes, container @ ..] = data else {
        return;
    };
    let options = SheetOptions::default()
        .with_header(options_sel & 1 != 0)
        .with_empty_rows_skipped(options_sel & 2 != 0);
    let max_rows = 1 + usize::from(sizes & 0x1F);
    let width = 1 + u32::from(sizes >> 5) % 3;
    compare::compare(container, options, width, max_rows);
});
