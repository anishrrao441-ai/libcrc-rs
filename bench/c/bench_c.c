/*
 * bench_c.c — the C-side benchmark driver for the libcrc -> Rust port.
 *
 * This file is a MEASUREMENT INSTRUMENT, not part of the port. Nothing under
 * crates/ links, references, or depends on it; `cargo build --release` never
 * compiles it. It exists so the C baseline can be measured on this machine,
 * with the same workloads and the same clock as the Rust port.
 *
 * It is deliberately linked against THREE different libraries, unchanged:
 *   c-shipped   ->  oracle/lib/libcrc.a          (the original, as `make` builds it)
 *   c-lto       ->  original sources at -O3 -flto (matches the port's lto=true)
 *   rust-cabi   ->  target/release/libcrc.a      (the PORT, through its C ABI)
 * Because the driver source, the compiler, the flags and the clock are then
 * identical across all three, the only variable left is the library itself.
 * A fourth configuration, `rust-native`, lives in ../rust/src/main.rs and
 * mirrors this file statement for statement; it measures what a Rust consumer
 * actually gets (direct calls, cross-crate LTO, no C ABI in the way).
 *
 * Build `-DNO_UPDATE_API` when linking against the Rust staticlib: the port's
 * C ABI shim exports the 13 symbols the original test suite needs and not
 * libcrc's `update_crc_*` family, so the incremental workloads are skipped there.
 *
 * Output format (stdout), kept small enough to commit verbatim:
 *   #M <key> <value>                      metadata
 *   #W <kind> <algo> <bytes> <k> <n>      workload header
 *   <ns>,<ns>,...                         n raw per-SAMPLE nanosecond timings
 * Each sample times a batch of <k> calls; per-call ns = sample_ns / k.
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "checksum.h"

#ifndef NO_UPDATE_API
/*
 * BUG D-01, seen from the benchmark side.
 *
 * checksum.h:99 declares `update_crc_64_ecma`. It is defined nowhere in src/
 * and is absent from lib/libcrc.a. The function that actually exists is
 * `update_crc_64` (src/crc64.c:103), which the public header never declares.
 * The incremental CRC-64 API is therefore unreachable through the public
 * header; benchmarking it needs the hand-written declaration below.
 */
extern uint64_t update_crc_64(uint64_t crc, unsigned char c);
#endif

/* -------------------------------------------------------------------------
 * Clock. QueryPerformanceCounter — the same clock Rust's std::time::Instant
 * uses on Windows. The `clockres` mode below reports the frequency and the
 * smallest observable non-zero delta, so that identity can be shown
 * empirically rather than asserted.
 * ---------------------------------------------------------------------- */
static LARGE_INTEGER g_freq;

static double ticks_to_ns(long long ticks) {
    return (double)ticks * 1e9 / (double)g_freq.QuadPart;
}

/* Volatile sink: stops the optimiser deleting the batch loop wholesale. The
 * Rust twin uses std::hint::black_box for the same purpose. */
static volatile uint64_t g_sink;

/* -------------------------------------------------------------------------
 * Buffers, filled from a deterministic xorshift stream and mapped into
 * printable ASCII minus '*', so one buffer is legal input for every algorithm
 * including checksum_NMEA (which stops at NUL, CR, LF or '*'). Every algorithm
 * here is a table lookup or a fixed shift chain, so timing is data-independent
 * and the restricted byte range costs no realism.
 * ---------------------------------------------------------------------- */
#define BUF_MAX ((size_t)104857600) /* 100 MiB */
#define MS_SLOTS 4096               /* 4096 * 64 B = a 256 KiB working set */
#define NMEA_MAX ((size_t)1025)

static unsigned char g_nmea_out[8];

static uint64_t xorshift64(uint64_t *s) {
    uint64_t x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *s = x;
    return x;
}

/* -------------------------------------------------------------------------
 * Batch kernels. One dedicated function per algorithm, so the call inside the
 * hot loop is a DIRECT call. A single shared function-pointer kernel would
 * block inlining under LTO, and it would do so on one side only — an unfair
 * handicap. Dispatch happens once, outside the timed region.
 * ---------------------------------------------------------------------- */

typedef uint64_t (*kernel_fn)(unsigned char *buf, size_t n, uint64_t k);

