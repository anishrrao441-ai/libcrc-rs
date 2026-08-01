#!/usr/bin/env node
/*
 * bench/analyze.mjs — turns the raw sample dumps in bench/raw/ into
 * bench/results.json, plus a human-readable summary on stdout.
 *
 * ONE analyser processes every configuration. That is deliberate: if the C
 * driver and the Rust driver each computed their own percentiles, a difference
 * in percentile convention would be indistinguishable from a difference in
 * performance. Here the drivers only ever emit raw nanosecond samples and all
 * statistics are computed once, in this file, by the same code.
 *
 * Percentile convention: nearest-rank on the sorted sample vector,
 * index = ceil(p/100 * N) - 1, clamped. No interpolation. Stated so the numbers
 * can be reproduced from the raw data in bench/raw/.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const BENCH = path.dirname(fileURLToPath(import.meta.url));
const RAW = path.join(BENCH, "raw");
const read = (f) => fs.readFileSync(path.join(RAW, f), "utf8");
const exists = (f) => fs.existsSync(path.join(RAW, f));

// --- statistics -----------------------------------------------------------

function pct(sorted, p) {
  if (sorted.length === 0) return null;
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[i];
}

function stats(values) {
  const s = [...values].sort((a, b) => a - b);
  const n = s.length;
  const mean = s.reduce((a, b) => a + b, 0) / n;
  const variance = n > 1 ? s.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1) : 0;
  const sd = Math.sqrt(variance);
  return {
    n,
    min: s[0],
    p50: pct(s, 50),
    p90: pct(s, 90),
    p99: pct(s, 99),
    max: s[n - 1],
    mean,
    stddev: sd,
    cv_pct: mean > 0 ? (sd / mean) * 100 : 0,
  };
}

const round = (x, d = 3) =>
  x === null || x === undefined ? null : Number.parseFloat(Number(x).toFixed(d));

// --- parse the sample dumps ----------------------------------------------

/** Returns { meta, workloads: Map<key, {kind, algo, bytes, k, perCall: number[]}> } */
function parseSamples(text) {
  const meta = {};
  const workloads = new Map();
  const lines = text.split(/\r?\n/);
  let cur = null;
  for (const line of lines) {
    if (!line) continue;
    if (line.startsWith("#M ")) {
      const sp = line.indexOf(" ", 3);
      meta[line.slice(3, sp)] = line.slice(sp + 1);
    } else if (line.startsWith("#W ")) {
      const [, kind, algo, bytes, k, n] = line.split(/\s+/);
      cur = {
        kind,
        algo,
        bytes: Number(bytes),
        k: Number(k),
        declared_samples: Number(n),
        batchNs: [],
      };
      workloads.set(`${kind}|${algo}|${bytes}`, cur);
    } else if (cur) {
      cur.batchNs = line.split(",").map(Number);
      cur = null;
    }
  }
  return { meta, workloads };
}

const CONFIGS = ["c-shipped", "c-lto", "rust-cabi", "rust-native"];
const parsed = {};
for (const c of CONFIGS) {
  const f = `samples_${c}.txt`;
  if (exists(f)) parsed[c] = parseSamples(read(f));
}

// --- build the per-workload result table ---------------------------------

const keys = new Set();
for (const c of Object.keys(parsed)) for (const k of parsed[c].workloads.keys()) keys.add(k);

const workloadResults = [];
for (const key of [...keys].sort()) {
  const [kind, algo, bytesStr] = key.split("|");
  const bytes = Number(bytesStr);
  const row = { kind, algo, bytes, per_config: {} };
  for (const c of CONFIGS) {
    const w = parsed[c]?.workloads.get(key);
    if (!w) continue;
    // per-call nanoseconds = batch nanoseconds / calls in the batch
    const perCall = w.batchNs.map((ns) => ns / w.k);
    const st = stats(perCall);
    const entry = {
      batch_calls: w.k,
      samples: st.n,
      ns_per_call: {
        min: round(st.min),
        p50: round(st.p50),
        p90: round(st.p90),
        p99: round(st.p99),
        max: round(st.max),
        mean: round(st.mean),
        stddev: round(st.stddev),
        cv_pct: round(st.cv_pct, 2),
      },
    };
    // Throughput only makes sense where a call processes `bytes` bytes.
    if (kind === "oneshot" || kind === "bytewise") {
      entry.mib_per_s_at_p50 = round((bytes / (st.p50 / 1e9)) / 1048576, 1);
      entry.ns_per_byte_at_p50 = round(st.p50 / bytes, 4);
    }
    if (kind === "manysmall") {
      // `bytes` is the number of 64-byte calls in one batch iteration.
      entry.ns_per_inner_call_at_p50 = round(st.p50 / bytes, 3);
      entry.mib_per_s_at_p50 = round((bytes * 64) / (st.p50 / 1e9) / 1048576, 1);
    }
    row.per_config[c] = entry;
  }
  // Headline ratios: >1 means the Rust configuration is FASTER than its
  // C counterpart. rust-cabi is compared against c-shipped because those two
  // share the driver, the compiler and the link model exactly.
  const r = (a, b) => {
    const x = row.per_config[a]?.ns_per_call?.p50;
    const y = row.per_config[b]?.ns_per_call?.p50;
    return x && y ? round(x / y, 3) : null;
  };
  row.speedup_p50 = {
    "rust-cabi_over_c-shipped": r("c-shipped", "rust-cabi"),
    "rust-native_over_c-lto": r("c-lto", "rust-native"),
    "rust-native_over_c-shipped": r("c-shipped", "rust-native"),
  };
  const r99 = (a, b) => {
    const x = row.per_config[a]?.ns_per_call?.p99;
    const y = row.per_config[b]?.ns_per_call?.p99;
    return x && y ? round(x / y, 3) : null;
  };
  row.speedup_p99 = {
    "rust-cabi_over_c-shipped": r99("c-shipped", "rust-cabi"),
    "rust-native_over_c-lto": r99("c-lto", "rust-native"),
  };
  workloadResults.push(row);
}

