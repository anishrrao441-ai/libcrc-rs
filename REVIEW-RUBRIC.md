# REVIEW-RUBRIC.md — independent adversarial judge review

Reviewer stance: assume a fatal flaw exists and find it. Every verdict below is backed by a
command that was actually run in this session. Where I could not verify something, I say so
rather than guessing.

**Tree reviewed:** `3184636` (`docs: record both upstream bug reports…`), 30 commits.
**Caveat, stated up front:** the working tree was **dirty** during this review — concurrent
workflows held uncommitted edits to `crates/libcrc-rs/Cargo.toml`, `src/lib.rs`,
`src/slice8.rs` and an untracked `.github/workflows/no-std.yml`. Local gate results below
(tests/clippy/fmt) are for that dirty tree. The **clean-clone** result is for pushed
`9f25b3a`, which was HEAD when the clone was taken.

---

## PART 1 — DISQUALIFICATION CHECKS

| # | Check | Verdict |
|---|---|---|
| (a) | First commit inside the window | **PASS** |
| (b) | History genuinely incremental | **PASS** |
| (c) | `tests/original/` unmodified | **PASS** |
| (d) | Nothing links/calls the original C library | **PASS** |
| (e) | Fresh GitHub clone builds green | **PASS** |

**No disqualification condition is met.** Detail:

**(a) First commit — PASS.** `8940f8d` at `2026-08-01T12:41:52+05:30` = **`2026-08-01T07:11:52Z`**,
which is 13.2 h *after* the `2026-07-31T18:00:00Z` start. Clear.

> ⚠ **The runbook's own command is wrong and gives the wrong answer.**
> `git log --reverse -1 --format=%cI` applies `-1` *before* `--reverse`, so it prints the
> **last** commit, not the first. It returned `2026-08-01T23:07:04+05:30`. The correct
> invocation is `git log --reverse --format=%cI | head -1`. Both pass here, but anyone
> auditing with the documented command is reading the wrong commit.

**(b) Incremental — PASS.** 30 commits over ~22 h, in a believable order: scaffold → 3
algorithms → all 13 → C ABI → full API → combine/digest → concurrency → fuzz → docs/CI →
hardening → CLI → slice-by-8. No "initial dump": the first commit is repo scaffolding only,
and the algorithms land across `549b6a0`/`65ce121`/`6a237c5`. Commit messages describe real
increments, including three self-corrections (`c60b0d7`, `5083199`, `82518d2`) that a
fabricated history would not contain.

**(c) Tests unmodified — PASS.** `sha256sum -c tests/original.sha256` → 4/4 `OK`, exit 0.
`.gitattributes` marks `tests/original/**` and the manifest `-text`, which is the correct fix
for the CRLF class.

**(d) No C-library dependency — PASS, inspected at the binary level.**
`nm --defined-only target/release/libcrc.a` shows all 21 libcrc symbols as **defined** (`T`)
— `crc_8 crc_16 crc_32 crc_64_ecma crc_64_we crc_ccitt_1d0f crc_ccitt_ffff crc_dnp crc_kermit
crc_modbus crc_sick crc_xmodem checksum_NMEA update_crc_8 update_crc_16 update_crc_32
update_crc_ccitt update_crc_dnp update_crc_kermit update_crc_sick update_crc_64_ecma`.
There is **no undefined reference to any libcrc symbol** — the archive is self-contained.
The only other non-Rust symbols are `compiler_builtins` and Win32 (`ProcessPrng`,
`WaitOnAddress`). `oracle/` is gitignored and appears in no crate's dependency graph.

**(e) Fresh clone — PASS.** Cloned `https://github.com/anishrrao441-ai/libcrc-rs` into a temp
dir and ran `./build.sh` there: **exit 0**, original suite `**** All tests succeeded`,
`crc.exe` and `libcrc.a` both produced. The CRLF-class bug is genuinely fixed.

### 🔴 F-1 (HIGH) — a *new* bug of exactly the class you asked me to hunt

