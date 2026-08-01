//! Slice-by-8 bulk folding — eight bytes per iteration, in pure safe Rust.
//!
//! # The problem this solves
//!
//! libcrc folds one byte at a time. Every step is `crc = f(crc, byte)`, so each table
//! load depends on the value produced by the previous one. The loop therefore runs at
//! the *latency* of an L1 hit — roughly 4-5 cycles per byte — no matter how wide the
//! machine is. Nothing in the port's byte-at-a-time loop was slow; the shape of the
//! recurrence was.
//!
//! Slice-by-8 (Kounavis & Berry, *Novel Table Lookup-Based Algorithms for
//! High-Performance CRC Generation*, IEEE Trans. Computers 57(11), 2008) rewrites the
//! fold so that eight bytes are consumed per iteration through eight *independent*
//! tables. The eight loads have no dependency on each other, so they pipeline; only the
//! final XOR reduction is serial. The chain shrinks from eight loads to one.
//!
//! Everything here is safe Rust. No `unsafe`, no intrinsics, no `target_feature`, no
//! CLMUL — the crate is `#![forbid(unsafe_code)]` and stays that way. The eight lookups
//! are ordinary indexed array reads; every index is masked or comes from a `u8`, so the
//! compiler proves them in range and elides the bounds checks on its own.
//!
//! # Why the result is bit-exact, not an approximation
//!
//! For every algorithm accelerated here the single-byte step is GF(2)-linear in `crc`
//! and `byte` jointly:
//!
//! ```text
//! width 8, forward:  f(crc, b) = T0[crc ^ b]
//! reflected:         f(crc, b) = (crc >> 8) ^ T0[(crc ^ b) & 0xFF]
//! forward (CCITT):   f(crc, b) = (crc << 8) ^ T0[((crc >> 8) ^ b) & 0xFF]
//! ```
//!
//! `>> 8` and `<< 8` are linear, and each `T0` is the lookup table of a linear map, so
//! `T0[x ^ y] == T0[x] ^ T0[y]`. Two facts follow.
//!
//! **1. Feeding a CRC its own leading bytes annihilates it.** Choose `b = crc & 0xFF`
//! in the reflected step and it collapses to `crc >> 8`; choose `b = crc >> 8` in the
//! forward step and it collapses to `crc << 8`. After `W/8` such steps a `W`-bit state
//! is zero. So `f^(W/8)(crc, its own bytes) == 0`, and by linearity
//!
//! ```text
//! f^(W/8)(crc, 0...0)  ==  f^(W/8)(0, bytes of crc)
//! ```
//!
//! That identity is what licenses XORing the first `W/8` message bytes straight into
//! the state and then treating the whole thing as message.
//!
//! **2. Byte contributions superpose.** Starting from zero, the state after eight bytes
//! `m0..m7` is `XOR_j T_(7-j)[m_j]`, where `T_k[i]` means "byte `i` folded into a zero
//! state, then shifted past `k` zero bytes":
//!
//! ```text
//! T_0     = the byte-at-a-time table already in lib.rs
//! T_k[i]  = shift(T_(k-1)[i]),   shift(v) = f(v, 0)
//! ```
//!
//! Combining the two, one iteration over `b0..b7` is exactly
//!
//! ```text
//! X   = crc ^ (the first W/8 bytes, in that algorithm's own byte order)
//! crc = T_7[X_0] ^ T_6[X_1] ^ ... ^ T_1[b6] ^ T_0[b7]
//! ```
//!
//! which is what the loops below compute. This is an algebraic identity, not a
//! heuristic: the accelerated path returns the same bits as the byte loop for every
//! input, and `tests` below check that for every length from 0 to 300 and on multi-
//! megabyte random buffers.
//!
//! # Memory cost, and how to decline it
//!
//! Only `T_1..T_7` are stored — `T_0` *is* the byte-at-a-time table in `lib.rs`, so
//! nothing is duplicated. Extra `.rodata`, on top of the tables the port already had:
//!
//! | Algorithms | Table | Added |
//! |---|---|---|
//! | `crc_8` | `[[u8; 256]; 7]` | 1792 B |
//! | `crc_16`, `crc_modbus` | `[[u16; 256]; 7]` | 3584 B |
//! | `crc_ccitt_1d0f`, `crc_ccitt_ffff`, `crc_xmodem` | `[[u16; 256]; 7]` | 3584 B |
//! | `crc_kermit` | `[[u16; 256]; 7]` | 3584 B |
//! | `crc_dnp` | `[[u16; 256]; 7]` | 3584 B |
//! | `crc_32` | `[[u32; 256]; 7]` | 7168 B |
//! | | **total** | **23 296 B** |
//!
//! 23 KiB of flash is nothing on a server and a great deal on a microcontroller, which
//! is exactly libcrc's audience. So this whole module sits behind the **`slice8`**
//! cargo feature. It is on by default; `--no-default-features` compiles the identity
//! shims at the bottom of this file instead, the byte-at-a-time path is restored
//! verbatim, and not one byte of these tables is emitted. The full test suite is run
//! both ways.
//!
//! # What is deliberately *not* accelerated
//!
//! * **`crc_sick` cannot be.** Its step is
//!   `crc = ((crc << 1) ^ POLY_SICK_if_msb_set) ^ (byte | prev_byte << 8)` — it mixes in
//!   the *previous* byte as well as the current one, so the message is not consumed as a
//!   stream of independent bytes and the fold is not a plain linear byte fold. The
//!   superposition argument above never gets off the ground: there is no `T_k[i]`,
//!   because the contribution of byte `i` depends on its neighbour. Slice-by-8 would
//!   need a table indexed by byte *pairs*, which is a different algorithm, not this one.
//!   `crc_sick` keeps the exact loop it always had.
//! * **`crc_64_ecma` / `crc_64_we` are left alone on purpose.** The maths works — the
//!   forward derivation above applies unchanged at `W = 64` — but the tables would cost
//!   another 14 336 B, the two functions were already at or above parity with the C
//!   original in `bench/results.json`, and speeding up something that is not slow is not
//!   worth 14 KiB of someone's flash. Adding it later is a copy of `forward_slices_u16`
//!   with the types widened.
//! * **The single-byte `update_crc_*` functions are untouched.** They are libcrc's
//!   actual API, they are what `tests/parity/` certifies exhaustively, and a caller
//!   handing over one byte at a time has given the accelerator nothing to work with.
//!   They remain the reference the tests below measure against.
//! * **`checksum_nmea`** is a delimiter-driven XOR, not a CRC. Nothing to fold.

