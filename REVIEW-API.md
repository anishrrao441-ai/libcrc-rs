# REVIEW-API.md — Senior Rust API & Code-Quality Review

Independent review pass. Scope: every line of `crates/libcrc-rs/src/*.rs`,
`crates/libcrc-rs/tests/properties.rs`, `crates/libcrc-rs/examples/update_parity.rs`,
`crates/libcrc-cabi/src/lib.rs`, and all of `crates/libcrc-cli/`.

**Report only. No source file outside this one was modified.** Every measurement below was
produced by running a command; the one proposed fix was verified in a *copy* of the crate in a
scratch directory, never in the repository.

---

## Verdict in one paragraph

This is idiomatic Rust, not C in Rust syntax — with one file as the exception. The port uses
slices instead of pointer+length, iterator folds instead of index loops, `const fn` tables
instead of a racy lazy init, exhaustive `match` instead of `switch`, and `split_first_chunk`
instead of an unchecked cast. The `no_std` + `forbid(unsafe_code)` discipline is real and holds.
The documentation prose is well above hackathon average, and `combine.rs` is genuinely good work.

But the API is **unfinished in ways a senior reviewer will notice immediately**: the streaming
digests are a second, slower reimplementation of the same fold the one-shot functions use — I
measured them at **4.3–4.7× slower** — so the crate's flagship optimisation is unreachable from
the binary the brief asks for. The library ships digests for only 5 of the 13 algorithms, forcing
the CLI to re-implement the other 8 *including their finalisation rules*, which is precisely the
mistake `digest.rs`'s own module doc says the digests exist to prevent. Not one of the 40 public
functions and methods is `#[must_use]`. The CLI's error type does not implement
`std::error::Error`. And paths are `String`, so `crc` cannot checksum a file whose name is not
valid UTF-8.

None of this is a correctness bug. All of it is Code Quality, which is what is being scored.

---

## How the findings were produced

| Command | Result |
|---|---|
| `cargo clippy -p libcrc-rs --lib --all-features -- -W missing_docs` | 4 undocumented public items |
| `cargo clippy -p libcrc-rs --lib --all-features -- -W clippy::pedantic` | 62 warnings: 40 `must_use`, 17 backticks, 5 casts |
| `cargo test -p libcrc-rs --doc` | 14 doc-tests, all pass, 0 on the primary hashing API |
| `cargo test -p libcrc-rs` | 68 + 7 + 14 pass |
| `cargo test -p libcrc-rs --no-default-features` | 60 + 7 + 14 pass (**not run by `build.sh` or CI**) |
| standalone throughput harness (scratch crate, path-dep on the real crate) | digests 4.29–4.68× slower than one-shot |
| same harness against a patched *copy* of the crate | 1.00× — parity, 89 tests still green |

The throughput harness asserts `Crc32Digest == crc_32` and `Crc16Digest == crc_16` on a 1 MiB
buffer before timing, takes the best of 200 runs per configuration, and uses
`std::hint::black_box`. Release profile inherited from the workspace (`opt-level = 3`,
`lto = true`, `codegen-units = 1`).

---

# CRITICAL

## C-1 — The streaming digests are a second copy of the fold, and it is the slow copy

`crates/libcrc-rs/src/digest.rs:42-46`, `:79-83`, `:122-126`, `:169-173`

```rust
// digest.rs:42
pub fn update(&mut self, data: &[u8]) {
    self.crc = data
        .iter()
        .fold(self.crc, |crc, &b| super::update_crc_16(crc, b));
}
```

Compare `crates/libcrc-rs/src/lib.rs:284-287`, which already exists and already does the right
thing:

```rust
fn fold_16(start: u16, data: &[u8]) -> u16 {
    let (crc, tail) = slice8::fold_16(start, data);
    tail.iter().fold(crc, |crc, &b| update_crc_16(crc, b))
}
```

Three consequences, in ascending order of how much they cost:

1. **Duplication.** The fold loop for CRC-16, CRC-32 and CRC-64 is written twice — once in
   `lib.rs`, once in `digest.rs` — and `Crc32Hasher::write` (`digest.rs:169-173`) writes it a
   *third* time. Nothing enforces that the copies agree.
2. **The crate's headline feature is unreachable from the streaming path.** `slice8.rs` is a
   748-line module with a 112-line derivation, 23,296 bytes of `.rodata`, a measured crossover
   table, a cargo feature and six negative-control mutations. `Crc16Digest::update` does not call
   any of it.
