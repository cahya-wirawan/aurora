//! Deliberately has zero dependency on `rawler` or any other LGPL crate —
//! confirmed, not just claimed, in FINDINGS.md via `ldd`/`nm` on the built
//! binary. RAW-decode functionality is reached only by `dlopen`-ing
//! `raw-shim`'s `.so` at run time, the "suitable shared library mechanism"
//! LGPL-2.1 §6(b) describes: loaded at run time rather than compiled in,
//! and swappable for a user's own interface-compatible build of the same
//! `.so` without recompiling this binary at all.

use libloading::{Library, Symbol};
use std::ffi::CString;
use std::os::raw::{c_char, c_int};

#[repr(C)]
struct RawImageFfi {
    ok: c_int,
    width: u32,
    height: u32,
    data: *mut u16,
    data_len: usize,
    cfa: [u8; 4],
}

fn main() {
    let shim_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: host <path-to-raw-shim-cdylib> [path-to-raw-file]");
        std::process::exit(2);
    });
    let raw_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "../raw-icc/reference/raw-samples/canon-eos-m200.cr3".to_string());

    // SAFETY: this is exactly the operation being evaluated -- loading a
    // shared library at run time, per LGPL-2.1 §6(b). The library's ABI is
    // the hand-written contract in raw-shim/src/lib.rs, checked by hand
    // against what's declared here since this deliberately does not link
    // against raw-shim's Rust types at compile time (that would defeat the
    // point: this binary must not depend on raw-shim, only load it).
    let lib = unsafe { Library::new(&shim_path) }
        .unwrap_or_else(|e| panic!("failed to dlopen {shim_path}: {e}"));

    let decode: Symbol<unsafe extern "C" fn(*const c_char) -> RawImageFfi> =
        unsafe { lib.get(b"aurora_raw_decode") }.expect("symbol aurora_raw_decode not found");
    let free: Symbol<unsafe extern "C" fn(RawImageFfi)> =
        unsafe { lib.get(b"aurora_raw_free") }.expect("symbol aurora_raw_free not found");

    let c_path = CString::new(raw_path.as_str()).expect("path has no interior NUL");
    let image = unsafe { decode(c_path.as_ptr()) };

    if image.ok == 0 {
        eprintln!("decode FAILED for {raw_path}");
        std::process::exit(1);
    }

    let samples = unsafe { std::slice::from_raw_parts(image.data, image.data_len) };
    let (min, max, sum) = samples
        .iter()
        .fold((u16::MAX, 0u16, 0u64), |(mn, mx, s), &v| {
            (mn.min(v), mx.max(v), s + u64::from(v))
        });
    let mean = sum as f64 / samples.len() as f64;
    let cfa = String::from_utf8_lossy(&image.cfa);

    println!(
        "Decoded via dlopen({shim_path}), zero LGPL code in this binary:\n\
         {}×{} px, cfa={cfa}, {} samples, range [{min}, {max}], mean {mean:.0}",
        image.width,
        image.height,
        samples.len()
    );

    unsafe { free(image) };
}
