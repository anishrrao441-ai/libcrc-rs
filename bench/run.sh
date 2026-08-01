#!/usr/bin/env bash
# bench/run.sh — reproduce every number in bench/results.json from scratch.
#
#   ./bench/run.sh all        build everything, measure everything, analyse
#   ./bench/run.sh build      just build the four benchmark binaries
#   ./bench/run.sh <stage>    build | clock | rssvalidate | main | rss | startup
#                             | firstcall | analyze
#
# Prerequisites (all verified present on the measurement machine):
#   gcc 16.1.0 (MSYS2), rustc/cargo 1.96.0 x86_64-pc-windows-gnu,
#   hyperfine 1.20.0, node, and a built C oracle at oracle/lib/libcrc.a.
#
# The oracle is gitignored and is NOT a dependency of the port. Build it with:
#   cp -r <libcrc-source>/. oracle/ && rm -rf oracle/.git
#   cd oracle && mingw32-make OS=posix CC=gcc EXEEXT=.exe
# `-funsigned-char` is part of libcrc's own CFLAGS and is NOT optional: gcc on
# x86 defaults to signed char, which makes the C baseline itself wrong.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BENCH="$ROOT/bench"
BUILD="$BENCH/build"
RAW="$BENCH/raw"
ORACLE="$ROOT/oracle"

# libcrc's own optimisation and signedness flags. Using anything weaker here
# would sandbag the C baseline; -O3 is the level the project ships with.
LIBCRC_CFLAGS="-O3 -funsigned-char"
# Driver flags. Identical for every C-linked configuration, so driver codegen
# is held constant and only the library under test varies.
DRIVER_CFLAGS="-O3 -funsigned-char -Wall -Wextra -std=c99"
# Windows import libraries the Rust staticlib needs (std, not the port itself).
RUST_LIBS="-lws2_32 -luserenv -lbcrypt -lntdll -ladvapi32 -lkernel32"

mkdir -p "$BUILD" "$RAW"

need_oracle() {
  if [ ! -f "$ORACLE/lib/libcrc.a" ]; then
    echo "FATAL: $ORACLE/lib/libcrc.a not found." >&2
    echo "  cd oracle && mingw32-make OS=posix CC=gcc EXEEXT=.exe" >&2
    exit 1
  fi
}

