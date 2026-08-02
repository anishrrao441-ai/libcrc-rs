# REVIEW-CORRECTNESS.md

Independent behavioural-divergence review of `crates/libcrc-rs` and the `libcrc-cabi`
shim against the original C library. Brief: **hunt for real divergence; report, do not
fix.** Priority target: `crates/libcrc-rs/src/slice8.rs`, written by an agent that then
died and never reviewed by anyone.

Reviewer changed **no code**. Working tree at time of review: `cargo test --workspace`
green, `cargo clippy --all-targets -- -D warnings` clean, `cargo fmt --all -- --check`
clean. Toolchain: `rustc 1.96.0 (ac68faa20 2026-05-25)`, `gcc 16.1.0` (MSYS2 UCRT64).

---

## 0. Verdict

**The port is numerically correct. No divergence was found in any of the thirteen
algorithms, in either feature configuration, on 204 441 differential cases.**

`slice8.rs` — the unreviewed module — is **correct**. Its derivation, its eight lane
tables, its tail handling, its endianness, and its crossover threshold were all checked
empirically against the compiled C library, not against the crate's own reference
functions. Nine deliberate defects were injected to prove the check can fail; eight were
caught and the one behaviour-preserving control was correctly not flagged.

Three real findings, **none of them in the port**:

| # | Severity | Where | What |
|---|---|---|---|
| **C-1** | HIGH | `libcrc-cabi` | `checksum_NMEA` faults on input the original handles. Reproduced: original prints `41`, port exits `0xC0000005`. |
| **C-2** | MEDIUM | `libcrc-cabi` | The port's `libcrc.a` does not export `update_crc_64`, which the original does. Link failure reproduced. |
| **C-3** | LOW | build/CI | "The full test suite is run both ways" is not enforced by anything. `cargo test --workspace --no-default-features` is a **no-op**. |

---

## 1. Headline evidence — VERIFIED

A three-way differential harness was written from scratch, outside the repo, and run. It
does not use the crate's own reference functions; the right-hand side is the real C
library compiled from `oracle/src/*.c`.

| Build | Path exercised |
|---|---|
| **C oracle** | original `libcrc`, `gcc -O2 -funsigned-char` |
| **Rust `--features slice8`** | the slice-by-8 accelerated path |
| **Rust default-features-off** | the byte-at-a-time path |

```
204 441 cases  ×  12 CRC values,  + 27 NMEA values  =  2 452 995 checksum values per build

diff rs_slice8.txt rs_bytewise.txt   ->  0 differing lines
diff rs_slice8.txt c.txt             ->  0 differing lines
diff rs_bytewise.txt c.txt           ->  0 differing lines
```

Case mix, all generated identically on both sides by SplitMix64:

| Section | Cases | Purpose |
|---|---|---|
| prefixes of one buffer, length 0…600 | 601 | every tail remainder, every block boundary, both sides of `MIN_LEN` |
| a **fresh** random buffer at every length 0…600 | 601 | content varies at each boundary, not just the length |
| all-`0x00` and all-`0xFF`, 0…300 | 602 | degenerate runs a random buffer never produces |
| large: 1 024, 4 096, 4 099, 65 535/6/7, 1 048 576, 1 048 583 | 8 | the folded loop does essentially all the work |
| incremental `update_crc_*` replay, 0…600 | 601 | the single-byte API and its finalisation rules |
| NMEA sentences (`$`, `$$`, `*`, CR, LF, NUL, high bytes, real GPGGA/GPGLL) | 25 | delimiter and `$`-prefix rules |
| NMEA NULL-argument contract | 2 | returns NULL for either arg NULL |
| NULL input pointer to all twelve length-driven functions | 1 | returns the init value, does not fault |
| **randomized sweep**, random length 0…2 048 | 200 000 | breadth |
| **randomized sweep**, random length 0…70 000 | 2 000 | breadth at multi-block sizes |