// ===========================================================================
// Accelerated implementation — cargo feature `slice8` (default on)
// ===========================================================================

#[cfg(feature = "slice8")]
mod accelerated {
    use crate::tables::SHT75_CRC_TABLE;
    use crate::{
        forward_table_u16, reflected_table_u16, reflected_table_u32, POLY_16, POLY_32, POLY_CCITT,
        POLY_DNP, POLY_KERMIT, TABLE_16, TABLE_32, TABLE_CCITT, TABLE_DNP, TABLE_KERMIT,
    };

    /// Bytes consumed per iteration of the folded loop.
    const STRIDE: usize = 8;

    /// Number of derived tables held here. `T_0` lives in `lib.rs` and is reused.
    const DERIVED: usize = STRIDE - 1;

    // -----------------------------------------------------------------------
    // Where to switch over — MEASURED, not guessed.
    //
    // Two effects compete. The folded loop does nothing until it has a whole
    // 8-byte block, and it touches eight tables where the byte loop touches one,
    // so a short call can pay for cache lines it never amortises. Against that,
    // even two blocks beats an eight-deep dependency chain.
    //
    // Speedup of the folded path over the byte-at-a-time path, p50 of 61 samples,
    // release profile (opt-level 3, LTO, codegen-units 1), rustc 1.96.0
    // x86_64-pc-windows-gnu, on the machine described in bench/methodology.md.
    // Same calibration rule as bench/rust/src/main.rs. Reproduce by setting both
    // thresholds to 8 and timing the public API at each length.
    //
    //     bytes   crc_8   crc_16  crc_32  ccitt   kermit  dnp
    //         8   3.000   0.959   0.878   0.943   0.970   0.924
    //        11   1.593   0.780   0.766   0.790   0.779   0.788
    //        15   1.558   0.735   0.710   0.726   0.754   0.758
    //        16   2.544   1.608   1.462   1.567   1.603   1.546   <-- crossover
    //        23   1.651   1.101   1.050   1.103   1.105   1.078
    //        32   2.967   2.579   2.143   2.483   2.490   2.358
    //
    // The 16-/32-bit algorithms LOSE 5-29% between 8 and 15 bytes and win from 16
    // upwards without ever dipping back — so the threshold is 16, not 8. `crc_8`
    // has a one-byte state that a single block already amortises and wins at every
    // length from 8 up (worst case 1.457x at 13 B), so it switches over at 8.
    // Giving it the same 16 as the others would throw away a 1.5-2.5x win on 8-15
    // byte inputs, which is precisely the size of a sensor packet.
    // -----------------------------------------------------------------------

