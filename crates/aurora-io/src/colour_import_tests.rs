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
    let want = expected(&src, [first[0], first[1], first[2]]);
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
fn extended_range_survives_the_conversion() {
    // Adobe green is outside sRGB: red goes negative and stays negative
    // in f16; a value above one stays above one.
    let mut samples = vec![
        f16::from_f32(0.0),
        f16::from_f32(1.0),
        f16::from_f32(0.0),
        f16::from_f32(1.0),
    ];
    let imported = bring_into_working(&mut samples, Tagging::Icc(&adobe_bytes()), false);
    assert!(imported.source.is_some());
    assert!(
        samples.first().is_some_and(|r| r.to_f32() < -0.05),
        "{samples:?}"
    );
    let mut hdr = vec![
        f16::from_f32(1.5),
        f16::from_f32(1.5),
        f16::from_f32(1.5),
        f16::from_f32(1.0),
    ];
    let _ = bring_into_working(&mut hdr, Tagging::Icc(&adobe_bytes()), false);
    assert!(hdr.iter().take(3).all(|c| c.to_f32() > 1.2), "{hdr:?}");
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
