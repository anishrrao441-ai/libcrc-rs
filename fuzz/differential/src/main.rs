//! Differential fuzzer: the original C libcrc against the Rust port.
//!
//! ```text
//! difffuzz --seed <u64> [--seconds <f64> | --cases <u64>] [--batch <n>]
//!          [--start <index>] [--case <index>] --oracle <exe> --workdir <dir> --log <file>
//! ```
//!
//! Both sides are handed identical bytes and asked for all 13 exported entry points plus
//! the same twelve values rebuilt through the 8 `update_crc_*` functions — 25 value
//! comparisons per case. Any disagreement is reported by algorithm name, minimised, and
//! written to the log. Nothing is swept up.
//!
//! The oracle is invoked one batch per process, never streamed. See `oracle.rs` for why
//! that is a hard requirement rather than a style choice.

mod cases;
mod model;
mod oracle;
mod report;
mod rng;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cases::Case;
use model::Diff;
use oracle::Oracle;
use report::{Divergence, Minimised, RunConfig, Stats};

/// Keep at most this many divergences in memory; the counter keeps counting past it.
const MAX_RECORDED_DIVERGENCES: usize = 20;

/// Ceiling on oracle round-trips spent shrinking one input.
const MAX_SHRINK_CALLS: u64 = 600;

struct Args {
    seed: u64,
    seconds: f64,
    cases: Option<u64>,
    batch: usize,
    start: u64,
    single_case: Option<u64>,
    oracle_exe: PathBuf,
    workdir: PathBuf,
    log: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            seed: 0,
            seconds: 60.0,
            cases: None,
            batch: 50_000,
            start: 0,
            single_case: None,
            oracle_exe: PathBuf::from("fuzz/build/oracle_harness.exe"),
            workdir: PathBuf::from("fuzz/build/work"),
            log: PathBuf::from("fuzz/log.txt"),
        }
    }
}

const USAGE: &str = "\
difffuzz — differential fuzzer, original C libcrc vs. the Rust port

  --seed <u64>       master seed. 0 or omitted derives one from the clock and prints it.
  --seconds <f64>    run for this long (default 60). The bonus needs >= 60 continuous.
  --cases <u64>      run exactly this many cases instead; this is how a run is replayed.
  --batch <n>        cases per oracle invocation (default 50000). Does not affect inputs.
  --start <index>    first case index (default 0).
  --case <index>     run one case, print both sides in full, exit. For investigation.
  --oracle <path>    the C oracle harness binary.
  --workdir <dir>    scratch directory for cases.bin / results.bin.
  --log <path>       where to write the run log (default fuzz/log.txt).
";

fn next_value<I: Iterator<Item = String>>(it: &mut I, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"))
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);

    while let Some(flag) = it.next() {
        let f = flag.as_str();
        match f {
            "--seed" => args.seed = parse_seed(&next_value(&mut it, f)?)?,
            "--seconds" => {
                args.seconds = next_value(&mut it, f)?.parse().map_err(|e| format!("--seconds: {e}"))?
            }
            "--cases" => {
                args.cases = Some(next_value(&mut it, f)?.parse().map_err(|e| format!("--cases: {e}"))?)
            }
            "--batch" => {
                args.batch = next_value(&mut it, f)?.parse().map_err(|e| format!("--batch: {e}"))?
            }
            "--start" => {
                args.start = next_value(&mut it, f)?.parse().map_err(|e| format!("--start: {e}"))?
            }
            "--case" => {
                args.single_case =
                    Some(next_value(&mut it, f)?.parse().map_err(|e| format!("--case: {e}"))?)
            }
            "--oracle" => args.oracle_exe = PathBuf::from(next_value(&mut it, f)?),
            "--workdir" => args.workdir = PathBuf::from(next_value(&mut it, f)?),
            "--log" => args.log = PathBuf::from(next_value(&mut it, f)?),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }

    if args.batch == 0 {
        return Err("--batch must be at least 1".into());
    }
    if args.seconds < 0.0 {
        return Err("--seconds must not be negative".into());
    }
    Ok(args)
}

/// Accept both `12345` and `0xDEADBEEF` so a seed copied out of a log round-trips.
fn parse_seed(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let parsed = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        t.parse()
    };
    parsed.map_err(|e| format!("--seed {t}: {e}"))
}

fn seed_from_clock() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED_5EED_5EED_5EED);
    rng::splitmix64(nanos ^ std::process::id() as u64)
}

// ===========================================================================
// Comparison
// ===========================================================================

