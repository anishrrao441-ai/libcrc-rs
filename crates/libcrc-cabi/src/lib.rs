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
