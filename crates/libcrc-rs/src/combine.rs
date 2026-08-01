//! CRC combination over GF(2) — a capability the original C library does not have.
//!
//! Given `CRC(A)`, `CRC(B)` and `len(B)`, compute `CRC(A ‖ B)` **without re-reading
//! either buffer**. This makes chunked and parallel CRC computation possible: hash
//! blocks independently, then stitch the results together in `O(log n)` matrix
//! squarings rather than `O(n)` byte folds.
//!
//! libcrc offers no way to do this. Its API is one-shot or byte-at-a-time only, so a
//! caller who has already hashed two blocks must re-scan to get the combined value.
//! zlib ships `crc32_combine()` for exactly one algorithm; this module generalises the
//! construction to **ten of libcrc's eleven checksums**, across four register widths and
//! both bit orders.
//!
//! # Why it works
//!
//! Every table-driven CRC here folds a byte with `R' = f(R) ⊕ g(b)`, where `f` (advance
//! the register past one zero byte) and `g` are both linear over GF(2). Composing that
//! across a message of `k` bytes gives
//!
//! ```text
//! raw(M, init) = L_k(init) ⊕ raw(M, 0)         where L_k = f∘f∘…∘f, k times
//! ```
//!
//! and splitting a message at any point gives
//!
//! ```text
//! raw(A ‖ B, init) = L_n(raw(A, init)) ⊕ raw(B, 0)          n = len(B)
//! ```
//!
//! Writing the finalised value as `crc(M) = raw(M, init) ⊕ xorout` and eliminating
//! `raw(B, 0)` between the two yields the identity this module implements:
//!
//! ```text
//! crc(A ‖ B) = L_n( crc(A) ⊕ init ⊕ xorout ) ⊕ crc(B)
//! ```
//!
//! The `init ⊕ xorout` correction is what makes the construction general, and it is the
//! reason this is not just zlib's routine with the constants swapped. zlib may omit it
//! because CRC-32 happens to have `init == xorout`; MODBUS, DNP and the CCITT pair do
//! not, and dropping the term there produces silently wrong answers. There is a test
//! below that asserts exactly that.
//!
//! # Where the operator comes from
//!
//! `L_1` is **not** re-derived from a polynomial. Each of its rows is obtained by calling
//! the port's own byte-fold on a basis vector — `update_crc_16(1 << i, 0)` and friends —
//! so the combine operator cannot drift away from the digest it has to agree with. It
//! also means [`crc_8_combine`] works without anybody having to reverse-engineer a
//! polynomial for the Sensirion SHT7x table, which libcrc transcribes rather than
//! derives. Every operator is a `const`, evaluated by the compiler into `.rodata`.
//!
//! `L_n` is `L_1` raised to the `n`-th power by binary exponentiation: bit `k` of `n`
//! selects the operator for `2^k` bytes, so combining costs `O(log n)` squarings of a
//! `w × w` bit matrix and touches neither buffer.
//!
//! # Not provided, deliberately
//!
//! * **`crc_sick`** — its fold mixes in the *previous* byte as well as the current one,
//!   so the register at a block boundary is not a sufficient summary of what came
//!   before: continuing into `B` requires the last byte of `A`, which `crc_sick(A)` does
//!   not carry. No function of `(crc(A), crc(B), len(B))` can recover it, so no combine
//!   is offered rather than one that is quietly wrong.
//! * **`checksum_nmea`** — not a CRC and not length-driven. It strips a leading `$` and
//!   stops at the first `*`, `\r`, `\n` or NUL, so concatenation is not even well defined
//!   for it and a `combine` would be meaningless.
//!
//! # Cost
//!
//! Combining is `O(log n)` in time and allocates nothing, but it does hold two `w × w`
//! bit matrices on the stack: 128 bytes for the 16-bit CRCs, 256 for CRC-32 and 1 KiB for
//! CRC-64. That is worth knowing on a small `no_std` target.

// ===========================================================================
// GF(2) machinery, generated once per register width
// ===========================================================================

