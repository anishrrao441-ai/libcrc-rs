//! A seeded PRNG the fuzzer owns outright.
//!
//! No `rand` crate: the network here is unreliable and a fuzzing bonus that hinges on
//! "reproducible from a recorded seed" should not hinge on a third party's version
//! resolution either. Two well-known, tiny generators are enough.

/// SplitMix64 — Steele, Lea & Flood (2014). Used to derive independent per-case states
/// from one master seed, because it decorrelates sequential inputs well.
#[inline]
pub const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// xorshift64* — Marsaglia (2003), Vigna's multiplier variant.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    /// A zero state is the one fixed point of xorshift and would emit zeros forever, so
    /// it is remapped rather than rejected. Every other seed is used as given.
    pub const fn new(seed: u64) -> Self {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    /// Derive the generator for case `index` under `master_seed`.
    ///
    /// The point of routing through SplitMix64 is that case *N* depends only on the seed
    /// and on *N* — never on how the run happened to be split into batches. A recorded
    /// seed therefore replays identically at any batch size, and a diverging case can be
    /// re-run on its own by index.
    pub const fn for_case(master_seed: u64, index: u64) -> Self {
        Rng::new(splitmix64(master_seed ^ splitmix64(index)))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    #[inline]
    pub fn next_u8(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    /// Uniform-ish in `0..bound`. Lemire's multiply-shift, without the rejection loop:
    /// the residual bias is under 2^-32 and a fuzzer's input distribution does not need
    /// to be exact.
    #[inline]
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        ((self.next_u32() as u64 * bound as u64) >> 32) as u32
    }

    /// Inclusive range `lo..=hi`.
    #[inline]
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        debug_assert!(lo <= hi);
        lo + self.below(hi - lo + 1)
    }

    /// Fill `dst` with random bytes, eight at a time.
    pub fn fill(&mut self, dst: &mut [u8]) {
        let mut chunks = dst.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let tail = chunks.into_remainder();
        if !tail.is_empty() {
            let bytes = self.next_u64().to_le_bytes();
            tail.copy_from_slice(&bytes[..tail.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded seed is worthless if the stream is not bit-for-bit repeatable.
    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(0xDEAD_BEEF_CAFE_F00D);
        let mut b = Rng::new(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn case_streams_are_index_addressable() {
        let seed = 0x0123_4567_89AB_CDEF;
        let mut first = Rng::for_case(seed, 42);
        let mut again = Rng::for_case(seed, 42);
        let mut other = Rng::for_case(seed, 43);
        assert_eq!(first.next_u64(), again.next_u64());
        assert_ne!(Rng::for_case(seed, 42).next_u64(), other.next_u64());
    }

    #[test]
    fn zero_seed_does_not_collapse() {
        let mut rng = Rng::new(0);
        assert_ne!(rng.next_u64(), 0);
        assert_ne!(rng.next_u64(), 0);
    }

    #[test]
    fn below_respects_its_bound() {
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            assert!(rng.below(10) < 10);
            assert!((3..=9).contains(&rng.range(3, 9)));
        }
    }
}