/// Compare one case. Returns the named disagreements, empty when the two sides agree.
fn compare(
    port_data: &[u8],
    is_null: bool,
    oracle_oneshot: &model::Block,
    oracle_incremental: &model::Block,
) -> Vec<Diff> {
    let port_oneshot = model::port_oneshot(port_data, is_null);
    let port_incremental = model::port_incremental(port_data);

    // Fast path: whole-block equality first, so the common case never allocates.
    if *oracle_oneshot == port_oneshot && *oracle_incremental == port_incremental {
        return Vec::new();
    }

    let mut diffs = Vec::new();
    model::diff_oneshot(oracle_oneshot, &port_oneshot, &mut diffs);
    model::diff_incremental(oracle_incremental, &port_incremental, &mut diffs);
    diffs
}

/// Ask the oracle about one literal input and report whether it disagrees with the port.
fn probe(oracle: &Oracle, data: &[u8], is_null: bool, calls: &mut u64) -> Option<Vec<Diff>> {
    *calls += 1;
    let batch = cases::build_literal(data, is_null);
    let results = oracle.run(&batch).ok()?;
    let port_view: &[u8] = if is_null { &[] } else { data };
    let diffs = compare(port_view, is_null, &results.oneshot(0), &results.incremental(0));
    if diffs.is_empty() {
        None
    } else {
        Some(diffs)
    }
}

/// Shrink a diverging input toward the smallest one that still diverges.
///
/// Delta-debugging proper would be better, but a diverging CRC input is expected to be
/// structural rather than positional, so halving-then-trimming from both ends, followed
/// by byte zeroing, finds the essence of it cheaply. Every step is re-confirmed against
/// the oracle — the shrinker never assumes a smaller input still fails.
fn shrink(oracle: &Oracle, data: &[u8], is_null: bool) -> Option<Minimised> {
    let mut calls = 0u64;
    let mut best = data.to_vec();
    let mut best_diffs = probe(oracle, &best, is_null, &mut calls)?;

    // Halve from the tail for as long as it keeps failing.
    let mut step = best.len() / 2;
    while step > 0 && calls < MAX_SHRINK_CALLS {
        let candidate = best[..best.len() - step.min(best.len())].to_vec();
        match probe(oracle, &candidate, is_null, &mut calls) {
            Some(d) => {
                best = candidate;
                best_diffs = d;
            }
            None => step /= 2,
        }
        step = step.min(best.len());
    }

    // Trim single bytes from the front.
    while !best.is_empty() && calls < MAX_SHRINK_CALLS {
        let candidate = best[1..].to_vec();
        match probe(oracle, &candidate, is_null, &mut calls) {
            Some(d) => {
                best = candidate;
                best_diffs = d;
            }
            None => break,
        }
    }

    // Flatten bytes to zero where that preserves the failure — it makes the surviving
    // bytes obviously the load-bearing ones.
    for i in 0..best.len() {
        if calls >= MAX_SHRINK_CALLS {
            break;
        }
        if best[i] == 0 {
            continue;
        }
        let mut candidate = best.clone();
        candidate[i] = 0;
        if let Some(d) = probe(oracle, &candidate, is_null, &mut calls) {
            best = candidate;
            best_diffs = d;
        }
    }

    Some(Minimised { data: best, is_null, diffs: best_diffs, oracle_calls: calls })
}

// ===========================================================================
// Pre-flight
// ===========================================================================

/// Confirm the oracle itself is sane before spending a minute trusting it.
///
/// The realistic failure is a C build without `-funsigned-char`: libcrc forces `char`
/// unsigned and gcc on x86 defaults to signed, so a careless oracle build is wrong in a
/// way that would manufacture divergences all night. Checking it against the nine values
/// recorded from the upstream library costs one process spawn.
fn oracle_self_check(oracle: &Oracle) -> Result<bool, String> {
    const CHECK: &[u8] = b"123456789";
    let batch = cases::build_literal(CHECK, false);
    let results = oracle.run(&batch).map_err(|e| format!("oracle self-check failed to run: {e}"))?;
    let got = results.oneshot(0);

    let expected: [(&str, u64, u64); 9] = [
        ("crc_16", got.crc_16 as u64, 0xBB3D),
        ("crc_modbus", got.crc_modbus as u64, 0x4B37),
        ("crc_sick", got.crc_sick as u64, 0x56A6),
        ("crc_xmodem", got.crc_xmodem as u64, 0x31C3),
        ("crc_ccitt_ffff", got.crc_ccitt_ffff as u64, 0x29B1),
        ("crc_ccitt_1d0f", got.crc_ccitt_1d0f as u64, 0xE5CC),
        ("crc_kermit", got.crc_kermit as u64, 0x8921),
        ("crc_dnp", got.crc_dnp as u64, 0x82EA),
        ("crc_32", got.crc_32 as u64, 0xCBF4_3926),
    ];

    let mut ok = true;
    for (name, actual, want) in expected {
        if actual != want {
            eprintln!("  oracle self-check MISMATCH {name}: got 0x{actual:X}, expected 0x{want:X}");
            ok = false;
        }
    }
    if !ok {
        eprintln!(
            "  The C oracle does not reproduce the values recorded from upstream libcrc.\n\
             \x20 Most likely it was built without -funsigned-char. Rebuild with:\n\
             \x20   mingw32-make OS=posix CC=gcc EXEEXT=.exe"
        );
    }
    Ok(ok)
}

