//! HyperCast's checked doors through the `_with` fills, at the raw ABI: a column's
//! `constraints`, its packed `enum` and its `checks`, judged before any cell and then per cell.

use super::packages::xlsx;
use super::support::{Harness, Sizing};
use hypercast::{
    Constraints, Decimal, Keyword, NumericRules, RawCheck, RawConstraints, Rounding, TextRules,
    decimal_for_declaration,
};
use hypertabular::Door;
use hypertabular::kernel::abi::{
    BuffersWith, CellNote, CellVerdict, ColumnBufferWith, ColumnSpecWith, ERR_CONTRACT, Filled, OK,
    RawDelimitedOptions, RawProblem, RawSheetOptions, Span,
};
use hypertabular::kernel::delimited::fill::State;
use hypertabular::kernel::exports::{
    hypertabular_delimited_fill_with, hypertabular_delimited_init_with,
    hypertabular_delimited_state_size, hypertabular_faults, hypertabular_workbook_fill_with,
    hypertabular_workbook_sheet_with,
};

/// What a buffer holds before a call, so a write the core shouldn't make shows.
const JUNK_VERDICT: CellVerdict = CellVerdict {
    offset: 0xAA,
    len: 0xAA,
    reason: 0xAA,
};
const JUNK_NOTE: CellNote = CellNote {
    detail: 0xAAAA,
    origin: 0xAAAA,
};
const JUNK_CHECK: RawCheck = RawCheck {
    keyword: 0xAA,
    adjusted: 0xAA,
    enum_index: 0xAA,
};

/// A declaration as it crosses the ABI: the layout and its packed `enum`.
pub(crate) struct Declared {
    pub(crate) raw: RawConstraints,
    pub(crate) entries: Vec<u8>,
}

impl Declared {
    pub(crate) fn of<T>(constraints: &Constraints<'_, T>) -> Declared {
        let len = constraints.pack_enum(&mut []).expect("it packs");
        let mut entries = vec![0u8; len];
        assert_eq!(constraints.pack_enum(&mut entries), Some(len));
        Declared {
            raw: constraints.raw(),
            entries,
        }
    }

    /// A layout HyperCast refuses: `multipleOf: 0`, its code 5.
    pub(crate) fn refused() -> Declared {
        Declared {
            raw: RawConstraints {
                flags: RawConstraints::HAS_MULTIPLE_OF,
                ..RawConstraints::ZERO
            },
            entries: Vec::new(),
        }
    }

    /// No keyword at all: a valid declaration that holds every value of a door that takes one.
    pub(crate) fn nothing() -> Declared {
        Declared {
            raw: RawConstraints::ZERO,
            entries: Vec::new(),
        }
    }

    /// `spec`, declaring this.
    pub(crate) fn on(&self, mut spec: ColumnSpecWith) -> ColumnSpecWith {
        spec.constraints = &self.raw;
        if !self.entries.is_empty() {
            spec.enum_ptr = self.entries.as_ptr();
            spec.enum_len = self.entries.len();
        }
        spec
    }
}

pub(crate) fn decimal(text: &str) -> Decimal {
    decimal_for_declaration(text).expect("a declaration's decimal")
}

pub(crate) fn at_most(text: &str) -> Constraints<'static, i32> {
    Constraints::numeric(NumericRules::new().maximum(decimal(text)), None).expect("it declares")
}

/// One column's arrays, junk-filled, room for `rows`.
pub(crate) struct Column {
    pub(crate) values: Vec<u8>,
    pub(crate) verdicts: Vec<CellVerdict>,
    pub(crate) notes: Vec<CellNote>,
    pub(crate) checks: Vec<RawCheck>,
    /// Whether `checks` is passed; null otherwise.
    pub(crate) checked: bool,
}

impl Column {
    pub(crate) fn new(rows: usize, checked: bool) -> Column {
        Column {
            values: vec![0xAA; 32 * rows.max(1)],
            verdicts: vec![JUNK_VERDICT; rows.max(1)],
            notes: vec![JUNK_NOTE; rows.max(1)],
            checks: vec![JUNK_CHECK; rows.max(1)],
            checked,
        }
    }

