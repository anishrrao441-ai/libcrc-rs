//! `bench_rs` — the Rust-side benchmark driver for the libcrc port.
//!
//! A MEASUREMENT INSTRUMENT, not part of the port. It is its own workspace root
//! (see Cargo.toml), so `cargo build --release` at the repository root does not
//! build it and the shipped library has no idea it exists.
//!
//! This file mirrors `../../c/bench_c.c` statement for statement: the same
//! workload table, the same calibration rule, the same warm-up count, the same
//! anti-dead-code sink, the same output format. If you change one, change the
//! other. That symmetry is the whole point — it is what lets the two sets of
//! numbers be compared at all.
//!
//! Configuration measured here: `rust-native` — direct calls into `libcrc-rs`
//! with cross-crate LTO, i.e. what a Rust consumer of the port actually gets.
//! The separately-built `rust-cabi` configuration runs the *C* driver against
//! the port's staticlib through its C ABI, which controls for driver codegen.
//!
//! Output format (stdout):
//!   `#M <key> <value>`                   metadata
//!   `#W <kind> <algo> <bytes> <k> <n>`   workload header
//!   `<ns>,<ns>,...`                      n raw per-SAMPLE nanosecond timings
//! Each sample times a batch of `k` calls; per-call ns = sample_ns / k.

use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use libcrc_rs as crc;

const BUF_MAX: usize = 104_857_600; // 100 MiB
const MS_SLOTS: usize = 4096; // 4096 * 64 B = a 256 KiB working set
const NMEA_MAX: usize = 1025;

/// Same rule as the C driver: grow the batch until it takes at least this long,
/// so clock quantisation contributes ~0.05% at a ~100 ns tick.
const TARGET_BATCH_NS: f64 = 200_000.0;
const MAX_K: u64 = 4_194_304;

const HEX: &[u8; 16] = b"0123456789ABCDEF";

type Kernel = fn(&mut [u8], usize, u64) -> u64;

// ---------------------------------------------------------------------------
// Batch kernels. One dedicated function per algorithm so the call inside the
// hot loop is a DIRECT call, exactly as in the C driver. Dispatch through the
// `Kernel` function pointer happens once per sample, outside the timed loop.
// ---------------------------------------------------------------------------

macro_rules! oneshot {
    ($name:ident, $f:path) => {
        fn $name(buf: &mut [u8], n: usize, k: u64) -> u64 {
            let mut acc: u64 = 0;
            for i in 0..k {
                buf[0] = b'0' + (i % 10) as u8;
                acc ^= $f(&buf[..n]) as u64;
            }
            acc
        }
    };
}

oneshot!(os_crc_8, crc::crc_8);
oneshot!(os_crc_16, crc::crc_16);
oneshot!(os_crc_modbus, crc::crc_modbus);
oneshot!(os_crc_32, crc::crc_32);
oneshot!(os_crc_64_ecma, crc::crc_64_ecma);
oneshot!(os_crc_64_we, crc::crc_64_we);
oneshot!(os_crc_ccitt_1d0f, crc::crc_ccitt_1d0f);
oneshot!(os_crc_ccitt_ffff, crc::crc_ccitt_ffff);
oneshot!(os_crc_xmodem, crc::crc_xmodem);
oneshot!(os_crc_kermit, crc::crc_kermit);
oneshot!(os_crc_dnp, crc::crc_dnp);
oneshot!(os_crc_sick, crc::crc_sick);

/// `checksum_NMEA` is delimiter-driven, not length-driven, so the workload size
/// is expressed by moving the NUL — saved and restored around the batch.
///
/// The two hex digits are formatted here as well, because the C function does
/// (`snprintf(result, 3, "%02hhX", ...)`, src/nmea-chk.c). The port keeps the
/// formatting in its C ABI shim rather than in the library, so it is reproduced
/// in the driver — otherwise this workload would compare a checksum against a
/// checksum *plus* a printf, which would be a rigged comparison.
fn os_nmea(buf: &mut [u8], n: usize, k: u64) -> u64 {
    let mut acc: u64 = 0;
    let mut out = [0u8; 8];
    let saved = buf[n];
    buf[n] = 0;
    for i in 0..k {
        buf[0] = b'0' + (i % 10) as u8;
        let c = crc::checksum_nmea(&buf[..]);
        out[0] = HEX[(c >> 4) as usize];
        out[1] = HEX[(c & 0x0F) as usize];
        out[2] = 0;
        acc ^= out[0] as u64;
    }
    buf[n] = saved;
    acc
}