    /// Crossover for the 16- and 32-bit algorithms.
    const MIN_LEN: usize = 16;

    /// Crossover for `crc_8`, whose 8-bit state is annihilated by a single byte.
    const MIN_LEN_8: usize = STRIDE;

    // -----------------------------------------------------------------------
    // Table construction — `const fn`, evaluated by the compiler into `.rodata`,
    // exactly like the byte-at-a-time tables. Nothing is built at run time and
    // there is no lazy initialisation to race on.
    // -----------------------------------------------------------------------

    /// CRC-8 with polynomial `0x31`, MSB-first.
    ///
    /// `tables.rs` holds the SHT7x table *transcribed* from the Sensirion datasheet
    /// rather than computed, and Rust cannot read a `static` during const evaluation,
    /// so the derived slices are built from the polynomial instead. That introduces a
    /// second description of the same map, so `derived_crc8_table_matches_transcribed`
    /// pins them together: if they ever disagree, a test fails rather than a checksum
    /// silently changing. The byte-at-a-time path still uses the transcribed table, and
    /// so does the `T_0` term of the folded loop.
    const POLY_8: u8 = 0x31;

    const fn forward_table_u8(poly: u8) -> [u8; 256] {
        let mut table = [0u8; 256];
        let mut index = 0usize;
        while index < 256 {
            let mut crc = index as u8;
            let mut bit = 0;
            while bit < 8 {
                if crc & 0x80 != 0 {
                    crc = (crc << 1) ^ poly;
                } else {
                    crc <<= 1;
                }
                bit += 1;
            }
            table[index] = crc;
            index += 1;
        }
        table
    }

    /// `T_1..T_7` for a width-8 forward CRC. `shift(v) = T_0[v]`.
    const fn slices_u8() -> [[u8; 256]; DERIVED] {
        let t0 = forward_table_u8(POLY_8);
        let mut slices = [[0u8; 256]; DERIVED];
        let mut k = 0;
        while k < DERIVED {
            let mut i = 0;
            while i < 256 {
                let prev = if k == 0 { t0[i] } else { slices[k - 1][i] };
                slices[k][i] = t0[prev as usize];
                i += 1;
            }
            k += 1;
        }
        slices
    }

    /// `T_1..T_7` for a reflected 16-bit CRC. `shift(v) = (v >> 8) ^ T_0[v & 0xFF]`.
    const fn reflected_slices_u16(poly: u16) -> [[u16; 256]; DERIVED] {
        let t0 = reflected_table_u16(poly);
        let mut slices = [[0u16; 256]; DERIVED];
        let mut k = 0;
        while k < DERIVED {
            let mut i = 0;
            while i < 256 {
                let prev = if k == 0 { t0[i] } else { slices[k - 1][i] };
                slices[k][i] = (prev >> 8) ^ t0[(prev & 0x00FF) as usize];
                i += 1;
            }
            k += 1;
        }
        slices
    }

    /// `T_1..T_7` for a forward 16-bit CRC. `shift(v) = (v << 8) ^ T_0[v >> 8]`.
    const fn forward_slices_u16(poly: u16) -> [[u16; 256]; DERIVED] {
        let t0 = forward_table_u16(poly);
        let mut slices = [[0u16; 256]; DERIVED];
        let mut k = 0;
        while k < DERIVED {
            let mut i = 0;
            while i < 256 {
                let prev = if k == 0 { t0[i] } else { slices[k - 1][i] };
                slices[k][i] = (prev << 8) ^ t0[(prev >> 8) as usize];
                i += 1;
            }
            k += 1;
        }
        slices
    }