    fn buffer(&mut self) -> ColumnBufferWith {
        ColumnBufferWith {
            size: ColumnBufferWith::SIZE,
            values: self.values.as_mut_ptr().cast(),
            verdicts: self.verdicts.as_mut_ptr(),
            notes: self.notes.as_mut_ptr(),
            checks: if self.checked {
                self.checks.as_mut_ptr()
            } else {
                core::ptr::null_mut()
            },
        }
    }

    /// Nothing written since [`Column::new`].
    pub(crate) fn untouched(&self) -> bool {
        self.values.iter().all(|&byte| byte == 0xAA)
            && self.verdicts.iter().all(|&verdict| verdict == JUNK_VERDICT)
            && self.notes.iter().all(|&note| note == JUNK_NOTE)
            && self.checks.iter().all(|&check| check == JUNK_CHECK)
    }

    pub(crate) fn value<T: Copy>(&self, row: usize) -> T {
        // SAFETY: the values hold 32 bytes a row, wider than any door's value.
        unsafe {
            self.values
                .as_ptr()
                .add(row * size_of::<T>())
                .cast::<T>()
                .read_unaligned()
        }
    }
}

fn options() -> RawDelimitedOptions {
    RawDelimitedOptions {
        size: RawDelimitedOptions::SIZE,
        separator: b',',
        quoting: 1,
        skip_blank_lines: 1,
        engine: 0,
        skip: 0,
        header_row: 0,
        short_rows: RawDelimitedOptions::SHORT_ROWS_PAD,
        long_rows: 0,
    }
}

/// All of `data`, last, through one `fill_with` of `max_rows`: the code and what `out` says.
/// `out.rows` starts at `0xAA`, so a refusal that wrote it shows.
pub(crate) fn delimited(
    data: &[u8],
    specs: &[ColumnSpecWith],
    columns: &mut [Column],
    tokens: &[u8],
    max_rows: usize,
) -> (i32, Filled) {
    // SAFETY: no contract.
    let bytes = unsafe { hypertabular_delimited_state_size() };
    let mut block = vec![0u64; bytes.div_ceil(8)];
    let state = block.as_mut_ptr().cast::<State>();
    // SAFETY: the block is as large as the library asks.
    assert_eq!(
        unsafe { hypertabular_delimited_init_with(state, &options()) },
        OK
    );
    let mut cells = vec![Span::default(); 64];
    let mut arena = vec![0u8; 256];
    let buffers_of: Vec<ColumnBufferWith> = columns.iter_mut().map(Column::buffer).collect();
    let mut buffers = BuffersWith::empty();
    buffers.arena = arena.as_mut_ptr();
    buffers.arena_cap = arena.len();
    buffers.cells = cells.as_mut_ptr();
    buffers.cells_cap = cells.len();
    buffers.tokens = tokens.as_ptr();
    buffers.tokens_len = tokens.len();
    let mut out = Filled {
        rows: 0xAA,
        ..Filled::default()
    };
    // SAFETY: every column holds `max_rows` of everything, and each spec's lists are live.
    let code = unsafe {
        hypertabular_delimited_fill_with(
            state,
            data.as_ptr(),
            data.len(),
            1,
            specs.as_ptr(),
            buffers_of.as_ptr(),
            specs.len(),
            max_rows,
            &buffers,
            &mut out,
        )
    };
    (code, out)
}

