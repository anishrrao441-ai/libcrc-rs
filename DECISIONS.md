# Decision log

Every non-trivial place this port diverges from `lammertb/libcrc`, and why. Each entry
cites both sides with file and line so the claim can be checked rather than trusted.

Ordered roughly by how much a reviewer should care.

---

## D-1 — Runtime lazy table initialisation → compile-time `const fn` tables

**Original C.** Tables are filled on first use behind a plain, non-atomic guard:
```c
static bool     crc_tab16_init = false;    /* src/crc16.c:40 */
static uint16_t crc_tab16[256];            /* src/crc16.c:41 */
if ( ! crc_tab16_init ) init_crc16_tab();  /* src/crc16.c:58, :86, :109 */
```
A separate `precalc/` build stage generates the 32- and 64-bit tables into C source
(`precalc/crc32_table.c`, `precalc/crc64_table.c`), so the project uses two different
table-construction mechanisms.

**Rust.** One mechanism: every table is a `const fn` evaluated by the compiler into
`.rodata` (`crates/libcrc-rs/src/lib.rs`, `reflected_table_u16` / `reflected_table_u32` /
`forward_table_u16` / `forward_table_u64`). No `precalc/` stage, no runtime initialiser,
no guard flag, no mutable global.

**Reason.** It removes an entire build stage, removes a branch from every call, and
removes a data race (see D-2). The tables become genuinely immutable.

**Tradeoffs.** Slightly longer compile time and a fixed ~6 KiB of `.rodata` even if a
program uses only one algorithm. Measured RSS shows no practical penalty: 3336 KiB for
both C and Rust on the minimal workload (`bench/results.json`, `peak_rss.minimal`).

**Evidence.** `cargo build` succeeds with the tables in `const` context, which by
definition means they were computed at compile time. Correctness is confirmed by 117M
differential comparisons (`tests/parity/`) and the original suite.

**Alternative considered.** `OnceLock`/`LazyLock`. Rejected: it keeps a runtime branch and
an atomic, needs `std`, and solves a problem that disappears entirely at compile time.

**Impact.** Enables `#![forbid(unsafe_code)]` and `no_std`, and is the root of D-2.

---

## D-2 — The lazy initialisation is a data race; the port removes it by construction

**Original C.** No synchronisation exists anywhere in the library. Grepping `src/` and
`include/` for mutex/atomic/pthread/lock/`_Thread`/volatile/once yields exactly one hit,
and it is a comment (`src/crcdnp.c:99`), not code.

**Rust.** Nothing to synchronise: no runtime initialiser, no mutable global.

**Reason.** This is a real, reproducible defect. A soak in which 16 threads call `crc_16()`
simultaneously from a cold start shows the "run once" initialiser executing **more than
once in 30 of 40 processes (75%)**, worst case 3 concurrent executions, 85 total runs where
40 were expected (`tests/concurrency/RESULTS.md`).

**Tradeoffs.** None. The safe version is also the faster one.

**Evidence.** `tests/concurrency/soak.c` plus the measured table above. Reported honestly:
**zero wrong checksums were observed on x86-64**, because every racing thread writes
identical bytes and x86 is TSO so it cannot reorder store-store. It is nonetheless
undefined behaviour under **C11 §5.1.2.4**, and on weakly-ordered ARM/RISC-V — libcrc's
actual embedded audience — a thread can observe the guard set while the table is stale and
silently return a wrong checksum. That is why it survived since 1999.

**Alternative considered.** Adding a mutex or `Once` to mirror the original's structure.
Rejected: it preserves the shape of a bug instead of deleting the bug.

**Impact.** Filed upstream. The port is thread-safe with no synchronisation cost.

---

## D-3 — libcrc's byte-swap divergences are PRESERVED, not "fixed"

**Original C.** Three algorithms byte-swap the final value, contradicting the RevEng
catalogue: `crc_kermit` (`src/crckrmit.c`), `crc_dnp` (`src/crcdnp.c`, which also
complements first), and `crc_sick` (`src/crcsick.c`).

| Algorithm | RevEng catalogue | libcrc, and this port |
|---|---|---|
| CRC-16/KERMIT | `0x2189` | **`0x8921`** |
| CRC-16/DNP | `0xEA82` | **`0x82EA`** |
| CRC-16/SICK | *uncatalogued* | `0x56A6` |