#define ONESHOT(NAME, FN)                                       \
    static uint64_t os_##NAME(unsigned char *buf, size_t n, uint64_t k) { \
        uint64_t acc = 0, i;                                    \
        for (i = 0; i < k; i++) {                               \
            buf[0] = (unsigned char)('0' + (i % 10));           \
            acc ^= (uint64_t)FN(buf, n);                        \
        }                                                       \
        return acc;                                             \
    }

ONESHOT(crc_8, crc_8)
ONESHOT(crc_16, crc_16)
ONESHOT(crc_modbus, crc_modbus)
ONESHOT(crc_32, crc_32)
ONESHOT(crc_64_ecma, crc_64_ecma)
ONESHOT(crc_64_we, crc_64_we)
ONESHOT(crc_ccitt_1d0f, crc_ccitt_1d0f)
ONESHOT(crc_ccitt_ffff, crc_ccitt_ffff)
ONESHOT(crc_xmodem, crc_xmodem)
ONESHOT(crc_kermit, crc_kermit)
ONESHOT(crc_dnp, crc_dnp)
ONESHOT(crc_sick, crc_sick)

/* checksum_NMEA is delimiter-driven, not length-driven, so the workload size is
 * expressed by moving the NUL. Saved and restored around the batch (two stores
 * per batch of k calls) so one buffer serves every size. */
static uint64_t os_nmea(unsigned char *buf, size_t n, uint64_t k) {
    uint64_t acc = 0, i;
    unsigned char saved = buf[n];
    buf[n] = 0;
    for (i = 0; i < k; i++) {
        buf[0] = (unsigned char)('0' + (i % 10));
        acc ^= (uint64_t)(uintptr_t)checksum_NMEA(buf, g_nmea_out);
        acc ^= g_nmea_out[0];
    }
    buf[n] = saved;
    return acc;
}

#ifndef NO_UPDATE_API
/* Incremental / streaming. libcrc's ONLY resumable API is one byte at a time —
 * there is no chunked update taking a slice — so this is what streaming costs. */
#define BYTEWISE(NAME, TY, START, CALL)                                    \
    static uint64_t bw_##NAME(unsigned char *buf, size_t n, uint64_t k) {  \
        uint64_t acc = 0, j;                                               \
        size_t i;                                                          \
        for (j = 0; j < k; j++) {                                          \
            TY crc = (TY)(START);                                          \
            for (i = 0; i < n; i++) crc = CALL(crc, buf[i]);               \
            acc ^= (uint64_t)crc;                                          \
        }                                                                  \
        return acc;                                                        \
    }

BYTEWISE(crc_8, uint8_t, CRC_START_8, update_crc_8)
BYTEWISE(crc_16, uint16_t, CRC_START_16, update_crc_16)
BYTEWISE(crc_32, uint32_t, CRC_START_32, update_crc_32)
BYTEWISE(crc_ccitt, uint16_t, CRC_START_CCITT_FFFF, update_crc_ccitt)
BYTEWISE(crc_kermit, uint16_t, CRC_START_KERMIT, update_crc_kermit)
BYTEWISE(crc_dnp, uint16_t, CRC_START_DNP, update_crc_dnp)
BYTEWISE(crc_64, uint64_t, CRC_START_64_WE, update_crc_64)

static uint64_t bw_crc_sick(unsigned char *buf, size_t n, uint64_t k) {
    uint64_t acc = 0, j;
    size_t i;
    for (j = 0; j < k; j++) {
        uint16_t crc = CRC_START_SICK;
        unsigned char prev = 0;
        for (i = 0; i < n; i++) {
            crc = update_crc_sick(crc, buf[i], prev);
            prev = buf[i];
        }
        acc ^= (uint64_t)crc;
    }
    return acc;
}
#endif /* NO_UPDATE_API */

/* Many small calls: n independent 64-byte one-shot calls cycling over a 256 KiB
 * region — the CRC table stays hot, the data does not fit in L1. This is the
 * shape of real serial/packet traffic, and the shape in which per-call overhead
 * (function call, NULL check, lazy-table-init branch) dominates. */
#define MANYSMALL(NAME, FN)                                                    \
    static uint64_t ms_##NAME(unsigned char *buf, size_t n, uint64_t k) {      \
        uint64_t acc = 0, j;                                                   \
        size_t i;                                                              \
        for (j = 0; j < k; j++) {                                              \
            for (i = 0; i < n; i++) {                                          \
                acc ^= (uint64_t)FN(buf + ((i & (MS_SLOTS - 1)) * 64), 64);    \
            }                                                                  \
        }                                                                      \
        return acc;                                                            \
    }

