//! The port side of the comparison, and the decoder for the oracle's result records.
//!
//! Both sides are reduced to the same [`Block`] so a mismatch names the algorithm that
//! disagreed instead of reporting "80 bytes differ". Two blocks per case: the one-shot
//! entry points, and the same twelve values rebuilt one byte at a time through the
//! `update_crc_*` family.

use libcrc_rs as port;

pub const BLOCK_BYTES: usize = 40;
pub const RECORD_BYTES: usize = 2 * BLOCK_BYTES;
pub const MAGIC_RESULTS: [u8; 4] = *b"PMFR";

/// libcrc's start values, from `include/checksum.h`. Needed to drive the incremental API,
/// which takes the running value rather than owning it.
mod start {
    pub const CRC_8: u8 = 0x00;
    pub const CRC_16: u16 = 0x0000;
    pub const MODBUS: u16 = 0xFFFF;
    pub const CRC_32: u32 = 0xFFFF_FFFF;
    pub const CCITT_1D0F: u16 = 0x1D0F;
    pub const CCITT_FFFF: u16 = 0xFFFF;
    pub const XMODEM: u16 = 0x0000;
    pub const KERMIT: u16 = 0x0000;
    pub const DNP: u16 = 0x0000;
    pub const SICK: u16 = 0x0000;
    pub const CRC_64_ECMA: u64 = 0x0000_0000_0000_0000;
    pub const CRC_64_WE: u64 = 0xFFFF_FFFF_FFFF_FFFF;
}

#[inline]
const fn byteswap(v: u16) -> u16 {
    (v >> 8) | (v << 8)
}

/// One case's worth of CRC values.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Block {
    pub crc_8: u8,
    /// `checksum_NMEA` returned a non-NULL pointer.
    pub nmea_ok: bool,
    /// The two uppercase hex digits it wrote. Meaningless when `nmea_ok` is false.
    pub nmea_hex: [u8; 2],
    pub crc_16: u16,
    pub crc_ccitt_1d0f: u16,
    pub crc_ccitt_ffff: u16,
    pub crc_dnp: u16,
    pub crc_kermit: u16,
    pub crc_modbus: u16,
    pub crc_sick: u16,
    pub crc_xmodem: u16,
    pub crc_32: u32,
    pub crc_64_ecma: u64,
    pub crc_64_we: u64,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut w = [0u8; 8];
    w.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(w)
}

