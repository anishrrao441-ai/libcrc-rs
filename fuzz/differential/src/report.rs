//! Run statistics and the published `fuzz/log.txt`.
//!
//! The log is the artifact, so it records what was actually measured — seed, wall clock,
//! case count, the input mix, and every divergence by name — plus the exact command to
//! replay it. Numbers that were not measured do not appear.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::cases::Class;
use crate::model::{Diff, INCREMENTAL_CHECKS, ONESHOT_CHECKS};

pub struct RunConfig {
    pub seed: u64,
    pub start_index: u64,
    pub batch_size: usize,
    pub requested_seconds: f64,
    pub requested_cases: Option<u64>,
    pub oracle_exe: String,
}

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

pub struct Stats {
    pub cases: u64,
    pub payload_bytes: u64,
    pub batches: u64,
    pub class_counts: [u64; Class::ALL.len()],
    pub divergences: u64,
    /// FNV-1a over every case the run actually fed to both sides.
    ///
    /// This turns "reproducible from the recorded seed" from a claim into something a
    /// reader can check: replay the seed at a *different* `--batch` and the digest must
    /// come out identical. FNV rather than one of the CRCs under test, so the check does
    /// not depend on the thing being tested.
    pub stream_digest: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            cases: 0,
            payload_bytes: 0,
            batches: 0,
            class_counts: [0; Class::ALL.len()],
            divergences: 0,
            stream_digest: FNV_OFFSET,
        }
    }
}

impl Stats {
    pub fn record_class(&mut self, class: Class) {
        if let Some(slot) = Class::ALL.iter().position(|&c| c == class) {
            self.class_counts[slot] += 1;
        }
    }

    /// Fold one case into the stream digest: its index, its NULL flag, the length handed
    /// to the C API, and the bytes the port saw.
    pub fn mix(&mut self, index: u64, is_null: bool, wire_len: usize, payload: &[u8]) {
        let mut h = self.stream_digest;
        let mut fold = |byte: u8| h = (h ^ byte as u64).wrapping_mul(FNV_PRIME);
        for b in index.to_le_bytes() {
            fold(b);
        }
        fold(u8::from(is_null));
        for b in (wire_len as u64).to_le_bytes() {
            fold(b);
        }
        for &b in payload {
            fold(b);
        }
        self.stream_digest = h;
    }

    /// 13 one-shot values plus 12 incremental values are compared per case.
    pub fn value_comparisons(&self) -> u64 {
        self.cases * (ONESHOT_CHECKS.len() as u64 + INCREMENTAL_CHECKS.len() as u64)
    }
}

/// A divergence, with the shrunk input if minimisation succeeded.
pub struct Divergence {
    pub index: u64,
    pub class: Class,
    pub len: usize,
    pub is_null: bool,
    pub data: Vec<u8>,
    pub diffs: Vec<Diff>,
    pub minimised: Option<Minimised>,
}

pub struct Minimised {
    pub data: Vec<u8>,
    pub is_null: bool,
    pub diffs: Vec<Diff>,
    pub oracle_calls: u64,
}

fn tool_version(cmd: &str, args: &[&str]) -> String {
    // `output()` drains both pipes itself, and these processes print a line and exit, so
    // the pipe-deadlock concern that shapes oracle.rs does not apply here.
    match Command::new(cmd).args(args).output() {
        Ok(out) => {
            let text = if out.stdout.is_empty() { out.stderr } else { out.stdout };
            String::from_utf8_lossy(&text)
                .lines()
                .next()
                .unwrap_or("(no output)")
                .trim()
                .to_string()
        }
        Err(e) => format!("(unavailable: {e})"),
    }
}

fn hexdump(data: &[u8], limit: usize) -> String {
    let shown = data.len().min(limit);
    let mut s = String::new();
    for (i, b) in data[..shown].iter().enumerate() {
        if i > 0 && i % 16 == 0 {
            s.push('\n');
            s.push_str("      ");
        } else if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02X}");
    }
    if data.len() > shown {
        let _ = write!(s, " ... (+{} more bytes)", data.len() - shown);
    }
    if s.is_empty() {
        s.push_str("(empty)");
    }
    s
}

fn printable(data: &[u8], limit: usize) -> String {
    data.iter()
        .take(limit)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
        .collect()
}

