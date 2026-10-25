//! A slice is handed to the core in windows when it is larger than one call takes; reading
//! it in small ones proves the seams lose nothing. A stream is buffered up to a ceiling,
//! which a record that fills without ending is too long; lowering it proves the ceiling is
//! the ceiling whatever buffer the reader began with.

use hypertabular::{Column, DelimitedReader, Dialect, Error, FailureKind};

fn rows(count: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for i in 0..count {
        data.extend_from_slice(
            format!("{i},\"name {i}, with \"\"quotes\"\"\",{i}.5\r\n").as_bytes(),
        );
    }
    data
}

fn collect(reader: &mut DelimitedReader<'_>) -> Result<Vec<(i32, String, f64)>, Error> {
    let mut out = Vec::new();
    while let Some(batch) = reader.read()? {
        for row in batch {
            let text = row.text_str(1).unwrap().into_owned();
            out.push((row.get(0).unwrap(), text, row.get(2).unwrap()));
        }
    }
    Ok(out)
}

const PLAN: [Column; 3] = [Column::i32(0), Column::text(1), Column::f64(2)];

#[test]
fn a_slice_read_in_windows_loses_nothing() {
    let data = rows(500);
    let dialect = Dialect::CSV.with_header(false);
    let options = DelimitedReader::options().batch_rows(64);
    let expected = collect(&mut options.from_slice(&data, dialect, &PLAN).unwrap()).unwrap();
    assert_eq!(expected.len(), 500);
    for window in [64, 100, 4096, data.len() - 14, data.len()] {
        let mut reader = options.from_slice(&data, dialect, &PLAN).unwrap();
        reader.limit_window(window);
        assert_eq!(collect(&mut reader).unwrap(), expected, "window {window}");
    }
}

#[test]
fn a_record_longer_than_a_window_is_too_long_and_not_a_hang() {
    let data = rows(10);
    let mut reader =
        DelimitedReader::from_slice(&data, Dialect::CSV.with_header(false), &PLAN).unwrap();
    reader.limit_window(20);
    let Err(Error::Structure(failure)) = collect(&mut reader) else {
        panic!("a record longer than the window must fail");
    };
    assert_eq!(failure.kind, FailureKind::RowTooLong);
    assert_eq!(failure.record, 0);
}

/// A record that fills the ceiling without ending is too long, and one that ends within it
/// is read, whether the buffer began smaller than the ceiling, at it, or past it; the rows
/// before the record are delivered, and the failure is the reader's last word.
#[test]
fn the_row_ceiling_does_not_depend_on_the_buffer() {
    const CEILING: usize = 8;
    let plan = [Column::text(0)];
    let dialect = Dialect::CSV.with_header(false);
    for initial in [1, 3, 5, CEILING, 12, 4096] {
        for (text, fits) in [
            ("xxxxxxx", true),
            ("xxxxxxx\n", true),
            ("xxxxxxxx", false),
            ("xxxxxxxx\n", false),
            ("xxxxxxxxx", false),
        ] {
            let mut reader = DelimitedReader::options()
                .buffer_bytes(initial)
                .from_reader(std::io::Cursor::new(text.as_bytes()), dialect, &plan)
                .unwrap();
            reader.limit_ceiling(CEILING);
            let read = reader.read().map(|batch| batch.map(|batch| batch.rows()));
            let case = format!("buffer {initial}, {text:?}");
            if fits {
                assert_eq!(read.unwrap(), Some(1), "{case}");
                assert_eq!(
                    reader.read().unwrap().map(|batch| batch.rows()),
                    None,
                    "{case}"
                );
            } else {
                let Err(Error::Structure(failure)) = read else {
                    panic!("{case}: a record that fills the ceiling is too long");
                };
                assert_eq!(failure.kind, FailureKind::RowTooLong, "{case}");
                assert_eq!(failure.record, 0, "{case}");
            }
        }

        let mut reader = DelimitedReader::options()
            .buffer_bytes(initial)
            .from_reader(
                std::io::Cursor::new(b"1\n2\nxxxxxxxxx\n4\n"),
                dialect,
                &plan,
            )
            .unwrap();
        reader.limit_ceiling(CEILING);
        let mut rows = 0;
        let failure = loop {
            match reader.read() {
                Ok(Some(batch)) => rows += batch.rows(),
                Ok(None) => panic!("buffer {initial}: the long record was read"),
                Err(Error::Structure(failure)) => break failure,
                Err(other) => panic!("buffer {initial}: {other:?}"),
            }
        };
        assert_eq!(rows, 2, "buffer {initial}: the rows before are delivered");
        assert_eq!(failure.kind, FailureKind::RowTooLong);
        assert_eq!((failure.record, failure.line, failure.byte), (2, 3, 4));
        for _ in 0..2 {
            let Err(Error::Structure(again)) = reader.read() else {
                panic!("buffer {initial}: the failure is final");
            };
            assert_eq!(again, failure);
        }
    }
}