    /// `T_1..T_7` for a reflected 32-bit CRC. `shift(v) = (v >> 8) ^ T_0[v & 0xFF]`.
    const fn reflected_slices_u32(poly: u32) -> [[u32; 256]; DERIVED] {
        let t0 = reflected_table_u32(poly);
        let mut slices = [[0u32; 256]; DERIVED];
        let mut k = 0;
        while k < DERIVED {
            let mut i = 0;
            while i < 256 {
                let prev = if k == 0 { t0[i] } else { slices[k - 1][i] };
                slices[k][i] = (prev >> 8) ^ t0[(prev & 0x0000_00FF) as usize];
                i += 1;
            }
            k += 1;
        }
        slices
    }

    static SLICES_8: [[u8; 256]; DERIVED] = slices_u8();
    static SLICES_16: [[u16; 256]; DERIVED] = reflected_slices_u16(POLY_16);
    static SLICES_KERMIT: [[u16; 256]; DERIVED] = reflected_slices_u16(POLY_KERMIT);
    static SLICES_DNP: [[u16; 256]; DERIVED] = reflected_slices_u16(POLY_DNP);
    static SLICES_CCITT: [[u16; 256]; DERIVED] = forward_slices_u16(POLY_CCITT);
    static SLICES_32: [[u32; 256]; DERIVED] = reflected_slices_u32(POLY_32);

    // -----------------------------------------------------------------------
    // The folded loops.
    //
    // Each returns the folded CRC plus whatever bytes did not fill a whole block,
    // so the caller finishes with the same byte-at-a-time loop it always used. One
    // tail path, shared with the unaccelerated build — there is no second copy of
    // the finalisation rules to get wrong.
    //
    // `split_first_chunk::<8>` yields a `&[u8; 8]`, so the eight message bytes are
    // read through a fixed-size array with no bounds check and no panic path.
    // -----------------------------------------------------------------------

    #[inline]
    fn run_u8(mut crc: u8, mut data: &[u8]) -> (u8, &[u8]) {
        while let Some((b, rest)) = data.split_first_chunk::<STRIDE>() {
            // W = 8: the state is annihilated by a single byte, so only b[0] is
            // XORed into it.
            crc = SLICES_8[6][(crc ^ b[0]) as usize]
                ^ SLICES_8[5][b[1] as usize]
                ^ SLICES_8[4][b[2] as usize]
                ^ SLICES_8[3][b[3] as usize]
                ^ SLICES_8[2][b[4] as usize]
                ^ SLICES_8[1][b[5] as usize]
                ^ SLICES_8[0][b[6] as usize]
                ^ SHT75_CRC_TABLE[b[7] as usize];
            data = rest;
        }
        (crc, data)
    }

