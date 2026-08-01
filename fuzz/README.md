# Differential fuzz harness — original C libcrc vs. the Rust port

```sh
./fuzz/run.sh --seconds 60
```

That builds the original C library, builds a batch oracle against it, builds the Rust
fuzzer, runs the fuzzer's own unit tests, fuzzes for sixty seconds, and writes
[`log.txt`](log.txt).

## The recorded run

| | |
|---|---|
| seed | `0xE86F885A2BCDBFC5` (`16748755460077502405`) |
| duration | **75.945 s** continuous, single uninterrupted run |
| cases | 1,100,000 |
| value comparisons | 27,500,000 (25 per case) |
| payload hashed | 633.2 MiB *per algorithm* |
| input stream digest | `0x0AC47573333F30C0` |
| **divergences** | **0** |

Full detail — input mix, per-check counts, environment — is in [`log.txt`](log.txt).
Replay it:

```sh
./fuzz/run.sh --seed 16748755460077502405 --cases 1100000
```

**Reproducibility was verified, not just claimed.** The same seed re-run at a different
batch size (`--batch 7919` instead of `50000`, so 139 oracle invocations instead of 22)
produced the identical input stream digest `0x0AC47573333F30C0`, the identical 633.2 MiB,
and 0 divergences. The batching is genuinely invisible to the input stream.

State it precisely: this is byte-for-byte agreement on every input the generator
produced. It is strong evidence, not proof. `crc_sick` in particular has no external
reference implementation — libcrc is its only definition — so its correctness argument
rests *entirely* on this differential parity.

---

## What is compared

Both sides receive **identical bytes** and are asked for the same 25 values:

| | |
|---|---|
| **13 one-shot** | `crc_8` `crc_16` `crc_32` `crc_64_ecma` `crc_64_we` `crc_ccitt_1d0f` `crc_ccitt_ffff` `crc_dnp` `crc_kermit` `crc_modbus` `crc_sick` `crc_xmodem` `checksum_NMEA` |
| **12 incremental** | the same values rebuilt one byte at a time through `update_crc_8` `update_crc_16` `update_crc_32` `update_crc_64` `update_crc_ccitt` `update_crc_dnp` `update_crc_kermit` `update_crc_sick` |

The shipped `examples/tstcrc.c` prints only nine of these and misses `crc_8`,
`crc_64_ecma`, `crc_64_we` and `checksum_NMEA` entirely, which is why this harness has
its own oracle driver.

The incremental half is the part that catches a finalisation applied in the wrong place.
`update_crc_*` returns the *raw* running value, so the caller has to re-apply the three
byte-swaps, DNP's complement and the two final XORs — exactly as a real libcrc user
streaming a message would. Both sides do it independently.

- **Oracle** — the original C library, built from the pristine upstream tree with the
  project's own `-O3 -funsigned-char`. Lives in the gitignored `oracle/`.
- **Port** — `crates/libcrc-rs`, called directly as a Rust library. No FFI, no shim, no C
  anywhere in the measured path. Nothing under `crates/` links, calls or knows about the
  oracle.

## Batch, never stream

A long-lived oracle fed over a pipe can deadlock on Windows: the anonymous-pipe buffer
fills, the oracle blocks writing its answers, the fuzzer blocks writing its next input,
and neither drains the other. CRC is a pure function, so there is nothing to interleave.

Each round therefore writes a whole batch to a file, runs the oracle **once** with stdin
closed, waits for it to exit, and only then reads the results file. No pipe is ever read,
so there is no pipe to deadlock on. Closing stdin is belt and braces — libcrc's own
`tstcrc` has interactive modes that prompt on stdin, and a hang there would look exactly
like a fuzzer bug.

## Reproducibility

Inputs are derived per case as `SplitMix64(seed ^ SplitMix64(index))`, so case *N*
depends only on the seed and on *N* — never on batch size or on how the run was split.
A recorded seed replays bit-for-bit at any `--batch`, and any single case can be re-run
on its own:

```sh
./fuzz/run.sh --seed <SEED> --case 12345      # dumps both sides in full
```

