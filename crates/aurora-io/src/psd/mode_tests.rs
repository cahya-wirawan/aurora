//! CMYK and 32-bit floating point PSD files (task 146): synthetic files
//! built in-test, the tracked 32-bit psd-tools fixture, hostile headers,
//! and a sweep over the refused-until-now corpus files.

// Tests index fixed-size arrays they built and compare exact constants.
#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::many_single_char_names
)]

use aurora_color::{CmykTransform, IccProfile, Transform};
use half::f16;

use super::effects_tests::{V, descriptor_bytes};
use super::test_writer::{TestLayer, TestPsd};
use super::{PsdDocument, decode, merged_image, merged_image_in_working_space, read};
use crate::error::IoError;

fn open(bytes: &[u8]) -> PsdDocument {
    match read(bytes) {
        Ok(d) => d,
        Err(e) => unreachable!("{e:?}"),
    }
}

/// The first `n` pixels of the first layer's image.
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

fn near(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn resource_1039(profile: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(b"8BIM");
    o.extend_from_slice(&1039_u16.to_be_bytes());
    o.extend_from_slice(&[0, 0]);
    o.extend_from_slice(&u32::try_from(profile.len()).unwrap_or(0).to_be_bytes());
    o.extend_from_slice(profile);
    if profile.len() % 2 == 1 {
        o.push(0);
    }
    o
}

fn synthetic_cmyk_bytes() -> Vec<u8> {
    match IccProfile::synthetic_cmyk().and_then(|p| p.to_bytes()) {
        Ok(b) => b,
        Err(e) => unreachable!("{e:?}"),
    }
}

/// Ink-coverage conversion through the synthetic profile, the reference
/// the reader is held to.
fn reference_cmyk(ink: [f32; 4]) -> [f32; 3] {
    let Ok(profile) = IccProfile::synthetic_cmyk() else {
        unreachable!("profile")
    };
    let Ok(t) = CmykTransform::to_working(&profile) else {
        unreachable!("transform")
    };
    let mut rgb = [0.0_f32; 3];
    let Ok(()) = t.transform(&ink, &mut rgb) else {
        unreachable!("convert")
    };
    rgb.map(|v| v.clamp(0.0, 1.0))
}

/// A CMYK file with one layer of `pixels` (`[c, m, y, k, alpha]` as
/// stored, 8-bit), carrying `profile` (resource 1039) when given.
fn cmyk_file(pixels: &[[u16; 5]], profile: Option<&[u8]>, compression: u16) -> Vec<u8> {
    let n = i32::try_from(pixels.len()).unwrap_or(1);
    TestPsd::new(1, u32::try_from(n).unwrap_or(1), 1, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            if let Some(profile) = profile {
                p.resources = resource_1039(profile);
            }
            p.layers.push(
                TestLayer::cmyk("L", 0, 0, n, 1, 8, pixels).with(|l| l.compression = compression),
            );
        })
        .write()
}

const STORED_PIXELS: [[u16; 5]; 4] = [
    // No ink at all: paper white.
    [255, 255, 255, 255, 255],
    // Full black ink only (stored 0 = full ink).
    [255, 255, 255, 0, 255],
    // Full cyan ink.
    [0, 255, 255, 255, 255],
    // A mixed colour, half transparent.
    [100, 200, 50, 180, 128],
];

fn ink_of(stored: [u16; 5]) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| 1.0 - f32::from(stored[i]) / 255.0)
}

// ---------------------------------------------------------------------
// CMYK
// ---------------------------------------------------------------------

#[test]
fn a_cmyk_layer_is_inverted_and_converted_through_its_profile() {
    let profile = synthetic_cmyk_bytes();
    let document = open(&cmyk_file(&STORED_PIXELS, Some(&profile), 0));
    let got = first_pixels(&document, 4);
    for (px, stored) in got.iter().zip(STORED_PIXELS) {
        let want = reference_cmyk(ink_of(stored));
        let alpha = f32::from(stored[4]) / 255.0;
        assert!(
            near(*px, [want[0], want[1], want[2], alpha], 3e-3),
            "{stored:?}: got {px:?}, want {want:?} alpha {alpha}"
        );
    }
    // Paper is white, black ink is dark: the inversion went the right way.
    assert!(got[0][..3].iter().all(|c| *c > 0.97), "{:?}", got[0]);
    assert!(got[1][..3].iter().all(|c| *c < 0.1), "{:?}", got[1]);
    assert!(got[2][0] < 0.1 && got[2][1] > 0.9, "cyan: {:?}", got[2]);
}