impl Block {
    /// Decode the 40-byte wire block the C harness emitted. Layout is documented at the
    /// top of `fuzz/oracle_harness.c` and must be changed in both places at once.
    pub fn decode(bytes: &[u8]) -> Block {
        debug_assert_eq!(bytes.len(), BLOCK_BYTES);
        Block {
            crc_8: bytes[0],
            nmea_ok: bytes[1] & 0x01 != 0,
            nmea_hex: [bytes[2], bytes[3]],
            crc_16: u16_at(bytes, 4),
            crc_ccitt_1d0f: u16_at(bytes, 6),
            crc_ccitt_ffff: u16_at(bytes, 8),
            crc_dnp: u16_at(bytes, 10),
            crc_kermit: u16_at(bytes, 12),
            crc_modbus: u16_at(bytes, 14),
            crc_sick: u16_at(bytes, 16),
            crc_xmodem: u16_at(bytes, 18),
            crc_32: u32_at(bytes, 20),
            crc_64_ecma: u64_at(bytes, 24),
            crc_64_we: u64_at(bytes, 32),
        }
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Format a checksum the way `snprintf(result, 3, "%02hhX", checksum)` does.
fn nmea_digits(checksum: u8) -> [u8; 2] {
    [HEX[(checksum >> 4) as usize], HEX[(checksum & 0x0F) as usize]]
}

/// Every one-shot entry point, computed by the port.
///
/// `is_null` reproduces the C caller passing a NULL pointer. libcrc's twelve CRC
/// functions guard their loop and return the init value, which is exactly the result of
/// folding an empty slice — so the caller hands us an empty payload and we only have to
/// special-case `checksum_NMEA`, which returns NULL instead.
pub fn port_oneshot(data: &[u8], is_null: bool) -> Block {
    Block {
        crc_8: port::crc_8(data),
        nmea_ok: !is_null,
        nmea_hex: if is_null { [0, 0] } else { nmea_digits(port::checksum_nmea(data)) },
        crc_16: port::crc_16(data),
        crc_ccitt_1d0f: port::crc_ccitt_1d0f(data),
        crc_ccitt_ffff: port::crc_ccitt_ffff(data),
        crc_dnp: port::crc_dnp(data),
        crc_kermit: port::crc_kermit(data),
        crc_modbus: port::crc_modbus(data),
        crc_sick: port::crc_sick(data),
        crc_xmodem: port::crc_xmodem(data),
        crc_32: port::crc_32(data),
        crc_64_ecma: port::crc_64_ecma(data),
        crc_64_we: port::crc_64_we(data),
    }
}

/// The same twelve values rebuilt through the byte-at-a-time API.
///
/// This is the half that would catch a finalisation applied in the wrong place: the
/// `update_*` functions return the *raw* running value, so the three byte-swaps, the two
/// final XORs and DNP's complement all have to be re-applied here by the caller — exactly
/// as a real libcrc user streaming a message would have to. `checksum_NMEA` has no
/// incremental form, so its fields stay zero on both sides.
pub fn port_incremental(data: &[u8]) -> Block {
    let mut c8 = start::CRC_8;
    let mut c16 = start::CRC_16;
    let mut cmb = start::MODBUS;
    let mut c32 = start::CRC_32;
    let mut c1d0f = start::CCITT_1D0F;
    let mut cffff = start::CCITT_FFFF;
    let mut cxmod = start::XMODEM;
    let mut ckerm = start::KERMIT;
    let mut cdnp = start::DNP;
    let mut csick = start::SICK;
    let mut cecma = start::CRC_64_ECMA;
    let mut cwe = start::CRC_64_WE;

    let mut prev = 0u8;
    for &b in data {
        c8 = port::update_crc_8(c8, b);
        c16 = port::update_crc_16(c16, b);
        cmb = port::update_crc_16(cmb, b);
        c32 = port::update_crc_32(c32, b);
        c1d0f = port::update_crc_ccitt(c1d0f, b);
        cffff = port::update_crc_ccitt(cffff, b);
        cxmod = port::update_crc_ccitt(cxmod, b);
        ckerm = port::update_crc_kermit(ckerm, b);
        cdnp = port::update_crc_dnp(cdnp, b);
        csick = port::update_crc_sick(csick, b, prev);
        cecma = port::update_crc_64(cecma, b);
        cwe = port::update_crc_64(cwe, b);
        prev = b;
    }

    Block {
        crc_8: c8,
        nmea_ok: false,
        nmea_hex: [0, 0],
        crc_16: c16,
        crc_ccitt_1d0f: c1d0f,
        crc_ccitt_ffff: cffff,
        crc_dnp: byteswap(!cdnp),
        crc_kermit: byteswap(ckerm),
        crc_modbus: cmb,
        crc_sick: byteswap(csick),
        crc_xmodem: cxmod,
        crc_32: c32 ^ 0xFFFF_FFFF,
        crc_64_ecma: cecma,
        crc_64_we: cwe ^ 0xFFFF_FFFF_FFFF_FFFF,
    }
}

/// A single named disagreement.
#[derive(Clone, Debug)]
pub struct Diff {
    pub check: &'static str,
    pub oracle: String,
    pub port: String,
}

/// The 13 one-shot checks, in the order they are reported.
pub const ONESHOT_CHECKS: [&str; 13] = [
    "crc_8",
    "crc_16",
    "crc_32",
    "crc_64_ecma",
    "crc_64_we",
    "crc_ccitt_1d0f",
    "crc_ccitt_ffff",
    "crc_dnp",
    "crc_kermit",
    "crc_modbus",
    "crc_sick",
    "crc_xmodem",
    "checksum_NMEA",
];

/// The 12 incremental checks, exercising the 8 `update_crc_*` functions.
pub const INCREMENTAL_CHECKS: [&str; 12] = [
    "update_crc_8",
    "update_crc_16[init=0x0000]",
    "update_crc_16[init=0xFFFF]",
    "update_crc_32",
    "update_crc_64[ecma]",
    "update_crc_64[we]",
    "update_crc_ccitt[init=0x1D0F]",
    "update_crc_ccitt[init=0xFFFF]",
    "update_crc_ccitt[init=0x0000]",
    "update_crc_dnp",
    "update_crc_kermit",
    "update_crc_sick",
];

macro_rules! cmp_field {
    ($out:expr, $name:expr, $a:expr, $b:expr, $width:literal) => {
        if $a != $b {
            $out.push(Diff {
                check: $name,
                oracle: format!(concat!("0x{:0", $width, "X}"), $a),
                port: format!(concat!("0x{:0", $width, "X}"), $b),
            });
        }
    };
}

/// Compare the one-shot blocks, naming each algorithm that disagreed.
pub fn diff_oneshot(oracle: &Block, port: &Block, out: &mut Vec<Diff>) {
    cmp_field!(out, "crc_8", oracle.crc_8, port.crc_8, 2);
    cmp_field!(out, "crc_16", oracle.crc_16, port.crc_16, 4);
    cmp_field!(out, "crc_32", oracle.crc_32, port.crc_32, 8);
    cmp_field!(out, "crc_64_ecma", oracle.crc_64_ecma, port.crc_64_ecma, 16);
    cmp_field!(out, "crc_64_we", oracle.crc_64_we, port.crc_64_we, 16);
    cmp_field!(out, "crc_ccitt_1d0f", oracle.crc_ccitt_1d0f, port.crc_ccitt_1d0f, 4);
    cmp_field!(out, "crc_ccitt_ffff", oracle.crc_ccitt_ffff, port.crc_ccitt_ffff, 4);
    cmp_field!(out, "crc_dnp", oracle.crc_dnp, port.crc_dnp, 4);
    cmp_field!(out, "crc_kermit", oracle.crc_kermit, port.crc_kermit, 4);
    cmp_field!(out, "crc_modbus", oracle.crc_modbus, port.crc_modbus, 4);
    cmp_field!(out, "crc_sick", oracle.crc_sick, port.crc_sick, 4);
    cmp_field!(out, "crc_xmodem", oracle.crc_xmodem, port.crc_xmodem, 4);

    if oracle.nmea_ok != port.nmea_ok {
        out.push(Diff {
            check: "checksum_NMEA",
            oracle: format!("returned {}", if oracle.nmea_ok { "non-NULL" } else { "NULL" }),
            port: format!("returned {}", if port.nmea_ok { "non-NULL" } else { "NULL" }),
        });
    } else if oracle.nmea_ok && oracle.nmea_hex != port.nmea_hex {
        out.push(Diff {
            check: "checksum_NMEA",
            oracle: format!("\"{}\"", String::from_utf8_lossy(&oracle.nmea_hex)),
            port: format!("\"{}\"", String::from_utf8_lossy(&port.nmea_hex)),
        });
    }
}

/// Compare the incremental blocks. Field names carry their seed so a failure points at
/// one call site rather than at a shared helper.
pub fn diff_incremental(oracle: &Block, port: &Block, out: &mut Vec<Diff>) {
    cmp_field!(out, "update_crc_8", oracle.crc_8, port.crc_8, 2);
    cmp_field!(out, "update_crc_16[init=0x0000]", oracle.crc_16, port.crc_16, 4);
    cmp_field!(out, "update_crc_16[init=0xFFFF]", oracle.crc_modbus, port.crc_modbus, 4);
    cmp_field!(out, "update_crc_32", oracle.crc_32, port.crc_32, 8);
    cmp_field!(out, "update_crc_64[ecma]", oracle.crc_64_ecma, port.crc_64_ecma, 16);
    cmp_field!(out, "update_crc_64[we]", oracle.crc_64_we, port.crc_64_we, 16);
    cmp_field!(out, "update_crc_ccitt[init=0x1D0F]", oracle.crc_ccitt_1d0f, port.crc_ccitt_1d0f, 4);
    cmp_field!(out, "update_crc_ccitt[init=0xFFFF]", oracle.crc_ccitt_ffff, port.crc_ccitt_ffff, 4);
    cmp_field!(out, "update_crc_ccitt[init=0x0000]", oracle.crc_xmodem, port.crc_xmodem, 4);
    cmp_field!(out, "update_crc_dnp", oracle.crc_dnp, port.crc_dnp, 4);
    cmp_field!(out, "update_crc_kermit", oracle.crc_kermit, port.crc_kermit, 4);
    cmp_field!(out, "update_crc_sick", oracle.crc_sick, port.crc_sick, 4);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values produced by RUNNING the original C library, recorded in
    /// .planning/libcrc-plan/00-VERIFIED-FACTS.md §11. If the port ever drifts from these
    /// the fuzzer is measuring the wrong thing.
    #[test]
    fn check_string_matches_recorded_c_output() {
        let b = port_oneshot(b"123456789", false);
        assert_eq!(b.crc_16, 0xBB3D);
        assert_eq!(b.crc_modbus, 0x4B37);
        assert_eq!(b.crc_sick, 0x56A6);
        assert_eq!(b.crc_xmodem, 0x31C3);
        assert_eq!(b.crc_ccitt_ffff, 0x29B1);
        assert_eq!(b.crc_ccitt_1d0f, 0xE5CC);
        assert_eq!(b.crc_kermit, 0x8921);
        assert_eq!(b.crc_dnp, 0x82EA);
        assert_eq!(b.crc_32, 0xCBF4_3926);
    }

    /// The incremental path must reach the same place as the one-shot path, including
    /// the byte-swaps and final XORs. This is the invariant the C harness mirrors.
    #[test]
    fn incremental_agrees_with_one_shot() {
        for data in [
            b"".as_slice(),
            b"a",
            b"123456789",
            b"$GPGLL,4916.45,N*7C\r\n",
            &[0u8; 257],
            &[0xFFu8; 129],
        ] {
            let one = port_oneshot(data, false);
            let inc = port_incremental(data);
            assert_eq!(one.crc_8, inc.crc_8, "crc_8 for {data:?}");
            assert_eq!(one.crc_16, inc.crc_16, "crc_16 for {data:?}");
            assert_eq!(one.crc_modbus, inc.crc_modbus, "modbus for {data:?}");
            assert_eq!(one.crc_32, inc.crc_32, "crc_32 for {data:?}");
            assert_eq!(one.crc_ccitt_1d0f, inc.crc_ccitt_1d0f, "1d0f for {data:?}");
            assert_eq!(one.crc_ccitt_ffff, inc.crc_ccitt_ffff, "ffff for {data:?}");
            assert_eq!(one.crc_xmodem, inc.crc_xmodem, "xmodem for {data:?}");
            assert_eq!(one.crc_kermit, inc.crc_kermit, "kermit for {data:?}");
            assert_eq!(one.crc_dnp, inc.crc_dnp, "dnp for {data:?}");
            assert_eq!(one.crc_sick, inc.crc_sick, "sick for {data:?}");
            assert_eq!(one.crc_64_ecma, inc.crc_64_ecma, "ecma for {data:?}");
            assert_eq!(one.crc_64_we, inc.crc_64_we, "we for {data:?}");
        }
    }

    #[test]
    fn null_case_returns_init_values_and_a_null_nmea() {
        let b = port_oneshot(&[], true);
        assert!(!b.nmea_ok, "checksum_NMEA(NULL, ..) must report NULL");
        assert_eq!(b.crc_16, 0x0000);
        assert_eq!(b.crc_modbus, 0xFFFF);
        assert_eq!(b.crc_ccitt_1d0f, 0x1D0F);
        assert_eq!(b.crc_dnp, 0xFFFF);
        assert_eq!(b.crc_32, 0x0000_0000);
    }

    #[test]
    fn nmea_digits_are_uppercase_and_padded() {
        assert_eq!(&nmea_digits(0x00), b"00");
        assert_eq!(&nmea_digits(0x0A), b"0A");
        assert_eq!(&nmea_digits(0xFF), b"FF");
        assert_eq!(&nmea_digits(0x7C), b"7C");
    }

    #[test]
    fn decode_round_trips_a_known_block() {
        let mut raw = [0u8; BLOCK_BYTES];
        raw[0] = 0xAB;
        raw[1] = 0x01;
        raw[2] = b'7';
        raw[3] = b'C';
        raw[4..6].copy_from_slice(&0xBB3Du16.to_le_bytes());
        raw[20..24].copy_from_slice(&0xCBF4_3926u32.to_le_bytes());
        raw[24..32].copy_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());

        let b = Block::decode(&raw);
        assert_eq!(b.crc_8, 0xAB);
        assert!(b.nmea_ok);
        assert_eq!(&b.nmea_hex, b"7C");
        assert_eq!(b.crc_16, 0xBB3D);
        assert_eq!(b.crc_32, 0xCBF4_3926);
        assert_eq!(b.crc_64_ecma, 0x0123_4567_89AB_CDEF);
    }

    #[test]
    fn identical_blocks_produce_no_diffs() {
        let a = port_oneshot(b"hello", false);
        let mut out = Vec::new();
        diff_oneshot(&a, &a, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_planted_mismatch_is_named() {
        let a = port_oneshot(b"hello", false);
        let mut b = a;
        b.crc_kermit ^= 1;
        let mut out = Vec::new();
        diff_oneshot(&a, &b, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].check, "crc_kermit");
    }
}