/// Print both sides of a single case in full. Investigation aid, not part of a run.
fn dump_case(oracle: &Oracle, fixed: &[Case], seed: u64, index: u64) -> Result<(), String> {
    let batch = cases::build_batch(fixed, seed, index, 1);
    let span = &batch.spans[0];
    let payload = batch.payload(span).to_vec();

    let results = oracle.run(&batch).map_err(|e| e.to_string())?;
    let o1 = results.oneshot(0);
    let o2 = results.incremental(0);
    let p1 = model::port_oneshot(&payload, span.is_null);

    println!("case {index}  seed 0x{seed:016X}");
    println!("  class   {}", span.class.name());
    println!("  length  {} bytes{}", span.len, if span.is_null { " (NULL pointer passed)" } else { "" });
    println!("  first bytes {:02X?}", &payload[..payload.len().min(32)]);
    println!();
    println!("  {:<32} {:<22} {}", "check", "C oracle", "Rust port");
    println!("  {:<32} {:<22} {}", "-".repeat(32), "-".repeat(22), "-".repeat(22));
    println!("  {:<32} 0x{:02X}{:<18} 0x{:02X}", "crc_8", o1.crc_8, "", p1.crc_8);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_16", o1.crc_16, "", p1.crc_16);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_modbus", o1.crc_modbus, "", p1.crc_modbus);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_ccitt_1d0f", o1.crc_ccitt_1d0f, "", p1.crc_ccitt_1d0f);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_ccitt_ffff", o1.crc_ccitt_ffff, "", p1.crc_ccitt_ffff);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_xmodem", o1.crc_xmodem, "", p1.crc_xmodem);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_kermit", o1.crc_kermit, "", p1.crc_kermit);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_dnp", o1.crc_dnp, "", p1.crc_dnp);
    println!("  {:<32} 0x{:04X}{:<16} 0x{:04X}", "crc_sick", o1.crc_sick, "", p1.crc_sick);
    println!("  {:<32} 0x{:08X}{:<12} 0x{:08X}", "crc_32", o1.crc_32, "", p1.crc_32);
    println!("  {:<32} 0x{:016X}{:<4} 0x{:016X}", "crc_64_ecma", o1.crc_64_ecma, "", p1.crc_64_ecma);
    println!("  {:<32} 0x{:016X}{:<4} 0x{:016X}", "crc_64_we", o1.crc_64_we, "", p1.crc_64_we);
    println!(
        "  {:<32} {:<22} {}",
        "checksum_NMEA",
        if o1.nmea_ok { format!("\"{}\"", String::from_utf8_lossy(&o1.nmea_hex)) } else { "NULL".into() },
        if p1.nmea_ok { format!("\"{}\"", String::from_utf8_lossy(&p1.nmea_hex)) } else { "NULL".into() },
    );

    let diffs = compare(&payload, span.is_null, &o1, &o2);
    println!();
    if diffs.is_empty() {
        println!("  all 25 comparisons agree");
    } else {
        println!("  {} DIVERGENCE(S):", diffs.len());
        for d in &diffs {
            println!("    {:<32} C={:<20} Rust={}", d.check, d.oracle, d.port);
        }
    }
    Ok(())
}

// ===========================================================================
// Driver
// ===========================================================================

