//! The per-block classifiers. Every engine answers the same two questions about a
//! 64-byte block: which bytes are the separator, `\n`, `\r`, and `"` (four bit masks),
//! and what is the inclusive prefix-XOR of a mask (the carry-less multiply that turns
//! quote positions into an inside-quotes mask — Langdale/Lemire's trick, as zsv, Polars,
//! and simdcsv use it).
//!
//! | Engine | Classify | Prefix XOR | Where |
//! | --- | --- | --- | --- |
//! | [`NeonPmull`] | `vceqq_u8` + the `vpaddq` bit gather | `vmull_p64` | aarch64 with `aes` |
//! | [`Neon`] | same | shift cascade | any aarch64 |
//! | [`Avx2Clmul`] | `_mm256_cmpeq_epi8` + `movemask` ×2 | `_mm_clmulepi64_si128` | x86-64 with `avx2` + `pclmulqdq` |
//! | [`Sse2Clmul`] | `_mm_cmpeq_epi8` + `movemask` ×4 | `_mm_clmulepi64_si128` | x86-64 with `pclmulqdq` |
//! | [`Sse2`] | same | shift cascade | any x86-64 |
//! | [`Swar`] | 8 bytes at a time in a `u64` | shift cascade | everywhere |
//!
//! [`detect`] picks the best available at run time, from `CPUID` itself; the scanner is monomorphised per
//! engine under the matching `#[target_feature]` so the intrinsics inline into the loop.
//! Only the aarch64 engines have been executed on the machine this was written on; the
//! x86-64 engines are exercised by the same conformance suite wherever it runs.

/// The four structural bit masks of one 64-byte block; bit `i` describes byte `i`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Masks {
    pub sep: u64,
    pub lf: u64,
    pub cr: u64,
    pub quote: u64,
}

/// A block classifier. See the module doc.
pub trait Engine {
    /// The masks of `block` for separator `sep`.
    ///
    /// # Safety
    /// The CPU features the engine needs were detected (see [`detect`]).
    unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks;

    /// Inclusive prefix XOR: bit `i` of the result is the XOR of bits `0..=i` of `x`.
    ///
    /// # Safety
    /// As [`Engine::classify`].
    unsafe fn prefix_xor(x: u64) -> u64;
}

/// Which engine [`detect`] chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Swar,
    Neon,
    NeonPmull,
    Sse2,
    Sse2Clmul,
    Avx2Clmul,
}

impl Kind {
    /// The engine's name, for benchmark labels.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Swar => "swar",
            Kind::Neon => "neon",
            Kind::NeonPmull => "neon+pmull",
            Kind::Sse2 => "sse2",
            Kind::Sse2Clmul => "sse2+pclmulqdq",
            Kind::Avx2Clmul => "avx2+pclmulqdq",
        }
    }
}

/// The best engine this CPU supports. The answer never changes for a process, and asking
/// costs a `CPUID` (a trap, under a hypervisor), so a caller asks once and keeps it.
pub fn detect() -> Kind {
    #[cfg(target_arch = "x86_64")]
    {
        let (avx2, clmul) = x86_features();
        if avx2 && clmul {
            return Kind::Avx2Clmul;
        }
        if clmul {
            return Kind::Sse2Clmul;
        }
        return Kind::Sse2;
    }
    #[cfg(target_arch = "aarch64")]
    {
        // The carry-less multiply: built in, or Apple silicon (which has always had it), or
        // what the operating system says of this CPU.
        if cfg!(target_feature = "aes") || cfg!(target_vendor = "apple") || system_pmull() {
            return Kind::NeonPmull;
        }
        return Kind::Neon;
    }
    #[allow(unreachable_code)]
    Kind::Swar
}

/// Whether the operating system reports the AES and polynomial-multiply instructions
/// for this CPU — what Rust's `aes` target feature means on aarch64, and what the standard
/// library's own detection asks. On Linux, the auxiliary vector the kernel hands every
/// process, through the C library's accessor; on Windows, the one question kernel32 has
/// for it. Anywhere else there is nobody to ask, and the answer is no.
#[cfg(target_arch = "aarch64")]
fn system_pmull() -> bool {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            safe fn getauxval(kind: core::ffi::c_ulong) -> core::ffi::c_ulong;
        }
        const AT_HWCAP: core::ffi::c_ulong = 16;
        const HWCAP_AES: core::ffi::c_ulong = 1 << 3;
        const HWCAP_PMULL: core::ffi::c_ulong = 1 << 4;
        return getauxval(AT_HWCAP) & (HWCAP_AES | HWCAP_PMULL) == HWCAP_AES | HWCAP_PMULL;
    }
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            safe fn IsProcessorFeaturePresent(feature: u32) -> i32;
        }
        const PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE: u32 = 30;
        return IsProcessorFeaturePresent(PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE) != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// `(avx2, pclmulqdq)` from `CPUID`, with `XGETBV` confirming the operating system saves