`./build.sh` **failed in my hands on a clean, unmodified tree**, printing:

```
==> Verifying the original test suite is unmodified
      0 [main] sha256sum (3368) child_copy: cygheap read copy failed, ... Win32 error 6
   1194 [main] sha256sum 657 ...sha256sum.exe: *** fatal error - couldn't create signal pipe, Win32 error 5
FAIL: tests/original/ has been MODIFIED — refusing to build
```

Nothing was modified. This was a **transient msys2/cygwin `fork()` failure** in `sha256sum`
— 5/5 immediate re-runs returned OK(0), and the next full `./build.sh` exited 0.

The defect is in `build.sh`:

```sh
$SHA256C "$MANIFEST" >/dev/null \
    || fail "tests/original/ has been MODIFIED — refusing to build"
```

Any non-zero exit — hash mismatch, missing file, **or the checker crashing** — is reported
as **tampering**. This is worse than the CRLF bug it replaced, because it is
**nondeterministic**: it can fire on any run, on any machine, including a judge's. A judge
who sees "tests/original/ has been MODIFIED" on a submission whose entire thesis is
"the tests are provably untouched" may stop reading. This is the single highest-risk item
in the repository.

*Fix (~15 min):* capture the checker's output and exit code; treat exit 1 with a `FAILED`
line as tampering, and anything else as a tool error with a distinct message — e.g. retry
once, then `fail "sha256 checker did not run (tool error, NOT a hash mismatch): <output>"`.

---

## PART 2 — SCORE AS A JUDGE

| Criterion | Weight | Score | Evidence |
|---|---|---|---|
| Functionality | 40% | **36 / 40** | see below |
| Behavioral Equivalence | 30% | **28 / 30** | see below |
| Code Quality | 20% | **17 / 20** | see below |
| Innovation | 10% | **8 / 10** | see below |
| **Base** | | **89 / 100** | |
| Fuzz Survivor | +5 | **+5** | `fuzz/log.txt` + negative control + replayable seed |
| Zero Unsafe | +5 | **+5** | `#![forbid(unsafe_code)]`, grep-verified |
| Bug Catcher | +3 | **+3** | upstream #26/#27 confirmed live via GitHub API |
| Decision Log | +3 | **+3** | 13 entries, counted |
| **Total** | | **105** | |

### Functionality — 36/40
**For:** all 21 public symbols exported and defined; all 13 algorithms produce the correct
check value (I ran `printf '123456789' | crc --all -` and got all 13 matching ground truth
exactly, including `kermit 0x8921`, `dnp 0x82EA`, `sick 0x56A6`); the unmodified original
suite compiles and passes; `no_std`; streaming digests; `core::hash::Hasher`; **11** combine
functions; a working `crc` CLI with `--check` manifests; `--no-default-features` restores
byte-at-a-time behaviour. `cargo test --workspace` = **138 pass** (0+28+21+68+7+14).

**Missing/weak:**
- **The CLI is invisible.** `build.sh` *does* emit `target/release/crc.exe` (confirmed in the
  clean clone) but never mentions it, and **no document mentions the CLI at all**. The crate
  that exists specifically to close the brief's "one-step build → binary" requirement cannot
  be found by a judge reading the README. This is a self-inflicted scoring loss.
- **The parity harness is not wired into anything.** `tests/parity/` is the strongest
  equivalence evidence in the repo, but neither `build.sh` nor CI runs it, and no doc says how.
  A judge cannot reproduce the 117M-comparison claim without reverse-engineering it.

### Behavioral Equivalence — 28/30
**For, and this is the submission's best work:**
- I **re-ran the parity harness myself**: `cargo run --release --example update_parity` and
  diffed against `tests/parity/expected_digests.txt` → **all 8 digests byte-identical**.
- Case count computed from the loop bounds: `65,536 + 7 × 16,777,216` = **117,506,048**
  comparisons. The "~117M" claim is exact, and "5 of 8 EXHAUSTIVE" is exact
  (`update_crc_8/16/ccitt/kermit/dnp`).
