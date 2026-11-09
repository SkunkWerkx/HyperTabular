//! The core's allocation story, which is that it has none. A counting allocator watches
//! a whole input go through it — the state, the header, every door, escaped cells, a
//! structural failure — and the count is not "nothing once warm" but nothing at all:
//! every byte the core touched was handed to it.
//!
//! `check-core.sh` proves the same thing of the shipped library a different way (it
//! imports no allocator); this is the proof for the code as the test suite builds it.
//!
//! The count is the test thread's own: the core runs on its caller's thread and starts
//! none, while the harness's threads allocate as they please (a process-wide count once
//! caught four of theirs on linux-arm64).

use hypertabular::kernel::abi::{
    BuffersWith, CellNote, CellVerdict, ColumnBuffer, ColumnBufferWith, ColumnSpec, ColumnSpecWith,
    ERR_STRUCTURE, Filled, OK, RawDelimitedOptions, RawProblem, RawSuggestion, RawTranscoded, Span,
};
use hypertabular::kernel::delimited::fill::{self, RawDialect, State};
use hypertabular::kernel::exports::{
    hypertabular_delimited_fill_with, hypertabular_delimited_header,
    hypertabular_delimited_init_with, hypertabular_faults, hypertabular_suggest,
    hypertabular_transcode,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;

thread_local! {
    // Const-initialised and without a destructor, so reading it never allocates.
    static COUNTED: Cell<bool> = const { Cell::new(false) };
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOC_COUNT.try_with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const ROWS: usize = 256;

#[test]
fn allocation_free() {
    COUNTED.set(true);
    // Everything the caller owns, allocated before the count starts.
    let mut text = b"\xEF\xBB\xBFid,amount,when,name,flag,note\n".to_vec();
    for row in 0..5_000 {
        text.extend_from_slice(
            format!(
                "{row},\"{}.{:02}\",2024-01-{:02}T10:30:00Z,\"n \"\"{row}\"\" x\",{},{}\n",
                row * 37,
                row % 100,
                1 + row % 28,
                if row % 2 == 0 { "yes" } else { "no" },
                if row % 7 == 0 { "not a number" } else { "12.5" },
            )
            .as_bytes(),
        );
    }
    text.extend_from_slice(b"too,few\n");

    // Every door, over the six columns.
    let mut specs = Vec::new();
    for code in 1..=22u32 {
        specs.push(ColumnSpec::new((code - 1) % 6, code, 1));
    }
    let mut values: Vec<Vec<u8>> = specs.iter().map(|_| vec![0u8; 16 * ROWS]).collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = specs
        .iter()
        .map(|_| vec![CellVerdict::default(); ROWS])
        .collect();
    let columns: Vec<ColumnBuffer> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        })
        .collect();
    let mut cells = vec![Span::default(); ROWS * 7];
    let mut arena = vec![0u8; 64 * 1024];
    let mut names = [Span::default(); 16];

    let before = ALLOC_COUNT.get();

    let dialect = RawDialect {
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
    };
    let mut state = State::init(dialect).expect("a valid dialect");
    let mut out = Filled::default();
    let mut at = 0usize;
    let code = fill::header(&mut state, &text, true, &mut names, &mut arena, &mut out);
    at += out.consumed as usize;
    let header = (code, out.rows);
    let mut rows = 0u64;
    let mut faults = 0u64;
    let last = loop {
        // SAFETY: sixteen bytes a row covers every door's value.
        let code = unsafe {
            fill::fill(
                &mut state,
                &text[at..],
                true,
                &specs,
                &columns,
                ROWS,
                &mut cells,
                &mut arena,
                &mut out,
            )
        };
        if code != OK || (out.rows == 0 && out.consumed == 0) {
            break code;
        }
        at += out.consumed as usize;
        rows += out.rows;
        for column in &verdicts {
            faults += column
                .iter()
                .take(out.rows as usize)
                .filter(|verdict| !verdict.is_ok())
                .count() as u64;
        }
    };

    let allocations = ALLOC_COUNT.get() - before;

    assert_eq!(header, (OK, 6));
    assert_eq!(rows, 5_000);
    assert!(faults > 0, "the failure paths were exercised too");
    assert_eq!(last, ERR_STRUCTURE, "and so was a structural failure");
    assert_eq!(allocations, 0, "the core allocated {allocations} time(s)");
}

