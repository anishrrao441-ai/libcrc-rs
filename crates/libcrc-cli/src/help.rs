//! `crc --help`.

use std::io::Write;

use crate::error::{CliError, Result};

const HELP: &str = "\
crc — checksums from the libcrc Rust port. Thirteen algorithms, zero dependencies.

USAGE
    crc [OPTIONS] [FILE...]
    crc --check MANIFEST
    crc --list

    With no FILE, or wherever FILE is \"-\", input is read from stdin.

OPTIONS
    -a, --algo NAME    Algorithm to compute. Repeatable, and accepts a comma-separated
                       list. Names are case-insensitive and short aliases work
                       (32, crc32, modbus, kermit, ...). Default: crc_16,crc_32
        --all          Compute all thirteen algorithms
    -c, --check FILE   Verify a manifest previously written by crc. Use \"-\" to read
                       the manifest from stdin
        --list         List every supported algorithm with its check value, and exit
        --hex          Print values as 0x-prefixed, zero-padded hex (the default)
        --dec          Print values as unsigned decimal
    -h, --help         Print this help and exit
    -V, --version      Print the version and exit
    --                 Treat every later argument as a FILE, even if it begins with -

OUTPUT
    One line per file per algorithm:

        <algorithm>  <value>  <path>

    That is exactly the format --check consumes, so

        crc --all data.bin > SUMS
        crc --check SUMS

    is a round trip. --check accepts hex or decimal values whichever flag wrote them,
    ignores blank lines and lines starting with '#', and preserves spaces in paths.

EXIT STATUS
    0   everything requested succeeded
    1   an input could not be read, or a --check entry did not match
    2   the command line was malformed

NOTES
    * Input is read in 64 KiB chunks and folded through the port's streaming Digest
      types, so memory use is constant however large FILE is, and every algorithm
      requested is computed in the same single pass over the data. The C original has
      no streaming API at all: it offers one-shot crc_16(ptr, len) and byte-at-a-time
      update_crc_16(crc, c), so a CLI built on it must either buffer the whole file or
      hand-roll the loop and each algorithm's finalisation rule.

    * This port reproduces libcrc exactly, including where libcrc disagrees with the
      RevEng CRC catalogue. crc_kermit, crc_dnp and crc_sick byte-swap their result:
      crc_kermit of \"123456789\" is 0x8921 here and 0x2189 in the catalogue. That is
      not a bug in the port; it is the behaviour being ported. Run `crc --list` to see
      every value this binary produces.

    * 'nmea' is the NMEA 0183 sentence checksum, not a CRC. It skips one leading '$'
      and stops at the first NUL, CR, LF or '*', so over an arbitrary file it reads
      only as far as the first such byte — by design, matching checksum_NMEA().

EXAMPLES
    crc README.md                       crc_16 and crc_32 of one file
    crc --all --dec data.bin            all thirteen, in decimal
    crc -a crc_32 *.bin > SUMS          write a manifest
    crc --check SUMS                    verify it later
    printf '123456789' | crc --all      the canonical CRC check string
    tar cf - dir | crc -a crc_64_ecma   checksum a stream that is never a file
";

pub fn write(out: &mut impl Write) -> Result<()> {
    out.write_all(HELP.as_bytes()).map_err(CliError::Output)
}
