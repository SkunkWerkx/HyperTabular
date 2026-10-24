//! The cells of a Numbers table, any bytes, inside a well-formed package: the zip, the
//! chunks and the object headers are left whole so that the fuzzer's mutations land in the
//! tile reader and the cell storage rather than in a broken container. The first byte
//! chooses where the rest goes — the whole payload of the first tile's object, or the
//! storage buffer and (after the first NUL) the cell offsets of its first row — and whether
//! the parts are deflated, the offsets wide, the sheet options and the batch size. Read
//! through compare.rs, as the container target is.

#![no_main]

mod compare;
#[path = "../../tests/support/packages.rs"]
mod packages;
#[path = "../../tests/support/workbook.rs"]
mod support;

use hypertabular::workbook::SheetOptions;
use libfuzzer_sys::fuzz_target;
use packages::{NCell, NSheet, NTable, NumbersShape};

fuzz_target!(|data: &[u8]| {
    let [sel, rest @ ..] = data else {
        return;
    };
    let options = SheetOptions::default()
        .with_header(sel & 0x08 != 0)
        .with_empty_rows_skipped(sel & 0x10 != 0);
    let max_rows = 1 + usize::from(sel >> 5) * 3;
    let mut shape = NumbersShape {
        deflate: sel & 0x02 != 0,
        wide: sel & 0x04 != 0,
        chunk: if sel & 0x04 != 0 { 29 } else { 65536 },
        ..NumbersShape::default()
    };
    if sel & 1 == 0 {
        shape.raw_tile = Some(rest.to_vec());
    } else {
        let (storage, offsets) = match rest.iter().position(|&byte| byte == 0) {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, &[][..]),
        };
        shape.raw_row = Some((storage.to_vec(), offsets.to_vec()));
    }
    let sheets = [NSheet {
        name: "S".to_string(),
        hidden: false,
        tables: vec![NTable {
            name: "T".to_string(),
            header_rows: 1,
            rows: vec![
                vec![
                    NCell::Text("a".to_string()),
                    NCell::Number(1.5),
                    NCell::Date(0.0, false),
                ],
                vec![
                    NCell::Rich("b c".to_string()),
                    NCell::Bool(true),
                    NCell::Duration(61.0),
                ],
            ],
        }],
    }];
    compare::compare(&packages::numbers(&sheets, &shape), options, 3, max_rows);
});
