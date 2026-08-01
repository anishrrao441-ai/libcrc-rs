/*
 * fuzz/oracle_harness.c — batch oracle for the libcrc differential fuzzer.
 *
 * This file is NOT part of the port. It links the ORIGINAL C libcrc (built into the
 * gitignored oracle/ tree) and exists only so the Rust fuzzer has something to disagree
 * with. Nothing in crates/ links, calls, or depends on it.
 *
 * ---------------------------------------------------------------------------------
 * WHY BATCH MODE (this is a hard design requirement, not a preference)
 * ---------------------------------------------------------------------------------
 * A long-lived oracle process fed over a pipe can deadlock on Windows: both ends block
 * writing once the anonymous-pipe buffer fills and neither drains the other. CRC is a
 * pure function, so there is nothing to interleave and no reason to stream.
 *
 * This harness therefore does exactly one thing and exits:
 *
 *     oracle_harness <cases.bin> <results.bin>
 *
 * It reads the whole case file, computes every CRC, writes the whole result file, and
 * returns. The fuzzer waits for the exit code and only then opens the results. There is
 * no pipe, no interactive prompt, and no bidirectional traffic anywhere in the design.
 * (libcrc's own examples/tstcrc.c has interactive -a/-x modes that prompt on stdin —
 * those are never used here. tstcrc also prints only 9 of the 13 algorithms.)
 *
 * ---------------------------------------------------------------------------------
 * COVERAGE: all 13 exported symbols + all 8 incremental update_* functions
 * ---------------------------------------------------------------------------------
 * One-shot:    crc_8 crc_16 crc_32 crc_64_ecma crc_64_we crc_ccitt_1d0f crc_ccitt_ffff
 *              crc_dnp crc_kermit crc_modbus crc_sick crc_xmodem checksum_NMEA
 * Incremental: update_crc_8 update_crc_16 update_crc_32 update_crc_64 update_crc_ccitt
 *              update_crc_dnp update_crc_kermit update_crc_sick
 *
 * ---------------------------------------------------------------------------------
 * FILE FORMATS (little-endian, no padding, no structs on the wire)
 * ---------------------------------------------------------------------------------
 * cases.bin    "PMFZ" u32 version=1  u32 count
 *              count x { u8 flags ; u32 len ; u8 payload[len] }
 *              flags bit0 = pass a NULL pointer instead of the payload (libcrc's
 *              documented NULL guard returns the init value rather than faulting).
 *
 * results.bin  "PMFR" u32 version=1  u32 count
 *              count x { u8 oneshot[40] ; u8 incremental[40] }
 *
 *              block layout, offsets in bytes:
 *                 0    u8   crc_8
 *                 1    u8   nmea_flags   bit0 = checksum_NMEA returned non-NULL
 *                 2    u8   nmea_hex[0]
 *                 3    u8   nmea_hex[1]
 *                 4    u16  crc_16
 *                 6    u16  crc_ccitt_1d0f
 *                 8    u16  crc_ccitt_ffff
 *                10    u16  crc_dnp
 *                12    u16  crc_kermit
 *                14    u16  crc_modbus
 *                16    u16  crc_sick
 *                18    u16  crc_xmodem
 *                20    u32  crc_32
 *                24    u64  crc_64_ecma
 *                32    u64  crc_64_we
 *
 *              The incremental block reuses the same layout; bytes 1..3 (the NMEA
 *              fields) are zeroed because NMEA has no incremental form in libcrc.
 *
 * Build (the -funsigned-char is NOT optional — see below):
 *     gcc -O2 -funsigned-char -I<oracle>/include -o oracle_harness oracle_harness.c \
 *         <oracle>/lib/libcrc.a
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include "checksum.h"

/*
 * BUG D-01, reconfirmed here mechanically.
 *
 * include/checksum.h:99 declares
 *     uint64_t update_crc_64_ecma( uint64_t crc, unsigned char c );
 * but no such function is defined anywhere in src/, and `nm lib/libcrc.a` finds zero
 * definitions of it. The symbol that actually exists is update_crc_64(), which the
 * public header does NOT declare. So the documented incremental CRC-64 entry point
 * fails to link, and the one that works is undocumented.
 *
 * We therefore have to declare the real one ourselves to use it at all. That extern is
 * itself the evidence.
 */
