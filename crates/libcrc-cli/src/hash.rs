//! Reading inputs in fixed-size chunks.
//!
//! Nothing here ever holds a whole file in memory. One 64 KiB buffer is allocated per
//! process and reused for every input, so `crc --all a-40-gigabyte-file` costs the same
//! resident memory as `crc /dev/null`. That is only possible because the port exposes
//! streaming digests; the C original offers one-shot `crc_16(ptr, len)` and
//! byte-at-a-time `update_crc_16(crc, c)` and nothing in between.
//!
//! All the algorithms selected for one input are folded in the same pass, so `--all`
//! reads the file once, not thirteen times.

use std::fs;
use std::io::{self, Read};

use crate::algo::{Algo, Checksum, Digest};
use crate::error::{CliError, Result};

/// 64 KiB. Large enough that the per-read syscall cost disappears, small enough to
/// stay comfortably inside a cache and inside a small embedded stack budget.
pub const CHUNK: usize = 64 * 1024;

/// Compute every requested algorithm over one input in a single pass.
///
/// `path` is `-` for stdin. The returned checksums are in the same order as `algos`.
pub fn checksums(path: &str, algos: &[Algo], buf: &mut [u8]) -> Result<Vec<Checksum>> {
    if path == "-" {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        return stream(&mut reader, algos, buf).map_err(|e| CliError::io("-", e));
    }

    // Checked before opening: on Unix a directory opens cleanly and only fails on the
    // first read, and on Windows it fails with a bare "Access is denied". Neither
    // message tells the user what is actually wrong.
    let metadata = fs::metadata(path).map_err(|e| CliError::io(path, e))?;
    if metadata.is_dir() {
        return Err(CliError::io(path, io::Error::other("is a directory")));
    }

    let mut file = fs::File::open(path).map_err(|e| CliError::io(path, e))?;
    stream(&mut file, algos, buf).map_err(|e| CliError::io(path, e))
}

fn stream<R: Read>(reader: &mut R, algos: &[Algo], buf: &mut [u8]) -> io::Result<Vec<Checksum>> {
    let mut digests: Vec<Digest> = algos.iter().map(|algo| algo.digest()).collect();
    loop {
        match reader.read(buf) {
            Ok(0) => break,
            Ok(read) => {
                let chunk = &buf[..read];
                for digest in &mut digests {
                    digest.update(chunk);
                }
            }
            // A signal arriving mid-read is not an error; retry rather than report it.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(digests.into_iter().map(Digest::finish).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that hands out one byte at a time, then a spurious `Interrupted`, to
    /// prove neither short reads nor retryable errors change the answer.
    struct Dribble<'a> {
        data: &'a [u8],
        interrupt_next: bool,
    }

    impl Read for Dribble<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.interrupt_next {
                self.interrupt_next = false;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "test"));
            }
            if self.data.is_empty() || out.is_empty() {
                return Ok(0);
            }
            self.interrupt_next = true;
            out[0] = self.data[0];
            self.data = &self.data[1..];
            Ok(1)
        }
    }

    #[test]
    fn short_reads_and_interrupts_do_not_change_the_result() {
        let data = b"123456789";
        let mut buf = vec![0u8; CHUNK];
        let mut dribble = Dribble {
            data,
            interrupt_next: false,
        };
        let streamed = stream(&mut dribble, &crate::algo::ALL, &mut buf).unwrap();

        let mut whole = &data[..];
        let mut buf2 = vec![0u8; CHUNK];
        let bulk = stream(&mut whole, &crate::algo::ALL, &mut buf2).unwrap();

        assert_eq!(streamed, bulk);
        // The canonical check value, straight from the original C library.
        let crc32 = streamed
            .iter()
            .find(|c| c.algo == Algo::Crc32)
            .expect("crc_32 requested");
        assert_eq!(crc32.value, 0xCBF4_3926);
    }

    /// A buffer smaller than the input forces many chunk boundaries. The result must
    /// not move.
    #[test]
    fn the_buffer_size_is_not_observable() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut reference = &data[..];
        let mut big = vec![0u8; CHUNK];
        let expected = stream(&mut reference, &crate::algo::ALL, &mut big).unwrap();

        for size in [1usize, 2, 3, 7, 64, 511, 4096] {
            let mut input = &data[..];
            let mut small = vec![0u8; size];
            let got = stream(&mut input, &crate::algo::ALL, &mut small).unwrap();
            assert_eq!(got, expected, "buffer size {size}");
        }
    }

    #[test]
    fn empty_input_yields_the_seed_values() {
        let mut empty: &[u8] = b"";
        let mut buf = vec![0u8; CHUNK];
        let sums = stream(&mut empty, &crate::algo::ALL, &mut buf).unwrap();
        let value = |algo: Algo| sums.iter().find(|c| c.algo == algo).unwrap().value;
        // Recorded from the original C library; see 00-VERIFIED-FACTS.md §11.
        assert_eq!(value(Algo::Crc16), 0x0000);
        assert_eq!(value(Algo::Modbus), 0xFFFF);
        assert_eq!(value(Algo::Sick), 0x0000);
        assert_eq!(value(Algo::Xmodem), 0x0000);
        assert_eq!(value(Algo::CcittFfff), 0xFFFF);
        assert_eq!(value(Algo::Ccitt1d0f), 0x1D0F);
        assert_eq!(value(Algo::Kermit), 0x0000);
        assert_eq!(value(Algo::Dnp), 0xFFFF);
        assert_eq!(value(Algo::Crc32), 0x0000_0000);
    }

    #[test]
    fn a_directory_is_reported_as_one_rather_than_read() {
        let mut buf = vec![0u8; CHUNK];
        let dir = std::env::temp_dir();
        let path = dir.to_string_lossy().to_string();
        let err = checksums(&path, &crate::algo::DEFAULT, &mut buf).unwrap_err();
        assert!(
            err.to_string().contains("is a directory"),
            "unexpected: {err}"
        );
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let mut buf = vec![0u8; CHUNK];
        let err = checksums(
            "definitely-not-a-real-file-9f3a2c.bin",
            &crate::algo::DEFAULT,
            &mut buf,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().starts_with("definitely-not-a-real-file"));
    }
}
