//! The thirteen algorithms libcrc exposes, and a streaming state for each.
//!
//! Where the port already ships a `Digest` type it is used verbatim — that is the
//! whole point of this crate as a demonstration: the C original has no streaming API,
//! so a CLI written against it would have to buffer the file or hand-roll a
//! byte-at-a-time loop *and* remember each algorithm's finalisation rule. The
//! algorithms with no `Digest` type yet are folded here from the port's public
//! `update_crc_*` primitives, which is exactly the same code path the one-shot
//! functions take.
//!
//! Nothing in this file hard-codes a check value. `crc --list` computes every value it
//! prints by running the algorithm on `b"123456789"` at that moment, so the table can
//! never drift away from the code.

use std::io::Write;

use libcrc_rs::{
    update_crc_8, update_crc_ccitt, update_crc_dnp, update_crc_kermit, update_crc_sick,
    Crc16Digest, Crc32Digest, Crc64Digest,
};

use crate::error::{CliError, Result};

/// The canonical CRC check string, used only by `--list`.
const CHECK_STRING: &[u8] = b"123456789";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Algo {
    Crc8,
    Crc16,
    Modbus,
    Sick,
    Xmodem,
    CcittFfff,
    Ccitt1d0f,
    Kermit,
    Dnp,
    Crc32,
    Crc64Ecma,
    Crc64We,
    Nmea,
}

/// All thirteen, in the order `--all` prints them: narrow to wide, then NMEA.
pub const ALL: [Algo; 13] = [
    Algo::Crc8,
    Algo::Crc16,
    Algo::Modbus,
    Algo::Sick,
    Algo::Xmodem,
    Algo::CcittFfff,
    Algo::Ccitt1d0f,
    Algo::Kermit,
    Algo::Dnp,
    Algo::Crc32,
    Algo::Crc64Ecma,
    Algo::Crc64We,
    Algo::Nmea,
];

/// What a bare `crc FILE` computes: the two most widely used of the thirteen.
pub const DEFAULT: [Algo; 2] = [Algo::Crc16, Algo::Crc32];

impl Algo {
    /// The name printed and accepted on the command line and in manifests.
    pub const fn name(self) -> &'static str {
        match self {
            Algo::Crc8 => "crc_8",
            Algo::Crc16 => "crc_16",
            Algo::Modbus => "crc_modbus",
            Algo::Sick => "crc_sick",
            Algo::Xmodem => "crc_xmodem",
            Algo::CcittFfff => "crc_ccitt_ffff",
            Algo::Ccitt1d0f => "crc_ccitt_1d0f",
            Algo::Kermit => "crc_kermit",
            Algo::Dnp => "crc_dnp",
            Algo::Crc32 => "crc_32",
            Algo::Crc64Ecma => "crc_64_ecma",
            Algo::Crc64We => "crc_64_we",
            Algo::Nmea => "nmea",
        }
    }

    /// The function in the original C library this reproduces, so a reader can map a
    /// line of output straight onto `include/checksum.h`.
    pub const fn c_function(self) -> &'static str {
        match self {
            Algo::Nmea => "checksum_NMEA",
            other => other.name(),
        }
    }

    pub const fn bits(self) -> u32 {
        match self {
            Algo::Crc8 | Algo::Nmea => 8,
            Algo::Crc16
            | Algo::Modbus
            | Algo::Sick
            | Algo::Xmodem
            | Algo::CcittFfff
            | Algo::Ccitt1d0f
            | Algo::Kermit
            | Algo::Dnp => 16,
            Algo::Crc32 => 32,
            Algo::Crc64Ecma | Algo::Crc64We => 64,
        }
    }

    /// Number of hex digits in a rendered value: 2, 4, 8 or 16.
    pub const fn hex_digits(self) -> usize {
        (self.bits() / 4) as usize
    }

    /// Short spellings accepted in addition to [`Algo::name`].
    const fn aliases(self) -> &'static [&'static str] {
        match self {
            Algo::Crc8 => &["8", "crc8", "sht75"],
            Algo::Crc16 => &["16", "crc16", "arc"],
            Algo::Modbus => &["modbus"],
            Algo::Sick => &["sick"],
            Algo::Xmodem => &["xmodem"],
            Algo::CcittFfff => &["ccitt", "ccitt_ffff"],
            Algo::Ccitt1d0f => &["ccitt_1d0f"],
            Algo::Kermit => &["kermit"],
            Algo::Dnp => &["dnp"],
            Algo::Crc32 => &["32", "crc32"],
            Algo::Crc64Ecma => &["64", "crc64", "ecma", "64_ecma"],
            Algo::Crc64We => &["we", "64_we"],
            Algo::Nmea => &["checksum_nmea"],
        }
    }

    /// Case-insensitive, and `-` is accepted wherever `_` is.
    pub fn parse(spelling: &str) -> Option<Algo> {
        let wanted = spelling.trim().to_ascii_lowercase().replace('-', "_");
        ALL.into_iter()
            .find(|a| a.name() == wanted || a.aliases().contains(&wanted.as_str()))
    }

    /// A fresh streaming state for this algorithm.
    pub fn digest(self) -> Digest {
        let state = match self {
            Algo::Crc8 => State::Crc8(0x00),
            Algo::Crc16 => State::Crc16(Crc16Digest::new()),
            Algo::Modbus => State::Crc16(Crc16Digest::modbus()),
            Algo::Sick => State::Sick {
                crc: 0x0000,
                prev: 0,
            },
            Algo::Xmodem => State::Ccitt(0x0000),
            Algo::CcittFfff => State::Ccitt(0xFFFF),
            Algo::Ccitt1d0f => State::Ccitt(0x1D0F),
            Algo::Kermit => State::Kermit(0x0000),
            Algo::Dnp => State::Dnp(0x0000),
            Algo::Crc32 => State::Crc32(Crc32Digest::new()),
            Algo::Crc64Ecma => State::Crc64(Crc64Digest::new()),
            Algo::Crc64We => State::Crc64(Crc64Digest::we()),
            Algo::Nmea => State::Nmea(Nmea::new()),
        };
        Digest { algo: self, state }
    }
}

