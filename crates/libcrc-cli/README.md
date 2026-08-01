# `crc` — the command-line binary

The workspace's runnable deliverable. `cargo build --release` (and therefore
`./build.sh`) produces `target/release/crc` — `crc.exe` on Windows.

```console
$ printf '123456789' | crc --all
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

Every one of those thirteen numbers was read out of the **original C library** before it
was written down here. `crc --help` documents the full interface; `crc --list` prints
the table above with the check values computed live at the moment you run it.

## Why this crate exists

The brief asks for a one-step build to a *binary*. The rest of the workspace is a
library plus a C-ABI test harness, so without this there is nothing to run.

It is not filler. It is the one place the port's advantage over the C original becomes
something you can watch happen:

| | libcrc (C) | this binary |
|---|---|---|
| API for a stream | none — one-shot `crc_16(ptr, len)` or `update_crc_16(crc, c)` | `Crc16Digest::update(&[u8])` |
| How the shipped CLI reads a file | `fgetc()`, one call per byte — `examples/tstcrc.c:210` | 64 KiB chunks |
| Algorithms printed | 9 of 13 — no CRC-8, neither CRC-64, no NMEA | 13 of 13 |
| Finalisation (byte-swap, complement, final XOR) | open-coded in the caller — `tstcrc.c:231-244` does the CRC-32 XOR, the DNP complement, and the DNP, SICK and Kermit byte swaps by hand | owned by the digest |
| Verifiable checksum manifest | no | `--check` |
| Dependencies | — | none |

**Measured on this machine**, 512 MiB of random data, release build, all thirteen
algorithms in one pass: **18.8 s wall, 2.97 MiB peak resident**. Memory is constant
because nothing larger than one 64 KiB buffer is ever held; the file could be a
terabyte. The same file through the original `tstcrc.exe` took 25.4 s to produce nine
algorithms in its own single `fgetc` pass, and the two agree on all nine. That is one
unrepeated run of two programs doing different amounts of work, so read it as an
architectural observation, not as a benchmark. The real benchmarks are in `bench/`.

## Design constraints

* **No dependencies.** Argument parsing is hand-rolled. The workspace `Cargo.lock`
  contains three path crates and nothing else, so there is no supply chain to audit.
* **No panics on input.** A missing file, a directory, a permission error, a malformed
  manifest, a non-UTF-8 argument and a closed pipe are all ordinary errors: one line on
  stderr and a documented exit status. `tests/cli.rs` asserts *on every single
  invocation it makes* that the child did not panic, so a regression anywhere trips a
  test somewhere.
* **No unsafe.** `#![forbid(unsafe_code)]`, as in the port itself.
* **`std` lives here.** `libcrc-rs` is `no_std`; this crate is the layer allowed to do
  file I/O, which is why they are separate crates rather than a feature flag.

## Exit status

| | |
|---|---|
| `0` | everything requested succeeded |
| `1` | an input could not be read, or a `--check` entry did not match |
| `2` | the command line was malformed |

## Verification

```console
$ cargo test -p libcrc-cli
```

49 tests: 28 unit (argument parsing, manifest parsing, the streaming states) and 21
integration tests that spawn the real binary and assert on its stdout, stderr and exit
status.

The integration tests pin the C library's check values as literals, so they need
nothing external. If you have also built the oracle (`oracle/`, gitignored, never a
dependency of the port), you can additionally diff the *binary* against the original
project's own CLI:

```console
$ crates/libcrc-cli/differential-vs-c.sh
63 comparisons over 7 file(s), 0 divergences
OK — the crc binary agrees with the original C library on every algorithm tstcrc prints.
```

The corpus straddles the 64 KiB read buffer deliberately — empty, 1 byte, 9 bytes,
65535, 65536, 65537 and 300007 bytes — because a chunking bug hides everywhere except
at a boundary. With the comparison deliberately sabotaged the same script reports
`63 divergences` and exits 1, so a pass means the harness was capable of failing.

## A warning about the numbers

`crc_kermit`, `crc_dnp` and `crc_sick` byte-swap their result **because libcrc does**.
`crc --list` says so, and so does `crc --help`. If you compare this binary against a
catalogue-conformant CRC tool, those three will differ, and this binary is the one
faithfully reproducing the library being ported. See `DECISIONS.md` in the repository
root.
