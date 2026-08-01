#!/usr/bin/env bash
# One-command build and verification for the libcrc Rust port.
#
#   ./build.sh
#
# Builds the port, compiles the UNMODIFIED original C test suite against it, runs that
# suite, and verifies the tests were not tampered with. Exits non-zero if anything fails.
set -euo pipefail

cd "$(dirname "$0")"
GREEN=$'\033[32m'; RED=$'\033[31m'; DIM=$'\033[2m'; OFF=$'\033[0m'
step() { printf '\n%s==>%s %s\n' "$DIM" "$OFF" "$1"; }
fail() { printf '%sFAIL:%s %s\n' "$RED" "$OFF" "$1" >&2; exit 1; }
ok()   { printf '%s  ok%s %s\n' "$GREEN" "$OFF" "$1"; }

# --- 0. toolchain ---------------------------------------------------------------
step "Checking toolchain"
command -v cargo >/dev/null || fail "cargo not found (install Rust: https://rustup.rs)"
command -v "${CC:-gcc}" >/dev/null || fail "C compiler not found (set \$CC, or install gcc)"
ok "cargo $(cargo --version | cut -d' ' -f2), $(${CC:-gcc} --version | head -1)"

# --- 1. the tests must be pristine ----------------------------------------------
# Checked BEFORE building so a tampered suite can never produce a green run.
step "Verifying the original test suite is unmodified"
if command -v sha256sum >/dev/null; then
    sha256sum -c tests/original.sha256 >/dev/null || fail "tests/original/ has been MODIFIED — refusing to build"
    ok "$(wc -l < tests/original.sha256) files match tests/original.sha256"
else
    printf '  sha256sum unavailable; skipping hash verification\n'
fi

# --- 2. build the port ----------------------------------------------------------
step "Building the Rust port"
cargo build --release --quiet
if   [ -f target/release/libcrc.a  ]; then STATICLIB=target/release/libcrc.a
elif [ -f target/release/crc.lib   ]; then STATICLIB=target/release/crc.lib
else fail "staticlib not produced by cargo build --release"
fi
ok "$STATICLIB ($(wc -c < "$STATICLIB") bytes)"

# --- 3. the port's own tests ----------------------------------------------------
step "Running the port's test suite"
cargo test --quiet 2>&1 | tail -5
ok "unit, integration and doc tests passed"

# --- 4. compile the UNMODIFIED original C tests against the Rust staticlib -------
# Uses libcrc's own CFLAGS. -funsigned-char is REQUIRED: libcrc forces unsigned char,
# and gcc on x86 defaults to signed, which would change behaviour.
step "Compiling the original C test suite against the port"
mkdir -p build
CFLAGS="-Wall -Wextra -Wstrict-prototypes -Wshadow -Wpointer-arith -Wcast-qual \
        -Wcast-align -Wwrite-strings -Wredundant-decls -Wnested-externs \
        -O3 -funsigned-char -Itests/include"
OBJS=""
for f in tests/original/*.c; do
    o="build/$(basename "${f%.c}").o"
    ${CC:-gcc} -c $CFLAGS "$f" -o "$o" || fail "compiling $f"
    OBJS="$OBJS $o"
done
ok "compiled $(echo $OBJS | wc -w) translation units, unmodified"

step "Linking the original tests against the Rust staticlib"
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) SYSLIBS="-lws2_32 -luserenv -lbcrypt -lntdll -ladvapi32 -lkernel32" ;;
    Darwin)               SYSLIBS="-framework Security -framework CoreFoundation" ;;
    *)                    SYSLIBS="-lpthread -ldl -lm" ;;
esac
${CC:-gcc} -o build/testall $OBJS "$STATICLIB" $SYSLIBS || fail "link failed"
ok "build/testall — nothing from the original C library is linked"

# --- 5. the moment of truth -----------------------------------------------------
step "Running the ORIGINAL test suite against the Rust port"
if ./build/testall; then
    ok "original suite passed"
else
    fail "original suite FAILED"
fi

printf '\n%sBUILD OK%s — the unmodified original C test suite passes against the Rust port.\n' "$GREEN" "$OFF"
