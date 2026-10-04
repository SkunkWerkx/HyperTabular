//! The block classifiers live in the allocation-free core
//! ([`crate::kernel::delimited::engine`]); this is the part that needs a `Vec`.

pub use crate::kernel::delimited::engine::*;

/// Every engine this build can run, best first — the conformance suite runs each.
pub fn available() -> Vec<Kind> {
    let mut kinds = Vec::new();
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("aes") {
            kinds.push(Kind::NeonPmull);
        }
        kinds.push(Kind::Neon);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let clmul = std::arch::is_x86_feature_detected!("pclmulqdq");
        if std::arch::is_x86_feature_detected!("avx2") && clmul {
            kinds.push(Kind::Avx2Clmul);
        }
        if clmul {
            kinds.push(Kind::Sse2Clmul);
        }
        kinds.push(Kind::Sse2);
    }
    kinds.push(Kind::Swar);
    debug_assert!(kinds.contains(&detect()));
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::delimited::engine::prefix_xor_cascade;

    fn reference(block: &[u8; 64], sep: u8) -> Masks {
        let mut masks = Masks::default();
        for (i, &byte) in block.iter().enumerate() {
            let bit = 1u64 << i;
            if byte == sep {
                masks.sep |= bit;
            }
            if byte == b'\n' {
                masks.lf |= bit;
            }
            if byte == b'\r' {
                masks.cr |= bit;
            }
            if byte == b'"' {
                masks.quote |= bit;
            }
        }
        masks
    }

    fn reference_prefix_xor(x: u64) -> u64 {
        let mut out = 0u64;
        let mut acc = 0u64;
        for i in 0..64 {
            acc ^= (x >> i) & 1;
            out |= acc << i;
        }
        out
    }

    fn sample_blocks() -> Vec<[u8; 64]> {
        let mut blocks = Vec::new();
        let mut block = [b'x'; 64];
        block[0] = b',';
        block[1] = b'"';
        block[10] = b'\r';
        block[11] = b'\n';
        block[63] = b'"';
        block[62] = b'\n';
        block[33] = 0xE2; // non-ASCII must never match
        block[34] = 0x80;
        block[35] = 0x9C;
        blocks.push(block);
        blocks.push([b'"'; 64]);
        blocks.push([b','; 64]);
        blocks.push([0; 64]);
        blocks.push([0xFF; 64]);
        // A pseudo-random block from a fixed xorshift seed.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = [0u8; 64];
        for byte in &mut random {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = b",\"\r\n\tab"[(state % 7) as usize];
        }
        blocks.push(random);
        blocks
    }

    #[test]
    fn swar_matches_the_byte_by_byte_reference() {
        for block in sample_blocks() {
            for &sep in b",\t|x" {
                assert_eq!(
                    unsafe { Swar::classify(&block, sep) },
                    reference(&block, sep)
                );
            }
        }
        for x in [
            0,
            1,
            1 << 63,
            u64::MAX,
            0x8000_0000_0000_0001,
            0xDEAD_BEEF_CAFE_F00D,
        ] {
            assert_eq!(prefix_xor_cascade(x), reference_prefix_xor(x), "{x:#x}");
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn neon_engines_match_the_reference() {
        for block in sample_blocks() {
            for &sep in b",\t|" {
                assert_eq!(
                    unsafe { Neon::classify(&block, sep) },
                    reference(&block, sep)
                );
            }
        }
        if std::arch::is_aarch64_feature_detected!("aes") {
            for x in [
                0,
                1,
                1 << 63,
                u64::MAX,
                0x8000_0000_0000_0001,
                0xDEAD_BEEF_CAFE_F00D,
            ] {
                assert_eq!(
                    unsafe { NeonPmull::prefix_xor(x) },
                    reference_prefix_xor(x),
                    "{x:#x}"
                );
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x86_engines_match_the_reference() {
        for block in sample_blocks() {
            for &sep in b",\t|" {
                assert_eq!(
                    unsafe { Sse2::classify(&block, sep) },
                    reference(&block, sep)
                );
                if std::arch::is_x86_feature_detected!("avx2") {
                    assert_eq!(
                        unsafe { Avx2Clmul::classify(&block, sep) },
                        reference(&block, sep)
                    );
                }
            }
        }
        if std::arch::is_x86_feature_detected!("pclmulqdq") {
            for x in [
                0,
                1,
                1 << 63,
                u64::MAX,
                0x8000_0000_0000_0001,
                0xDEAD_BEEF_CAFE_F00D,
            ] {
                assert_eq!(
                    unsafe { Sse2Clmul::prefix_xor(x) },
                    reference_prefix_xor(x),
                    "{x:#x}"
                );
            }
        }
    }

    #[test]
    fn detect_returns_an_available_engine() {
        assert!(available().contains(&detect()));
        assert_eq!(available().last(), Some(&Kind::Swar));
    }
}