#[cfg(feature = "async")]
mod asynchronous {
    use futures_io::AsyncRead;
    use hypertabular::{Column, DelimitedReader, Dialect, Error, FailureKind};
    use std::future::Future;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

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
    /// two bytes.
    struct Trickle<'a>(&'a [u8], bool);

    impl AsyncRead for Trickle<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.1 = !self.1;
            if !self.1 {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let n = self.0.len().min(buf.len()).min(2);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Poll::Ready(Ok(n))
        }
    }

    /// The same ceiling, over an asynchronous stream.
    #[test]
    fn the_row_ceiling_does_not_depend_on_the_buffer() {
        const CEILING: usize = 8;
        let plan = [Column::text(0)];
        let dialect = Dialect::CSV.with_header(false);
        for initial in [1, 3, 5, CEILING, 12, 4096] {
            for (text, fits) in [
                ("xxxxxxx", true),
                ("xxxxxxx\n", true),
                ("xxxxxxxx", false),
                ("xxxxxxxx\n", false),
                ("xxxxxxxxx", false),
            ] {
                let case = format!("buffer {initial}, {text:?}");
                let read = block_on(async {
                    let mut reader = DelimitedReader::options()
                        .buffer_bytes(initial)
                        .from_async_reader(Trickle(text.as_bytes(), false), dialect)
                        .await?;
                    reader.bind(&plan)?;
                    reader.limit_ceiling(CEILING);
                    let rows = reader.read_async().await?.map(|batch| batch.rows());
                    if fits {
                        assert_eq!(reader.read_async().await?.map(|batch| batch.rows()), None);
                    }
                    Ok::<_, Error>(rows)
                });
                if fits {
                    assert_eq!(read.unwrap(), Some(1), "{case}");
                } else {
                    let Err(Error::Structure(failure)) = read else {
                        panic!("{case}: a record that fills the ceiling is too long");
                    };
                    assert_eq!(failure.kind, FailureKind::RowTooLong, "{case}");
                }
            }

            let (rows, failure, again) = block_on(async {
                let mut reader = DelimitedReader::options()
                    .buffer_bytes(initial)
                    .from_async_reader(Trickle(b"1\n2\nxxxxxxxxx\n4\n", false), dialect)
                    .await
                    .unwrap();
                reader.bind(&plan).unwrap();
                reader.limit_ceiling(CEILING);
                let mut rows = 0;
                let failure = loop {
                    match reader.read_async().await {
                        Ok(Some(batch)) => rows += batch.rows(),
                        Ok(None) => panic!("buffer {initial}: the long record was read"),
                        Err(Error::Structure(failure)) => break failure,
                        Err(other) => panic!("buffer {initial}: {other:?}"),
                    }
                };
                let again = reader
                    .read_async()
                    .await
                    .map(|batch| batch.map(|batch| batch.rows()));
                (rows, failure, again)
            });
            assert_eq!(rows, 2, "buffer {initial}: the rows before are delivered");
            assert_eq!(failure.kind, FailureKind::RowTooLong);
            assert_eq!((failure.record, failure.line, failure.byte), (2, 3, 4));
            let Err(Error::Structure(again)) = again else {
                panic!("buffer {initial}: the failure is final");
            };
            assert_eq!(again, failure);
        }
    }
}
