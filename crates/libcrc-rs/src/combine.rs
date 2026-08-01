//! CRC combination over GF(2) — a capability the original C library does not have.
//!
//! Given `CRC(A)`, `CRC(B)` and `len(B)`, compute `CRC(A ‖ B)` **without re-reading
//! either buffer**. This makes chunked and parallel CRC computation possible: hash
//! blocks independently, then stitch the results together in `O(log n)` matrix
//! squarings rather than `O(n)` byte folds.
//!
//! libcrc offers no way to do this. Its API is one-shot or byte-at-a-time only, so a
//! caller who has already hashed two blocks must re-scan to get the combined value.
//!
//! # How it works
//!
//! Folding one bit into a reflected CRC is a linear map over GF(2), so it can be
//! written as a 32×32 bit-matrix acting on the CRC register. Advancing by `n` zero
//! bits is that matrix raised to the `n`-th power. Repeated squaring gets there in
//! `log2(n)` steps, so combining is logarithmic in the length of the second buffer
//! rather than linear.

/// Number of bits in the CRC register we combine over.
const WIDTH: usize = 32;

/// Multiply a GF(2) vector by a GF(2) matrix (XOR of the rows selected by set bits).
const fn matrix_times(matrix: &[u32; WIDTH], mut vec: u32) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < WIDTH && vec != 0 {
        if vec & 1 != 0 {
            sum ^= matrix[i];
        }
        vec >>= 1;
        i += 1;
    }
    sum
}

/// Square a GF(2) matrix: `square = mat * mat`.
const fn matrix_square(mat: &[u32; WIDTH]) -> [u32; WIDTH] {
    let mut square = [0u32; WIDTH];
    let mut i = 0;
    while i < WIDTH {
        square[i] = matrix_times(mat, mat[i]);
        i += 1;
    }
    square
}

/// The operator for advancing a reflected CRC-32 by a single zero bit.
const fn single_bit_operator(poly: u32) -> [u32; WIDTH] {
    let mut odd = [0u32; WIDTH];
    odd[0] = poly;
    let mut row = 1u32;
    let mut i = 1;
    while i < WIDTH {
        odd[i] = row;
        row <<= 1;
        i += 1;
    }
    odd
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
    if len_b == 0 {
        return crc_a;
    }

    // `odd` starts as the one-bit operator; two squarings make it the four-bit
    // operator, so the first squaring inside the loop yields the one-BYTE operator.
    let mut odd = single_bit_operator(super::POLY_32);
    let mut even = matrix_square(&odd);
    odd = matrix_square(&even);

    // Consume `len_b` bit by bit, squaring the operator at each step: log2(len_b)
    // squarings instead of len_b byte folds.
    let mut crc = crc_a;
    let mut len = len_b;
    loop {
        even = matrix_square(&odd);
        if len & 1 != 0 {
            crc = matrix_times(&even, crc);
        }
        len >>= 1;
        if len == 0 {
            break;
        }

        odd = matrix_square(&even);
        if len & 1 != 0 {
            crc = matrix_times(&odd, crc);
        }
        len >>= 1;
        if len == 0 {
            break;
        }
    }

    crc ^ crc_b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc_32;

    fn concat(a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut v = Vec::from(a);
        v.extend_from_slice(b);
        v
    }

    /// The defining property: combining must equal hashing the concatenation.
    #[test]
    fn combine_equals_concatenation() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"", b"abc"),
            (b"abc", b""),
            (b"a", b"b"),
            (b"123456789", b"123456789"),
            (b"the quick brown ", b"fox jumps over the lazy dog"),
        ];
        for (a, b) in cases {
            let joined = concat(a, b);
            assert_eq!(
                crc_32_combine(crc_32(a), crc_32(b), b.len()),
                crc_32(&joined),
                "combine failed for {:?} ++ {:?}",
                a,
                b
            );
        }
    }

    /// Property sweep over many split points of one buffer: for every possible split,
    /// combining the halves must reproduce the whole.
    #[test]
    fn combine_is_associative_over_every_split_point() {
        let data: Vec<u8> = (0u32..=512)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let whole = crc_32(&data);
        for split in 0..data.len() {
            let (a, b) = data.split_at(split);
            assert_eq!(
                crc_32_combine(crc_32(a), crc_32(b), b.len()),
                whole,
                "split at {} did not reproduce the whole",
                split
            );
        }
    }

    /// Three-way combination must also hold, which exercises the operator powers.
    #[test]
    fn combine_chains_across_three_blocks() {
        let (a, b, c) = (
            b"alpha".as_slice(),
            b"bravo!!".as_slice(),
            b"charlie123".as_slice(),
        );
        let ab = crc_32_combine(crc_32(a), crc_32(b), b.len());
        let abc = crc_32_combine(ab, crc_32(c), c.len());
        let joined = concat(&concat(a, b), c);
        assert_eq!(abc, crc_32(&joined));
    }
}