#[test]
fn a_cmyk_open_reports_the_conversion_and_keeps_no_rgb_profile() {
    let profile = synthetic_cmyk_bytes();
    let document = open(&cmyk_file(&STORED_PIXELS, Some(&profile), 0));
    // The CMYK profile is not an RGB source profile: it is not kept as one.
    assert!(document.colour.source.is_none());
    assert_eq!(
        document.report.items.len(),
        1,
        "{:?}",
        document.report.items
    );
    let line = &document.report.items[0];
    assert!(
        line.contains("CMYK profile \"Aurora synthetic CMYK\""),
        "{line}"
    );
    assert!(
        line.contains("converted") && line.contains("clipped"),
        "{line}"
    );
    assert!(line.contains("not kept"), "{line}");
    assert!(
        document
            .colour
            .report
            .info
            .iter()
            .any(|l| l.contains("Converted from the CMYK profile"))
    );
}

#[test]
fn a_cmyk_file_without_a_profile_uses_the_naive_formula_and_says_so() {
    let document = open(&cmyk_file(&STORED_PIXELS, None, 0));
    let got = first_pixels(&document, 4);
    for (px, stored) in got.iter().zip(STORED_PIXELS) {
        let want = aurora_color::naive_cmyk_to_rgb(ink_of(stored));
        assert!(
            near(
                *px,
                [want[0], want[1], want[2], f32::from(stored[4]) / 255.0],
                2e-3
            ),
            "{stored:?}: {px:?} vs {want:?}"
        );
    }
    assert_eq!(
        document.report.items.len(),
        1,
        "{:?}",
        document.report.items
    );
    let line = &document.report.items[0];
    assert!(line.contains("no CMYK colour profile"), "{line}");
    assert!(line.contains("simple formula") && line.contains("will not match Photoshop"));
}

#[test]
fn an_unusable_cmyk_profile_falls_back_to_the_formula_with_the_reason() {
    let rgb = IccProfile::srgb().to_bytes().unwrap_or_default();
    for (bad, why) in [
        (b"junk".to_vec(), "not a valid ICC profile"),
        (rgb, "an RGB colour space, not CMYK"),
    ] {
        let document = open(&cmyk_file(&STORED_PIXELS, Some(&bad), 0));
        let want = aurora_color::naive_cmyk_to_rgb(ink_of(STORED_PIXELS[2]));
        assert!(near(
            first_pixels(&document, 3)[2],
            [want[0], want[1], want[2], 1.0],
            2e-3
        ));
        assert_eq!(
            document.report.items.len(),
            1,
            "{:?}",
            document.report.items
        );
        let line = &document.report.items[0];
        assert!(
            line.contains("was not used") && line.contains(why),
            "{line}"
        );
        assert!(line.contains("simple formula"), "{line}");
    }
}

#[test]
fn every_compression_gives_the_same_cmyk_pixels() {
    let profile = synthetic_cmyk_bytes();
    let raw = first_pixels(&open(&cmyk_file(&STORED_PIXELS, Some(&profile), 0)), 4);
    for compression in [1_u16, 2, 3] {
        let got = first_pixels(
            &open(&cmyk_file(&STORED_PIXELS, Some(&profile), compression)),
            4,
        );
        assert_eq!(raw, got, "compression {compression}");
    }
}

#[test]
fn sixteen_bit_cmyk_matches_the_eight_bit_values() {
    let profile = synthetic_cmyk_bytes();
    let n = STORED_PIXELS.len();
    let wide: Vec<[u16; 5]> = STORED_PIXELS.iter().map(|p| p.map(|v| v * 257)).collect();
    let file = TestPsd::new(1, 4, 1, 16)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            p.lr16 = true;
            p.resources = resource_1039(&profile);
            p.layers.push(TestLayer::cmyk("L", 0, 0, 4, 1, 16, &wide));
        })
        .write();
    let eight = first_pixels(&open(&cmyk_file(&STORED_PIXELS, Some(&profile), 0)), n);
    let sixteen = first_pixels(&open(&file), n);
    for (a, b) in eight.iter().zip(&sixteen) {
        assert!(near(*a, *b, 2e-3), "{a:?} vs {b:?}");
    }
}