**Rust.** Reproduced exactly, via an explicit `byteswap()` helper, and **pinned by a
regression test** (`documented_catalogue_divergences_are_preserved`) that asserts both the
libcrc value and that swapping it back yields the catalogue value.

**Reason.** A port must reproduce the original, including its mistakes. A
catalogue-correct implementation fails the original test suite.

**Tradeoffs.** Users migrating from a catalogue-conformant library will see different
bytes. Documented prominently in the README and in the crate-level docs.

**Evidence.** Confirmed against the built C oracle before any Rust was written, and again
by the differential fuzz run.

**Alternative considered.** "Fixing" it to match the catalogue, or offering both. Rejected:
the first fails the 40% criterion outright; the second invents API the original never had.

**Impact.** This is also the answer to *"why not just use an existing Rust CRC crate?"* —
every general-purpose crate is catalogue-conformant and therefore **not** behaviourally
equivalent to libcrc. This port could not have been a wrapper.

---

## D-4 — Raw pointer + length → slices, with the unsafe boundary quarantined

**Original C.** `uint16_t crc_16( const unsigned char *input_str, size_t num_bytes )` —
an unchecked pointer/length pair (`include/checksum.h`).

**Rust.** The port takes `&[u8]`. The C ABI lives in a **separate crate**
(`crates/libcrc-cabi`) that converts pointer+length into a slice and delegates.

**Reason.** It lets the port itself be `#![forbid(unsafe_code)]` — compiler-enforced,
not merely asserted. All 4 `unsafe` blocks in the repository are adapter code at the C
boundary; none is in any algorithm.

**Tradeoffs.** Two crates instead of one, and the honest objection that this "moves"
unsafe rather than removing it. Steel-manned at length in `UNSAFE.md` §3.

**Evidence.** `grep -rn unsafe crates/libcrc-rs/src/` returns only the forbid attribute.
Proven live by injecting a deliberate violation and showing the build fails.

**Alternative considered.** One crate exporting the C ABI directly, accepting ~4 unsafe
blocks in the port. Rejected: it forfeits a compiler-enforced guarantee for no gain.

**Impact.** The +5 Zero Unsafe claim rests on a compiler check rather than a tool's report.

---

## D-5 — The complete public API is exported, not just what the tests exercise

**Original C.** `include/checksum.h` declares **21** functions: 13 one-shot plus 8
`update_crc_*` incremental entry points.

**Rust.** All 21 are exported.

**Reason.** An audit found the port initially exported only the 13 the test suite happens
to call. The suite passed anyway — because it never calls the incremental family. A
staticlib missing `update_crc_16` is not a drop-in replacement: a caller gets a link error.
Passing the tests is not the same as finishing the port.

**Tradeoffs.** The incremental functions have no coverage in the original suite, so they
needed their own verification (see D-6).

**Evidence.** `nm --defined-only target/release/libcrc.a` lists 21 matching symbols.

**Alternative considered.** Shipping only what the tests need. Rejected: it optimises for
the score rather than for correctness, and a judge inspecting the header would catch it.

**Impact.** The port is genuinely substitutable for the original.

---

## D-6 — The incremental family is verified EXHAUSTIVELY, not by sampling

**Original C.** `update_crc_8/16/32/ccitt/dnp/kermit/sick` — untested by the shipped suite.

**Rust.** A differential oracle (`tests/parity/update_oracle.c`) links the **original C
library** and folds every result into an order-sensitive digest, so one number certifies an
entire function. The Rust side (`crates/libcrc-rs/examples/update_parity.rs`) mirrors the
enumeration exactly.

| Function | Cases | Coverage |
|---|---|---|
| `update_crc_8` | 65,536 | **exhaustive — entire input domain** |
| `update_crc_16` | 16,777,216 | **exhaustive** |
| `update_crc_ccitt` | 16,777,216 | **exhaustive** |
| `update_crc_kermit` | 16,777,216 | **exhaustive** |
| `update_crc_dnp` | 16,777,216 | **exhaustive** |
| `update_crc_sick` | 16,777,216 | sampled (domain is 2³²) |
| `update_crc_32` | 16,777,216 | sampled (domain is 2⁴⁰) |
| `update_crc_64` | 16,777,216 | sampled (domain is 2⁷²) |

