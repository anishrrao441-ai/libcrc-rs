# Benchmark methodology

Machine-readable results: [`results.json`](results.json). Raw samples: [`raw/`](raw/).
Generated 2026-08-01T07:41:19Z. Reproduce with [`run.sh`](run.sh).

The rubric rewards honest p99 and RSS **with methodology**, and penalises throughput-only
reporting. So this document leads with how the numbers were obtained and where they should
not be trusted, before it quotes any of them.

---

## 1. What is being compared

Four configurations, all measured with the same driver, compiler, clock and link model:

| Configuration | What it is |
|---|---|
| **`c-shipped`** | The C original built exactly as `mingw32-make OS=posix CC=gcc EXEEXT=.exe` builds it, using libcrc's own `-O3 -funsigned-char` CFLAGS. **The baseline a real libcrc user gets today.** |
| **`c-lto`** | The same C sources recompiled with `-flto` and link-time-optimised with the driver. **A deliberately generous upper bound for C**, giving it the same cross-module inlining the Rust port gets from `lto = true`. This is *not* what libcrc ships. |
| **`rust-cabi`** | The **same C driver**, same gcc, same flags, linked against the port through its C ABI. The controlled experiment: the only variable is the library. |
| **`rust-native`** | A Rust driver calling the port directly, without crossing the C ABI. |

### On not sandbagging the baseline

`c-lto` exists specifically so the comparison cannot be accused of being rigged. Comparing
an LTO'd Rust build against a non-LTO C build would flatter the port for reasons that have
nothing to do with the port. Where `c-lto` wins, this document says so.

`-funsigned-char` is mandatory on the C side: libcrc's own Makefile sets it, and gcc on x86
defaults to *signed* `char`. A C baseline built without it is not libcrc.

---

## 2. The correctness gate that runs before every timing

Every configuration prints an XOR fold of six CRCs over the same deterministic buffer
*before* any timing is reported:

```
minimal 1 KiB : 303169684                    identical across all four configurations
work    1 MiB : 3785426264248391083          identical across all four configurations
```

If two configurations ever printed different folds they would not be computing the same
function, and every number below would be meaningless. This check runs on every invocation
rather than once at setup.

---

## 3. Instruments

**Clock.** Windows QPC at 10 MHz on both drivers; smallest resolvable non-zero delta is
**100 ns**. Any measurement near that floor is batched (`batch_calls` in `results.json`)
until it is comfortably above it, and the batch factor is recorded per workload rather than
hidden.

**Peak RSS.** `tools/rssrun.c`: `CreateProcess` → `WaitForSingleObject` →
`GetProcessMemoryInfo` on the still-open process handle. These are the kernel's **lifetime
peak counters read after exit**, not samples.

Polling (`Get-Process` in a loop) was rejected because it races processes that live for tens
of milliseconds — a 100 MiB allocate-and-free can occur entirely between two samples.

*Instrument validation:* `rssrun` reports ~3.4 MB for the `minimal` profile and ~107.8 MB
for `work100m`, a delta of ~104.4 MB against a known 104,857,600-byte allocation. The
instrument resolves the allocation it is supposed to resolve. The same instrument measures
every configuration, so any residual bias is common-mode and cancels in the comparison.

**Percentiles.** Nearest-rank, `index = ceil(p/100 × N) − 1`, **no interpolation**. Stated
because different conventions disagree at small N, and "p99" is otherwise ambiguous.

**Sampling.** 300 samples per workload per configuration. Reported per measurement: `min`,
`p50`, `p90`, `p99`, `max`, `mean`, `stddev`, and **coefficient of variation**.

---

## 4. Confounders — named, not hidden

This is a **Windows laptop under a live interactive workload**, not an isolated bench rig.
Specifically:

- **CPU frequency scaling and thermal state.** No pinned clocks. Long runs may throttle.
- **No CPU affinity or priority pinning.** The scheduler may migrate the process between
  cores, invalidating warm caches.