Runtime for the identical workload: C oracle 8.88 s, Rust byte-at-a-time 8.83 s, Rust
slice-by-8 **3.96 s**.

---

## 2. Negative control — the harness can fail — VERIFIED

A harness that cannot fail proves nothing. The port was copied to a scratch directory,
one deliberate defect applied per row, rebuilt, and re-diffed against the C oracle. The
repo itself was never touched.

```
M1 u32 fold: slice tables 5 and 4 transposed                 202272 / 204441 cases diverge
M2 ccitt fold: from_be_bytes -> from_le_bytes                201694 / 204441 cases diverge
M3 reflected-16 fold: the two halves of X swapped            202272 / 204441 cases diverge
M4 u8 fold: T_0 term taken from SLICES_8[0]                  201119 / 204441 cases diverge
M5 reflected-16 table recurrence: >> 8 becomes << 8          202272 / 204441 cases diverge
M6 crc_32 tail loop: last leftover byte dropped              178417 / 204441 cases diverge
M7 CONTROL crossover MIN_LEN 16 -> 8 (must stay at 0)             0 / 204441 cases diverge
M8 crc_sick: previous byte forced to 0                       203304 / 204441 cases diverge
M9 nmea: two leading '$' stripped instead of one                 15 / 204441 cases diverge
RESTORED (sanity)                                                 0 / 204441 cases diverge
```

Read M7 and M9 carefully — they are what make the other rows mean something.

* **M7** moves the crossover so that *every* input ≥ 8 bytes takes the folded path
  instead of ≥ 16. Zero divergences. The threshold is therefore a **pure performance
  knob with no correctness role** — which is the reassuring answer, because it means no
  input can fall into a gap between the two paths.
* **M9** perturbs only NMEA, and exactly 15 of the 27 NMEA cases move. The harness
  localises, it does not just light up.

---

## 3. `slice8.rs` — the priority target — line by line

The brief named five specific risk areas. Each was checked by reading **and** by
experiment.

### 3.1 The tail / remainder loop — CLEAN

`run_*` folds whole 8-byte blocks and returns the leftover slice; `lib.rs` finishes it
with libcrc's own `update_crc_*`. There is exactly one copy of each finalisation rule,
shared with the unaccelerated build. Every remainder 0–7 is exercised at 601 consecutive
lengths, twice (fixed prefix and fresh content), plus 202 000 random lengths. **M6**
(dropping one tail byte) is caught by 178 417 cases, so the tail is genuinely load-bearing
and genuinely covered.

### 3.2 The crossover threshold — CLEAN, and provably not a correctness surface

`MIN_LEN = 16` for the 16/32-bit algorithms, `MIN_LEN_8 = 8` for `crc_8`. Lengths
0…600 straddle both. **M7** proves moving the threshold changes no output.

*Not verified, out of scope:* the per-length speedup table in `slice8.rs:145-151` (the
`3.000 / 0.959 / 0.878 …` figures) is a performance claim I did not reproduce. The only
timing datum I have is directional and corroborating: 3.96 s vs 8.83 s for the same
204 441-case workload.

### 3.3 Table derivation for each of the eight lanes — CLEAN

Checked algebraically and empirically.

* `slices_u8`: `slices[k]` = `T0` applied `k+2` times, so `SLICES_8[k-1] = T_k = T0^(k+1)`,
  and the fold's `T_0` term is the transcribed SHT7x table. Consistent.
* `reflected_slices_u16/u32`: `shift(v) = (v >> 8) ^ t0[v & 0xFF]` — exactly `f(v, 0)`.
* `forward_slices_u16`: `shift(v) = (v << 8) ^ t0[v >> 8]` — exactly `f(v, 0)`; `v >> 8`
  on a `u16` is always in range.
* The `u32` fold is the canonical Kounavis–Berry layout
  (`T7[x&0xFF] ^ T6[(x>>8)&0xFF] ^ T5[(x>>16)&0xFF] ^ T4[x>>24] ^ T3[b4] … ^ T0[b7]`).

