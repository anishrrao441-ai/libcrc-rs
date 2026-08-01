#!/usr/bin/env bash
#
# fuzz/negative-control.sh — prove the fuzzer can actually fail.
#
# "Zero divergences" is only meaningful if a divergence would have been caught. This
# builds a deliberately corrupted oracle (-DPM_SABOTAGE flips one bit of crc_kermit for
# any input containing byte 0x7A), runs the fuzzer against it, and expects:
#
#   * a non-zero exit code,
#   * crc_kermit named as the disagreeing algorithm,
#   * the input minimised to the single trigger byte.
#
# It writes to fuzz/negative-control.log, NOT to fuzz/log.txt, so the published run is
# never overwritten by a sabotaged one.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ORACLE_DIR="$ROOT/oracle"
BUILD_DIR="$ROOT/fuzz/build"
SABOTAGED="$BUILD_DIR/oracle_harness_sabotaged.exe"
FUZZER="$ROOT/fuzz/differential/target/release/difffuzz.exe"
LOG="$ROOT/fuzz/negative-control.log"

mkdir -p "$BUILD_DIR/work-negative"

echo "==> building the SABOTAGED oracle harness"
gcc -Wall -Wextra -Werror -O2 -funsigned-char -DPM_SABOTAGE \
	-I"$ORACLE_DIR/include" -o "$SABOTAGED" \
	"$ROOT/fuzz/oracle_harness.c" "$ORACLE_DIR/lib/libcrc.a"

echo "==> fuzzing against it (this MUST report divergences)"
echo
"$FUZZER" \
	--seed 0xC0FFEE \
	--seconds 6 \
	--batch 2000 \
	--oracle "$SABOTAGED" \
	--workdir "$BUILD_DIR/work-negative" \
	--log "$LOG"
status=$?

echo
if [ "$status" -eq 0 ]; then
	echo "NEGATIVE CONTROL FAILED: the fuzzer found nothing against a broken oracle." >&2
	echo "The harness is not detecting divergences. Do not trust any clean run." >&2
	exit 1
fi

if grep -q "crc_kermit" "$LOG"; then
	echo "NEGATIVE CONTROL PASSED: divergences found and crc_kermit named. Log: $LOG"
	exit 0
fi

echo "NEGATIVE CONTROL INCONCLUSIVE: divergences found but crc_kermit was not named." >&2
echo "Inspect $LOG." >&2
exit 1