MANYSMALL(crc_8, crc_8)
MANYSMALL(crc_16, crc_16)
MANYSMALL(crc_32, crc_32)
MANYSMALL(crc_ccitt_ffff, crc_ccitt_ffff)

/* -------------------------------------------------------------------------
 * Workload table — MUST stay identical to the Rust twin.
 * ---------------------------------------------------------------------- */
typedef struct {
    const char *kind;
    const char *algo;
    size_t bytes;    /* buffer length, or call count for manysmall */
    uint32_t samples;
    int nmea;        /* use the NUL-terminated buffer */
    kernel_fn fn;
} workload;

static const size_t SMALL_SIZES[] = {16, 64, 256, 1024};
static const size_t LARGE_SIZES[] = {1048576, 16777216, 104857600};
static const uint32_t LARGE_SAMPLES[] = {200, 60, 25};

typedef struct { const char *algo; kernel_fn fn; } entry;

static const entry ONESHOT_ALL[] = {
    {"crc_8", os_crc_8},                   {"crc_16", os_crc_16},
    {"crc_modbus", os_crc_modbus},         {"crc_32", os_crc_32},
    {"crc_64_ecma", os_crc_64_ecma},       {"crc_64_we", os_crc_64_we},
    {"crc_ccitt_1d0f", os_crc_ccitt_1d0f}, {"crc_ccitt_ffff", os_crc_ccitt_ffff},
    {"crc_xmodem", os_crc_xmodem},         {"crc_kermit", os_crc_kermit},
    {"crc_dnp", os_crc_dnp},               {"crc_sick", os_crc_sick},
};

static const entry ONESHOT_LARGE[] = {
    {"crc_8", os_crc_8},                   {"crc_16", os_crc_16},
    {"crc_32", os_crc_32},                 {"crc_64_we", os_crc_64_we},
    {"crc_ccitt_ffff", os_crc_ccitt_ffff}, {"crc_sick", os_crc_sick},
};

#ifndef NO_UPDATE_API
static const entry BYTEWISE_ALL[] = {
    {"crc_8", bw_crc_8},           {"crc_16", bw_crc_16},
    {"crc_32", bw_crc_32},         {"crc_ccitt", bw_crc_ccitt},
    {"crc_kermit", bw_crc_kermit}, {"crc_dnp", bw_crc_dnp},
    {"crc_64", bw_crc_64},         {"crc_sick", bw_crc_sick},
};
#endif

static const entry MANYSMALL_ALL[] = {
    {"crc_8", ms_crc_8},   {"crc_16", ms_crc_16},
    {"crc_32", ms_crc_32}, {"crc_ccitt_ffff", ms_crc_ccitt_ffff},
};

#define NELEM(a) (sizeof(a) / sizeof((a)[0]))
#define MAX_WORKLOADS 160
static workload g_wl[MAX_WORKLOADS];
static size_t g_nwl;

static void add(const char *kind, const char *algo, size_t bytes, uint32_t samples,
                int nmea, kernel_fn fn) {
    if (g_nwl >= MAX_WORKLOADS) { fprintf(stderr, "workload table overflow\n"); exit(2); }
    g_wl[g_nwl].kind = kind;   g_wl[g_nwl].algo = algo;
    g_wl[g_nwl].bytes = bytes; g_wl[g_nwl].samples = samples;
    g_wl[g_nwl].nmea = nmea;   g_wl[g_nwl].fn = fn;
    g_nwl++;
}

static void build_workloads(void) {
    size_t s, a;
    for (s = 0; s < NELEM(SMALL_SIZES); s++) {
        for (a = 0; a < NELEM(ONESHOT_ALL); a++)
            add("oneshot", ONESHOT_ALL[a].algo, SMALL_SIZES[s], 500, 0, ONESHOT_ALL[a].fn);
        add("oneshot", "checksum_NMEA", SMALL_SIZES[s], 500, 1, os_nmea);
    }
    for (s = 0; s < NELEM(LARGE_SIZES); s++)
        for (a = 0; a < NELEM(ONESHOT_LARGE); a++)
            add("oneshot", ONESHOT_LARGE[a].algo, LARGE_SIZES[s], LARGE_SAMPLES[s], 0,
                ONESHOT_LARGE[a].fn);
#ifndef NO_UPDATE_API
    for (a = 0; a < NELEM(BYTEWISE_ALL); a++)
        add("bytewise", BYTEWISE_ALL[a].algo, 65536, 300, 0, BYTEWISE_ALL[a].fn);
#endif
    for (a = 0; a < NELEM(MANYSMALL_ALL); a++)
        add("manysmall", MANYSMALL_ALL[a].algo, 100000, 100, 0, MANYSMALL_ALL[a].fn);
}