extern uint64_t update_crc_64( uint64_t crc, unsigned char c );

#define BLOCK_BYTES  40u
#define RECORD_BYTES (2u * BLOCK_BYTES)

#ifdef PM_SABOTAGE
/*
 * ===========================================================================
 * NEGATIVE CONTROL — compiled in ONLY with -DPM_SABOTAGE, never for a real run.
 * ===========================================================================
 *
 * A fuzzer that reports "zero divergences" is worthless until you have watched it
 * report a non-zero one. This deliberately corrupts a single algorithm, for inputs
 * containing a single trigger byte, so a run against this build MUST fail and MUST
 * shrink the input down to that one byte. See fuzz/negative-control.sh.
 *
 * The build announces itself on stderr so this binary can never be mistaken for the
 * real oracle.
 */
static int sabotage_trigger( const unsigned char *data, size_t len ) {

	size_t i;

	if ( data == NULL ) return 0;
	for ( i = 0; i < len; i++ ) if ( data[i] == 0x7Au ) return 1;
	return 0;
}
#endif

/* ------------------------------------------------------------------ little-endian */

static void put_u8( unsigned char *p, uint8_t v ) {
	p[0] = v;
}

static void put_u16( unsigned char *p, uint16_t v ) {
	p[0] = (unsigned char) ( v        & 0xFFu );
	p[1] = (unsigned char) ( (v >> 8) & 0xFFu );
}

static void put_u32( unsigned char *p, uint32_t v ) {
	int i;
	for ( i = 0; i < 4; i++ ) p[i] = (unsigned char) ( ( v >> ( 8 * i ) ) & 0xFFu );
}

static void put_u64( unsigned char *p, uint64_t v ) {
	int i;
	for ( i = 0; i < 8; i++ ) p[i] = (unsigned char) ( ( v >> ( 8 * i ) ) & 0xFFu );
}

static uint32_t get_u32( const unsigned char *p ) {
	return   (uint32_t) p[0]
	     | ( (uint32_t) p[1] <<  8 )
	     | ( (uint32_t) p[2] << 16 )
	     | ( (uint32_t) p[3] << 24 );
}

/* ------------------------------------------------------------- the one-shot block */

static void compute_oneshot( unsigned char *out, const unsigned char *data, size_t len ) {

	unsigned char  nmea_buf[4];
	unsigned char *nmea_ret;

	memset( out, 0, BLOCK_BYTES );

	put_u8 ( out +  0, crc_8         ( data, len ) );
	put_u16( out +  4, crc_16        ( data, len ) );
	put_u16( out +  6, crc_ccitt_1d0f( data, len ) );
	put_u16( out +  8, crc_ccitt_ffff( data, len ) );
	put_u16( out + 10, crc_dnp       ( data, len ) );
	put_u16( out + 14, crc_modbus    ( data, len ) );
	put_u16( out + 16, crc_sick      ( data, len ) );
	put_u16( out + 18, crc_xmodem    ( data, len ) );
	put_u32( out + 20, crc_32        ( data, len ) );
	put_u64( out + 24, crc_64_ecma   ( data, len ) );
	put_u64( out + 32, crc_64_we     ( data, len ) );

	{
		uint16_t kermit = crc_kermit( data, len );
#ifdef PM_SABOTAGE
		if ( sabotage_trigger( data, len ) ) kermit = (uint16_t) ( kermit ^ 0x0001u );
#endif
		put_u16( out + 12, kermit );
	}

	/*
	 * checksum_NMEA is delimiter driven, not length driven: it walks a NUL-terminated
	 * string and stops at NUL, CR, LF or '*'. The caller has guaranteed data[len] is a
	 * writable NUL (see run_batch), so the payload is already a valid C string.
	 */
	nmea_buf[0] = 0;
	nmea_buf[1] = 0;
	nmea_buf[2] = 0;
	nmea_buf[3] = 0;
	nmea_ret    = checksum_NMEA( data, nmea_buf );

	if ( nmea_ret != NULL ) {
		put_u8( out + 1, 0x01 );
		put_u8( out + 2, nmea_buf[0] );
		put_u8( out + 3, nmea_buf[1] );
	}
}