/// Incremental / streaming. libcrc's only resumable API is one byte at a time,
/// so the port mirrors it and so does this workload.
macro_rules! bytewise {
    ($name:ident, $ty:ty, $start:expr, $f:path) => {
        fn $name(buf: &mut [u8], n: usize, k: u64) -> u64 {
            let mut acc: u64 = 0;
            for _ in 0..k {
                let mut c: $ty = $start;
                for i in 0..n {
                    c = $f(c, buf[i]);
                }
                acc ^= c as u64;
            }
            acc
        }
    };
}

bytewise!(bw_crc_8, u8, 0x00, crc::update_crc_8);
bytewise!(bw_crc_16, u16, 0x0000, crc::update_crc_16);
bytewise!(bw_crc_32, u32, 0xFFFF_FFFF, crc::update_crc_32);
bytewise!(bw_crc_ccitt, u16, 0xFFFF, crc::update_crc_ccitt);
bytewise!(bw_crc_kermit, u16, 0x0000, crc::update_crc_kermit);
bytewise!(bw_crc_dnp, u16, 0x0000, crc::update_crc_dnp);
bytewise!(bw_crc_64, u64, 0xFFFF_FFFF_FFFF_FFFF, crc::update_crc_64);

fn bw_crc_sick(buf: &mut [u8], n: usize, k: u64) -> u64 {
    let mut acc: u64 = 0;
    for _ in 0..k {
        let mut c: u16 = 0x0000;
        let mut prev: u8 = 0;
        for i in 0..n {
            c = crc::update_crc_sick(c, buf[i], prev);
            prev = buf[i];
        }
        acc ^= c as u64;
    }
    acc
}

/// Many small calls: `n` independent 64-byte one-shot calls cycling over a
/// 256 KiB region. The table stays hot, the data does not fit in L1, and
/// per-call overhead dominates.
macro_rules! manysmall {
    ($name:ident, $f:path) => {
        fn $name(buf: &mut [u8], n: usize, k: u64) -> u64 {
            let mut acc: u64 = 0;
            for _ in 0..k {
                for i in 0..n {
                    let off = (i & (MS_SLOTS - 1)) * 64;
                    acc ^= $f(&buf[off..off + 64]) as u64;
                }
            }
            acc
        }
    };
}

manysmall!(ms_crc_8, crc::crc_8);
manysmall!(ms_crc_16, crc::crc_16);
manysmall!(ms_crc_32, crc::crc_32);
manysmall!(ms_crc_ccitt_ffff, crc::crc_ccitt_ffff);

// ---------------------------------------------------------------------------
// Workload table — MUST stay identical to the C twin.
// ---------------------------------------------------------------------------

struct Workload {
    kind: &'static str,
    algo: &'static str,
    bytes: usize,
    samples: u32,
    nmea: bool,
    f: Kernel,
}

const SMALL_SIZES: [usize; 4] = [16, 64, 256, 1024];
const LARGE_SIZES: [usize; 3] = [1_048_576, 16_777_216, 104_857_600];
const LARGE_SAMPLES: [u32; 3] = [200, 60, 25];

const ONESHOT_ALL: [(&str, Kernel); 12] = [
    ("crc_8", os_crc_8),
    ("crc_16", os_crc_16),
    ("crc_modbus", os_crc_modbus),
    ("crc_32", os_crc_32),
    ("crc_64_ecma", os_crc_64_ecma),
    ("crc_64_we", os_crc_64_we),
    ("crc_ccitt_1d0f", os_crc_ccitt_1d0f),
    ("crc_ccitt_ffff", os_crc_ccitt_ffff),
    ("crc_xmodem", os_crc_xmodem),
    ("crc_kermit", os_crc_kermit),
    ("crc_dnp", os_crc_dnp),
    ("crc_sick", os_crc_sick),
];