3. **The shipped binary never touches it either.** `crc` reads files through
   `hash.rs:44-61 → Digest::update → Crc16Digest/Crc32Digest/Crc64Digest::update`. So the "one-step
   build → binary" deliverable runs at byte-at-a-time speed for the five algorithms that *do* have
   a digest type, and at byte-at-a-time speed for the other eight as well (see H-1). The
   crossover table at `slice8.rs:145-151` and everything in `bench/` describe a code path the
   executable never executes.

**Measured, on this machine, release profile:**

```
n=    4096  crc_32 one-shot  1861.818 MB/s | Crc32Digest   417.959 MB/s | 4.45x slower
n=    4096  crc_16 one-shot  1861.818 MB/s | Crc16Digest   397.670 MB/s | 4.68x slower
n=   65536  crc_32 one-shot  1810.387 MB/s | Crc32Digest   421.183 MB/s | 4.30x slower
n=   65536  crc_16 one-shot  1805.399 MB/s | Crc16Digest   395.033 MB/s | 4.57x slower
n= 1048576  crc_32 one-shot  1807.578 MB/s | Crc32Digest   421.606 MB/s | 4.29x slower
n= 1048576  crc_16 one-shot  1807.890 MB/s | Crc16Digest   395.093 MB/s | 4.58x slower
```

64 KiB is the CLI's read chunk (`hash.rs:20`), so the 65,536-byte row is exactly the shipped
configuration.

**Fix — 4 statements, verified green in a scratch copy.** In `lib.rs`, extract the CRC-32 fold
the same way `fold_16` and `fold_ccitt` already are, and add the trivial CRC-64 one:

```rust
pub fn crc_32(data: &[u8]) -> u32 { fold_32(START_32, data) ^ 0xFFFF_FFFF }

fn fold_32(start: u32, data: &[u8]) -> u32 {
    let (crc, tail) = slice8::fold_32(start, data);
    tail.iter().fold(crc, |crc, &b| update_crc_32(crc, b))
}
fn fold_64(start: u64, data: &[u8]) -> u64 {
    data.iter().fold(start, |crc, &b| update_crc_64(crc, b))
}
```

Then each `update` becomes one line — `self.crc = super::fold_16(self.crc, data);` and so on, and
`Crc32Hasher::write` becomes `self.digest.0 = super::fold_32(self.digest.0, bytes);`.

**Why this is safe, and how I know.** `fold_16(crc, a ‖ b) == fold_16(fold_16(crc, a), b)` because
`fold_16` is exactly "fold every byte of `data` into `crc`", block-folded or not — which
`slice8.rs`'s own `assert_all_agree` already proves for six arbitrary mid-stream seeds
(`slice8.rs:495`), not just the documented initial values. Chunk boundaries therefore cannot
change the answer, which is the property `properties.rs:113` and `digest.rs:213` already assert.

**Verified in the scratch copy, not the repo:**

```
cargo test                      68 + 7 + 14 pass
cargo test --no-default-features 60 + 7 + 14 pass
cargo clippy --all-targets -- -D warnings   clean
cargo fmt --all -- --check                  clean
throughput                      1.00x — Crc32Digest == crc_32, Crc16Digest == crc_16
```

Note honestly what this does **not** fix: `Crc64Digest` gains nothing but the dedup, because
CRC-64 is deliberately not accelerated (`slice8.rs:101-106`); and the eight algorithms the CLI
hand-rolls stay slow until H-1 is addressed.

---

# HIGH

## H-1 — The library ships 5 digests of 13, so the CLI re-implements the other 8 — finalisation rules and all

`crates/libcrc-cli/src/algo.rs:171-189` (the `State` enum), `:191-225` (`update`), `:231-247`
(`finish`), `:250-292` (`Nmea`).

`digest.rs:1-11` states the case for the digests exactly right:

> "Hashing a stream therefore means either buffering the whole input or writing a manual byte loop
> and remembering each algorithm's finalisation rule — the byte-swap for Kermit, the
> complement-then-swap for DNP, the final XOR for CRC-32. These types own that finalisation, so
> the caller cannot get it wrong."

And then the CLI, the crate's own first consumer, writes a manual byte loop and remembers each
algorithm's finalisation rule:

```rust
// algo.rs:235-238 — the exact rules digest.rs promised to own
State::Sick   { crc, .. } => u64::from(crc.swap_bytes()),
State::Kermit(crc)        => u64::from(crc.swap_bytes()),
State::Dnp(crc)           => u64::from((!crc).swap_bytes()),
```

