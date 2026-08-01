//! One error type for the whole CLI.
//!
//! Every failure path in this binary returns a `CliError`. Nothing unwraps, nothing
//! panics on user input: a missing file, a directory passed as a file, a malformed
//! manifest line, a non-UTF-8 argument and a closed stdout all arrive here and are
//! printed as a single diagnostic line on stderr.

use std::fmt;
use std::io;

/// Shorthand used throughout the crate.
pub type Result<T> = std::result::Result<T, CliError>;

#[derive(Debug)]
pub enum CliError {
    /// The command line itself was wrong. Exits 2, like most POSIX tools.
    Usage(String),
    /// An input could not be opened or read.
    Io { path: String, source: io::Error },
    /// A `--check` manifest could not be parsed.
    Manifest {
        path: String,
        line: usize,
        detail: String,
    },
    /// Writing to stdout failed. Broken pipe is treated as success — `crc --all big |
    /// head -1` closes the pipe on purpose and must not be reported as an error.
    Output(io::Error),
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        CliError::Usage(message.into())
    }

    pub fn io(path: impl Into<String>, source: io::Error) -> Self {
        CliError::Io {
            path: path.into(),
            source,
        }
    }

    pub fn is_usage(&self) -> bool {
        matches!(self, CliError::Usage(_))
    }

    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, CliError::Output(e) if e.kind() == io::ErrorKind::BrokenPipe)
    }

    pub fn exit_code(&self) -> u8 {
        if self.is_usage() {
            2
        } else {
            1
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(message) => write!(f, "{message}"),
            CliError::Io { path, source } => write!(f, "{path}: {source}"),
            CliError::Manifest { path, line, detail } => write!(f, "{path}:{line}: {detail}"),
            CliError::Output(source) => write!(f, "write error: {source}"),
        }
    }
}
