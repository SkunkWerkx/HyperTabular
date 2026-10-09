//! The reader as a caller who knows its columns by name meets it: the header read before
//! any plan, the plan bound once, then read — and a batch read across, a row at a time.
//! The corpus replays (corpus_delimited.rs, corpus_workbook.rs) read every case header
//! first as well; this file holds the promises around that path.

use hypertabular::{Column, DelimitedReader, Dialect, Error, SheetOptions, Workbook};
use std::borrow::Cow;

const TEXT: &[u8] = b"Region Code,Region Name,M49 Code,Region Code\n\
002,Africa,4,dup\n\
019,Americas,8,dup\n\
142,Asia,12,dup\n\
150,Europe,20,dup\n\
009,Oceania,24,dup\n";

#[test]
fn a_plan_is_built_from_the_header_and_bound_once() {
    let mut reader = DelimitedReader::from_slice_unbound(TEXT, Dialect::CSV).unwrap();
    assert!(!reader.is_bound());
    assert!(reader.plan().is_empty());
    assert_eq!(reader.column_count(), Some(4));
    assert!(matches!(reader.read(), Err(Error::Unbound)));
    // Still unbound, and reading still unbound is not a failure that sticks.
    assert!(matches!(reader.read(), Err(Error::Unbound)));

    let header = reader.header().unwrap();
    // The first of two columns with one name; a `&str` and bytes find the same.
    assert_eq!(header.ordinal("Region Code"), Some(0));
    assert_eq!(header.ordinal(b"M49 Code"), Some(2));
    assert_eq!(header.ordinal("m49 code"), None, "exact, case included");
    assert_eq!(header.ordinal(" M49 Code"), None, "exact, spaces included");
    assert!(matches!(
        header.require("Country"),
        Err(Error::NoColumn(name)) if name == "Country"
    ));
    let plan = [
        Column::i32(header.require("M49 Code").unwrap()),
        Column::text(header.require("Region Name").unwrap()),
    ];
    reader.bind(&plan).unwrap();
    assert!(reader.is_bound());
    assert_eq!(reader.plan(), &plan);
    assert!(matches!(reader.bind(&plan), Err(Error::AlreadyBound)));

    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.i32(0), &[4, 8, 12, 20, 24]);
    assert_eq!(batch.text(1, 4), Ok(&b"Oceania"[..]));
    assert!(reader.read().unwrap().is_none());
    assert!(matches!(reader.bind(&plan), Err(Error::AlreadyBound)));
}

#[test]
fn a_headerless_source_is_bound_by_position() {
    let dialect = Dialect::CSV.with_header(false);
    let mut reader = DelimitedReader::from_slice_unbound(b"1,a\n2,b\n", dialect).unwrap();
    assert!(reader.header().is_none());
    assert_eq!(reader.column_count(), None, "no record has been read");
    reader.bind(&[Column::text(1), Column::i64(0)]).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.i64(1), &[1, 2]);
    assert_eq!(reader.column_count(), Some(2));
}

#[test]
fn a_plan_that_cannot_be_honoured_is_refused_at_bind() {
    let mut reader = DelimitedReader::from_slice_unbound(TEXT, Dialect::CSV).unwrap();
    let wide = Column::text(usize::MAX);
    assert!(matches!(
        reader.bind(&[wide]),
        Err(Error::Plan { column: 0 })
    ));
    // A refused plan binds nothing: a good one still can.
    assert!(!reader.is_bound());
    reader.bind(&[Column::text(0)]).unwrap();
}

#[test]
fn a_sheet_is_opened_header_first() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook/basic.xlsx");
    let book = Workbook::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    let options = SheetOptions::default();
    let mut sheet = book.sheet_unbound(0, options).unwrap();
    assert!(matches!(sheet.read(), Err(Error::Unbound)));
    let header = sheet.header().expect("a header row").clone();
    assert!(!header.is_empty());
    let first = header.name(0).unwrap().to_vec();
    sheet
        .bind(&[Column::text(header.require(&first).unwrap())])
        .unwrap();
    assert!(matches!(
        sheet.bind(&[Column::text(0)]),
        Err(Error::AlreadyBound)
    ));
    let unbound: Vec<Vec<u8>> = rows_of(&mut sheet);

    let mut planned = book.sheet(0, options, &[Column::text(0)]).unwrap();
    assert_eq!(planned.header(), Some(&header));
    assert_eq!(rows_of(&mut planned), unbound);
}

