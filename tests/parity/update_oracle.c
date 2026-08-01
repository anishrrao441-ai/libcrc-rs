/* Differential oracle for libcrc's incremental update_crc_* family.
 * Links the ORIGINAL C library. Gitignored; never a dependency of the port.
 *
 * Exhaustively enumerates the whole input domain where that is feasible and folds
 * every result into a digest, so a single number certifies the entire function. */
#include <stdio.h>
#include <stdint.h>
#include "checksum.h"
extern uint64_t update_crc_64( uint64_t crc, unsigned char c );

/* FNV-1a style accumulator: order-sensitive, so a permutation cannot alias. */
static uint64_t acc;
static void mix(uint64_t v) { acc ^= v + 0x9E3779B97F4A7C15ull + (acc << 6) + (acc >> 2); }

int main(void) {
    uint32_t crc, byte, prev;

    /* update_crc_8 : 256 x 256 = 65,536 -> EXHAUSTIVE */
    acc = 0;
    for (crc = 0; crc < 256; crc++)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_8((uint8_t)crc, (unsigned char)byte));
    printf("update_crc_8 %016llX\n", (unsigned long long)acc);

    /* update_crc_16 : 65536 x 256 = 16,777,216 -> EXHAUSTIVE */
    acc = 0;
    for (crc = 0; crc < 65536; crc++)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_16((uint16_t)crc, (unsigned char)byte));
    printf("update_crc_16 %016llX\n", (unsigned long long)acc);

    /* update_crc_ccitt : EXHAUSTIVE */
    acc = 0;
    for (crc = 0; crc < 65536; crc++)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_ccitt((uint16_t)crc, (unsigned char)byte));
    printf("update_crc_ccitt %016llX\n", (unsigned long long)acc);

    /* update_crc_kermit : EXHAUSTIVE */
    acc = 0;
    for (crc = 0; crc < 65536; crc++)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_kermit((uint16_t)crc, (unsigned char)byte));
    printf("update_crc_kermit %016llX\n", (unsigned long long)acc);

    /* update_crc_dnp : EXHAUSTIVE */
    acc = 0;
    for (crc = 0; crc < 65536; crc++)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_dnp((uint16_t)crc, (unsigned char)byte));
    printf("update_crc_dnp %016llX\n", (unsigned long long)acc);

    /* update_crc_sick : crc x byte x prev = 4.3e9, too large.
     * Exhaustive over (byte, prev) for every 17th crc -> 3,858 x 65,536 = 252,968,448
     * ... still large; use every 257th crc: 256 x 256 x 256 = 16,777,216 */
    acc = 0;
    for (crc = 0; crc < 65536; crc += 257)
        for (byte = 0; byte < 256; byte++)
            for (prev = 0; prev < 256; prev++)
                mix(update_crc_sick((uint16_t)crc, (unsigned char)byte, (unsigned char)prev));
    printf("update_crc_sick %016llX\n", (unsigned long long)acc);

    /* update_crc_32 : 2^32 crc space. Deterministic stride sweep x all bytes. */
    acc = 0;
    for (uint64_t c = 0; c < 0x100000000ull; c += 0x10001ull)  /* prime-ish stride */
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_32((uint32_t)c, (unsigned char)byte));
    printf("update_crc_32 %016llX\n", (unsigned long long)acc);

    /* update_crc_64 : the symbol that actually exists. NOTE update_crc_64_ecma is
     * DECLARED in checksum.h:99 but defined nowhere, so it cannot be called here. */
    acc = 0;
    for (uint64_t c = 0; c < 0xFFFFFFFFFFFFFFFFull - 0x1000000000000000ull; c += 0x1000100010001ull)
        for (byte = 0; byte < 256; byte++)
            mix(update_crc_64(c, (unsigned char)byte));
    printf("update_crc_64 %016llX\n", (unsigned long long)acc);

    return 0;
}
