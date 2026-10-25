//! A live source need not fill the input buffer or close before a complete row is read.

use hypertabular::{Column, DelimitedReader, Dialect, Error};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

enum Event {
    Bytes(&'static [u8]),
    Error(io::ErrorKind),
    End,
}

#[derive(Clone, Default)]
struct Source(Arc<Mutex<VecDeque<Event>>>);

impl Source {
    fn send(&self, event: Event) {
        self.0.lock().unwrap().push_back(event);
    }

    fn next(&self, buf: &mut [u8]) -> Option<io::Result<usize>> {
        let mut events = self.0.lock().unwrap();
        Some(match events.pop_front()? {
            Event::Bytes(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() {
                    events.push_front(Event::Bytes(&bytes[n..]));
                }
                Ok(n)
            }
            Event::Error(kind) => Err(io::Error::from(kind)),
            Event::End => Ok(0),
        })
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.next(buf)
            .unwrap_or_else(|| Err(io::ErrorKind::WouldBlock.into()))
    }
}

#[test]
fn a_header_and_batch_are_delivered_before_the_next_source_read() {
    let source = Source::default();
    source.send(Event::Bytes(b"n\n1\n"));
    let mut reader = DelimitedReader::from_reader_unbound(source.clone(), Dialect::CSV)
        .expect("a complete header must not wait for more bytes");
    reader.bind(&[Column::i32(0)]).unwrap();
    assert_eq!(reader.read().unwrap().unwrap().i32(0), &[1]);
    source.send(Event::Bytes(b"2\n"));
    assert_eq!(reader.read().unwrap().unwrap().i32(0), &[2]);
    source.send(Event::End);
    assert!(reader.read().unwrap().is_none());
}

#[test]
fn partial_records_survive_short_and_interrupted_reads_and_errors_stick() {
    let source = Source::default();
    source.send(Event::Error(io::ErrorKind::Interrupted));
    source.send(Event::Bytes(b"n\n12"));
    let mut reader =
        DelimitedReader::from_reader(source.clone(), Dialect::CSV, &[Column::i32(0)]).unwrap();
    source.send(Event::Error(io::ErrorKind::Interrupted));
    source.send(Event::Bytes(b"3\n"));
    assert_eq!(reader.read().unwrap().unwrap().i32(0), &[123]);
    source.send(Event::Error(io::ErrorKind::ConnectionReset));
    for _ in 0..2 {
        assert!(
            matches!(reader.read(), Err(Error::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset)
        );
    }
}

#[cfg(feature = "async")]
mod asynchronous {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    impl futures_io::AsyncRead for Source {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.next(buf).map_or(Poll::Pending, Poll::Ready)
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => {
                panic!("complete input must be delivered before the source is polled again")
            }
        }
    }

    #[test]
    fn a_live_header_and_batch_do_not_wait_for_eof() {
        let source = Source::default();
        source.send(Event::Bytes(b"n\n1\n"));
        let mut reader = ready(DelimitedReader::from_async_reader(
            source.clone(),
            Dialect::CSV,
        ))
        .unwrap();
        reader.bind(&[Column::i32(0)]).unwrap();
        assert_eq!(ready(reader.read_async()).unwrap().unwrap().i32(0), &[1]);
        source.send(Event::Bytes(b"2\n"));
        assert_eq!(ready(reader.read_async()).unwrap().unwrap().i32(0), &[2]);
        source.send(Event::End);
        assert!(ready(reader.read_async()).unwrap().is_none());
    }

    #[test]
    fn cancelling_a_partial_record_preserves_it_and_terminal_errors_stick() {
        let source = Source::default();
        source.send(Event::Error(io::ErrorKind::Interrupted));
        source.send(Event::Bytes(b"n\n12"));
        let mut reader = ready(DelimitedReader::from_async_reader(
            source.clone(),
            Dialect::CSV,
        ))
        .unwrap();
        reader.bind(&[Column::i32(0)]).unwrap();
        {
            let read = std::pin::pin!(reader.read_async());
            assert!(
                read.poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        source.send(Event::Error(io::ErrorKind::Interrupted));
        source.send(Event::Bytes(b"3\n"));
        assert_eq!(ready(reader.read_async()).unwrap().unwrap().i32(0), &[123]);
        source.send(Event::Error(io::ErrorKind::ConnectionReset));
        for _ in 0..2 {
            assert!(
                matches!(ready(reader.read_async()), Err(Error::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset)
            );
        }
    }
}
