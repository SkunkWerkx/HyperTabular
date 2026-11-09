//! A column's grouping through the Rust API: Western unless the caller declares Indian on that
//! column, in the same read as a column that doesn't, and a stale policy word refused where it's
//! handed over, before any cell. What the shapes read is HyperCast's and the corpus's.

use hypertabular::{
    Column, DelimitedOptions, DelimitedReader, Dialect, Door, Error, Grouping, NumFormat, Reason,
    SheetOptions, SuggestOptions, SuggestStatus, Workbook,
};
use std::path::Path;

#[path = "support/packages.rs"]
mod packages;

const INDIAN: NumFormat = NumFormat::INVARIANT.with_grouping(Grouping::Indian);
const BOTH: &[u8] = b"indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";

/// `NumFormat::new` takes a plain word without a word, the way a format stored by 1.0's HyperCast or
/// an old program's literal arrives.
fn stale(word: u32) -> NumFormat {
    NumFormat::new('.', ',', word)
}

fn planned(format: NumFormat) -> [Column; 2] {
    [Column::i32(0), Column::text(1).format(format)]
}

/// Asserts a refusal names the plan column, and nothing was read to get there.
fn is_plan<T>(result: Result<T, Error>, column: usize, what: &str) {
    match result {
        Err(Error::Plan { column: found }) => assert_eq!(found, column, "{what}"),
        Err(other) => panic!("{what}: {other:?}"),
        Ok(_) => panic!("{what}: it was taken"),
    }
}

#[test]
fn an_indian_column_reads_beside_a_western_one() {
    let plan = [Column::i32(0).format(INDIAN), Column::i32(1)];
    let mut reader = DelimitedReader::from_slice(BOTH, Dialect::CSV, &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.get::<i32>(0, 0), Ok(123456));
    assert_eq!(
        batch.get::<i32>(1, 0).unwrap_err().reason,
        Reason::Malformed
    );
    assert_eq!(
        batch.get::<i32>(0, 1).unwrap_err().reason,
        Reason::Malformed
    );
    assert_eq!(batch.get::<i32>(1, 1), Ok(1234567));
}

#[test]
fn an_indian_sheet_column_reads_beside_a_western_one() {
    for package in [
        packages::grouping_xlsx(),
        packages::grouping_ods(),
        packages::grouping_numbers(),
    ] {
        let book = Workbook::from_slice(&package).unwrap();
        let plan = [Column::i32(2).format(INDIAN), Column::i32(2)];
        let mut sheet = book.sheet(0, SheetOptions::new(), &plan).unwrap();
        let batch = sheet.read().unwrap().unwrap();
        assert_eq!(batch.get::<i32>(0, 0), Ok(123456));
        assert_eq!(
            batch.get::<i32>(1, 0).unwrap_err().reason,
            Reason::Malformed
        );
        assert_eq!(
            batch.get::<i32>(0, 1).unwrap_err().reason,
            Reason::Malformed
        );
        assert_eq!(batch.get::<i32>(1, 1), Ok(1234567));
        // A typed number has no notation: it's the same under either.
        assert_eq!(batch.get::<i32>(0, 2), Ok(1234));
        assert_eq!(batch.get::<i32>(1, 2), Ok(1234));
    }
}

/// Every door a Rust caller hands a plan to, each with a control that the same call takes the same
/// plan without the bit, so the refusal is the word's.
#[test]
fn a_stale_policy_is_refused_at_every_delimited_entry_point() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grouping.csv");
    std::fs::write(&path, BOTH).unwrap();
    let dialect = Dialect::CSV;
    let options = DelimitedOptions::new();
    for (word, refused) in [(351, true), (863, true), (95, false), (607, false)] {
        let plan = planned(stale(word));
        let check = |result: Result<DelimitedReader<'_>, Error>, what: &str| {
            let what = format!("{what}, {word}");
            if refused {
                is_plan(result, 1, &what);
            } else {
                result.unwrap_or_else(|error| panic!("{what}: {error:?}"));
            }
        };
        check(DelimitedReader::open(&path, dialect, &plan), "open");
        check(DelimitedReader::from_slice(BOTH, dialect, &plan), "slice");
        check(DelimitedReader::from_reader(BOTH, dialect, &plan), "reader");
        check(options.open(&path, dialect, &plan), "options open");
        check(options.from_slice(BOTH, dialect, &plan), "options slice");
        check(options.from_reader(BOTH, dialect, &plan), "options reader");

        let mut unbound = DelimitedReader::from_slice_unbound(BOTH, dialect).unwrap();
        let bound = unbound.bind(&plan);
        if refused {
            is_plan(bound, 1, &format!("bind, {word}"));
            // Nothing bound, and no cell read.
            assert!(!unbound.is_bound());
            assert!(matches!(unbound.read(), Err(Error::Unbound)));
        } else {
            bound.unwrap();
        }
    }
}