/* ---------------------------------------------------------- the incremental block */

static void compute_incremental( unsigned char *out, const unsigned char *data, size_t len ) {

	uint8_t  c8    = CRC_START_8;
	uint16_t c16   = CRC_START_16;
	uint16_t cmb   = CRC_START_MODBUS;
	uint32_t c32   = CRC_START_32;
	uint16_t c1d0f = CRC_START_CCITT_1D0F;
	uint16_t cffff = CRC_START_CCITT_FFFF;
	uint16_t cxmod = CRC_START_XMODEM;
	uint16_t ckerm = CRC_START_KERMIT;
	uint16_t cdnp  = CRC_START_DNP;
	uint16_t csick = CRC_START_SICK;
	uint64_t cecma = CRC_START_64_ECMA;
	uint64_t cwe   = CRC_START_64_WE;

	unsigned char prev = 0;
	size_t a;

	memset( out, 0, BLOCK_BYTES );

	/* data == NULL reproduces libcrc's one-shot NULL guard: zero iterations. */
	if ( data != NULL ) for ( a = 0; a < len; a++ ) {

		unsigned char b = data[a];

		c8    = update_crc_8     ( c8,    b );
		c16   = update_crc_16    ( c16,   b );
		cmb   = update_crc_16    ( cmb,   b );
		c32   = update_crc_32    ( c32,   b );
		c1d0f = update_crc_ccitt ( c1d0f, b );
		cffff = update_crc_ccitt ( cffff, b );
		cxmod = update_crc_ccitt ( cxmod, b );
		ckerm = update_crc_kermit( ckerm, b );
		cdnp  = update_crc_dnp   ( cdnp,  b );
		csick = update_crc_sick  ( csick, b, prev );
		cecma = update_crc_64    ( cecma, b );
		cwe   = update_crc_64    ( cwe,   b );

		prev = b;
	}

	/*
	 * Finalisation, replicated from the one-shot functions. This is where libcrc's
	 * three catalogue divergences live: kermit, dnp and sick byte-swap the result.
	 * Reproduce, never "correct".
	 */
	ckerm = (uint16_t) ( ( ( ckerm & 0xFF00u ) >> 8 ) | ( ( ckerm & 0x00FFu ) << 8 ) );
	cdnp  = (uint16_t) ~cdnp;
	cdnp  = (uint16_t) ( ( ( cdnp  & 0xFF00u ) >> 8 ) | ( ( cdnp  & 0x00FFu ) << 8 ) );
	csick = (uint16_t) ( ( ( csick & 0xFF00u ) >> 8 ) | ( ( csick & 0x00FFu ) << 8 ) );
	c32  ^= 0xFFFFFFFFu;
	cwe  ^= 0xFFFFFFFFFFFFFFFFull;

	put_u8 ( out +  0, c8    );
	put_u16( out +  4, c16   );
	put_u16( out +  6, c1d0f );
	put_u16( out +  8, cffff );
	put_u16( out + 10, cdnp  );
	put_u16( out + 12, ckerm );
	put_u16( out + 14, cmb   );
	put_u16( out + 16, csick );
	put_u16( out + 18, cxmod );
	put_u32( out + 20, c32   );
	put_u64( out + 24, cecma );
	put_u64( out + 32, cwe   );
}

/* ------------------------------------------------------------------------- driver */

static unsigned char *read_whole_file( const char *path, size_t *out_len ) {

	FILE          *fp;
	long           size;
	unsigned char *buf;

	fp = fopen( path, "rb" );
	if ( fp == NULL ) return NULL;

	if ( fseek( fp, 0L, SEEK_END ) != 0 ) { fclose( fp ); return NULL; }
	size = ftell( fp );
	if ( size < 0 )                       { fclose( fp ); return NULL; }
	if ( fseek( fp, 0L, SEEK_SET ) != 0 ) { fclose( fp ); return NULL; }

	/* One spare byte so the last payload can be NUL-terminated in place for NMEA. */
	buf = (unsigned char *) malloc( (size_t) size + 1u );
	if ( buf == NULL ) { fclose( fp ); return NULL; }

	if ( fread( buf, 1u, (size_t) size, fp ) != (size_t) size ) {
		free( buf );
		fclose( fp );
		return NULL;
	}
	fclose( fp );

	buf[ size ] = 0;
	*out_len    = (size_t) size;
	return buf;
}