/// The first sheet of `package`, positioned, through one `fill_with` of `max_rows`.
pub(crate) fn workbook(
    package: &[u8],
    specs: &[ColumnSpecWith],
    columns: &mut [Column],
    max_rows: usize,
) -> (i32, Filled) {
    let mut harness = Harness::new(package, Sizing::Roomy);
    harness.open().expect("it opens");
    let sheets = harness.sheets().expect("its sheets");
    harness.load().expect("its tables");
    harness.row = vec![Default::default(); 8];
    let options = RawSheetOptions {
        size: RawSheetOptions::SIZE,
        has_header: 0,
        skip_empty_rows: 0,
        skip: 0,
        header_row: 0,
        first_row: 0,
        first_column: 0,
        last_row: 0,
        last_column: 0,
    };
    let mut out = Filled::default();
    let (part, index) = (&sheets[0].part, sheets[0].index);
    // SAFETY: the state was opened over this container.
    let code = unsafe {
        hypertabular_workbook_sheet_with(
            &mut *harness.state,
            package.as_ptr(),
            package.len(),
            part.as_ptr(),
            part.len(),
            index,
            &options,
            &mut out,
        )
    };
    assert_eq!(code, OK);
    let buffers_of: Vec<ColumnBufferWith> = columns.iter_mut().map(Column::buffer).collect();
    let mut buffers = BuffersWith::empty();
    buffers.window = harness.window.as_mut_ptr();
    buffers.window_cap = harness.window.len();
    buffers.arena = harness.arena.as_mut_ptr();
    buffers.arena_cap = harness.arena.len();
    buffers.cells = harness.cells.as_mut_ptr();
    buffers.cells_cap = harness.cells.len();
    buffers.row = harness.row.as_mut_ptr();
    buffers.row_cap = harness.row.len();
    buffers.strings = harness.strings.as_ptr();
    buffers.strings_len = harness.strings.len();
    buffers.table = harness.table.as_ptr();
    buffers.table_len = harness.table.len();
    buffers.kinds = harness.kinds.as_ptr();
    buffers.kinds_len = harness.kinds.len();
    let mut out = Filled {
        rows: 0xAA,
        ..Filled::default()
    };
    // SAFETY: every column holds `max_rows` of everything, and each spec's lists are live.
    let code = unsafe {
        hypertabular_workbook_fill_with(
            &mut *harness.state,
            package.as_ptr(),
            package.len(),
            specs.as_ptr(),
            buffers_of.as_ptr(),
            specs.len(),
            max_rows,
            &buffers,
            &mut out,
        )
    };
    (code, out)
}

/// A sheet of one column of inline strings, one a row.
pub(crate) fn sheet_of(cells: &[&str]) -> Vec<u8> {
    let cells: Vec<String> = cells
        .iter()
        .map(|cell| format!("<c t=\"inlineStr\"><is><t>{cell}</t></is></c>"))
        .collect();
    sheet_with(&cells.iter().map(String::as_str).collect::<Vec<_>>())
}

/// A sheet of one column, each row's cell element as given.
pub(crate) fn sheet_with(cells: &[&str]) -> Vec<u8> {
    let rows: String = cells
        .iter()
        .map(|cell| format!("<row>{cell}</row>"))
        .collect();
    xlsx(&[("Data", &rows)], "", false, false)
}

fn spec(door: Door) -> ColumnSpecWith {
    ColumnSpecWith::new(0, door.code(), 0)
}

/// Each refusal the fills share, with the column it refuses and whether that column passes
/// `checks`; the second column in each is fine, and must be left as it was too.
fn refusals<'d>(
    refused: &'d Declared,
    nothing: &'d Declared,
) -> Vec<(&'static str, ColumnSpecWith, bool)> {
    let mut cases = vec![
        ("a refused declaration", refused.on(spec(Door::I32)), true),
        ("no checks", nothing.on(spec(Door::I32)), false),
    ];
    for door in [Door::Uuid, Door::Date, Door::Bool] {
        cases.push(("an unchecked door", nothing.on(spec(door)), true));
    }
    cases
}

/// Every refusal, through `fill`, with the bad column first and then last: nothing written.
fn every_refusal_writes_nothing(
    fill: impl Fn(&[ColumnSpecWith], &mut [Column], usize) -> (i32, Filled),
) {
    let (refused, nothing) = (Declared::refused(), Declared::nothing());
    for (name, bad, checked) in refusals(&refused, &nothing) {
        for bad_first in [true, false] {
            let fine = spec(Door::I32);
            let specs = if bad_first { [bad, fine] } else { [fine, bad] };
            let mut columns = if bad_first {
                [Column::new(2, checked), Column::new(2, false)]
            } else {
                [Column::new(2, false), Column::new(2, checked)]
            };
            let (code, out) = fill(&specs, &mut columns, 2);
            assert_eq!(code, ERR_CONTRACT, "{name}, first: {bad_first}");
            assert_eq!(out.rows, 0xAA, "{name} wrote out");
            for (at, column) in columns.iter().enumerate() {
                assert!(column.untouched(), "{name} wrote column {at}");
            }
        }
    }
}

