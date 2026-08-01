//! `crc --check MANIFEST` — verify a checksum list this program wrote earlier.
//!
//! The manifest format is exactly the output format:
//!
//! ```text
//! crc_32  0xCBF43926  path/with spaces/in it.bin
//! ```
//!
//! Fields are separated by runs of whitespace; the path is everything after the second
//! run, so interior spaces survive a round trip. Blank lines and `#` comments are
//! ignored. Values are accepted in hex or decimal regardless of which flag wrote them.
//!
//! Entries are grouped by path before verification, so a manifest listing all thirteen
//! algorithms for one file reads that file once.

use std::fs;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use crate::algo::{parse_value, Algo};
use crate::error::{CliError, Result};
use crate::hash;

#[derive(Debug)]
struct Entry {
    algo: Algo,
    expected: u64,
    path: String,
}

/// Returns the process exit code: success, or 1 if anything failed to match or to read.
pub fn verify(manifest: &str, out: &mut impl Write) -> Result<ExitCode> {
    let text = read_manifest(manifest)?;
    let entries = parse_manifest(&text, manifest)?;
    if entries.is_empty() {
        return Err(CliError::Manifest {
            path: manifest.to_string(),
            line: 0,
            detail: "no checksum lines found".to_string(),
        });
    }

    let mut buf = vec![0u8; hash::CHUNK];
    let mut mismatched = 0usize;
    let mut unreadable = 0usize;

    for group in group_by_path(&entries) {
        let algos: Vec<Algo> = group.iter().map(|entry| entry.algo).collect();
        let path = group[0].path.as_str();
        match hash::checksums(path, &algos, &mut buf) {
            Ok(sums) => {
                for (entry, sum) in group.iter().zip(sums) {
                    if sum.value == entry.expected {
                        writeln!(out, "{}: {} OK", entry.path, entry.algo.name())
                            .map_err(CliError::Output)?;
                    } else {
                        mismatched += 1;
                        writeln!(out, "{}: {} FAILED", entry.path, entry.algo.name())
                            .map_err(CliError::Output)?;
                        out.flush().map_err(CliError::Output)?;
                        let _ = writeln!(
                            io::stderr(),
                            "crc: {}: {} is {}, manifest says {}",
                            entry.path,
                            entry.algo.name(),
                            sum.to_hex(),
                            crate::algo::Checksum {
                                algo: entry.algo,
                                value: entry.expected
                            }
                            .to_hex()
                        );
                    }
                }
            }
            Err(error) => {
                unreadable += group.len();
                for entry in group {
                    writeln!(
                        out,
                        "{}: {} FAILED open or read",
                        entry.path,
                        entry.algo.name()
                    )
                    .map_err(CliError::Output)?;
                }
                out.flush().map_err(CliError::Output)?;
                let _ = writeln!(io::stderr(), "crc: {error}");
            }
        }
    }

    out.flush().map_err(CliError::Output)?;
    report(entries.len(), mismatched, unreadable);
    if mismatched == 0 && unreadable == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

fn report(total: usize, mismatched: usize, unreadable: usize) {
    let mut stderr = io::stderr();
    if mismatched > 0 {
        let _ = writeln!(
            stderr,
            "crc: WARNING: {mismatched} of {total} computed checksums did NOT match"
        );
    }
    if unreadable > 0 {
        let plural = if unreadable == 1 { "entry" } else { "entries" };
        let _ = writeln!(
            stderr,
            "crc: WARNING: {unreadable} listed {plural} could not be read"
        );
    }
}

fn read_manifest(manifest: &str) -> Result<String> {
    if manifest == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::io("-", e))?;
        return Ok(text);
    }
    fs::read_to_string(manifest).map_err(|e| CliError::io(manifest, e))
}

fn parse_manifest(text: &str, manifest: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for (offset, raw) in text.lines().enumerate() {
        let number = offset + 1;
        // `lines()` strips \n but not the \r of a CRLF manifest.
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let entry = parse_line(line).ok_or_else(|| CliError::Manifest {
            path: manifest.to_string(),
            line: number,
            detail: format!("expected '<algorithm>  <value>  <path>', found {trimmed:?}"),
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

fn parse_line(line: &str) -> Option<Entry> {
    let (name, rest) = next_field(line)?;
    let (value, path) = next_field(rest)?;
    if path.is_empty() {
        return None;
    }
    Some(Entry {
        algo: Algo::parse(name)?,
        expected: parse_value(value)?,
        path: path.to_string(),
    })
}

/// Take one whitespace-delimited field and return it with the remainder, which has had
/// the separating whitespace removed but is otherwise untouched — so a path containing
/// spaces survives.
fn next_field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace)?;
    let (field, rest) = text.split_at(end);
    Some((field, rest.trim_start()))
}

/// Consecutive-or-not, every entry naming the same path ends up in one group, and the
/// groups appear in order of first mention.
fn group_by_path(entries: &[Entry]) -> Vec<Vec<&Entry>> {
    let mut groups: Vec<Vec<&Entry>> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|g| g[0].path == entry.path) {
            Some(group) => group.push(entry),
            None => groups.push(vec![entry]),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_manifest_parses() {
        let text = "# written by crc\n\
                    crc_32  0xCBF43926  a.bin\n\
                    \n\
                    crc_16  47933  a.bin\n\
                    crc_8   0x01    dir/with space.bin\n";
        let entries = parse_manifest(text, "sums").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].algo, Algo::Crc32);
        assert_eq!(entries[0].expected, 0xCBF4_3926);
        assert_eq!(entries[0].path, "a.bin");
        // Decimal and hex must be interchangeable: 47933 == 0xBB3D.
        assert_eq!(entries[1].expected, 0xBB3D);
        assert_eq!(entries[2].path, "dir/with space.bin");
    }

    #[test]
    fn crlf_manifests_parse() {
        let entries = parse_manifest("crc_32  0x00000000  a.bin\r\n", "sums").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.bin");
    }

    #[test]
    fn malformed_lines_are_reported_with_a_line_number() {
        for (text, line) in [
            ("crc_32 0xCBF43926\n", 1),
            ("\nnot_an_algo 0x1 a.bin\n", 2),
            ("crc_32  zzz  a.bin\n", 1),
            ("crc_32\n", 1),
        ] {
            let err = parse_manifest(text, "sums").unwrap_err();
            match err {
                CliError::Manifest { line: got, .. } => assert_eq!(got, line, "{text:?}"),
                other => panic!("expected a manifest error, got {other}"),
            }
        }
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        assert!(parse_manifest("# only a comment\n\n   \n", "sums")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn entries_are_grouped_by_path_in_first_mention_order() {
        let text = "crc_32  0x1  b.bin\ncrc_16  0x2  a.bin\ncrc_8  0x3  b.bin\n";
        let entries = parse_manifest(text, "sums").unwrap();
        let groups = group_by_path(&entries);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0][0].path, "b.bin");
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1][0].path, "a.bin");
    }
}