Five of thirteen use a port digest (`Crc16Digest` ×2, `Crc32Digest`, `Crc64Digest` ×2). The other
eight — `crc_8`, `crc_sick`, `crc_xmodem`, `crc_ccitt_ffff`, `crc_ccitt_1d0f`, `crc_kermit`,
`crc_dnp`, `nmea` — are re-derived in the binary. `algo.rs:6-9` is candid about it ("The
algorithms with no `Digest` type yet are folded here"), but `help.rs:50-51` is not: it tells the
user input is "folded through the port's streaming Digest types", which is true for 5/13.

This is the single clearest piece of evidence that the library's API is incomplete: the only
non-test consumer could not be written against it.

**Fix.** Add `Crc8Digest`, `CcittDigest` (seed parameter covers XMODEM and both CCITT variants),
`KermitDigest`, `DnpDigest`, `SickDigest` and `NmeaChecksum` to `libcrc-rs`, each delegating to
the corresponding `fold_*` helper from C-1. `algo.rs`'s `State` enum then becomes thirteen thin
variants with no arithmetic in it at all, and `Digest::finish` collapses to `u64::from(..)` calls.
`SickDigest` matters most: `update_crc_sick(crc, byte, prev_byte)` (`lib.rs:246`) is a public
function whose third argument the caller must thread by hand, and `algo.rs:200-205` is the proof
that consumers will have to.

## H-2 — `CliError` does not implement `std::error::Error`

`crates/libcrc-cli/src/error.rs:14-69`. There is no `impl std::error::Error for CliError`
anywhere in the workspace (`grep`ed: the only `Display` impl in three crates is `CliError`'s).

The rubric asks for "Result, not errno". The *plumbing* is genuinely good — see the praise
section — but the error type is half-built. Without `Error`:

- `CliError` cannot be boxed into `Box<dyn Error>` and cannot cross an FFI or library boundary.
- `source()` is unavailable, so the `io::Error` inside `CliError::Io` is reachable only through
  the flattened `Display` string.
- `anyhow`/`eyre`-style consumers, and `fn main() -> Result<(), Box<dyn Error>>`, cannot use it.

**Fix — 10 lines:**

```rust
impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CliError::Io { source, .. } | CliError::Output(source) => Some(source),
            CliError::Usage(_) | CliError::Manifest { .. } => None,
        }
    }
}
```

## H-3 — Not one of the 40 public functions and methods is `#[must_use]`

`cargo clippy -p libcrc-rs --lib --all-features -- -W clippy::pedantic` reports 32
`this function could have a #[must_use] attribute` plus 8 for methods — which is *every* public
item in the crate: 13 one-shot functions, 8 `update_crc_*`, 11 `*_combine`, and 8 digest methods.

For a library of pure total functions this is not a style nit. `crc_32(&data);` as a statement is
always a bug — the call has no other effect — and `#[must_use]` is the one-token Rust idiom that
makes the compiler say so. Its complete absence is the most visible signal in the crate that the
public surface was written as functions rather than designed as an API.

**Fix.** `#[must_use]` on all 32 free functions and on `new`/`modbus`/`we`/`finalize`. Or set it
crate-wide in `Cargo.toml`:

```toml
[lints.clippy]
must_use_candidate = "warn"
```

## H-4 — Paths are `String`, so `crc` cannot checksum a file whose name is not valid UTF-8

`crates/libcrc-cli/src/main.rs:49-61`, `args.rs:29`, `hash.rs:25`.

`collect_args` reads `args_os()` and then rejects any argument that is not valid UTF-8 as a usage
error. The stated motivation (`main.rs:46-48`) is correct — `std::env::args()` panics on such an
argument and this program must not panic — but the chosen remedy converts a *type* problem into a
*capability* loss: `files: Vec<String>` and `hash::checksums(path: &str, ..)` mean a legitimate
file whose name is not UTF-8 (routine on Linux, possible on Windows) cannot be hashed at all, and
the diagnostic blames the user's command line.

`&str` for a filesystem path is one of the clearest C-isms left in the codebase. Rust has
`OsString`/`PathBuf` precisely for this.

**Fix.** Keep `files: Vec<PathBuf>` (from `OsString`, infallibly), keep the UTF-8 check only for
*option values* (`-a`, `-c`), and take `path: &Path` in `hash::checksums`. Manifest lines stay
`String` — the file format is text and that is a separate, defensible decision — but should then
be documented as "manifest paths must be UTF-8" rather than being an accident of the argument
type. `check.rs:150-161` already parses paths out of text, so only the argv route needs changing.

---

# MEDIUM

## M-1 — Documentation: 4 public items undocumented, the crate root has no example, and the primary API has no doc-tests

`cargo clippy -p libcrc-rs --lib -- -W missing_docs` reports exactly four:

```
digest.rs:75  Crc32Digest::new
digest.rs:79  Crc32Digest::update
digest.rs:122 Crc64Digest::update
digest.rs:128 Crc64Digest::finalize
```

`Crc16Digest`'s equivalents *are* documented, so this is drift, not policy — and nothing prevents
more of it, because the crate has `#![forbid(unsafe_code)]` (`lib.rs:47`) but no
`#![deny(missing_docs)]`.

Worse, the doc-test distribution is inverted. All 14 doc-tests are on the *new* API — 11
`*_combine` plus 3 digest types. **Every one of the 13 one-shot functions has zero examples**, and
so do all 8 `update_crc_*` and `checksum_nmea` and `Crc64Digest`. The functions a user reaches for
first — `crc_16`, `crc_32`, `crc_modbus` — are documented by a single sentence and a check value.
The crate-level doc (`lib.rs:1-45`) contains a comparison table and three prose sections and **not
one line of runnable Rust**; a reader arriving on docs.rs cannot see how to call the library
without clicking through.

**Fix.** Add `#![deny(missing_docs)]`, fix the four, add a five-line quick-start to `lib.rs`, and
add `# Examples` to `crc_16`, `crc_32` and `checksum_nmea`. Doc-tests already run in `build.sh`
(`build.sh:131`), so these become executed evidence for free.

## M-2 — `slice8.rs` duplicates fourfold what `combine.rs` solved with a macro

`slice8.rs:205-219`, `:222-236`, `:239-253`, `:256-270` are four `const fn` slice builders that
differ **only** in the width and one `shift` expression:

```rust
slices[k][i] = t0[prev as usize];                        // u8, forward
slices[k][i] = (prev >> 8) ^ t0[(prev & 0x00FF) as usize];  // u16, reflected
slices[k][i] = (prev << 8) ^ t0[(prev >> 8) as usize];      // u16, forward
slices[k][i] = (prev >> 8) ^ t0[(prev & 0x0000_00FF) as usize]; // u32, reflected
```

`slice8.rs:292-368` then repeats a second fourfold copy of the folded loop. That is roughly 120
lines where 40 would do. Meanwhile `combine.rs:82-144` faces the *identical* stable-Rust
constraint (`const fn` cannot be generic over integer types) and solves it correctly with
`gf2_machinery!`, generating four widths from one 60-line body. Same repository, same author, same
problem, opposite decisions, no comment explaining why.

The duplication has already cost something concrete: the direct table-recurrence test
(`slice8.rs:638-658`) checks `SLICES_16`, `SLICES_32`, `SLICES_CCITT` and `SLICES_8` but **not**
`SLICES_KERMIT` or `SLICES_DNP` — a hole that only exists because the four builders are separate
things a test has to remember to enumerate. (They are covered indirectly by `assert_all_agree`, so
this is a test-shape gap, not a correctness one.)

Second, smaller inconsistency in the same block: `run_reflected_u16` (`:310-315`) takes its tables
as parameters, while `run_forward_u16` (`:335`) and `run_reflected_u32` (`:354`) hard-code
`SLICES_CCITT`/`SLICES_32` in the body. Two conventions for one shape in three adjacent functions.

**Fix.** A `slice_tables!($ty, $t0_builder, $shift)` macro mirroring `gf2_machinery!`, and pass
tables by parameter uniformly. If the duplication is kept deliberately (a defensible call — four
explicit const fns are easier to read cold than one macro), then say so in a comment, because the
reader's first question is "why did combine.rs do this differently?"

**On the question of opacity, explicitly:** the `const fn` table machinery and the slice8 layer
are **not** clever-but-opaque. The derivation at `slice8.rs:22-67` — annihilation, superposition,
`T_k[i] = shift(T_(k-1)[i])` — is one of the best pieces of writing in the repo and a maintainer
will follow it in six months without help. The failure mode here is the opposite of cleverness:
it is honest repetition.

## M-3 — `Crc64Digest` encodes its variant as runtime data; `Crc16Digest` does not

`digest.rs:99-131` vs `:25-52`. `Crc64Digest` carries `final_xor: u64` as a field, doubling the
struct to 16 bytes and making `Crc64Digest::new() == Crc64Digest::we()` impossible even at equal
state. `Crc16Digest` distinguishes ARC from MODBUS purely by seed, so it is 2 bytes — but
consequently an ARC digest and a MODBUS digest with the same running value compare `Eq`, which is
meaningless. Two designs for one problem, and neither is obviously right.

Deriving `PartialEq`/`Eq` on a digest is itself questionable: what is the use case for comparing
two half-consumed hash states?

**Fix.** Pick one. Either encode the variant in the type (`Crc64Digest<Ecma>` / a `const XOR: u64`
parameter — clean on this Rust version), or accept the runtime field consistently and give
`Crc16Digest` one too. Drop `PartialEq`/`Eq` unless a caller needs them.

## M-4 — `finalize(self)` is the right default, but its absence of a `&self` counterpart already cost a duplicated type

Answering the question directly: **`finalize(self)` is correct.** Consuming the digest prevents
use-after-finalise, which matters because `Crc32Digest::finalize` applies the final XOR
(`digest.rs:86-88`) and calling it twice on a `&mut self` design would be a real bug source. Keep it.

But it needs a companion, and the code already shows why. `Crc32Hasher` (`digest.rs:154-178`)
could not reuse `Crc32Digest`, because `Hasher::finish(&self)` takes `&self` and
`finalize(self)` consumes. The workaround is a private newtype that re-implements the same state
and the same three lines:

```rust
// digest.rs:159-166 — a second Crc32Digest wearing a different name
#[derive(Debug, Clone)]
struct Crc32DigestState(u32);
impl Default for Crc32DigestState { fn default() -> Self { Self(0xFFFF_FFFF) } }
```

The magic constant `0xFFFF_FFFF` now appears four times in one 253-line file — `digest.rs:76`,
`:87`, `:165`, `:176` — three of which must stay in sync by hand.

**Fix, either of:**
- `#[must_use] pub const fn peek(&self) -> u32 { self.crc ^ 0xFFFF_FFFF }` alongside
  `finalize(self)`, then `Crc32Hasher { digest: Crc32Digest }` and `finish` is `self.digest.peek()`; or
- keep only `finalize(self)` and use the existing `Clone`: `self.digest.clone().finalize()`.

Either deletes `Crc32DigestState` and three of the four literals.

## M-5 — Inherent methods where std traits belong

| Site | Now | Should be |
|---|---|---|
| `algo.rs:134` | `Algo::parse(&str) -> Option<Algo>` | `impl FromStr for Algo` (with an error type, so `args.rs:175-180` need not rebuild the message) |
| `algo.rs:66` | `Algo::name() -> &'static str` | `impl fmt::Display for Algo` (keep `name()` as the `const fn` it needs to be) |
| `algo.rs:303-313` | `Checksum::to_hex(self) -> String`, `render(bool) -> String` | `impl fmt::UpperHex`/`fmt::Display` — writes into the formatter, allocates nothing |

`to_hex` allocates a `String` per algorithm per file per line. `check.rs:66-71` goes further and
constructs an entire throwaway `Checksum { algo, value }` purely to reach `to_hex` for one error
message; with `UpperHex` it would be `{:#0width$X}` inline. `parse` returning `Option` is also why
`args.rs:175` has to re-synthesise "unknown algorithm '{}'" at the call site rather than getting it
from the parse failure.

## M-6 — `group_by_path` is O(n²)

`crates/libcrc-cli/src/check.rs:175-184`:

```rust
for entry in entries {
    match groups.iter_mut().find(|g| g[0].path == entry.path) { .. }
}
```

A linear scan with a full string comparison per existing group, per entry. A manifest written by
`crc --all *.bin` over 5,000 files is 65,000 entries and 5,000 groups — on the order of 10⁸ string
comparisons before a single byte is read. The comment above it advertises the first-mention
ordering as the reason for the shape, but that ordering is preserved just as well by a
`HashMap<&str, usize>` from path to group index, which is the same six lines and linear.

## M-7 — `examples/update_parity.rs` is the one file that reads as C transliterated into Rust

`crates/libcrc-rs/examples/update_parity.rs` — 113 lines, 31 `clippy::pedantic` warnings, which is
one every 3.6 lines and by far the worst density in the repository. It has:

- no `//!` module doc (every other file in the workspace has one)
- no doc comment on either function
- `as u64` / `as u16` / `as u8` casts throughout where `u64::from` is infallible (`:19`, `:31`,
  `:43`, `:55`, `:67`, `:81`, `:96`, `:108`)
- hand-rolled `while` loops with manual counters (`:75-86`, `:92-100`, `:106-111`) where the rest
  of the crate uses `for`/iterators
- positional `println!("... {:016X}", acc)` where every other file in the workspace uses inline
  captures (`{acc:016X}`)

It is a differential harness that mirrors `update_oracle.c` deliberately, and mirroring the C
enumeration *order* is right. Mirroring the C *style* is not — and this file lives inside the port
crate, so `cargo build --examples` compiles it and a reviewer browsing `crates/libcrc-rs/` will
open it.

**Fix.** 20 minutes: a `//!` header explaining the mirroring, `u64::from`, `for crc in 0u16..=u16::MAX`
where the range allows, inline format captures. Do not change the enumeration or the stride
constants — those are load-bearing against the C side.

## M-8 — Manifests are missing everything a published crate declares, including MSRV

All three `Cargo.toml` files have `name`/`version`/`edition`/`license`/`description` and nothing
else. Missing: `repository`, `readme`, `keywords`, `categories`, `authors`, and — the one that has
teeth — **`rust-version`**.

The crate uses `slice::split_first_chunk` (`slice8.rs:293`, stable 1.77) and
`io::Error::other` (`hash.rs:37`, stable 1.74). Nothing declares that, so a user on an older
toolchain gets a raw compiler error instead of cargo's MSRV diagnostic. For a crate whose pitch is
"drop-in replacement for an embedded C library", where old pinned toolchains are the norm, this is
not cosmetic.

Also absent: a `[lints]` table. `-D warnings` is enforced only inside `build.sh` and CI, so a
contributor running plain `cargo clippy` sees a clean run that CI will reject. `[lints.rust]
missing_docs = "deny"` + `[lints.clippy] must_use_candidate = "warn"` would move M-1 and H-3 into
the compiler where they belong.

## M-9 — `--no-default-features` is claimed to be *tested* and is only *built*

`slice8.rs:88-89` states: *"The full test suite is run both ways."*

What actually runs it: nothing. `build.sh` has exactly one cargo test invocation
(`build.sh:129`, default features) and `.github/workflows/ci.yml` adds none.

**Scope note, checked during this review:** a `.github/workflows/no-std.yml` appeared in the
working tree while I was writing (untracked at the time of writing). It is thorough and it closes
the *other* half of what this finding originally covered — it builds `-p libcrc-rs` for
`thumbv7em-none-eabihf` in four configurations including `--no-default-features` (dev and
release+LTO), links a real Cortex-M4F firmware image, asserts `.rodata` is non-empty so the tables
cannot have been dead-stripped, and carries a negative control that injects `use std::vec::Vec`
into a throwaway copy and requires the build to fail with E0433. That is a genuinely strong piece
of evidence and I withdraw the "`no_std` is never verified against a bare-metal target" half of
this finding.

It does **not** close this half. Every step in that workflow is `cargo build`; none is
`cargo test`. So the sentence at `slice8.rs:88-89` is still an assertion.

I ran it myself: `cargo test -p libcrc-rs --no-default-features` passes, 60 + 7 + 14. The claim is
true today — by manual discipline, not by construction. The `slice8` feature gate is the mechanism
the crate's entire embedded story rests on, and the identity shims at `slice8.rs:716-742` are the
code path that gets no automated exercise at all.

**Fix — one line in `build.sh` next to line 129**, or one step in either workflow:
`cargo test -p libcrc-rs --no-default-features`. It converts an assertion into evidence for the
cost of 0.4 seconds.

---

# LOW

**L-1 — `pub` in a binary crate.** `Algo`, `Checksum`, `Digest`, `Mode`, `Options`, `CliError` and
their methods are all `pub` (`algo.rs:28`, `:45`, `:62`, `:166`, `:297`; `args.rs:16`, `:24`;
`error.rs:15`). A `[[bin]]` has no external consumers; every one of these should be `pub(crate)`.
Answering the "anything public that should be private" question: this is the whole of it — nothing
in `libcrc-rs` itself is over-exposed.

**L-2 — Two names for one concept.** `libcrc-rs` says `finalize` (`digest.rs:49`), the CLI's own
`Digest` says `finish` (`algo.rs:231`), and `Crc32Hasher` says `finish` because `Hasher` requires
it. Three spellings in one workspace. Rename the CLI's to `finalize`.

**L-3 — `byteswap` earns its keep only half the time.** `lib.rs:192-195` wraps `u16::swap_bytes`
in a `const fn` with a good explanatory comment, used four times in `lib.rs`/`combine.rs` — but
`algo.rs:235-238` and `slice8.rs:610`/`615` call `.swap_bytes()` directly, so the abstraction does
not actually centralise anything. Better: make it public and named for its meaning
(`pub const fn to_catalogue_order(crc: u16) -> u16`), which also gives users of `crc_kermit` and
`crc_dnp` the conversion the docs tell them they need and currently do not provide.

**L-4 — `libcrc-cabi` is not `#![no_std]`** despite importing only `core` (`cabi/src/lib.rs:30`).
It is a `staticlib` aimed at libcrc's embedded audience; adding `#![no_std]` costs nothing and
stops std from being linked in. Low priority only because this crate is explicitly a test harness.

**L-5 — glob import in a test.** `tests/properties.rs:12` `use libcrc_rs::*;`. Every other file in
the workspace imports explicitly.

**L-6 — redundant `continue`.** `hash.rs:56` — `Err(e) if e.kind() == Interrupted => continue` at
the end of a `match` inside `loop`. Clippy pedantic flags it; the comment above it is worth
keeping either way.

**L-7 — `Crc32Hasher` is the wrong width for `Hasher`.** `Hasher::finish` returns `u64` and
`digest.rs:176` zero-extends a 32-bit value into it, throwing away half the output space of every
`HashMap` bucket computation. `Crc64Hasher` over the existing `update_crc_64` is the natural fit
and is missing. (`Crc32Hasher` is still worth keeping — CRC-32 is what people ask for.)

**L-8 — Which std traits are actually missing.** Answering the question directly:

| Trait | Verdict |
|---|---|
| `io::Write` for the digests, behind a `std` feature | **Yes — highest value.** It would let `hash.rs` use `io::copy` and delete its whole read loop, and it is what every other hashing crate offers. |
| `Extend<u8>` / `Extend<&u8>`, `FromIterator<u8>` | Yes, cheap, idiomatic, three lines each. |
| `LowerHex`/`UpperHex` on `Checksum` | Yes — see M-5. |
| `Display` on the digests | **No.** There is no natural rendering of a half-consumed CRC state; `Display` on a digest would be noise. |
| `Hash` on the digests | No. Meaningless. |
| `FromStr`/`Display` on the CLI's `Algo` | Yes — see M-5. |

**L-9 — no `reset()` on any digest.** Fine for the CLI (a fresh digest per file is free), but a
`no_std` caller with a fixed buffer and no allocator has to rebuild the struct. One line each.

**L-10 — a doc comment attached to the wrong item.** `slice8.rs:173-181` is a nine-line
explanation of the transcribed-vs-derived CRC-8 table strategy, attached to
`const POLY_8: u8 = 0x31;`. It documents `forward_table_u8`/`slices_u8`, not the polynomial.

**L-11 — `c_export!` invocations are formatted unusually.** `cabi/src/lib.rs:53-76` puts the doc
comment inside the macro call on the same line as the arguments:

```rust
c_export!(/// libcrc `crc_8()`.
    crc_8 -> u8);
```

Legible, but rustfmt will not touch macro interiors, so this stays odd forever. Moving the `///`
above the `c_export!(` line and taking it as a `$(#[$m:meta])*` prefix (which the macro already
accepts, `:44`) reads normally.

---

# Direct answers to the questions asked

**Is this idiomatic Rust or C transliterated?** Idiomatic, with one exception. Concretely on the
right side: `checksum_nmea` (`lib.rs:371-379`) is `strip_prefix` → `take_while` → `fold`, where the
C is a pointer walk with two sentinels; `crc_sick` (`lib.rs:345-351`) carries the previous byte in
a fold accumulator tuple instead of a mutable local; `split_first_chunk::<8>` (`slice8.rs:293`)
gets a `&[u8; 8]` with no bounds check *and* no `unsafe`, which is the exact idiom for that job;
the `State` enum in `algo.rs:171-189` makes the thirteen algorithms a closed set the compiler
checks. The `const fn` table builders (`lib.rs:68-155`) use `while` loops with manual indices,
which *looks* like C — but iterators are not available in stable `const fn`, the code says so, and
this is the correct stable-Rust shape. The exception is `examples/update_parity.rs` (M-7).

**Is the naming Rust-y or C-y?** The port's names are deliberately C-y (`crc_16`,
`update_crc_16`, `checksum_nmea`) and that is right — they are the API contract being ported, and
there is a structural reason too: `cabi/src/lib.rs:43-51` re-exports `libcrc_rs::$name` under the
same identifier, so a rename would break the macro that lets the unmodified C suite link. The one
place where Rust naming was available and not taken is `*_combine`: that API has no upstream name
to preserve, so `combine::crc_16(a, b, len)` would have read better than `crc_16_combine`.
Defensible as consistency; worth a sentence in `DECISIONS.md` rather than a change now.

**Error handling — is `Result` needed in the core?** **No, and saying so is the honest answer.**
Every CRC function here is total: every `&[u8]` maps to exactly one value, there is no allocation,
no I/O, no parse step and no invalid input. `Result<u16, Infallible>` would be ceremony that every
caller unwraps, and it would make the `no_std` embedded story worse, not better. The C original's
`errno`-shaped failure mode is *NULL returns a seed value*, which this port models correctly by
not having pointers at all — the `Option`-free `&[u8]` signature is the fix. Where `Result`
belongs, it is present and threaded properly (`main.rs`, `hash.rs`, `check.rs`, `args.rs`); see
H-2 and H-4 for the two gaps in it.

**Duplication that should be generic:** C-1 (digests), H-1 (CLI re-implementations), M-2
(`slice8.rs`). **Macros that should be plain code:** none. `gf2_machinery!` and `byte_operator!`
(`combine.rs:82`, `:159`) are both justified — `const fn` cannot be generic over integer widths on
stable — and `combine_suite!` (`combine.rs:416`) generates 44 tests from one body, which is exactly
what test macros are for. `c_export!` (`cabi:43`) is fine. The problem in this repo is the reverse:
one module reached for macros and its neighbour did not.

---

# Earned praise (short, and only where it is earned)

- **`combine.rs` is the strongest file in the repository.** The `init ⊕ xorout` correction
  (`:37-41`) is a real generalisation past zlib's CRC-32-only formula, the operators are derived
  from the port's own byte-fold rather than re-derived from polynomials (`:45-50`, which is also
  what makes `crc_8_combine` possible at all for a table libcrc states no polynomial for), and
  `dropping_the_correction_term_breaks_the_algorithms_that_need_it` (`:658`) is a genuine negative
  control, not decoration.