    #[inline]
    fn run_reflected_u16<'a>(
        mut crc: u16,
        mut data: &'a [u8],
        slices: &[[u16; 256]; DERIVED],
        t0: &[u16; 256],
    ) -> (u16, &'a [u8]) {
        while let Some((b, rest)) = data.split_first_chunk::<STRIDE>() {
            // Reflected: the low byte of the state is consumed first, so the first
            // two message bytes enter little-end first. `from_le_bytes` states that
            // explicitly and is therefore correct on a big-endian target too.
            let x = crc ^ u16::from_le_bytes([b[0], b[1]]);
            crc = slices[6][(x & 0x00FF) as usize]
                ^ slices[5][(x >> 8) as usize]
                ^ slices[4][b[2] as usize]
                ^ slices[3][b[3] as usize]
                ^ slices[2][b[4] as usize]
                ^ slices[1][b[5] as usize]
                ^ slices[0][b[6] as usize]
                ^ t0[b[7] as usize];
            data = rest;
        }
        (crc, data)
    }

    #[inline]
    fn run_forward_u16(mut crc: u16, mut data: &[u8]) -> (u16, &[u8]) {
        while let Some((b, rest)) = data.split_first_chunk::<STRIDE>() {
            // Forward: the high byte of the state is consumed first, hence big-end
            // first here. Again spelled out rather than relying on host endianness.
            let x = crc ^ u16::from_be_bytes([b[0], b[1]]);
            crc = SLICES_CCITT[6][(x >> 8) as usize]
                ^ SLICES_CCITT[5][(x & 0x00FF) as usize]
                ^ SLICES_CCITT[4][b[2] as usize]
                ^ SLICES_CCITT[3][b[3] as usize]
                ^ SLICES_CCITT[2][b[4] as usize]
                ^ SLICES_CCITT[1][b[5] as usize]
                ^ SLICES_CCITT[0][b[6] as usize]
                ^ TABLE_CCITT[b[7] as usize];
            data = rest;
        }
        (crc, data)
    }

    #[inline]
    fn run_reflected_u32(mut crc: u32, mut data: &[u8]) -> (u32, &[u8]) {
        while let Some((b, rest)) = data.split_first_chunk::<STRIDE>() {
            let x = crc ^ u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            crc = SLICES_32[6][(x & 0x0000_00FF) as usize]
                ^ SLICES_32[5][((x >> 8) & 0x0000_00FF) as usize]
                ^ SLICES_32[4][((x >> 16) & 0x0000_00FF) as usize]
                ^ SLICES_32[3][(x >> 24) as usize]
                ^ SLICES_32[2][b[4] as usize]
                ^ SLICES_32[1][b[5] as usize]
                ^ SLICES_32[0][b[6] as usize]
                ^ TABLE_32[b[7] as usize];
            data = rest;
        }
        (crc, data)
    }

    // -----------------------------------------------------------------------
    // Length-dispatching entry points used by lib.rs.
    // -----------------------------------------------------------------------

    pub(crate) fn fold_8(crc: u8, data: &[u8]) -> (u8, &[u8]) {
        if data.len() < MIN_LEN_8 {
            return (crc, data);
        }
        run_u8(crc, data)
    }

    pub(crate) fn fold_16(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        if data.len() < MIN_LEN {
            return (crc, data);
        }
        run_reflected_u16(crc, data, &SLICES_16, &TABLE_16)
    }

    pub(crate) fn fold_kermit(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        if data.len() < MIN_LEN {
            return (crc, data);
        }
        run_reflected_u16(crc, data, &SLICES_KERMIT, &TABLE_KERMIT)
    }

    pub(crate) fn fold_dnp(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        if data.len() < MIN_LEN {
            return (crc, data);
        }
        run_reflected_u16(crc, data, &SLICES_DNP, &TABLE_DNP)
    }

    pub(crate) fn fold_ccitt(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        if data.len() < MIN_LEN {
            return (crc, data);
        }
        run_forward_u16(crc, data)
    }

    pub(crate) fn fold_32(crc: u32, data: &[u8]) -> (u32, &[u8]) {
        if data.len() < MIN_LEN {
            return (crc, data);
        }
        run_reflected_u32(crc, data)
    }

    // =======================================================================
    // Tests
    //
    // The reference on the right-hand side of every assertion is built from the
    // public single-byte `update_crc_*` primitives, which this module does not
    // touch and which `tests/parity/` certifies EXHAUSTIVELY against the original
    // C library. So "slice-by-8 equals the byte loop" chains onto "the byte loop
    // equals libcrc" and the accelerated path inherits that evidence.
    //
    // The `run_*` functions are called directly, bypassing MIN_LEN, so short
    // lengths genuinely exercise the folded loop instead of silently falling
    // through to the byte path and passing for the wrong reason.
    //
    // NEGATIVE CONTROL — a test that cannot fail proves nothing, so these were
    // run against six deliberately broken builds. Every one was caught:
    //
    //   mutation                                        tests failed
    //   u32 fold: slices 5 and 4 transposed                        9
    //   ccitt fold: from_be_bytes -> from_le_bytes                13
    //   reflected-16 fold: the two halves of X swapped            18
    //   u8 fold: T_0 term taken from SLICES_8[0]                   7
    //   reflected-16 table recurrence: `>> 8` -> `<< 8`           19
    //   u32 fold: last byte b[7] -> b[6]                           7
    //
    // Reproduce by applying any one of those edits and running `cargo test`.
    // =======================================================================
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{
            crc_16, crc_32, crc_8, crc_ccitt_1d0f, crc_ccitt_ffff, crc_dnp, crc_kermit, crc_modbus,
            crc_xmodem, update_crc_16, update_crc_32, update_crc_8, update_crc_ccitt,
            update_crc_dnp, update_crc_kermit,
        };

        /// SplitMix64 — same generator as `tests/properties.rs`, so a failure here is
        /// reproducible from the seed alone with no dependency on any crate.
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
        }

        const SEED: u64 = 0x5111_CE88_0FF5_1CE8;

        fn ref_8(crc: u8, data: &[u8]) -> u8 {
            data.iter().fold(crc, |c, &b| update_crc_8(c, b))
        }
        fn ref_16(crc: u16, data: &[u8]) -> u16 {
            data.iter().fold(crc, |c, &b| update_crc_16(c, b))
        }
        fn ref_kermit(crc: u16, data: &[u8]) -> u16 {
            data.iter().fold(crc, |c, &b| update_crc_kermit(c, b))
        }
        fn ref_dnp(crc: u16, data: &[u8]) -> u16 {
            data.iter().fold(crc, |c, &b| update_crc_dnp(c, b))
        }
        fn ref_ccitt(crc: u16, data: &[u8]) -> u16 {
            data.iter().fold(crc, |c, &b| update_crc_ccitt(c, b))
        }
        fn ref_32(crc: u32, data: &[u8]) -> u32 {
            data.iter().fold(crc, |c, &b| update_crc_32(c, b))
        }

        /// Every seed each algorithm is actually started with, plus arbitrary ones —
        /// a mid-stream `crc` is unconstrained, so the identity has to hold for all of
        /// them, not just the documented initial values.
        const SEEDS_16: [u16; 6] = [0x0000, 0xFFFF, 0x1D0F, 0x8921, 0x0001, 0xA5C3];

        /// Check one buffer against the byte-at-a-time reference for every algorithm.
        fn assert_all_agree(data: &[u8], ctx: &str) {
            let (c, tail) = run_u8(0x00, data);
            assert!(tail.len() < 8, "{ctx}: crc_8 tail too long");
            assert_eq!(ref_8(c, tail), ref_8(0x00, data), "{ctx}: crc_8");

            for &seed in &SEEDS_16 {
                let (c, tail) = run_reflected_u16(seed, data, &SLICES_16, &TABLE_16);
                assert_eq!(
                    ref_16(c, tail),
                    ref_16(seed, data),
                    "{ctx}: crc_16 seed {seed:#06X}"
                );

                let (c, tail) = run_reflected_u16(seed, data, &SLICES_KERMIT, &TABLE_KERMIT);
                assert_eq!(
                    ref_kermit(c, tail),
                    ref_kermit(seed, data),
                    "{ctx}: kermit seed {seed:#06X}"
                );

                let (c, tail) = run_reflected_u16(seed, data, &SLICES_DNP, &TABLE_DNP);
                assert_eq!(
                    ref_dnp(c, tail),
                    ref_dnp(seed, data),
                    "{ctx}: dnp seed {seed:#06X}"
                );

                let (c, tail) = run_forward_u16(seed, data);
                assert_eq!(
                    ref_ccitt(c, tail),
                    ref_ccitt(seed, data),
                    "{ctx}: ccitt seed {seed:#06X}"
                );
            }

            for &seed in &[0x0000_0000u32, 0xFFFF_FFFF, 0x1234_5678, 0xDEAD_BEEF] {
                let (c, tail) = run_reflected_u32(seed, data);
                assert_eq!(
                    ref_32(c, tail),
                    ref_32(seed, data),
                    "{ctx}: crc_32 seed {seed:#010X}"
                );
            }
        }

        /// EVERY length from 0 to 300 — covering all eight tail remainders, the
        /// sub-block lengths where the folded loop never runs, and the boundary at
        /// each multiple of 8.
        #[test]
        fn slice_by_8_equals_byte_at_a_time_for_every_length_0_to_300() {
            let mut rng = Rng::new(SEED);
            let data = rng.bytes(301);
            for len in 0..=300usize {
                assert_all_agree(&data[..len], &format!("len {len}"));
            }
        }

        /// Same sweep on all-zero and all-ones input. Random bytes never produce a
        /// long run of an identical value, and a table indexing bug can hide behind
        /// that.
        #[test]
        fn slice_by_8_equals_byte_at_a_time_on_degenerate_input() {
            for len in 0..=300usize {
                assert_all_agree(&vec![0x00u8; len], &format!("zeros len {len}"));
                assert_all_agree(&vec![0xFFu8; len], &format!("ones len {len}"));
            }
        }

        /// Large buffers, at and around block boundaries, where the folded loop does
        /// essentially all of the work.
        #[test]
        fn slice_by_8_equals_byte_at_a_time_on_large_random_buffers() {
            let mut rng = Rng::new(SEED ^ 0x1234_5678_9ABC_DEF0);
            for &len in &[
                1_024usize, 4_096, 4_099, 65_536, 65_535, 65_537, 1_048_576, 1_048_583,
            ] {
                let data = rng.bytes(len);
                assert_all_agree(&data, &format!("large len {len}"));
            }
        }

        /// The public one-shot API — dispatch, finalisation and all — must equal the
        /// byte-at-a-time reference including the byte-swap and complement rules.
        /// Sweeps across MIN_LEN so both sides of the dispatch are covered.
        #[test]
        fn public_api_matches_byte_at_a_time_across_the_dispatch_threshold() {
            let mut rng = Rng::new(SEED ^ 0xF0F0_F0F0_F0F0_F0F0);
            let data = rng.bytes(600);
            for len in 0..=600usize {
                let d = &data[..len];
                let ctx = format!("len {len}");
                assert_eq!(crc_8(d), ref_8(0x00, d), "{ctx}: crc_8");
                assert_eq!(crc_16(d), ref_16(0x0000, d), "{ctx}: crc_16");
                assert_eq!(crc_modbus(d), ref_16(0xFFFF, d), "{ctx}: crc_modbus");
                assert_eq!(
                    crc_32(d),
                    ref_32(0xFFFF_FFFF, d) ^ 0xFFFF_FFFF,
                    "{ctx}: crc_32"
                );
                assert_eq!(crc_xmodem(d), ref_ccitt(0x0000, d), "{ctx}: crc_xmodem");
                assert_eq!(
                    crc_ccitt_ffff(d),
                    ref_ccitt(0xFFFF, d),
                    "{ctx}: crc_ccitt_ffff"
                );
                assert_eq!(
                    crc_ccitt_1d0f(d),
                    ref_ccitt(0x1D0F, d),
                    "{ctx}: crc_ccitt_1d0f"
                );
                assert_eq!(
                    crc_kermit(d),
                    ref_kermit(0x0000, d).swap_bytes(),
                    "{ctx}: crc_kermit"
                );
                assert_eq!(
                    crc_dnp(d),
                    (!ref_dnp(0x0000, d)).swap_bytes(),
                    "{ctx}: crc_dnp"
                );
            }
        }

        /// The CRC-8 slices are derived from polynomial `0x31`, while `lib.rs` uses the
        /// table transcribed from the Sensirion SHT7x datasheet. Those are two
        /// descriptions of one map; this pins them equal so the two halves of the fold
        /// can never drift apart.
        #[test]
        fn derived_crc8_table_matches_transcribed_sht75_table() {
            assert_eq!(
                forward_table_u8(POLY_8),
                SHT75_CRC_TABLE,
                "the SHT7x datasheet table is CRC-8 poly 0x31, MSB-first"
            );
        }

        /// `T_k` must be `T_(k-1)` pushed past one more zero byte. Checking the
        /// recurrence directly means a table bug shows up as a table failure rather
        /// than as a mysterious checksum mismatch.
        #[test]
        fn derived_slices_are_the_byte_table_shifted_by_k_zero_bytes() {
            for i in 0..256usize {
                // Each T_k[i] must equal "fold byte i, then k zero bytes".
                for k in 1..=DERIVED {
                    let mut v16 = TABLE_16[i];
                    let mut v32 = TABLE_32[i];
                    let mut vcc = TABLE_CCITT[i];
                    let mut v8 = SHT75_CRC_TABLE[i];
                    for _ in 0..k {
                        v16 = update_crc_16(v16, 0);
                        v32 = update_crc_32(v32, 0);
                        vcc = update_crc_ccitt(vcc, 0);
                        v8 = update_crc_8(v8, 0);
                    }
                    assert_eq!(SLICES_16[k - 1][i], v16, "SLICES_16[{k}][{i}]");
                    assert_eq!(SLICES_32[k - 1][i], v32, "SLICES_32[{k}][{i}]");
                    assert_eq!(SLICES_CCITT[k - 1][i], vcc, "SLICES_CCITT[{k}][{i}]");
                    assert_eq!(SLICES_8[k - 1][i], v8, "SLICES_8[{k}][{i}]");
                }
            }
        }

        /// A guard against the accelerator being accidentally disabled — if the folded
        /// loop stopped consuming blocks, every equality test above would still pass
        /// (both sides would be the byte loop) and the speedup would silently vanish
        /// with the suite still green.
        #[test]
        fn the_folded_loop_actually_consumes_whole_blocks() {
            let data = [0xA5u8; 64];
            let (_, tail) = run_reflected_u32(0, &data);
            assert!(tail.is_empty(), "64 bytes is 8 whole blocks");
            let (_, tail) = run_reflected_u32(0, &data[..7]);
            assert_eq!(tail.len(), 7, "7 bytes cannot fill a block");
            let (_, tail) = fold_32(0, &data);
            assert!(
                tail.is_empty(),
                "dispatch must reach the folded loop at 64 B"
            );
        }

        /// The measured thresholds are load-bearing — below them the folded path is
        /// slower, not faster. Pin them so a later edit cannot quietly re-introduce
        /// the 0.71-0.97x regression the sweep above found at 8-15 bytes.
        #[test]
        fn dispatch_honours_the_measured_crossovers() {
            let data = [0x5Au8; 64];

            for len in 0..MIN_LEN {
                let (crc, tail) = fold_32(0xFFFF_FFFF, &data[..len]);
                assert_eq!(crc, 0xFFFF_FFFF, "len {len} must not be folded");
                assert_eq!(tail.len(), len, "len {len} must be handed back whole");
            }
            let (_, tail) = fold_32(0xFFFF_FFFF, &data[..MIN_LEN]);
            assert_eq!(
                tail.len(),
                MIN_LEN % STRIDE,
                "at MIN_LEN every whole block is folded"
            );

            for len in 0..MIN_LEN_8 {
                let (crc, tail) = fold_8(0x00, &data[..len]);
                assert_eq!(crc, 0x00, "crc_8 len {len} must not be folded");
                assert_eq!(tail.len(), len);
            }
            let (_, tail) = fold_8(0x00, &data[..MIN_LEN_8]);
            assert!(tail.is_empty(), "crc_8 folds from a single whole block");
        }
    }
}