const ONESHOT_LARGE: [(&str, Kernel); 6] = [
    ("crc_8", os_crc_8),
    ("crc_16", os_crc_16),
    ("crc_32", os_crc_32),
    ("crc_64_we", os_crc_64_we),
    ("crc_ccitt_ffff", os_crc_ccitt_ffff),
    ("crc_sick", os_crc_sick),
];

const BYTEWISE_ALL: [(&str, Kernel); 8] = [
    ("crc_8", bw_crc_8),
    ("crc_16", bw_crc_16),
    ("crc_32", bw_crc_32),
    ("crc_ccitt", bw_crc_ccitt),
    ("crc_kermit", bw_crc_kermit),
    ("crc_dnp", bw_crc_dnp),
    ("crc_64", bw_crc_64),
    ("crc_sick", bw_crc_sick),
];

const MANYSMALL_ALL: [(&str, Kernel); 4] = [
    ("crc_8", ms_crc_8),
    ("crc_16", ms_crc_16),
    ("crc_32", ms_crc_32),
    ("crc_ccitt_ffff", ms_crc_ccitt_ffff),
];

fn build_workloads() -> Vec<Workload> {
    let mut v = Vec::new();
    let mut push = |kind, algo, bytes, samples, nmea, f| {
        v.push(Workload { kind, algo, bytes, samples, nmea, f })
    };
    for &size in SMALL_SIZES.iter() {
        for &(algo, f) in ONESHOT_ALL.iter() {
            push("oneshot", algo, size, 500, false, f);
        }
        push("oneshot", "checksum_NMEA", size, 500, true, os_nmea as Kernel);
    }
    for (i, &size) in LARGE_SIZES.iter().enumerate() {
        for &(algo, f) in ONESHOT_LARGE.iter() {
            push("oneshot", algo, size, LARGE_SAMPLES[i], false, f);
        }
    }
    for &(algo, f) in BYTEWISE_ALL.iter() {
        push("bytewise", algo, 65536, 300, false, f);
    }
    for &(algo, f) in MANYSMALL_ALL.iter() {
        push("manysmall", algo, 100_000, 100, false, f);
    }
    v
}

// ---------------------------------------------------------------------------
// Calibration + measurement
// ---------------------------------------------------------------------------

fn time_batch(f: Kernel, buf: &mut [u8], n: usize, k: u64) -> f64 {
    let t0 = Instant::now();
    let acc = f(buf, n, k);
    let dt = t0.elapsed();
    black_box(acc);
    dt.as_nanos() as f64
}

fn calibrate(f: Kernel, buf: &mut [u8], n: usize) -> u64 {
    let mut k: u64 = 1;
    loop {
        let ns = time_batch(f, buf, n, k);
        if ns >= TARGET_BATCH_NS || k >= MAX_K {
            return k;
        }
        k *= 2;
    }
}

fn xorshift64(s: &mut u64) -> u64 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *s = x;
    x
}

fn run_all(label: &str) {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    // Same seed and same mapping as the C driver, so both sides see the same
    // bytes. CRC timing is data-independent, but identical input removes the
    // question entirely.
    let mut s: u64 = 0x2026_0701_1800_0000;
    let mut buf = vec![0u8; BUF_MAX];
    for b in buf.iter_mut() {
        let v = (0x21 + (xorshift64(&mut s) % 93)) as u8;
        *b = if v == b'*' { b'+' } else { v };
    }
    let mut nmea = vec![0u8; NMEA_MAX];
    nmea[..NMEA_MAX - 1].copy_from_slice(&buf[..NMEA_MAX - 1]);
    nmea[NMEA_MAX - 1] = 0;

    writeln!(out, "#M impl {}", label).unwrap();
    writeln!(out, "#M clock std::time::Instant (QueryPerformanceCounter on Windows)").unwrap();
    writeln!(out, "#M target_batch_ns {:.0}", TARGET_BATCH_NS).unwrap();

    for wl in build_workloads() {
        let b: &mut [u8] = if wl.nmea { &mut nmea } else { &mut buf };
        let k = calibrate(wl.f, b, wl.bytes);
        // Three discarded warm-up batches, matching the C driver.
        time_batch(wl.f, b, wl.bytes, k);
        time_batch(wl.f, b, wl.bytes, k);
        time_batch(wl.f, b, wl.bytes, k);

        writeln!(out, "#W {} {} {} {} {}", wl.kind, wl.algo, wl.bytes, k, wl.samples).unwrap();
        for i in 0..wl.samples {
            let ns = time_batch(wl.f, b, wl.bytes, k);
            if i > 0 {
                write!(out, ",").unwrap();
            }
            write!(out, "{:.0}", ns).unwrap();
        }
        writeln!(out).unwrap();
        out.flush().unwrap();
    }
}