/// The text door's validation, on what it has to look into: multi-byte text, quoted and
/// escaped, a cell it refuses, and a header it refuses.
#[test]
fn valid_text_allocates_nothing() {
    COUNTED.set(true);
    let mut text = "prénom,ville\n".as_bytes().to_vec();
    for row in 0..5_000 {
        text.extend_from_slice(format!("é{row},\"日本 \"\"語\"\" 😀\"\n").as_bytes());
    }
    text.extend_from_slice(b"\xe9t\xe9,\"\xff\"\n");
    let bad_header = b"a,\xe9\n1,2\n";

    let specs = [ColumnSpec::new(0, 18, 0), ColumnSpec::new(1, 18, 0)];
    let mut values = vec![vec![0u8; size_of::<Span>() * ROWS]; 2];
    let mut verdicts = vec![vec![CellVerdict::default(); ROWS]; 2];
    let columns: Vec<ColumnBuffer> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .map(|(values, verdicts)| ColumnBuffer {
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
        })
        .collect();
    let mut cells = vec![Span::default(); ROWS * 3];
    let mut arena = vec![0u8; 64 * 1024];
    let mut names = [Span::default(); 4];
    let dialect = RawDialect {
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
    };

    let before = ALLOC_COUNT.get();

    let mut state = State::init(dialect).expect("a valid dialect");
    let mut out = Filled::default();
    let header = fill::header(&mut state, &text, true, &mut names, &mut arena, &mut out);
    let mut at = out.consumed as usize;
    let (mut rows, mut faults) = (0u64, 0u64);
    loop {
        // SAFETY: each column holds `ROWS` spans and verdicts.
        let code = unsafe {
            fill::fill(
                &mut state,
                &text[at..],
                true,
                &specs,
                &columns,
                ROWS,
                &mut cells,
                &mut arena,
                &mut out,
            )
        };
        assert_eq!(code, OK);
        if out.rows == 0 {
            break;
        }
        at += out.consumed as usize;
        rows += out.rows;
        for column in &verdicts {
            faults += column
                .iter()
                .take(out.rows as usize)
                .filter(|verdict| verdict.reason == 2)
                .count() as u64;
        }
    }
    let mut state = State::init(dialect).expect("a valid dialect");
    let refused = fill::header(
        &mut state, bad_header, true, &mut names, &mut arena, &mut out,
    );

    let allocations = ALLOC_COUNT.get() - before;

    assert_eq!(header, OK);
    assert_eq!((rows, faults), (5_001, 2));
    assert_eq!(refused, ERR_STRUCTURE);
    assert_eq!(allocations, 0, "the core allocated {allocations} time(s)");
}