stage_build() {
  need_oracle
  echo "== building the port (release, lto=true, codegen-units=1) =="
  (cd "$ROOT" && cargo build --release)

  echo "== rssrun (the RSS instrument) =="
  gcc -O2 -o "$BUILD/rssrun.exe" "$BENCH/tools/rssrun.c" -lpsapi

  echo "== c-shipped: driver + oracle/lib/libcrc.a, exactly as make builds it =="
  gcc $DRIVER_CFLAGS -I"$ORACLE/include" -o "$BUILD/bench_c_shipped.exe" \
      "$BENCH/c/bench_c.c" "$ORACLE/lib/libcrc.a"

  echo "== c-lto: same driver, libcrc sources recompiled at -O3 -flto =="
  echo "   (matches the port's lto=true so cross-module inlining is available to both)"
  gcc $DRIVER_CFLAGS -flto -I"$ORACLE/include" -o "$BUILD/bench_c_lto.exe" \
      "$BENCH/c/bench_c.c" $LIBCRC_CFLAGS -flto "$ORACLE"/src/*.c

  echo "== rust-cabi: the SAME C driver, linked against the port's staticlib =="
  echo "   (-DNO_UPDATE_API: the port's C ABI shim exports the 13 symbols the"
  echo "    original suite needs, not libcrc's update_crc_* family)"
  gcc $DRIVER_CFLAGS -DNO_UPDATE_API -I"$ROOT/tests/include" \
      -o "$BUILD/bench_rustcabi.exe" \
      "$BENCH/c/bench_c.c" "$ROOT/target/release/libcrc.a" $RUST_LIBS

  echo "== rust-native: Rust driver, direct calls, cross-crate LTO =="
  (cd "$BENCH/rust" && cargo build --release)
  cp "$BENCH/rust/target/release/bench_rs.exe" "$BUILD/bench_rs.exe"

  ls -la "$BUILD"
}

stage_clock() {
  echo "== clock resolution, measured not assumed =="
  "$BUILD/bench_c_shipped.exe" clockres | tee "$RAW/clock_c.txt"
  "$BUILD/bench_rs.exe" clockres | tee "$RAW/clock_rust.txt"
}

# Prove the RSS instrument responds to a known 100 MiB allocation before any of
# its numbers are published.
stage_rssvalidate() {
  echo "== validating rssrun against a known 100 MiB allocation =="
  {
    echo "-- c-shipped minimal"; "$BUILD/rssrun.exe" "$BUILD/bench_c_shipped.exe" rss minimal
    echo "-- c-shipped work100m"; "$BUILD/rssrun.exe" "$BUILD/bench_c_shipped.exe" rss work100m
    echo "-- rust-native minimal"; "$BUILD/rssrun.exe" "$BUILD/bench_rs.exe" rss minimal
    echo "-- rust-native work100m"; "$BUILD/rssrun.exe" "$BUILD/bench_rs.exe" rss work100m
  } | tee "$RAW/rss_validate.txt"
}

stage_main() {
  echo "== main measurement sweep (this takes a few minutes) =="
  "$BUILD/bench_c_shipped.exe" run c-shipped  > "$RAW/samples_c-shipped.txt"
  "$BUILD/bench_c_lto.exe"     run c-lto      > "$RAW/samples_c-lto.txt"
  "$BUILD/bench_rustcabi.exe"  run rust-cabi  > "$RAW/samples_rust-cabi.txt"
  "$BUILD/bench_rs.exe"        run rust-native > "$RAW/samples_rust-native.txt"
  wc -l "$RAW"/samples_*.txt
}

# An independent repeat of the whole sweep, in a separate set of processes and
# minutes later. Nothing is averaged across the two: rep2 exists so the report
# can state how much of the run-to-run spread is the machine rather than the
# code. A p50 that moves by 15% between identical runs is not a 5% result.
stage_main2() {
  echo "== repeat sweep for run-to-run reproducibility =="
  "$BUILD/bench_c_shipped.exe" run c-shipped   > "$RAW/rep2_c-shipped.txt"
  "$BUILD/bench_c_lto.exe"     run c-lto       > "$RAW/rep2_c-lto.txt"
  "$BUILD/bench_rustcabi.exe"  run rust-cabi   > "$RAW/rep2_rust-cabi.txt"
  "$BUILD/bench_rs.exe"        run rust-native > "$RAW/rep2_rust-native.txt"
}

stage_probe() {
  echo "== codegen control experiments (branch vs cmov, snprintf vs hex table) =="
  gcc -O3 -funsigned-char -Wall -Wextra -std=c99 \
      -o "$BUILD/probe_codegen.exe" "$BENCH/c/probe_codegen.c"
  "$BUILD/probe_codegen.exe" 2>/dev/null | tee "$RAW/probe_codegen.txt"
}

stage_rss() {
  echo "== peak RSS, 5 repeats per (binary, profile) =="
  : > "$RAW/rss.txt"
  for prof in minimal work1m work100m; do
    for b in bench_c_shipped bench_c_lto bench_rustcabi bench_rs; do
      for _ in 1 2 3 4 5; do
        echo "### $b $prof" >> "$RAW/rss.txt"
        "$BUILD/rssrun.exe" "$BUILD/$b.exe" rss "$prof" >> "$RAW/rss.txt"
      done
    done
  done
  echo "wrote $RAW/rss.txt"
}

stage_startup() {
  echo "== startup: hyperfine, 30 warmups + 300 runs, shell bypassed (-N) =="
  hyperfine -N --warmup 30 --runs 300 --style none \
    --export-json "$RAW/startup.json" \
    "$BUILD/bench_c_shipped.exe noop" \
    "$BUILD/bench_c_lto.exe noop" \
    "$BUILD/bench_rustcabi.exe noop" \
    "$BUILD/bench_rs.exe noop"

  echo "== end-to-end 1 MiB job: process spawn + init + six CRCs over 1 MiB =="
  hyperfine -N --warmup 10 --runs 100 --style none \
    --export-json "$RAW/e2e_1mib.json" \
    "$BUILD/bench_c_shipped.exe rss work1m" \
    "$BUILD/bench_c_lto.exe rss work1m" \
    "$BUILD/bench_rustcabi.exe rss work1m" \
    "$BUILD/bench_rs.exe rss work1m"
}

# Cold first call: a fresh process per measurement, so the C library actually
# pays init_crc16_tab() and the port actually pays its .rodata page fault.
stage_firstcall() {
  echo "== cold first-call latency, 150 fresh processes per (binary, algo) =="
  : > "$RAW/firstcall.txt"
  for algo in crc_16 crc_ccitt_ffff crc_kermit crc_32 crc_8; do
    for b in bench_c_shipped bench_c_lto bench_rustcabi bench_rs; do
      # One value per line under a header. Do NOT reassemble these into a CSV
      # row with paste/tr: MSYS paste merged records here, silently splicing
      # two binaries' samples onto one line.
      echo "### $b $algo" >> "$RAW/firstcall.txt"
      for _ in $(seq 150); do "$BUILD/$b.exe" firstcall "$algo"; done \
        >> "$RAW/firstcall.txt"
    done
  done
  echo "wrote $RAW/firstcall.txt"
}

stage_analyze() {
  echo "== analysis =="
  node "$BENCH/analyze.mjs"
}

stage="${1:-all}"
case "$stage" in
  build)       stage_build ;;
  clock)       stage_clock ;;
  rssvalidate) stage_rssvalidate ;;
  main)        stage_main ;;
  main2)       stage_main2 ;;
  probe)       stage_probe ;;
  rss)         stage_rss ;;
  startup)     stage_startup ;;
  firstcall)   stage_firstcall ;;
  analyze)     stage_analyze ;;
  all)
    stage_build; stage_clock; stage_rssvalidate; stage_main; stage_main2
    stage_probe; stage_rss; stage_startup; stage_firstcall; stage_analyze ;;
  *) echo "unknown stage: $stage" >&2; exit 1 ;;
esac
