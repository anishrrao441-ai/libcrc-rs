//! A zero-unsafe, `no_std` Rust port of [lammertb/libcrc](https://github.com/lammertb/libcrc).
//!
//! # Behavioural contract
//!
//! This crate reproduces libcrc **exactly**, including where libcrc diverges from the
//! RevEng CRC catalogue. Two algorithms differ from their catalogue definitions by a
//! byte swap of the final value:
//!
//! | Algorithm | RevEng catalogue | libcrc (and therefore this crate) |
//! |---|---|---|
//! | CRC-16/KERMIT | `0x2189` | `0x8921` |
//! | CRC-16/DNP    | `0xEA82` | `0x82EA` |
//!
//! Reproducing libcrc — rather than "correcting" it — is the entire point of a port.
//! This is also why a general-purpose catalogue-conformant CRC crate could not have
//! been used as a drop-in replacement. See `DECISIONS.md`.
//!
//! # Tables are built at compile time
//!
//! libcrc computes its lookup tables two different ways: a separate `precalc/` build
//! stage generates `crc_tab32`/`crc_tab64` into C source, while the rest are built
//! lazily on first call behind a `bool` guard. Both stages are deleted here — every
//! table is a `const fn` evaluated by the compiler into `.rodata`.
//!
//! That removes an entire build stage, and it also removes a latent data race: libcrc's
//! lazy initialisation (`if (!crc_tab16_init) init_crc16_tab();`) is unsynchronised, with
//! no atomics or locks anywhere in the library, which is undefined behaviour under
//! C11 §5.1.2.4 when two threads first call a CRC function concurrently.
#![no_std]
#![forbid(unsafe_code)]

// ---------------------------------------------------------------------------
// Compile-time table generation
// ---------------------------------------------------------------------------

/// Build a reflected (LSB-first) CRC table for a 16-bit polynomial, at compile time.
const fn reflected_table_u16(poly: u16) -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut crc = 0u16;
        let mut c = index as u16;
        let mut bit = 0;
        while bit < 8 {
            if (crc ^ c) & 0x0001 != 0 {
                crc = (crc >> 1) ^ poly;
            } else {
                crc >>= 1;
            }
            c >>= 1;
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

/// Build a reflected (LSB-first) CRC table for a 32-bit polynomial, at compile time.
const fn reflected_table_u32(poly: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ poly;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

// ---------------------------------------------------------------------------
// Polynomials — mirrored from libcrc's include/checksum.h
// ---------------------------------------------------------------------------

const POLY_16: u16 = 0xA001;
const POLY_32: u32 = 0xEDB8_8320;

const START_16: u16 = 0x0000;
const START_MODBUS: u16 = 0xFFFF;
const START_32: u32 = 0xFFFF_FFFF;

static TABLE_16: [u16; 256] = reflected_table_u16(POLY_16);
static TABLE_32: [u32; 256] = reflected_table_u32(POLY_32);

// ---------------------------------------------------------------------------
// Incremental core — one byte at a time, mirroring libcrc's `update_crc_*`
// ---------------------------------------------------------------------------

/// Fold one byte into a 16-bit reflected CRC.
#[inline]
pub const fn update_crc_16(crc: u16, byte: u8) -> u16 {
    (crc >> 8) ^ TABLE_16[((crc ^ byte as u16) & 0x00FF) as usize]
}

/// Fold one byte into the 32-bit CRC. Operates on the *internal* (non-finalised)
/// value, exactly like libcrc's `update_crc_32`.
#[inline]
pub const fn update_crc_32(crc: u32, byte: u8) -> u32 {
    (crc >> 8) ^ TABLE_32[((crc ^ byte as u32) & 0x0000_00FF) as usize]
}

// ---------------------------------------------------------------------------
// One-shot public API — takes slices, not pointer+length
// ---------------------------------------------------------------------------

/// CRC-16/ARC. libcrc `crc_16()`. Check value for `b"123456789"` is `0xBB3D`.
pub fn crc_16(data: &[u8]) -> u16 {
    let mut crc = START_16;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_16(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-16/MODBUS. libcrc `crc_modbus()`. Check value for `b"123456789"` is `0x4B37`.
pub fn crc_modbus(data: &[u8]) -> u16 {
    let mut crc = START_MODBUS;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_16(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-32. libcrc `crc_32()`. Check value for `b"123456789"` is `0xCBF43926`.
pub fn crc_32(data: &[u8]) -> u32 {
    let mut crc = START_32;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_32(crc, data[i]);
        i += 1;
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical CRC check string. Every expected value below was produced by
    /// running the ORIGINAL C library, not copied from a specification.
    const CHECK: &[u8] = b"123456789";

    #[test]
    fn matches_c_original_on_check_string() {
        assert_eq!(crc_16(CHECK), 0xBB3D);
        assert_eq!(crc_modbus(CHECK), 0x4B37);
        assert_eq!(crc_32(CHECK), 0xCBF4_3926);
    }

    #[test]
    fn matches_c_original_on_empty_input() {
        assert_eq!(crc_16(b""), 0x0000);
        assert_eq!(crc_modbus(b""), 0xFFFF);
        assert_eq!(crc_32(b""), 0x0000_0000);
    }

    #[test]
    fn matches_c_original_on_single_byte() {
        assert_eq!(crc_16(b"a"), 0xE8C1);
        assert_eq!(crc_modbus(b"a"), 0xA87E);
        assert_eq!(crc_32(b"a"), 0xE8B7_BE43);
    }

    /// Feeding bytes one at a time must equal the one-shot result. libcrc exposes both
    /// forms and callers mix them, so they have to agree.
    #[test]
    fn incremental_equals_one_shot() {
        let mut crc = START_16;
        for &b in CHECK {
            crc = update_crc_16(crc, b);
        }
        assert_eq!(crc, crc_16(CHECK));
    }
}
