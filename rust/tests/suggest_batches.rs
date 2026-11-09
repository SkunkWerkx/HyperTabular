//! A sample doesn't depend on how many batches it travels in, and a sampler's batch is never why a
//! suggestion is refused. The sizes here are refused or chosen before anything is allocated, and a
//! table that's allowed is a lazily zeroed one, so a regression fails on the answer.

use super::packages;
use crate::suggest::{Sample, Suggestion};
use crate::{
    Column, DelimitedOptions, DelimitedReader, Dialect, Header, SheetOptions, SuggestOptions,
    Workbook,
};

/// A delimited suggestion read in batches of `rows`.
fn delimited_in(text: &[u8], dialect: Dialect, sample: usize, rows: usize) -> Suggestion {
    let options = SuggestOptions::new().sample_rows(sample);
    let mut reader = DelimitedOptions::new()
        .notes(false)
        .batch_rows(rows)
        .from_slice_unbound(text, dialect)
        .unwrap();
    let width = reader.header().map_or(0, Header::len);
    let plan: Vec<Column> = (0..width).map(Column::text).collect();
    reader.bind(&plan).unwrap();
    let mut taken = Sample::new(width, sample);
    taken.read_from(&mut reader).unwrap();
    taken.judge(reader.header(), &options).unwrap()
}

/// A sheet's suggestion read in batches of `rows`.
fn sheet_in(book: &Workbook<'_>, sample: usize, rows: usize) -> Option<Suggestion> {
    let options = SuggestOptions::new().sample_rows(sample);
    let read = SheetOptions::new().with_notes(true).with_batch_rows(rows);
    let mut sheet = book.sheet_unbound(0usize, read).ok()?;
    let width = sheet.header().map_or(0, Header::len);
    if width == 0 {
        return None;
    }
    sheet.bind(&Sample::sheet_plan(width)).unwrap();
    let mut taken = Sample::of_sheet(width, sample);
    taken.read_from(&mut sheet).ok()?;
    Some(taken.judge(sheet.header(), &options).unwrap())
}

const MESSY: &[u8] = b"id,price,when,flag,note\n\
    1,\"1,234.50\",2024-01-02,true,plain\n\
    2,1.234,2024-02-03,false,\n\
    3,,2024-03-04,true,\"with, comma\"\n\
    x,0.5,03/04/2024,maybe,\"two\nlines\"\n\
    5,12,2024-05-06,false,more\n\
    6,13,2024-06-07,true,more\n\
    7,14,2024-07-08,,more\n\
    8,15,,false,more\n\
    9,16,2024-09-10,true,last\n";

#[test]
fn a_delimited_sample_says_the_same_in_any_number_of_batches() {
    for sample in [1, 2, 3, 5, 8, 9, 10, 1000] {
        let whole = delimited_in(MESSY, Dialect::CSV, sample, sample + 1);
        // What the public call says, with whatever batch the core chose.
        let public = DelimitedReader::suggest_slice(
            MESSY,
            Dialect::CSV,
            &SuggestOptions::new().sample_rows(sample),
        )
        .unwrap();
        assert_eq!(public, whole, "sample {sample}");
        for rows in [1, 2, 3, 7, 11] {
            assert_eq!(
                delimited_in(MESSY, Dialect::CSV, sample, rows),
                whole,
                "sample {sample}, batches of {rows}"
            );
        }
    }
    // A control: the sample's size does move the answer, so the equality above can fail.
    assert_ne!(
        delimited_in(MESSY, Dialect::CSV, 1, 2),
        delimited_in(MESSY, Dialect::CSV, 1000, 1001)
    );
}

#[test]
fn a_sheets_sample_says_the_same_in_any_number_of_batches() {
    let mut compared = 0;
    let generated = [
        ("generated", packages::xlsx_generated(7, 40, 6, true)),
        (
            "generated wide",
            packages::xlsx_generated(11, 12, 40, false),
        ),
        ("suggest", packages::suggest_xlsx()),
        ("suggest ods", packages::suggest_ods()),
    ];
    for (name, package) in generated {
        let book = Workbook::from_vec(package).unwrap();
        for sample in [1, 2, 5, 13, 1000] {
            let whole = sheet_in(&book, sample, sample + 1).expect(name);
            for rows in [1, 2, 3, 7] {
                let found = sheet_in(&book, sample, rows).expect(name);
                assert_eq!(found, whole, "{name}, sample {sample}, batches of {rows}");
                compared += 1;
            }
            // What the public call says, with whatever batch the core chose.
            let public = book
                .sheet_unbound(0usize, SheetOptions::new().with_batch_rows(sample.max(1)))
                .unwrap()
                .suggest(&SuggestOptions::new().sample_rows(sample))
                .unwrap();
            assert_eq!(public, whole, "{name}, sample {sample}");
        }
    }
    assert_eq!(compared, 80);
}

fn wide_csv(width: usize) -> Vec<u8> {
    let names: Vec<String> = (0..width).map(|column| format!("c{column}")).collect();
    let row = vec!["1"; width].join(",");
    format!("{}\n{row}\n{row}\n", names.join(",")).into_bytes()
}

#[test]
fn a_wide_source_is_sampled_at_defaults() {
    for width in [268_000, 300_000, 1 << 20] {
        let found =
            DelimitedReader::suggest_slice(&wide_csv(width), Dialect::CSV, &SuggestOptions::new())
                .unwrap();
        assert_eq!(found.report.len(), width);
        assert_eq!(found.report[width - 1].sampled, 2);
        assert!(!found.truncated);
    }
}

#[test]
fn a_wide_sheet_is_sampled_at_a_large_sample() {
    // No reference on a cell: each takes the next column.
    let cells = |text: &str| {
        let cell = format!(r#"<c t="inlineStr"><is><t>{text}</t></is></c>"#);
        format!("<row>{}</row>", cell.repeat(700))
    };
    let sheet_xml = format!("{}{}{}", cells("name"), cells("1"), cells("2"));
    let package = packages::xlsx(&[("Wide", &sheet_xml)], "", false, true);
    let book = Workbook::from_vec(package).unwrap();
    let sheet = book
        .sheet(
            0usize,
            SheetOptions::new().with_batch_rows(100_000),
            &[Column::i32(0)],
        )
        .unwrap();
    let found = sheet
        .suggest(&SuggestOptions::new().sample_rows(100_000))
        .unwrap();
    assert_eq!(found.report.len(), 700);
    assert_eq!(found, sheet.suggest(&SuggestOptions::new()).unwrap());
    // At exactly the limit, with no plan, and a sample past every batch.
    let at_the_limit = book
        .sheet_unbound(0usize, SheetOptions::new().with_batch_rows(1 << 29))
        .unwrap();
    assert_eq!(
        at_the_limit
            .suggest(&SuggestOptions::new().sample_rows(usize::MAX / 2))
            .unwrap(),
        found
    );
}
