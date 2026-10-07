//! The core's inflater over any bytes, fed and drained the way the workbook reader does
//! it — input in chunks, output into a window slid down to its last 32 KiB whenever it
//! fills — and held to zlib's: a stream one of them finishes, the other finishes with the
//! same bytes, and a stream one refuses, the other refuses or finds unfinished. It must
//! also never panic, never stall, and never claim more input or output than it was given.
//! The first byte chooses the feed and the window; the rest is the raw deflate stream.

#![no_main]

use flate2::{Decompress, FlushDecompress, Status as ZStatus};
use hypertabular::kernel::inflate::{Inflate, Status};
use libfuzzer_sys::fuzz_target;

const HISTORY: usize = 32 * 1024;
/// Output past which a stream is not followed further: a few kilobytes of deflate can say
/// gigabytes, and both sides agreeing on the first megabytes is the point.
const OUTPUT_LIMIT: usize = 8 << 20;

#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Done(Vec<u8>),
    Invalid,
    Truncated,
    Long,
}

fn core(stream: &[u8], feed: usize, window: usize) -> Ending {
    let mut state = Box::new(Inflate::new());
    let mut out = vec![0u8; window];
    let (mut at, mut read) = (0usize, 0usize);
    let mut result = Vec::new();
    let mut calls = 0u64;
    loop {
        calls += 1;
        assert!(calls < 10_000_000, "the inflater is not making progress");
        let end = (read + feed).min(stream.len());
        let progress = state.run(&stream[read..end], &mut out, at);
        assert!(
            progress.consumed <= end - read,
            "consumed more than it was given"
        );
        assert!(at + progress.written <= out.len(), "wrote past the window");
        result.extend_from_slice(&out[at..at + progress.written]);
        if result.len() > OUTPUT_LIMIT {
            return Ending::Long;
        }
        read += progress.consumed;
        at += progress.written;
        match progress.status {
            Status::Done => return Ending::Done(result),
            Status::Invalid => return Ending::Invalid,
            Status::OutputFull => {
                assert!(at > HISTORY, "full with only {at} bytes written");
                out.copy_within(at - HISTORY..at, 0);
                at = HISTORY;
            }
            Status::NeedsInput => {
                if end == stream.len() && progress.consumed == 0 {
                    return Ending::Truncated;
                }
            }
        }
    }
}

fn zlib(stream: &[u8]) -> Ending {
    let mut state = Decompress::new(false);
    let mut result = Vec::with_capacity(1 << 16);
    loop {
        if result.len() > OUTPUT_LIMIT {
            return Ending::Long;
        }
        result.reserve(1 << 16);
        let before = (state.total_in(), state.total_out());
        let at = state.total_in() as usize;
        match state.decompress_vec(&stream[at..], &mut result, FlushDecompress::Finish) {
            // Past the limit is past it however the output arrived: zlib writes in large
            // steps, and the core's side stops the moment it crosses.
            Ok(ZStatus::StreamEnd) if result.len() > OUTPUT_LIMIT => return Ending::Long,
            Ok(ZStatus::StreamEnd) => return Ending::Done(result),
            Ok(_) => {
                if (state.total_in(), state.total_out()) == before {
                    return Ending::Truncated;
                }
            }
            Err(_) => return Ending::Invalid,
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let [sel, stream @ ..] = data else {
        return;
    };
    let feed = 1 + usize::from(sel & 0x0F) * 97;
    // The reader's own minimum window, 64 KiB, and two above it.
    let window = (64 << 10) << ((sel >> 4) % 3);
    let ours = core(stream, feed, window);
    let theirs = zlib(stream);
    match (&ours, &theirs) {
        (Ending::Done(a), Ending::Done(b)) => assert!(a == b, "the two inflate differently"),
        (Ending::Done(_), _) | (_, Ending::Done(_)) => {
            panic!(
                "the core says {:?} and zlib says {:?}",
                short(&ours),
                short(&theirs)
            )
        }
        _ => {}
    }
});

fn short(ending: &Ending) -> String {
    match ending {
        Ending::Done(bytes) => format!("done, {} bytes", bytes.len()),
        other => format!("{other:?}"),
    }
}