fn run() -> Result<i32, String> {
    let mut args = parse_args()?;
    if args.seed == 0 {
        args.seed = seed_from_clock();
    }

    let oracle = Oracle::new(&args.oracle_exe, &args.workdir).map_err(|e| e.to_string())?;
    let fixed = cases::prologue();

    if let Some(index) = args.single_case {
        dump_case(&oracle, &fixed, args.seed, index)?;
        return Ok(0);
    }

    println!("libcrc differential fuzzer");
    println!("  seed        0x{:016X}  ({})", args.seed, args.seed);
    println!("  oracle      {}", args.oracle_exe.display());
    println!("  fixed corpus {} cases (indices 0..{})", fixed.len(), fixed.len() - 1);
    match args.cases {
        Some(n) => println!("  target      {n} cases (replay mode)"),
        None => println!("  target      {:.0} s continuous", args.seconds),
    }
    println!("  batch       {} cases per oracle invocation", args.batch);
    println!();

    print!("pre-flight: oracle self-check against the recorded upstream values ... ");
    let golden_ok = oracle_self_check(&oracle)?;
    println!("{}", if golden_ok { "PASS" } else { "FAIL" });
    if !golden_ok {
        return Err("refusing to fuzz against an oracle that fails its own golden vectors".into());
    }
    println!();

    let mut stats = Stats::default();
    let mut divergences: Vec<Divergence> = Vec::new();
    let deadline = Duration::from_secs_f64(args.seconds);
    let mut next_index = args.start;

    // The clock starts here: setup and the self-check are outside the measured window,
    // and the loop below runs without pausing until it stops.
    let started = Instant::now();

    loop {
        let remaining_cases = args.cases.map(|target| target.saturating_sub(stats.cases));
        match remaining_cases {
            Some(0) => break,
            None if started.elapsed() >= deadline => break,
            _ => {}
        }

        let this_batch = match remaining_cases {
            Some(r) => (r as usize).min(args.batch),
            None => args.batch,
        };

        let batch = cases::build_batch(&fixed, args.seed, next_index, this_batch);
        let results = oracle.run(&batch).map_err(|e| e.to_string())?;

        for (i, span) in batch.spans.iter().enumerate() {
            let payload = batch.payload(span);
            stats.record_class(span.class);
            stats.payload_bytes += payload.len() as u64;
            stats.mix(span.index, span.is_null, span.len, payload);

            let diffs = compare(payload, span.is_null, &results.oneshot(i), &results.incremental(i));
            if diffs.is_empty() {
                continue;
            }

            stats.divergences += 1;
            eprintln!(
                "\n*** DIVERGENCE at case {} (class {}, {} bytes) ***",
                span.index,
                span.class.name(),
                span.len
            );
            for d in &diffs {
                eprintln!("      {:<32} C={:<20} Rust={}", d.check, d.oracle, d.port);
            }

            if divergences.len() < MAX_RECORDED_DIVERGENCES {
                let data = payload.to_vec();
                eprintln!("      minimising ...");
                let minimised = shrink(&oracle, &data, span.is_null);
                if let Some(m) = &minimised {
                    eprintln!("      minimised to {} bytes in {} oracle calls", m.data.len(), m.oracle_calls);
                }
                divergences.push(Divergence {
                    index: span.index,
                    class: span.class,
                    len: span.len,
                    is_null: span.is_null,
                    data,
                    diffs,
                    minimised,
                });
            }
        }

        stats.cases += batch.spans.len() as u64;
        stats.batches += 1;
        next_index += batch.spans.len() as u64;

        let secs = started.elapsed().as_secs_f64();
        println!(
            "  [{:6.1}s] {:>12} cases · {:>7.0} cases/s · {:>8.1} MiB · {} divergence(s)",
            secs,
            stats.cases,
            stats.cases as f64 / secs.max(1e-9),
            stats.payload_bytes as f64 / (1024.0 * 1024.0),
            stats.divergences
        );
    }

    let elapsed = started.elapsed();

    let cfg = RunConfig {
        seed: args.seed,
        start_index: args.start,
        batch_size: args.batch,
        requested_seconds: args.seconds,
        requested_cases: args.cases,
        oracle_exe: args.oracle_exe.display().to_string(),
    };
    let body = report::render(&cfg, &stats, elapsed, &divergences, golden_ok);
    report::write_log(Path::new(&args.log), &body).map_err(|e| format!("writing log: {e}"))?;

    println!();
    println!("---------------------------------------------------------------");
    println!("  seed          0x{:016X}  ({})", args.seed, args.seed);
    println!("  duration      {:.3} s", elapsed.as_secs_f64());
    println!("  cases         {}", stats.cases);
    println!("  comparisons   {}", stats.value_comparisons());
    println!("  stream digest 0x{:016X}", stats.stream_digest);
    println!("  divergences   {}", stats.divergences);
    println!("  log           {}", args.log.display());
    println!("---------------------------------------------------------------");

    Ok(if stats.divergences == 0 { 0 } else { 1 })
}

fn main() -> ExitCode {
    match run() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code as u8),
        Err(msg) => {
            eprintln!("difffuzz: {msg}");
            ExitCode::from(2)
        }
    }
}