int main( int argc, char *argv[] ) {

	unsigned char *cases;
	unsigned char *results;
	size_t         cases_len;
	size_t         pos;
	uint32_t       count;
	uint32_t       i;
	FILE          *out;

	if ( argc != 3 ) {
		fprintf( stderr, "usage: %s <cases.bin> <results.bin>\n", argv[0] );
		return 2;
	}

#ifdef PM_SABOTAGE
	fprintf( stderr,
	         "*** SABOTAGED ORACLE — crc_kermit is deliberately corrupted for inputs\n"
	         "*** containing byte 0x7A. This build exists only to prove the fuzzer can\n"
	         "*** detect and minimise a divergence. Never use it for a real run.\n" );
#endif

	cases = read_whole_file( argv[1], &cases_len );
	if ( cases == NULL ) {
		fprintf( stderr, "oracle_harness: cannot read %s\n", argv[1] );
		return 3;
	}

	if ( cases_len < 12u || memcmp( cases, "PMFZ", 4u ) != 0 ) {
		fprintf( stderr, "oracle_harness: bad magic in %s\n", argv[1] );
		free( cases );
		return 4;
	}
	if ( get_u32( cases + 4 ) != 1u ) {
		fprintf( stderr, "oracle_harness: unsupported case-file version\n" );
		free( cases );
		return 4;
	}

	count   = get_u32( cases + 8 );
	results = (unsigned char *) malloc( (size_t) count * RECORD_BYTES + 12u );
	if ( results == NULL ) {
		fprintf( stderr, "oracle_harness: out of memory for %lu results\n",
		         (unsigned long) count );
		free( cases );
		return 5;
	}

	memcpy( results, "PMFR", 4u );
	put_u32( results + 4, 1u );
	put_u32( results + 8, count );

	pos = 12u;

	for ( i = 0; i < count; i++ ) {

		unsigned char  flags;
		uint32_t       len;
		unsigned char *payload;
		unsigned char *rec;
		unsigned char  saved;
		const unsigned char *arg;

		if ( pos + 5u > cases_len ) {
			fprintf( stderr, "oracle_harness: truncated case %lu\n", (unsigned long) i );
			free( results );
			free( cases );
			return 6;
		}

		flags = cases[ pos ];
		len   = get_u32( cases + pos + 1u );
		pos  += 5u;

		if ( pos + (size_t) len > cases_len ) {
			fprintf( stderr, "oracle_harness: truncated payload for case %lu\n",
			         (unsigned long) i );
			free( results );
			free( cases );
			return 6;
		}

		payload = cases + pos;
		pos    += (size_t) len;

		/*
		 * NUL-terminate the payload in place for checksum_NMEA, then restore the byte
		 * we clobbered — it belongs to the next record's header. The allocation carries
		 * one spare byte so this is still in bounds for the final record.
		 */
		saved         = cases[ pos ];
		cases[ pos ]  = 0;

		rec = results + 12u + (size_t) i * RECORD_BYTES;
		arg = ( flags & 0x01u ) ? NULL : payload;

		compute_oneshot    ( rec,                arg, (size_t) len );
		compute_incremental( rec + BLOCK_BYTES,  arg, (size_t) len );

		cases[ pos ] = saved;
	}

	free( cases );

	out = fopen( argv[2], "wb" );
	if ( out == NULL ) {
		fprintf( stderr, "oracle_harness: cannot write %s\n", argv[2] );
		free( results );
		return 7;
	}
	if ( fwrite( results, 1u, (size_t) count * RECORD_BYTES + 12u, out )
	     != (size_t) count * RECORD_BYTES + 12u ) {
		fprintf( stderr, "oracle_harness: short write to %s\n", argv[2] );
		fclose( out );
		free( results );
		return 7;
	}
	fclose( out );
	free( results );

	return 0;
}