#[test]
fn a_stale_policy_is_refused_at_every_sheet_entry_point() {
    let package = packages::grouping_xlsx();
    let book = Workbook::from_slice(&package).unwrap();
    for (word, refused) in [(351, true), (863, true), (95, false), (607, false)] {
        let plan = planned(stale(word));
        let sheet = book.sheet(0, SheetOptions::new(), &plan);
        let mut unbound = book.sheet_unbound(0, SheetOptions::new()).unwrap();
        let bound = unbound.bind(&plan);
        if refused {
            is_plan(sheet, 1, &format!("sheet, {word}"));
            is_plan(bound, 1, &format!("bind, {word}"));
            assert!(matches!(unbound.read(), Err(Error::Unbound)));
        } else {
            sheet.unwrap();
            bound.unwrap();
        }
    }
}

#[test]
fn a_stale_policy_is_refused_when_asking_what_to_declare() {
    let package = packages::grouping_xlsx();
    let book = Workbook::from_slice(&package).unwrap();
    let sheet = book.sheet_unbound(0, SheetOptions::new()).unwrap();
    for (word, refused) in [(351, true), (863, true), (95, false), (607, false)] {
        for options in [
            SuggestOptions::new().format(stale(word)),
            SuggestOptions::new().column_format(0, stale(word)),
        ] {
            let delimited = DelimitedReader::suggest_slice(BOTH, Dialect::CSV, &options);
            let sheeted = sheet.suggest(&options);
            if refused {
                is_plan(delimited, 0, &format!("delimited, {word}"));
                is_plan(sheeted, 0, &format!("sheet, {word}"));
            } else {
                delimited.unwrap();
                sheeted.unwrap();
            }
        }
    }
}

/// A suggest judges every format it's given before it opens anything: one never met, one replaced,
/// one past the sample, an empty source and a broken one. A word without the retired bit is the
/// control, which gets as far as the source does.
#[test]
fn a_stale_format_is_refused_before_a_suggest_opens_anything() {
    let good = |word| !matches!(word, 351 | 863);
    let headerless = Dialect::CSV.with_header(false);
    for word in [351, 863, 95, 607] {
        let format = stale(word);
        let every = SuggestOptions::new().format(format);
        // The rows of the review that found these, each named by what it would otherwise do.
        let ask = |bytes: &[u8], dialect: Dialect, options: &SuggestOptions, column, what: &str| {
            let what = format!("{what}, {word}");
            let found = DelimitedReader::suggest_slice(bytes, dialect, options);
            if good(word) {
                found.unwrap_or_else(|error| panic!("{what}: {error:?}"));
            } else {
                is_plan(found, column, &what);
            }
        };
        ask(b"a\n1\n", Dialect::CSV, &every, 0, "a sample");
        ask(b"", Dialect::CSV, &every, 0, "empty");
        ask(b"", headerless, &every, 0, "empty, headerless");
        let past = SuggestOptions::new().column_format(99, format);
        ask(b"a\n1\n", Dialect::CSV, &past, 99, "past the sample");
        let replaced = every.clone().column_format(0, INDIAN);
        ask(
            b"a\n1\n",
            Dialect::CSV,
            &replaced,
            0,
            "replaced for a column",
        );
        let overridden = SuggestOptions::new()
            .column_format(0, format)
            .column_format(0, INDIAN);
        ask(b"a\n1\n", Dialect::CSV, &overridden, 0, "given twice");

        // A broken source says so only of a format that's sound.
        for (bytes, dialect) in [
            (&b"a\n\"1\n"[..], Dialect::CSV),
            (&b"\"1\n"[..], headerless),
        ] {
            let found = DelimitedReader::suggest_slice(bytes, dialect, &every);
            if good(word) {
                assert!(
                    matches!(found, Err(Error::Structure(_))),
                    "{word}: {found:?}"
                );
            } else {
                is_plan(found, 0, &format!("a broken source, {word}"));
            }
        }
        let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-file.csv");
        let found = DelimitedReader::suggest(&missing, Dialect::CSV, &every);
        if good(word) {
            assert!(matches!(found, Err(Error::Io(_))), "{word}: {found:?}");
        } else {
            is_plan(found, 0, &format!("no such file, {word}"));
        }

        // A sheet: the same, and one whose sample would meet its broken part.
        let package = packages::grouping_xlsx();
        let book = Workbook::from_slice(&package).unwrap();
        let sheet = book.sheet_unbound(0, SheetOptions::new()).unwrap();
        for (options, column) in [(&every, 0), (&past, 99), (&replaced, 0), (&overridden, 0)] {
            let found = sheet.suggest(options);
            if good(word) {
                found.unwrap();
            } else {
                is_plan(found, column, &format!("a sheet, {word}"));
            }
        }
        let broken = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../corpus/workbook/broken.xlsx"
        ))
        .unwrap();
        let book = Workbook::from_slice(&broken).unwrap();
        let sheet = book.sheet_unbound(0, SheetOptions::new()).unwrap();
        let found = sheet.suggest(&every);
        if good(word) {
            assert!(
                matches!(found, Err(Error::Structure(_))),
                "{word}: {found:?}"
            );
        } else {
            is_plan(found, 0, &format!("a broken sheet, {word}"));
        }
    }
}

