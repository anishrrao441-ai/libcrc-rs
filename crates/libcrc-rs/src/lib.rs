//! A zero-unsafe, `no_std` Rust port of [lammertb/libcrc](https://github.com/lammertb/libcrc).
//!
//! # Behavioural contract
//!
//! This crate reproduces libcrc **exactly**, including where libcrc diverges from the
//! RevEng CRC catalogue. Three algorithms byte-swap their final value, which the
//! catalogue does not:
//!
//! | Algorithm | RevEng catalogue | libcrc (and therefore this crate) |
//! |---|---|---|
//! | CRC-16/KERMIT | `0x2189` | `0x8921` |
//! | CRC-16/DNP    | `0xEA82` | `0x82EA` |
//! | CRC-16/SICK   | *(not catalogued)* | `0x56A6` |
//!
//! Reproducing libcrc — rather than "correcting" it — is the entire point of a port.
//! It is also why a catalogue-conformant general-purpose CRC crate could not have been
//! used as a drop-in replacement here. See `DECISIONS.md`.
//!
//! # Tables are built at compile time
//!
//! libcrc builds its tables two different ways: a separate `precalc/` build stage
//! generates the 32- and 64-bit tables into C source, while the rest are filled lazily
//! on first call behind a `bool` guard. Both stages are deleted here — every table is a
//! `const fn` evaluated by the compiler into `.rodata`.
//!
//! That removes an entire build stage, and it removes a latent data race: libcrc's lazy
//! initialisation (`if (!crc_tab16_init) init_crc16_tab();`) is unsynchronised, with no
//! atomics or locks anywhere in the library — undefined behaviour under C11 §5.1.2.4
//! when two threads first call a CRC function concurrently.
#![no_std]
#![forbid(unsafe_code)]

mod tables;

use tables::SHT75_CRC_TABLE;

// ===========================================================================
// Compile-time table construction
// ===========================================================================

/// Reflected (LSB-first) 16-bit table.
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

/// Reflected (LSB-first) 32-bit table.
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