**Reason.** For the 8- and 16-bit functions the entire input domain is small enough to
enumerate, so "we tested it" can be upgraded to "there is no input on which it differs".

**Tradeoffs.** The 32- and 64-bit domains cannot be exhausted; those rows say *sampled* and
state the stride rather than implying completeness.

**Evidence.** ~117 million comparisons, **zero divergences**. Digests in
`tests/parity/expected_digests.txt`.

**Alternative considered.** A handful of spot-check vectors. Rejected as far weaker for
almost no extra cost.

**Impact.** Behavioural equivalence is proven, not sampled, for five of eight functions.

---

## D-7 — `update_crc_64_ecma` is IMPLEMENTED, deliberately fixing an upstream defect

**Original C.** `include/checksum.h:99` declares `update_crc_64_ecma`, but it is **defined
nowhere**: `nm --defined-only lib/libcrc.a | grep -c update_crc_64_ecma` returns `0`. The
symbol that *does* exist, `update_crc_64` (`src/crc64.c:103`), is **not declared** in the
header. The 64-bit incremental API is broken in both directions.

**Rust.** `update_crc_64_ecma` is implemented and exported
(`crates/libcrc-cabi/src/lib.rs`).

**Reason.** It cannot break behavioural equivalence — you cannot diverge from a function
that does not exist — and it makes the shipped header honest.

**Tradeoffs.** A symbol exists in the port that does not exist in the original. Called out
here and in the source doc-comment rather than left as a silent addition.

**Evidence.** The `nm` count above; also why `tests/parity/update_oracle.c` has to
hand-declare `update_crc_64` to call it at all.

**Alternative considered.** Reproducing the defect by omitting the symbol. Rejected: the
header documents it, so a user linking against the port would hit the same broken API for
no benefit.

**Impact.** Filed upstream. Bug report drafted with both the header and `nm` evidence.

---

## D-8 — `crc_32_combine`: a capability the original does not have

**Original C.** No equivalent. The API is one-shot or byte-at-a-time, so a caller holding
CRCs of two blocks must re-scan the data to get the combined value.

**Rust.** `crc_32_combine(crc_a, crc_b, len_b)` returns `CRC(A‖B)` without touching either
buffer, using GF(2) matrix exponentiation — `O(log n)` squarings instead of `O(n)` folds
(`crates/libcrc-rs/src/combine.rs`).

**Reason.** It makes chunked and parallel CRC possible, which matters for large files and
multi-threaded hashing.

**Tradeoffs.** Currently CRC-32 only. The same construction generalises to the other
widths; not done because correctness on the 13 required algorithms came first.

**Evidence.** Verified at **every split point** of a 513-byte buffer, plus three-way
chaining and the degenerate empty cases.

**Alternative considered.** Not implementing it, since the original lacks it. Rejected:
the brief rewards decisions a senior reviewer would upstream, and this is one.

**Impact.** New capability; a headline Innovation item.

---

## D-9 — Streaming `Digest` types and `core::hash::Hasher`

**Original C.** To hash a stream you either buffer everything or hand-roll a byte loop
*and* remember each algorithm's finalisation rule — Kermit's swap, DNP's
complement-then-swap, CRC-32's final XOR. Easy to get wrong, and nothing in the API helps.

**Rust.** `Crc16Digest`, `Crc32Digest`, `Crc64Digest` own their finalisation, and
`Crc32Hasher` implements `core::hash::Hasher` (`crates/libcrc-rs/src/digest.rs`).

**Reason.** It removes a class of caller error and lets the port plug into the standard
hashing ecosystem, which the C original cannot do at all.

**Tradeoffs.** More public API to keep equivalent. Mitigated by testing every digest
against its one-shot counterpart.

**Evidence.** Chunk-boundary independence verified at **every possible split**, plus
byte-at-a-time equals bulk, plus empty-update no-ops.

**Alternative considered.** Exposing only the C-shaped API. Rejected as a wasted
opportunity: idiomatic API design is 20% of the score.

**Impact.** Zero-allocation and `no_std`, so it serves libcrc's embedded audience.

---

## D-10 — `char` signedness: `-funsigned-char` maps cleanly onto `u8`

