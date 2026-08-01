/*
 * probe_codegen.c — two CONTROL EXPERIMENTS, written to attack our own results.
 *
 * The main sweep produced two large wins for the port. Before publishing either
 * as "the Rust port is faster", they have to survive the obvious objection:
 * is this the *language*, or is it just that libcrc's C happens to be written
 * in a way one compiler handles badly?
 *
 * CONTROL 1 — crc_sick.
 *   The port is ~3.2x faster than C on 1 MiB buffers. Disassembly shows gcc
 *   emitting a data-dependent branch (`test %ax,%ax; jns`) where LLVM emits a
 *   branchless `cmovns`. With pseudorandom data that branch is ~50%
 *   unpredictable, so C eats a mispredict on roughly every other byte.
 *   This probe implements the SAME algorithm in C twice — once exactly as
 *   libcrc writes it, once with the conditional expressed as an arithmetic
 *   mask — and compiles both with the same gcc -O3. If the branchless C
 *   variant lands near the Rust number, the win belongs to the CODEGEN, not to
 *   Rust, and must be reported that way.
 *
 * CONTROL 2 — checksum_NMEA.
 *   The port is ~3.5x faster. libcrc finishes with
 *   `snprintf(result, 3, "%02hhX", checksum)` (src/nmea-chk.c); the port's C
 *   ABI shim indexes a 16-entry hex table. This probe times the identical XOR
 *   scan followed by each of the two formatting choices, in C, so the cost can
 *   be attributed to the printf rather than to the language.
 *
 * This file never touches oracle/. It re-implements the algorithms locally so
 * the original stays unmodified. Correctness of each variant is asserted
 * against the others before any timing is reported.
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CRC_POLY_SICK 0x8005
#define SAMPLES 200

static LARGE_INTEGER g_freq;
static volatile uint64_t g_sink;

static double ns_since(LARGE_INTEGER t0) {
    LARGE_INTEGER t1;
    QueryPerformanceCounter(&t1);
    return (double)(t1.QuadPart - t0.QuadPart) * 1e9 / (double)g_freq.QuadPart;
}

/* --- CONTROL 1 ---------------------------------------------------------- */

/* Exactly libcrc's formulation (src/crcsick.c). */
static uint16_t sick_branchy(const unsigned char *p, size_t n) {
    uint16_t crc = 0, short_c, short_p = 0, low, high;
    size_t a;
    for (a = 0; a < n; a++) {
        short_c = 0x00FF & (uint16_t)*p;
        if (crc & 0x8000) crc = (crc << 1) ^ CRC_POLY_SICK;
        else              crc = crc << 1;
        crc ^= (short_c | short_p);
        short_p = short_c << 8;
        p++;
    }
    low = (crc & 0xFF00) >> 8;
    high = (crc & 0x00FF) << 8;
    return (uint16_t)(low | high);
}

/* Same algorithm, conditional expressed as an arithmetic mask so gcc has no
 * branch to mispredict. Bit-identical output, asserted below. */
static uint16_t sick_branchless(const unsigned char *p, size_t n) {
    uint16_t crc = 0, short_c, short_p = 0, low, high;
    size_t a;
    for (a = 0; a < n; a++) {
        uint16_t mask = (uint16_t)(0u - (uint16_t)((crc >> 15) & 1u));
        short_c = 0x00FF & (uint16_t)*p;
        crc = (uint16_t)((crc << 1) ^ (CRC_POLY_SICK & mask));
        crc ^= (short_c | short_p);
        short_p = short_c << 8;
        p++;
    }
    low = (crc & 0xFF00) >> 8;
    high = (crc & 0x00FF) << 8;
    return (uint16_t)(low | high);
}

/* --- CONTROL 2 ---------------------------------------------------------- */

static unsigned char nmea_scan(const unsigned char *p) {
    unsigned char sum = 0;
    if (*p == '$') p++;
    while (*p && *p != '\r' && *p != '\n' && *p != '*') sum ^= *p++;
    return sum;
}

/* libcrc's formatting choice. */
static unsigned char *nmea_snprintf(const unsigned char *in, unsigned char *out) {
    unsigned char sum;
    if (in == NULL || out == NULL) return NULL;
    sum = nmea_scan(in);
    snprintf((char *)out, 3, "%02hhX", sum);
    return out;
}

