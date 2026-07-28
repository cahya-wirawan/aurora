//! RAW decode + ICC transform feasibility spike — PLAN 0.6.
//!
//! Aurora needs to decode Camera RAW from major vendors (FR-015) and apply
//! ICC transforms (FR-016). PRD §8.2 named the choice as pure-Rust vs. FFI
//! for both, with a licensing risk (§14) assumed to apply mainly to the FFI
//! side (LibRaw is LGPL-2.1/CDDL). This spike checked both the licensing
//! assumption and actual capability against real camera files and real ICC
//! profiles — see FINDINGS.md for what turned out to still be true and what
//! didn't.
//!
//!   cargo run -- raw   decodes reference/raw-samples/*.{cr3,nef,arw}, writes preview PPMs to out/
//!   cargo run -- icc   cross-validates lcms2 (FFI) against moxcms (pure Rust) on real ICC profiles

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "raw" => raw_demo(),
        "icc" => icc_demo(),
        other => {
            eprintln!("usage: cargo run -- raw|icc   (got {other:?})");
            std::process::exit(2);
        }
    }
}

/// One real file per major vendor (raw.pixls.us, a public RAW sample
/// archive used by rawspeed/darktable/RawTherapee for exactly this kind of
/// testing) — not synthesized. Files are gitignored (multi-MB each, PLAN
/// 0.7's "corpora are gitignored" rule); see reference/README.md for exact
/// provenance and `reference/fetch-samples.sh` to re-fetch them.
const SAMPLES: &[(&str, &str)] = &[
    (
        "Canon CR3 (EOS M200)",
        "reference/raw-samples/canon-eos-m200.cr3",
    ),
    ("Nikon NEF (1 J1)", "reference/raw-samples/nikon-1-j1.nef"),
    (
        "Sony ARW (DSC-RX1)",
        "reference/raw-samples/sony-dsc-rx1.arw",
    ),
];

fn raw_demo() {
    let mut any_missing = false;
    std::fs::create_dir_all("out").expect("create out/");
    for (label, path) in SAMPLES {
        if !std::path::Path::new(path).exists() {
            println!("{label}: MISSING — run reference/fetch-samples.sh first ({path})");
            any_missing = true;
            continue;
        }
        print!("{label}: ");
        match rawler::decode_file(path) {
            Ok(image) => {
                let rawler::RawImageData::Integer(samples) = &image.data else {
                    println!("FAILED — unexpectedly Float data, not Integer");
                    continue;
                };
                // Real image data, not a zeroed or garbage buffer -- min/max/mean
                // over actual sensor values, not just "decode_file returned Ok".
                let (min, max, sum) = samples
                    .iter()
                    .fold((u16::MAX, 0u16, 0u64), |(mn, mx, s), &v| {
                        (mn.min(v), mx.max(v), s + u64::from(v))
                    });
                let mean = sum as f64 / samples.len() as f64;
                println!(
                    "OK — {}×{} px, camera={:?}, cfa={:?}, {} samples, range [{min}, {max}], mean {mean:.0}",
                    image.width,
                    image.height,
                    image.camera.model,
                    image.camera.cfa,
                    samples.len(),
                );

                // Crude "set R=G=B from the raw mosaic" preview, same as
                // rawler's own doc example -- not a real demosaic, just
                // enough to visually confirm this is a real photograph and
                // not noise. Downscaled 4x so the PPM stays small.
                let ppm_path = format!(
                    "out/{}.ppm",
                    path.rsplit('/').next().unwrap_or(path).replace('.', "_")
                );
                write_preview_ppm(&ppm_path, image.width, image.height, samples, 4, min, max);
                println!("  preview written to {ppm_path}");
            }
            Err(e) => println!("FAILED — {e}"),
        }
    }
    if any_missing {
        std::process::exit(1);
    }
}

/// Downscaled grayscale PPM from raw mosaic samples (nearest-neighbor pick,
/// no actual demosaic or white balance) — visual sanity check only.
/// Stretched by this file's own [min, max] rather than a fixed bit shift,
/// since sensor bit depth varies by camera (e.g. 4037 max on the Nikon 1 J1
/// vs. 16383 on the other two) — a fixed >>8 shift left the low-range one
/// looking black even though the data was fine.
fn write_preview_ppm(
    path: &str,
    width: usize,
    height: usize,
    samples: &[u16],
    downscale: usize,
    min: u16,
    max: u16,
) {
    use std::io::Write;
    let out_w = width / downscale;
    let out_h = height / downscale;
    let range = f32::from(max - min).max(1.0);
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).expect("create ppm"));
    write!(f, "P6\n{out_w} {out_h}\n255\n").expect("write header");
    for y in 0..out_h {
        for x in 0..out_w {
            let sy = y * downscale;
            let sx = x * downscale;
            let v = samples[sy * width + sx];
            let stretched = f32::from(v.saturating_sub(min)) / range;
            let byte = (stretched.clamp(0.0, 1.0) * 255.0) as u8;
            f.write_all(&[byte, byte, byte]).expect("write pixel");
        }
    }
}