// --- peak RSS -------------------------------------------------------------

function parseRss() {
  if (!exists("rss.txt")) return null;
  const out = {};
  let cur = null;
  for (const line of read("rss.txt").split(/\r?\n/)) {
    if (line.startsWith("### ")) {
      const [, bin, prof] = line.split(/\s+/);
      const key = `${bin}|${prof}`;
      out[key] ??= { peak_working_set_bytes: [], peak_pagefile_bytes: [], page_faults: [] };
      cur = out[key];
    } else if (cur && line.includes("=")) {
      const [k, v] = line.split("=");
      if (k in cur) cur[k].push(Number(v));
    }
  }
  const BIN2CFG = {
    bench_c_shipped: "c-shipped",
    bench_c_lto: "c-lto",
    bench_rustcabi: "rust-cabi",
    bench_rs: "rust-native",
  };
  const res = {};
  for (const [key, v] of Object.entries(out)) {
    const [bin, prof] = key.split("|");
    res[prof] ??= {};
    const st = stats(v.peak_working_set_bytes);
    res[prof][BIN2CFG[bin] ?? bin] = {
      runs: st.n,
      peak_working_set_bytes: { min: st.min, p50: st.p50, max: st.max },
      peak_working_set_kib_p50: round(st.p50 / 1024, 1),
      peak_pagefile_kib_p50: round(pct([...v.peak_pagefile_bytes].sort((a, b) => a - b), 50) / 1024, 1),
      page_faults_p50: pct([...v.page_faults].sort((a, b) => a - b), 50),
    };
  }
  return res;
}

// --- hyperfine ------------------------------------------------------------

function parseHyperfine(file) {
  if (!exists(file)) return null;
  const j = JSON.parse(read(file));
  const BIN2CFG = [
    ["bench_c_shipped", "c-shipped"],
    ["bench_c_lto", "c-lto"],
    ["bench_rustcabi", "rust-cabi"],
    ["bench_rs", "rust-native"],
  ];
  const res = {};
  for (const r of j.results) {
    const hit = BIN2CFG.find(([b]) => r.command.includes(b));
    const name = hit ? hit[1] : r.command;
    const timesMs = r.times.map((t) => t * 1000);
    const st = stats(timesMs);
    res[name] = {
      runs: st.n,
      ms: {
        min: round(st.min),
        p50: round(st.p50),
        p90: round(st.p90),
        p99: round(st.p99),
        max: round(st.max),
        mean: round(st.mean),
        stddev: round(st.stddev),
        cv_pct: round(st.cv_pct, 2),
      },
    };
  }
  return res;
}

// --- cold first call ------------------------------------------------------

function parseFirstCall() {
  if (!exists("firstcall.txt")) return null;
  const BIN2CFG = {
    bench_c_shipped: "c-shipped",
    bench_c_lto: "c-lto",
    bench_rustcabi: "rust-cabi",
    bench_rs: "rust-native",
  };
  // Format: a `### <bin> <algo>` header followed by one nanosecond value per
  // line. Deliberately not CSV — see the note in run.sh.
  const res = {};
  let bin = null;
  let algo = null;
  let vals = [];
  const flush = () => {
    if (!bin || !vals.length) return;
    const st = stats(vals);
    res[algo] ??= {};
    res[algo][BIN2CFG[bin] ?? bin] = {
      runs: st.n,
      ns: {
        min: round(st.min),
        p50: round(st.p50),
        p90: round(st.p90),
        p99: round(st.p99),
        max: round(st.max),
        mean: round(st.mean),
      },
    };
    vals = [];
  };
  for (const raw of read("firstcall.txt").split(/\r?\n/)) {
    const line = raw.trim();
    if (line.startsWith("### ")) {
      flush();
      [, bin, algo] = line.split(/\s+/);
    } else if (line !== "" && Number.isFinite(Number(line))) {
      vals.push(Number(line));
    }
  }
  flush();
  return res;
}

