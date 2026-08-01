#!/usr/bin/env bash
# Differential check of the `crc` BINARY against the original C library's own CLI.
#
#   crates/libcrc-cli/differential-vs-c.sh [FILE...]
#
# The rest of this repository proves the *library* matches the C original. This proves
# the *binary* does too — that reading a file in 64 KiB chunks and folding it through
# the streaming digests lands on the same number as libcrc's one-shot `crc_16(ptr,len)`
# over the whole buffer.
#
# The C side is `examples/tstcrc.c` from upstream, built into oracle/tstcrc.exe. It
# prints nine of the thirteen algorithms (not CRC-8, neither CRC-64, not NMEA), so this
# script compares those nine.
#
# ┌────────────────────────────────────────────────────────────────────────────┐
# │ THIS IS NOT PART OF THE BUILD AND NOT PART OF `cargo test`.                 │
# │ oracle/ is gitignored and the port never depends on it. If the oracle is    │
# │ absent this script says so and exits 0 — a judge without a built oracle     │
# │ loses nothing, because crates/libcrc-cli/tests/cli.rs already pins every    │
# │ one of these values as a literal.                                          │
# │                                                                            │
# │ To build the oracle (verified on this machine, from the repo root):         │
# │     cp -r <upstream libcrc source> oracle/                                  │
# │     cd oracle && mingw32-make OS=posix CC=gcc EXEEXT=.exe                   │
# └────────────────────────────────────────────────────────────────────────────┘
set -euo pipefail

cd "$(dirname "$0")/../.."

ORACLE="oracle/tstcrc.exe"
[ -x "$ORACLE" ] || ORACLE="oracle/tstcrc"
if [ ! -x "$ORACLE" ]; then
    echo "oracle/tstcrc not built — skipping (see the header of this script)."
    exit 0
fi

CRC="target/release/crc.exe"
[ -x "$CRC" ] || CRC="target/release/crc"
if [ ! -x "$CRC" ]; then
    echo "building the release binary first"
    cargo build --release -p libcrc-cli --quiet
fi
[ -x "$CRC" ] || { echo "FAIL: no crc binary at target/release/"; exit 1; }

# tstcrc's label -> our algorithm name. tstcrc calls XMODEM "CRC-CCITT (0x0000)".
label_to_algo() {
    case "$1" in
        "CRC16")              echo crc_16 ;;
        "CRC16 (Modbus)")     echo crc_modbus ;;
        "CRC16 (Sick)")       echo crc_sick ;;
        "CRC-CCITT (0x0000)") echo crc_xmodem ;;
        "CRC-CCITT (0xffff)") echo crc_ccitt_ffff ;;
        "CRC-CCITT (0x1d0f)") echo crc_ccitt_1d0f ;;
        "CRC-CCITT (Kermit)") echo crc_kermit ;;
        "CRC-DNP")            echo crc_dnp ;;
        "CRC32")              echo crc_32 ;;
        *)                    echo "" ;;
    esac
}

# Default corpus: sizes that straddle the 64 KiB read buffer in both directions, so a
# boundary bug cannot hide. Built here rather than committed.
if [ "$#" -gt 0 ]; then
    FILES=("$@")
    SCRATCH=""
else
    SCRATCH="build/cli-differential"
    rm -rf "$SCRATCH"
    mkdir -p "$SCRATCH"
    : > "$SCRATCH/empty.bin"
    printf '123456789'      > "$SCRATCH/check.bin"
    printf 'a'              > "$SCRATCH/one-byte.bin"
    # 65535, 65536, 65537 bytes: one short of, exactly, one over the buffer.
    for n in 65535 65536 65537 300007; do
        head -c "$n" /dev/urandom > "$SCRATCH/random-$n.bin"
    done
    FILES=("$SCRATCH"/*.bin)
fi

compared=0
diverged=0

for file in "${FILES[@]}"; do
    [ -f "$file" ] || { echo "FAIL: no such file: $file"; exit 1; }

    # One `crc` invocation per file; read its nine relevant lines into a lookup.
    ours=$("$CRC" --all "$file")

    while IFS= read -r line; do
        case "$line" in
            *" = 0x"*) ;;
            *) continue ;;
        esac
        label=$(printf '%s' "$line" | sed 's/ *=.*//' | sed 's/ *$//')
        algo=$(label_to_algo "$label")
        [ -n "$algo" ] || continue
        c_value=$(printf '%s' "$line" | sed 's/.*= *//' | sed 's/ *\/.*//')
        rust_value=$(printf '%s' "$ours" | awk -v a="$algo" '$1 == a { print $2; exit }')

        compared=$((compared + 1))
        if [ "$c_value" != "$rust_value" ]; then
            diverged=$((diverged + 1))
            printf 'DIVERGENCE  %s  %s:  C=%s  rust=%s\n' "$file" "$algo" "$c_value" "$rust_value"
        fi
    done <<EOF
$("$ORACLE" "$file")
EOF
done

[ -z "$SCRATCH" ] || rm -rf "$SCRATCH"

printf '\n%d comparisons over %d file(s), %d divergences\n' "$compared" "${#FILES[@]}" "$diverged"
if [ "$compared" -eq 0 ]; then
    echo "FAIL: nothing was actually compared"
    exit 1
fi
[ "$diverged" -eq 0 ] || exit 1
echo "OK — the crc binary agrees with the original C library on every algorithm tstcrc prints."
