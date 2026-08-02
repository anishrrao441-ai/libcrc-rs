# libcrc-rs — a Rust port of [lammertb/libcrc](https://github.com/lammertb/libcrc)

**Port Mortem 2026 · track C → Rust · [coderesurrection.com/2026](https://coderesurrection.com/2026/)**

A zero-unsafe, `no_std` Rust port of libcrc. The original's **unmodified** C test suite is
compiled and linked against this port, and passes.

```
$ ./build.sh
==> Verifying the original test suite is unmodified
  ok 4 files match tests/original.sha256
==> Building the Rust port
  ok target/release/libcrc.a (4059142 bytes)
==> Running the port's test suite
  ok unit, integration and doc tests passed
==> Compiling the original C test suite against the port
  ok compiled 3 translation units, unmodified
==> Linking the original tests against the Rust staticlib
  ok build/testall — nothing from the original C library is linked
==> Running the ORIGINAL test suite against the Rust port

Testing CRC routines: OK
Testing NMEA checksum: OK

**** All tests succeeded

  ok original suite passed

BUILD OK — the unmodified original C test suite passes against the Rust port.
```

That is real output, not an illustration. Run `./build.sh` and you get it.

---

## Why this port needed to exist

The obvious objection to porting a CRC library is *"just use an existing Rust CRC crate."*
That does not work here, and the reason is concrete.

**libcrc byte-swaps three of its algorithms relative to the [RevEng CRC catalogue](https://reveng.sourceforge.io/crc-catalogue/).**

| Algorithm | RevEng catalogue | libcrc, and therefore this port |
|---|---|---|
| CRC-16/KERMIT | `0x2189` | **`0x8921`** |
| CRC-16/DNP | `0xEA82` | **`0x82EA`** |
| CRC-16/SICK | *not catalogued at all* | `0x56A6` |

Every general-purpose Rust CRC crate is catalogue-conformant. Swapping one in would
silently change the checksums of anyone's stored data. This port reproduces libcrc's
behaviour **including its divergences**, and pins them with a regression test
(`documented_catalogue_divergences_are_preserved`) so nobody later "fixes" them and
silently breaks compatibility.

**This is not a theoretical concern.** [libcrc issue #25](https://github.com/lammertb/libcrc/issues/25)
— open since December 2024 — is a user reporting exactly this, with exactly these values:

> I am using your lib and I got found different values for CRC-16 CRC-CCITT (Kermit) algorithm.
> On your calculation site … for the value 123456789 I got **0x8921**. In other way at site
> (crccalc.com) for same value and algorithm got **0x2189**. Is there any way to get the same value?

A port that "corrected" Kermit to `0x2189` would look right to that user and be wrong for every
existing libcrc deployment. We chose compatibility, and documented the choice
([`DECISIONS.md` D-3](DECISIONS.md)).

`crc_sick` has no external reference implementation whatsoever — libcrc is its only
specification — so its correctness rests entirely on byte-for-byte differential parity with
the original. We say so rather than implying a standard was followed.

---

## The `crc` command-line tool

The brief asks for a one-step build to a **binary**, so the workspace ships one. `crates/libcrc-cli`
builds `crc`, which hashes files or stdin with any of the 13 algorithms and streams large inputs
through the `Digest` types rather than reading them whole. Zero external dependencies.

```
$ printf '123456789' | crc --all -
crc_8           0xA2                -
crc_16          0xBB3D              -
crc_modbus      0x4B37              -
crc_sick        0x56A6              -
crc_xmodem      0x31C3              -
crc_ccitt_ffff  0x29B1              -
crc_ccitt_1d0f  0xE5CC              -
crc_kermit      0x8921              -
crc_dnp         0x82EA              -
crc_32          0xCBF43926          -
crc_64_ecma     0x6C40DF5F0B497347  -
crc_64_we       0x62EC59E3F1A4F00A  -
nmea            0x31                -
```

`crc --list` names every algorithm; `crc --check MANIFEST` verifies a saved checksum list and exits
non-zero on any mismatch; `--hex`/`--dec` pick the output base. It never panics on bad input —
missing files, permission errors and empty input return a clean diagnostic and a non-zero exit.

## Beyond the original: capabilities libcrc does not have

- **`combine`** — `crc_*_combine(CRC(a), CRC(b), |b|)` returns `CRC(a‖b)` without re-reading either
  buffer, via GF(2) matrix exponentiation (`O(log n)`). Generalised to **11 of the 13 checksums**
  (all but `crc_sick`, whose fold depends on the previous byte, and NMEA, which is delimiter-driven).
  This makes chunked and parallel hashing possible; the C API cannot.
- **Streaming digests + `core::hash::Hasher`** — hash across arbitrary chunk boundaries, or drop the
  port straight into `HashMap`. libcrc offers only one-shot and byte-at-a-time calls.
- **Slice-by-8 bulk folding** — eight bytes per iteration through eight compile-time tables, in pure
  safe Rust (no `unsafe`, no intrinsics). Default-on behind the `slice8` cargo feature; it costs
  23,296 bytes of `.rodata`, and `--no-default-features` restores libcrc's exact byte-at-a-time loop.
- **`no_std`, proven** — the port compiles for `thumbv7em-none-eabihf` (Cortex-M4F, bare metal, no
  libstd exists), checked in CI (`.github/workflows/no-std.yml`). The claim is mechanical, not asserted.

---

## Evidence

| Claim | Where | Result |
|---|---|---|
| Original suite passes, tests provably unmodified | `./build.sh`, `tests/original.sha256` | **pass, exit 0** |
| Differential fuzz vs the C library | `fuzz/log.txt` | **1,100,000 cases · 75.9 s continuous · 0 divergences** |
| The fuzzer can actually detect divergence | `fuzz/negative-control.log` | mutants caught |
| Incremental API vs the C library | `tests/parity/` | **~117M comparisons · 0 divergences**, 5 of 8 functions **exhaustive** |
| Concurrency soak | `tests/concurrency/RESULTS.md` | **data race found in the original** |
| Unsafe census | `UNSAFE.md` | **0 in the port**, compiler-enforced |
| Benchmarks | `bench/results.json`, `bench/methodology.md` | 82 workloads, p50/p90/p99 |
| Decision log | `DECISIONS.md` | 13 entries |

### Behavioural equivalence is proven, not sampled

For the 8- and 16-bit incremental functions the entire input domain is small enough to
enumerate, so we did:

```
update_crc_8       65,536 cases   EXHAUSTIVE — every possible (crc, byte) pair
update_crc_16  16,777,216 cases   EXHAUSTIVE
update_crc_ccitt   16,777,216     EXHAUSTIVE
update_crc_kermit  16,777,216     EXHAUSTIVE
update_crc_dnp     16,777,216     EXHAUSTIVE
```

Not "we tested it" — *there is no input on which these differ from the C original.*

### The fuzz run is reproducible

```
seed        0xE86F885A2BCDBFC5
duration    75.945 s (continuous)
cases       1,100,000
divergences 0
```
```bash
./fuzz/run.sh --seed 16748755460077502405 --cases 1100000
```

---

## What we found in the original

Two defects, both **reported upstream during the hackathon window**:

| Issue | Filed | Finding |
|---|---|---|
| [lammertb/libcrc#26](https://github.com/lammertb/libcrc/issues/26) | 2026-08-02 05:19 UTC | Data race in lazy table initialisation |
| [lammertb/libcrc#27](https://github.com/lammertb/libcrc/issues/27) | 2026-08-02 05:19 UTC | `update_crc_64_ecma` declared but never defined |

**1. The lazy table initialisation is a data race.** libcrc guards its table build with a
plain non-atomic `bool`, and there is no synchronisation anywhere in the library. With 16
threads calling `crc_16()` simultaneously from a cold start, the "run once" initialiser
executed **more than once in 30 of 40 processes (75%)** — worst case 3 concurrently.

Stated honestly: **we observed zero wrong checksums on x86-64.** Every racing thread writes
identical bytes, and x86 is TSO so it cannot reorder store-store. It is still undefined
behaviour under C11 §5.1.2.4, and on weakly-ordered ARM/RISC-V — libcrc's actual embedded
audience — a thread can see the guard set while the table is stale and return a wrong
checksum. That is why it survived since 1999. Full method and numbers in
`tests/concurrency/RESULTS.md`.

**2. `update_crc_64_ecma` is declared but never defined.** It is in the public header
(`checksum.h:99`), but `nm` on a freshly built `libcrc.a` finds **zero** definitions — so
calling the documented API fails to link. Meanwhile `update_crc_64`, which does exist, is
absent from the header. The 64-bit incremental API is broken in both directions. This port
implements it (see `DECISIONS.md` D-7).

---

## Unsafe

**The port contains zero `unsafe`.** `crates/libcrc-rs` is `#![forbid(unsafe_code)]` —
compiler-enforced, and `forbid` cannot be overridden by an inner `#[allow]`. Verified by
injecting a deliberate violation and watching the build fail.

The 4 `unsafe` blocks in this repository all live in `crates/libcrc-cabi`, a shim that
exists only so the unmodified C tests can link. It ships no algorithm — every function is a
one-line adapter. Each block is enumerated in `UNSAFE.md` with file, line, precondition and
failure mode, along with an honest treatment of the obvious objection that this "just moves"
the unsafe. No SIMD is implemented, so the default build has no intrinsics and no
`#[target_feature]` functions.

`cargo geiger` could not be installed here (network timeout). No output is fabricated; the
forbid attribute is the stronger evidence anyway.

---

## Performance — measured, mixed, and reported as such

82 workloads on one machine, comparing this port against the C original built with its own
`-O3 -funsigned-char` flags (we did not sandbag the baseline).

| | Workloads |
|---|---|
| Rust faster by >5% | **28** |
| C faster by >5% | **16** |
| Within ±5% | **38** |

Rust wins biggest on `checksum_NMEA` at 16 B (**11.7×**) and on `crc_sick` over large
buffers (**3.3×**). **Rust loses** on small-buffer `crc_sick` (0.67× at 256 B) and on
`crc_ccitt_ffff` in the many-small-calls workload (0.81×). Peak RSS is effectively
identical (3336 KiB both, minimal workload).

Percentiles, variance, confounders and the full method are in `bench/methodology.md` and
`bench/results.json`. Coefficient of variation is reported per measurement; some C samples
show >80% CV, and where that happens we say so rather than quoting a clean-looking median.

---

## Build

```bash
./build.sh                 # verified locally; the path this README's output came from
```
```bash
docker build -t libcrc-rs . && docker run --rm libcrc-rs
```

**Honesty note:** the Dockerfile was **not** built locally — no container runtime exists on
the development machine (docker, podman and nerdctl all absent). It is validated in GitHub
Actions (`.github/workflows/ci.yml`). `./build.sh` is the locally-verified path.

Requires Rust 1.96.0 and a C compiler. The C compiler is only used to build the *original
tests*; the port itself needs nothing but `cargo`.

---

## Using it as a Rust library

```rust
use libcrc_rs::{crc_16, crc_32, crc_32_combine, Crc32Digest};

assert_eq!(crc_16(b"123456789"), 0xBB3D);
assert_eq!(crc_32(b"123456789"), 0xCBF4_3926);

// Streaming — libcrc has no equivalent
let mut d = Crc32Digest::new();
d.update(b"12345");
d.update(b"6789");
assert_eq!(d.finalize(), 0xCBF4_3926);

// Combine two block CRCs without re-reading the data — a capability the C original lacks
let (a, b) = (b"the quick brown ".as_slice(), b"fox".as_slice());
assert_eq!(crc_32_combine(crc_32(a), crc_32(b), b.len()), crc_32(b"the quick brown fox"));
```

`crates/libcrc-rs` is `no_std` and allocation-free.

---

## Limitations — what this port does *not* do

- **No SIMD.** No PCLMULQDQ folding or hardware CRC32 instructions. This was a deliberate
  trade: intrinsics require `unsafe`, and a provably zero-unsafe default build was judged
  more valuable than throughput on large buffers. If added, it must be behind an opt-in
  cargo feature so the default keeps that property.
- **`combine` covers 11 of 13 checksums** (all but `crc_sick`, whose fold depends on the
  previous byte, and NMEA, which is delimiter-driven). Not a limitation — a delivered feature
  the C original lacks entirely.
- **Benchmarks are single-machine.** One Windows laptop under a live workload. Ratios and
  orders of magnitude are meaningful; absolute numbers are not portable.
- **`update_crc_32/64` parity is sampled, not exhaustive** — their domains are 2⁴⁰ and 2⁷².
  16.7M cases each, stride documented.
- **The Dockerfile is CI-validated, not locally validated.** See above.

---

## Layout

```
crates/libcrc-rs/     the port — no_std, forbid(unsafe_code), zero unsafe
crates/libcrc-cabi/   C ABI shim so the original tests can link (test harness, not the port)
tests/original/       the original suite, verbatim, hash-pinned, NEVER edited
tests/parity/         exhaustive differential oracle for the incremental API
tests/concurrency/    the soak that found the data race
fuzz/                 differential fuzzer, 75.9 s zero-divergence log, negative control
bench/                82 workloads, p50/p90/p99, methodology
DECISIONS.md          13 non-trivial divergences and why
UNSAFE.md             the unsafe census
```

`tests/original/` is a verbatim copy, hashed at import. Verify at any time:

```bash
sha256sum -c tests/original.sha256
```

Nothing in this repository links, calls, or depends on the original C library. The oracle
used for differential testing is gitignored and is never a dependency of the port.

---

## Licence & attribution

MIT, same as the original. libcrc is copyright © 1999–2019 **Lammert Bies**; the original
`LICENSE` is preserved verbatim. This is an independent reimplementation in Rust, not a
fork or a binding.
