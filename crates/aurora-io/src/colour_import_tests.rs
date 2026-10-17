use std::path::Path;

use aurora_color::{IccProfile, Transform};
use half::f16;

use super::{ColourImport, Tagging, bring_into_working, decode_to_working};
use crate::Image;

const SRGB_ICC: &[u8] = include_bytes!("../../../corpora/icc/sRGB.icc");
const D65: [f64; 2] = [0.3127, 0.3290];

fn adobe_like() -> IccProfile {
    match IccProfile::synthetic_rgb(
        D65,
        [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
        2.199_218_75,
    ) {
        Ok(p) => p,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn adobe_bytes() -> Vec<u8> {
    match adobe_like().to_bytes() {
        Ok(b) => b,
        Err(e) => unreachable!("{e:?}"),
    }
}

/// 2 x 1 RGBA pixels of 8-bit-exact values, tagged `profile`.
fn tagged(profile: IccProfile) -> Image {
    let v = |n: u8| f16::from_f32(f32::from(n) / 255.0);
    let samples = vec![v(51), v(153), v(102), v(204), v(255), v(0), v(0), v(128)];
    match Image::new(2, 1, profile, samples) {
        Ok(i) => i,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn encode(ext: &str, image: &Image) -> Vec<u8> {
    match crate::encode_by_extension(Path::new(ext), image) {
        Ok(b) => b,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn raw(ext: &str, bytes: &[u8]) -> Image {
    match crate::decode_by_extension(Path::new(ext), bytes) {
        Ok(i) => i,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn working(ext: &str, bytes: &[u8]) -> (Image, ColourImport) {
    match decode_to_working(Path::new(ext), bytes) {
        Ok(r) => r,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn bits(image: &Image) -> Vec<u16> {
    image.samples().iter().map(|s| s.to_bits()).collect()
}

/// What the colour channels must become, computed straight from lcms2.
fn expected(source: &IccProfile, px: [f32; 3]) -> [f32; 3] {
    let Ok(t) = Transform::to_working(source) else {
        unreachable!()
    };
    let mut out = [0.0_f32; 3];
    assert!(t.transform(&px, &mut out).is_ok());
    out
}

fn assert_converted(ext: &str, tolerance: f32) {
    let src = adobe_like();
    let bytes = encode(ext, &tagged(adobe_like()));
    let stored = raw(ext, &bytes);
    let (image, imported) = working(ext, &bytes);
    // The pixels are now sRGB and say so.
    assert!(matches!(image.color_space().is_exactly_srgb(), Ok(true)));
    let first: Vec<f32> = stored
        .samples()
        .iter()
        .take(3)
        .map(|s| s.to_f32())
        .collect();
    let &[r, g, b, ..] = first.as_slice() else {
        unreachable!()
    };
    // The conversion clips out-of-gamut results to 0..1.
    let want = expected(&src, [r, g, b]).map(|v| v.clamp(0.0, 1.0));
    for (got, want) in image.samples().iter().take(3).zip(want) {
        assert!(
            (got.to_f32() - want).abs() <= tolerance,
            "{ext}: {got:?} vs {want}"
        );
    }
    // The converted values really differ from the stored ones.
    assert_ne!(bits(&image).get(..3), bits(&stored).get(..3), "{ext}");
    let kept = imported.source.as_ref().map(|s| s.bytes.clone());
    assert_eq!(
        kept,
        Some(adobe_bytes()),
        "{ext}: the source profile is kept"
    );
    assert!(
        imported
            .report
            .info
            .iter()
            .any(|l| l.contains("Converted from")),
        "{ext}: {:?}",
        imported.report
    );
    assert!(imported.report.warnings.is_empty());
}

#[test]
fn a_profiled_png_is_converted_into_srgb() {
    assert_converted("a.png", 1e-3);
}

#[test]
fn a_profiled_jpeg_is_converted_into_srgb() {
    // JPEG is lossy; the stored value is the one read back raw, so the
    // comparison is against that and the tolerance is the f16 step.
    assert_converted("a.jpg", 1e-3);
}

#[test]
fn a_profiled_tiff_is_converted_into_srgb() {
    assert_converted("a.tif", 1e-3);
}

#[test]
fn alpha_is_untouched_by_the_conversion() {
    let bytes = encode("a.png", &tagged(adobe_like()));
    let stored = raw("a.png", &bytes);
    let (image, _) = working("a.png", &bytes);
    for i in [3_usize, 7] {
        assert_eq!(image.samples().get(i), stored.samples().get(i));
    }
}

#[test]
fn an_srgb_profile_is_a_bit_identical_no_op() {
    let Ok(srgb_file_profile) = IccProfile::from_bytes(SRGB_ICC) else {
        unreachable!()
    };
    for (ext, profile) in [
        ("a.png", srgb_file_profile),
        ("a.tif", IccProfile::srgb()),
        ("a.jpg", IccProfile::srgb()),
    ] {
        // `a.png` with the colord sRGB profile embeds a real iCCP.
        let bytes = encode(ext, &tagged(profile));
        let (image, imported) = working(ext, &bytes);
        assert_eq!(bits(&image), bits(&raw(ext, &bytes)), "{ext}");
        assert_eq!(imported, ColourImport::default(), "{ext}");
    }
}

#[test]
fn an_untagged_file_is_assumed_srgb_with_an_info_line_and_untouched_pixels() {
    // A PNG without iCCP or sRGB chunks: write it with the png crate.
    let mut bytes = Vec::new();
    {
        let mut info = png::Info::with_size(1, 1);
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        let Ok(encoder) = png::Encoder::with_info(&mut bytes, info) else {
            unreachable!()
        };
        let Ok(mut writer) = encoder.write_header() else {
            unreachable!()
        };
        assert!(writer.write_image_data(&[10, 200, 30, 255]).is_ok());
    }
    let (image, imported) = working("a.png", &bytes);
    assert_eq!(bits(&image), bits(&raw("a.png", &bytes)));
    assert_eq!(imported.source, None);
    assert_eq!(imported.report.info.len(), 1);
    assert!(
        imported
            .report
            .info
            .iter()
            .any(|l| l.contains("assumed sRGB"))
    );
    // A PNG sRGB chunk is tagged, not untagged: no note.
    let srgb = encode("a.png", &tagged(IccProfile::srgb()));
    assert_eq!(working("a.png", &srgb).1, ColourImport::default());
}

#[test]
fn unusable_profiles_are_ignored_with_a_line_and_never_fail_the_open() {
    // Garbage bytes, a gray profile on RGB data, and an abstract-class
    // profile: each opens, pixels untouched, one warning.
    let Ok(gray) = IccProfile::synthetic_gray(2.2).and_then(|p| p.to_bytes()) else {
        unreachable!()
    };
    let mut abst = adobe_bytes();
    if let Some(c) = abst.get_mut(12..16) {
        c.copy_from_slice(b"abst");
    }
    let image = vec![f16::from_f32(0.25); 8];
    for (name, bad) in [
        ("garbage", b"not an icc profile at all".to_vec()),
        ("gray", gray),
        ("abstract", abst),
        ("oversized", vec![0_u8; aurora_color::MAX_PROFILE_BYTES + 1]),
    ] {
        let mut samples = image.clone();
        let imported = bring_into_working(&mut samples, Tagging::Icc(&bad), false);
        assert_eq!(samples, image, "{name}: pixels untouched");
        assert_eq!(imported.source, None, "{name}");
        assert_eq!(
            imported.report.warnings.len(),
            1,
            "{name}: {:?}",
            imported.report
        );
        assert!(
            imported
                .report
                .warnings
                .iter()
                .any(|l| l.contains("not used")),
            "{name}"
        );
    }
}

#[test]
fn a_png_carrying_an_undecodable_profile_still_opens() {
    let mut bytes = Vec::new();
    {
        let mut info = png::Info::with_size(1, 1);
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = Some(b"junk junk junk".to_vec().into());
        let Ok(encoder) = png::Encoder::with_info(&mut bytes, info) else {
            unreachable!()
        };
        let Ok(mut writer) = encoder.write_header() else {
            unreachable!()
        };
        assert!(writer.write_image_data(&[10, 200, 30, 255]).is_ok());
    }
    // The strict legacy reading refuses it; the colour-managed one ignores it.
    assert!(crate::png::decode(&bytes).is_err());
    let (_, imported) = working("a.png", &bytes);
    assert_eq!(imported.report.warnings.len(), 1);
}

#[test]
fn a_grayscale_profile_on_grayscale_data_is_reported_not_applied() {
    let Ok(gray) = IccProfile::synthetic_gray(1.8).and_then(|p| p.to_bytes()) else {
        unreachable!()
    };
    let mut samples = vec![f16::from_f32(0.5); 4];
    let imported = bring_into_working(&mut samples, Tagging::Icc(&gray), true);
    assert_eq!(samples, vec![f16::from_f32(0.5); 4]);
    assert_eq!(imported.report.warnings.len(), 1);
}

#[test]
fn out_of_gamut_colour_is_clipped_after_the_conversion() {
    // Adobe green is outside sRGB: its red would go negative. The result
    // is clipped to 0..1 like Photoshop's conversion (design owner,
    // 2026-10-17); alpha is untouched.
    let mut samples = vec![
        f16::from_f32(0.0),
        f16::from_f32(1.0),
        f16::from_f32(0.0),
        f16::from_f32(0.5),
    ];
    let imported = bring_into_working(&mut samples, Tagging::Icc(&adobe_bytes()), false);
    assert!(imported.source.is_some());
    assert_eq!(
        samples.first().map(|r| r.to_f32()),
        Some(0.0),
        "{samples:?}"
    );
    assert!(
        samples.iter().all(|c| (0.0..=1.0).contains(&c.to_f32())),
        "{samples:?}"
    );
    assert_eq!(
        samples.get(3).map(|a| a.to_f32()),
        Some(0.5),
        "alpha untouched"
    );
    // A value above one is clipped to one, too.
    let mut hdr = vec![
        f16::from_f32(1.5),
        f16::from_f32(1.5),
        f16::from_f32(1.5),
        f16::from_f32(1.0),
    ];
    let _ = bring_into_working(&mut hdr, Tagging::Icc(&adobe_bytes()), false);
    assert!(hdr.iter().take(3).all(|c| c.to_f32() <= 1.0), "{hdr:?}");
    // An sRGB-equivalent profile is still a bit-exact no-op: nothing is
    // clipped when nothing was converted.
    let mut untouched = vec![f16::from_f32(1.5); 4];
    let before = untouched.clone();
    let _ = bring_into_working(&mut untouched, Tagging::Untagged, false);
    assert_eq!(untouched, before);
}

#[test]
fn a_large_image_converts_in_chunks_without_changing_the_result() {
    // More pixels than one chunk: every pixel of a uniform image gets the
    // same answer.
    let n = 40_000;
    let mut samples: Vec<f16> = (0..n)
        .flat_map(|_| {
            [
                f16::from_f32(0.2),
                f16::from_f32(0.6),
                f16::from_f32(0.4),
                f16::from_f32(1.0),
            ]
        })
        .collect();
    let _ = bring_into_working(&mut samples, Tagging::Icc(&adobe_bytes()), false);
    let first: Vec<u16> = samples.iter().take(4).map(|s| s.to_bits()).collect();
    assert!(
        samples
            .chunks_exact(4)
            .all(|p| p.iter().map(|s| s.to_bits()).collect::<Vec<_>>() == first)
    );
}

#[test]
fn white_stays_white_under_the_relative_colorimetric_intent() {
    // A profile whose white point is D50, not sRGB's D65: relative
    // colorimetric maps its white to the working space's white; absolute
    // colorimetric would keep the tint (and so would not be white).
    let Ok(d50) = IccProfile::synthetic_rgb(
        [0.3457, 0.3585],
        [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        0.0,
    ) else {
        unreachable!()
    };
    let Ok(bytes) = d50.to_bytes() else {
        unreachable!()
    };
    let mut samples = vec![
        f16::from_f32(1.0),
        f16::from_f32(1.0),
        f16::from_f32(1.0),
        f16::from_f32(1.0),
    ];
    let imported = bring_into_working(&mut samples, Tagging::Icc(&bytes), false);
    assert!(imported.source.is_some(), "{:?}", imported.report);
    for c in samples.iter().take(3) {
        assert!((c.to_f32() - 1.0).abs() < 2e-3, "{samples:?}");
    }
}

#[test]
fn the_parallel_conversion_equals_the_single_threaded_one_and_skips_transparent_pixels() {
    use super::{ConvertDirection, convert_profile_rgba, convert_rgba};
    // Over the parallel threshold (2^20 samples), a varied pattern, with
    // every third pixel fully transparent.
    let pixels = 300_000_usize;
    let mut original: Vec<f16> = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        let t = |k: usize| f16::from_f32(((i * k) % 251) as f32 / 250.0);
        let a = if i % 3 == 0 { 0.0 } else { 1.0 };
        original.extend([t(7), t(13), t(29), f16::from_f32(a)]);
    }
    let bytes = adobe_bytes();
    let mut parallel = original.clone();
    assert!(convert_profile_rgba(&mut parallel, &bytes, ConvertDirection::ToWorking).is_ok());
    let Ok(adobe) = IccProfile::from_bytes(&bytes) else {
        unreachable!()
    };
    let Ok(t) = Transform::to_working(&adobe) else {
        unreachable!()
    };
    let mut single = original.clone();
    assert!(convert_rgba(&mut single, &t).is_ok());
    assert_eq!(bits_of(&parallel), bits_of(&single));
    assert_ne!(bits_of(&parallel), bits_of(&original));
    for (i, (p, o)) in parallel
        .chunks_exact(4)
        .zip(original.chunks_exact(4))
        .enumerate()
    {
        if i % 3 == 0 {
            assert_eq!(bits_of(p), bits_of(o), "transparent pixel {i} untouched");
        }
        assert_eq!(p.get(3), o.get(3), "alpha {i}");
    }
}

fn bits_of(s: &[f16]) -> Vec<u16> {
    s.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn a_failed_chunk_leaves_the_image_exactly_as_it_was() {
    use super::{ConvertDirection, convert_profile_rgba_with};
    let bytes = adobe_bytes();
    // Small (single-threaded path), a failure in the second chunk, so the
    // first chunk had already converted before it failed.
    let pixels = 40_000_usize;
    let original: Vec<f16> = (0..pixels)
        .flat_map(|i| {
            let t = |k: usize| f16::from_f32(((i * k) % 251) as f32 / 250.0);
            [t(3), t(5), t(7), f16::from_f32(1.0)]
        })
        .collect();
    let mut samples = original.clone();
    assert!(
        convert_profile_rgba_with(&mut samples, &bytes, ConvertDirection::ToWorking, Some(1))
            .is_err()
    );
    assert_eq!(bits_of(&samples), bits_of(&original));
    // The parallel path (over the threshold), failing in part zero's chunk.
    let big: Vec<f16> = (0..300_000_usize)
        .flat_map(|i| {
            let t = |k: usize| f16::from_f32(((i * k) % 251) as f32 / 250.0);
            [t(3), t(5), t(7), f16::from_f32(1.0)]
        })
        .collect();
    let mut samples = big.clone();
    assert!(
        convert_profile_rgba_with(&mut samples, &bytes, ConvertDirection::ToWorking, Some(1))
            .is_err()
    );
    assert_eq!(bits_of(&samples), bits_of(&big));
    // And without the injection the same input converts.
    assert!(
        convert_profile_rgba_with(&mut samples, &bytes, ConvertDirection::ToWorking, None).is_ok()
    );
    assert_ne!(bits_of(&samples), bits_of(&big));
}

#[test]
fn clamping_keeps_alpha_and_fixes_out_of_range_colour() {
    let mut samples = vec![
        f16::from_f32(-1.15),
        f16::from_f32(1.4),
        f16::from_f32(0.5),
        f16::from_f32(0.25),
    ];
    super::clamp_colour_unit(&mut samples);
    let v: Vec<f32> = samples.iter().map(|s| s.to_f32()).collect();
    assert_eq!(v, vec![0.0, 1.0, 0.5, 0.25]);
}

#[test]
fn a_failed_conversion_is_reported_truthfully_and_leaves_the_pixels_alone() {
    let original = vec![
        f16::from_f32(0.2),
        f16::from_f32(0.6),
        f16::from_f32(0.4),
        f16::from_f32(1.0),
    ];
    let mut samples = original.clone();
    super::TEST_FAIL_CHUNK.with(|c| c.set(Some(0)));
    let imported = bring_into_working(&mut samples, Tagging::Icc(&adobe_bytes()), false);
    super::TEST_FAIL_CHUNK.with(|c| c.set(None));
    assert_eq!(bits_of(&samples), bits_of(&original));
    assert_eq!(imported.source, None, "no source profile is claimed");
    assert!(imported.report.info.is_empty(), "no 'Converted' line");
    assert_eq!(imported.report.warnings.len(), 1);
    assert!(
        imported
            .report
            .warnings
            .iter()
            .any(|l| l.contains("treated as sRGB"))
    );
}