- 1.1M fuzz cases × 25 values = 27.5M comparisons, 0 divergences, replayable seed — plus a
  **negative control that produced 367 divergences**, which is what makes "zero" mean anything.

**Missing/weak:** single platform (x86-64 windows-gnu), single-threaded, and `crc_sick` has no
external reference implementation — all three are disclosed honestly in `fuzz/log.txt`, which
is why the deduction is small. `update_crc_sick/32/64` are sampled, not exhaustive.

### Code Quality — 17/20
**For:** zero `unsafe` in the port *and* in the CLI (both `forbid(unsafe_code)`); the 4 unsafe
blocks are quarantined in the shim, each with a `// SAFETY:` comment; `cargo clippy
--all-targets -- -D warnings` **exit 0**; `cargo fmt --all -- --check` **exit 0**; clean module
split (`tables`/`slice8`/`combine`/`digest`); `slice8.rs` documents its own memory cost
(1792 + 3584×4 + 7168 = **23,296 B**, which matches the crate docs exactly).

**Missing/weak:** documentation/code drift (Part 3) — including a mis-numbered SAFETY comment
in the very file the "zero unsafe" bonus rests on.

### Innovation — 8/10
**For:** slice-by-8 bulk folding implemented **entirely in safe Rust** under
`forbid(unsafe_code)` (the interesting result: you can get multi-table ILP without
intrinsics); GF(2) matrix-exponentiation combine generalised to 11 algorithms; `const fn`
tables that delete an entire upstream build stage *and* a data race; a negative-controlled
differential fuzzer; a concurrency soak that found a 26-year-old bug.

**Missing/weak:** **the two newest and most innovative pieces — slice-by-8 and the CLI — are
mentioned in zero documents.** The innovation exists in the code but not where it is scored.

---

## PART 3 — ANTI-FABRICATION AUDIT

### VERIFIED — every one of these I reproduced

| Claim | Verdict | How |
|---|---|---|
| 1,100,000 cases · 75.945 s · 0 divergences · seed `0xE86F885A2BCDBFC5` · 27.5M comparisons | **VERIFIED** | present and self-consistent in `fuzz/log.txt` |
| 367 divergences in the negative control | **VERIFIED** | `grep -m1 DIVERGENCES fuzz/negative-control.log` → `367` |
| ~117M parity comparisons | **VERIFIED** | computed **117,506,048** from loop bounds |
| Parity digests reproduce | **VERIFIED** | re-ran `update_parity`; **all 8 digests identical** |
| 5 of 8 functions exhaustive | **VERIFIED** | loop bounds in `update_parity.rs` / `update_oracle.c` |
| 30 of 40 processes, worst 3, 85 total inits | **VERIFIED** | `tests/concurrency/RESULTS.md`, internally consistent |
| 82 workloads | **VERIFIED** | `results.json.workloads.length` = **82** |
| 13 DECISIONS entries | **VERIFIED** | `grep -cE '^#+ *D-[0-9]'` = **13** (D-1…D-13) |
| 21 public symbols | **VERIFIED** | `nm` on the built `libcrc.a`, counted 21 |
| 138 tests | **VERIFIED** | `cargo test --workspace` |
| 0 unsafe in port, 4 in shim, 0 in CLI | **VERIFIED** | grep for `unsafe {`/`unsafe fn`/`unsafe impl`/`unsafe trait` |
| All 13 golden check values | **VERIFIED** | `crc --all` output matches ground truth exactly |
| slice8 costs 23,296 B `.rodata` | **VERIFIED** | table arithmetic matches the documented figure |
| `checksum.h:99` declares `update_crc_64_ecma`, no definition exists | **VERIFIED** | only `update_crc_64` at `crc64.c:103` |
| `crc16.c:40/41/58/86/109` (race citations) | **VERIFIED** | all five lines are exactly as quoted |
| `crc16.c:63` = `if ( ptr != NULL )` | **VERIFIED** | exact |
| `crcdnp.c:99` is a comment, not code | **VERIFIED** | exact |
| `lib.rs:11-13` is the byte-swap table (DEMO 1:45) | **VERIFIED** | exact |
| Upstream issues #25 / #26 / #27 | **VERIFIED** | GitHub API: #26 and #27 created `2026-08-02T05:19Z`, open |
| CI green | **VERIFIED** | `gh run list`: 5 consecutive successes |
| Fresh clone `./build.sh` exit 0 | **VERIFIED** | ran it |
| 8 quoted bench figures | **VERIFIED as numbers** | all 8 match `rust-native_over_c-lto` to 3 dp |