#[test]
fn a_delimited_fill_judges_every_column_before_any_cell() {
    every_refusal_writes_nothing(|specs, columns, rows| {
        delimited(b"1\n2\n", specs, columns, &[], rows)
    });
}

#[test]
fn a_workbook_fill_judges_every_column_before_any_cell() {
    let package = sheet_of(&["1", "2"]);
    every_refusal_writes_nothing(|specs, columns, rows| workbook(&package, specs, columns, rows));
}

#[test]
fn an_empty_batch_still_judges_declarations() {
    let (refused, nothing) = (Declared::refused(), Declared::nothing());
    // A refused declaration, and a declaration with no checks to write to: no rows to fill, or
    // no room for any, and the column is still judged.
    for (name, bad, checked) in [
        ("a refused declaration", refused.on(spec(Door::I32)), true),
        ("no checks", nothing.on(spec(Door::I32)), false),
    ] {
        let specs = [bad];
        for (input, rows) in [(&b""[..], 2), (&b"1\n"[..], 0)] {
            let mut columns = [Column::new(rows, checked)];
            let (code, _) = delimited(input, &specs, &mut columns, &[], rows);
            assert_eq!(code, ERR_CONTRACT, "{name}: {input:?} into {rows}");
            assert!(columns[0].untouched());
        }
        for (cells, rows) in [(&[][..], 2), (&["1"][..], 0)] {
            let mut columns = [Column::new(rows, checked)];
            let (code, _) = workbook(&sheet_of(cells), &specs, &mut columns, rows);
            assert_eq!(code, ERR_CONTRACT, "{name}: {cells:?} into {rows}");
            assert!(columns[0].untouched());
        }
    }
}

#[test]
fn a_valid_declaration_reads() {
    let declared = Declared::of(&at_most("10"));
    let specs = [declared.on(spec(Door::I32))];
    let mut columns = [Column::new(2, true)];
    let (code, out) = delimited(b"1\n2\n", &specs, &mut columns, &[], 2);
    assert_eq!((code, out.rows), (OK, 2));
    let text = Declared::of(
        &Constraints::text(TextRules::new().max_length(3), None).expect("it declares"),
    );
    let specs = [text.on(spec(Door::Text))];
    let mut columns = [Column::new(2, true)];
    let (code, out) = workbook(&sheet_of(&["a", "b"]), &specs, &mut columns, 2);
    assert_eq!((code, out.rows), (OK, 2));
}

fn text_rules(rules: TextRules, entries: Option<&'static [&'static str]>) -> Declared {
    Declared::of(&Constraints::text(rules, entries).expect("it declares"))
}

/// One text column of `data` under `declared`, `rows` rows of it.
fn text_column(data: &[u8], declared: &Declared, rows: usize) -> Column {
    let specs = [declared.on(spec(Door::Text))];
    let mut columns = [Column::new(rows, true)];
    let (code, out) = delimited(data, &specs, &mut columns, &[], rows);
    assert_eq!((code, out.rows), (OK, rows as u64), "{data:?}");
    let [column] = columns;
    column
}

fn violation(offset: u32, len: u32) -> CellVerdict {
    CellVerdict {
        offset,
        len,
        reason: CellVerdict::VIOLATION,
    }
}

fn broke(keyword: Keyword) -> RawCheck {
    RawCheck {
        keyword: keyword.code(),
        adjusted: 0,
        enum_index: u32::MAX,
    }
}

fn passed(adjusted: bool, enum_index: Option<u32>) -> RawCheck {
    RawCheck {
        keyword: 0,
        adjusted: u32::from(adjusted),
        enum_index: enum_index.unwrap_or(u32::MAX),
    }
}

