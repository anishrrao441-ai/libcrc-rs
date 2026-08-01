# 5-minute demo video — shot script

Everything below was timed on the author's machine. **`./build.sh` takes ~34 s even warm**, so
the script is built around that rather than discovering it on camera.

## Pre-flight (do this BEFORE recording)

- [ ] `cargo build --release` once, so nothing compiles from scratch on camera
- [ ] `./build.sh` once — confirms green and warms the cache (the take itself will still take ~34 s)
- [ ] Terminal font ≥ 18 pt, window ~110 columns, clear scrollback (`clear`)
- [ ] Close Slack/Discord/mail; disable notifications
- [ ] `cd C:/Users/pc/Desktop/portmortem-2026`
- [ ] Have the GitHub repo open in a browser tab: https://github.com/anishrrao441-ai/libcrc-rs
- [ ] Have `fuzz/log.txt` and `tests/concurrency/RESULTS.md` open in an editor tab

**Recording tip for the 34 s build:** either keep talking over it (there is exactly enough to say —
see 1:05), or cut and rejoin on the result. Do **not** sit in silence.

---

## The script

| Time | On screen | Command | Say |
|---|---|---|---|
| **0:00** | README top | *(scroll)* | "libcrc is a 26-year-old C checksum library. I ported it to Rust. The claim I want to prove is that the original's own test suite — unmodified — passes against my Rust code." |
| **0:20** | `tests/original/` | `ls tests/original/` | "These are the original C test files, copied verbatim. I never edited them." |
| **0:35** | hash check *(~0.1 s)* | `sha256sum -c tests/original.sha256` | "Hashed at import. Four OKs — that's proof, not a promise. If I'd touched a byte this fails." |
| **0:50** | *(start build)* | `./build.sh` | "One command. It re-checks those hashes first, builds the Rust port, then compiles the untouched C tests and links them against Rust." |
| **1:05** | build scrolling *(~34 s)* | *(talk over it)* | **Fill the time:** "Nothing here links the original C library — the dependency runs C-tests → Rust. The port itself is `forbid(unsafe_code)`; the only unsafe in the repo is four blocks in a shim that exists purely so these C tests can link." |
| **1:30** | **`**** All tests succeeded`** | *(let it sit 3 s)* | "That's the original suite. Passing. Against Rust." ← **THE MONEY SHOT — do not rush past it** |
| **1:45** | `crates/libcrc-rs/src/lib.rs` line 11-13 | *(scroll to the byte-swap table)* | "Here's why this port had to exist. libcrc byte-swaps three algorithms relative to the standard CRC catalogue. Kermit gives 0x8921 where every other library gives 0x2189." |
| **2:10** | upstream issue #25 in browser | *(open the tab)* | "This isn't theoretical — a real user filed this in 2024, confused by exactly that. Every general-purpose Rust CRC crate is catalogue-conformant, so none of them is a drop-in for libcrc. I couldn't have wrapped one." |
| **2:30** | the regression test | `cargo test documented_catalogue -- --nocapture` | "So I preserved the divergence and pinned it with a test, so nobody 'fixes' it later and silently breaks compatibility." |
| **2:50** | `fuzz/log.txt` | `head -20 fuzz/log.txt` | "76 seconds of continuous differential fuzzing against the real C library. 1.1 million cases. Zero divergences — and the seed is recorded, so you can replay it exactly." |
| **3:10** | `fuzz/negative-control.log` | `grep -m1 DIVERGENCES fuzz/negative-control.log` | "And here's the control: I deliberately corrupted the oracle and the harness caught 367 divergences. That's what makes 'zero' mean something." |
| **3:30** | `tests/concurrency/RESULTS.md` | *(scroll to the table)* | "Then I went looking for bugs in the original. libcrc initialises its lookup tables lazily behind a plain bool, with no synchronisation anywhere in the library. Sixteen threads, cold start: the 'run once' initialiser ran **more than once in 30 of 40 runs**." |
| **3:55** | *(same file, the honesty paragraph)* | *(scroll)* | "I saw zero wrong checksums on x86 — and I say so. It's still undefined behaviour under C11, and on ARM, which is libcrc's actual audience, it can return a wrong checksum. My port can't have that bug: the tables are built at compile time, so there's no initialiser to race." |
| **4:15** | unsafe proof | `grep -rn "unsafe" crates/libcrc-rs/src/` | "Two hits: a doc comment, and the attribute that bans it. Zero unsafe in the port, enforced by the compiler." |
| **4:35** | benchmark summary | `head -30 bench/methodology.md` | "82 benchmark workloads against the C original built with its own -O3 flags. Rust wins 28. **C wins 16.** I'm reporting both — small-buffer crc_sick is 0.67×, and pretending otherwise would be the fastest way to lose your trust." |
| **4:55** | README / repo | *(scroll to evidence table)* | "Everything's reproducible: one command, hashes, a seeded fuzz log, and a decision log with 13 entries. Thanks for watching." |

**Total ≈ 5:00.**

---

## If you overrun — cut in this order

1. **2:30** the regression-test run *(the table at 1:45 already makes the point)*
2. **3:10** the negative control *(painful to lose — it's what makes "zero divergences" credible)*
3. **4:35** benchmarks *(but the honesty beat is genuinely worth points)*

**Never cut:** 0:35 hash check · 0:50→1:30 the build and its result · 1:45 the byte-swap argument.

## Verified command timings

| Command | Time |
|---|---|
| `sha256sum -c tests/original.sha256` | 0.14 s |
| `./build.sh` (warm) | **33.8 s** |
| `./build/testall` alone | 0.12 s |

**Fallback if the build is too slow for your edit:** run `./build.sh` before recording, then on camera
run `./build/testall` alone (0.12 s) — but say out loud that you built it a moment ago. Do not imply a
fresh build happened when it didn't; the whole submission is built on being straight about the evidence.

## The one-line pitch, if you need it

> "The original library's own test suite, unmodified and hash-verified, passes against my Rust port —
> and along the way I found a 26-year-old data race in the original."