/// A running checksum. Feed it any number of chunks of any size; the result is
/// identical to hashing the concatenation in one call.
pub struct Digest {
    algo: Algo,
    state: State,
}

enum State {
    Crc8(u8),
    /// Covers CRC-16/ARC and MODBUS, which differ only in seed.
    Crc16(Crc16Digest),
    /// SICK is bitwise, not table-driven, and each step mixes in the previous byte —
    /// so the carried state is two values, not one.
    Sick {
        crc: u16,
        prev: u8,
    },
    /// Covers XMODEM and both seeded CCITT variants.
    Ccitt(u16),
    Kermit(u16),
    Dnp(u16),
    Crc32(Crc32Digest),
    /// Covers ECMA and WE, which differ only in seed and final XOR.
    Crc64(Crc64Digest),
    Nmea(Nmea),
}

impl Digest {
    pub fn update(&mut self, data: &[u8]) {
        match &mut self.state {
            State::Crc8(crc) => {
                for &byte in data {
                    *crc = update_crc_8(*crc, byte);
                }
            }
            State::Crc16(digest) => digest.update(data),
            State::Sick { crc, prev } => {
                for &byte in data {
                    *crc = update_crc_sick(*crc, byte, *prev);
                    *prev = byte;
                }
            }
            State::Ccitt(crc) => {
                for &byte in data {
                    *crc = update_crc_ccitt(*crc, byte);
                }
            }
            State::Kermit(crc) => {
                for &byte in data {
                    *crc = update_crc_kermit(*crc, byte);
                }
            }
            State::Dnp(crc) => {
                for &byte in data {
                    *crc = update_crc_dnp(*crc, byte);
                }
            }
            State::Crc32(digest) => digest.update(data),
            State::Crc64(digest) => digest.update(data),
            State::Nmea(nmea) => nmea.update(data),
        }
    }

    /// Apply the algorithm's finalisation and widen to `u64` for uniform printing.
    ///
    /// The byte swaps below are libcrc's, not the RevEng catalogue's, and are
    /// deliberate: see the crate documentation of `libcrc-rs`.
    pub fn finish(self) -> Checksum {
        let value = match self.state {
            State::Crc8(crc) => u64::from(crc),
            State::Crc16(digest) => u64::from(digest.finalize()),
            State::Sick { crc, .. } => u64::from(crc.swap_bytes()),
            State::Ccitt(crc) => u64::from(crc),
            State::Kermit(crc) => u64::from(crc.swap_bytes()),
            State::Dnp(crc) => u64::from((!crc).swap_bytes()),
            State::Crc32(digest) => u64::from(digest.finalize()),
            State::Crc64(digest) => digest.finalize(),
            State::Nmea(nmea) => u64::from(nmea.checksum),
        };
        Checksum {
            algo: self.algo,
            value,
        }
    }
}

