//! CMYK and 32-bit floating point PSD files (task 146): synthetic files
//! built in-test, the tracked 32-bit psd-tools fixture, hostile headers,
//! and a sweep over the refused-until-now corpus files.

use half::f16;

use super::test_writer::{TestLayer, TestPsd};
use super::{PsdDocument, decode, merged_image, merged_image_in_working_space, read};
use crate::error::IoError;

fn open(bytes: &[u8]) -> PsdDocument {
    match read(bytes) {
        Ok(d) => d,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn first_pixels(document: &PsdDocument, n: usize) -> Vec<[f32; 4]> {
    let Some(p) = document.pixels.first() else {
        unreachable!("a layer")
    };
    p.image
        .samples()
        .chunks_exact(4)
        .take(n)
        .map(|c| {
            let mut o = [f32::NAN; 4];
            for (slot, v) in o.iter_mut().zip(c) {
                *slot = f16::to_f32(*v);
            }
            o
        })
        .collect()
}

// ---------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------

fn corpus_roots() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![root.join("corpora/psd"), root.join("images")]
}

/// Every PSD/PSB under the corpus roots whose header says CMYK or 32-bit.
fn mode_files() -> Vec<(std::path::PathBuf, u16, u16)> {
    let mut pending = corpus_roots();
    let mut out = Vec::new();
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
                && let Ok(head) = read_head(&path)
                && let [b'8', b'B', b'P', b'S', ..] = head.as_slice()
            {
                let depth = u16::from_be_bytes([head[22], head[23]]);
                let mode = u16::from_be_bytes([head[24], head[25]]);
                if mode == 4 || depth == 32 {
                    out.push((path, mode, depth));
                }
            }
        }
    }
    out.sort();
    out
}

fn read_head(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut head = vec![0_u8; 26];
    std::fs::File::open(path)?.read_exact(&mut head)?;
    Ok(head)
}

#[test]
fn every_cmyk_and_32_bit_corpus_file_opens() {
    let files = mode_files();
    if files.is_empty() {
        println!("SKIPPED: no CMYK / 32-bit PSD corpus files present");
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        return;
    }
    let (mut cmyk, mut float) = (0, 0);
    for (path, mode, depth) in &files {
        let Ok(bytes) = std::fs::read(path) else {
            unreachable!("{} unreadable", path.display())
        };
        let started = std::time::Instant::now();
        match read(&bytes) {
            Ok(document) => {
                println!(
                    "OPENED {} mode {mode} depth {depth}: {} layers, {} report lines, {:?}",
                    path.display(),
                    document.pixels.len(),
                    document.report.items.len(),
                    started.elapsed()
                );
                for line in &document.report.items {
                    println!("    {line}");
                }
            }
            Err(err) => unreachable!("{} did not open: {err:?}", path.display()),
        }
        if *mode == 4 {
            cmyk += 1;
        } else {
            float += 1;
        }
    }
    println!("CMYK {cmyk}, 32-bit {float}");
    // The task's corpus: 13 CMYK and 17 32-bit files.
    assert!(cmyk >= 13 && float >= 17, "CMYK {cmyk}, 32-bit {float}");
    // Silence unused warnings for the helpers used by later tests.
    let _ = (decode, merged_image, merged_image_in_working_space);
    let _: Option<IoError> = None;
    let _ = TestPsd::new(1, 1, 1, 8);
    let _ = TestLayer::empty("x");
}
