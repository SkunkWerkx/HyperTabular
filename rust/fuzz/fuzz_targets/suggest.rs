//! The suggester, any bytes: the export must never panic, must keep to what it says of each
//! column, and must write nothing when it refuses; and what the Rust surface plans must read back
//! over the rows it sampled with no cell refused, failing only where a read fails.
//!
//! The leading two bytes choose the separator, the quoting, whether a header is read and the
//! format declared, then the sample's shape, with which cells are marked typed; the rest is the
//! input. The export takes the input cut into cells at 0x1F, and the Rust surface takes it whole.

#![no_main]

use hypertabular::kernel::abi::{ColumnSpec, ERR_CONTRACT, OK, RawSuggestion, Span};
use hypertabular::kernel::door::Door;
use hypertabular::kernel::exports::hypertabular_suggest;
use hypertabular::kernel::suggest::COMMA_DECIMAL;
use hypertabular::{
    Column, DelimitedReader, Dialect, Error, Grouping, NumFormat, Reason, SuggestOptions,
};
use libfuzzer_sys::fuzz_target;

const SEPARATORS: [u8; 4] = *b",\t;|";

fn declared(selector: u8) -> Option<NumFormat> {
    match selector >> 4 & 3 {
        1 => Some(NumFormat::INVARIANT),
        2 => Some(COMMA_DECIMAL),
        3 => Some(NumFormat::INVARIANT.with_grouping(Grouping::Indian)),
        _ => None,
    }
}

/// The export over `data`'s cells, `columns` wide, with a span past the end if `stray`.
fn export(data: &[u8], columns: usize, format: Option<NumFormat>, stray: bool, marks: u8) {
    let mut cells: Vec<Span> = Vec::new();
    let mut offset = 0;
    for cell in data.split(|&byte| byte == 0x1F) {
        cells.push(Span {
            offset: offset as u32,
            len: cell.len() as u32,
        });
        offset += cell.len() + 1;
    }
    let rows = cells.len() / columns;
    cells.truncate(rows * columns);
    if stray && let Some(last) = cells.last_mut() {
        last.offset = data.len() as u32;
        last.len = 1;
    }
    // The top bits of the shape, over and over, or no marks at all.
    let typed: Vec<u8> = (0..cells.len()).map(|at| marks >> (at % 4) & 1).collect();
    // The crate doesn't name HyperCast: a spec's field is the raw shape the export takes.
    let formats = match format {
        Some(format) => {
            let mut spec = ColumnSpec::new(0, Door::I32.code(), 0);
            spec.format = format.into();
            vec![spec.format; columns]
        }
        None => Vec::new(),
    };
    let sentinel = RawSuggestion {
        door: u32::MAX,
        ..RawSuggestion::default()
    };
    let mut out = vec![sentinel; columns];
    // SAFETY: every pointer and length is of the vector or slice just above.
    let code = unsafe {
        hypertabular_suggest(
            data.as_ptr(),
            data.len(),
            data.as_ptr(),
            0,
            cells.as_ptr(),
            if marks == 0 {
                core::ptr::null()
            } else {
                typed.as_ptr()
            },
            rows,
            columns,
            if formats.is_empty() {
                std::ptr::null()
            } else {
                formats.as_ptr()
            },
            out.as_mut_ptr(),
        )
    };
    if stray && !cells.is_empty() {
        assert_eq!(code, ERR_CONTRACT);
        assert!(out.iter().all(|entry| *entry == sentinel));
        return;
    }
    assert_eq!(code, OK);
    for entry in &out {
        assert!(entry.matched <= entry.sampled && entry.sampled as usize <= rows);
        match entry.status {
            RawSuggestion::RESOLVED => {
                assert_eq!(entry.matched, entry.sampled);
                assert!(entry.sampled > 0);
                let door = Door::from_code(entry.door, entry.param).expect("a door");
                if door.is_numeric() {
                    let notation = format.is_none()
                        && (entry.param == RawSuggestion::NOTATION_COMMA_GROUPS
                            || entry.param == RawSuggestion::NOTATION_COMMA_DECIMAL);
                    assert!(entry.param == 0 || notation, "{entry:?}");
                }
            }
            RawSuggestion::AMBIGUOUS => {
                assert_eq!(entry.matched, entry.sampled);
                assert!(entry.sampled > 0);
                assert!(entry.param != 0);
                Door::from_code(entry.door, entry.param.trailing_zeros()).expect("a door");
            }
            RawSuggestion::UNRESOLVED => {
                assert_eq!(entry.param, 0);
                assert_eq!(Door::from_code(entry.door, 0), Some(Door::Text));
            }
            other => panic!("status {other}"),
        }
    }
}

/// The Rust surface over the input whole: a resolved plan reads back what was sampled, and a
/// failure is one a read has too.
fn surface(data: &[u8], dialect: Dialect, format: Option<NumFormat>) {
    let mut options = SuggestOptions::new().sample_rows(64);
    if let Some(format) = format {
        options = options.format(format);
    }
    let found = match DelimitedReader::suggest_slice(data, dialect, &options) {
        Ok(found) => found,
        // A source too wide to plan for, which a read of one of its columns needn't mind.
        Err(Error::Plan { .. }) => return,
        Err(error) => {
            let Ok(mut read) = DelimitedReader::from_slice(data, dialect, &[Column::text(0)])
            else {
                return;
            };
            loop {
                match read.read() {
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("{error:?}, where a read has no failure"),
                    Err(_) => return,
                }
            }
        }
    };
    if found.plan.is_empty() {
        return;
    }
    let mut read = DelimitedReader::from_slice(data, dialect, &found.plan).expect("the sample's");
    let mut rows = 0;
    while rows < 64 {
        let Ok(Some(batch)) = read.read() else {
            break;
        };
        for problem in batch.faults() {
            let refused = matches!(problem.reason, Some(Reason::Malformed | Reason::OutOfRange));
            if refused && problem.row + rows < 64 {
                let door = found.plan[problem.plan_column.expect("a column")].door;
                assert_eq!(door, Door::Text, "{problem:?}");
            }
        }
        rows += batch.rows();
    }
}

fuzz_target!(|data: &[u8]| {
    let [selector, shape, input @ ..] = data else {
        return;
    };
    let format = declared(*selector);
    let dialect = Dialect::new(SEPARATORS[usize::from(selector & 3)])
        .with_quoting(selector & 4 != 0)
        .with_header(selector & 8 != 0);
    export(
        input,
        usize::from(shape & 7) + 1,
        format,
        shape & 8 != 0,
        shape >> 4,
    );
    surface(input, dialect, format);
});
