#!/usr/bin/env bash
#
# fuzz/prove_d01.sh — reproduce D-01: a documented libcrc entry point that cannot link.
#
# include/checksum.h:99 declares
#
#     uint64_t update_crc_64_ecma( uint64_t crc, unsigned char c );
#
# It is never defined. The symbol that exists is update_crc_64(), which the public header
# does not declare. So the documented incremental CRC-64 function fails to link, and the
# working one is undocumented.
#
# This script compiles a three-line program that uses nothing but the public header, and
# shows it failing at link time. Exit 0 means the bug reproduced.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ORACLE_DIR="$ROOT/oracle"
WORK="$ROOT/fuzz/build/d01"

if [ ! -f "$ORACLE_DIR/lib/libcrc.a" ]; then
	echo "oracle/lib/libcrc.a not built. Run ./fuzz/run.sh first." >&2
	exit 2
fi

mkdir -p "$WORK"

cat > "$WORK/d01.c" <<'EOF'
/* Uses only the documented public API, exactly as checksum.h advertises it. */
#include <stdio.h>
#include <stdint.h>
#include "checksum.h"

int main( void ) {
	uint64_t crc = update_crc_64_ecma( 0, (unsigned char) 'a' );
	printf( "%llu\n", (unsigned long long) crc );
	return 0;
}
EOF

echo "==> 1. the declaration exists in the public header"
grep -n "update_crc_64" "$ORACLE_DIR/include/checksum.h" || true
echo

echo "==> 2. no definition exists anywhere in src/"
if grep -rn "update_crc_64_ecma" "$ORACLE_DIR/src/" ; then
	echo "    (unexpectedly found one)"
else
	echo "    none found"
fi
echo

echo "==> 3. what the built archive actually exports"
nm "$ORACLE_DIR/lib/libcrc.a" 2>/dev/null | grep -i "update_crc_64" || echo "    (none)"
echo

echo "==> 4. the caller compiles cleanly against the public header ..."
if ! gcc -c -Wall -Wextra -Werror -O2 -funsigned-char -I"$ORACLE_DIR/include" \
	-o "$WORK/d01.o" "$WORK/d01.c"; then
	echo "    unexpected: the caller did not even compile" >&2
	exit 2
fi
echo "    yes — no warnings, no errors"
echo

echo "==> 5. ... and needs a symbol the archive does not contain"
nm "$WORK/d01.o" | grep -i "update_crc" | sed 's/^/    /'
echo "    'U' = undefined: this object requires update_crc_64_ecma at link time,"
echo "    and step 3 shows the archive defines only update_crc_64."
echo

echo "==> 6. linking"
if gcc -o "$WORK/d01.exe" "$WORK/d01.o" "$ORACLE_DIR/lib/libcrc.a" 2> "$WORK/link.err"; then
	echo
	echo "D-01 DID NOT REPRODUCE: it linked. Upstream may have fixed it." >&2
	exit 1
fi

echo "    link failed, as expected:"
sed 's/^/    /' "$WORK/link.err"
# collect2 on this MSYS2 toolchain does not propagate ld's "undefined reference" line,
# which is why steps 3 and 5 carry the evidence rather than the linker message.
echo
echo "D-01 REPRODUCED: update_crc_64_ecma() is declared in the public header at"
echo "                 include/checksum.h:99, is defined nowhere, and cannot be linked."
echo "                 update_crc_64() exists in the archive but is undeclared, so the"
echo "                 working entry point is undocumented and the documented one is"
echo "                 uncallable."
exit 0
