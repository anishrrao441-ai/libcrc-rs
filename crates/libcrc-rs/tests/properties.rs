//! Property-based tests over the algebraic laws CRCs must obey.
//!
//! The brief notes that property tests survive translation where example-based tests do
//! not: a table of expected values only proves the port agrees with the original on the
//! inputs someone happened to write down. These assert *laws* — statements that must hold
//! for every input — so they keep their meaning independent of any fixed vector.
//!
//! The generator is a seeded SplitMix64 implemented here rather than pulled from a crate,
//! so the suite is dependency-free, offline, and byte-for-byte reproducible: a failure
//! reported by CI replays identically on any machine.

use libcrc_rs::*;

/// SplitMix64. Deterministic, so any failure is reproducible from the seed alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 24) as u8).collect()
    }
    /// Lengths biased toward the small sizes and boundaries where bugs live.
    fn len(&mut self) -> usize {
        match self.next_u64() % 10 {
            0 => 0,
            1 => 1,
            2..=5 => (self.next_u64() % 64) as usize,
            6..=8 => (self.next_u64() % 512) as usize,
            _ => (self.next_u64() % 4096) as usize,
        }
    }
}

const SEED: u64 = 0x5011_CE64_C0FF_EE01;
const ROUNDS: usize = 2_000;

/// LAW 1 — feeding bytes one at a time must equal the one-shot call, for every
/// algorithm that exposes both forms. libcrc ships both and callers mix them, so any
/// disagreement is a real defect regardless of what the fixed vectors say.
#[test]
fn incremental_equals_one_shot_for_every_algorithm() {
    let mut rng = Rng::new(SEED);
    for round in 0..ROUNDS {
        let n = rng.len();
        let data = rng.bytes(n);
        let ctx = || format!("round {round}, len {}", data.len());

        assert_eq!(
            data.iter().fold(0u8, |c, &b| update_crc_8(c, b)),
            crc_8(&data),
            "crc_8 {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0x0000u16, |c, &b| update_crc_16(c, b)),
            crc_16(&data),
            "crc_16 {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0xFFFFu16, |c, &b| update_crc_16(c, b)),
            crc_modbus(&data),
            "crc_modbus {}",
            ctx()
        );
        assert_eq!(
            data.iter()
                .fold(0xFFFF_FFFFu32, |c, &b| update_crc_32(c, b))
                ^ 0xFFFF_FFFF,
            crc_32(&data),
            "crc_32 {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0x0000u16, |c, &b| update_crc_ccitt(c, b)),
            crc_xmodem(&data),
            "crc_xmodem {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0xFFFFu16, |c, &b| update_crc_ccitt(c, b)),
            crc_ccitt_ffff(&data),
            "crc_ccitt_ffff {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0x1D0Fu16, |c, &b| update_crc_ccitt(c, b)),
            crc_ccitt_1d0f(&data),
            "crc_ccitt_1d0f {}",
            ctx()
        );
        assert_eq!(
            data.iter().fold(0u64, |c, &b| update_crc_64(c, b)),
            crc_64_ecma(&data),
            "crc_64_ecma {}",
            ctx()
        );
    }
}

/// LAW 2 — chunking must not matter. A streaming digest fed in arbitrary pieces must
/// equal the one-shot result; if it does not, the digest is unusable for streams.
#[test]
fn digest_is_independent_of_chunk_boundaries() {
    let mut rng = Rng::new(SEED ^ 0xA5A5);
    for round in 0..ROUNDS {
        let n = rng.len().max(1);
        let data = rng.bytes(n);

        let mut d32 = Crc32Digest::new();
        let mut d16 = Crc16Digest::new();
        let mut off = 0;
        while off < data.len() {
            let take = ((rng.next_u64() % 17) as usize + 1).min(data.len() - off);
            d32.update(&data[off..off + take]);
            d16.update(&data[off..off + take]);
            off += take;
        }
        assert_eq!(d32.finalize(), crc_32(&data), "crc_32 round {round}");
        assert_eq!(d16.finalize(), crc_16(&data), "crc_16 round {round}");
    }
}