**The C-side citations are the strongest part of the audit: every single one is exact.**
That is unusual and it should be said plainly.

### WRONG / STALE / CONTRADICTED — 13 findings

> ⚠ The pattern you predicted is real and worse than expected: **`README.md`, `UNSAFE.md` and
> `DEMO.md` all predate the CLI and slice-by-8, and none was updated.** Three of these
> findings make the submission look *smaller* than it is; two would fail live on camera.

**F-2 (HIGH) — README misattributes the benchmark baseline. Contradicts `methodology.md`.**
README: *"82 workloads … comparing this port against the C original built with its own
`-O3 -funsigned-char` flags"* — that is the **`c-shipped`** configuration — then quotes
**28 / 16 / 38**.
Measured from `results.json`:

| Basis | Rust >5% | C >5% | within 5% |
|---|---|---|---|
| `rust-native_over_c-shipped` | 20 | 15 | 47 |
| **`rust-native_over_c-lto`** | **28** | **16** | **38** ← the quoted row |

`bench/methodology.md:109` gets it **right**: *"comparing the port against the **better** of
the two C builds."* So **README contradicts methodology.md**, and `DEMO.md:39` repeats the
README's wrong wording on camera. All eight individual figures (11.7×, 4.5×, 3.3×, 3.3×,
0.67×, 0.70×, 0.81×, 0.86×) are `c-lto` too — so the *numbers* are sound and the *label* is
wrong. Verdict: **WRONG (mislabelled baseline)**. Note this also hides that the team used the
*harder* baseline — the honest framing is better than the one printed.

**F-3 (HIGH) — `UNSAFE.md` §1 publishes a terminal transcript that no longer reproduces.**
It shows `grep -rn "unsafe" crates/libcrc-rs/src/` returning **2 lines**, with
`lib.rs:31:#![forbid(unsafe_code)]`. Actual output today is **5 lines**, and the attribute is
at **`lib.rs:47`**:
```
lib.rs:1  lib.rs:37  lib.rs:47(#![forbid])  slice8.rs:17  slice8.rs:18
```
The document that says *"Please run the commands rather than trust the prose"* fails its own
command. Verdict: **STALE / WRONG**.

**F-4 (HIGH) — `DEMO.md` 4:15 will visibly fail on video.** The script says to run
`grep -rn "unsafe" crates/libcrc-rs/src/` and narrate *"Two hits: a doc comment, and the
attribute that bans it."* Five hits appear on screen. Recording this as written puts a
visible contradiction in the demo. Verdict: **STALE**.

**F-5 (HIGH) — README Limitations understates the work.** *"**`crc_32_combine` is CRC-32
only.** The construction generalises to the other widths; correctness on the 13 required
algorithms came first."* There are **11** combine functions in `combine.rs` (`crc_8`,
`crc_16`, `crc_modbus`, `crc_kermit`, `crc_dnp`, `crc_xmodem`, `crc_ccitt_ffff`,
`crc_ccitt_1d0f`, `crc_32`, `crc_64_ecma`, `crc_64_we`). Verdict: **STALE** — and it is in
the *Limitations* section, so a judge reads a completed feature as an admitted gap.