#[test]
fn a_transparent_cmyk_pixel_is_transparent_black_and_a_missing_channel_is_no_ink() {
    // Alpha 0: the colour cannot be seen and is not converted.
    let document = open(&cmyk_file(&[[0, 0, 0, 0, 0]], None, 0));
    assert_eq!(first_pixels(&document, 1)[0], [0.0, 0.0, 0.0, 0.0]);
    // A layer without its black channel: that ink is absent (paper), not
    // full black, and the layer says so.
    let file = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            p.layers.push(
                TestLayer::cmyk("L", 0, 0, 1, 1, 8, &[[255, 255, 255, 0, 255]])
                    .with(|l| l.channels.retain(|(id, _)| *id != 3)),
            );
        })
        .write();
    let document = open(&file);
    assert!(near(
        first_pixels(&document, 1)[0],
        [1.0, 1.0, 1.0, 1.0],
        2e-3
    ));
    assert!(
        document
            .report
            .items
            .iter()
            .any(|l| l.contains("missing a colour channel")),
        "{:?}",
        document.report.items
    );
}

#[test]
fn a_cmyk_flat_file_opens_from_its_merged_image() {
    let profile = synthetic_cmyk_bytes();
    let plane = |c: usize| -> Vec<u8> {
        STORED_PIXELS
            .iter()
            .map(|p| u8::try_from(p[c]).unwrap_or(0))
            .collect()
    };
    for compression in [0_u16, 1, 2, 3] {
        let file = TestPsd::new(1, 4, 1, 8)
            .with(|p| {
                p.color_mode = 4;
                p.channels = 4;
                p.resources = resource_1039(&profile);
                p.merged = Some(vec![plane(0), plane(1), plane(2), plane(3)]);
                p.merged_compression = compression;
            })
            .write();
        let document = open(&file);
        let got = first_pixels(&document, 4);
        for (px, stored) in got.iter().zip(STORED_PIXELS) {
            let want = reference_cmyk(ink_of(stored));
            assert!(
                near(*px, [want[0], want[1], want[2], 1.0], 3e-3),
                "compression {compression}, {stored:?}: {px:?} vs {want:?}"
            );
        }
        // The merged-image entry points agree.
        let merged = match merged_image(&file) {
            Ok(i) => i,
            Err(e) => unreachable!("{e:?}"),
        };
        let working = match merged_image_in_working_space(&file) {
            Ok(i) => i,
            Err(e) => unreachable!("{e:?}"),
        };
        assert_eq!(merged.samples(), working.samples());
        assert_eq!(
            &merged.samples()[..4],
            &document.pixels[0].image.samples()[..4]
        );
    }
}

#[test]
fn cmyk_adjustments_and_effects_are_left_out_and_counted() {
    let file = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            p.layers.push(
                TestLayer::cmyk("L", 0, 0, 1, 1, 8, &[[255, 255, 255, 255, 255]])
                    .with(|l| l.blocks.push((*b"lfx2", vec![0; 16]))),
            );
            p.layers.push(TestLayer::empty("Invert").with(|l| {
                l.blocks.push((*b"nvrt", Vec::new()));
            }));
            p.layers.push(TestLayer::empty("Fill").with(|l| {
                l.blocks.push((*b"SoCo", vec![0; 8]));
            }));
        })
        .write();
    let document = open(&file);
    let text = document.report.items.join("\n");
    assert!(text.contains("1 adjustment layer was left out"), "{text}");
    assert!(
        text.contains("layer effects of 1 layer was left out"),
        "{text}"
    );
    // Only the pixel layer survives: no adjustment layer was imported with
    // RGB semantics, and nothing else about the effects was reported.
    assert_eq!(document.pixels.len(), 1);
    assert_eq!(document.layers.roots().len(), 1, "{text}");
    assert_eq!(document.report.items.len(), 5, "{text}");
}

#[test]
fn a_cmyk_header_needs_four_channels_and_thirty_two_bit_cmyk_does_not_exist() {
    let thin = TestPsd::new(1, 1, 1, 8).with(|p| p.color_mode = 4).write();
    assert!(matches!(decode(&thin), Err(IoError::PsdMalformed { .. })));
    let deep = TestPsd::new(1, 1, 1, 32)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
        })
        .write();
    assert!(matches!(
        decode(&deep),
        Err(IoError::UnsupportedPsdDepth(32))
    ));
}

