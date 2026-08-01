# Concurrency soak — a latent data race in the original C library

Run 2026-08-01 on x86_64 (Windows, gcc 16.1.0, `-O2 -funsigned-char`).
Reproduce: `gcc -O2 -funsigned-char -Iinclude soak.c crc16_instrumented.c -o soak.exe`
then run the binary repeatedly. Each process starts 16 threads that park on a gate and
call `crc_16()` simultaneously from a cold start.

## The defect

libcrc guards its lazy table build with a plain, non-atomic `bool`:

```c
static bool     crc_tab16_init = false;    /* src/crc16.c:40 */
static uint16_t crc_tab16[256];            /* src/crc16.c:41 */

uint16_t crc_16( const unsigned char *input_str, size_t num_bytes ) {
        if ( ! crc_tab16_init ) init_crc16_tab();   /* :58, also :86, :109 */
        ... reads crc_tab16 ...
}
```

There is **no synchronisation anywhere in the library** — grepping `src/` and `include/`
for mutex/atomic/pthread/lock/`_Thread`/volatile/once yields exactly one hit, and it is a
comment (`crcdnp.c:99`), not code.

## Measured result

An instrumented copy of the oracle counts how many times the "run once" initialiser
actually executes:

| Metric | Result |
|---|---|
| Processes run | 40 |
| Processes where the initialiser ran **more than once** | **30 (75%)** |
| Worst observed concurrent executions | **3** |
| Total initialiser executions (40 expected) | **85 — 2.1×** |
| Processes returning a wrong CRC | 0 |

So in 75% of cold starts, two or three threads were inside `init_crc16_tab()`
simultaneously, writing `crc_tab16[256]` while other threads were reading it.

## Why zero wrong answers, and why it is still a real bug

Reported honestly, because the nuance matters:

- **No wrong values were observed on x86**, for two reasons. Every racing thread writes
  *identical* table content, so a torn interleaving still lands on the right bytes; and
  x86-64 is TSO, which does not reorder store-store, so `crc_tab16_init = true` cannot
  become visible before the table writes it follows.
- **It is nonetheless undefined behaviour.** C11 §5.1.2.4 makes an unsynchronised write
  and read of the same non-atomic object from different threads a data race, full stop.
  A compiler is entitled to assume it does not happen.
- **It is genuinely dangerous on weakly-ordered hardware.** On ARM, RISC-V or POWER there
  is no store-store ordering guarantee and no barrier here, so a second thread can observe
  `crc_tab16_init == true` while `crc_tab16` is still stale, and silently return a wrong
  checksum. libcrc's primary audience is embedded/serial/networking — i.e. ARM.
- **This is why it survived since 1999:** it is close to unobservable on the platform most
  people test on.

Affected tables: `crc_tab` (CRC-8/kermit), `crc_tab16`, `crc_tabccitt`, `crc_tabdnp`,
`sht75_crc_table`. Not `crc_tab32`/`crc_tab64`, which `precalc/` generates at build time.

## How the Rust port eliminates it

Not by adding a mutex — by deleting the problem. Every table is a `const fn` evaluated by
the compiler into `.rodata`, so there is no runtime initialiser to race, no mutable global,
and no guard flag. The race is impossible by construction, and the port is `no_std` and
allocation-free, so it costs nothing on the embedded targets that are most exposed.