/// Generates the GF(2) matrix machinery for one CRC register width.
///
/// A `w`-bit CRC register is a vector over GF(2), so "advance past one zero byte" is a
/// linear map and can be stored as a `w × w` bit matrix — one row per register bit,
/// packed into one word.
macro_rules! gf2_machinery {
    ($ty:ty, $bits:literal, $times:ident, $square:ident, $advance:ident, $combine:ident) => {
        /// Apply a GF(2) matrix to a vector: XOR together the rows its set bits select.
        const fn $times(matrix: &[$ty; $bits], mut vec: $ty) -> $ty {
            let mut sum: $ty = 0;
            let mut i = 0;
            while i < $bits && vec != 0 {
                if vec & 1 != 0 {
                    sum ^= matrix[i];
                }
                vec >>= 1;
                i += 1;
            }
            sum
        }

        /// Compose a GF(2) operator with itself. Squaring "advance `k` bytes" gives
        /// "advance `2k` bytes", which is what makes combining logarithmic.
        const fn $square(matrix: &[$ty; $bits]) -> [$ty; $bits] {
            let mut squared: [$ty; $bits] = [0; $bits];
            let mut i = 0;
            while i < $bits {
                squared[i] = $times(matrix, matrix[i]);
                i += 1;
            }
            squared
        }

        /// Advance `reg` past `len` zero bytes in `O(log len)` squarings, by binary
        /// exponentiation of the one-byte operator.
        fn $advance(one_byte: &[$ty; $bits], mut reg: $ty, mut len: usize) -> $ty {
            let mut power = *one_byte;
            loop {
                if len & 1 != 0 {
                    reg = $times(&power, reg);
                }
                len >>= 1;
                if len == 0 {
                    return reg;
                }
                power = $square(&power);
            }
        }

        /// `crc(A ‖ B) = L_n(crc(A) ⊕ init ⊕ xorout) ⊕ crc(B)` — see the module docs.
        ///
        /// The identity degenerates correctly at `len_b == 0` (where `L_0` is the
        /// identity map and `crc(B)` is exactly `init ⊕ xorout`); returning early is
        /// just a shortcut past the arithmetic.
        fn $combine(
            one_byte: &[$ty; $bits],
            crc_a: $ty,
            crc_b: $ty,
            len_b: usize,
            init_xor_xorout: $ty,
        ) -> $ty {
            if len_b == 0 {
                return crc_a;
            }
            $advance(one_byte, crc_a ^ init_xor_xorout, len_b) ^ crc_b
        }
    };
}

gf2_machinery!(u8, 8, times_u8, square_u8, advance_u8, combine_u8);
gf2_machinery!(u16, 16, times_u16, square_u16, advance_u16, combine_u16);
gf2_machinery!(u32, 32, times_u32, square_u32, advance_u32, combine_u32);
gf2_machinery!(u64, 64, times_u64, square_u64, advance_u64, combine_u64);

// ===========================================================================
// The one-byte operators, built from the port's own byte-folds
// ===========================================================================

/// Builds the "advance past one zero byte" operator by calling the port's own byte-fold
/// on each basis vector. Deriving it this way rather than from the polynomial means the
/// operator cannot disagree with the digest it is required to match — and it works for
/// the SHT7x CRC-8, whose table libcrc states no polynomial for.
macro_rules! byte_operator {
    ($name:ident, $ty:ty, $bits:literal, $fold:path) => {
        const $name: [$ty; $bits] = {
            let mut operator: [$ty; $bits] = [0; $bits];
            let mut i = 0;
            while i < $bits {
                operator[i] = $fold(1 << i, 0);
                i += 1;
            }
            operator
        };
    };
}

byte_operator!(OP_8, u8, 8, super::update_crc_8);
byte_operator!(OP_16, u16, 16, super::update_crc_16);
byte_operator!(OP_KERMIT, u16, 16, super::update_crc_kermit);
byte_operator!(OP_DNP, u16, 16, super::update_crc_dnp);
byte_operator!(OP_CCITT, u16, 16, super::update_crc_ccitt);
byte_operator!(OP_32, u32, 32, super::update_crc_32);
byte_operator!(OP_64, u64, 64, super::update_crc_64);