// ---------------------------------------------------------------------
// 32-bit floating point
// ---------------------------------------------------------------------

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_srgb_bytes() -> Vec<u8> {
    match IccProfile::synthetic_rgb(
        [0.3127, 0.3290],
        [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        1.0,
    )
    .and_then(|p| p.to_bytes())
    {
        Ok(b) => b,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn float_plane(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(f32::to_be_bytes).collect()
}

/// A 32-bit RGB file with one layer of `rgba` pixels.
fn float_file(rgba: &[[f32; 4]], profile: Option<&[u8]>, compression: u16) -> Vec<u8> {
    let n = i32::try_from(rgba.len()).unwrap_or(1);
    TestPsd::new(1, u32::try_from(n).unwrap_or(1), 1, 32)
        .with(|p| {
            p.lr16 = true;
            if let Some(profile) = profile {
                p.resources = resource_1039(profile);
            }
            p.layers.push(
                TestLayer::floats("L", 0, 0, n, 1, rgba).with(|l| l.compression = compression),
            );
        })
        .write()
}

const FLOATS: [[f32; 4]; 4] = [
    [0.0, 0.0, 0.0, 1.0],
    [0.5, 0.25, 0.01, 1.0],
    [1.0, 1.0, 1.0, 0.5],
    [0.18, 0.18, 0.18, 1.0],
];

#[test]
fn a_32_bit_layer_is_read_as_linear_light_and_encoded_to_srgb() {
    let linear = linear_srgb_bytes();
    let document = open(&float_file(&FLOATS, Some(&linear), 0));
    let got = first_pixels(&document, 4);
    for (px, src) in got.iter().zip(FLOATS) {
        let want = [
            srgb_encode(src[0]),
            srgb_encode(src[1]),
            srgb_encode(src[2]),
            src[3],
        ];
        assert!(near(*px, want, 3e-3), "{src:?}: {px:?} vs {want:?}");
    }
    // The 0.18 mid-grey of linear light is about 0.46 encoded.
    assert!((got[3][0] - 0.461).abs() < 3e-3, "{:?}", got[3]);
    let line = document.report.items.join("\n");
    assert!(line.contains("32-bit floating point"), "{line}");
    assert!(
        line.contains("linear light") && line.contains("not kept"),
        "{line}"
    );
}

#[test]
fn a_32_bit_file_without_a_profile_is_assumed_linear_srgb() {
    let with = first_pixels(
        &open(&float_file(&FLOATS, Some(&linear_srgb_bytes()), 0)),
        4,
    );
    let document = open(&float_file(&FLOATS, None, 0));
    let without = first_pixels(&document, 4);
    for (a, b) in with.iter().zip(&without) {
        assert!(near(*a, *b, 2e-3), "{a:?} vs {b:?}");
    }
    let text = document.report.items.join("\n");
    assert!(text.contains("linear light, sRGB assumed"), "{text}");
}

#[test]
fn a_wide_gamut_linear_profile_converts_primaries_too() {
    let adobe = match IccProfile::synthetic_rgb(
        [0.3127, 0.3290],
        [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
        1.0,
    ) {
        Ok(p) => p,
        Err(e) => unreachable!("{e:?}"),
    };
    let Ok(bytes) = adobe.to_bytes() else {
        unreachable!()
    };
    let src = [[0.1_f32, 0.6, 0.2, 1.0]];
    let got = first_pixels(&open(&float_file(&src, Some(&bytes), 0)), 1)[0];
    let Ok(t) = Transform::to_working(&adobe) else {
        unreachable!()
    };
    let mut want = [0.0_f32; 3];
    assert!(t.transform(&[0.1, 0.6, 0.2], &mut want).is_ok());
    assert!(
        near(
            got,
            [
                want[0].clamp(0.0, 1.0),
                want[1].clamp(0.0, 1.0),
                want[2].clamp(0.0, 1.0),
                1.0
            ],
            4e-3
        ),
        "{got:?} vs {want:?}"
    );
    // It is not the sRGB-primaries answer.
    assert!((got[0] - srgb_encode(0.1)).abs() > 0.01, "{got:?}");
}

#[test]
fn highlights_nan_and_infinity_are_clipped_and_reported() {
    let src = [
        [4.0_f32, 0.5, 0.5, 1.0],
        [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0],
        [1e30, -2.0, 0.5, 1.0],
    ];
    let document = open(&float_file(&src, Some(&linear_srgb_bytes()), 0));
    let got = first_pixels(&document, 3);
    assert_eq!(got[0][0], 1.0, "{:?}", got[0]);
    assert!(
        got.iter()
            .all(|p| p.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c))),
        "{got:?}"
    );
    assert_eq!(got[1][0], 0.0, "NaN is black: {:?}", got[1]);
    assert_eq!(got[1][1], 1.0, "+inf clips to white: {:?}", got[1]);
    assert_eq!(got[1][2], 0.0, "-inf clips to black: {:?}", got[1]);
    assert_eq!(got[2][1], 0.0, "negative is black: {:?}", got[2]);
    let text = document.report.items.join("\n");
    assert!(text.contains("brighter than white"), "{text}");
    // An in-range file does not say it.
    let calm = open(&float_file(&FLOATS, Some(&linear_srgb_bytes()), 0));
    assert!(!calm.report.items.join("\n").contains("brighter than white"));
}

#[test]
fn every_compression_gives_the_same_float_pixels() {
    let linear = linear_srgb_bytes();
    let raw = first_pixels(&open(&float_file(&FLOATS, Some(&linear), 0)), 4);
    for compression in [1_u16, 2, 3] {
        let got = first_pixels(&open(&float_file(&FLOATS, Some(&linear), compression)), 4);
        assert_eq!(raw, got, "compression {compression}");
    }
}

#[test]
fn float_prediction_deinterleaves_byte_planes_after_the_delta() {
    // One row of three samples. Stored: the four byte planes
    // (all first bytes, then all second bytes, ...) delta-coded bytewise.
    let samples = [1.0_f32, -2.5, 0.1];
    let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_be_bytes()).collect();
    let mut planar = Vec::new();
    for k in 0..4 {
        for i in 0..3 {
            planar.push(bytes[i * 4 + k]);
        }
    }
    let mut coded = Vec::new();
    let mut previous = 0_u8;
    for b in &planar {
        coded.push(b.wrapping_sub(previous));
        previous = *b;
    }
    super::undo_prediction(&mut coded, 12, 4);
    assert_eq!(coded, bytes);
    // Rows are independent, and a ragged tail row is not a panic.
    let mut two = coded.clone();
    two.extend_from_slice(&[1, 2, 3, 4, 5]);
    super::undo_prediction(&mut two, 12, 4);
}

#[test]
fn a_32_bit_mask_is_float_coverage() {
    let file = TestPsd::new(1, 2, 1, 32)
        .with(|p| {
            p.lr16 = true;
            p.layers.push(
                TestLayer::floats("L", 0, 0, 2, 1, &[[0.5, 0.5, 0.5, 1.0]; 2]).with(|l| {
                    l.mask = Some((0, 0, 1, 2, 0, 0));
                    l.channels.push((-2, float_plane([1.0, 0.25])));
                }),
            );
        })
        .write();
    let document = open(&file);
    let Some(mask) = document.masks.first() else {
        unreachable!("a mask")
    };
    assert_eq!(mask.coverage, vec![f16::ONE, f16::from_f32(0.25)]);
}

#[test]
fn a_32_bit_grayscale_file_is_linear_grey_encoded_to_srgb() {
    let file = TestPsd::new(1, 2, 1, 32)
        .with(|p| {
            p.lr16 = true;
            p.color_mode = 1;
            p.channels = 1;
            let mut layer = TestLayer::floats(
                "L",
                0,
                0,
                2,
                1,
                &[[0.18, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]],
            );
            layer.channels.retain(|(id, _)| *id == -1 || *id == 0);
            p.layers.push(layer);
        })
        .write();
    let document = open(&file);
    let got = first_pixels(&document, 2);
    assert!(
        near(got[0], [0.461, 0.461, 0.461, 1.0], 3e-3),
        "{:?}",
        got[0]
    );
    assert!(near(got[1], [1.0, 1.0, 1.0, 1.0], 1e-3), "{:?}", got[1]);
    let text = document.report.items.join("\n");
    assert!(text.contains("linear grey") && text.contains("Grayscale file is held as RGB"));
}

#[test]
fn a_32_bit_flat_file_opens_from_its_merged_image() {
    let linear = linear_srgb_bytes();
    let (r, g, b) = (
        float_plane([0.0, 0.5, 1.0]),
        float_plane([0.0, 0.25, 1.0]),
        float_plane([0.0, 0.01, 1.0]),
    );
    for compression in [0_u16, 1, 2, 3] {
        let file = TestPsd::new(1, 3, 1, 32)
            .with(|p| {
                p.resources = resource_1039(&linear);
                p.merged = Some(vec![r.clone(), g.clone(), b.clone()]);
                p.merged_compression = compression;
            })
            .write();
        let got = first_pixels(&open(&file), 3);
        assert!(near(
            got[1],
            [srgb_encode(0.5), srgb_encode(0.25), srgb_encode(0.01), 1.0],
            3e-3
        ));
        assert!(near(got[2], [1.0; 4], 1e-3));
        let working = match merged_image_in_working_space(&file) {
            Ok(i) => i,
            Err(e) => unreachable!("{e:?}"),
        };
        let first: Vec<f32> = working.samples()[4..8].iter().map(|v| v.to_f32()).collect();
        assert!(near([first[0], first[1], first[2], first[3]], got[1], 1e-6));
    }
}

#[test]
fn an_srgb_encoded_profile_on_float_data_is_taken_at_its_word() {
    let srgb = IccProfile::srgb().to_bytes().unwrap_or_default();
    let document = open(&float_file(&FLOATS, Some(&srgb), 0));
    let got = first_pixels(&document, 4);
    assert!(near(got[1], [0.5, 0.25, 0.01, 1.0], 1e-3), "{:?}", got[1]);
    assert!(
        document
            .report
            .items
            .join("\n")
            .contains("used as encoded sRGB"),
        "{:?}",
        document.report.items
    );
}

#[test]
fn the_tracked_32_bit_fixture_matches_photoshops_own_16_bit_conversion() {
    // `16bit5x5.psd` and `32bit5x5.psd` are the same picture saved by
    // Photoshop in 16-bit sRGB and in 32-bit linear light. Reading the
    // 32-bit one (linear values, the Linear RGB profile, sRGB curve) must
    // land on the 16-bit file's values: this is the ground truth for the
    // choice of reading 32-bit data as linear light.
    let a = open(include_bytes!("../../tests/fixtures/psd/16bit5x5.psd"));
    let b = open(include_bytes!("../../tests/fixtures/psd/32bit5x5.psd"));
    assert_eq!(a.pixels.len(), b.pixels.len());
    let mut worst = 0.0_f32;
    for (x, y) in a.pixels.iter().zip(&b.pixels) {
        for (u, v) in x
            .image
            .samples()
            .chunks_exact(4)
            .zip(y.image.samples().chunks_exact(4))
        {
            for c in 0..3 {
                worst = worst.max((u[c].to_f32() - v[c].to_f32()).abs());
            }
        }
    }
    println!("32-bit against 16-bit: worst channel difference {worst}");
    // Half an 8-bit level: far inside what the f16 tiles distinguish.
    assert!(worst < 0.5 / 255.0, "{worst}");
}

/// A file of `mode`/`depth` whose one layer claims `side` x `side` pixels
/// but stores one byte per channel.
fn greedy_layer_file(mode: u16, depth: u16, channels: u16, side: i32) -> Vec<u8> {
    TestPsd::new(1, 8, 8, depth)
        .with(|p| {
            p.color_mode = mode;
            p.channels = channels;
            p.lr16 = depth != 8;
            let mut l = TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]]);
            l.right = side;
            l.bottom = side;
            if mode == 4 {
                l.channels.push((3, vec![0]));
            }
            p.layers.push(l);
        })
        .write()
}

