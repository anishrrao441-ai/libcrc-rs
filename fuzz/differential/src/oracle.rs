//! Talking to the original C library — strictly one batch per process lifetime.
//!
//! # Why there is no streaming protocol here
//!
//! The obvious design is a long-lived oracle process fed inputs on stdin and answering on
//! stdout. On Windows that design can deadlock: anonymous pipes have a fixed buffer, and
//! once the oracle's stdout buffer fills it blocks in `write` while the fuzzer is still
//! blocked in `write` on the oracle's stdin. Neither side drains the other. It is the one
//! failure mode that could quietly cost this entire exercise.
//!
//! CRC is a pure function of its input, so there is no reason to interleave anything.
//! This module therefore:
//!
//!   1. writes the whole batch to a file,
//!   2. runs the oracle **once**, with stdin closed so no prompt can ever block it,
//!   3. waits for it to exit,
//!   4. reads the whole result file.
//!
//! Nothing is ever read from a pipe, so there is no pipe to deadlock on. Closing stdin is
//! belt and braces: libcrc's own `examples/tstcrc.c` has interactive modes that prompt on
//! stdin, and a hang there would look exactly like a fuzzer bug.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cases::Batch;
use crate::model::{Block, BLOCK_BYTES, MAGIC_RESULTS, RECORD_BYTES};

pub struct Oracle {
    exe: PathBuf,
    cases_path: PathBuf,
    results_path: PathBuf,
}

/// One batch's worth of oracle output, kept as raw bytes so decoding stays lazy.
pub struct OracleResults {
    raw: Vec<u8>,
    count: usize,
}

impl OracleResults {
    /// The one-shot block for case `i`.
    pub fn oneshot(&self, i: usize) -> Block {
        debug_assert!(i < self.count, "case {i} is outside a {}-case result set", self.count);
        let at = 12 + i * RECORD_BYTES;
        Block::decode(&self.raw[at..at + BLOCK_BYTES])
    }

    /// The incremental block for case `i`.
    pub fn incremental(&self, i: usize) -> Block {
        debug_assert!(i < self.count, "case {i} is outside a {}-case result set", self.count);
        let at = 12 + i * RECORD_BYTES + BLOCK_BYTES;
        Block::decode(&self.raw[at..at + BLOCK_BYTES])
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl Oracle {
    pub fn new(exe: &Path, workdir: &Path) -> io::Result<Oracle> {
        fs::create_dir_all(workdir)?;
        if !exe.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "oracle harness not found at {}\n\
                     Build it with fuzz/run.sh, which also builds the C library it links.",
                    exe.display()
                ),
            ));
        }
        Ok(Oracle {
            exe: exe.to_path_buf(),
            cases_path: workdir.join("cases.bin"),
            results_path: workdir.join("results.bin"),
        })
    }

    /// Run one batch to completion.
    pub fn run(&self, batch: &Batch) -> io::Result<OracleResults> {
        fs::write(&self.cases_path, &batch.blob)?;

        let status = Command::new(&self.exe)
            .arg(&self.cases_path)
            .arg(&self.results_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;

        if !status.success() {
            return Err(invalid(format!(
                "oracle harness exited with {status}; batch of {} cases left at {}",
                batch.spans.len(),
                self.cases_path.display()
            )));
        }

        let raw = fs::read(&self.results_path)?;
        self.validate(raw, batch.spans.len())
    }

    fn validate(&self, raw: Vec<u8>, expected: usize) -> io::Result<OracleResults> {
        if raw.len() < 12 || raw[0..4] != MAGIC_RESULTS {
            return Err(invalid("result file has the wrong magic".into()));
        }
        let version = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        if version != 1 {
            return Err(invalid(format!("unsupported result-file version {version}")));
        }
        let count = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]) as usize;
        if count != expected {
            return Err(invalid(format!(
                "oracle answered {count} cases but was asked {expected}"
            )));
        }
        let want = 12 + count * RECORD_BYTES;
        if raw.len() != want {
            return Err(invalid(format!(
                "result file is {} bytes, expected {want} for {count} cases",
                raw.len()
            )));
        }
        Ok(OracleResults { raw, count })
    }
}