fn rows_of(sheet: &mut hypertabular::Sheet<'_>) -> Vec<Vec<u8>> {
    let mut rows = Vec::new();
    sheet
        .for_each_row(|row| {
            rows.push(row.raw(0).into_owned());
            Ok::<(), Error>(())
        })
        .unwrap();
    rows
}

#[test]
fn a_row_is_its_batch_read_across() {
    let plan = [
        Column::i32(2),
        Column::text(1),
        Column::i32(1),
        Column::text(3),
    ];
    let mut reader = DelimitedReader::from_slice(TEXT, Dialect::CSV, &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert_eq!(batch.iter().len(), batch.rows());
    let mut seen = 0;
    for (index, row) in batch.into_iter().enumerate() {
        assert_eq!(row.index(), index);
        assert_eq!(row.line(), batch.line(index));
        assert_eq!(row.get::<i32>(0), batch.get::<i32>(0, index));
        assert_eq!(row.get::<i32>(2), batch.get::<i32>(2, index));
        assert_eq!(row.verdict(2), batch.verdicts(2)[index]);
        assert_eq!(row.text(1), batch.text(1, index));
        assert_eq!(row.text_str(1), batch.text_str(1, index));
        assert_eq!(row.raw(2), batch.raw(2, index));
        seen += 1;
    }
    assert_eq!(seen, 5);
    let last = batch.iter().next_back().unwrap();
    assert_eq!(last.index(), 4);
    assert_eq!(batch.row(1).text_str(1).unwrap(), "Americas");
}

#[test]
fn text_as_str_borrows_and_replaces_only_what_is_not_utf8() {
    let plan = [Column::text(0)];
    let dialect = Dialect::CSV.with_header(false);
    let text = b"caf\xC3\xA9\n\"say \"\"hi\"\"\"\nbad\xFF\n";
    let mut reader = DelimitedReader::from_slice(text, dialect, &plan).unwrap();
    let batch = reader.read().unwrap().unwrap();
    assert!(matches!(batch.text_str(0, 0), Ok(Cow::Borrowed("café"))));
    assert_eq!(batch.text_str(0, 1).unwrap(), "say \"hi\"");
    assert!(matches!(batch.text_str(0, 2), Ok(Cow::Owned(ref s)) if s == "bad\u{FFFD}"));
}

#[test]
fn rows_run_across_batches() {
    let mut text = b"n\n".to_vec();
    for n in 0..10 {
        text.extend_from_slice(format!("{n}\n").as_bytes());
    }
    let options = DelimitedReader::options().batch_rows(3);
    let mut reader = options.from_slice_unbound(&text, Dialect::CSV).unwrap();
    let ordinal = reader.header().unwrap().require("n").unwrap();
    reader.bind(&[Column::i32(ordinal)]).unwrap();
    let mut seen = Vec::new();
    reader
        .for_each_row(|row| {
            seen.push((row.index(), row.get::<i32>(0).unwrap()));
            Ok::<(), Error>(())
        })
        .unwrap();
    let want: Vec<(usize, i32)> = (0..10).map(|n| (n as usize % 3, n)).collect();
    assert_eq!(
        seen, want,
        "four batches: three of three rows and one of one"
    );

    // The caller's own error stops it, and is the caller's.
    let mut reader = DelimitedReader::from_slice(&text, Dialect::CSV, &[Column::i32(0)]).unwrap();
    let stopped: Result<(), Box<dyn std::error::Error>> =
        reader.for_each_row(|row| match row.get::<i32>(0) {
            Ok(4) => Err("four".into()),
            _ => Ok(()),
        });
    assert_eq!(stopped.unwrap_err().to_string(), "four");
}

#[cfg(feature = "async")]
mod asynchronous {
    use super::TEXT;
    use futures_io::AsyncRead;
    use hypertabular::{Column, DelimitedReader, Dialect, Error, Workbook};
    use std::future::Future;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    /// Polls `future` to its end on this thread, every wake ignored: what a runtime would
    /// do, minus waiting, since every source here is ready on its second poll.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    /// A source that answers every other poll with `Pending`, and the rest with at most
    /// `most` bytes: a slow network, a short read at a time.
    struct Trickle<'a> {
        bytes: &'a [u8],
        most: usize,
        ready: bool,
    }

    impl AsyncRead for Trickle<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.ready = !self.ready;
            if !self.ready {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let n = self.bytes.len().min(buf.len()).min(self.most);
            buf[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Poll::Ready(Ok(n))
        }
    }

    fn trickle(bytes: &[u8], most: usize) -> Trickle<'_> {
        Trickle {
            bytes,
            most,
            ready: false,
        }
    }

    #[test]
    fn an_async_read_is_the_same_read() {
        let plan = [Column::i32(2), Column::text(1)];
        let mut want = Vec::new();
        let mut reader = DelimitedReader::from_slice(TEXT, Dialect::CSV, &plan).unwrap();
        reader
            .for_each_row(|row| {
                want.push((row.get::<i32>(0), row.text(1).map(<[u8]>::to_vec)));
                Ok::<(), Error>(())
            })
            .unwrap();
        for (most, buffer_bytes, batch_rows) in [(1, 1, 1), (3, 4, 2), (5, 64, 1024)] {
            let got = block_on(async {
                let options = DelimitedReader::options()
                    .buffer_bytes(buffer_bytes)
                    .batch_rows(batch_rows);
                let mut reader = options
                    .from_async_reader(trickle(TEXT, most), Dialect::CSV)
                    .await
                    .unwrap();
                assert!(
                    matches!(reader.read(), Err(Error::Io(_))),
                    "awaited, not read"
                );
                let header = reader.header().unwrap();
                let plan = [
                    Column::i32(header.require("M49 Code").unwrap()),
                    Column::text(header.require("Region Name").unwrap()),
                ];
                reader.bind(&plan).unwrap();
                let mut got = Vec::new();
                while let Some(batch) = reader.read_async().await.unwrap() {
                    for row in batch {
                        got.push((row.get::<i32>(0), row.text(1).map(<[u8]>::to_vec)));
                    }
                }
                got
            });
            assert_eq!(got, want, "{most} bytes a read, {buffer_bytes}-byte buffer");
        }
    }

    #[test]
    fn a_read_abandoned_part_way_loses_nothing() {
        let plan = [Column::i32(2)];
        let got = block_on(async {
            let options = DelimitedReader::options().buffer_bytes(2);
            let mut reader = options
                .from_async_reader(trickle(TEXT, 2), Dialect::CSV)
                .await
                .unwrap();
            reader.bind(&plan).unwrap();
            // Polled once and dropped, again and again: each time mid-refill.
            for _ in 0..20 {
                let mut cx = Context::from_waker(Waker::noop());
                let read = std::pin::pin!(reader.read_async());
                let _ = read.poll(&mut cx).is_pending();
            }
            let mut got = Vec::new();
            while let Some(batch) = reader.read_async().await.unwrap() {
                got.extend_from_slice(batch.i32(0));
            }
            got
        });
        // The dropped reads may have delivered a batch each; what is left is the rest, in
        // order, nothing lost and nothing twice.
        assert!(
            [4, 8, 12, 20, 24].ends_with(&got) && !got.is_empty(),
            "{got:?}"
        );
    }

    #[test]
    fn a_workbook_is_awaited_whole() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/workbook/basic.xlsx");
        let bytes = std::fs::read(path).unwrap();
        let book = block_on(Workbook::from_async_reader(trickle(&bytes, 977))).unwrap();
        let direct = Workbook::from_slice(&bytes).unwrap();
        assert_eq!(book.sheets(), direct.sheets());
    }
}