// ===========================================================================
// The `init ⊕ xorout` corrections
// ===========================================================================

// libcrc writes its finalisation XORs inline in the one-shot functions rather than
// naming them, so they are named here and each is annotated with the `xorout` it came
// from. Every one of these is auditable against the matching function in `lib.rs`.

/// `crc_8`: init `0x00`, no final XOR.
const FIX_8: u8 = super::START_8;
/// `crc_16`: init `0x0000`, no final XOR.
const FIX_16: u16 = super::START_16;
/// `crc_modbus`: init `0xFFFF`, no final XOR — the correction is *not* zero.
const FIX_MODBUS: u16 = super::START_MODBUS;
/// `crc_kermit`: init `0x0000`, no final XOR (the byte swap is handled separately).
const FIX_KERMIT: u16 = super::START_KERMIT;
/// `crc_dnp`: init `0x0000`, and libcrc complements before swapping, so `xorout` is
/// `0xFFFF`.
const FIX_DNP: u16 = super::START_DNP ^ 0xFFFF;
/// `crc_xmodem`: init `0x0000`, no final XOR.
const FIX_XMODEM: u16 = super::START_XMODEM;
/// `crc_ccitt_ffff`: init `0xFFFF`, no final XOR.
const FIX_CCITT_FFFF: u16 = super::START_CCITT_FFFF;
/// `crc_ccitt_1d0f`: init `0x1D0F`, no final XOR.
const FIX_CCITT_1D0F: u16 = super::START_CCITT_1D0F;
/// `crc_32`: init and final XOR are both `0xFFFF_FFFF`, so they cancel. This is the one
/// case zlib's uncorrected formula gets away with.
const FIX_32: u32 = super::START_32 ^ 0xFFFF_FFFF;
/// `crc_64_ecma`: init `0`, no final XOR.
const FIX_64_ECMA: u64 = super::START_64_ECMA;
/// `crc_64_we`: init and final XOR are both all-ones, so they cancel.
const FIX_64_WE: u64 = super::START_64_WE ^ 0xFFFF_FFFF_FFFF_FFFF;

// ===========================================================================
// Public API — one `*_combine` per algorithm the construction applies to
// ===========================================================================

/// Combine two CRC-8 (Sensirion SHT7x) values: given `crc_a = crc_8(a)`,
/// `crc_b = crc_8(b)` and `len_b = b.len()`, return `crc_8(a ‖ b)`.
///
/// ```
/// # use libcrc_rs::{crc_8, crc_8_combine};
/// assert_eq!(
///     crc_8_combine(crc_8(b"12345"), crc_8(b"6789"), 4),
///     crc_8(b"123456789")
/// );
/// ```
pub fn crc_8_combine(crc_a: u8, crc_b: u8, len_b: usize) -> u8 {
    combine_u8(&OP_8, crc_a, crc_b, len_b, FIX_8)
}

/// Combine two CRC-16/ARC values: given `crc_a = crc_16(a)`, `crc_b = crc_16(b)` and
/// `len_b = b.len()`, return `crc_16(a ‖ b)` without touching either buffer.
///
/// ```
/// # use libcrc_rs::{crc_16, crc_16_combine};
/// assert_eq!(crc_16_combine(crc_16(b"12345"), crc_16(b"6789"), 4), 0xBB3D);
/// ```
pub fn crc_16_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    combine_u16(&OP_16, crc_a, crc_b, len_b, FIX_16)
}

/// Combine two CRC-16/MODBUS values.
///
/// MODBUS seeds with `0xFFFF` and applies no final XOR, so unlike CRC-32 it needs the
/// `init ⊕ xorout` correction term; the uncorrected zlib-style formula is wrong here.
///
/// ```
/// # use libcrc_rs::{crc_modbus, crc_modbus_combine};
/// assert_eq!(
///     crc_modbus_combine(crc_modbus(b"12345"), crc_modbus(b"6789"), 4),
///     0x4B37
/// );
/// ```
pub fn crc_modbus_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    combine_u16(&OP_16, crc_a, crc_b, len_b, FIX_MODBUS)
}