/// What the owner's delimiter rule says stays as it was: nothing says `1,23,456` is Indian.
#[test]
fn an_indian_shape_has_no_culture_until_one_is_declared() {
    let ask = |bytes: &[u8], options: &SuggestOptions| {
        DelimitedReader::suggest_slice(bytes, Dialect::CSV, options).unwrap()
    };
    let none = SuggestOptions::new();
    let found = ask(b"a\n\"1,23,456\"\n", &none);
    assert_eq!(
        (found.report[0].door, found.report[0].status),
        (Door::Text, SuggestStatus::Resolved)
    );
    let found = ask(b"a\n\"1,23,456\"\n5\n", &none);
    assert_eq!(
        (found.report[0].door, found.report[0].status),
        (Door::Text, SuggestStatus::Unresolved)
    );
    // A suggested plan is Western: flags 95, and a later Indian shape is refused.
    let found = ask(b"a\n\"1,234,567\"\n", &none);
    assert_eq!(found.report[0].format, Some(NumFormat::INVARIANT));
    assert_eq!(found.plan[0].format.flags, 95);
    let mut reader =
        DelimitedReader::from_slice(b"a\n\"1,23,456\"\n", Dialect::CSV, &found.plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(
        batch.get::<i32>(0, 0).unwrap_err().reason,
        Reason::Malformed
    );

    // Declared, the column is a number, its plan reads it, and a Western shape is what's refused.
    let declared = SuggestOptions::new().column_format(0, INDIAN);
    let found = ask(b"a,b\n\"1,23,456\",\"1,23,456\"\n", &declared);
    assert_eq!(
        (found.report[0].door, found.report[0].status),
        (Door::I32, SuggestStatus::Resolved)
    );
    assert_eq!(found.report[0].format, Some(INDIAN));
    assert_eq!(found.plan[0].format.flags, 607);
    let mut reader = DelimitedReader::from_slice(
        b"a,b\n\"1,23,456\",\"1,23,456\"\n",
        Dialect::CSV,
        &found.plan,
    )
    .unwrap();
    assert_eq!(reader.read().unwrap().unwrap().get::<i32>(0, 0), Ok(123456));
    // The other column asked nothing declared, and stays text.
    assert_eq!(found.report[1].door, Door::Text);
    let found = ask(b"a\n\"1,234,567\"\n", &declared);
    assert_eq!(
        (found.report[0].door, found.report[0].status),
        (Door::Text, SuggestStatus::Resolved)
    );
    assert_eq!(found.report[0].format, None);
}