// ===========================================================================
// Identity shims — what `--no-default-features` compiles instead.
//
// Each returns the input untouched, so the caller's tail loop folds the entire
// buffer one byte at a time and the port behaves exactly as it did before this
// module existed. None of the tables above are emitted.
// ===========================================================================

#[cfg(not(feature = "slice8"))]
mod identity {
    #[inline]
    pub(crate) fn fold_8(crc: u8, data: &[u8]) -> (u8, &[u8]) {
        (crc, data)
    }
    #[inline]
    pub(crate) fn fold_16(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        (crc, data)
    }
    #[inline]
    pub(crate) fn fold_kermit(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        (crc, data)
    }
    #[inline]
    pub(crate) fn fold_dnp(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        (crc, data)
    }
    #[inline]
    pub(crate) fn fold_ccitt(crc: u16, data: &[u8]) -> (u16, &[u8]) {
        (crc, data)
    }
    #[inline]
    pub(crate) fn fold_32(crc: u32, data: &[u8]) -> (u32, &[u8]) {
        (crc, data)
    }
}

#[cfg(feature = "slice8")]
pub(crate) use accelerated::{fold_16, fold_32, fold_8, fold_ccitt, fold_dnp, fold_kermit};

#[cfg(not(feature = "slice8"))]
pub(crate) use identity::{fold_16, fold_32, fold_8, fold_ccitt, fold_dnp, fold_kermit};
