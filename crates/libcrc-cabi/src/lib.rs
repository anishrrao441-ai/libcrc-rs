//! C ABI shim: lets the **unmodified** original libcrc test suite link against the Rust port.
//!
//! # This crate is not part of the port
//!
//! `libcrc-rs` is the port. It is `#![forbid(unsafe_code)]` and contains zero `unsafe`
//! blocks. This crate exists solely so that `tests/original/*.c` — which are hashed at
//! kickoff and never edited — can be compiled and linked against the Rust implementation
//! to prove behavioural equivalence.
//!
//! The C API is `(const unsigned char *, size_t)`. Reconstructing a slice from a caller-
//! supplied pointer and length is inherently unsafe, so every `unsafe` block in this
//! project lives here, in a test harness, and each one is justified in `UNSAFE.md`.
//!
//! Nothing here links, calls, or depends on the original C library.
//!
//! # NULL handling
//!
//! libcrc guards its loops with `if ( ptr != NULL )` and returns the initial value for a
//! NULL pointer rather than faulting (see `src/crc16.c:63`). That behaviour is reproduced
//! exactly — it is observable through the public API, so it is part of the contract.

use core::slice;

/// Rebuild a `&[u8]` from the C `(pointer, length)` pair, reproducing libcrc's NULL guard.
///
/// # Safety
///
/// The caller must uphold the C contract: `ptr` is either NULL, or is valid for reads of
/// `len` bytes. The original C code dereferences it under exactly the same assumption.
fn as_slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() {
        return &[];
    }
    // SAFETY: justified in UNSAFE.md. The C caller guarantees `ptr` is valid for `len`
    // bytes; this is the same precondition the original C loop relies on. NULL is handled
    // above, matching libcrc's own `if ( ptr != NULL )` guard.
    #[allow(unsafe_code)]
    unsafe {
        slice::from_raw_parts(ptr, len)
    }
}

/// CRC-16/ARC — libcrc `crc_16()`.
#[no_mangle]
pub extern "C" fn crc_16(input_str: *const u8, num_bytes: usize) -> u16 {
    libcrc_rs::crc_16(as_slice(input_str, num_bytes))
}

/// CRC-16/MODBUS — libcrc `crc_modbus()`.
#[no_mangle]
pub extern "C" fn crc_modbus(input_str: *const u8, num_bytes: usize) -> u16 {
    libcrc_rs::crc_modbus(as_slice(input_str, num_bytes))
}

/// CRC-32 — libcrc `crc_32()`.
#[no_mangle]
pub extern "C" fn crc_32(input_str: *const u8, num_bytes: usize) -> u32 {
    libcrc_rs::crc_32(as_slice(input_str, num_bytes))
}