const CELL: CellNote = CellNote {
    detail: 0,
    origin: CellNote::ORIGIN_CELL,
};

#[test]
fn a_violation_keeps_the_value_as_cast() {
    let declared = Declared::of(&at_most("10"));
    let specs = [declared.on(spec(Door::I32))];
    let mut columns = [Column::new(2, true)];
    let (code, out) = delimited(b"11\n7\n", &specs, &mut columns, &[], 2);
    assert_eq!((code, out.rows), (OK, 2));
    let column = &columns[0];
    assert_eq!(column.value::<i32>(0), 11);
    assert_eq!(column.verdicts[0], violation(0, 2));
    assert_eq!(column.notes[0], CELL);
    assert_eq!(column.checks[0], broke(Keyword::Maximum));
    assert_eq!(column.value::<i32>(1), 7);
    assert_eq!(column.verdicts[1], CellVerdict::OK);
    assert_eq!(column.checks[1], passed(false, None));
}

#[test]
fn multiple_of_rounding_flags_adjusted() {
    let declared = Declared::of(
        &Constraints::<f64>::numeric(
            NumericRules::new().multiple_of(decimal("0.5"), Rounding::Midpoint),
            None,
        )
        .expect("it declares"),
    );
    let specs = [declared.on(spec(Door::F64))];
    let mut columns = [Column::new(2, true)];
    let (code, _) = delimited(b"1.3\n2.5\n", &specs, &mut columns, &[], 2);
    assert_eq!(code, OK);
    let column = &columns[0];
    assert_eq!(column.value::<f64>(0), 1.5);
    assert_eq!(column.verdicts[0], CellVerdict::OK);
    assert_eq!(column.checks[0], passed(true, None));
    assert_eq!(column.value::<f64>(1), 2.5);
    assert_eq!(column.checks[1], passed(false, None));
}

#[test]
fn a_fault_writes_no_check() {
    let declared = Declared::of(&at_most("10"));
    let specs = [declared.on(spec(Door::I32))];
    let mut columns = [Column::new(1, true)];
    let (code, _) = delimited(b"x\n", &specs, &mut columns, &[], 1);
    assert_eq!(code, OK);
    let column = &columns[0];
    assert_eq!(column.verdicts[0].reason, 2);
    assert_eq!(column.checks[0], JUNK_CHECK);
}

#[test]
fn a_trimmed_text_spans_the_judged_text() {
    let declared = text_rules(TextRules::new().trim(true).max_length(1), None);
    let column = text_column(b"x,  ab \n", &declared, 1);
    // The second cell starts at byte 2; its judged text, `ab`, two bytes further.
    let specs = [ColumnSpecWith::new(1, Door::Text.code(), 0)];
    let specs = [declared.on(specs[0])];
    let mut columns = [Column::new(1, true)];
    let (code, _) = delimited(b"x,  ab \n", &specs, &mut columns, &[], 1);
    assert_eq!(code, OK);
    let column_at_1 = &columns[0];
    assert_eq!(column_at_1.verdicts[0], violation(2, 2));
    assert_eq!(column_at_1.value::<Span>(0), Span { offset: 4, len: 2 });
    assert_eq!(column_at_1.checks[0], broke(Keyword::MaxLength));
    // The first column, `x`, passes.
    assert_eq!(column.value::<Span>(0), Span { offset: 0, len: 1 });
    assert_eq!(column.checks[0], passed(false, None));
}

#[test]
fn a_quoted_cell_in_the_arena_maps_its_span() {
    let matched = text_rules(TextRules::new().trim(true), Some(&["a\"b"]));
    let column = text_column(b"\"  a\"\"b  \"\n", &matched, 1);
    assert_eq!(column.verdicts[0], CellVerdict::OK);
    assert_eq!(column.checks[0], passed(false, Some(0)));
    assert_eq!(
        column.value::<Span>(0),
        Span {
            offset: 2,
            len: 3 | Span::FLAG
        }
    );
    let short = text_rules(TextRules::new().trim(true).max_length(1), None);
    let column = text_column(b"\"  a\"\"b  \"\n", &short, 1);
    assert_eq!(column.verdicts[0], violation(2, 3));
    assert_eq!(column.checks[0], broke(Keyword::MaxLength));
    assert_eq!(
        column.value::<Span>(0),
        Span {
            offset: 2,
            len: 3 | Span::FLAG
        }
    );
}