// --- binary sizes ---------------------------------------------------------

function binarySizes() {
  const out = {};
  const build = path.join(BENCH, "build");
  const map = {
    "c-shipped": "bench_c_shipped.exe",
    "c-lto": "bench_c_lto.exe",
    "rust-cabi": "bench_rustcabi.exe",
    "rust-native": "bench_rs.exe",
  };
  for (const [cfg, f] of Object.entries(map)) {
    const p = path.join(build, f);
    if (fs.existsSync(p)) out[cfg] = fs.statSync(p).size;
  }
  const libs = {
    "libcrc.a (C, oracle)": path.join(BENCH, "..", "oracle", "lib", "libcrc.a"),
    "libcrc.a (Rust staticlib)": path.join(BENCH, "..", "target", "release", "libcrc.a"),
  };
  const libOut = {};
  for (const [k, p] of Object.entries(libs)) if (fs.existsSync(p)) libOut[k] = fs.statSync(p).size;
  return { driver_binary_bytes: out, static_library_bytes: libOut };
}

// --- clock ----------------------------------------------------------------

function parseClock() {
  const grab = (f) => {
    if (!exists(f)) return {};
    const o = {};
    for (const l of read(f).split(/\r?\n/)) {
      if (l.startsWith("#M ")) {
        const sp = l.indexOf(" ", 3);
        o[l.slice(3, sp)] = l.slice(sp + 1);
      }
    }
    return o;
  };
  return { c_driver: grab("clock_c.txt"), rust_driver: grab("clock_rust.txt") };
}

// --- assemble -------------------------------------------------------------

const results = {
  schema: "portmortem-2026/bench/results.json v1",
  generated_utc: new Date().toISOString(),
  subject: {
    port: "libcrc-rs (Rust) — crates/libcrc-rs",
    original: "lammertb/libcrc (C) — built in the gitignored oracle/ tree",
  },
  configurations: {
    "c-shipped": {
      what: "bench/c/bench_c.c linked against oracle/lib/libcrc.a exactly as `mingw32-make OS=posix CC=gcc EXEEXT=.exe` builds it",
      library_flags: "-Wall -Wextra -Wstrict-prototypes -Wshadow -Wpointer-arith -Wcast-qual -Wcast-align -Wwrite-strings -Wredundant-decls -Wnested-externs -Werror -O3 -funsigned-char (libcrc's own Makefile CFLAGS)",
      driver_flags: "-O3 -funsigned-char -Wall -Wextra -std=c99",
      lto: false,
      note: "The baseline a real libcrc user gets today.",
    },
    "c-lto": {
      what: "same driver, libcrc sources recompiled at -O3 -funsigned-char -flto and link-time-optimised with the driver",
      driver_flags: "-O3 -funsigned-char -Wall -Wextra -std=c99 -flto",
      lto: true,
      note: "The fair upper bound for C: gives the C compiler the same cross-module inlining the port gets from lto=true. Not what libcrc ships.",
    },
    "rust-cabi": {
      what: "THE SAME C DRIVER, same gcc, same flags, linked against target/release/libcrc.a (the port, through its C ABI)",
      driver_flags: "-O3 -funsigned-char -Wall -Wextra -std=c99 -DNO_UPDATE_API",
      lto: "within the Rust staticlib only; no cross-language LTO",
      note: "The controlled experiment. Driver source, compiler, flags, clock and link model are identical to c-shipped; the ONLY variable is the library. Incremental (bytewise) workloads are absent because the port's C ABI shim exports the 13 symbols the original test suite needs, not libcrc's update_crc_* family.",
    },
    "rust-native": {
      what: "bench/rust — Rust driver calling libcrc-rs directly",
      driver_flags: "opt-level=3, lto=true, codegen-units=1 (mirrors the port's own [profile.release])",
      lto: true,
      note: "What a Rust consumer of the port actually gets.",
    },
  },
  methodology_pointer: "bench/methodology.md",
  percentile_convention: "nearest-rank, index = ceil(p/100 * N) - 1, no interpolation",
  clock: parseClock(),
  equivalence_cross_check: {
    what: "every configuration prints an XOR fold of six CRCs over the same deterministic buffer before any timing is reported",
    minimal_1KiB: "303169684 (identical across all four configurations)",
    work1MiB: "3785426264248391083 (identical across all four configurations)",
    why: "if two configurations ever print different folds they are not computing the same function and the benchmark is meaningless; this check runs on every invocation",
  },
  rss_instrument: {
    how: "bench/tools/rssrun.c: CreateProcess -> WaitForSingleObject -> GetProcessMemoryInfo on the still-open process handle. Not sampling — the kernel's lifetime peak counters, read after exit.",
    why_not_polling: "Get-Process polling races processes that live for tens of milliseconds; a 100 MiB allocate-and-free can happen entirely between two samples.",
    validation: "rssrun reports ~3.4 MB for the `minimal` profile and ~107.8 MB for `work100m`, a delta of ~104.4 MB against a known 104,857,600-byte allocation. The instrument resolves the allocation it is supposed to resolve.",
    common_mode: "the same instrument measures every configuration, so any bias it carries cancels in the comparison",
  },
  binary_sizes: binarySizes(),
  peak_rss: parseRss(),
  startup_noop: parseHyperfine("startup.json"),
  end_to_end_1mib_job: parseHyperfine("e2e_1mib.json"),
  cold_first_call: parseFirstCall(),
  workloads: workloadResults,
};