Independently: **all seven byte-fold tables were dumped through the public API from both
the C library and the Rust port and compared entry by entry — 2 304 entries, identical.**
That includes `crc_tab32[]` and `crc_tab64[]`, the two tables the C library ships as
*generated source* from its `precalc/` stage and exports in `checksum.h`, versus the
port's `const fn` reconstructions.

The SHT7x table in `tables.rs` was also diffed directly against `oracle/src/crc8.c`:
**all 256 entries byte-for-byte identical.**

### 3.4 Endianness — CLEAN

`u16::from_le_bytes` for the reflected folds, `u16::from_be_bytes` for the forward
(CCITT) fold, `u32::from_le_bytes` for CRC-32. Spelled out rather than inherited from the
host, so the module is correct on a big-endian target too. **M2** (flipping CCITT to
little-endian) is caught by 201 694 cases.

### 3.5 Does `--no-default-features` really restore the original? — YES, but see C-3

Three independent confirmations:

1. The `rs_bytewise` dump is byte-identical to the C oracle across all 204 441 cases.
2. `cargo test -p libcrc-rs --no-default-features` → 60 + 7 + 14 tests, all pass.
   (`--features slice8` → 68 + 7 + 14: the 8 extra are `slice8`'s own, correctly `cfg`'d out.)
3. Linked-binary `.rdata` shrinks by **22 816 bytes** when the feature is off
   (`0x273B8 → 0x21898` in the same probe binary), and `.text` shrinks by 2 304 bytes.

The stated table cost of 23 296 B is arithmetically right
(`1792 + 4×3584 + 7168`). The 480-byte gap against the measured `.rdata` delta is linker
layout, not a missing table — noting it only because the source now quotes an exact byte
count where it used to say "23 KiB".

---

## 4. Per-algorithm read of the C source beside `lib.rs` — CLEAN

`oracle/src/*.c` read against `crates/libcrc-rs/src/lib.rs`. Polynomial, seed, reflection,
final XOR, byte-swap and loop shape agree for all thirteen, and for all eight
`update_crc_*`.

| Algorithm | C | Rust | Agrees |
|---|---|---|---|
| `crc_8` | `sht75_crc_table[(*ptr++) ^ crc]`, init `0x00` | `SHT75_CRC_TABLE[(byte ^ crc)]` | yes |
| `crc_16` / `crc_modbus` | `(crc>>8) ^ tab16[(crc ^ *ptr)&0xFF]`, init `0x0000` / `0xFFFF` | same | yes |
| `crc_ccitt_*` / `crc_xmodem` | `(crc<<8) ^ tabccitt[((crc>>8) ^ *ptr)&0xFF]`, init `0x1D0F`/`0xFFFF`/`0x0000` | same | yes |
| `crc_kermit` | reflected, poly `0x8408`, then `low\|high` byte swap | `byteswap(fold)` | yes |
| `crc_dnp` | reflected, poly `0xA6BC`, `crc = ~crc` **then** swap | `byteswap(!fold)` | yes |
| `crc_sick` | bitwise, poly `0x8005`, `short_p` starts 0 and is set to `short_c<<8` **after** the XOR | fold carrying `(crc, prev)` from `(0, 0)` | yes |
| `crc_32` | reflected, init `0xFFFF_FFFF`, `^ 0xFFFF_FFFF` at the end | same | yes |
| `crc_64_ecma` / `crc_64_we` | forward, `crc_tab64`, init `0` / all-ones, WE xors all-ones | same | yes |
| `checksum_NMEA` | skip **one** `$`, XOR until NUL/CR/LF/`*` | `strip_prefix` + `take_while` | yes |

### 4.1 C integer promotion vs Rust exact-width arithmetic — CLEAN

Every promotion site was checked and none is observable:

