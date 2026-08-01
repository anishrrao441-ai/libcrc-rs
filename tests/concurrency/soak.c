/* Concurrency soak: does libcrc's lazy table initialisation race?
 *
 * libcrc guards its table build with a plain non-atomic bool:
 *
 *     static bool     crc_tab16_init = false;      // src/crc16.c:40
 *     static uint16_t crc_tab16[256];              // src/crc16.c:41
 *     uint16_t crc_16(...) {
 *         if ( ! crc_tab16_init ) init_crc16_tab();   // :58
 *         ... reads crc_tab16 ...
 *     }
 *
 * There is NO synchronisation anywhere in the library (verified by grepping src/
 * and include/ for mutex/atomic/pthread/lock/_Thread/volatile/once).
 *
 * This program starts N threads that all spin on a start gate and then call
 * crc_16() simultaneously from a cold start, and counts how many times the
 * "run once" initialiser actually runs. Anything above 1 means the guard failed
 * and two threads were inside the initialiser at the same time, writing the same
 * table while another thread was reading it.
 *
 * Links an INSTRUMENTED COPY of the oracle. The Rust port is never involved.
 */
#include <stdio.h>
#include <stdint.h>
#include <windows.h>

extern uint16_t crc_16(const unsigned char *input_str, size_t num_bytes);
extern volatile long g_init_count;
extern volatile long g_barrier;

#define THREADS 16
#define EXPECTED 0xBB3D

static volatile long ready = 0;
static volatile long go = 0;
static volatile long wrong = 0;

static DWORD WINAPI worker(LPVOID arg) {
    (void)arg;
    InterlockedIncrement(&ready);
    while (!go) { YieldProcessor(); }          /* all threads released together */
    uint16_t v = crc_16((const unsigned char *)"123456789", 9);
    if (v != EXPECTED) InterlockedIncrement(&wrong);
    return 0;
}

int main(int argc, char **argv) {
    int trial = (argc > 1) ? atoi(argv[1]) : 0;
    HANDLE h[THREADS];

    for (int i = 0; i < THREADS; i++)
        h[i] = CreateThread(NULL, 0, worker, NULL, 0, NULL);

    while (ready < THREADS) { YieldProcessor(); }   /* wait until all are parked */
    InterlockedExchange(&go, 1);                    /* release the herd */

    WaitForMultipleObjects(THREADS, h, TRUE, INFINITE);
    for (int i = 0; i < THREADS; i++) CloseHandle(h[i]);

    printf("trial=%d threads=%d init_ran=%ld wrong_results=%ld\n",
           trial, THREADS, g_init_count, wrong);
    /* exit 1 if the "run once" initialiser ran more than once */
    return (g_init_count > 1) ? 1 : 0;
}