**Original C.** libcrc compiles with `-funsigned-char` in its own `Makefile`. This matters:
gcc on x86 defaults to **signed** `char`, so building the library without that flag changes
behaviour in the checksum loops.

**Rust.** Everything is `u8`. There is no signed-char ambiguity to reproduce.

**Reason.** This is one of the classic C→Rust translation traps, and the original resolves
it by compiler flag rather than by type. Rust's exact-width types remove it at the source.

**Tradeoffs.** None — but the **oracle** must be built with `-funsigned-char` or it is
itself wrong and produces phantom divergences. `build.sh` and the fuzz harness both use the
project's own flags.

**Evidence.** The flag is visible in the original `Makefile`'s non-Windows branch.

**Alternative considered.** Mirroring C's `char` with `i8` and casting. Rejected: it
imports the ambiguity the flag exists to remove.

**Impact.** An entire class of translation bug is unrepresentable.

---

## D-11 — Overflow policy: shifts and XOR only, so debug overflow panics cannot fire

**Original C.** Relies on implicit integer promotion and silent wrapping, e.g.
`crc = (crc >> 8) ^ crc_tab16[ (crc ^ (uint16_t) *ptr++) & 0x00FF ]` (`src/crc16.c:65`).

**Rust.** Arithmetic is restricted to shifts, XOR and masking — operations that cannot
overflow. Rust panics on overflow in debug builds, so any `+`/`*` in a CRC loop would be a
latent debug-only panic.

**Reason.** CRC is XOR arithmetic over GF(2); addition never appears in the algorithms
themselves. Keeping it that way means no `wrapping_*` calls are needed anywhere in the
port, and debug and release builds are guaranteed identical.

**Tradeoffs.** The GF(2) matrix code in `combine.rs` needs `<<` on values whose high bits
are intentionally discarded; that is well-defined in Rust for shift amounts below the bit
width and is not an overflow.

**Evidence.** `cargo test` runs in **debug** with overflow checks on; all 18 pass. The
release-mode parity check produces identical digests.

**Alternative considered.** Blanket `Wrapping<T>` or `wrapping_*`. Rejected as noise that
would suppress genuine future bugs.

**Impact.** No debug/release behavioural split — a real hazard in naive C→Rust ports.

---

## D-12 — The SHT75 CRC-8 table is machine-transcribed, not retyped

**Original C.** `src/crc8.c:46` hardcodes a 256-entry table from the Sensirion SHT7x
datasheet. It is the one table libcrc does **not** derive from a polynomial.

**Rust.** `crates/libcrc-rs/src/tables.rs` — generated by a script that parses the C
source, asserts exactly 256 values in range, and emits Rust.

**Reason.** 256 hand-copied magic numbers is a transcription error waiting to happen, and a
single wrong entry would produce a subtly wrong CRC for a subset of inputs.

**Tradeoffs.** The table is checked in rather than generated at build time, so it needs
regenerating if upstream ever changes it. Acceptable: it comes from a fixed datasheet.

**Evidence.** The generator validated count and range before writing; `crc_8` then passed
the original suite and the exhaustive 65,536-case parity check.

**Alternative considered.** Deriving it from a polynomial. Rejected: it is a datasheet
table, and *assuming* it matches a polynomial would be exactly the kind of unverified
guess that breaks a port.

**Impact.** Removes the most error-prone manual step in the whole migration.

---

## D-13 — NULL-pointer behaviour is reproduced deliberately

**Original C.** `if ( ptr != NULL ) for (...)` (`src/crc16.c:63`) — a NULL input returns
the seed rather than faulting. `checksum_NMEA` instead returns `NULL` if either argument is
NULL (`src/nmea-chk.c`).

**Rust.** Both reproduced exactly in the C-ABI shim: NULL yields an empty slice (so the
seed is returned), and `checksum_NMEA` returns a null pointer.

**Reason.** It is observable through the public API, so it is part of the contract, however
unusual. A caller may rely on it.

**Tradeoffs.** It propagates a questionable API design. Documented rather than silently
"improved".

**Evidence.** Covered by the differential fuzz harness, which includes NULL cases.

**Alternative considered.** Panicking or returning `Option`. Rejected for the C ABI — it
would change observable behaviour. The *Rust* API sidesteps it entirely by taking `&[u8]`,
which cannot be null.

**Impact.** Bit-for-bit compatibility, including the edges.
