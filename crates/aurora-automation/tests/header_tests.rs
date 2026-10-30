//! PSD / PSB layer rectangles read from the header structure alone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::io::Cursor;

use aurora_automation::header::{self, LAYER_TOTAL_FACTOR};

/// One layer record: a rectangle (top, left, bottom, right) and an optional
/// layer-mask rectangle.
struct Layer {
    rect: [i32; 4],
    mask: Option<[i32; 4]>,
}

fn layer(rect: [i32; 4]) -> Layer {
    Layer { rect, mask: None }
}

fn big(width: i32, height: i32) -> Layer {
    layer([0, 0, height, width])
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn len(out: &mut Vec<u8>, psb: bool, n: usize) {
    if psb {
        out.extend_from_slice(&(n as u64).to_be_bytes());
    } else {
        be32(out, n as u32);
    }
}

/// The layer-info body: count, records (no channel image data: the scan
/// never reads it).
fn layer_records(layers: &[Layer], psb: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(layers.len() as i16).to_be_bytes());
    for l in layers {
        for v in l.rect {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&2_u16.to_be_bytes());
        for id in [0_i16, -1] {
            out.extend_from_slice(&id.to_be_bytes());
            len(&mut out, psb, 4);
        }
        out.extend_from_slice(b"8BIMnorm");
        out.extend_from_slice(&[255, 0, 0, 0]);
        let mut extra = Vec::new();
        match l.mask {
            Some(m) => {
                be32(&mut extra, 20);
                for v in m {
                    extra.extend_from_slice(&v.to_be_bytes());
                }
                extra.extend_from_slice(&[0, 0, 0, 0]);
            }
            None => be32(&mut extra, 0),
        }
        be32(&mut extra, 0); // blending ranges
        extra.extend_from_slice(&[1, b'a', 0, 0]); // Pascal name, padded
        be32(&mut out, extra.len() as u32);
        out.extend_from_slice(&extra);
    }
    out
}

/// A PSD/PSB of a `w` x `h` canvas holding `layers`; `lr16` puts the layer
/// info in an `Lr16` global block (what a 16-bit file does).
fn file(psb: bool, w: u32, h: u32, layers: &[Layer], lr16: bool) -> Vec<u8> {
    let mut out = b"8BPS".to_vec();
    out.extend_from_slice(&(if psb { 2_u16 } else { 1 }).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&3_u16.to_be_bytes());
    be32(&mut out, h);
    be32(&mut out, w);
    out.extend_from_slice(&16_u16.to_be_bytes());
    out.extend_from_slice(&3_u16.to_be_bytes());
    be32(&mut out, 0); // colour mode data
    be32(&mut out, 0); // image resources
    let records = layer_records(layers, psb);
    let mut section = Vec::new();
    if lr16 {
        len(&mut section, psb, 0); // empty layer info
        be32(&mut section, 0); // global layer mask info
        // A block ahead of Lr16, with a length that needs padding.
        section.extend_from_slice(b"8BIMlnkE");
        len(&mut section, psb, 3);
        section.extend_from_slice(&[1, 2, 3, 0]);
        section.extend_from_slice(b"8BIMLr16");
        len(&mut section, psb, records.len());
        section.extend_from_slice(&records);
    } else {
        len(&mut section, psb, records.len());
        section.extend_from_slice(&records);
        be32(&mut section, 0);
    }
    len(&mut out, psb, section.len());
    out.extend_from_slice(&section);
    out
}

fn scan(bytes: &[u8], psb: bool) -> Result<header::LayerExtent, String> {
    header::psd_layers(&mut Cursor::new(bytes), psb)
}

const CAP: u64 = 50_000_000;

fn check(bytes: &[u8], format: &str) -> Result<(), String> {
    header::check_psd_layers(&mut Cursor::new(bytes), format, CAP)
}

#[test]
fn layer_rectangles_are_read_from_a_psd_and_a_psb_in_both_layouts() {
    for psb in [false, true] {
        for lr16 in [false, true] {
            let layers = [big(100, 50), big(10, 10), layer([5, 5, 5, 99])];
            let bytes = file(psb, 64, 64, &layers, lr16);
            let extent = scan(&bytes, psb).unwrap();
            assert_eq!(extent.layers, 3, "psb {psb} lr16 {lr16}");
            assert_eq!(extent.largest, 5000);
            assert_eq!(extent.total, 5100, "an empty rectangle counts for nothing");
        }
    }
}

#[test]
fn a_small_canvas_with_a_huge_layer_is_refused_before_decoding() {
    for psb in [false, true] {
        for lr16 in [false, true] {
            // 100 x 100 canvas; the layer says 60,000 x 60,000 (3.6 Gpx).
            let bytes = file(psb, 100, 100, &[big(60_000, 60_000)], lr16);
            let message = check(&bytes, if psb { "PSB" } else { "PSD" }).unwrap_err();
            assert!(
                message.contains("3600 Mpx") && message.contains("at most 50 Mpx"),
                "{message}"
            );
        }
    }
}

