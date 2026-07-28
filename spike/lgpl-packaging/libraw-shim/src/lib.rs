//! The LibRaw-backed counterpart to `raw-shim` — same `RawImageFfi` layout,
//! same exported symbol names, so `host` (unmodified) can `dlopen` either
//! one. This is what actually closes the gap ADR 0007 named: the packaging
//! mechanism was first proven with `rawler`; this proves it again with the
//! library ADR 0007 actually chose, not just by analogy.
//!
//! Uses `libraw_rs_vendor`'s raw bindgen output directly (no friendly Rust
//! wrapper exists for it) — LibRaw's own C API, called from Rust the same
//! way any C program would. Scope: one file, one function, proving the
//! mechanism — not a real decoder integration. See FINDINGS.md.

use libraw_rs_vendor as ffi;
use std::ffi::{CStr, CString, c_char};
use std::os::raw::c_int;

/// Identical layout to `raw-shim`'s `RawImageFfi` — this is the actual ABI
/// contract; keeping both definitions manually in sync (rather than
/// sharing a common crate) is deliberate, matching how two independently
/// built implementations of the same interface would work in practice.
#[repr(C)]
pub struct RawImageFfi {
    pub ok: c_int,
    pub width: u32,
    pub height: u32,
    pub data: *mut u16,
    pub data_len: usize,
    pub cfa: [u8; 4],
}

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
    let Ok(c_path) = CString::new(path_str) else {
        return failure;
    };

    // SAFETY: standard LibRaw C API sequence (init -> open -> unpack),
    // exactly as any C caller would use it. `lr` is checked non-null and
    // always passed to `libraw_close` before returning, on every path.
    unsafe {
        let lr = ffi::libraw_init(0);
        if lr.is_null() {
            return failure;
        }

        if ffi::libraw_open_file(lr, c_path.as_ptr()) != 0 {
            ffi::libraw_close(lr);
            return failure;
        }
        if ffi::libraw_unpack(lr) != 0 {
            ffi::libraw_close(lr);
            return failure;
        }

        let sizes = (*lr).rawdata.sizes;
        let width = u32::from(sizes.raw_width);
        let height = u32::from(sizes.raw_height);
        let data_len = (width as usize) * (height as usize);
        let raw_image = (*lr).rawdata.raw_image;
        if raw_image.is_null() || data_len == 0 {
            ffi::libraw_close(lr);
            return failure;
        }

        // Copy out of LibRaw's own allocation -- freeing at the C ABI
        // boundary must use the same allocator that made the allocation
        // (Rust's `Box`, matching raw-shim's `aurora_raw_free`), not
        // LibRaw's internal allocator.
        let samples: Vec<u16> = std::slice::from_raw_parts(raw_image, data_len).to_vec();
        let mut boxed = samples.into_boxed_slice();
        let data = boxed.as_mut_ptr();
        std::mem::forget(boxed);

        let cdesc = (*lr).rawdata.iparams.cdesc;
        let mut cfa = [0u8; 4];
        for (dst, src) in cfa.iter_mut().zip(cdesc.iter()) {
            *dst = *src as u8;
        }

        ffi::libraw_close(lr);

        RawImageFfi {
            ok: 1,
            width,
            height,
            data,
            data_len,
            cfa,
        }
    }
}

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