/// The NMEA 0183 sentence checksum, as a state machine.
///
/// `checksum_NMEA` is the odd one out in libcrc: it is delimiter-driven rather than
/// length-driven. It skips one leading `$` and stops at the first NUL, CR, LF or `*`.
/// Streaming it means remembering both facts across chunk boundaries.
struct Nmea {
    checksum: u8,
    /// Has the optional leading `$` already been considered? Only the very first byte
    /// of the whole input can be one.
    started: bool,
    /// Has a terminator been seen? Everything after it is ignored.
    done: bool,
}

impl Nmea {
    const fn new() -> Self {
        Self {
            checksum: 0,
            started: false,
            done: false,
        }
    }

    fn update(&mut self, data: &[u8]) {
        if self.done || data.is_empty() {
            return;
        }
        let mut data = data;
        if !self.started {
            self.started = true;
            if let Some(rest) = data.strip_prefix(b"$") {
                data = rest;
            }
        }
        for &byte in data {
            if matches!(byte, 0 | b'\r' | b'\n' | b'*') {
                self.done = true;
                return;
            }
            self.checksum ^= byte;
        }
    }
}

/// A finished checksum, tagged with the algorithm that produced it so the printer
/// knows how wide to render it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Checksum {
    pub algo: Algo,
    pub value: u64,
}

impl Checksum {
    pub fn to_hex(self) -> String {
        format!("0x{:0width$X}", self.value, width = self.algo.hex_digits())
    }

    pub fn render(self, hex: bool) -> String {
        if hex {
            self.to_hex()
        } else {
            self.value.to_string()
        }
    }
}

/// Accepts either spelling so a manifest written with `--dec` can be verified by a
/// default (`--hex`) invocation and vice versa.
pub fn parse_value(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(digits) => u64::from_str_radix(digits, 16).ok(),
        None => text.parse::<u64>().ok(),
    }
}