#[test]
fn tokens_and_padding_stay_empty_under_constraints() {
    let declared = text_rules(TextRules::new().min_length(3), None);
    let tokens = [&2u32.to_le_bytes()[..], b"NA"].concat();
    let specs = [
        declared.on(ColumnSpecWith::new(0, Door::Text.code(), 0)),
        declared.on(ColumnSpecWith::new(1, Door::Text.code(), 0)),
    ];
    let mut columns = [Column::new(2, true), Column::new(2, true)];
    let (code, out) = delimited(b"NA,abc\nabc\n", &specs, &mut columns, &tokens, 2);
    assert_eq!((code, out.rows), (OK, 2));
    let empty = CellVerdict {
        offset: 0,
        len: 0,
        reason: 1,
    };
    assert_eq!(columns[0].verdicts[0], empty);
    assert_eq!(columns[0].notes[0].origin, CellNote::ORIGIN_TOKEN);
    assert_eq!(columns[0].checks[0], JUNK_CHECK);
    assert_eq!(columns[1].verdicts[1], empty);
    assert_eq!(columns[1].notes[1].origin, CellNote::ORIGIN_PADDED);
    assert_eq!(columns[1].checks[1], JUNK_CHECK);
    assert_eq!(columns[0].checks[1], passed(false, None));
    assert_eq!(columns[1].checks[0], passed(false, None));
}

#[test]
fn a_number_in_a_text_enum_never_matches() {
    let declared = text_rules(TextRules::new(), Some(&["01", "1.0"]));
    let column = text_column(b"1\n1.0\n", &declared, 2);
    assert_eq!(column.verdicts[0], violation(0, 1));
    assert_eq!(column.checks[0], broke(Keyword::Enum));
    assert_eq!(column.checks[1], passed(false, Some(1)));
}

#[test]
fn case_folding_unless_case_sensitive() {
    let folded = text_rules(TextRules::new(), Some(&["Yes"]));
    let column = text_column(b"yES\n", &folded, 1);
    assert_eq!(column.checks[0], passed(false, Some(0)));
    assert_eq!(column.value::<Span>(0), Span { offset: 0, len: 3 });
    let exact = text_rules(TextRules::new().case_sensitive(true), Some(&["Yes"]));
    let column = text_column(b"yES\n", &exact, 1);
    assert_eq!(column.verdicts[0], violation(0, 3));
    assert_eq!(column.checks[0], broke(Keyword::Enum));
}

#[test]
fn duplicate_enum_entries_first_wins() {
    let declared = text_rules(TextRules::new(), Some(&["b", "A", "a"]));
    let column = text_column(b"a\nA\n", &declared, 2);
    assert_eq!(column.checks[0], passed(false, Some(1)));
    assert_eq!(column.checks[1], passed(false, Some(1)));
}

#[test]
fn two_columns_over_one_ordinal_keep_their_own_verdicts() {
    let (ten, twenty) = (Declared::of(&at_most("10")), Declared::of(&at_most("20")));
    let specs = [ten.on(spec(Door::I32)), twenty.on(spec(Door::I32))];
    let mut columns = [Column::new(1, true), Column::new(1, true)];
    let (code, _) = delimited(b"15\n", &specs, &mut columns, &[], 1);
    assert_eq!(code, OK);
    assert_eq!(columns[0].verdicts[0], violation(0, 2));
    assert_eq!(columns[0].checks[0], broke(Keyword::Maximum));
    assert_eq!(columns[1].verdicts[0], CellVerdict::OK);
    assert_eq!(columns[1].checks[0], passed(false, None));
    assert_eq!(columns[0].value::<i32>(0), 15);
    assert_eq!(columns[1].value::<i32>(0), 15);
}