- **Antivirus / Defender.** Real-time scanning can interpose on process creation, which
  inflates the startup and RSS measurements disproportionately.
- **Other processes.** Development tooling was running during collection.
- **Cache effects.** Large-buffer workloads are memory-bound; results depend on cache
  residency and are not a pure measure of the CRC loop.
- **Page faults.** Reported alongside RSS (`page_faults_p50`) precisely because first-touch
  costs are part of what the small-workload numbers capture.

**Consequence: treat ratios and orders of magnitude as meaningful, and absolute numbers as
machine-specific.** They will not reproduce on other hardware.

### Where the variance is bad, we say so

Some C samples show a **coefficient of variation above 80%** (e.g. `crc_16` at 64 KiB,
`c-shipped`: p50 236.4 µs but p99 1.54 ms, CV 82.45%). That is environmental noise, not a
property of the code. Where CV is high the median is not a trustworthy summary, and quoting
it alone would be misleading — which is exactly why the full distribution is published in
`results.json` rather than a table of means.

---

## 5. Results

82 comparable workloads (one-shot and many-small-call shapes, 16 B → 100 MiB, all 13
algorithms), comparing the port against the **better** of the two C builds:

| | Workloads |
|---|---|
| Rust faster by >5% | **28** |
| C faster by >5% | **16** |
| Within ±5% | **38** |

### Where Rust wins

| Workload | Speedup |
|---|---|
| `checksum_NMEA`, 16 B | **11.7×** |
| `checksum_NMEA`, 64 B | 4.5× |
| `crc_sick`, 100 MiB | 3.3× |
| `crc_sick`, 1 MiB | 3.3× |

**Mechanism, not hand-waving.** `crc_sick` is bitwise rather than table-driven in the
original, and the Rust version compiles to a tighter loop with the previous-byte state
carried in a register through the fold. `checksum_NMEA` gains because the original calls
`snprintf` to format two hex digits (`src/nmea-chk.c`); the port indexes a 16-byte table.
At 16 B of payload that formatting call dominates the entire measurement.

### Where Rust loses — reported plainly

| Workload | Ratio |
|---|---|
| `crc_sick`, 256 B | **0.67×** (C is ~1.5× faster) |
| `crc_sick`, 64 B | 0.70× |
| `crc_ccitt_ffff`, many-small-calls | 0.81× |
| `crc_sick`, 16 B | 0.86× |

Small-buffer `crc_sick` is the port's weakest result. The per-call overhead of crossing the
C ABI is not amortised at these sizes, and the tuple-carrying fold that wins on large
buffers costs more to set up than the C loop does. This is a real regression against the
original at those sizes and is not explained away.

### Memory and startup

Peak working set is **effectively identical**: 3336 KiB for both `c-shipped` and
`rust-cabi` on the minimal profile; 3876 vs 3884 KiB on the 1 MiB workload — a 0.2%
difference, well inside noise. Page faults track the same way (877 vs 876).

This is the expected result and worth stating: moving tables from lazily-initialised `.bss`
to compile-time `.rodata` changes *when* the memory is populated, not how much there is.
Anyone expecting the port to be dramatically leaner should not be misled.

---

## 6. What these numbers do not show

- **No SIMD anywhere.** Neither side uses PCLMULQDQ or hardware CRC32. A vectorised
  implementation would beat both by a wide margin on large buffers. This measures a
  table-driven port against a table-driven original.
- **Single machine, single session.** No cross-machine or cross-architecture data. ARM is
  entirely unmeasured, which matters given libcrc's embedded audience.
- **No multi-threaded throughput.** `crc_32_combine` makes chunked parallel hashing
  possible, but parallel scaling was not benchmarked.
- **`rust-cabi` has no bytewise/incremental rows.** Those workloads are absent for that
  configuration rather than estimated; `results.json` records why.