#[test]
fn hostile_cmyk_and_float_headers_are_refused_by_name_before_any_buffer() {
    for (mode, depth, channels) in [(4_u16, 8_u16, 4_u16), (4, 16, 5), (3, 32, 3), (1, 32, 1)] {
        // A document-ceiling canvas with 16 bytes of data: the pixel
        // budget refuses it from the header alone.
        let flat = TestPsd::new(1, 30_000, 30_000, depth)
            .with(|p| {
                p.color_mode = mode;
                p.channels = channels;
                p.merged = Some(vec![vec![0; 16]; usize::from(channels)]);
            })
            .write();
        assert!(
            matches!(read(&flat), Err(IoError::PsdPixelBudget { .. })),
            "flat, mode {mode} depth {depth}"
        );
        // A layer whose rectangle alone is past the budget.
        assert!(
            matches!(
                read(&greedy_layer_file(mode, depth, channels, 20_000)),
                Err(IoError::PsdPixelBudget { .. })
            ),
            "20,000 px layer, mode {mode} depth {depth}"
        );
        // A layer inside the budget (100 Mpx, up to 1.6 GB of f16 RGBA)
        // whose channels hold a few bytes: refused as too short before the
        // buffer its rectangle implies exists.
        assert!(
            matches!(
                read(&greedy_layer_file(mode, depth, channels, 10_000)),
                Err(IoError::PsdMalformed { what }) if what.contains("too short")
            ),
            "10,000 px layer, mode {mode} depth {depth}"
        );
    }
    // An absurd channel count is malformed, not allocated.
    let many = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4000;
        })
        .write();
    assert!(matches!(read(&many), Err(IoError::PsdMalformed { .. })));
    // A CMYK plane shorter than the layer reaches the converter only
    // through a lying plan; the converter refuses it by name.
    let converter = crate::colour_cmyk::CmykConverter::new(None);
    let short = [0_u8; 3];
    let planes = [
        Some(&short[..]),
        Some(&short[..]),
        Some(&short[..]),
        Some(&short[..]),
    ];
    assert!(matches!(
        converter.convert_planes(planes, None, 1, 4),
        Err(IoError::PsdMalformed { .. })
    ));
    // And a pixel count whose buffer cannot be sized is refused, not
    // wrapped or allocated.
    assert!(
        converter
            .convert_planes([None, None, None, None], None, 2, usize::MAX / 2)
            .is_err()
    );
}