/// ICC transform feasibility: FFI (`lcms2`, vendors and statically compiles
/// real Little CMS C source — see FINDINGS.md) vs. pure Rust (`moxcms`,
/// already a transitive dependency of `rawler` for its own RAW color work —
/// not a hypothetical library, something a real, actively-maintained
/// project already ships).
///
/// Cross-validates the two independent implementations against each other
/// on the same real transform (sRGB -> ECI-RGBv2, both real ICC profiles,
/// not synthetic) for a handful of known colors — the same discipline that
/// caught FINDINGS.md finding 14's bug in the PSD spike: two independently
/// written implementations agreeing is real corroboration, one implementation
/// agreeing with itself is not.
fn icc_demo() {
    let srgb_bytes =
        std::fs::read("reference/icc-profiles/sRGB.icc").expect("read reference sRGB.icc");
    let eci_bytes = std::fs::read("reference/icc-profiles/ECI-RGBv2.icc")
        .expect("read reference ECI-RGBv2.icc");

    let colors: &[(&str, [f32; 3])] = &[
        ("white", [1.0, 1.0, 1.0]),
        ("black", [0.0, 0.0, 0.0]),
        ("mid-gray", [0.5, 0.5, 0.5]),
        ("red", [1.0, 0.0, 0.0]),
        ("green", [0.0, 1.0, 0.0]),
        ("blue", [0.0, 0.0, 1.0]),
    ];

    println!("sRGB -> ECI-RGBv2, lcms2 (FFI) vs moxcms (pure Rust):\n");
    let lcms2_out = lcms2_transform(&srgb_bytes, &eci_bytes, colors);
    let moxcms_out = moxcms_transform(&srgb_bytes, &eci_bytes, colors);

    println!(
        "{:<10} {:>24} {:>24} {:>12}",
        "color", "lcms2", "moxcms", "max |Δ|"
    );
    let mut worst: f32 = 0.0;
    for (i, (name, _)) in colors.iter().enumerate() {
        let a = lcms2_out[i];
        let b = moxcms_out[i];
        let delta = a
            .iter()
            .zip(b.iter())
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        worst = worst.max(delta);
        println!(
            "{name:<10} {:>24} {:>24} {delta:>12.4}",
            format!("[{:.4}, {:.4}, {:.4}]", a[0], a[1], a[2]),
            format!("[{:.4}, {:.4}, {:.4}]", b[0], b[1], b[2]),
        );
    }
    println!("\nWorst per-channel disagreement across all colors: {worst:.4}");
}

fn lcms2_transform(src_icc: &[u8], dst_icc: &[u8], colors: &[(&str, [f32; 3])]) -> Vec<[f32; 3]> {
    let src = lcms2::Profile::new_icc(src_icc).expect("lcms2: parse sRGB.icc");
    let dst = lcms2::Profile::new_icc(dst_icc).expect("lcms2: parse ECI-RGBv2.icc");
    let transform: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
        &src,
        lcms2::PixelFormat::RGB_FLT,
        &dst,
        lcms2::PixelFormat::RGB_FLT,
        lcms2::Intent::RelativeColorimetric,
    )
    .expect("lcms2: build transform");

    let src_pixels: Vec<[f32; 3]> = colors.iter().map(|(_, c)| *c).collect();
    let mut dst_pixels = vec![[0.0f32; 3]; src_pixels.len()];
    transform.transform_pixels(&src_pixels, &mut dst_pixels);
    dst_pixels
}

fn moxcms_transform(src_icc: &[u8], dst_icc: &[u8], colors: &[(&str, [f32; 3])]) -> Vec<[f32; 3]> {
    let src = moxcms::ColorProfile::new_from_slice(src_icc).expect("moxcms: parse sRGB.icc");
    let dst = moxcms::ColorProfile::new_from_slice(dst_icc).expect("moxcms: parse ECI-RGBv2.icc");
    // moxcms defaults to Perceptual intent, which deliberately compresses
    // out-of-gamut colors into range -- not comparable to lcms2's
    // RelativeColorimetric above. Matched explicitly, or this "disagreement"
    // is just two different, both-correct algorithms, not a finding.
    let options = moxcms::TransformOptions {
        rendering_intent: moxcms::RenderingIntent::RelativeColorimetric,
        prefer_fixed_point: false,
        allow_extended_range_rgb_xyz: true,
        ..Default::default()
    };
    let transform = src
        .create_transform_f32(moxcms::Layout::Rgb, &dst, moxcms::Layout::Rgb, options)
        .expect("moxcms: build transform");

    let src_pixels: Vec<f32> = colors.iter().flat_map(|(_, c)| c.iter().copied()).collect();
    let mut dst_pixels = vec![0.0f32; src_pixels.len()];
    transform
        .transform(&src_pixels, &mut dst_pixels)
        .expect("moxcms: transform");
    dst_pixels
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}