* `crc << 8` and `crc << 1` on a `uint16_t` promote to `int` in C, then truncate on
  assignment back to `uint16_t`. Rust's `u16 << n` discards the same bits. XOR is
  bitwise, so truncating early or late gives the same answer.
* `~crc` on a `uint16_t` complements 32 bits in C and truncates on assignment — equal to
  Rust's 16-bit `!`. This is DNP's finalisation and it matches on every case.
* `crc >> 8` on `uint16_t`/`uint32_t` is a logical shift in both (the value is always
  non-negative, and `uint32_t` does not promote).
* `(*ptr++) ^ crc` in `crc_8` promotes both operands to `int`, yielding 0…255 — the same
  index Rust computes in `u8`.
* No shift anywhere has a width ≥ the type's bit count, so Rust cannot panic on a shift
  the C accepts. `-funsigned-char` was used for every oracle build.

### 4.2 `crc_sick` first and last byte — CLEAN

C sets `short_p = 0` before the loop and assigns `short_p = short_c << 8` *after* the XOR,
so iteration *i* mixes in byte *i-1*, and the value computed after the final byte is
discarded. The Rust fold starts at `(START_SICK, 0u8)` and drops the final `prev`.
Identical. Confirmed at every length 0…600, on 202 000 random buffers, and by **M8**
(203 304 cases diverge when `prev` is forced to 0).

### 4.3 NMEA `$` and delimiter rules — CLEAN

Only **one** leading `$` is stripped, in both. Verified against the C library on 25
sentences including `""`, `"$"`, `"$$"`, `"*"`, `"$*"`, `"\r"`, `"\n"`, an embedded NUL,
`"$$GPGLL,x*7C"`, `"$ABC\rDEF"`, high-bit bytes, and two real NMEA sentences.
`checksum_NMEA(NULL, r)` and `checksum_NMEA(s, NULL)` both return NULL in both.

### 4.4 NULL input pointer — CLEAN

`crc_X(NULL, 99)` returns the **init** value, not a fault, for all twelve length-driven
functions in both the original and the shim (`0x00`, `0x0000`, `0xFFFF`, `0x0000`,
`0x0000`, `0xFFFF`, `0x1D0F`, `0x0000`, `0xFFFF`, `0x00000000`, `0`, `0`).

---

## 5. `combine.rs` — the `init ⊕ xorout` correction — CLEAN

The brief singled out MODBUS and DNP, where `init != xorout`. Both are right.

* **MODBUS**: `crc(M) = raw(M, 0xFFFF)`, `xorout = 0`, so the correction is `0xFFFF`.
  `FIX_MODBUS = START_MODBUS`. Substituting into
  `raw(A‖B, init) = L_n(raw(A,init) ⊕ init) ⊕ raw(B,init)` reproduces the implementation
  exactly.
* **DNP**: libcrc complements *then* swaps, so `xorout = 0xFFFF` and `init = 0`;
  `FIX_DNP = START_DNP ^ 0xFFFF`. The swap is undone on the way in and reapplied on the
  way out, because a byte swap is not the linear map the operator acts on. Correct.
* `len_b == 0` short-circuits to `crc_a`, which agrees with `L_0 = id`.

**Stress test beyond the in-crate suite** (which caps `len_b` at 4 096): every
`2^k - 1`, `2^k`, `2^k + 1` for k = 0…21 plus 6 291 457, and every split point of a
1 KiB buffer, for all eleven combine functions, in **both** feature configurations:

```
combine stress: 11979 checks, 0 divergences   (--features slice8)
combine stress: 11979 checks, 0 divergences   (default-features off)
max len_b tested: 6291457
```

`crc_sick` and `checksum_nmea` correctly have no `combine` — for `crc_sick` the register
at a block boundary genuinely is not a sufficient summary, because continuing needs the
last byte of `A`.

---

## 6. `libcrc-cli` — CLEAN numerically

The CLI is new, so its obligation is only that its numbers equal the C library's. They do.