#[test]
fn truncating_cmyk_and_float_files_never_panics() {
    let profile = synthetic_cmyk_bytes();
    let cmyk = cmyk_file(&STORED_PIXELS, Some(&profile), 3);
    let float = float_file(&FLOATS, Some(&linear_srgb_bytes()), 3);
    for bytes in [cmyk, float] {
        for len in (0..bytes.len()).step_by(3) {
            let _ = read(bytes.get(..len).unwrap_or(&[]));
        }
    }
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
}

/// Photoshop's own RGB export (a PNG beside each `adjustments/*_cmyk.psd`)
/// is the ground truth for the CMYK conversion: Aurora's converted merged
/// image must land within a fraction of an 8-bit level of it. Measured when
/// this was written (SWOP v2 profile, 8 files): mean 0.00-0.19 levels, max
/// 1-3 levels.
#[test]
fn cmyk_merged_images_match_photoshops_own_srgb_export() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/adjustments");
    if !dir.is_dir() {
        println!("SKIPPED: corpus not present at {}", dir.display());
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        return;
    }
    let mut checked = 0;
    for name in [
        "brightnesscontrast_cmyk",
        "brightnesscontrast_legacy_cmyk",
        "curves_cmyk",
        "invert_cmyk",
        "levels_cmyk",
        "posterize_cmyk",
        "threshold_cmyk",
        "posterize_16bits_cmyk",
    ] {
        let psd = std::fs::read(dir.join(format!("{name}.psd")));
        let png = std::fs::read(dir.join(format!("{name}.png")));
        let (Ok(psd), Ok(png)) = (psd, png) else {
            unreachable!("{name}: fixture missing")
        };
        let ours = match merged_image_in_working_space(&psd) {
            Ok(i) => i,
            Err(e) => unreachable!("{name}: {e:?}"),
        };
        let theirs = match crate::decode_by_extension(std::path::Path::new("x.png"), &png) {
            Ok(i) => i,
            Err(e) => unreachable!("{name}: {e:?}"),
        };
        assert_eq!(
            (ours.width(), ours.height()),
            (theirs.width(), theirs.height())
        );
        let (mut sum, mut worst, mut n) = (0.0_f64, 0.0_f64, 0.0_f64);
        for (a, b) in ours
            .samples()
            .chunks_exact(4)
            .zip(theirs.samples().chunks_exact(4))
        {
            for c in 0..3 {
                let d = (f64::from(a[c].to_f32()) * 255.0).round()
                    - (f64::from(b[c].to_f32()) * 255.0).round();
                sum += d.abs();
                worst = worst.max(d.abs());
                n += 1.0;
            }
        }
        let mean = sum / n;
        println!("{name}: mean {mean:.3} levels, max {worst}");
        assert!(
            mean < 0.5 && worst <= 4.0,
            "{name}: mean {mean}, max {worst}"
        );
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn a_readable_drop_shadow_in_a_cmyk_or_32_bit_file_is_left_out_not_drawn() {
    use super::effects_tests::{V, effects_block, shadow};
    let block = effects_block(&[(b"dropShadowMulti", V::List(vec![shadow(true, true, 3.0)]))]);
    let cmyk = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            p.layers.push(
                TestLayer::cmyk("L", 0, 0, 1, 1, 8, &[[255, 255, 255, 255, 255]])
                    .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
            );
        })
        .write();
    let float = TestPsd::new(1, 1, 1, 32)
        .with(|p| {
            p.lr16 = true;
            p.layers.push(
                TestLayer::floats("L", 0, 0, 1, 1, &[[0.5, 0.5, 0.5, 1.0]])
                    .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
            );
        })
        .write();
    for (name, bytes) in [("CMYK", cmyk), ("32-bit", float)] {
        let document = open(&bytes);
        let Some(id) = document.layers.roots().first().copied() else {
            unreachable!("a layer")
        };
        assert!(
            document.layers.effects(id).is_none(),
            "{name}: shadow drawn"
        );
        let text = document.report.items.join("\n");
        assert!(
            text.contains("layer effects of 1 layer was left out"),
            "{name}: {text}"
        );
    }
    // The same block on an 8-bit RGB layer is kept as data (the contrast).
    let rgb = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                    .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
            );
        })
        .write();
    let document = open(&rgb);
    let Some(id) = document.layers.roots().first().copied() else {
        unreachable!("a layer")
    };
    assert!(document.layers.effects(id).is_some());
}