/// LAW 3 — `crc_32_combine(CRC(a), CRC(b), |b|)` must equal `CRC(a ‖ b)` for every
/// split of every input. This is the defining property of the combine operation and the
/// only thing that makes chunked or parallel hashing safe.
#[test]
fn combine_equals_concatenation_for_random_splits() {
    let mut rng = Rng::new(SEED ^ 0xC0FFEE);
    for round in 0..ROUNDS {
        let n = rng.len();
        let data = rng.bytes(n);
        let split = if data.is_empty() {
            0
        } else {
            (rng.next_u64() as usize) % (data.len() + 1)
        };
        let (a, b) = data.split_at(split);
        assert_eq!(
            crc_32_combine(crc_32(a), crc_32(b), b.len()),
            crc_32(&data),
            "round {round}: split {split} of {} bytes",
            data.len()
        );
    }
}

/// LAW 4 — the CRC-32 residue. Appending a message's own CRC (little-endian) and
/// hashing the result yields a constant, independent of the message. This is the
/// property that makes CRCs usable as a wire check: a receiver hashes payload+checksum
/// and compares against one fixed value.
#[test]
fn crc32_residue_is_constant_for_every_message() {
    let mut rng = Rng::new(SEED ^ 0x1234_5678);
    let mut residue = None;
    for round in 0..ROUNDS {
        let n = rng.len();
        let data = rng.bytes(n);
        let mut framed = data.clone();
        framed.extend_from_slice(&crc_32(&data).to_le_bytes());
        let r = crc_32(&framed);
        match residue {
            None => residue = Some(r),
            Some(expected) => assert_eq!(
                r,
                expected,
                "round {round}: residue changed for a {}-byte message",
                data.len()
            ),
        }
    }
    // The well-known CRC-32 residue. Asserted explicitly so a change in the final-XOR
    // or reflection convention is caught, not just an internally-consistent drift.
    assert_eq!(residue, Some(0x2144_DF1C), "unexpected CRC-32 residue");
}

/// LAW 5 — an empty input must return the seed, unchanged, for every algorithm.
/// Cheap, but it is the edge every hand-written byte loop gets wrong first.
#[test]
fn empty_input_returns_the_seed() {
    assert_eq!(crc_8(b""), 0x00);
    assert_eq!(crc_16(b""), 0x0000);
    assert_eq!(crc_modbus(b""), 0xFFFF);
    assert_eq!(crc_xmodem(b""), 0x0000);
    assert_eq!(crc_ccitt_ffff(b""), 0xFFFF);
    assert_eq!(crc_ccitt_1d0f(b""), 0x1D0F);
    assert_eq!(crc_64_ecma(b""), 0);
    assert_eq!(crc_32(b""), 0x0000_0000);
}

/// LAW 6 — determinism. The same input must always produce the same output; a CRC that
/// depended on hidden state (as the original's lazily-initialised tables arguably do)
/// would violate this.
#[test]
fn hashing_is_deterministic() {
    let mut rng = Rng::new(SEED ^ 0xDEAD_BEEF);
    for _ in 0..ROUNDS {
        let n = rng.len();
        let data = rng.bytes(n);
        assert_eq!(crc_32(&data), crc_32(&data));
        assert_eq!(crc_sick(&data), crc_sick(&data));
        assert_eq!(crc_kermit(&data), crc_kermit(&data));
    }
}

/// LAW 7 — a single-bit change must change the checksum. This is the entire purpose of
/// an error-detecting code; a port that silently ignored some input bytes would still
/// pass fixed vectors but fail here.
#[test]
fn every_single_bit_flip_changes_the_checksum() {
    let mut rng = Rng::new(SEED ^ 0x0BADF00D);
    for _ in 0..200 {
        let n = (rng.next_u64() % 64) as usize + 1;
        let mut data = rng.bytes(n);
        let original = crc_32(&data);
        let byte = (rng.next_u64() as usize) % data.len();
        let bit = (rng.next_u64() % 8) as u8;
        data[byte] ^= 1 << bit;
        assert_ne!(
            crc_32(&data),
            original,
            "flipping bit {bit} of byte {byte} left the CRC unchanged"
        );
    }
}