* `crc --all` on a **1 048 583-byte** file: all 13 values match the C oracle exactly,
  including NMEA and `crc_sick` across 16 chunk boundaries.
* Files of 65 535 / 65 536 / 65 537 / 65 544 / 131 072 / 131 073 / 300 000 bytes: all 13
  match at every 64 KiB boundary. This is the real test of `State::Sick`'s `prev` carry
  and of the `Nmea` state machine's `started` / `done` flags.
* NMEA terminators placed deliberately at the boundary — `*` at the last byte of chunk 0,
  `*` at the first byte of chunk 1, CR at the edge, NUL at the edge, leading `$`, leading
  `$$`, `$`-plus-`*`-at-edge, and the files `""`, `"$"`, `"$$"` — all 10 match the C
  library.
* stdin (`crc --all -`) produces the same 13 values as the file path.

---

## 7. Findings

### C-1 — HIGH — `checksum_NMEA` in the shim faults where the original returns

**Where:** `crates/libcrc-cabi/src/lib.rs:161-166`.

The shim measures the string with a `strlen`-style walk to the **NUL**, then hands the
whole slice to `checksum_nmea`. The original never scans past the delimiter:

```c
while ( *ptr && *ptr != '\r' && *ptr != '\n' && *ptr != '*' ) checksum ^= *ptr++;
```

`*` is a documented terminator — and in real NMEA it is *the* terminator, because the
transmitted checksum follows it. So the shim reads memory the original provably does not.

**Exact minimal input:** the four bytes `$ A * B`, not NUL-terminated, positioned so that
the byte after `B` is unmapped.

**Reproduced** with a two-page `VirtualAlloc`, page 0 committed R/W, page 1 left
`PAGE_NOACCESS`, sentence at `page0_end - 4`:

```
===== ORACLE (original C) =====
about to call checksum_NMEA on a '*'-terminated sentence at the page edge
RETURNED OK: 41
oracle exit=0
===== PORT (Rust cabi shim) =====
about to call checksum_NMEA on a '*'-terminated sentence at the page edge
port exit=-1073741819          <- 0xC0000005 STATUS_ACCESS_VIOLATION
```

**Control**, same program, same address, only `B` replaced by `\0`:

```
===== CONTROL: NUL-terminated, PORT =====
RETURNED OK: 41
ctl exit=0
```

so the fault is isolated to the pre-scan, and the *value* is never wrong — `take_while`
still stops at `*`. This is a **memory-safety / robustness** divergence, not a numeric one.

**Caveats, stated plainly.** The C header documents the argument as a NUL-terminated
string, so a caller omitting the NUL is technically out of contract; the original merely
happens to survive it. And this is in `libcrc-cabi`, the test harness, not in the port —
`libcrc-rs::checksum_nmea` takes a slice and cannot over-read. But `libcrc.a` is the
drop-in artefact, and on this input the drop-in crashes where the original does not.

### C-2 — MEDIUM — the port's `libcrc.a` drops a symbol the original exports

**Where:** `crates/libcrc-cabi/src/lib.rs:143`, `DECISIONS.md` D-7.

D-7 correctly documents *adding* `update_crc_64_ecma` (declared in `checksum.h:99`,
defined nowhere in the original). It does not address the other half: the original **does**
export `update_crc_64`, and the port **does not**.

```
$ nm -g --defined-only oracle/lib/libcrc.a       | grep update_crc_64
0000000000f0 T update_crc_64

$ nm -g --defined-only target/release/libcrc.a   | grep update_crc_64
             T update_crc_64_ecma
```

Both archives export exactly 21 libcrc symbols; the sets differ by one element.
A caller of the only working 64-bit incremental API in the original — the symbol
`tests/parity/update_oracle.c:9` itself has to hand-declare in order to call it — fails
to link against the port:

