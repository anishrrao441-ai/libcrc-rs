#!/usr/bin/env bash
#
# fuzz/run.sh — build everything the differential fuzzer needs, then run it.
#
#   ./fuzz/run.sh                          # 60 s, fresh seed from the clock
#   ./fuzz/run.sh --seconds 300            # longer soak
#   ./fuzz/run.sh --seed 0x... --cases N   # exact replay of a recorded run
#   ./fuzz/run.sh --case 12345 --seed 0x.. # dump one case, both sides, in full
#
# Any flag is passed straight through to the fuzzer; see `difffuzz --help`.
#
# What this builds:
#   oracle/lib/libcrc.a          the ORIGINAL C library, from the pristine upstream tree
#   fuzz/build/oracle_harness    a batch driver linking it, covering all 13 symbols
#   fuzz/differential/           the Rust fuzzer, linking crates/libcrc-rs directly
#
# oracle/ is gitignored and is never a dependency of the port. Nothing under crates/
# links, calls or knows about it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ORACLE_DIR="$ROOT/oracle"
BUILD_DIR="$ROOT/fuzz/build"
HARNESS="$BUILD_DIR/oracle_harness.exe"
FUZZER="$ROOT/fuzz/differential/target/release/difffuzz.exe"

CC_FLAGS="-Wall -Wextra -Wstrict-prototypes -Wshadow -Wpointer-arith -Wcast-qual"
CC_FLAGS="$CC_FLAGS -Wcast-align -Wwrite-strings -Wredundant-decls -Wnested-externs"
# -funsigned-char is NOT optional. libcrc forces char unsigned; gcc on x86 defaults to
# signed, and an oracle built without it is wrong in a way that manufactures divergences.
CC_FLAGS="$CC_FLAGS -Werror -O2 -funsigned-char"

mkdir -p "$BUILD_DIR/work"

# ---------------------------------------------------------------- 1. the C library
if [ ! -f "$ORACLE_DIR/lib/libcrc.a" ]; then
	echo "==> building the original C libcrc (oracle)"
	if [ ! -d "$ORACLE_DIR" ]; then
		echo "    oracle/ is missing. Populate it from the pristine upstream tree:" >&2
		echo "      git clone https://github.com/lammertb/libcrc oracle && rm -rf oracle/.git" >&2
		exit 1
	fi
	# Plain `mingw32-make` picks the MSVC branch of the shipped Makefile and fails.
	# EXEEXT=.exe is required or the build dies at Makefile:190, where strip looks for
	# `prc` while gcc emitted `prc.exe`.
	( cd "$ORACLE_DIR" && mingw32-make OS=posix CC=gcc EXEEXT=.exe )
else
	echo "==> oracle/lib/libcrc.a already built"
fi

# ------------------------------------------------------------- 2. the batch harness
echo "==> building the oracle batch harness"
# shellcheck disable=SC2086
gcc $CC_FLAGS -I"$ORACLE_DIR/include" -o "$HARNESS" \
	"$ROOT/fuzz/oracle_harness.c" "$ORACLE_DIR/lib/libcrc.a"

# ------------------------------------------------------------------ 3. the fuzzer
echo "==> building the Rust fuzzer"
( cd "$ROOT/fuzz/differential" && cargo build --release )

echo "==> running the fuzzer's own unit tests"
( cd "$ROOT/fuzz/differential" && cargo test --release --quiet )

# --------------------------------------------------------------------- 4. go
echo
exec "$FUZZER" \
	--oracle "$HARNESS" \
	--workdir "$BUILD_DIR/work" \
	--log "$ROOT/fuzz/log.txt" \
	"$@"