/// `init_with` and `fill_with`, with everything on: lines skipped, records discarded, rows padded
/// and cut, tokens at both levels, date options with a vocabulary, notes and row widths.
#[test]
fn options_and_notes_allocate_nothing() {
    COUNTED.set(true);
    let mut text = b"title \"one\nx,y\na,b,c,d\n".to_vec();
    for row in 0..5_000 {
        match row % 5 {
            0 => text.extend_from_slice(format!("{row},NA,\"NA\",07-Jan-26\n").as_bytes()),
            1 => text.extend_from_slice(format!("{row}\n").as_bytes()),
            2 => text
                .extend_from_slice(format!("x{row},-,\"\"\"\",1-Feb-26,more,cells\n").as_bytes()),
            3 => {
                text.extend_from_slice(format!("{row},-,").as_bytes());
                text.extend_from_slice(b"\xff,bad\n");
            }
            _ => text.extend_from_slice(format!("{row},é,日本,3-Mar-26\n").as_bytes()),
        }
    }
    let mut months = Vec::new();
    for (month, name) in [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .enumerate()
    {
        months.push(month as u8 + 1);
        months.extend_from_slice(&(name.len() as u32).to_le_bytes());
        months.extend_from_slice(name.as_bytes());
    }
    let tokens = b"\x02\0\0\0NA\x00\0\0\0".to_vec();
    let dash = b"\x01\0\0\0-".to_vec();
    let mut specs = [
        ColumnSpecWith::new(0, 4, 0),
        ColumnSpecWith::new(1, 18, 0),
        ColumnSpecWith::new(1, 18, 0),
        ColumnSpecWith::new(2, 18, 0),
        ColumnSpecWith::new(3, 20, 3),
        ColumnSpecWith::new(0, 15, 0),
    ];
    specs[2].tokens = dash.as_ptr();
    specs[2].tokens_len = dash.len();
    specs[4].date = hypercast::RawDateOptions {
        size: hypercast::RawDateOptions::SIZE,
        flags: 0,
        pivot: 2029,
    };
    specs[4].months = months.as_ptr();
    specs[4].months_len = months.len();
    specs[5].date = hypercast::RawDateOptions {
        size: hypercast::RawDateOptions::SIZE,
        flags: hypercast::DateOptions::COMPACT_DATE,
        pivot: 0,
    };
    let mut values: Vec<Vec<u8>> = specs.iter().map(|_| vec![0u8; 16 * ROWS]).collect();
    let mut verdicts: Vec<Vec<CellVerdict>> = specs
        .iter()
        .map(|_| vec![CellVerdict::default(); ROWS])
        .collect();
    let mut notes: Vec<Vec<CellNote>> = specs
        .iter()
        .map(|_| vec![CellNote::default(); ROWS])
        .collect();
    let columns: Vec<ColumnBufferWith> = values
        .iter_mut()
        .zip(verdicts.iter_mut())
        .zip(notes.iter_mut())
        .map(|((values, verdicts), notes)| ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: values.as_mut_ptr().cast(),
            verdicts: verdicts.as_mut_ptr(),
            notes: notes.as_mut_ptr(),
        })
        .collect();
    let mut cells = vec![Span::default(); ROWS * 5];
    let mut arena = vec![0u8; 64 * 1024];
    let mut names = [Span::default(); 16];
    let mut widths = vec![0u32; ROWS];
    let mut block = vec![0u64; size_of::<State>().div_ceil(8)];
    let state = block.as_mut_ptr().cast::<State>();
    let options = RawDelimitedOptions {
        size: RawDelimitedOptions::SIZE,
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
        skip: 1,
        header_row: 1,
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        long_rows: RawDelimitedOptions::LONG_ROWS_REPORT,
    };

    let before = ALLOC_COUNT.get();

    let mut out = Filled::default();
    let mut buffers = BuffersWith::empty();
    buffers.arena = arena.as_mut_ptr();
    buffers.arena_cap = arena.len();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    buffers.tokens = tokens.as_ptr();
    buffers.tokens_len = tokens.len();
    buffers.row_widths = widths.as_mut_ptr();
    let (mut rows, mut noted, mut listed) = (0u64, 0u64, 0u64);
    let mut problems = [RawProblem::default(); 3];
    let (init, header);
    // SAFETY: every buffer is the size given beside it, sixteen bytes a row a value.
    unsafe {
        init = hypertabular_delimited_init_with(state, &options);
        header = hypertabular_delimited_header(
            state,
            text.as_ptr(),
            text.len(),
            1,
            names.as_mut_ptr(),
            names.len(),
            arena.as_mut_ptr(),
            arena.len(),
            &mut out,
        );
        let mut at = out.consumed as usize;
        loop {
            let rest = &text[at..];
            let code = hypertabular_delimited_fill_with(
                state,
                rest.as_ptr(),
                rest.len(),
                1,
                specs.as_ptr(),
                columns.as_ptr(),
                specs.len(),
                ROWS,
                &buffers,
                &mut out,
            );
            assert_eq!(code, OK);
            if out.rows == 0 {
                break;
            }
            at += out.consumed as usize;
            rows += out.rows;
            for column in &notes {
                noted += column
                    .iter()
                    .take(out.rows as usize)
                    .filter(|note| note.origin != 0 || note.detail != 0)
                    .count() as u64;
            }
            // The batch's list, a few entries a call.
            let mut cursor = 0;
            loop {
                let mut written = 0;
                let code = hypertabular_faults(
                    columns.as_ptr(),
                    columns.len(),
                    out.rows as usize,
                    widths.as_ptr(),
                    (*state).expected,
                    RawDelimitedOptions::LONG_ROWS_REPORT,
                    &mut cursor,
                    problems.as_mut_ptr(),
                    problems.len(),
                    &mut written,
                );
                assert_eq!(code, OK);
                listed += written as u64;
                if written < problems.len() {
                    break;
                }
            }
        }
    }

    let allocations = ALLOC_COUNT.get() - before;

    assert_eq!((init, header), (OK, OK));
    assert_eq!(rows, 5_000);
    assert!(noted > 5_000, "{noted} cells had a note");
    assert!(listed >= 1_000, "{listed} problems were listed");
    assert_eq!(allocations, 0, "the core allocated {allocations} time(s)");
}

