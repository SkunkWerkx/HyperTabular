//! The core's inflate against zlib's deflate: whatever `flate2` compresses, at any level,
//! the core gives back byte for byte — fed a byte at a time or all at once, into a window
//! barely larger than the dictionary — and nothing it is handed can make it panic, loop,
//! or write outside the buffer.

use flate2::Compression;
use flate2::write::DeflateEncoder;
use hypertabular::kernel::inflate::{Inflate, Status};
use std::io::Write;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(level));
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Inflates `stream` the way the workbook reader will: input in chunks of at most
/// `feed`, output into a window of `window` bytes that is slid down — keeping the last
/// 32 KiB — whenever it fills.
fn inflate(stream: &[u8], feed: usize, window: usize) -> Result<Vec<u8>, &'static str> {
    const HISTORY: usize = 32 * 1024;
    let mut state = Box::new(Inflate::new());
    let mut out = vec![0u8; window];
    let (mut at, mut read) = (0usize, 0usize);
    let mut result = Vec::new();
    let mut calls = 0u64;
    loop {
        calls += 1;
        if calls > 50_000_000 {
            return Err("no progress");
        }
        let end = (read + feed).min(stream.len());
        let progress = state.run(&stream[read..end], &mut out, at);
        assert!(progress.consumed <= end - read);
        assert!(at + progress.written <= out.len());
        result.extend_from_slice(&out[at..at + progress.written]);
        read += progress.consumed;
        at += progress.written;
        match progress.status {
            Status::Done => return Ok(result),
            Status::Invalid => return Err("invalid"),
            Status::OutputFull => {
                assert!(
                    at > HISTORY,
                    "a window this small cannot hold the dictionary"
                );
                out.copy_within(at - HISTORY..at, 0);
                at = HISTORY;
            }
            Status::NeedsInput => {
                if end == stream.len() && progress.consumed == 0 {
                    return Err("truncated");
                }
            }
        }
    }
}

fn samples(random: &mut Random) -> Vec<Vec<u8>> {
    let mut samples: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"a".to_vec(),
        b"abcabcabcabcabcabcabcabcabcabc".to_vec(),
        vec![0u8; 100_000],
        (0..=255u8).cycle().take(70_000).collect(),
    ];
    // Sheet XML: long, repetitive, with numbers that are not.
    let mut xml = String::new();
    for row in 1..4000 {
        xml.push_str(&format!(
            "<row r=\"{row}\"><c r=\"A{row}\"><v>{row}</v></c><c r=\"B{row}\" t=\"s\"><v>{}</v></c><c r=\"C{row}\" s=\"3\"><v>{}.{}</v></c></row>",
            random.below(5000),
            45000 + random.below(900),
            random.next() % 10_000_000_000
        ));
    }
    samples.push(xml.into_bytes());
    // Incompressible: the encoder falls back to stored blocks.
    samples.push((0..90_000).map(|_| random.next() as u8).collect());
    // Text over a small alphabet, with long-range repeats.
    let mut text = Vec::new();
    for _ in 0..30_000 {
        text.extend_from_slice(
            [
                &b"the "[..],
                b"quick ",
                b"brown ",
                b"fox ",
                b"\n",
                b"0123456789",
            ][random.below(6)],
        );
        if random.below(50) == 0 && text.len() > 40_000 {
            let from = random.below(text.len() - 300);
            let repeat = text[from..from + 258].to_vec();
            text.extend_from_slice(&repeat);
        }
    }
    samples.push(text);
    samples
}

#[test]
fn whatever_zlib_deflates_the_core_inflates() {
    let mut random = Random(0x1F1A_7E00_C0DE_F00D);
    for (index, data) in samples(&mut random).into_iter().enumerate() {
        for level in 0..=9 {
            let stream = deflate(&data, level);
            for (feed, window) in [
                (usize::MAX >> 1, 1 << 20),
                (1, 32 * 1024 + 300),
                (7, 32 * 1024 + 258),
                (1 + random.below(4000), 40_000 + random.below(40_000)),
            ] {
                let got = inflate(&stream, feed, window);
                assert!(
                    got.as_deref() == Ok(&data[..]),
                    "sample {index}, level {level}, feed {feed}, window {window}: {:?}",
                    got.map(|bytes| bytes.len())
                );
            }
            // Cut short, it asks for more rather than making something up.
            if stream.len() > 2 {
                let cut = 1 + random.below(stream.len() - 1);
                assert_eq!(inflate(&stream[..cut], 4096, 1 << 20), Err("truncated"));
            }
        }
    }
}

#[test]
fn damaged_streams_fail_or_finish_and_never_misbehave() {
    let mut random = Random(0xBAD0_B175_0000_0001);
    let data: Vec<u8> = (0..20_000)
        .map(|i| (i % 97) as u8 ^ (random.next() % 3) as u8)
        .collect();
    for level in [1, 6, 9] {
        let stream = deflate(&data, level);
        for _ in 0..3_000 {
            let mut damaged = stream.clone();
            for _ in 0..1 + random.below(4) {
                let at = random.below(damaged.len());
                damaged[at] ^= 1 << random.below(8);
            }
            // Any outcome but a panic, a hang, or an out-of-bounds write is acceptable;
            // `inflate` asserts the bounds and counts the calls.
            let _ = inflate(
                &damaged,
                1 + random.below(600),
                32 * 1024 + 258 + random.below(5000),
            );
        }
    }
    // Bytes that were never a deflate stream.
    for _ in 0..3_000 {
        let noise: Vec<u8> = (0..random.below(400))
            .map(|_| random.next() as u8)
            .collect();
        let _ = inflate(&noise, 1 + random.below(64), 32 * 1024 + 258);
    }
    assert_eq!(
        inflate(&[0b0000_0111], 16, 1 << 16),
        Err("invalid"),
        "block type 3"
    );
}