// ---------------------------------------------------------------------------
// Auxiliary modes
// ---------------------------------------------------------------------------

fn clockres() {
    let mut min: u128 = u128::MAX;
    for _ in 0..2_000_000 {
        let a = Instant::now();
        let b = Instant::now();
        let d = b.duration_since(a).as_nanos();
        if d > 0 && d < min {
            min = d;
        }
    }
    println!("#M min_nonzero_delta_ns {}", min);
    println!("#M implied_hz {:.0}", 1e9 / min as f64);
}

fn firstcall(algo: &str) {
    let mut small = [0u8; 64];
    for (i, b) in small.iter_mut().enumerate() {
        *b = b'0' + (i % 10) as u8;
    }
    let t0 = Instant::now();
    let r: u64 = match algo {
        "crc_16" => crc::crc_16(&small) as u64,
        "crc_32" => crc::crc_32(&small) as u64,
        "crc_ccitt_ffff" => crc::crc_ccitt_ffff(&small) as u64,
        "crc_kermit" => crc::crc_kermit(&small) as u64,
        "crc_dnp" => crc::crc_dnp(&small) as u64,
        "crc_8" => crc::crc_8(&small) as u64,
        "crc_64_we" => crc::crc_64_we(&small),
        "crc_sick" => crc::crc_sick(&small) as u64,
        _ => {
            eprintln!("unknown algo {}", algo);
            std::process::exit(2);
        }
    };
    let dt = t0.elapsed();
    black_box(r);
    println!("{}", dt.as_nanos());
}

fn rss_profile(profile: &str) {
    let mut sink: u64 = 0;
    match profile {
        "minimal" => {
            let mut small = [0u8; 1024];
            for (i, b) in small.iter_mut().enumerate() {
                *b = (i & 0x7f) as u8;
            }
            sink ^= crc::crc_16(&small) as u64;
            sink ^= crc::crc_32(&small) as u64;
        }
        "work1m" | "work100m" => {
            let n = if profile == "work1m" { 1_048_576 } else { 104_857_600 };
            let mut b = vec![0u8; n];
            for (i, x) in b.iter_mut().enumerate() {
                *x = (i & 0x7f) as u8;
            }
            sink ^= crc::crc_8(&b) as u64;
            sink ^= crc::crc_16(&b) as u64;
            sink ^= crc::crc_32(&b) as u64;
            sink ^= crc::crc_64_we(&b);
            sink ^= crc::crc_ccitt_ffff(&b) as u64;
            sink ^= crc::crc_sick(&b) as u64;
        }
        _ => {
            eprintln!("unknown rss profile {}", profile);
            std::process::exit(2);
        }
    }
    println!("{}", black_box(sink));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("noop") => {}
        Some("clockres") => clockres(),
        Some("firstcall") => firstcall(args.get(2).map(String::as_str).unwrap_or("")),
        Some("rss") => rss_profile(args.get(2).map(String::as_str).unwrap_or("")),
        Some("run") => run_all(args.get(2).map(String::as_str).unwrap_or("rust-native")),
        _ => {
            eprintln!(
                "usage: {} run <label> | noop | clockres | firstcall <algo> | rss <profile>",
                args.first().map(String::as_str).unwrap_or("bench_rs")
            );
            std::process::exit(1);
        }
    }
}