- **The two "cannot be done" sections are worth more than the features around them.**
  `slice8.rs:93-100` and `combine.rs:58-65` explain precisely why `crc_sick` is excluded from both,
  in terms of the algebra rather than as an apology. Shipping a wrong `crc_sick_combine` would have
  been easy and nobody would have caught it.
- **The CLI's failure handling is above average for a hackathon.** Broken pipe treated as success
  (`main.rs:63-76`), `ErrorKind::Interrupted` retried (`hash.rs:56`), directories detected *before*
  open because the OS message is useless on both platforms (`hash.rs:32-38`), one bad input not
  abandoning the rest (`main.rs:113`), stdout flushed before each stderr line so a combined log
  stays ordered (`main.rs:136`, `check.rs:60`). None of that is accidental.
- **`tests/cli.rs:144-153` asserts no invocation panics, on every call in the file.** Making the
  harness enforce the contract rather than trusting one dedicated test is the right instinct.

---

# The ONE change to make with the time remaining

**Fix C-1: make the digests call `fold_16`/`fold_32`/`fold_64` instead of re-implementing the byte
loop.**

Four statements. It is the highest-leverage change available on the Code Quality axis because it is
the only finding that is simultaneously (a) *duplication removed* — the same fold stops being
written three times, which is what "idiomatic to a senior Rust reviewer" actually measures,
(b) *a 4.3–4.7× measured improvement in the shipped binary's hot path*, and (c) *the change that
makes the crate's most heavily documented feature reachable from the deliverable a judge runs* —
`slice8.rs`, `bench/`, the cargo feature and the crossover table currently describe a code path
`crc` never executes.

I have already verified the exact patch in a scratch copy of the crate: 89 tests pass with default
features, 81 with `--no-default-features`, `cargo clippy --all-targets -- -D warnings` is clean,
`cargo fmt --all -- --check` is clean, and `Crc32Digest`/`Crc16Digest` land at 1.00× of the
one-shot path.

If a second change fits: **H-3**, `#[must_use]` on the public surface — mechanical, zero risk, and
it removes 40 of the 62 pedantic warnings a reviewer will see if they run clippy themselves.