/// `crc --list`. Every check value is computed here and now, not recalled.
pub fn write_list(out: &mut impl Write) -> Result<()> {
    // Padded by hand to the column layout used for the rows below.
    writeln!(
        out,
        "ALGORITHM       BITS  C FUNCTION      CHECK \"123456789\""
    )
    .map_err(CliError::Output)?;
    for algo in ALL {
        let mut digest = algo.digest();
        digest.update(CHECK_STRING);
        writeln!(
            out,
            "{:<15} {:>4}  {:<14}  {}",
            algo.name(),
            algo.bits(),
            algo.c_function(),
            digest.finish().to_hex()
        )
        .map_err(CliError::Output)?;
    }
    writeln!(
        out,
        "\ncrc_kermit, crc_dnp and crc_sick byte-swap their result because libcrc does.\n\
         The RevEng catalogue gives 0x2189 for KERMIT and 0xEA82 for DNP; this port\n\
         reproduces libcrc exactly, including that divergence.\n\
         'nmea' is a sentence checksum, not a CRC: it skips a leading '$' and stops at\n\
         the first NUL, CR, LF or '*'.\n\
         Names are case-insensitive, '-' works wherever '_' does, and short aliases are\n\
         accepted (32, crc32, modbus, kermit, ccitt, ...)."
    )
    .map_err(CliError::Output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oneshot(algo: Algo, data: &[u8]) -> u64 {
        let mut digest = algo.digest();
        digest.update(data);
        digest.finish().value
    }

    /// The values the original C library produces for the canonical check string.
    /// Recorded in `.planning/libcrc-plan/00-VERIFIED-FACTS.md` by running it.
    #[test]
    fn check_string_matches_the_c_original() {
        assert_eq!(oneshot(Algo::Crc16, b"123456789"), 0xBB3D);
        assert_eq!(oneshot(Algo::Modbus, b"123456789"), 0x4B37);
        assert_eq!(oneshot(Algo::Sick, b"123456789"), 0x56A6);
        assert_eq!(oneshot(Algo::Xmodem, b"123456789"), 0x31C3);
        assert_eq!(oneshot(Algo::CcittFfff, b"123456789"), 0x29B1);
        assert_eq!(oneshot(Algo::Ccitt1d0f, b"123456789"), 0xE5CC);
        assert_eq!(oneshot(Algo::Kermit, b"123456789"), 0x8921);
        assert_eq!(oneshot(Algo::Dnp, b"123456789"), 0x82EA);
        assert_eq!(oneshot(Algo::Crc32, b"123456789"), 0xCBF4_3926);
    }

    /// Every streaming state must be split-invariant, at every boundary, or the
    /// chunked reader in `hash.rs` is not equivalent to a one-shot call.
    #[test]
    fn every_algorithm_is_split_invariant() {
        let data = b"$GPGLL,4916.45,N,12311.12,W,225444,A";
        for algo in ALL {
            let whole = oneshot(algo, data);
            for split in 0..=data.len() {
                let mut digest = algo.digest();
                digest.update(&data[..split]);
                digest.update(&data[split..]);
                assert_eq!(
                    digest.finish().value,
                    whole,
                    "{} diverged when split at {split}",
                    algo.name()
                );
            }
        }
    }

    /// Empty chunks are common when a reader returns a short read; they must be inert.
    /// NMEA is the one that can get this wrong, because an empty first chunk must not
    /// consume its one chance to strip a leading '$'.
    #[test]
    fn empty_chunks_are_inert() {
        for algo in ALL {
            let mut digest = algo.digest();
            digest.update(b"");
            digest.update(b"$AB");
            digest.update(b"");
            assert_eq!(
                digest.finish().value,
                oneshot(algo, b"$AB"),
                "{}",
                algo.name()
            );
        }
    }

    #[test]
    fn names_round_trip_and_aliases_resolve() {
        for algo in ALL {
            assert_eq!(Algo::parse(algo.name()), Some(algo));
            assert_eq!(Algo::parse(&algo.name().to_ascii_uppercase()), Some(algo));
            assert_eq!(Algo::parse(&algo.name().replace('_', "-")), Some(algo));
            for alias in algo.aliases() {
                assert_eq!(Algo::parse(alias), Some(algo), "alias {alias}");
            }
        }
        assert_eq!(Algo::parse("no-such-algorithm"), None);
        assert_eq!(Algo::parse(""), None);
    }

    /// Aliases must be unambiguous, or `--algo 32` silently means something else after
    /// a later edit.
    #[test]
    fn no_name_or_alias_is_claimed_twice() {
        let mut seen: Vec<&str> = Vec::new();
        for algo in ALL {
            for spelling in std::iter::once(algo.name()).chain(algo.aliases().iter().copied()) {
                assert!(!seen.contains(&spelling), "duplicate spelling {spelling}");
                seen.push(spelling);
            }
        }
    }

    #[test]
    fn values_parse_in_hex_and_decimal() {
        assert_eq!(parse_value("0xBB3D"), Some(0xBB3D));
        assert_eq!(parse_value("0XBB3D"), Some(0xBB3D));
        assert_eq!(parse_value(" 47933 "), Some(47933));
        assert_eq!(parse_value("0xBB3D"), parse_value("47933"));
        assert_eq!(parse_value("0xZZ"), None);
        assert_eq!(parse_value("-1"), None);
        assert_eq!(parse_value(""), None);
    }

    #[test]
    fn rendering_is_zero_padded_to_the_algorithm_width() {
        let eight = Checksum {
            algo: Algo::Crc8,
            value: 0x05,
        };
        assert_eq!(eight.to_hex(), "0x05");
        assert_eq!(eight.render(false), "5");
        let sixty_four = Checksum {
            algo: Algo::Crc64Ecma,
            value: 0x1F,
        };
        assert_eq!(sixty_four.to_hex(), "0x000000000000001F");
    }

    #[test]
    fn nmea_stops_at_its_terminator_across_chunks() {
        let mut digest = Algo::Nmea.digest();
        digest.update(b"$GPGLL,x");
        digest.update(b"*7C");
        digest.update(b"trailing garbage");
        assert_eq!(digest.finish().value, oneshot(Algo::Nmea, b"GPGLL,x"));
    }
}