The PRNG (xorshift64\*, SplitMix64) is implemented here rather than pulled from `rand`:
a bonus that hinges on "reproducible from a recorded seed" should not also hinge on a
third party's version resolution, and the network on this machine is unreliable. There
are no external dependencies at all.

`cargo-fuzz` is not usable here — it requires a nightly toolchain and this machine has
only stable 1.96.0 — so the generator is hand-rolled rather than coverage-guided. The
mitigation is the fixed corpus below, which pins the structural edge cases instead of
hoping to stumble on them.

## Input coverage

A deterministic **fixed corpus of 719 cases** runs before any random input, so these are
covered on *every* run:

- empty input
- all 256 byte values, as a single byte and as a 17-byte uniform fill
- lengths 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129 under four fill
  patterns (zero, `0xFF`, counting, `0x55`/`0xAA`)
- all-zero and all-`0xFF` buffers at 0, 1, 255, 256, 257, 1023, 1024, 1025, 4095, 4096, 4097
- long buffers: 65535, 65536, 65537 and 262144 bytes, four fill patterns
- NMEA sentences with and without the leading `$`, terminated by end-of-string, `*`, CR,
  LF and CRLF, plus embedded NULs and delimiters at interior offsets
- the NULL-pointer contract at four lengths — libcrc returns the init value from its
  twelve CRC functions and NULL from `checksum_NMEA`

Then weighted random classes: random binary (small and medium), boundary lengths, single
bytes, NMEA sentences, uniform fills, zero-heavy, `0xFF`-heavy, sparse, delimiter-rich
ASCII, NULL pointers, and 16 KiB–256 KiB long buffers. The exact mix is reported in
`log.txt` for the run that produced it.

## Two guards on the result

**Pre-flight self-check.** Before the clock starts, the oracle is asked for `"123456789"`
and its nine answers are checked against the values recorded from upstream libcrc. The
realistic failure is a C build without `-funsigned-char`: libcrc forces `char` unsigned
and gcc on x86 defaults to signed, so a careless oracle build is wrong in a way that
would manufacture divergences all night. The fuzzer refuses to run if this fails.

**Negative control.** "Zero divergences" means nothing until you have watched the harness
report a non-zero one.

```sh
./fuzz/negative-control.sh
```

builds a deliberately corrupted oracle (`-DPM_SABOTAGE` flips one bit of `crc_kermit` for
any input containing byte `0x7A`) and fuzzes against it. It must fail, must name
`crc_kermit`, and the shrinker must reduce multi-kilobyte inputs to the single trigger
byte. Results go to `negative-control.log`, never to `log.txt`.

## On finding a divergence

Divergences are reported by algorithm name, not as "80 bytes differ", then minimised
against the oracle: halve from the tail, trim from the front, then zero individual bytes,
re-confirming every step so the shrinker never assumes a smaller input still fails. The
minimised input, both values, and a single-case replay command all go into the log.

## Files

| | |
|---|---|
| `run.sh` | build everything and run. The one command. |
| `oracle_harness.c` | batch oracle. Links the original C library. Not part of the port. |
| `differential/` | the Rust fuzzer. Standalone workspace, so `cargo build` at the repo root never sees it. |
| `negative-control.sh` | proves the harness can fail |
| `prove_d01.sh` | reproduces the uncallable-public-API bug (see below) |
| `log.txt` | the published run |
| `negative-control.log` | the published negative control |

## Upstream note — a public API that cannot be called

Building this harness surfaced it mechanically. `include/checksum.h:99` declares

```c
uint64_t update_crc_64_ecma( uint64_t crc, unsigned char c );
```

No definition exists anywhere in `src/`. `nm lib/libcrc.a` reports exactly one matching
symbol, `update_crc_64`, which the public header does **not** declare. So the documented
incremental CRC-64 entry point fails to link, and the one that works is undocumented —
`oracle_harness.c` has to declare `update_crc_64` itself to reach it at all, and that
extern is the evidence. `./fuzz/prove_d01.sh` reproduces it end to end.