#[test]
fn transcoding_allocates_nothing() {
    COUNTED.set(true);
    // Everything the caller owns, allocated before the count starts.
    let single: Vec<u8> = (0..=255).cycle().take(64 * 1024).collect();
    let good: Vec<u8> = "a\u{e9}\u{20AC}\u{1F600}\n"
        .encode_utf16()
        .cycle()
        .take(64 * 1024)
        .flat_map(u16::to_le_bytes)
        .collect();
    let bad = [&good[..], &[0x00, 0xDC]].concat();
    let mut out = vec![0u8; 256 * 1024];
    let inputs = [
        (RawTranscoded::WINDOWS_1252, &single),
        (RawTranscoded::LATIN_1, &single),
        (RawTranscoded::UTF_16LE, &good),
        (RawTranscoded::UTF_16LE, &bad),
    ];

    ALLOC_COUNT.set(0);
    for (encoding, input) in inputs {
        // All of it at once, then through little room and in little pieces.
        for (cut, cap) in [(input.len(), out.len()), (1021, 7), (7, 1021)] {
            let mut at = 0;
            loop {
                let end = input.len().min(at + cut);
                let mut progress = RawTranscoded::default();
                // SAFETY: each pointer is the slice's, for what's given beside it.
                let code = unsafe {
                    hypertabular_transcode(
                        encoding,
                        input[at..end].as_ptr(),
                        end - at,
                        u32::from(end == input.len()),
                        out.as_mut_ptr(),
                        cap,
                        &mut progress,
                    )
                };
                assert_eq!(code, OK);
                at += progress.consumed as usize;
                if progress.failed != 0 || at == input.len() {
                    break;
                }
            }
        }
    }
    assert_eq!(ALLOC_COUNT.get(), 0, "the core asked for memory");
}

/// Every door tried on every cell, a declared format and the two candidates, arena and input both.
#[test]
fn suggesting_allocates_nothing() {
    COUNTED.set(true);
    let values: [&[u8]; 16] = [
        b"true",
        b"1",
        b"5000000000",
        b"2.5",
        b"1,234",
        b"007",
        b"99999999999999999999",
        b"2026-01-07",
        b"01/02/2026",
        b"2026-01-07T00:00:00Z",
        b"6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        b"abc",
        b"\xff",
        b"  ",
        b"",
        b"3.14159265358979323846264338327950288",
    ];
    let mut input = Vec::new();
    let mut arena = Vec::new();
    let mut cells = Vec::new();
    for column in 0..values.len() {
        for row in 0..ROWS {
            // Some columns are of one kind, and some of every kind.
            let value = if column % 2 == 0 {
                values[column]
            } else {
                values[row % values.len()]
            };
            let (store, flag) = if row % 3 == 0 {
                (&mut arena, Span::FLAG)
            } else {
                (&mut input, 0)
            };
            cells.push(Span {
                offset: store.len() as u32,
                len: value.len() as u32 | flag,
            });
            store.extend_from_slice(value);
        }
    }
    let mut out = vec![RawSuggestion::default(); values.len()];
    let formats =
        vec![hypercast::RawNumFormat::from(hypercast::NumFormat::INVARIANT); values.len()];
    let marks: Vec<u8> = (0..cells.len()).map(|at| (at % 2) as u8).collect();
    ALLOC_COUNT.set(0);
    for (declared, typed) in [
        (core::ptr::null(), core::ptr::null()),
        (formats.as_ptr(), core::ptr::null()),
        (core::ptr::null(), marks.as_ptr()),
        (formats.as_ptr(), marks.as_ptr()),
    ] {
        let code = unsafe {
            hypertabular_suggest(
                input.as_ptr(),
                input.len(),
                arena.as_ptr(),
                arena.len(),
                cells.as_ptr(),
                typed,
                ROWS,
                values.len(),
                declared,
                out.as_mut_ptr(),
            )
        };
        assert_eq!(code, OK);
    }
    assert_eq!(ALLOC_COUNT.get(), 0);
    // A column of every kind, to say the whole thing ran.
    assert_eq!(out[1].status, RawSuggestion::UNRESOLVED);
    // Two of the sixteen values are blanks.
    assert_eq!(out[1].sampled, 224);
    assert_eq!(out[0].door, 1);
}
