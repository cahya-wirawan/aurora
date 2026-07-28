//! The only crate in this spike that touches `rawler` (LGPL-2.1). Compiled
//! as a `cdylib` — a genuine OS shared library (`.so`/`.dylib`/`.dll`) with
//! a hand-written, `extern "C"` interface. Deliberately NOT a Rust `dylib`:
//! Rust's own dylib format embeds compiler-version-specific metadata and
//! has no stable cross-version ABI, which would fail LGPL-2.1 §6(b)'s
//! requirement that a user's *modified, interface-compatible* replacement
//! keep working — a C ABI, kept stable by hand rather than derived from
//! Rust types, is what actually satisfies that.
//!
//! Scope: one function, proving the mechanism, not a real API surface.
//! A real one would cover more of `rawler`'s surface and version the ABI
//! explicitly (see FINDINGS.md).

use std::ffi::{CStr, c_char};
use std::os::raw::c_int;

/// Plain-old-data, `#[repr(C)]`, no Rust-specific types crossing the
/// boundary — this is what makes the ABI stable across compiler versions,
/// not anything about `cdylib` alone.
#[repr(C)]
pub struct RawImageFfi {
    pub ok: c_int,
    pub width: u32,
    pub height: u32,
    pub data: *mut u16,
    pub data_len: usize,
    /// 4-char CFA pattern, e.g. b"RGGB", not NUL-terminated.
    pub cfa: [u8; 4],
}

/// Decodes a RAW file at `path`. Returns a `RawImageFfi` with `ok == 0` and
/// zeroed fields on any failure (bad path, non-UTF8, decode error) — never
/// panics across the FFI boundary, which would be undefined behavior.
///
/// # Safety
/// `path` must be a valid, NUL-terminated C string for the duration of the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aurora_raw_decode(path: *const c_char) -> RawImageFfi {
    let failure = RawImageFfi {
        ok: 0,
        width: 0,
        height: 0,
        data: std::ptr::null_mut(),
        data_len: 0,
        cfa: [0; 4],
    };
    if path.is_null() {
        return failure;
    }
    let Ok(path_str) = (unsafe { CStr::from_ptr(path) }).to_str() else {
        return failure;
    };
    let Ok(image) = rawler::decode_file(path_str) else {
        return failure;
    };
    let rawler::RawImageData::Integer(samples) = image.data else {
        return failure;
    };

    let cfa_bytes = image.camera.cfa.name.as_bytes();
    let mut cfa = [0u8; 4];
    for (dst, src) in cfa.iter_mut().zip(cfa_bytes.iter()) {
        *dst = *src;
    }

    let mut boxed = samples.into_boxed_slice();
    let data = boxed.as_mut_ptr();
    let data_len = boxed.len();
    std::mem::forget(boxed); // ownership crosses to the caller; freed via aurora_raw_free

    RawImageFfi {
        ok: 1,
        width: image.width as u32,
        height: image.height as u32,
        data,
        data_len,
        cfa,
    }
}

/// Frees a `RawImageFfi` returned by `aurora_raw_decode`. Must be called
/// exactly once per successful decode, from the same allocator (i.e., only
/// ever call this from code linked against this exact `raw-shim` build).
///
/// # Safety
/// `img.data`/`img.data_len` must be exactly what `aurora_raw_decode`
/// returned, not modified in between.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aurora_raw_free(img: RawImageFfi) {
    if !img.data.is_null() {
        let slice_ptr = std::ptr::slice_from_raw_parts_mut(img.data, img.data_len);
        drop(unsafe { Box::from_raw(slice_ptr) });
    }
}
