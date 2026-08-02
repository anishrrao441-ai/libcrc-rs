# Robustness review — panics, overflow, UB, hostile input

Adversarial review of `crates/libcrc-rs`, `crates/libcrc-cabi` and `crates/libcrc-cli`.
Every number, exit code and message below was produced by a command run on this machine.
Nothing is extrapolated unless it says so. Where I could not verify something, it says
that instead.

**Environment.** rustc 1.96.0 x86_64-pc-windows-gnu · gcc 16.1.0 · Windows 10 Pro 19045.

**Overflow checks were verified ON, not assumed.** `cargo build -v` shows cargo passes
neither `-C opt-level` nor `-C debug-assertions` for the dev profile, so rustc's defaults
apply. Confirmed with a standalone probe compiled the same way:

```
$ rustc ovf.rs -o ovf.exe && ./ovf.exe
thread 'main' panicked at ovf.rs:3:17:
attempt to add with overflow
exit=101
```

Every "debug build" result below therefore has bounds checks **and** arithmetic overflow
checks live.

---

## Verdict

**The port itself — `crates/libcrc-rs` — is clean.** 935 480 one-shot calls across
71 960 adversarial buffers, in a debug build with overflow checks on, in both feature
configurations: zero panics, zero overflows, zero out-of-bounds. Every table index is
either a `u8` or masked, including the eight per-lane indices in the new `slice8.rs`;
the audit below enumerates all of them individually.

**One real defect was found, and it is in the shim.** `checksum_NMEA` in `libcrc-cabi`
reads out of bounds — and segfaults — on an input where the original C library does not.
`UNSAFE.md` currently asserts the opposite. That is F1, and it is the only finding here
that I would call a must-fix.

The CLI does not panic on anything I threw at it. Its three findings are resource
behaviour, not crashes.

| # | Severity | Where | Finding |
|---|---|---|---|
| F1 | **HIGH** | `libcrc-cabi/src/lib.rs:160` | Segfaults where the C original returns cleanly, on input inside the C's documented contract. `UNSAFE.md` U-2 states the opposite. |
| F2 | MEDIUM | `libcrc-cli/src/check.rs:176` | `--check` is O(N²) in distinct manifest paths. 3.85× time for 2× N, measured. 64 000 paths = 13.1 s vs 0.37 s. |
| F3 | MEDIUM | `libcrc-cli/src/check.rs:135` | `--check` slurps the manifest whole: 226 MB peak RSS for a 64 MiB manifest, unbounded, and an allocation failure aborts rather than travelling as a `CliError`. `--help` claims constant memory. |
| F4 | LOW | `libcrc-cli/src/check.rs:47` | Duplicate manifest entries are not de-duplicated: a 12 KB manifest costs 10.8 s against an 8 MiB file. `--algo` de-duplicates; `--check` does not. |
| F5 | LOW | `libcrc-cli/src/hash.rs:38` | The `fs::metadata` directory pre-check makes `crc NUL` fail on Windows, where `File::open` alone would have worked. |
| F6 | NOTE | `libcrc-cli/src/hash.rs:56` | `&buf[..read]` panics if a `Read` impl over-reports. Unreachable through `File`/`Stdin`; one-line hardening available. |
| F7 | NOTE | `libcrc-cabi/src/lib.rs:40` | `slice::from_raw_parts` requires `len <= isize::MAX`, a *stricter* precondition than the C loop's. Undocumented in `UNSAFE.md` U-1. |
| F8 | NOTE | `libcrc-cli/src/check.rs:135` | `--check <directory>` reports a raw OS error where `crc <directory>` says "is a directory". |

---

## F1 — HIGH — the port reads out of bounds where the C original does not

`crates/libcrc-cabi/src/lib.rs:160-163`

```rust
let mut len = 0usize;
while unsafe { *input_str.add(len) } != 0 {
    len += 1;
}
```