/// the wide registers — the same questions the standard library asks, without it.
#[cfg(target_arch = "x86_64")]
// `__cpuid` and `__cpuid_count` are safe functions from Rust 1.94 and unsafe ones before it;
// the blocks keep the crate building on its rust-version, and newer compilers call them
// unused.
#[allow(unused_unsafe)]
pub fn x86_features() -> (bool, bool) {
    use core::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
    // SAFETY: CPUID exists on every x86_64 processor.
    let leaf1 = unsafe { __cpuid(1) };
    let clmul = leaf1.ecx & (1 << 1) != 0;
    let os_xsave = leaf1.ecx & (1 << 27) != 0;
    let avx = leaf1.ecx & (1 << 28) != 0;
    // SAFETY: `XGETBV` exists when CPUID reports OSXSAVE.
    let wide_saved = os_xsave && unsafe { _xgetbv(0) } & 0b110 == 0b110;
    // SAFETY: as above; leaf 7 answers zeros on a processor that lacks it.
    let avx2 = avx && wide_saved && unsafe { __cpuid_count(7, 0) }.ebx & (1 << 5) != 0;
    (avx2, clmul)
}

/// [`detect`]'s answer, kept: `0` until the first call asks. The one thing the core
/// remembers on its own, and a fact about the machine rather than about any caller.
static BEST: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The best engine this CPU supports, asked for once per process.
pub fn best() -> Kind {
    use core::sync::atomic::Ordering;
    match Kind::from_code(BEST.load(Ordering::Relaxed)) {
        Some(kind) => kind,
        None => {
            let kind = detect();
            BEST.store(kind.code(), Ordering::Relaxed);
            kind
        }
    }
}

/// True when this CPU can run `kind`: it asks for nothing [`best`] does not have.
pub fn usable(kind: Kind) -> bool {
    kind.rank() <= best().rank()
}

impl Kind {
    /// How much an engine asks of the CPU, within one architecture.
    const fn rank(self) -> u8 {
        match self {
            Kind::Swar => 0,
            Kind::Neon | Kind::Sse2 => 1,
            Kind::NeonPmull | Kind::Sse2Clmul => 2,
            Kind::Avx2Clmul => 3,
        }
    }

    /// The engine's number in a caller's state block; `0` is never one.
    pub const fn code(self) -> u8 {
        match self {
            Kind::Swar => 1,
            Kind::Neon => 2,
            Kind::NeonPmull => 3,
            Kind::Sse2 => 4,
            Kind::Sse2Clmul => 5,
            Kind::Avx2Clmul => 6,
        }
    }

    /// The engine a state block names, if this build can run it. The scanner still never
    /// trusts the number with an instruction the CPU lacks: it comes from [`detect`].
    pub const fn from_code(code: u8) -> Option<Kind> {
        Some(match code {
            1 => Kind::Swar,
            #[cfg(target_arch = "aarch64")]
            2 => Kind::Neon,
            #[cfg(target_arch = "aarch64")]
            3 => Kind::NeonPmull,
            #[cfg(target_arch = "x86_64")]
            4 => Kind::Sse2,
            #[cfg(target_arch = "x86_64")]
            5 => Kind::Sse2Clmul,
            #[cfg(target_arch = "x86_64")]
            6 => Kind::Avx2Clmul,
            _ => return None,
        })
    }
}

// --- SWAR: the portable floor ---

/// Eight bytes at a time in a `u64`; correct everywhere, fast nowhere in particular.
pub struct Swar;

const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const GATHER: u64 = 0x0102_0408_1020_4080;

