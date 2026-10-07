//! The XML a workbook's parts hold, any text, inside a well-formed package: the zip and
//! inflate layers are left whole so that the fuzzer's mutations land in the tokenizer, the
//! cell and row readers and the shared strings rather than in a broken directory. The
//! first byte chooses XLSX or ODS, whether the parts are deflated, the 1904 date system,
//! the sheet options and the batch size; for XLSX the rest is a sheet's `<sheetData>`
//! content and, after the first NUL, the shared strings' `<si>` elements; for ODS it is the
//! spreadsheet's `<table:table>` elements. Read through compare.rs, as the container
//! target is.

#![no_main]

mod compare;
#[path = "../../tests/support/packages.rs"]
mod packages;
#[path = "../../tests/support/workbook.rs"]
mod support;

use hypertabular::workbook::SheetOptions;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [sel, rest @ ..] = data else {
        return;
    };
    let options = SheetOptions::default()
        .with_header(sel & 0x08 != 0)
        .with_empty_rows_skipped(sel & 0x10 != 0);
    let deflate = sel & 0x02 != 0;
    let max_rows = 1 + usize::from(sel >> 5) * 3;
    let package = if sel & 1 == 0 {
        let (sheet, strings) = match rest.iter().position(|&byte| byte == 0) {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, &[][..]),
        };
        packages::xlsx(
            &[("S", &String::from_utf8_lossy(sheet))],
            &String::from_utf8_lossy(strings),
            sel & 0x04 != 0,
            deflate,
        )
    } else {
        packages::ods(&String::from_utf8_lossy(rest), deflate)
    };
    compare::compare(&package, options, 2, max_rows);
});