/* -------------------------------------------------------------------------
 * Calibration + measurement. Identical rule in both drivers: grow k (x2, from
 * 1) until one batch takes >= TARGET_BATCH_NS. With a ~100 ns clock tick a
 * 200 us batch bounds the quantisation error at about 0.05%.
 * ---------------------------------------------------------------------- */
#define TARGET_BATCH_NS 200000.0
#define MAX_K 4194304u

static unsigned char *g_buf;
static unsigned char *g_nmea;

static double time_batch(kernel_fn fn, unsigned char *buf, size_t n, uint64_t k) {
    LARGE_INTEGER t0, t1;
    uint64_t acc;
    QueryPerformanceCounter(&t0);
    acc = fn(buf, n, k);
    QueryPerformanceCounter(&t1);
    g_sink += acc;
    return ticks_to_ns(t1.QuadPart - t0.QuadPart);
}

static uint64_t calibrate(kernel_fn fn, unsigned char *buf, size_t n) {
    uint64_t k = 1;
    for (;;) {
        double ns = time_batch(fn, buf, n, k);
        if (ns >= TARGET_BATCH_NS || k >= MAX_K) return k;
        k *= 2;
    }
}

static void run_all(const char *label) {
    size_t w;
    printf("#M impl %s\n", label);
    printf("#M clock QueryPerformanceCounter\n");
    printf("#M qpc_hz %lld\n", (long long)g_freq.QuadPart);
    printf("#M target_batch_ns %.0f\n", TARGET_BATCH_NS);
    fflush(stdout);

    for (w = 0; w < g_nwl; w++) {
        workload *wl = &g_wl[w];
        unsigned char *buf = wl->nmea ? g_nmea : g_buf;
        uint64_t k = calibrate(wl->fn, buf, wl->bytes);
        uint32_t i;
        /* three discarded warm-up batches: page in the buffer, settle the
         * branch predictors, and — on the C side — force the lazy table init,
         * so that cost is measured separately in `firstcall` and never smeared
         * across the steady-state distribution. */
        time_batch(wl->fn, buf, wl->bytes, k);
        time_batch(wl->fn, buf, wl->bytes, k);
        time_batch(wl->fn, buf, wl->bytes, k);

        printf("#W %s %s %llu %llu %u\n", wl->kind, wl->algo,
               (unsigned long long)wl->bytes, (unsigned long long)k, wl->samples);
        for (i = 0; i < wl->samples; i++) {
            double ns = time_batch(wl->fn, buf, wl->bytes, k);
            printf("%s%.0f", i ? "," : "", ns);
        }
        printf("\n");
        fflush(stdout);
    }
    fprintf(stderr, "sink=%llu\n", (unsigned long long)g_sink);
}

/* -------------------------------------------------------------------------
 * Auxiliary modes
 * ---------------------------------------------------------------------- */
static void clockres(void) {
    LARGE_INTEGER a, b;
    long long min_delta = 0x7fffffffffffffffLL;
    int i;
    for (i = 0; i < 2000000; i++) {
        QueryPerformanceCounter(&a);
        QueryPerformanceCounter(&b);
        if (b.QuadPart > a.QuadPart && b.QuadPart - a.QuadPart < min_delta)
            min_delta = b.QuadPart - a.QuadPart;
    }
    printf("#M qpc_hz %lld\n", (long long)g_freq.QuadPart);
    printf("#M min_nonzero_delta_ticks %lld\n", min_delta);
    printf("#M min_nonzero_delta_ns %.1f\n", ticks_to_ns(min_delta));
}

/* Cold first call: the very first CRC in a fresh process. The C library pays
 * init_crc16_tab() — 256 entries x 8 shifts — plus the .bss page faults; the
 * port's table is already in .rodata, so it pays only the fault. */
