/*
 * rssrun.c — peak resident-set measurement for a child process on Windows.
 *
 * There is no `/usr/bin/time -v` here, and polling `Get-Process` from
 * PowerShell races short-lived processes: a program that runs for 40 ms can
 * allocate and free its whole working set between two samples. So instead of
 * sampling, this launches the child with CreateProcess, waits for it to exit,
 * and then calls GetProcessMemoryInfo on the still-open process handle. The
 * kernel keeps the accounting alive as long as a handle is open, so the peak
 * counters are the process's true lifetime maxima, not a sample.
 *
 * The same instrument measures BOTH the C baseline and the Rust port, so any
 * bias it carries is common-mode and cancels in the comparison.
 *
 * Validate it before trusting it:
 *     rssrun.exe bench_c.exe rss minimal      -> small
 *     rssrun.exe bench_c.exe rss work100m     -> >= 100 MiB
 * If the second is not about 100 MiB larger than the first, the instrument is
 * lying and the RSS numbers must be withdrawn rather than published.
 *
 * Usage: rssrun.exe <program> [args...]
 * Prints `key=value` lines on stdout.
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <psapi.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    PROCESS_MEMORY_COUNTERS pmc;
    LARGE_INTEGER freq, t0, t1;
    DWORD exit_code = 0;
    char cmdline[4096];
    size_t used = 0;
    int i;

    if (argc < 2) {
        fprintf(stderr, "usage: %s <program> [args...]\n", argv[0]);
        return 1;
    }

    cmdline[0] = 0;
    for (i = 1; i < argc; i++) {
        size_t len = strlen(argv[i]);
        int quote = (strchr(argv[i], ' ') != NULL);
        if (used + len + 4 >= sizeof(cmdline)) {
            fprintf(stderr, "command line too long\n");
            return 1;
        }
        if (i > 1) cmdline[used++] = ' ';
        if (quote) cmdline[used++] = '"';
        memcpy(cmdline + used, argv[i], len);
        used += len;
        if (quote) cmdline[used++] = '"';
        cmdline[used] = 0;
    }

    ZeroMemory(&si, sizeof(si));
    si.cb = sizeof(si);
    ZeroMemory(&pi, sizeof(pi));

    QueryPerformanceFrequency(&freq);
    QueryPerformanceCounter(&t0);

    if (!CreateProcessA(NULL, cmdline, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi)) {
        fprintf(stderr, "CreateProcess failed: %lu\n", (unsigned long)GetLastError());
        return 2;
    }

    WaitForSingleObject(pi.hProcess, INFINITE);
    QueryPerformanceCounter(&t1);
    GetExitCodeProcess(pi.hProcess, &exit_code);

    ZeroMemory(&pmc, sizeof(pmc));
    pmc.cb = sizeof(pmc);
    if (!GetProcessMemoryInfo(pi.hProcess, &pmc, sizeof(pmc))) {
        fprintf(stderr, "GetProcessMemoryInfo failed: %lu\n", (unsigned long)GetLastError());
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        return 3;
    }

    printf("peak_working_set_bytes=%llu\n", (unsigned long long)pmc.PeakWorkingSetSize);
    printf("working_set_at_exit_bytes=%llu\n", (unsigned long long)pmc.WorkingSetSize);
    printf("peak_pagefile_bytes=%llu\n", (unsigned long long)pmc.PeakPagefileUsage);
    printf("page_faults=%lu\n", (unsigned long)pmc.PageFaultCount);
    printf("wall_ns=%.0f\n",
           (double)(t1.QuadPart - t0.QuadPart) * 1e9 / (double)freq.QuadPart);
    printf("exit=%lu\n", (unsigned long)exit_code);

    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    return 0;
}