**F-6 (HIGH) — `DECISIONS.md` D-8 repeats the same stale limitation.** *"**Tradeoffs.**
Currently CRC-32 only."* Same contradiction, in the decision log that carries a +3 bonus.
Verdict: **STALE**.

**F-7 (HIGH) — slice-by-8 and the CLI appear in NO document.**
`grep -niE "slice.?by.?8|slice8|libcrc-cli|crc binary" README.md DECISIONS.md UNSAFE.md DEMO.md`
returns **nothing**. Consequences: the README `Layout` block omits `crates/libcrc-cli/`;
`UNSAFE.md` §3's per-crate table omits `libcrc-cli` (which is `forbid(unsafe_code)`, 0 blocks
— free credit left on the table); there is no DECISIONS entry for either. The two newest
features are undiscoverable. Verdict: **STALE / MISSING**.

**F-8 (MEDIUM) — the README's headline transcript is not current output.**
README shows `ok target/release/libcrc.a (4059142 bytes)` under the caption *"That is real
output, not an illustration. Run `./build.sh` and you get it."* Measured: **4,084,860** bytes
(clean clone) and **4,084,764** bytes (local) — the size is also **not stable between runs**.
The transcript additionally omits two steps `build.sh` actually prints (`==> Checking
toolchain`, `==> Determining the native libraries this target needs`) and drops the
`(via sha256sum -c)` suffix. The +25,718 B delta is slice-by-8's 23 KiB of tables, confirming
the transcript predates that commit. Verdict: **STALE**. *Quoting an exact byte count that
varies run-to-run is a bad idea regardless — drop the number.*

**F-9 (MEDIUM) — `tests/concurrency/RESULTS.md` cites a file that is not in the repo.**
Its reproduction line is
`gcc -O2 -funsigned-char -Iinclude soak.c crc16_instrumented.c -o soak.exe`, but
`crc16_instrumented.c` **does not exist anywhere in the repository** (`tests/concurrency/`
contains only `RESULTS.md` and `soak.c`). The Bug Catcher evidence therefore **cannot be
reproduced by a judge**. Verdict: **UNSUPPORTED (as reproducible)** — the *finding* is
corroborated by upstream issue #26 and by the verified C-source citations, but the stated
command cannot be run.

**F-10 (MEDIUM) — SAFETY comments cite the wrong `UNSAFE.md` entries.**
| `libcrc-cabi/src/lib.rs` | says | should say |
|---|---|---|
| :37 | U-1 | U-1 ✓ |
| :158 | U-2 | U-2 ✓ |
| **:165** | **U-1** | **U-3** |
| **:176** | **U-3** | **U-4** |
Two of four are wrong, in the exact file the "Zero Unsafe +5" bonus is argued from. A judge
cross-referencing `UNSAFE.md` against the code hits an inconsistency immediately. Verdict:
**WRONG**. (Cheapest high-value fix in the repo — 2 words.)

**F-11 (LOW) — README undercounts the sampled parity functions.** Limitations says
*"`update_crc_32/64` parity is sampled"*; **`update_crc_sick` is also sampled**
(`crc += 257` stride). Three functions are sampled, not two — consistent with the same
document's own "5 of 8 exhaustive". Verdict: **WRONG (incomplete)**.

**F-12 (LOW) — DEMO timing is optimistic.** DEMO claims `./build.sh` warm = **33.8 s** / "~34 s".
Measured warm on the current tree: **36.7 s**. Verdict: **STALE** (slice8 + CLI added build
work). Update the number or say "~37 s".

**F-13 (LOW) — README's `cargo geiger` and Docker honesty notes.** Both are stated as
*not* done locally with reasons given. I could not independently verify the claimed
`cargo install cargo-geiger` network timeout or the absence of a container runtime, but both
are **disclosed as limitations rather than asserted as results**, which is the correct
behaviour. Verdict: **UNVERIFIED, correctly disclosed** — no action needed.

### Contradictions between documents — summary

