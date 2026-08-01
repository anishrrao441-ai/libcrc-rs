//! C ABI shim: lets the **unmodified** original libcrc test suite link against the Rust port.
//!
//! # This crate is not part of the port
//!
//! `libcrc-rs` is the port. It is `#![forbid(unsafe_code)]` and contains zero `unsafe`
//! blocks. This crate exists solely so that `tests/original/*.c` — hashed at kickoff and
//! never edited — can be compiled and linked against the Rust implementation to prove
//! behavioural equivalence.
//!
//! The C API is `(const unsigned char *, size_t)`. Reconstructing a slice from a caller-
//! supplied pointer and length is inherently unsafe, so every `unsafe` block in this
//! project lives here, in a test harness, and each is justified in `UNSAFE.md`.
//!
//! Nothing here links, calls, or depends on the original C library.
//!
//! # NULL handling
//!
//! libcrc guards its loops with `if ( ptr != NULL )` and returns the initial value for a
//! NULL pointer rather than faulting (`src/crc16.c:63`). `checksum_NMEA` instead returns
//! NULL for either argument being NULL (`src/nmea-chk.c`). Both behaviours are observable
//! through the public API, so both are reproduced exactly.
#![allow(non_snake_case)] // `checksum_NMEA` must match the C symbol exactly.

use core::slice;

/// Rebuild a `&[u8]` from the C `(pointer, length)` pair, reproducing libcrc's NULL guard.
fn as_slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() {
        return &[];
    }
    // SAFETY: justified in UNSAFE.md (U-1). The C caller guarantees `ptr` is valid for
    // `len` bytes — the same precondition the original C loop relies on. NULL is handled
    // above, matching libcrc's own `if ( ptr != NULL )` guard.
    unsafe { slice::from_raw_parts(ptr, len) }
}