/* The port's C ABI shim's formatting choice, transliterated back into C. */
static unsigned char *nmea_hextab(const unsigned char *in, unsigned char *out) {
    static const char HEX[] = "0123456789ABCDEF";
    unsigned char sum;
    if (in == NULL || out == NULL) return NULL;
    sum = nmea_scan(in);
    out[0] = (unsigned char)HEX[sum >> 4];
    out[1] = (unsigned char)HEX[sum & 0x0F];
    out[2] = 0;
    return out;
}

/* --- harness ------------------------------------------------------------ */

static int cmp_double(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static void report(const char *label, double *v, int n, double per) {
    qsort(v, (size_t)n, sizeof(double), cmp_double);
    printf("%-34s p50=%9.3f  p90=%9.3f  p99=%9.3f  min=%9.3f\n", label,
           v[(int)(0.50 * n)] / per, v[(int)(0.90 * n)] / per,
           v[(int)(0.99 * n) < n ? (int)(0.99 * n) : n - 1] / per, v[0] / per);
}

int main(void) {
    const size_t N = 1048576;
    unsigned char *buf = (unsigned char *)malloc(N);
    unsigned char nmea[257], out[8];
    double *v = (double *)malloc(sizeof(double) * SAMPLES);
    uint64_t s = 0x2026070118000000ull;
    size_t i;
    int r;

    QueryPerformanceFrequency(&g_freq);
    if (!buf || !v) return 3;

    for (i = 0; i < N; i++) {
        uint64_t x = s;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17; s = x;
        buf[i] = (unsigned char)(0x21 + (x % 93));
        if (buf[i] == '*') buf[i] = '+';
    }
    for (i = 0; i < 256; i++) nmea[i] = buf[i];
    nmea[256] = 0;

    /* Correctness first. A faster wrong answer is not a result. */
    if (sick_branchy(buf, N) != sick_branchless(buf, N)) {
        fprintf(stderr, "FATAL: sick variants disagree\n");
        return 4;
    }
    {
        unsigned char a[8], b[8];
        nmea_snprintf(nmea, a);
        nmea_hextab(nmea, b);
        if (memcmp(a, b, 3) != 0) {
            fprintf(stderr, "FATAL: nmea variants disagree (%s vs %s)\n", a, b);
            return 4;
        }
    }
    printf("# variants verified bit-identical before timing\n");
    printf("# gcc, same -O3 -funsigned-char, same machine, same clock\n\n");

    printf("== CONTROL 1: crc_sick over 1 MiB, ns/byte ==\n");
    for (r = 0; r < SAMPLES; r++) {
        LARGE_INTEGER t0; QueryPerformanceCounter(&t0);
        g_sink ^= sick_branchy(buf, N);
        v[r] = ns_since(t0);
    }
    report("C, libcrc's own branchy form", v, SAMPLES, (double)N);
    for (r = 0; r < SAMPLES; r++) {
        LARGE_INTEGER t0; QueryPerformanceCounter(&t0);
        g_sink ^= sick_branchless(buf, N);
        v[r] = ns_since(t0);
    }
    report("C, same algorithm, mask form", v, SAMPLES, (double)N);

    printf("\n== CONTROL 2: checksum_NMEA over 256 B, ns/call ==\n");
    {
        const int K = 20000;
        int j;
        for (r = 0; r < SAMPLES; r++) {
            LARGE_INTEGER t0; QueryPerformanceCounter(&t0);
            for (j = 0; j < K; j++) { nmea[0] = (unsigned char)('0' + (j % 10));
                                      g_sink ^= (uint64_t)nmea_snprintf(nmea, out)[0]; }
            v[r] = ns_since(t0);
        }
        report("C, libcrc's snprintf form", v, SAMPLES, (double)K);
        for (r = 0; r < SAMPLES; r++) {
            LARGE_INTEGER t0; QueryPerformanceCounter(&t0);
            for (j = 0; j < K; j++) { nmea[0] = (unsigned char)('0' + (j % 10));
                                      g_sink ^= (uint64_t)nmea_hextab(nmea, out)[0]; }
            v[r] = ns_since(t0);
        }
        report("C, hex-table form (as the port)", v, SAMPLES, (double)K);
    }

    fprintf(stderr, "sink=%llu\n", (unsigned long long)g_sink);
    return 0;
}