#[test]
fn a_layer_mask_rectangle_counts_too() {
    let mut huge_mask = layer([0, 0, 10, 10]);
    huge_mask.mask = Some([0, 0, 40_000, 40_000]);
    let bytes = file(false, 10, 10, &[huge_mask], false);
    assert!(check(&bytes, "PSD").is_err());
    let mut fine = layer([0, 0, 10, 10]);
    fine.mask = Some([0, 0, 10, 10]);
    check(&file(false, 10, 10, &[fine], false), "PSD").unwrap();
}

#[test]
fn rectangles_at_the_extremes_do_not_overflow() {
    let wild = layer([i32::MIN, i32::MIN, i32::MAX, i32::MAX]);
    let bytes = file(false, 1, 1, &[wild], false);
    let extent = scan(&bytes, false).unwrap();
    assert!(extent.largest > CAP);
    assert!(check(&bytes, "PSD").is_err());
    // Upside-down rectangles are empty, not huge.
    let flipped = layer([i32::MAX, i32::MAX, i32::MIN, i32::MIN]);
    let bytes = file(false, 1, 1, &[flipped], false);
    assert_eq!(scan(&bytes, false).unwrap().largest, 0);
}

#[test]
fn many_legitimate_layers_pass_and_a_pile_past_the_total_is_refused() {
    // 10 full-canvas 20 Mpx layers: each under the cap, 200 Mpx in all.
    let layers: Vec<Layer> = (0..10).map(|_| big(5000, 4000)).collect();
    check(&file(false, 5000, 4000, &layers, true), "PSD").unwrap();
    // 50 of them: 1000 Mpx, past 16 x 50 = 800 Mpx.
    let layers: Vec<Layer> = (0..50).map(|_| big(5000, 4000)).collect();
    let message = check(&file(false, 5000, 4000, &layers, true), "PSD").unwrap_err();
    assert!(
        message.contains("1000 Mpx") && message.contains(&format!("{LAYER_TOTAL_FACTOR} times")),
        "{message}"
    );
}

#[test]
fn files_without_layers_and_other_formats_pass() {
    check(&file(false, 10, 10, &[], false), "PSD").unwrap();
    check(&file(true, 10, 10, &[], true), "PSB").unwrap();
    // No layer section at all.
    let mut bytes = file(false, 10, 10, &[], false);
    bytes.truncate(26 + 4 + 4);
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(scan(&bytes, false).unwrap(), header::LayerExtent::default());
    // Other formats are not looked at.
    check(&[0_u8; 8], "PNG").unwrap();
}

#[test]
fn a_cut_or_lying_layer_section_is_refused_not_guessed_at() {
    let bytes = file(false, 10, 10, &[big(5, 5), big(6, 6)], false);
    for cut_at in [27, 34, 40, bytes.len() - 20, bytes.len() - 1] {
        let short = &bytes[..cut_at];
        assert!(scan(short, false).is_err(), "cut at {cut_at}");
    }
    // A record count far beyond the data: refused, quickly, no allocation.
    let mut lying = file(false, 10, 10, &[big(5, 5)], false);
    let count_at = 26 + 4 + 4 + 4 + 4;
    lying[count_at..count_at + 2].copy_from_slice(&32_000_i16.to_be_bytes());
    assert!(scan(&lying, false).is_err());
    // A section length that runs past the file.
    let mut long = file(false, 10, 10, &[big(5, 5)], false);
    long[34..38].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(scan(&long, false).is_err());
}

/// Every PSD/PSB in the local corpora scans cleanly (a file the scan
/// cannot read would be refused by the bridge). Skips when there are none.
#[test]
fn the_local_corpus_scans_cleanly() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["images", "corpora"] {
        collect(&root.join(dir), &mut files);
    }
    if files.is_empty() {
        println!("SKIPPED: no local PSD corpus");
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none_or(|v| v.is_empty() || v == "0"),
            "AURORA_REQUIRE_CORPUS is set but there are no PSD files"
        );
        return;
    }
    let mut failures = Vec::new();
    let mut over = Vec::new();
    for path in &files {
        let mut f = std::fs::File::open(path).unwrap();
        match header::dimensions(&mut f) {
            Ok(Some(d)) if d.format == "PSD" || d.format == "PSB" => {
                match header::psd_layers(&mut f, d.format == "PSB") {
                    Ok(extent) if extent.largest > CAP => {
                        over.push(format!(
                            "{} ({} Mpx)",
                            path.display(),
                            extent.largest / 1_000_000
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => failures.push(format!("{}: {e}", path.display())),
                }
            }
            Ok(_) => {}
            Err(e) => println!("{}: header: {e} (not a layer scan problem)", path.display()),
        }
    }
    println!(
        "scanned {} PSD/PSB files; {} have a layer over 50 Mpx: {over:?}",
        files.len(),
        over.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
        {
            out.push(path);
        }
    }
}