/// Bit `i` set iff byte `i` of `chunk` equals byte `i` of `needle` (needle replicated).
/// Exact — the classic `(x - 0x01…) & !x & 0x80…` has false positives above a match.
#[inline(always)]
fn eq8(chunk: u64, needle: u64) -> u64 {
    let x = chunk ^ needle;
    let high_if_nonzero_low7 = (x & LOW7).wrapping_add(LOW7);
    let zero_bytes = !(high_if_nonzero_low7 | x | LOW7);
    (zero_bytes >> 7).wrapping_mul(GATHER) >> 56
}

#[inline(always)]
const fn replicate(byte: u8) -> u64 {
    (byte as u64) * 0x0101_0101_0101_0101
}

/// The six-round shift cascade: an inclusive prefix XOR without a carry-less multiplier.
#[inline(always)]
pub const fn prefix_xor_cascade(mut x: u64) -> u64 {
    x ^= x << 1;
    x ^= x << 2;
    x ^= x << 4;
    x ^= x << 8;
    x ^= x << 16;
    x ^= x << 32;
    x
}

impl Engine for Swar {
    #[inline(always)]
    unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
        let (sep_n, lf_n, cr_n, quote_n) = (
            replicate(sep),
            replicate(b'\n'),
            replicate(b'\r'),
            replicate(b'"'),
        );
        let mut masks = Masks::default();
        for (index, chunk) in block.as_chunks::<8>().0.iter().enumerate() {
            let word = u64::from_le_bytes(*chunk);
            let shift = index * 8;
            masks.sep |= eq8(word, sep_n) << shift;
            masks.lf |= eq8(word, lf_n) << shift;
            masks.cr |= eq8(word, cr_n) << shift;
            masks.quote |= eq8(word, quote_n) << shift;
        }
        masks
    }

    #[inline(always)]
    unsafe fn prefix_xor(x: u64) -> u64 {
        prefix_xor_cascade(x)
    }
}

