//! Streaming digests — an API shape the original C library does not provide.
//!
//! libcrc exposes only one-shot functions (`crc_16(ptr, len)`) and byte-at-a-time
//! updates (`update_crc_16(crc, c)`). Hashing a stream therefore means either
//! buffering the whole input or writing a manual byte loop and remembering each
//! algorithm's finalisation rule — the byte-swap for Kermit, the complement-then-swap
//! for DNP, the final XOR for CRC-32.
//!
//! These types own that finalisation, so the caller cannot get it wrong, and they
//! accept slices rather than single bytes so the hot loop stays in one place.
//!
//! Every digest is zero-allocation and `no_std`, matching libcrc's embedded audience.

use core::hash::Hasher;

/// Streaming CRC-16. Defaults to CRC-16/ARC; use [`Crc16Digest::modbus`] for MODBUS.
///
/// ```
/// # use libcrc_rs::Crc16Digest;
/// let mut d = Crc16Digest::new();
/// d.update(b"12345");
/// d.update(b"6789");
/// assert_eq!(d.finalize(), 0xBB3D);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crc16Digest {
    crc: u16,
}

impl Crc16Digest {
    /// CRC-16/ARC, seeded `0x0000`.
    pub const fn new() -> Self {
        Self { crc: 0x0000 }
    }

    /// CRC-16/MODBUS, seeded `0xFFFF`.
    pub const fn modbus() -> Self {
        Self { crc: 0xFFFF }
    }

    /// Absorb a chunk. May be called any number of times.
    pub fn update(&mut self, data: &[u8]) {
        self.crc = data.iter().fold(self.crc, |crc, &b| super::update_crc_16(crc, b));
    }

    /// Consume the digest and return the checksum.
    pub const fn finalize(self) -> u16 {
        self.crc
    }
}

impl Default for Crc16Digest {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming CRC-32. Applies the final XOR on [`finalize`](Crc32Digest::finalize), so
/// the caller never has to remember it.
///
/// ```
/// # use libcrc_rs::Crc32Digest;
/// let mut d = Crc32Digest::new();
/// for chunk in b"123456789".chunks(2) { d.update(chunk); }
/// assert_eq!(d.finalize(), 0xCBF4_3926);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crc32Digest {
    crc: u32,
}

impl Crc32Digest {
    pub const fn new() -> Self {
        Self { crc: 0xFFFF_FFFF }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.crc = data.iter().fold(self.crc, |crc, &b| super::update_crc_32(crc, b));
    }

    /// Consume the digest, applying the final XOR.
    pub const fn finalize(self) -> u32 {
        self.crc ^ 0xFFFF_FFFF
    }
}

impl Default for Crc32Digest {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming CRC-64. Defaults to ECMA; use [`Crc64Digest::we`] for the WE variant,
/// which differs only in seed and final XOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crc64Digest {
    crc: u64,
    final_xor: u64,
}

impl Crc64Digest {
    /// CRC-64/ECMA: seed `0`, no final XOR.
    pub const fn new() -> Self {
        Self { crc: 0, final_xor: 0 }
    }

    /// CRC-64/WE: seed all-ones, final XOR all-ones.
    pub const fn we() -> Self {
        Self { crc: u64::MAX, final_xor: u64::MAX }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.crc = data.iter().fold(self.crc, |crc, &b| super::update_crc_64(crc, b));
    }

    pub const fn finalize(self) -> u64 {
        self.crc ^ self.final_xor
    }
}

impl Default for Crc64Digest {
    fn default() -> Self {
        Self::new()
    }
}

/// A [`core::hash::Hasher`] backed by CRC-32, so the port drops straight into
/// `HashMap`, `BuildHasherDefault` and anything else in the standard hashing
/// ecosystem. The C original cannot participate in any of that.
///
/// Not a cryptographic hash and not collision-resistant against an adversary — CRC is
/// an error-detection code. Use it for checksumming and integrity, not for
/// untrusted-input hash tables.
///
/// ```
/// # use libcrc_rs::Crc32Hasher;
/// # use core::hash::Hasher;
/// let mut h = Crc32Hasher::default();
/// h.write(b"123456789");
/// assert_eq!(h.finish(), 0xCBF4_3926);
/// ```
#[derive(Debug, Clone, Default)]
pub struct Crc32Hasher {
    digest: Crc32DigestState,
}

#[derive(Debug, Clone)]
struct Crc32DigestState(u32);

impl Default for Crc32DigestState {
    fn default() -> Self {
        Self(0xFFFF_FFFF)
    }
}

impl Hasher for Crc32Hasher {
    fn write(&mut self, bytes: &[u8]) {
        self.digest.0 = bytes.iter().fold(self.digest.0, |crc, &b| super::update_crc_32(crc, b));
    }

    fn finish(&self) -> u64 {
        (self.digest.0 ^ 0xFFFF_FFFF) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crc_16, crc_32, crc_64_ecma, crc_64_we, crc_modbus};

    const CHECK: &[u8] = b"123456789";

    #[test]
    fn streaming_equals_one_shot() {
        let mut d = Crc16Digest::new();
        d.update(CHECK);
        assert_eq!(d.finalize(), crc_16(CHECK));

        let mut d = Crc16Digest::modbus();
        d.update(CHECK);
        assert_eq!(d.finalize(), crc_modbus(CHECK));

        let mut d = Crc32Digest::new();
        d.update(CHECK);
        assert_eq!(d.finalize(), crc_32(CHECK));

        let mut d = Crc64Digest::new();
        d.update(CHECK);
        assert_eq!(d.finalize(), crc_64_ecma(CHECK));

        let mut d = Crc64Digest::we();
        d.update(CHECK);
        assert_eq!(d.finalize(), crc_64_we(CHECK));
    }

    /// Chunk boundaries must not affect the result — the property that makes a
    /// streaming API trustworthy. Checked for every possible split.
    #[test]
    fn chunking_is_irrelevant_at_every_boundary() {
        for split in 0..=CHECK.len() {
            let (a, b) = CHECK.split_at(split);
            let mut d = Crc32Digest::new();
            d.update(a);
            d.update(b);
            assert_eq!(d.finalize(), crc_32(CHECK), "split at {split}");

            let mut d = Crc16Digest::new();
            d.update(a);
            d.update(b);
            assert_eq!(d.finalize(), crc_16(CHECK), "split at {split}");
        }
    }

    /// One byte at a time must equal one big chunk.
    #[test]
    fn byte_at_a_time_equals_bulk() {
        let mut d = Crc32Digest::new();
        for &b in CHECK {
            d.update(&[b]);
        }
        assert_eq!(d.finalize(), crc_32(CHECK));
    }

    #[test]
    fn empty_updates_are_noops() {
        let mut d = Crc32Digest::new();
        d.update(b"");
        d.update(CHECK);
        d.update(b"");
        assert_eq!(d.finalize(), crc_32(CHECK));
    }

    #[test]
    fn hasher_matches_one_shot() {
        let mut h = Crc32Hasher::default();
        h.write(CHECK);
        assert_eq!(h.finish(), crc_32(CHECK) as u64);
    }
}
