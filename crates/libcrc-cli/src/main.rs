//! `crc` — a command-line front end for the libcrc Rust port.
//!
//! The original C project ships `examples/tstcrc.c`, which builds a small CRC utility.
//! This is the same idea done properly: constant memory on inputs of any size, all
//! thirteen algorithms in one pass, a verifiable manifest format, and no interactive
//! prompt to deadlock a pipeline.
//!
//! Design constraints, all deliberate:
//!
//! * **No dependencies.** The whole workspace resolves to three path crates and
//!   nothing from the network. Argument parsing is 150 lines and is easier to audit
//!   than a dependency tree.
//! * **No panics on input.** Every failure — missing file, directory, permission
//!   denied, malformed manifest, non-UTF-8 argument, closed pipe — travels as a
//!   `CliError` and leaves as one line on stderr and a non-zero status.
//! * **No unsafe.** `forbid(unsafe_code)` here as in the port itself.
//!
//! `libcrc-rs` is a `no_std` crate; this binary is the layer that is allowed to use
//! `std`, which is why the two are separate crates.
#![forbid(unsafe_code)]

mod algo;
mod args;
mod check;
mod error;
mod hash;
mod help;

use std::io::{self, Write};
use std::process::ExitCode;

use crate::args::{Mode, Options};
use crate::error::{CliError, Result};

fn main() -> ExitCode {
    let argv = match collect_args() {
        Ok(argv) => argv,
        Err(error) => return report(&error),
    };
    match run(&argv) {
        Ok(code) => code,
        Err(error) => report(&error),
    }
}

/// `std::env::args()` panics on an argument that is not valid UTF-8. This program must
/// never panic on input, so the conversion is done by hand and a bad argument becomes
/// an ordinary usage error.
fn collect_args() -> Result<Vec<String>> {
    std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string().map_err(|bad| {
                CliError::usage(format!(
                    "argument is not valid UTF-8: {:?}",
                    bad.to_string_lossy()
                ))
            })
        })
        .collect()
}

fn report(error: &CliError) -> ExitCode {
    // `crc --all huge | head -1` closes the pipe deliberately. Reporting that as a
    // failure would be wrong, and panicking on it — which is what println! does — would
    // be worse.
    if error.is_broken_pipe() {
        return ExitCode::SUCCESS;
    }
    let mut stderr = io::stderr();
    let _ = writeln!(stderr, "crc: {error}");
    if error.is_usage() {
        let _ = writeln!(stderr, "Try 'crc --help' for more information.");
    }
    ExitCode::from(error.exit_code())
}

fn run(argv: &[String]) -> Result<ExitCode> {
    let options = args::parse(argv)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let code = match options.mode {
        Mode::Help => {
            help::write(&mut out)?;
            ExitCode::SUCCESS
        }
        Mode::Version => {
            writeln!(
                out,
                "crc {} — libcrc-rs port of lammertb/libcrc",
                env!("CARGO_PKG_VERSION")
            )
            .map_err(CliError::Output)?;
            ExitCode::SUCCESS
        }
        Mode::List => {
            algo::write_list(&mut out)?;
            ExitCode::SUCCESS
        }
        Mode::Run => match options.check.as_deref() {
            Some(manifest) => check::verify(manifest, &mut out)?,
            None => hash_inputs(&options, &mut out)?,
        },
    };

    out.flush().map_err(CliError::Output)?;
    Ok(code)
}

/// One unreadable input does not abandon the rest: the others are still checksummed
/// and the process exits 1 at the end, which is what `md5sum` and friends do.
fn hash_inputs(options: &Options, out: &mut impl Write) -> Result<ExitCode> {
    let layout = Layout::for_algos(options);
    let mut buf = vec![0u8; hash::CHUNK];
    let mut failed = false;

    for path in &options.files {
        match hash::checksums(path, &options.algos, &mut buf) {
            Ok(sums) => {
                for sum in sums {
                    writeln!(
                        out,
                        "{:<name$}  {:<value$}  {}",
                        sum.algo.name(),
                        sum.render(options.hex),
                        path,
                        name = layout.name,
                        value = layout.value
                    )
                    .map_err(CliError::Output)?;
                }
            }
            Err(error) => {
                // Keep the two streams in order for anyone reading a combined log.
                out.flush().map_err(CliError::Output)?;
                let _ = writeln!(io::stderr(), "crc: {error}");
                failed = true;
            }
        }
    }

    if failed {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// Column widths, so `--all` lines up. Hex widths are known before anything is read;
/// decimal widths are not, so decimal output is left unpadded rather than padded to a
/// pessimistic twenty columns.
struct Layout {
    name: usize,
    value: usize,
}

impl Layout {
    fn for_algos(options: &Options) -> Self {
        let name = options
            .algos
            .iter()
            .map(|algo| algo.name().len())
            .max()
            .unwrap_or(0);
        let value = if options.hex {
            options
                .algos
                .iter()
                .map(|algo| algo.hex_digits() + 2)
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        Self { name, value }
    }
}