static void firstcall(const char *algo) {
    LARGE_INTEGER t0, t1;
    uint64_t r = 0;
    unsigned char small[64];
    int i;
    for (i = 0; i < 64; i++) small[i] = (unsigned char)('0' + (i % 10));

    QueryPerformanceCounter(&t0);
    if      (!strcmp(algo, "crc_16"))         r = crc_16(small, 64);
    else if (!strcmp(algo, "crc_32"))         r = crc_32(small, 64);
    else if (!strcmp(algo, "crc_ccitt_ffff")) r = crc_ccitt_ffff(small, 64);
    else if (!strcmp(algo, "crc_kermit"))     r = crc_kermit(small, 64);
    else if (!strcmp(algo, "crc_dnp"))        r = crc_dnp(small, 64);
    else if (!strcmp(algo, "crc_8"))          r = crc_8(small, 64);
    else if (!strcmp(algo, "crc_64_we"))      r = crc_64_we(small, 64);
    else if (!strcmp(algo, "crc_sick"))       r = crc_sick(small, 64);
    else { fprintf(stderr, "unknown algo %s\n", algo); exit(2); }
    QueryPerformanceCounter(&t1);
    g_sink += r;
    printf("%.0f\n", ticks_to_ns(t1.QuadPart - t0.QuadPart));
}

/* RSS profiles: fixed amounts of real work, so the difference in resident set
 * between two binaries is the library plus language runtime, not the workload.
 *
 * The printed value is an XOR fold of every CRC computed, over a deterministic
 * buffer, using the same fold in the Rust twin. All four configurations must
 * therefore print the SAME number — a free end-to-end equivalence check that
 * runs every time the benchmark runs. If they ever disagree, the benchmark is
 * comparing two things that are not the same function and must be stopped. */
static void rss_profile(const char *profile) {
    if (!strcmp(profile, "minimal")) {
        unsigned char small[1024];
        int i;
        for (i = 0; i < 1024; i++) small[i] = (unsigned char)(i & 0x7f);
        g_sink ^= crc_16(small, 1024);
        g_sink ^= crc_32(small, 1024);
    } else if (!strcmp(profile, "work1m") || !strcmp(profile, "work100m")) {
        size_t n = !strcmp(profile, "work1m") ? (size_t)1048576 : (size_t)104857600;
        unsigned char *b = (unsigned char *)malloc(n);
        size_t i;
        if (!b) exit(3);
        for (i = 0; i < n; i++) b[i] = (unsigned char)(i & 0x7f);
        g_sink ^= crc_8(b, n);
        g_sink ^= crc_16(b, n);
        g_sink ^= crc_32(b, n);
        g_sink ^= crc_64_we(b, n);
        g_sink ^= crc_ccitt_ffff(b, n);
        g_sink ^= crc_sick(b, n);
        free(b);
    } else {
        fprintf(stderr, "unknown rss profile %s\n", profile);
        exit(2);
    }
    printf("%llu\n", (unsigned long long)g_sink);
}

int main(int argc, char **argv) {
    QueryPerformanceFrequency(&g_freq);

    if (argc >= 2 && !strcmp(argv[1], "noop")) return 0;
    if (argc >= 2 && !strcmp(argv[1], "clockres")) { clockres(); return 0; }
    if (argc >= 3 && !strcmp(argv[1], "firstcall")) { firstcall(argv[2]); return 0; }
    if (argc >= 3 && !strcmp(argv[1], "rss")) { rss_profile(argv[2]); return 0; }

    if (argc >= 2 && !strcmp(argv[1], "run")) {
        const char *label = (argc >= 3) ? argv[2] : "c";
        uint64_t s = 0x2026070118000000ull; /* the kickoff timestamp, as a seed */
        size_t i;
        g_buf = (unsigned char *)malloc(BUF_MAX);
        g_nmea = (unsigned char *)malloc(NMEA_MAX);
        if (!g_buf || !g_nmea) { fprintf(stderr, "alloc failed\n"); return 3; }
        for (i = 0; i < BUF_MAX; i++) {
            unsigned char v = (unsigned char)(0x21 + (xorshift64(&s) % 93)); /* 0x21..0x7D */
            g_buf[i] = (v == '*') ? (unsigned char)'+' : v;
        }
        for (i = 0; i + 1 < NMEA_MAX; i++) g_nmea[i] = g_buf[i];
        g_nmea[NMEA_MAX - 1] = 0;
        build_workloads();
        run_all(label);
        return 0;
    }

    fprintf(stderr,
            "usage: %s run <label> | noop | clockres | firstcall <algo> | rss <profile>\n",
            argv[0]);
    return 1;
}