1. `README.md` (bench baseline = "its own -O3 flags") **vs** `bench/methodology.md:109`
   ("the better of the two C builds"). `DEMO.md:39` sides with the wrong one.
2. `README.md` Limitations + `DECISIONS.md` D-8 ("CRC-32 only") **vs** `combine.rs`
   (11 functions).
3. `UNSAFE.md` §1 transcript + `DEMO.md` 4:15 ("two hits") **vs** actual grep (5 hits).
4. `UNSAFE.md` U-3/U-4 numbering **vs** the SAFETY comments in `libcrc-cabi/src/lib.rs`.
5. `README.md` Layout **vs** the actual crate list (`libcrc-cli` missing).

---

## RANKED NEXT ACTIONS — one remaining session, highest points-per-hour first

| # | Action | Time | Why it pays |
|---|---|---|---|
| **1** | **Fix `build.sh` F-1**: distinguish "hash mismatch" from "checker failed to run"; retry once; print the checker's real output. | 15 min | **Removes the only failure mode that can make an honest submission look like tampering.** Nondeterministic, so it can hit the judge. Highest risk-reduction per minute in the repo. |
| **2** | **Fix F-5 + F-6**: replace "crc_32_combine is CRC-32 only" in README Limitations and DECISIONS D-8 with the true "11 of 13 (all but `crc_sick` and NMEA)". | 10 min | Two edits convert a self-declared *limitation* into a delivered *feature*. Directly moves Functionality and Innovation. |
| **3** | **Fix F-3 + F-4 + F-10**: paste the real 5-line grep into `UNSAFE.md`, correct `lib.rs:31`→`:47`, fix the DEMO narration to "five hits — three doc comments and the attribute that bans it", fix the two SAFETY comment numbers. | 20 min | Protects the **+5 Zero Unsafe** bonus and stops the demo contradicting itself on camera. |
| **4** | **Fix F-2**: change README (and DEMO 4:35) to "against the **better** of two C builds, including an LTO baseline we built to avoid sandbagging". | 10 min | Turns a mislabel into a *strength* — you used the harder baseline. Protects the anti-fabrication story, which is this submission's whole identity. |
| **5** | **Fix F-7**: add a README section + Layout line for the `crc` CLI (with `crc --all` output) and for slice-by-8 (23,296 B, default-on, `--no-default-features` restores original behaviour); add DECISIONS D-14/D-15. | 45 min | The brief's "one-step build → binary" is currently **undiscoverable**. Largest pure point *gain* available; ranked below the cheap fixes only because it costs more time. |
| **6** | **Fix F-9**: commit `crc16_instrumented.c` (or rewrite the repro line to what actually works). | 20 min | Makes the **+3 Bug Catcher** evidence reproducible instead of merely asserted. |
| **7** | **Fix F-8**: delete the byte count from the README transcript and re-paste current `build.sh` output verbatim. | 10 min | The caption promises "run it and you get this". Cheap, and it is the first thing a judge sees. |
| **8** | **Wire parity into `build.sh` or CI** (a `--parity` flag that runs `update_parity` and diffs `expected_digests.txt` — I verified this takes **11.8 s**). | 30 min | Converts the strongest equivalence evidence from "documented" to "reproducible in one command". |
| **9** | **Fix F-11 + F-12**: add `update_crc_sick` to the sampled list; update DEMO timing to ~37 s. | 5 min | Small, but this submission is scored on precision. |
| **10** | Add `libcrc-cli` (0 blocks) to the `UNSAFE.md` §3 crate table. | 5 min | Free credit for a third `forbid(unsafe_code)` crate. |

**If only one hour is available: do 1, 2, 3, 4, 7, 9, 10** — that is ~75 min of edits, touches
no code paths, cannot break green, and clears every HIGH finding except F-7.

**Do not touch** the fuzz logs, `tests/original/`, `tests/parity/expected_digests.txt`, or
`bench/results.json` — all four were audited and are sound.