// --- aarch64 ---

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use super::{Engine, Masks, prefix_xor_cascade};
    use core::arch::aarch64::*;

    /// NEON compares with the `vpaddq` bit gather (simdjson's `to_bitmask`); prefix XOR
    /// by shift cascade.
    pub struct Neon;
    /// [`Neon`] classification with `vmull_p64` for the prefix XOR.
    pub struct NeonPmull;

    const BITS: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];

    #[inline(always)]
    unsafe fn bitmask(v: &[uint8x16_t; 4], needle: uint8x16_t, bits: uint8x16_t) -> u64 {
        unsafe {
            let m0 = vandq_u8(vceqq_u8(v[0], needle), bits);
            let m1 = vandq_u8(vceqq_u8(v[1], needle), bits);
            let m2 = vandq_u8(vceqq_u8(v[2], needle), bits);
            let m3 = vandq_u8(vceqq_u8(v[3], needle), bits);
            let s0 = vpaddq_u8(m0, m1);
            let s1 = vpaddq_u8(m2, m3);
            let s = vpaddq_u8(s0, s1);
            let s = vpaddq_u8(s, s);
            vgetq_lane_u64::<0>(vreinterpretq_u64_u8(s))
        }
    }

    #[inline(always)]
    unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
        unsafe {
            let p = block.as_ptr();
            let v = [
                vld1q_u8(p),
                vld1q_u8(p.add(16)),
                vld1q_u8(p.add(32)),
                vld1q_u8(p.add(48)),
            ];
            let bits = vld1q_u8(BITS.as_ptr());
            Masks {
                sep: bitmask(&v, vdupq_n_u8(sep), bits),
                lf: bitmask(&v, vdupq_n_u8(b'\n'), bits),
                cr: bitmask(&v, vdupq_n_u8(b'\r'), bits),
                quote: bitmask(&v, vdupq_n_u8(b'"'), bits),
            }
        }
    }

    impl Engine for Neon {
        #[inline(always)]
        unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
            unsafe { classify(block, sep) }
        }

        #[inline(always)]
        unsafe fn prefix_xor(x: u64) -> u64 {
            prefix_xor_cascade(x)
        }
    }

    impl Engine for NeonPmull {
        #[inline(always)]
        unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
            unsafe { classify(block, sep) }
        }

        #[inline(always)]
        unsafe fn prefix_xor(x: u64) -> u64 {
            // Carry-less multiply by all-ones: bit i = XOR of bits 0..=i.
            unsafe {
                let product = vmull_p64(x, u64::MAX);
                vgetq_lane_u64::<0>(vreinterpretq_u64_p128(product))
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub use aarch64::{Neon, NeonPmull};

// --- x86-64 ---

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::{Engine, Masks, prefix_xor_cascade};
    use core::arch::x86_64::*;

    /// Four 16-byte compares per class; prefix XOR by shift cascade.
    pub struct Sse2;
    /// [`Sse2`] classification with `pclmulqdq` for the prefix XOR.
    pub struct Sse2Clmul;
    /// Two 32-byte compares per class with `pclmulqdq` for the prefix XOR.
    pub struct Avx2Clmul;

    #[inline(always)]
    unsafe fn sse2_bitmask(v: &[__m128i; 4], needle: __m128i) -> u64 {
        unsafe {
            let m0 = _mm_movemask_epi8(_mm_cmpeq_epi8(v[0], needle)) as u16 as u64;
            let m1 = _mm_movemask_epi8(_mm_cmpeq_epi8(v[1], needle)) as u16 as u64;
            let m2 = _mm_movemask_epi8(_mm_cmpeq_epi8(v[2], needle)) as u16 as u64;
            let m3 = _mm_movemask_epi8(_mm_cmpeq_epi8(v[3], needle)) as u16 as u64;
            m0 | (m1 << 16) | (m2 << 32) | (m3 << 48)
        }
    }

    #[inline(always)]
    unsafe fn sse2_classify(block: &[u8; 64], sep: u8) -> Masks {
        unsafe {
            let p = block.as_ptr().cast::<__m128i>();
            let v = [
                _mm_loadu_si128(p),
                _mm_loadu_si128(p.add(1)),
                _mm_loadu_si128(p.add(2)),
                _mm_loadu_si128(p.add(3)),
            ];
            Masks {
                sep: sse2_bitmask(&v, _mm_set1_epi8(sep as i8)),
                lf: sse2_bitmask(&v, _mm_set1_epi8(b'\n' as i8)),
                cr: sse2_bitmask(&v, _mm_set1_epi8(b'\r' as i8)),
                quote: sse2_bitmask(&v, _mm_set1_epi8(b'"' as i8)),
            }
        }
    }

    #[inline(always)]
    unsafe fn clmul_prefix_xor(x: u64) -> u64 {
        unsafe {
            let a = _mm_set_epi64x(0, x as i64);
            let ones = _mm_set1_epi8(-1);
            _mm_cvtsi128_si64(_mm_clmulepi64_si128::<0>(a, ones)) as u64
        }
    }

    impl Engine for Sse2 {
        #[inline(always)]
        unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
            unsafe { sse2_classify(block, sep) }
        }

        #[inline(always)]
        unsafe fn prefix_xor(x: u64) -> u64 {
            prefix_xor_cascade(x)
        }
    }

    impl Engine for Sse2Clmul {
        #[inline(always)]
        unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
            unsafe { sse2_classify(block, sep) }
        }

        #[inline(always)]
        unsafe fn prefix_xor(x: u64) -> u64 {
            unsafe { clmul_prefix_xor(x) }
        }
    }

    #[inline(always)]
    unsafe fn avx2_bitmask(lo: __m256i, hi: __m256i, needle: __m256i) -> u64 {
        unsafe {
            let a = _mm256_movemask_epi8(_mm256_cmpeq_epi8(lo, needle)) as u32 as u64;
            let b = _mm256_movemask_epi8(_mm256_cmpeq_epi8(hi, needle)) as u32 as u64;
            a | (b << 32)
        }
    }

    impl Engine for Avx2Clmul {
        #[inline(always)]
        unsafe fn classify(block: &[u8; 64], sep: u8) -> Masks {
            unsafe {
                let p = block.as_ptr().cast::<__m256i>();
                let lo = _mm256_loadu_si256(p);
                let hi = _mm256_loadu_si256(p.add(1));
                Masks {
                    sep: avx2_bitmask(lo, hi, _mm256_set1_epi8(sep as i8)),
                    lf: avx2_bitmask(lo, hi, _mm256_set1_epi8(b'\n' as i8)),
                    cr: avx2_bitmask(lo, hi, _mm256_set1_epi8(b'\r' as i8)),
                    quote: avx2_bitmask(lo, hi, _mm256_set1_epi8(b'"' as i8)),
                }
            }
        }

        #[inline(always)]
        unsafe fn prefix_xor(x: u64) -> u64 {
            unsafe { clmul_prefix_xor(x) }
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub use x86_64::{Avx2Clmul, Sse2, Sse2Clmul};