#[test]
fn malformed_text_is_a_fault_under_constraints() {
    let declared = text_rules(TextRules::new().max_length(9), None);
    let column = text_column(b"ok\n\xff\n", &declared, 2);
    assert_eq!(column.checks[0], passed(false, None));
    assert_eq!(column.verdicts[1].reason, 2);
    assert_eq!(column.checks[1], JUNK_CHECK);
}

/// One column of `cells` through a workbook fill under `declared` at `door`.
fn workbook_column(cells: &[&str], declared: &Declared, door: Door) -> Column {
    let specs = [declared.on(spec(door))];
    let mut columns = [Column::new(cells.len(), true)];
    let (code, out) = workbook(&sheet_with(cells), &specs, &mut columns, cells.len());
    assert_eq!((code, out.rows), (OK, cells.len() as u64), "{cells:?}");
    let [column] = columns;
    column
}

fn at_least_f64(text: &str) -> Declared {
    Declared::of(
        &Constraints::<f64>::numeric(NumericRules::new().minimum(decimal(text)), None)
            .expect("it declares"),
    )
}

#[test]
fn a_typed_negative_zero_is_positive_zero() {
    let column = workbook_column(&["<c><v>-0</v></c>"], &at_least_f64("0"), Door::F64);
    assert_eq!(column.verdicts[0], CellVerdict::OK);
    assert_eq!(column.value::<f64>(0).to_bits(), 0);
    assert_eq!(column.checks[0], passed(false, None));
}

#[test]
fn a_typed_sum_is_judged_on_its_shortest_digits() {
    let declared = Declared::of(
        &Constraints::<f64>::numeric(NumericRules::new().maximum(decimal("0.3")), None)
            .expect("it declares"),
    );
    let column = workbook_column(
        &["<c><v>0.30000000000000004</v></c>", "<c><v>0.3</v></c>"],
        &declared,
        Door::F64,
    );
    assert_eq!(column.verdicts[0], violation(0, 0));
    assert_eq!(column.value::<f64>(0), 0.1 + 0.2);
    assert_eq!(column.checks[0], broke(Keyword::Maximum));
    assert_eq!(column.notes[0], CELL);
    assert_eq!(column.checks[1], passed(false, None));
}

#[test]
fn a_typed_integer_is_judged_through_its_twin() {
    let column = workbook_column(
        &["<c><v>11</v></c>", "<c><v>7</v></c>"],
        &Declared::of(&at_most("10")),
        Door::I32,
    );
    assert_eq!(column.verdicts[0], violation(0, 0));
    assert_eq!(column.value::<i32>(0), 11);
    assert_eq!(column.checks[0], broke(Keyword::Maximum));
    assert_eq!(column.value::<i32>(1), 7);
    assert_eq!(column.checks[1], passed(false, None));
}

#[test]
fn a_text_cell_is_judged_with_its_span() {
    let column = workbook_column(
        &["<c t=\"inlineStr\"><is><t> 11</t></is></c>"],
        &Declared::of(&at_most("10")),
        Door::I32,
    );
    assert_eq!(column.verdicts[0], violation(1, 2));
    assert_eq!(column.value::<i32>(0), 11);
}

#[test]
fn a_typed_number_at_the_text_door_matches_its_text() {
    let declared = text_rules(TextRules::new(), Some(&["42"]));
    let column = workbook_column(
        &["<c><v>42</v></c>", "<c><v>7</v></c>"],
        &declared,
        Door::Text,
    );
    assert_eq!(column.verdicts[0], CellVerdict::OK);
    assert_eq!(column.checks[0], passed(false, Some(0)));
    let span = column.value::<Span>(0);
    assert!(span.flagged(), "the rendering is in the arena");
    assert_eq!(span.len(), 2);
    assert_eq!(column.verdicts[1], violation(0, 1));
    assert_eq!(column.checks[1], broke(Keyword::Enum));
}

#[test]
fn a_trimmed_text_cell_spans_the_judged_text() {
    let declared = text_rules(TextRules::new().trim(true).max_length(1), None);
    let column = workbook_column(
        &["<c t=\"inlineStr\"><is><t xml:space=\"preserve\">  ab </t></is></c>"],
        &declared,
        Door::Text,
    );
    assert_eq!(column.verdicts[0], violation(2, 2));
    assert_eq!(column.value::<Span>(0).len(), 2);
    assert_eq!(column.checks[0], broke(Keyword::MaxLength));
}