pub fn render(
    cfg: &RunConfig,
    stats: &Stats,
    elapsed: Duration,
    divergences: &[Divergence],
    golden_ok: bool,
) -> String {
    let secs = elapsed.as_secs_f64();
    let rate = if secs > 0.0 { stats.cases as f64 / secs } else { 0.0 };
    let mib = stats.payload_bytes as f64 / (1024.0 * 1024.0);

    let mut s = String::new();
    let w = &mut s;

    let _ = writeln!(w, "=========================================================================");
    let _ = writeln!(w, " libcrc differential fuzz run — original C library vs. Rust port");
    let _ = writeln!(w, " Port Mortem 2026 · track C -> Rust · lammertb/libcrc");
    let _ = writeln!(w, "=========================================================================");
    let _ = writeln!(w);

    // ---------------------------------------------------------------- headline
    let _ = writeln!(w, "RESULT");
    let _ = writeln!(w, "  seed                  0x{:016X}  ({})", cfg.seed, cfg.seed);
    let _ = writeln!(w, "  wall-clock duration   {secs:.3} s (continuous, single uninterrupted run)");
    let _ = writeln!(w, "  cases executed        {}", stats.cases);
    let _ = writeln!(w, "  cases / second        {rate:.0}");
    let _ = writeln!(w, "  value comparisons     {}  (25 per case)", stats.value_comparisons());
    let _ = writeln!(w, "  payload hashed        {mib:.1} MiB per algorithm");
    let _ = writeln!(w, "  batches               {}  ({} cases per batch)", stats.batches, cfg.batch_size);
    let _ = writeln!(w, "  input stream digest   0x{:016X}  (FNV-1a over every case)", stats.stream_digest);
    let _ = writeln!(w, "  DIVERGENCES           {}", stats.divergences);
    let _ = writeln!(w);

    // ------------------------------------------------------------------ replay
    let _ = writeln!(w, "REPLAY (bit-for-bit; case N depends only on the seed and on N)");
    let _ = writeln!(w, "  ./fuzz/run.sh --seed {} --cases {}", cfg.seed, stats.cases);
    let _ = writeln!(w);
    let _ = writeln!(w, "  Or drive the binary directly, from the repo root:");
    let _ = writeln!(
        w,
        "    ./fuzz/differential/target/release/difffuzz \\\n\
         \x20       --seed {} --cases {} --batch {} --start {} \\\n\
         \x20       --oracle {} --workdir fuzz/build/work --log fuzz/log.txt",
        cfg.seed, stats.cases, cfg.batch_size, cfg.start_index, cfg.oracle_exe
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "  A single case can be re-run on its own:");
    let _ = writeln!(w, "    ... --seed {} --case <INDEX>", cfg.seed);
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "  Batch size does not affect the stream: inputs are derived per case as\n\
         \x20 SplitMix64(seed ^ SplitMix64(index)), so any --batch replays the same bytes.\n\
         \n\
         \x20 That is checkable rather than merely asserted. Re-run the seed at a DIFFERENT\n\
         \x20 batch size and the input stream digest above must come out identical:\n\
         \x20   ./fuzz/run.sh --seed {} --cases {} --batch 7919\n\
         \x20   -> input stream digest must be 0x{:016X}",
        cfg.seed, stats.cases, stats.stream_digest
    );
    if cfg.requested_cases.is_none() {
        let _ = writeln!(
            w,
            "  This run was time-bounded (--seconds {:.0}); the replay above is case-bounded\n\
             \x20 at the count actually reached, which reproduces it exactly.",
            cfg.requested_seconds
        );
    }
    let _ = writeln!(w);

    // ------------------------------------------------------------------ design
    let _ = writeln!(w, "HOW THE TWO SIDES ARE COMPARED");
    let _ = writeln!(
        w,
        "  Shared public API. Both sides are handed identical bytes and asked for all 13\n\
         \x20 exported entry points, plus the same twelve values rebuilt one byte at a time\n\
         \x20 through the 8 update_crc_* functions.\n\
         \n\
         \x20 Oracle   the ORIGINAL C libcrc, built from the pristine upstream tree with the\n\
         \x20          project's own CFLAGS (-O3 -funsigned-char). It lives in the gitignored\n\
         \x20          oracle/ directory and nothing in crates/ links, calls or depends on it.\n\
         \x20 Port     crates/libcrc-rs, called directly as a Rust library. No FFI, no shim,\n\
         \x20          no C in the measured path.\n\
         \n\
         \x20 BATCH, NOT STREAM. Each round writes a whole batch to a file, runs the oracle\n\
         \x20 once with stdin closed, waits for it to exit, then reads the results file. No\n\
         \x20 pipe is ever read, so the Windows anonymous-pipe deadlock that a long-lived\n\
         \x20 bidirectional oracle would risk has no surface here. CRC is a pure function;\n\
         \x20 there is nothing to interleave."
    );
    let _ = writeln!(w);

    // ---------------------------------------------------------------- coverage
    let _ = writeln!(w, "API COVERAGE — 13 one-shot + 12 incremental checks per case");
    for name in ONESHOT_CHECKS.iter() {
        let _ = writeln!(w, "  {:<32} {} cases", name, stats.cases);
    }
    for name in INCREMENTAL_CHECKS.iter() {
        let _ = writeln!(w, "  {:<32} {} cases", name, stats.cases);
    }
    let _ = writeln!(
        w,
        "\n  checksum_NMEA has no incremental form in libcrc, hence 12 rather than 13.\n\
         \x20 update_crc_64 is used for both CRC-64 seeds. The header's documented\n\
         \x20 update_crc_64_ecma() cannot be called at all — see UPSTREAM NOTE below."
    );
    let _ = writeln!(w);

    // ------------------------------------------------------------------- input
    let _ = writeln!(w, "INPUT MIX");
    for (slot, class) in Class::ALL.iter().enumerate() {
        let n = stats.class_counts[slot];
        if n == 0 {
            continue;
        }
        let pct = 100.0 * n as f64 / stats.cases.max(1) as f64;
        let _ = writeln!(w, "  {:<28} {:>12}  {:>6.2}%", class.name(), n, pct);
    }
    let _ = writeln!(
        w,
        "\n  The 'fixed-corpus' class is a deterministic prologue that runs before any\n\
         \x20 random input, so these are covered on EVERY run rather than probabilistically:\n\
         \x20   - empty input\n\
         \x20   - all 256 byte values as a single byte, and as a 17-byte uniform fill\n\
         \x20   - lengths 1,7,8,9,15,16,17,31,32,33,63,64,65,127,128,129 under four fill\n\
         \x20     patterns (zero, 0xFF, counting, 0x55/0xAA)\n\
         \x20   - all-zero and all-0xFF buffers at 0,1,255,256,257,1023,1024,1025,4095,4096,4097\n\
         \x20   - long buffers: 65535, 65536, 65537 and 262144 bytes, four fill patterns\n\
         \x20   - NMEA sentences, with and without the leading '$', terminated by end-of-string,\n\
         \x20     '*', CR, LF and CRLF, including embedded NULs and delimiters at interior\n\
         \x20     offsets\n\
         \x20   - the NULL-pointer contract at four lengths (libcrc returns the init value from\n\
         \x20     its twelve CRC functions and NULL from checksum_NMEA)"
    );
    let _ = writeln!(w);

    // -------------------------------------------------------------- self-check
    let _ = writeln!(w, "ORACLE SELF-CHECK (run before the clock started)");
    let _ = writeln!(
        w,
        "  The oracle is asked for \"123456789\" and its answers are compared against the\n\
         \x20 values recorded from the upstream library in 00-VERIFIED-FACTS.md §11. If the\n\
         \x20 oracle were mis-built — most likely by dropping -funsigned-char, which libcrc\n\
         \x20 requires — it would be wrong itself and every 'divergence' below would be an\n\
         \x20 artefact. Result: {}",
        if golden_ok { "PASS" } else { "*** FAIL ***" }
    );
    let _ = writeln!(w);

    // ------------------------------------------------------------ divergences
    let _ = writeln!(w, "DIVERGENCES");
    if divergences.is_empty() {
        let _ = writeln!(
            w,
            "  None. {} cases x 25 value comparisons = {} comparisons, all identical.",
            stats.cases,
            stats.value_comparisons()
        );
        let _ = writeln!(
            w,
            "\n  Stated precisely, because the distinction matters: this is byte-for-byte\n\
             \x20 agreement on every input the generator produced. It is evidence, not proof.\n\
             \x20 crc_sick in particular has no external reference implementation — libcrc is\n\
             \x20 its only definition — so its correctness argument rests entirely on this\n\
             \x20 differential parity."
        );
    } else {
        for (n, d) in divergences.iter().enumerate() {
            let _ = writeln!(w, "  --- divergence {} of {} ---", n + 1, divergences.len());
            let _ = writeln!(w, "  case index   {}", d.index);
            let _ = writeln!(w, "  input class  {}", d.class.name());
            let _ = writeln!(w, "  length       {} bytes{}", d.len, if d.is_null { " (NULL pointer passed)" } else { "" });
            let _ = writeln!(w, "  bytes        {}", hexdump(&d.data, 256));
            let _ = writeln!(w, "  as text      {:?}", printable(&d.data, 128));
            for diff in &d.diffs {
                let _ = writeln!(w, "    {:<32} C={:<20} Rust={}", diff.check, diff.oracle, diff.port);
            }
            match &d.minimised {
                Some(m) => {
                    let _ = writeln!(
                        w,
                        "  MINIMISED to {} bytes{} after {} oracle calls:",
                        m.data.len(),
                        if m.is_null { " (still passing NULL)" } else { "" },
                        m.oracle_calls
                    );
                    let _ = writeln!(w, "    bytes      {}", hexdump(&m.data, 256));
                    let _ = writeln!(w, "    as text    {:?}", printable(&m.data, 128));
                    for diff in &m.diffs {
                        let _ = writeln!(w, "    {:<32} C={:<20} Rust={}", diff.check, diff.oracle, diff.port);
                    }
                }
                None => {
                    let _ = writeln!(w, "  (minimisation did not reduce this input)");
                }
            }
            let _ = writeln!(w, "  replay       ... --seed {} --case {}", cfg.seed, d.index);
            let _ = writeln!(w);
        }
    }
    let _ = writeln!(w);

    // ---------------------------------------------------------- upstream note
    let _ = writeln!(w, "UPSTREAM NOTE — a public API that cannot be called");
    let _ = writeln!(
        w,
        "  Building this harness surfaced it mechanically. include/checksum.h:99 declares\n\
         \n\
         \x20     uint64_t update_crc_64_ecma( uint64_t crc, unsigned char c );\n\
         \n\
         \x20 and no definition exists anywhere in src/. `nm lib/libcrc.a | grep update_crc_64`\n\
         \x20 reports exactly one symbol, `update_crc_64`, which the public header does NOT\n\
         \x20 declare. So the documented incremental CRC-64 entry point fails to link, and the\n\
         \x20 one that works is undocumented. fuzz/oracle_harness.c has to declare\n\
         \x20 `update_crc_64` itself to reach it — that extern is the evidence. Reproduce with\n\
         \x20 fuzz/prove_d01.sh."
    );
    let _ = writeln!(w);

    // ----------------------------------------------------------- known limits
    let _ = writeln!(w, "WHAT THIS RUN DOES *NOT* SHOW");
    let _ = writeln!(
        w,
        "  - It compares crates/libcrc-rs directly. The C-ABI shim in crates/libcrc-cabi is\n\
         \x20   covered separately, by the unmodified original test suite linking against it.\n\
         \x20   The NULL-pointer cases here do exercise the shim's NULL->empty-slice mapping\n\
         \x20   as a model, but not the shim's own pointer arithmetic.\n\
         \x20 - Inputs are generated, not coverage-guided. cargo-fuzz needs a nightly\n\
         \x20   toolchain and this machine has none, so there is no libFuzzer feedback loop\n\
         \x20   steering toward new edges. The mitigation is the fixed corpus above, which\n\
         \x20   pins the structural edge cases rather than hoping to stumble on them.\n\
         \x20 - Single platform: x86-64 windows-gnu. libcrc's lazy table initialisation is\n\
         \x20   unsynchronised, which is undefined behaviour under C11 5.1.2.4 and can go\n\
         \x20   wrong on a weakly ordered CPU; x86 is TSO and will not show it. The port has\n\
         \x20   no runtime initialisation at all, so it cannot exhibit the bug either way.\n\
         \x20 - Single-threaded. Concurrency is a separate exercise, not this one."
    );
    let _ = writeln!(w);

    // ---------------------------------------------------------------- toolchain
    let _ = writeln!(w, "ENVIRONMENT");
    let _ = writeln!(w, "  rustc                 {}", tool_version("rustc", &["--version"]));
    let _ = writeln!(w, "  cargo                 {}", tool_version("cargo", &["--version"]));
    let _ = writeln!(w, "  gcc (oracle)          {}", tool_version("gcc", &["--version"]));
    let _ = writeln!(w, "  host                  {}", std::env::consts::OS);
    let _ = writeln!(w, "  arch                  {}", std::env::consts::ARCH);
    let _ = writeln!(w);
    let _ = writeln!(w, "  Oracle build:  mingw32-make OS=posix CC=gcc EXEEXT=.exe");
    let _ = writeln!(w, "  Oracle CFLAGS: -O3 -funsigned-char (libcrc's own; -funsigned-char is");
    let _ = writeln!(w, "                 mandatory — gcc on x86 defaults to signed char, which");
    let _ = writeln!(w, "                 would make the oracle itself wrong)");
    let _ = writeln!(w, "  Harness build: gcc -Wall -Wextra -Werror -O2 -funsigned-char");
    let _ = writeln!(w);
    let _ = writeln!(w, "=========================================================================");

    s
}

pub fn write_log(path: &Path, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)
}