fn solid_fill_block(colour: V<'static>) -> Vec<u8> {
    let mut out = 16_u32.to_be_bytes().to_vec();
    out.extend_from_slice(&descriptor_bytes(&[(b"Clr ", colour)]));
    out
}

#[test]
fn solid_fill_layers_without_pixels_are_drawn_in_cmyk_and_32_bit_files() {
    // CMYK: `CMYC` ink percentages go through the file's profile.
    let profile = synthetic_cmyk_bytes();
    let cmyc = V::Obj(vec![
        (b"Cyn ", V::Doub(100.0)),
        (b"Mgnt", V::Doub(25.0)),
        (b"Ylw ", V::Doub(0.0)),
        (b"Blck", V::Doub(10.0)),
    ]);
    let cmyk = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.color_mode = 4;
            p.channels = 4;
            p.resources = resource_1039(&profile);
            p.layers.push(
                TestLayer::empty("Fill")
                    .with(|l| l.blocks.push((*b"SoCo", solid_fill_block(cmyc)))),
            );
        })
        .write();
    let got = first_pixels(&open(&cmyk), 1)[0];
    let want = reference_cmyk([1.0, 0.25, 0.0, 0.1]);
    assert!(
        near(got, [want[0], want[1], want[2], 1.0], 3e-3),
        "{got:?} vs {want:?}"
    );
    // 32-bit: the descriptor's 0..255 numbers are linear light x 255. The
    // values are Photoshop's own (psd-tools `300dpi.psd`, "Shape 2"), whose
    // merged image shows (0.8404, 0.2347, 0.2347) there.
    let linear = V::Obj(vec![
        (b"Rd  ", V::Doub(172.000_004_947_185_5)),
        (b"Grn ", V::Doub(11.466_925_889_253_616)),
        (b"Bl  ", V::Doub(11.466_925_889_253_616)),
    ]);
    let float = TestPsd::new(1, 2, 2, 32)
        .with(|p| {
            p.lr16 = true;
            p.resources = resource_1039(&linear_srgb_bytes());
            p.layers.push(
                TestLayer::empty("Fill")
                    .with(|l| l.blocks.push((*b"SoCo", solid_fill_block(linear)))),
            );
        })
        .write();
    let got = first_pixels(&open(&float), 1)[0];
    assert!(near(got, [0.8404, 0.2347, 0.2347, 1.0], 2e-3), "{got:?}");
}