macro_rules! c_export {
    ($(#[$m:meta])* $name:ident -> $ret:ty) => {
        $(#[$m])*
        #[no_mangle]
        pub extern "C" fn $name(input_str: *const u8, num_bytes: usize) -> $ret {
            libcrc_rs::$name(as_slice(input_str, num_bytes))
        }
    };
}

c_export!(/// libcrc `crc_8()`.
    crc_8 -> u8);
c_export!(/// libcrc `crc_16()`.
    crc_16 -> u16);
c_export!(/// libcrc `crc_modbus()`.
    crc_modbus -> u16);
c_export!(/// libcrc `crc_32()`.
    crc_32 -> u32);
c_export!(/// libcrc `crc_ccitt_1d0f()`.
    crc_ccitt_1d0f -> u16);
c_export!(/// libcrc `crc_ccitt_ffff()`.
    crc_ccitt_ffff -> u16);
c_export!(/// libcrc `crc_xmodem()`.
    crc_xmodem -> u16);
c_export!(/// libcrc `crc_kermit()`. Byte-swapped relative to the RevEng catalogue.
    crc_kermit -> u16);
c_export!(/// libcrc `crc_dnp()`. Complemented and byte-swapped.
    crc_dnp -> u16);
c_export!(/// libcrc `crc_sick()`.
    crc_sick -> u16);
c_export!(/// libcrc `crc_64_ecma()`.
    crc_64_ecma -> u64);
c_export!(/// libcrc `crc_64_we()`.
    crc_64_we -> u64);

// ---------------------------------------------------------------------------
// The incremental (`update_crc_*`) family.
//
// The original test suite never calls these, so passing the suite does NOT prove
// they exist. They are nonetheless part of libcrc's public header, and a program
// that calls one and links against a staticlib lacking it fails at link time.
// A port that omitted them would not be a drop-in replacement.
// ---------------------------------------------------------------------------

/// libcrc `update_crc_8()`.
#[no_mangle]
pub extern "C" fn update_crc_8(crc: u8, c: u8) -> u8 {
    libcrc_rs::update_crc_8(crc, c)
}

/// libcrc `update_crc_16()`. Also serves MODBUS, which shares the table.
#[no_mangle]
pub extern "C" fn update_crc_16(crc: u16, c: u8) -> u16 {
    libcrc_rs::update_crc_16(crc, c)
}

/// libcrc `update_crc_32()`. Operates on the internal, non-finalised value: the
/// caller applies the final XOR, exactly as in the original.
#[no_mangle]
pub extern "C" fn update_crc_32(crc: u32, c: u8) -> u32 {
    libcrc_rs::update_crc_32(crc, c)
}

/// libcrc `update_crc_ccitt()`.
#[no_mangle]
pub extern "C" fn update_crc_ccitt(crc: u16, c: u8) -> u16 {
    libcrc_rs::update_crc_ccitt(crc, c)
}

/// libcrc `update_crc_kermit()`. Returns the un-swapped running value; the caller
/// byte-swaps at the end, as `crc_kermit()` does.
#[no_mangle]
pub extern "C" fn update_crc_kermit(crc: u16, c: u8) -> u16 {
    libcrc_rs::update_crc_kermit(crc, c)
}

/// libcrc `update_crc_dnp()`. Returns the running value before complement and swap.
#[no_mangle]
pub extern "C" fn update_crc_dnp(crc: u16, c: u8) -> u16 {
    libcrc_rs::update_crc_dnp(crc, c)
}

/// libcrc `update_crc_sick()`. Bitwise, and needs the previous byte (`0` for the
/// first byte of a message).
#[no_mangle]
pub extern "C" fn update_crc_sick(crc: u16, c: u8, prev_byte: u8) -> u16 {
    libcrc_rs::update_crc_sick(crc, c, prev_byte)
}

/// libcrc `update_crc_64_ecma()` — **deliberately fixes an upstream defect.**
///
/// This symbol is declared in the original's public header (`include/checksum.h:99`)
/// but is **defined nowhere**: `nm` on a freshly built `libcrc.a` reports zero
/// definitions, so any program that calls the documented API fails to link. Only the
/// unprefixed `update_crc_64` exists, and it is not declared in the header at all.
///
/// Implementing it cannot break behavioural equivalence — you cannot diverge from a
/// function that does not exist — and it makes the shipped header honest. Reported
/// upstream; see `DECISIONS.md`.
#[no_mangle]
pub extern "C" fn update_crc_64_ecma(crc: u64, c: u8) -> u64 {
    libcrc_rs::update_crc_64(crc, c)
}

/// libcrc `checksum_NMEA()`.
///
/// Takes a NUL-terminated sentence and writes two uppercase hex digits plus a NUL
/// terminator into `result` (a 3-byte buffer), returning `result`. Returns NULL if
/// either argument is NULL, matching the original.
#[no_mangle]
pub extern "C" fn checksum_NMEA(input_str: *const u8, result: *mut u8) -> *mut u8 {
    if input_str.is_null() || result.is_null() {
        return core::ptr::null_mut();
    }

    // SAFETY: justified in UNSAFE.md (U-2). The C contract is a NUL-terminated string;
    // the original walks the same bytes with the same assumption. We stop at the NUL, so
    // we never read past the terminator the caller promised.
    let mut len = 0usize;
    while unsafe { *input_str.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: justified in UNSAFE.md (U-1). `len` was just measured up to the NUL.
    let sentence = unsafe { slice::from_raw_parts(input_str, len) };

    let checksum = libcrc_rs::checksum_nmea(sentence);

    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let digits = [HEX[(checksum >> 4) as usize], HEX[(checksum & 0x0F) as usize], 0u8];
    // SAFETY: justified in UNSAFE.md (U-3). The C contract requires `result` to point to
    // at least 3 writable bytes; the original writes the same 3 via snprintf(result, 3, ..).
    unsafe {
        core::ptr::copy_nonoverlapping(digits.as_ptr(), result, 3);
    }
    result
}
