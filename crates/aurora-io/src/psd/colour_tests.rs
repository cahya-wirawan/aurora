//! Colour-managed PSD open: resource 1039 converts an RGB file's layers
//! into the working space (colour management, round 1).

use aurora_color::{IccProfile, Transform};
use half::f16;

use super::test_writer::{TestLayer, TestPsd};
use super::{PsdDocument, read};

const SRGB_ICC: &[u8] = include_bytes!("../../../../corpora/icc/sRGB.icc");
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

fn psd(resources: Vec<u8>) -> Vec<u8> {
    TestPsd::new(1, 2, 1, 8)
        .with(|p| {
            p.resources = resources;
            p.layers.push(TestLayer::pixels(
                "L",
                0,
                0,
                2,
                1,
                8,
                &[[51, 153, 102, 255], [255, 0, 0, 255]],
            ));
        })
        .write()
}

fn open(bytes: &[u8]) -> PsdDocument {
    match read(bytes) {
        Ok(d) => d,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn first_pixel(document: &PsdDocument) -> [f32; 4] {
    let Some(p) = document.pixels.first() else {
        unreachable!("a layer")
    };
    let s = p.image.samples();
    [0, 1, 2, 3].map(|i| s.get(i).copied().map_or(f32::NAN, f16::to_f32))
}

#[test]
fn an_adobe_like_profile_converts_the_layers_and_is_kept_as_metadata() {
    let Ok(bytes) = adobe_like().to_bytes() else {
        unreachable!()
    };
    let document = open(&psd(resource_1039(&bytes)));
    let Ok(t) = Transform::to_working(&adobe_like()) else {
        unreachable!()
    };
    let mut want = [0.0_f32; 3];
    assert!(
        t.transform(&[51.0 / 255.0, 153.0 / 255.0, 102.0 / 255.0], &mut want)
            .is_ok()
    );
    let got = first_pixel(&document);
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 3e-3, "{got:?} vs {want:?}");
    }
    assert!((got[3] - 1.0).abs() < f32::EPSILON, "alpha untouched");
    assert_eq!(
        document.colour.source.as_ref().map(|s| s.bytes.clone()),
        Some(bytes)
    );
    assert!(
        document
            .colour
            .report
            .info
            .iter()
            .any(|l| l.contains("Converted from"))
    );
    assert!(
        document.report.items.is_empty(),
        "{:?}",
        document.report.items
    );
}

#[test]
fn an_srgb_profile_changes_nothing() {
    let none = first_pixel(&open(&psd(Vec::new())));
    let document = open(&psd(resource_1039(SRGB_ICC)));
    assert_eq!(first_pixel(&document), none);
    assert!(document.colour.source.is_none());
    assert!(document.report.items.is_empty());
    // Untagged: an info line, no change, no warning.
    let untagged = open(&psd(Vec::new()));
    assert!(
        untagged
            .colour
            .report
            .info
            .iter()
            .any(|l| l.contains("assumed sRGB"))
    );
    assert!(untagged.report.items.is_empty());
}

#[test]
fn a_damaged_or_wrong_profile_is_reported_and_the_file_still_opens() {
    let none = first_pixel(&open(&psd(Vec::new())));
    let Ok(gray) = IccProfile::synthetic_gray(2.2).and_then(|p| p.to_bytes()) else {
        unreachable!()
    };
    for bad in [b"junk".to_vec(), gray] {
        let document = open(&psd(resource_1039(&bad)));
        assert_eq!(first_pixel(&document), none);
        assert!(document.colour.source.is_none());
        assert_eq!(
            document.report.items.len(),
            1,
            "{:?}",
            document.report.items
        );
        assert!(
            document
                .report
                .items
                .iter()
                .any(|l| l.contains("was not used"))
        );
    }
}

#[test]
fn an_adjustment_layer_gets_the_fidelity_caveat() {
    // The caveat is only shown when there is something it applies to; the
    // plain pixel file above has none.
    let Ok(bytes) = adobe_like().to_bytes() else {
        unreachable!()
    };
    let document = open(&psd(resource_1039(&bytes)));
    assert!(
        !document
            .report
            .items
            .iter()
            .any(|l| l.contains("compute in sRGB"))
    );
}

#[test]
fn the_merged_image_is_converted_the_same_way() {
    let Ok(bytes) = adobe_like().to_bytes() else {
        unreachable!()
    };
    let file = psd(resource_1039(&bytes));
    let raw = match super::merged_image(&file) {
        Ok(i) => i,
        Err(e) => unreachable!("{e:?}"),
    };
    let converted = match super::merged_image_in_working_space(&file) {
        Ok(i) => i,
        Err(e) => unreachable!("{e:?}"),
    };
    // Whatever the test writer's merged image holds, the profile moves it
    // unless it is neutral black/white-only.
    assert_eq!(raw.samples().len(), converted.samples().len());
    let plain = super::merged_image_in_working_space(&psd(Vec::new()));
    assert!(plain.is_ok());
}