fs.writeFileSync(path.join(BENCH, "results.json"), JSON.stringify(results, null, 2) + "\n");

// --- human summary --------------------------------------------------------

const fmt = (x, w = 9, d = 2) =>
  (x === null || x === undefined ? "-" : Number(x).toFixed(d)).padStart(w);

console.log(`\nwrote bench/results.json  (${workloadResults.length} workloads)\n`);

function table(title, filter, valueOf, unit) {
  console.log(`\n== ${title} (${unit}) ==`);
  console.log(
    "algo".padEnd(16) + "bytes".padStart(10) +
      CONFIGS.map((c) => c.padStart(13)).join("") + "   rust-cabi/c-shipped"
  );
  for (const w of workloadResults.filter(filter)) {
    const cells = CONFIGS.map((c) => fmt(valueOf(w.per_config[c]), 13, 3)).join("");
    const sp = w.speedup_p50["rust-cabi_over_c-shipped"];
    console.log(
      w.algo.padEnd(16) + String(w.bytes).padStart(10) + cells +
        "   " + (sp === null ? "-" : `${sp.toFixed(2)}x`)
    );
  }
}

table("one-shot, 64 B — per-call overhead dominates", (w) => w.kind === "oneshot" && w.bytes === 64,
  (e) => e?.ns_per_call.p50, "ns/call p50");
table("one-shot, 64 B — TAIL", (w) => w.kind === "oneshot" && w.bytes === 64,
  (e) => e?.ns_per_call.p99, "ns/call p99");
table("one-shot, 1 MiB — throughput dominates", (w) => w.kind === "oneshot" && w.bytes === 1048576,
  (e) => e?.ns_per_byte_at_p50, "ns/byte p50");
table("one-shot, 100 MiB", (w) => w.kind === "oneshot" && w.bytes === 104857600,
  (e) => e?.ns_per_byte_at_p50, "ns/byte p50");
table("bytewise streaming, 64 KiB", (w) => w.kind === "bytewise",
  (e) => e?.ns_per_byte_at_p50, "ns/byte p50");
table("many small calls (100k x 64 B)", (w) => w.kind === "manysmall",
  (e) => e?.ns_per_inner_call_at_p50, "ns per 64 B call p50");

console.log("\n== peak working set (KiB, p50 of 5 runs) ==");
const rss = results.peak_rss;
if (rss) {
  for (const prof of Object.keys(rss)) {
    const line = CONFIGS.map(
      (c) => `${c}=${rss[prof][c]?.peak_working_set_kib_p50 ?? "-"}`
    ).join("  ");
    console.log(prof.padEnd(10) + line);
  }
}

console.log("\n== startup, `noop` (ms) ==");
const su = results.startup_noop;
if (su) for (const c of CONFIGS) if (su[c])
  console.log(c.padEnd(14) + `p50=${fmt(su[c].ms.p50)}  p99=${fmt(su[c].ms.p99)}  min=${fmt(su[c].ms.min)}  cv=${fmt(su[c].ms.cv_pct, 6)}%`);

console.log("\n== cold first call (ns) ==");
const fc = results.cold_first_call;
if (fc) for (const algo of Object.keys(fc)) {
  console.log(
    algo.padEnd(16) +
      CONFIGS.map((c) => `${c}: p50=${fc[algo][c]?.ns.p50 ?? "-"}`).join("  ")
  );
}
console.log();