/// Combine two CRC-16/KERMIT values, as libcrc computes them.
///
/// libcrc byte-swaps Kermit's result (`0x8921` where the RevEng catalogue says `0x2189`),
/// and a byte swap is not the linear map the operator acts on. The swap is therefore
/// undone on the way in and reapplied on the way out, so this function combines the
/// values libcrc actually returns.
///
/// ```
/// # use libcrc_rs::{crc_kermit, crc_kermit_combine};
/// assert_eq!(
///     crc_kermit_combine(crc_kermit(b"12345"), crc_kermit(b"6789"), 4),
///     0x8921
/// );
/// ```
pub fn crc_kermit_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    super::byteswap(combine_u16(
        &OP_KERMIT,
        super::byteswap(crc_a),
        super::byteswap(crc_b),
        len_b,
        FIX_KERMIT,
    ))
}

/// Combine two CRC-16/DNP values, as libcrc computes them.
///
/// libcrc complements *then* byte-swaps DNP's result (`0x82EA` where the RevEng catalogue
/// says `0xEA82`). The swap is undone on the way in and reapplied on the way out; the
/// complement is an `xorout` of `0xFFFF` and folds into the correction term.
///
/// ```
/// # use libcrc_rs::{crc_dnp, crc_dnp_combine};
/// assert_eq!(crc_dnp_combine(crc_dnp(b"12345"), crc_dnp(b"6789"), 4), 0x82EA);
/// ```
pub fn crc_dnp_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    super::byteswap(combine_u16(
        &OP_DNP,
        super::byteswap(crc_a),
        super::byteswap(crc_b),
        len_b,
        FIX_DNP,
    ))
}

/// Combine two CRC-16/XMODEM values.
///
/// XMODEM is MSB-first, which needs a different operator from the reflected CRCs — but
/// the same construction, because the forward byte-fold is equally linear over GF(2).
///
/// ```
/// # use libcrc_rs::{crc_xmodem, crc_xmodem_combine};
/// assert_eq!(
///     crc_xmodem_combine(crc_xmodem(b"12345"), crc_xmodem(b"6789"), 4),
///     0x31C3
/// );
/// ```
pub fn crc_xmodem_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    combine_u16(&OP_CCITT, crc_a, crc_b, len_b, FIX_XMODEM)
}

/// Combine two CRC-CCITT values seeded with `0xFFFF`.
///
/// ```
/// # use libcrc_rs::{crc_ccitt_ffff, crc_ccitt_ffff_combine};
/// assert_eq!(
///     crc_ccitt_ffff_combine(crc_ccitt_ffff(b"12345"), crc_ccitt_ffff(b"6789"), 4),
///     0x29B1
/// );
/// ```
pub fn crc_ccitt_ffff_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    combine_u16(&OP_CCITT, crc_a, crc_b, len_b, FIX_CCITT_FFFF)
}

/// Combine two CRC-CCITT values seeded with `0x1D0F`.
///
/// ```
/// # use libcrc_rs::{crc_ccitt_1d0f, crc_ccitt_1d0f_combine};
/// assert_eq!(
///     crc_ccitt_1d0f_combine(crc_ccitt_1d0f(b"12345"), crc_ccitt_1d0f(b"6789"), 4),
///     0xE5CC
/// );
/// ```
pub fn crc_ccitt_1d0f_combine(crc_a: u16, crc_b: u16, len_b: usize) -> u16 {
    combine_u16(&OP_CCITT, crc_a, crc_b, len_b, FIX_CCITT_1D0F)
}

/// Combine two CRC-32 values: given `crc_a = crc_32(a)`, `crc_b = crc_32(b)` and
/// `len_b = b.len()`, return `crc_32(a ‖ b)` without touching either buffer.
///
/// ```
/// # use libcrc_rs::{crc_32, crc_32_combine};
/// let (a, b) = (b"the quick brown ".as_slice(), b"fox jumps over".as_slice());
/// let mut joined = a.to_vec();
/// joined.extend_from_slice(b);
/// assert_eq!(crc_32_combine(crc_32(a), crc_32(b), b.len()), crc_32(&joined));
/// ```
pub fn crc_32_combine(crc_a: u32, crc_b: u32, len_b: usize) -> u32 {
    combine_u32(&OP_32, crc_a, crc_b, len_b, FIX_32)
}