#[test]
fn an_uncached_formula_stays_empty() {
    let column = workbook_column(
        &["<c><f>A2</f></c>"],
        &Declared::of(&at_most("10")),
        Door::I32,
    );
    assert_eq!(column.verdicts[0].reason, 1);
    assert_eq!(column.notes[0].origin, CellNote::ORIGIN_UNCACHED);
    assert_eq!(column.checks[0], JUNK_CHECK);
}

#[test]
fn a_boolean_cell_at_a_numeric_door_keeps_its_note() {
    let checked = workbook_column(
        &["<c t=\"b\"><v>1</v></c>"],
        &Declared::of(&at_most("10")),
        Door::I32,
    );
    let specs = [spec(Door::I32)];
    let mut columns = [Column::new(1, false)];
    let (code, _) = workbook(
        &sheet_with(&["<c t=\"b\"><v>1</v></c>"]),
        &specs,
        &mut columns,
        1,
    );
    assert_eq!(code, OK);
    assert_eq!(checked.verdicts[0], columns[0].verdicts[0]);
    assert_eq!(checked.notes[0], columns[0].notes[0]);
    assert_eq!(checked.verdicts[0].reason, 2);
    assert_eq!(checked.checks[0], JUNK_CHECK);
}

#[test]
fn a_blank_cell_is_empty_with_no_check() {
    let column = workbook_column(
        &["<c t=\"inlineStr\"><is><t></t></is></c>"],
        &Declared::of(&at_most("10")),
        Door::I32,
    );
    assert_eq!(column.verdicts[0].reason, 1);
    assert_eq!(column.checks[0], JUNK_CHECK);
}

/// Every entry `hypertabular_faults` lists for one column of `rows` rows.
fn listed(column: &mut Column, rows: usize) -> Vec<RawProblem> {
    let buffers = [column.buffer()];
    let mut out = vec![RawProblem::default(); 8];
    let (mut cursor, mut written) = (0u64, 0usize);
    // SAFETY: the column holds `rows` of everything, and `out` its length.
    let code = unsafe {
        hypertabular_faults(
            buffers.as_ptr(),
            1,
            rows,
            core::ptr::null(),
            0,
            RawDelimitedOptions::LONG_ROWS_REJECT,
            &mut cursor,
            out.as_mut_ptr(),
            out.len(),
            &mut written,
        )
    };
    assert_eq!(code, OK);
    out.truncate(written);
    out
}

#[test]
fn faults_list_a_violation_with_its_keyword() {
    let declared = Declared::of(&at_most("10"));
    let specs = [declared.on(spec(Door::I32))];
    let mut columns = [Column::new(3, true)];
    let (code, _) = delimited(b"11\nx\n7\n", &specs, &mut columns, &[], 3);
    assert_eq!(code, OK);
    let listed = listed(&mut columns[0], 3);
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(
        listed[0],
        RawProblem {
            row: 0,
            plan_column: 0,
            reason: CellVerdict::VIOLATION,
            detail: 0,
            origin: CellNote::ORIGIN_CELL,
            offset: 0,
            len: 2,
            expected: 0,
            found: 0,
            keyword: Keyword::Maximum.code(),
        }
    );
    assert_eq!(
        (listed[1].row, listed[1].reason, listed[1].keyword),
        (1, 2, 0)
    );
}

#[test]
fn a_column_without_checks_lists_no_violation() {
    let declared = Declared::of(&at_most("10"));
    let specs = [declared.on(spec(Door::I32))];
    let mut columns = [Column::new(3, true)];
    let (code, _) = delimited(b"11\nx\n7\n", &specs, &mut columns, &[], 3);
    assert_eq!(code, OK);
    // The same batch, its `checks` not passed: there is no keyword to name.
    columns[0].checked = false;
    let listed = listed(&mut columns[0], 3);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!((listed[0].row, listed[0].reason), (1, 2));
}