/// Forward (MSB-first) 16-bit table, used by the CCITT family.
const fn forward_table_u16(poly: u16) -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut crc = 0u16;
        let mut c = (index as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            if (crc ^ c) & 0x8000 != 0 {
                crc = (crc << 1) ^ poly;
            } else {
                crc <<= 1;
            }
            c <<= 1;
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

/// Forward (MSB-first) 64-bit table.
const fn forward_table_u64(poly: u64) -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut crc = 0u64;
        let mut c = (index as u64) << 56;
        let mut bit = 0;
        while bit < 8 {
            if (crc ^ c) & 0x8000_0000_0000_0000 != 0 {
                crc = (crc << 1) ^ poly;
            } else {
                crc <<= 1;
            }
            c <<= 1;
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

// ===========================================================================
// Parameters — mirrored from libcrc's include/checksum.h
// ===========================================================================

const POLY_16: u16 = 0xA001;
const POLY_32: u32 = 0xEDB8_8320;
const POLY_64: u64 = 0x42F0_E1EB_A9EA_3693;
const POLY_CCITT: u16 = 0x1021;
const POLY_DNP: u16 = 0xA6BC;
const POLY_KERMIT: u16 = 0x8408;
const POLY_SICK: u16 = 0x8005;

const START_8: u8 = 0x00;
const START_16: u16 = 0x0000;
const START_MODBUS: u16 = 0xFFFF;
const START_XMODEM: u16 = 0x0000;
const START_CCITT_1D0F: u16 = 0x1D0F;
const START_CCITT_FFFF: u16 = 0xFFFF;
const START_KERMIT: u16 = 0x0000;
const START_SICK: u16 = 0x0000;
const START_DNP: u16 = 0x0000;
const START_32: u32 = 0xFFFF_FFFF;
const START_64_ECMA: u64 = 0x0000_0000_0000_0000;
const START_64_WE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

static TABLE_16: [u16; 256] = reflected_table_u16(POLY_16);
static TABLE_32: [u32; 256] = reflected_table_u32(POLY_32);
static TABLE_64: [u64; 256] = forward_table_u64(POLY_64);
static TABLE_CCITT: [u16; 256] = forward_table_u16(POLY_CCITT);
static TABLE_KERMIT: [u16; 256] = reflected_table_u16(POLY_KERMIT);
static TABLE_DNP: [u16; 256] = reflected_table_u16(POLY_DNP);

/// Swap the two bytes of a 16-bit value. libcrc does this by hand at the end of
/// `crc_kermit`, `crc_dnp` and `crc_sick`; it is the reason those three diverge from
/// the RevEng catalogue.
#[inline]
const fn byteswap(crc: u16) -> u16 {
    (crc >> 8) | (crc << 8)
}

// ===========================================================================
// Incremental API — one byte at a time, mirroring libcrc's `update_crc_*`
// ===========================================================================

/// Fold one byte into a CRC-8 (Sensirion SHT7x) value.
#[inline]
pub const fn update_crc_8(crc: u8, byte: u8) -> u8 {
    SHT75_CRC_TABLE[(byte ^ crc) as usize]
}

/// Fold one byte into a reflected 16-bit CRC (CRC-16 / MODBUS).
#[inline]
pub const fn update_crc_16(crc: u16, byte: u8) -> u16 {
    (crc >> 8) ^ TABLE_16[((crc ^ byte as u16) & 0x00FF) as usize]
}

/// Fold one byte into the internal, non-finalised CRC-32 value.
#[inline]
pub const fn update_crc_32(crc: u32, byte: u8) -> u32 {
    (crc >> 8) ^ TABLE_32[((crc ^ byte as u32) & 0x0000_00FF) as usize]
}

/// Fold one byte into a CCITT-family CRC (MSB-first).
#[inline]
pub const fn update_crc_ccitt(crc: u16, byte: u8) -> u16 {
    (crc << 8) ^ TABLE_CCITT[(((crc >> 8) ^ byte as u16) & 0x00FF) as usize]
}

/// Fold one byte into a Kermit CRC. Note the caller must byte-swap at the end.
#[inline]
pub const fn update_crc_kermit(crc: u16, byte: u8) -> u16 {
    (crc >> 8) ^ TABLE_KERMIT[((crc ^ byte as u16) & 0x00FF) as usize]
}

/// Fold one byte into a DNP CRC. The caller must complement and byte-swap at the end.
#[inline]
pub const fn update_crc_dnp(crc: u16, byte: u8) -> u16 {
    (crc >> 8) ^ TABLE_DNP[((crc ^ byte as u16) & 0x00FF) as usize]
}

/// Fold one byte into the internal, non-finalised CRC-64 value.
#[inline]
pub const fn update_crc_64(crc: u64, byte: u8) -> u64 {
    (crc << 8) ^ TABLE_64[(((crc >> 56) ^ byte as u64) & 0xFF) as usize]
}

/// Fold one byte into a SICK CRC, which is bitwise rather than table-driven and needs
/// the *previous* byte. `prev_byte` is `0` for the first byte of a message.
#[inline]
pub const fn update_crc_sick(crc: u16, byte: u8, prev_byte: u8) -> u16 {
    let short_c = byte as u16;
    let short_p = (prev_byte as u16) << 8;
    let crc = if crc & 0x8000 != 0 {
        (crc << 1) ^ POLY_SICK
    } else {
        crc << 1
    };
    crc ^ (short_c | short_p)
}

// ===========================================================================
// One-shot API — takes slices, not pointer+length
// ===========================================================================

/// CRC-8, Sensirion SHT7x table. libcrc `crc_8()`.
pub fn crc_8(data: &[u8]) -> u8 {
    let mut crc = START_8;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_8(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-16/ARC. libcrc `crc_16()`. Check value for `b"123456789"` is `0xBB3D`.
pub fn crc_16(data: &[u8]) -> u16 {
    fold_16(START_16, data)
}

/// CRC-16/MODBUS. libcrc `crc_modbus()`. Check value is `0x4B37`.
pub fn crc_modbus(data: &[u8]) -> u16 {
    fold_16(START_MODBUS, data)
}

fn fold_16(start: u16, data: &[u8]) -> u16 {
    let mut crc = start;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_16(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-32. libcrc `crc_32()`. Check value is `0xCBF43926`.
pub fn crc_32(data: &[u8]) -> u32 {
    let mut crc = START_32;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_32(crc, data[i]);
        i += 1;
    }
    crc ^ 0xFFFF_FFFF
}

fn fold_ccitt(start: u16, data: &[u8]) -> u16 {
    let mut crc = start;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_ccitt(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-CCITT seeded with `0x1D0F`. libcrc `crc_ccitt_1d0f()`. Check value is `0xE5CC`.
pub fn crc_ccitt_1d0f(data: &[u8]) -> u16 {
    fold_ccitt(START_CCITT_1D0F, data)
}

/// CRC-CCITT seeded with `0xFFFF`. libcrc `crc_ccitt_ffff()`. Check value is `0x29B1`.
pub fn crc_ccitt_ffff(data: &[u8]) -> u16 {
    fold_ccitt(START_CCITT_FFFF, data)
}

/// CRC-16/XMODEM. libcrc `crc_xmodem()`. Check value is `0x31C3`.
pub fn crc_xmodem(data: &[u8]) -> u16 {
    fold_ccitt(START_XMODEM, data)
}

/// CRC-16/KERMIT. libcrc `crc_kermit()`.
///
/// Byte-swaps the final value, diverging from the RevEng catalogue: libcrc returns
/// `0x8921` for `b"123456789"` where the catalogue specifies `0x2189`.
pub fn crc_kermit(data: &[u8]) -> u16 {
    let mut crc = START_KERMIT;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_kermit(crc, data[i]);
        i += 1;
    }
    byteswap(crc)
}

/// CRC-16/DNP. libcrc `crc_dnp()`.
///
/// Complements *then* byte-swaps, diverging from the RevEng catalogue: libcrc returns
/// `0x82EA` for `b"123456789"` where the catalogue specifies `0xEA82`.
pub fn crc_dnp(data: &[u8]) -> u16 {
    let mut crc = START_DNP;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_dnp(crc, data[i]);
        i += 1;
    }
    byteswap(!crc)
}

/// CRC-16/SICK. libcrc `crc_sick()`. Check value is `0x56A6`.
///
/// Bitwise rather than table-driven, and each step mixes in the *previous* byte. There
/// is no RevEng catalogue entry for this algorithm, so its only reference is libcrc
/// itself — correctness rests entirely on byte-for-byte differential parity.
pub fn crc_sick(data: &[u8]) -> u16 {
    let mut crc = START_SICK;
    let mut prev = 0u8;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_sick(crc, data[i], prev);
        prev = data[i];
        i += 1;
    }
    byteswap(crc)
}

/// CRC-64/ECMA. libcrc `crc_64_ecma()`.
pub fn crc_64_ecma(data: &[u8]) -> u64 {
    let mut crc = START_64_ECMA;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_64(crc, data[i]);
        i += 1;
    }
    crc
}

/// CRC-64/WE. libcrc `crc_64_we()`. Same table as ECMA, different seed and final XOR.
pub fn crc_64_we(data: &[u8]) -> u64 {
    let mut crc = START_64_WE;
    let mut i = 0;
    while i < data.len() {
        crc = update_crc_64(crc, data[i]);
        i += 1;
    }
    crc ^ 0xFFFF_FFFF_FFFF_FFFF
}

/// NMEA 0183 sentence checksum. libcrc `checksum_NMEA()`.
///
/// Skips a leading `$`, then XORs bytes until NUL, CR, LF or `*`. Unlike every other
/// function here this one is delimiter-driven rather than length-driven, so it takes the
/// sentence bytes and stops on its own terminator.
pub fn checksum_nmea(sentence: &[u8]) -> u8 {
    let mut checksum = 0u8;
    let mut i = 0;
    if !sentence.is_empty() && sentence[0] == b'$' {
        i = 1;
    }
    while i < sentence.len() {
        let b = sentence[i];
        if b == 0 || b == b'\r' || b == b'\n' || b == b'*' {
            break;
        }
        checksum ^= b;
        i += 1;
    }
    checksum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical CRC check string. Every expected value below was produced by
    /// **running the original C library**, not copied from a specification.
    const CHECK: &[u8] = b"123456789";

    #[test]
    fn check_string_matches_c_original() {
        assert_eq!(crc_16(CHECK), 0xBB3D);
        assert_eq!(crc_modbus(CHECK), 0x4B37);
        assert_eq!(crc_sick(CHECK), 0x56A6);
        assert_eq!(crc_xmodem(CHECK), 0x31C3);
        assert_eq!(crc_ccitt_ffff(CHECK), 0x29B1);
        assert_eq!(crc_ccitt_1d0f(CHECK), 0xE5CC);
        assert_eq!(crc_kermit(CHECK), 0x8921);
        assert_eq!(crc_dnp(CHECK), 0x82EA);
        assert_eq!(crc_32(CHECK), 0xCBF4_3926);
    }

    #[test]
    fn empty_input_matches_c_original() {
        assert_eq!(crc_16(b""), 0x0000);
        assert_eq!(crc_modbus(b""), 0xFFFF);
        assert_eq!(crc_sick(b""), 0x0000);
        assert_eq!(crc_xmodem(b""), 0x0000);
        assert_eq!(crc_ccitt_ffff(b""), 0xFFFF);
        assert_eq!(crc_ccitt_1d0f(b""), 0x1D0F);
        assert_eq!(crc_kermit(b""), 0x0000);
        assert_eq!(crc_dnp(b""), 0xFFFF);
        assert_eq!(crc_32(b""), 0x0000_0000);
    }

    #[test]
    fn single_byte_matches_c_original() {
        assert_eq!(crc_16(b"a"), 0xE8C1);
        assert_eq!(crc_modbus(b"a"), 0xA87E);
        assert_eq!(crc_32(b"a"), 0xE8B7_BE43);
    }

    /// libcrc byte-swaps Kermit and DNP relative to the RevEng catalogue. Pin that
    /// divergence explicitly so nobody "fixes" it later and silently fails the suite.
    #[test]
    fn documented_catalogue_divergences_are_preserved() {
        assert_eq!(crc_kermit(CHECK), 0x8921, "libcrc byte-swaps; catalogue says 0x2189");
        assert_eq!(byteswap(crc_kermit(CHECK)), 0x2189, "swapping back yields the catalogue value");
        assert_eq!(crc_dnp(CHECK), 0x82EA, "libcrc byte-swaps; catalogue says 0xEA82");
        assert_eq!(byteswap(crc_dnp(CHECK)), 0xEA82, "swapping back yields the catalogue value");
    }

    /// Feeding bytes one at a time must equal the one-shot result; libcrc exposes both
    /// forms and callers mix them.
    #[test]
    fn incremental_equals_one_shot() {
        let mut crc = START_16;
        for &b in CHECK {
            crc = update_crc_16(crc, b);
        }
        assert_eq!(crc, crc_16(CHECK));

        let mut crc8 = START_8;
        for &b in CHECK {
            crc8 = update_crc_8(crc8, b);
        }
        assert_eq!(crc8, crc_8(CHECK));
    }

    #[test]
    fn nmea_skips_dollar_and_stops_at_star() {
        assert_eq!(checksum_nmea(b"$GPGLL,x*7C"), checksum_nmea(b"GPGLL,x"));
        assert_eq!(checksum_nmea(b"AB"), b'A' ^ b'B');
    }
}