/// Combine two CRC-64/ECMA values.
///
/// ```
/// # use libcrc_rs::{crc_64_ecma, crc_64_ecma_combine};
/// assert_eq!(
///     crc_64_ecma_combine(crc_64_ecma(b"12345"), crc_64_ecma(b"6789"), 4),
///     crc_64_ecma(b"123456789")
/// );
/// ```
pub fn crc_64_ecma_combine(crc_a: u64, crc_b: u64, len_b: usize) -> u64 {
    combine_u64(&OP_64, crc_a, crc_b, len_b, FIX_64_ECMA)
}

/// Combine two CRC-64/WE values.
///
/// ```
/// # use libcrc_rs::{crc_64_we, crc_64_we_combine};
/// assert_eq!(
///     crc_64_we_combine(crc_64_we(b"12345"), crc_64_we(b"6789"), 4),
///     crc_64_we(b"123456789")
/// );
/// ```
pub fn crc_64_we_combine(crc_a: u64, crc_b: u64, len_b: usize) -> u64 {
    combine_u64(&OP_64, crc_a, crc_b, len_b, FIX_64_WE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crc_16, crc_32, crc_64_ecma, crc_64_we, crc_8, crc_ccitt_1d0f, crc_ccitt_ffff, crc_dnp,
        crc_kermit, crc_modbus, crc_xmodem,
    };

    fn concat(a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut v = Vec::from(a);
        v.extend_from_slice(b);
        v
    }

    /// Deterministic pseudo-random bytes — an xorshift64, so the buffer exercises every
    /// bit position rather than the narrow range a counter would produce.
    fn sample_data(len: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// The full proof obligation for one algorithm: every split point of a
    /// multi-hundred-byte buffer, three-way chaining, the empty-input edges, and the
    /// block lengths that drive the binary exponentiation hardest.
    macro_rules! combine_suite {
        ($suite:ident, $crc:ident, $combine:ident) => {
            mod $suite {
                use super::{concat, sample_data};
                use crate::{$combine, $crc};

                /// The defining property, checked at EVERY split point.
                #[test]
                fn reproduces_the_whole_at_every_split_point() {
                    let data = sample_data(517);
                    let whole = $crc(&data);
                    for split in 0..=data.len() {
                        let (a, b) = data.split_at(split);
                        assert_eq!(
                            $combine($crc(a), $crc(b), b.len()),
                            whole,
                            "{}: split at {} of {} did not reproduce the whole",
                            stringify!($combine),
                            split,
                            data.len()
                        );
                    }
                }

                /// Chaining three blocks exercises three different operator powers and
                /// feeds an already-combined value back in as an input.
                #[test]
                fn chains_across_three_blocks() {
                    let data = sample_data(300);
                    let (x, rest) = data.split_at(7);
                    let (y, z) = rest.split_at(101);
                    let xy = $combine($crc(x), $crc(y), y.len());
                    assert_eq!(
                        $combine(xy, $crc(z), z.len()),
                        $crc(&data),
                        "{}: three-way chain",
                        stringify!($combine)
                    );
                }

                /// Empty inputs on either side, and the one-byte edges.
                #[test]
                fn handles_empty_and_single_byte_edges() {
                    let cases: &[(&[u8], &[u8])] = &[
                        (b"", b""),
                        (b"", b"a"),
                        (b"a", b""),
                        (b"a", b"b"),
                        (b"", b"123456789"),
                        (b"123456789", b""),
                        (b"1", b"23456789"),
                    ];
                    for (a, b) in cases {
                        let joined = concat(a, b);
                        assert_eq!(
                            $combine($crc(a), $crc(b), b.len()),
                            $crc(&joined),
                            "{}: {:?} ++ {:?}",
                            stringify!($combine),
                            a,
                            b
                        );
                    }
                }

                /// Lengths either side of a power of two are where an off-by-one in the
                /// exponentiation would show up; 4096 forces twelve squarings.
                #[test]
                fn handles_power_of_two_and_long_blocks() {
                    for len_b in [1usize, 2, 3, 4, 7, 8, 15, 16, 255, 256, 257, 1024, 4096] {
                        let data = sample_data(37 + len_b);
                        let (a, b) = data.split_at(37);
                        assert_eq!(
                            $combine($crc(a), $crc(b), b.len()),
                            $crc(&data),
                            "{}: second block of {} bytes",
                            stringify!($combine),
                            len_b
                        );
                    }
                }
            }
        };
    }

    combine_suite!(crc_8_suite, crc_8, crc_8_combine);
    combine_suite!(crc_16_suite, crc_16, crc_16_combine);
    combine_suite!(crc_modbus_suite, crc_modbus, crc_modbus_combine);
    combine_suite!(crc_kermit_suite, crc_kermit, crc_kermit_combine);
    combine_suite!(crc_dnp_suite, crc_dnp, crc_dnp_combine);
    combine_suite!(crc_xmodem_suite, crc_xmodem, crc_xmodem_combine);
    combine_suite!(crc_ccitt_ffff_suite, crc_ccitt_ffff, crc_ccitt_ffff_combine);
    combine_suite!(crc_ccitt_1d0f_suite, crc_ccitt_1d0f, crc_ccitt_1d0f_combine);
    combine_suite!(crc_32_suite, crc_32, crc_32_combine);
    combine_suite!(crc_64_ecma_suite, crc_64_ecma, crc_64_ecma_combine);
    combine_suite!(crc_64_we_suite, crc_64_we, crc_64_we_combine);

    /// Every split of the canonical check string must still produce the value the
    /// ORIGINAL C library produces for the whole string. These constants came from
    /// running libcrc, not from a specification.
    #[test]
    fn golden_check_values_survive_every_split() {
        const CHECK: &[u8] = b"123456789";
        for split in 0..=CHECK.len() {
            let (a, b) = CHECK.split_at(split);
            let n = b.len();
            assert_eq!(
                crc_16_combine(crc_16(a), crc_16(b), n),
                0xBB3D,
                "@{}",
                split
            );
            assert_eq!(
                crc_modbus_combine(crc_modbus(a), crc_modbus(b), n),
                0x4B37,
                "@{}",
                split
            );
            assert_eq!(
                crc_xmodem_combine(crc_xmodem(a), crc_xmodem(b), n),
                0x31C3,
                "@{}",
                split
            );
            assert_eq!(
                crc_ccitt_ffff_combine(crc_ccitt_ffff(a), crc_ccitt_ffff(b), n),
                0x29B1,
                "@{}",
                split
            );
            assert_eq!(
                crc_ccitt_1d0f_combine(crc_ccitt_1d0f(a), crc_ccitt_1d0f(b), n),
                0xE5CC,
                "@{}",
                split
            );
            assert_eq!(
                crc_kermit_combine(crc_kermit(a), crc_kermit(b), n),
                0x8921,
                "@{}",
                split
            );
            assert_eq!(
                crc_dnp_combine(crc_dnp(a), crc_dnp(b), n),
                0x82EA,
                "@{}",
                split
            );
            assert_eq!(
                crc_32_combine(crc_32(a), crc_32(b), n),
                0xCBF4_3926,
                "@{}",
                split
            );
            // CRC-8 and the CRC-64 pair have no golden constant recorded for this
            // string, so they are checked against the port's own one-shot result — the
            // value the differential harness compares against the C library.
            assert_eq!(
                crc_8_combine(crc_8(a), crc_8(b), n),
                crc_8(CHECK),
                "@{}",
                split
            );
            assert_eq!(
                crc_64_ecma_combine(crc_64_ecma(a), crc_64_ecma(b), n),
                crc_64_ecma(CHECK),
                "@{}",
                split
            );
            assert_eq!(
                crc_64_we_combine(crc_64_we(a), crc_64_we(b), n),
                crc_64_we(CHECK),
                "@{}",
                split
            );
        }
    }

    /// The whole construction rests on one claim: each `OP_*` really is the port's own
    /// byte-fold expressed as a matrix. For the 8- and 16-bit registers that claim is
    /// checked against EVERY possible register value, not a sample.
    #[test]
    fn operators_match_the_byte_fold_exhaustively_for_8_and_16_bit() {
        for reg in 0..=u8::MAX {
            assert_eq!(
                times_u8(&OP_8, reg),
                crate::update_crc_8(reg, 0),
                "{reg:#04X}"
            );
        }
        for reg in 0..=u16::MAX {
            assert_eq!(
                times_u16(&OP_16, reg),
                crate::update_crc_16(reg, 0),
                "{reg:#06X}"
            );
            assert_eq!(
                times_u16(&OP_KERMIT, reg),
                crate::update_crc_kermit(reg, 0),
                "{reg:#06X}"
            );
            assert_eq!(
                times_u16(&OP_DNP, reg),
                crate::update_crc_dnp(reg, 0),
                "{reg:#06X}"
            );
            assert_eq!(
                times_u16(&OP_CCITT, reg),
                crate::update_crc_ccitt(reg, 0),
                "{reg:#06X}"
            );
        }
    }

    /// The same claim for the 32- and 64-bit registers, where exhaustion is impossible:
    /// a pseudo-random sweep plus the all-ones extreme.
    #[test]
    fn operators_match_the_byte_fold_for_32_and_64_bit() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..50_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let reg32 = state as u32;
            assert_eq!(times_u32(&OP_32, reg32), crate::update_crc_32(reg32, 0));
            assert_eq!(times_u64(&OP_64, state), crate::update_crc_64(state, 0));
        }
        assert_eq!(
            times_u32(&OP_32, u32::MAX),
            crate::update_crc_32(u32::MAX, 0)
        );
        assert_eq!(
            times_u64(&OP_64, u64::MAX),
            crate::update_crc_64(u64::MAX, 0)
        );
    }

    /// Negative control: the `init ⊕ xorout` correction is load-bearing. Dropping it —
    /// which is what porting zlib's CRC-32-only formula verbatim would do — must produce
    /// a WRONG answer for the algorithms whose init and xorout differ, and the right one
    /// for CRC-32, where they cancel.
    #[test]
    fn dropping_the_correction_term_breaks_the_algorithms_that_need_it() {
        let data = sample_data(64);
        let (a, b) = data.split_at(21);
        let n = b.len();

        let uncorrected = advance_u16(&OP_16, crc_modbus(a), n) ^ crc_modbus(b);
        assert_ne!(
            uncorrected,
            crc_modbus(&data),
            "MODBUS: correction term should have been load-bearing"
        );
        assert_eq!(
            crc_modbus_combine(crc_modbus(a), crc_modbus(b), n),
            crc_modbus(&data)
        );

        let uncorrected = advance_u16(&OP_CCITT, crc_ccitt_ffff(a), n) ^ crc_ccitt_ffff(b);
        assert_ne!(
            uncorrected,
            crc_ccitt_ffff(&data),
            "CCITT-FFFF: correction term should have been load-bearing"
        );
        assert_eq!(
            crc_ccitt_ffff_combine(crc_ccitt_ffff(a), crc_ccitt_ffff(b), n),
            crc_ccitt_ffff(&data)
        );

        // CRC-32 is the one case where init == xorout, so the uncorrected form is right.
        let uncorrected = advance_u32(&OP_32, crc_32(a), n) ^ crc_32(b);
        assert_eq!(uncorrected, crc_32(&data));
    }

    /// A combined value must not depend on the buffers at all — only on the two CRCs and
    /// the length. Combining the CRCs of two *different* buffers of the same length must
    /// give the CRC of that other concatenation.
    #[test]
    fn depends_only_on_the_two_crcs_and_the_length() {
        let left = sample_data(64);
        let right: Vec<u8> = sample_data(64).iter().map(|b| !b).collect();
        for (a, b) in [(&left, &right), (&right, &left)] {
            assert_eq!(
                crc_32_combine(crc_32(a), crc_32(b), b.len()),
                crc_32(&concat(a, b))
            );
            assert_eq!(
                crc_16_combine(crc_16(a), crc_16(b), b.len()),
                crc_16(&concat(a, b))
            );
        }
    }
}