```
--- link against ORACLE libcrc.a ---   exit=0
--- link against PORT   libcrc.a ---
ld.exe: undefined reference to `update_crc_64'
port link exit=1
```

No behavioural divergence (the function computes the same thing under either name), but
"drop-in replacement" is false for this one symbol, and the direction of the break is the
opposite of the one D-7 discusses. Exporting both names would cost one four-line function.

### C-3 — LOW — "the full test suite is run both ways" is enforced by nothing

**Where:** `crates/libcrc-rs/src/slice8.rs:88-89` ("The full test suite is run both
ways"), `crates/libcrc-rs/Cargo.toml:19` ("Both configurations are tested").

The claim is **true today** — I ran it, 60 + 7 + 14 tests pass with
`-p libcrc-rs --no-default-features` — but no automation checks it:

* `build.sh:129` runs `cargo test --quiet`, i.e. default features.
* `.github/workflows/ci.yml:82` runs `./build.sh` (so, the same default-features suite);
  its own steps are `cargo clippy --all-targets -- -D warnings` (line 105) and
  `cargo fmt --all -- --check` (line 108). Neither passes `--no-default-features`.
* `.github/workflows/no-std.yml` uses `--no-default-features` only to **build** for a
  bare-metal target and to run `cargo tree`. No tests execute there.

And the obvious command does not do what it looks like it does:

```
cargo test --workspace --no-default-features   ->  68 tests   (slice8 STILL ON)
cargo test -p libcrc-rs --no-default-features  ->  60 tests   (slice8 actually off)
```

`libcrc-cabi` and `libcrc-cli` depend on `libcrc-rs` with default features, so workspace
feature unification silently re-enables `slice8`. Anyone "verifying both configurations"
with the workspace form is testing the same build twice. The 8-test delta is the tell.

Reported as documentation/CI hygiene, not as a defect in the code.

### C-4 — INFO — the exact `.rodata` figure

`slice8.rs:82` and `Cargo.toml:15` now state **23 296 B**. The arithmetic is right
(`7×256 + 4×(7×256×2) + 7×256×4`). The measured `.rdata` delta in a linked probe binary
was **22 816 B** (`.text` also moved by 2 304 B). Section deltas are not a clean way to
measure a table, so this is not a discrepancy to fix — recorded only because the number
is now quoted to the byte.

---

## 8. What was checked and found clean — explicit list

So that the absence of findings is legible rather than assumed:

- [x] slice-by-8 == byte-at-a-time, every length 0…600, twice, plus degenerate runs
- [x] slice-by-8 == byte-at-a-time on buffers up to 1 048 583 B
- [x] both == the real C library, 204 441 cases, 2 452 995 values
- [x] the harness can fail (8/8 real defects caught; 1/1 no-op control correctly silent)
- [x] all seven byte-fold tables, 2 304 entries, C vs Rust — identical
- [x] the SHT7x table vs `oracle/src/crc8.c` — 256/256 identical
- [x] `crc_tab32[]` / `crc_tab64[]` (C's `precalc`-generated source) vs the port's `const fn` — identical
- [x] polynomial, seed, reflection, final XOR, byte-swap, loop shape — 13/13
- [x] all 8 `update_crc_*` — including the incremental replay at every length 0…600
- [x] C integer promotion, shift width, signedness — no observable difference
- [x] `crc_sick` previous-byte handling at the first and last byte
- [x] NMEA `$`-prefix and NUL/CR/LF/`*` delimiter rules, 25 sentences + 10 boundary cases
- [x] NULL-pointer contracts, all 12 length-driven functions + both NMEA arguments
- [x] combine `init ⊕ xorout`, derived for all 11 and stress-tested to `len_b` = 6 291 457
- [x] the CLI's streaming state across 64 KiB chunk boundaries, 13/13 algorithms
- [x] `cargo clippy --all-targets -- -D warnings` clean in **both** feature configurations

Explicitly **not** verified (out of scope for this review): the per-length speedup table
in `slice8.rs:145-151`, and the numbers in `bench/`.