The shim scans for **NUL only**. The original stops at **NUL, CR, LF, or `*`**
(`source/src/nmea-chk.c:73`):

```c
while ( *ptr  &&  *ptr != '\r'  &&  *ptr != '\n'  &&  *ptr != '*' ) checksum ^= *ptr++;
```

and the function's own header comment says so in as many words: the calculation stops at
a linefeed, carriage return, `*`, or end of string. A caller handing over a buffer that
ends in `*` is therefore inside the C's contract — and that is what every real NMEA 0183
sentence looks like: `$GPGLL,4916.45,N,...*31`. The shim walks straight past that `*`
hunting for a NUL the caller never promised.

### Proof — guard page, one probe, two libraries

The probe reserves two pages, commits only the first, and places the sentence so its
final delimiter is the **last byte of the committed page**. A read one byte further
faults. It is compiled twice: once against `oracle/lib/libcrc.a` (the real C library),
once against `target/release/libcrc.a` (this port's shim). Same source, same compiler,
same flags — only the library differs.

```c
static unsigned char *tail_of_guarded_page(const char *data, size_t n, char **base_out) {
    SYSTEM_INFO si; GetSystemInfo(&si);
    SIZE_T page = si.dwPageSize;
    char *base = (char *)VirtualAlloc(NULL, page * 2, MEM_RESERVE, PAGE_NOACCESS);
    VirtualAlloc(base, page, MEM_COMMIT, PAGE_READWRITE);
    memset(base, 'A', page);
    unsigned char *p = (unsigned char *)(base + page - n);   /* ends AT the page edge */
    memcpy(p, data, n);
    *base_out = base;
    return p;
}
/* case "star": */
unsigned char *p = tail_of_guarded_page("$GPGLL,x*", 9, &base);
checksum_NMEA(p, out);
```

```
gcc -funsigned-char -O0 -I oracle/include probe2.c oracle/lib/libcrc.a  -o p2_orig.exe
gcc -funsigned-char -O0            probe2.c target/release/libcrc.a -o p2_port.exe \
    -lkernel32 -lntdll -luserenv -lws2_32 -ldbghelp -ladvapi32 -lbcrypt
```

```
CASE               | ORIGINAL C libcrc                      | RUST PORT (libcrc-cabi)
-------------------+----------------------------------------+------------------------
star               | exit=0   ok result="04"                | exit=139   <-- SEGV
cr                 | exit=0   ok result="04"                | exit=139   <-- SEGV
lf                 | exit=0   ok result="04"                | exit=139   <-- SEGV
nul                | exit=0   ok result="04"                | exit=0   ok result="04"
len_exact          | exit=0   ok crc_16=0xBB3D              | exit=0   ok crc_16=0xBB3D
len_over           | exit=139                               | exit=139   (parity — both fault)
null_ptr           | exit=0   ok crc_16(NULL,5)=0x0000      | exit=0   ok crc_16(NULL,5)=0x0000
zero_len           | exit=0   ok crc_16(p,0)=0x0000         | exit=0   ok crc_16(p,0)=0x0000
nmea_null_result   | exit=0   returns NULL                  | exit=0   returns NULL
nmea_null_input    | exit=0   returns NULL                  | exit=0   returns NULL
```

exit 139 = SIGSEGV as this shell reports it (`0xC0000005` access violation on Windows).

Three rows — `star`, `cr`, `lf` — are cases where **the C is safe and the port crashes**.
Every other row is byte-for-byte parity. Note `len_over` in particular: a length larger
than the real buffer faults in *both*, because the C loop reads exactly `num_bytes` too.
That one is genuine parity and not a regression, which is what makes the three that
differ meaningful rather than noise.

### It contradicts a claim already in the repo

`UNSAFE.md`, U-2:

> **If violated:** reads past the buffer until it finds a zero byte — again identical to
> the original's behaviour.

It is not identical. The original stops at four byte values; the shim stops at one. This
sentence should be corrected whether or not the code changes. A reviewer who checks it
will find it false in about ninety seconds, and a false safety claim is the most damaging
kind of error in a submission that is otherwise this disciplined about evidence.

### Fix

Scan for the same delimiter set the C does. This changes no output:
`libcrc_rs::checksum_nmea` already applies
`take_while(|b| !matches!(b, 0 | b'\r' | b'\n' | b'*'))`, so handing it a slice that was
truncated earlier feeds it exactly the bytes it was going to keep anyway.

```rust
// The C stops at NUL, CR, LF or '*' (src/nmea-chk.c:73). Scanning for NUL alone reads
// further than the original ever would: a buffer ending in '*' with no terminator is
// inside the C's documented contract.
let mut len = 0usize;
while !matches!(unsafe { *input_str.add(len) }, 0 | b'\r' | b'\n' | b'*') {
    len += 1;
}
```

Boundary cases still agree with the C:

| input | C result | after fix |
|---|---|---|
| `"$*"` | skips `$`, loop stops at `*` → `0x00` | `len == 1`, slice `b"$"`, prefix stripped → `0x00` |
| `"*..."` | loop stops immediately → `0x00` | `len == 0`, empty slice → `0x00` |
| `"\0"` | not `$`, loop stops → `0x00` | `len == 0` → `0x00` |
| `"$GPGLL,x*7C"` | `0x04` | `0x04` |

Then correct `UNSAFE.md` U-2 to name the delimiter set, and add the three guard-page
cases to the suite so the regression cannot return.

---

## F2 — MEDIUM — `--check` is quadratic in the number of distinct paths

`crates/libcrc-cli/src/check.rs:176-179`

```rust
fn group_by_path(entries: &[Entry]) -> Vec<Vec<&Entry>> {
    let mut groups: Vec<Vec<&Entry>> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|g| g[0].path == entry.path) {
```

A linear scan of every group created so far, for every entry. With N distinct paths that
is N²/2 full string comparisons.

Measured on the release build. Each manifest has N lines; the test manifests name N
**distinct** non-existent files (so per-entry I/O is a constant failed open), the control
manifests name **one** path N times (one group, so the quadratic term vanishes):

| N entries | N distinct paths | 1 distinct path (control) | grouping cost = difference |
|---|---|---|---|
| 16 000 | 1 438 ms | — | — |
| 32 000 | 3 567 ms | 264 ms | 3 303 ms |
| 64 000 | **13 097 ms** | 367 ms | 12 730 ms |

12 730 / 3 303 = **3.85 for a doubling of N** — quadratic to within measurement noise. At
64 000 entries the tool is 36× slower than the same number of entries in one bucket.

A long-path variant (185-char paths sharing a deep prefix, which is what a real tree
looks like) costs 2 341 ms at only 16 000 entries, versus 1 438 ms for short paths at the
same N — the comparison constant matters too.

This is not a crash. It matters because manifests are exactly the file that gets large —
a backup catalogue, a release artefact list — and because in any pipeline that verifies a
manifest fetched over the network, N is attacker-chosen.

### Fix

Group with a hash map. `check.rs` already uses `std`, so this costs nothing:

```rust
use std::collections::HashMap;

fn group_by_path(entries: &[Entry]) -> Vec<Vec<&Entry>> {
    let mut order: Vec<&str> = Vec::new();
    let mut by_path: HashMap<&str, Vec<&Entry>> = HashMap::new();
    for entry in entries {
        by_path
            .entry(entry.path.as_str())
            .or_insert_with(|| {
                order.push(entry.path.as_str());
                Vec::new()
            })
            .push(entry);
    }
    order
        .into_iter()
        .map(|p| by_path.remove(p).unwrap_or_default())
        .collect()
}
```

First-mention ordering — pinned by the existing test
`entries_are_grouped_by_path_in_first_mention_order` — is preserved by `order`.

---

## F3 — MEDIUM — `--check` memory is unbounded in the manifest size

`crates/libcrc-cli/src/check.rs:135-146` reads the whole manifest with
`fs::read_to_string` (or `read_to_string` on stdin for `--check -`), then materialises
every line into an owned `Entry { algo, expected, path: String }`.

Peak working set, sampled every 10 ms while the release binary ran, output to `NUL`:

| command | input size | peak RSS |
|---|---|---|
| `crc --all big8m.bin` | 8 MiB | **2.9 MB** |
| `crc --all big64m.bin` | 64 MiB | **2.9 MB** |
| `crc --check ctl_64000.txt` | 2.1 MiB manifest | 10.5 MB |
| `crc --check mem_64m.txt` | 64.2 MiB manifest | **226.3 MB** |

The hashing path is exactly as advertised: 8 MiB and 64 MiB both cost 2.9 MB. `--check`
is 3.5× the manifest size and grows without bound — the text, plus a `String` per line,
plus the `Vec<Entry>`.

`crc --check -` fed by a stream nobody bounds (`some-producer | crc --check -`) therefore
grows until the allocator fails, and **a Rust allocation failure aborts**: it goes through
`alloc::handle_alloc_error`, not through `CliError`, so it is the one input-driven failure
in this binary that does not produce the tidy one-line diagnostic every other path does.

I did not run this machine out of memory to demonstrate the abort, and deliberately so.
The measured linear growth above is the evidence; the abort follows from the allocation
contract.

This also makes `--help` overclaim:

> Input is read in 64 KiB chunks … so memory use is constant however large FILE is

True of FILE. Not true of MANIFEST.

### Fix

Stream the manifest. `parse_manifest` already works one line at a time; it only needs a
`BufRead` source instead of a `&str`:

```rust
let reader: Box<dyn BufRead> = if manifest == "-" {
    Box::new(io::stdin().lock())
} else {
    Box::new(io::BufReader::new(
        fs::File::open(manifest).map_err(|e| CliError::io(manifest, e))?,
    ))
};
for (offset, line) in reader.lines().enumerate() { /* same body */ }
```

Memory becomes O(entries) rather than O(entries + file), which removes the 3.5×
multiplier and the whole-file copy. If a hard bound is wanted, cap the entry count and
return a `CliError::Manifest`. Either way, soften the `--help` sentence to say which
input it is talking about.

---

## F4 — LOW — duplicate manifest entries are not de-duplicated

`crates/libcrc-cli/src/check.rs:47-48` builds one `Algo` per manifest line in a group and
`hash.rs:47` instantiates one `Digest` per `Algo`. A manifest that names the same
`(path, algorithm)` pair N times folds the file through N identical digests.

Release build, N duplicate `crc_32` lines against the same **existing** 8 MiB file:

| manifest | size | wall |
|---|---|---|
| `crc -a crc_32 big8m.bin` (baseline) | — | 201 ms |
| N=1 | 30 B | 210 ms |
| N=50 | 1.5 KB | 1 146 ms |
| N=200 | 6 KB | 4 562 ms |
| N=400 | **12 KB** | **10 838 ms** |

Linear in N, as expected — but it means a 12 KB input buys 10.8 s of CPU, roughly a 900×
byte-for-byte amplification, and it scales with the size of the file named rather than
the size of the manifest.

**Honest caveat:** `md5sum -c` does the same thing (worse, in fact — it re-reads the file
per line). So this is not below the bar set by the reference tool. What makes it worth
listing is the asymmetry inside this program: `args.rs:172` de-duplicates
`--algo` (`if !into.contains(&algo)`, so `crc --all --all` computes 13 and not 26) while
`--check` does not. One of the two behaviours is wrong.

### Fix

De-duplicate `(path, algo)` when building each group, and fan the single computed value
back out to every manifest line that asked for it, so the OK/FAILED output stays
line-for-line with the manifest.

---

## F5 — LOW — the directory pre-check breaks Windows device paths

`crates/libcrc-cli/src/hash.rs:38-42` calls `fs::metadata` before opening, to give a
better message than the OS does for a directory. On Windows that call fails for device
paths that `File::open` handles fine. Probed directly:

```
NUL   metadata=Err("Incorrect function. (os error 1)")  open=Ok  first_read=Ok(0)
```

So the CLI reports:

```
$ crc NUL
crc: NUL: Incorrect function. (os error 1)
exit=1
```

where without the pre-check it would have read zero bytes and printed the seed values —
which is what `crc /dev/null` does on POSIX. Same for `COM1` and `\\.\NUL`.

**This is a genuine trade-off, not a clear bug.** The same pre-check is what stops
`crc CON` from blocking forever on console input: `crc CON` currently exits 1 with
"The parameter is incorrect. (os error 87)", whereas a bare `File::open("CON")` blocks
(my own probe hit the 3-minute timeout doing exactly that). Removing the guard would
trade a cosmetic Windows-device regression for a hang.

### Options, in order of preference

1. Leave the code and say so in `--help`: on Windows, device paths are not accepted;
   use `-` for stdin. Cheapest, and honest.
2. Fall back to `File::open` when `metadata` fails with `ErrorKind::Uncategorized`,
   and accept that `crc CON` then waits on the console the same way `crc -` does.

I would take option 1 at this stage of the clock.

---

## F6 — NOTE — one panic-capable slice in the CLI, not reachable from input

`crates/libcrc-cli/src/hash.rs:56`

```rust
Ok(read) => {
    let chunk = &buf[..read];
```

If a `Read` implementation returns `read > buf.len()`, this slices out of range and
panics. `Read` is a safe trait, so an over-reporting impl is a bug rather than UB, and
the only implementations reached here are `fs::File` and `StdinLock`, neither of which
does it. **Not reachable from hostile input** — listed only because the crate's own
module doc says "No panics on input", and a one-line change makes that unconditional:

```rust
Ok(read) => {
    let chunk = buf.get(..read).ok_or_else(|| {
        io::Error::other("reader reported more bytes than the buffer holds")
    })?;
```

---

## F7 — NOTE — `from_raw_parts` has a stricter precondition than the C loop

`crates/libcrc-cabi/src/lib.rs:40`

```rust
unsafe { slice::from_raw_parts(ptr, len) }
```

`slice::from_raw_parts` documents that `len * size_of::<T>()` must be no larger than
`isize::MAX`. The C loop `for (a=0; a<num_bytes; a++)` has no such requirement — it
simply reads until it walks off a mapped page. So `crc_16(ptr, usize::MAX)` is immediate
UB in the shim (the compiler is entitled to assume it cannot happen) where in the C it is
merely a read that faults.

Observably today, both fault — that is the `len_over` row in F1's matrix, and I did not
find a case where the difference is visible. It is a contract gap, not a demonstrated
miscompile, and I am not claiming more than that.

**Fix:** add the bound to `UNSAFE.md` U-1's precondition list ("`len` must not exceed
`isize::MAX`; this is stricter than the C loop, which has no such limit"). A runtime
check is not worth it in a shim whose whole purpose is to be a thin adapter.

---

## F8 — NOTE — inconsistent diagnostics for a directory

`hash::checksums` has a directory pre-check; `read_manifest` does not:

```
$ crc adir
crc: adir: is a directory                          exit=1
$ crc --check adir
crc: adir: Access is denied. (os error 5)          exit=1
```

Same mistake, two different messages, one of them unhelpful. Reusing the pre-check in
`read_manifest` costs three lines.

Related and equally cosmetic: `--check` accepts a value wider than the algorithm can
produce and reports a mismatch rather than a manifest error —
`crc_8  0x1FF  one.bin` prints `crc_8 FAILED … manifest says 0x1FF`. Rejecting
`expected > algo max` at parse time would be a better diagnostic, but nothing breaks.

---

## Panic audit — `crates/libcrc-rs`

The brief called out table indices specifically, and the new `slice8.rs` in particular,
where indices are computed differently per lane. Here is every index in the crate.

### `lib.rs` — the byte-at-a-time family

All six are masked to `& 0xFF` or come from a `u8`:

| line | index expression | why it is in `0..=255` |
|---|---|---|
| 204 | `SHT75_CRC_TABLE[(byte ^ crc) as usize]` | `u8 ^ u8` is a `u8` |
| 210 | `TABLE_16[((crc ^ byte as u16) & 0x00FF) as usize]` | masked |
| 216 | `TABLE_32[((crc ^ byte as u32) & 0x0000_00FF) as usize]` | masked |
| 222 | `TABLE_CCITT[(((crc >> 8) ^ byte as u16) & 0x00FF) as usize]` | masked |
| 228 | `TABLE_KERMIT[((crc ^ byte as u16) & 0x00FF) as usize]` | masked |
| 234 | `TABLE_DNP[((crc ^ byte as u16) & 0x00FF) as usize]` | masked |
| 240 | `TABLE_64[(((crc >> 56) ^ byte as u64) & 0xFF) as usize]` | masked |

Table construction (lines 84, 105, 128, 151) indexes `table[index]` inside
`while index < 256` over a `[T; 256]`. In range by construction, and evaluated at compile
time — a failure would be a build error, not a runtime panic.

### `slice8.rs` — the eight per-lane indices, which is where the brief expected trouble

`run_u8` (296-303) — `crc ^ b[0]` is `u8 ^ u8`; `b[1..7]` are `u8`. All eight lane
indices in `0..=255`. Outer indices `SLICES_8[0..=6]` are literals against
`[[u8; 256]; DERIVED]` where `DERIVED == 7`.

`run_reflected_u16` (320-328) — `x: u16`.
`(x & 0x00FF)` masked. **`(x >> 8) as usize` is not masked** — but `x` is `u16`, so
`x >> 8` is at most `0xFF`. In range. `b[2..7]` are `u8`.

`run_forward_u16` (339-347) — mirror image: `(x >> 8)` unmasked but `u16`-bounded,
`(x & 0x00FF)` masked.

`run_reflected_u32` (356-364) — `x: u32`. Three masked lanes
(`x & 0xFF`, `(x >> 8) & 0xFF`, `(x >> 16) & 0xFF`) and one **unmasked**:
`SLICES_32[3][(x >> 24) as usize]`. `x` is `u32`, so `x >> 24` is at most `0xFF`. In
range.

So there are **three unmasked indices** in `slice8.rs`, all of the form `value >> (W-8)`
on a `W`-bit unsigned integer. Each is provably `<= 0xFF` from the type alone; none is a
latent panic. They are worth naming explicitly because the crate documentation says
"every index is masked or comes from a `u8`", and a future widening of `x` to a type
larger than the shift accounts for would break exactly these three and nothing else.

Table construction (212, 229, 246, 263) uses `slices[k - 1][i]` guarded by
`if k == 0 { t0[i] } else { … }`, so `k - 1` never underflows, and `k < DERIVED`,
`i < 256` bound both dimensions. `const fn`, so again a build error rather than a panic.

The eight message bytes come from `data.split_first_chunk::<8>()`, which yields a
`&[u8; 8]` — fixed-size, so `b[0..7]` carry no bounds check and no panic path at all.

### `combine.rs`

`matrix[i]` (90), `squared[i]` (104), `operator[i]` (165) are all inside `while i < $bits`
over `[$ty; $bits]`. `1 << i` for `i < $bits` cannot overflow the type. `advance` shifts
`len >>= 1` and terminates in at most 64 iterations for any `usize`, including
`usize::MAX` — exercised below.

### Arithmetic

The only `+`/`-` operations outside tests in the whole port are loop counters
(`index += 1`, `bit += 1`, `i += 1`, `k += 1`), each bounded by a `while` condition, and
`STRIDE - 1` / `k - 1`, both const-evaluated or explicitly guarded. Every shift amount is
a literal strictly less than the operand's width. There is no user-influenced arithmetic
anywhere in `libcrc-rs`.

The single unbounded counter in the repository is `len += 1` in the `checksum_NMEA` NUL
scan (F1) — it would overflow only after `usize::MAX` iterations, which is unreachable
because the process faults long before.

---

## What was actually run

### The port, debug build, both feature configurations

An adversarial driver (`scratchpad/adv`) linked against `libcrc-rs` by path and built
with overflow checks on:

```
PASS lengths 0..=1024 x 6 fills          (0x00, 0xFF, 0x01, 0x80, '$', '*')
PASS large buffers up to 4 MiB           (64 KiB±1, 1 MiB±1, 4 MiB; zeros, ones, ramp)
PASS all 1- and 2-byte messages (65 792 cases)
PASS update_crc_* full byte sweep at extreme states
PASS combine at usize edges incl. usize::MAX
PASS combine identity
PASS digests with 1000 empty chunks around 1 MB
PASS Crc32Hasher incl. write_u64/write_usize/write_i8
PASS checksum_nmea adversarial
ALL PASS  acc=0xC7AFFB84520731FB
exit=0
```

71 960 distinct buffers × 13 one-shot functions = **935 480 calls**, plus the incremental
sweep and 110 `combine` calls at `usize` edges. Specifically covered, as the brief asked:
empty, 1 byte, all-`0x00`, all-`0xFF`, and 64 KiB+ — at every length from 0 to 1024 and
on either side of both slice-by-8 crossovers (8 and 16) and every 8-byte stride boundary.

`combine` was called with `len_b ∈ {0, 1, 2, 7, 8, 255, usize::MAX/2, usize::MAX-1,
usize::MAX, 1<<63}`. No overflow, no non-termination — binary exponentiation gives at
most 64 squarings.

Run twice, from two genuinely different binaries:

```
$ md5sum adv_noslice8.exe adv_slice8.exe
83f1759dc6ffbf617113c27a8d5f3316 *adv_noslice8.exe   5 274 070 bytes
afe9a9abaef9f3363a49c1b4bdae58b5 *adv_slice8.exe     5 310 527 bytes
```

Different binaries (36 457 bytes apart, consistent with the documented 23 296 B of
derived tables plus the folded loops), **identical accumulator
`0xC7AFFB84520731FB`**. The accelerated and unaccelerated paths agree bit-for-bit across
every one of those 935 480 calls.

The repository's own suite also passes in debug — 28 + 21 + 68 + 7 + 14 = **138 tests,
0 failures**, with overflow checks on.

### The CLI, debug build

Every case below was run against `target/debug/crc.exe`. **No panic in any of them.**

| case | exit | stderr |
|---|---|---|
| no args, empty stdin | 0 | — (prints seed values for `-`) |
| missing file | 1 | `crc: nosuchfile.bin: The system cannot find the file specified. (os error 2)` |
| directory as file | 1 | `crc: adir: is a directory` |
| empty file | 0 | — |
| 1-byte file | 0 | — |
| all-`0x00` 64 KiB, `--all` | 0 | — |
| all-`0xFF` 64 KiB, `--all` | 0 | — |
| binary 0x00..0xFF, `--all` | 0 | — |
| 8 MiB / 64 MiB, `--all` | 0 | — (2.9 MB RSS) |
| binary on stdin | 0 | — |
| bad `--algo` name | 2 | `unknown algorithm 'bogus' (run 'crc --list' …)` |
| `--algo` with no value | 2 | `--algo needs a value` |
| `--algo ,` (empty list) | 2 | `--algo needs at least one algorithm name` |
| `--algo=` (empty attached) | 2 | `--algo needs at least one algorithm name` |
| unknown long flag | 2 | `unrecognised option '--frobnicate'` |
| unknown short flag | 2 | `unrecognised option '-z'` |
| `-é` (multi-byte, would panic on byte slicing) | 2 | `unrecognised option '-é'` |
| `-aé` | 2 | `unknown algorithm 'é'` |
| `--=x` | 2 | `unrecognised option '--=x'` |
| `-- --all -a` (dashes as filenames) | 1 | two "cannot find the file" lines |
| 8 000-char filename | 1 | `… The filename, directory name, or volume label syntax is incorrect. (os error 123)` |
| `--algo` spec of 1 000 comma-separated names | 0 | — (de-duplicated to one) |
| missing + existing file together | 1 | error for the missing one; the other is still hashed |
| file held with an exclusive `FileShare.None` handle | 1 | `… The process cannot access the file because it is being used by another process. (os error 32)` |
| `NUL`, `CON`, `COM1`, `\\.\NUL` | 1 | clean OS error, no hang — see F5 |
| `--all big8m.bin \| head -1` | 0 | broken pipe correctly treated as success |
| `--help`, `--list`, `-V` | 0 | — |

`--check`, all against the debug binary:

| case | exit | behaviour |
|---|---|---|
| well-formed manifest | 0 | `OK` per line |
| mismatched value | 1 | `FAILED` + expected/actual on stderr + summary warning |
| garbage line | 1 | `m_garbage.txt:1: expected '<algorithm>  <value>  <path>', found "garbage"` |
| two fields only | 1 | line number reported |
| empty manifest | 1 | `m_empty.txt:0: no checksum lines found` |
| comments/blank only | 1 | same |
| value that overflows `u64` | 1 | reported as a malformed line, not a panic |
| value wider than the algorithm (`crc_8 0x1FF`) | 1 | `FAILED` (see F8) |
| manifest names a missing file | 1 | `FAILED open or read` + `1 listed entry could not be read` |
| manifest names a directory | 1 | `FAILED open or read` + `is a directory` |
| manifest itself missing | 1 | clean OS error |
| manifest itself is a directory | 1 | `Access is denied. (os error 5)` (see F8) |
| manifest is binary / non-UTF-8 | 1 | `stream did not contain valid UTF-8` |
| manifest path is `-` with empty stdin | 1 | hashes empty stdin, reports mismatch |
| manifest held under an exclusive handle | 1 | clean OS error |

The parse errors all carry a line number, which is the thing that makes a malformed
manifest debuggable rather than merely rejected.

---

## What I did not do

* **I did not run this machine out of memory** to demonstrate the F3 abort. The linear
  growth (2.9 MB → 226.3 MB) is the evidence; the abort follows from Rust's allocation
  contract, and I have labelled it as an inference rather than an observation.
* **No Miri, no ASan, no Valgrind.** Miri cannot execute the `libcrc-cabi` FFI boundary
  that F1 lives at, and there is no nightly toolchain on this machine, which the kickoff
  brief already records. The guard-page probe is the substitute, and for this particular
  bug it is a better one — it exercises the real compiled library through the real C ABI.
* **`crc_sick` was not differentially fuzzed here**; that is `tests/parity/`'s job and it
  is outside this review's scope. This review covers crash-safety, not correctness.
* **I changed no code.** `git status` shows only this file. The repository is exactly as
  green as it was: 138 tests pass in debug, and nothing was touched to make that true.

---

## If only one thing gets fixed

F1. It is a segfault the C original does not have, on input the C original explicitly
documents as valid, and the repository currently contains a written claim that it does
not exist. The code fix is four tokens; the `UNSAFE.md` correction matters just as much.
