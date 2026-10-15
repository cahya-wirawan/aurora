//! Grayscale adjustment layers (0.249.0): the block layouts of the ten
//! Grayscale fixtures in `psd-tools-fixtures/adjustments/`, the mapping of
//! each imported kind onto Aurora's RGB adjustments acting on a document
//! whose grey is held as R = G = B, the kinds that stay left out, hostile
//! and truncated blocks, and the corpus files (which print `SKIPPED` when
//! absent). Block bytes are Photoshop's own, copied from the fixtures.
//!
//! Mapping, from the bytes:
//!
//! - `curv`: a Grayscale file's one curve is channel 1 (bit 1 of the map,
//!   channel 1 in `Crv `) -> the same curve on red, green and blue.
//! - `levl`: record 0 (composite) stays the identity, record 1 holds the
//!   grey -> the same record on red, green and blue.
//! - `nvrt`, `thrs`, `post`, `brit`/`CgEd`: the RGB layouts, applied to the
//!   grey (R = G = B, so a luma threshold is the grey value itself).
//! - `expA` is left out: Photoshop works in the linear light of the file's
//!   grey profile (Dot Gain 20 % in the fixture), which Aurora ignores.

#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref
)]

use aurora_core::{LevelsChannel, LevelsParams};
use aurora_doc::Adjustment;

use super::PsdDocument;
use super::read;
use super::read_adjust_tests::{BRIT_LEGACY, CGED_BC1, adjustments_of, corpus, unhex};
use super::test_writer::{TestLayer, TestPsd};
use super::write_tests::ok;

const CURV_GRAY_1: &str = "00000100000002000300000000009300b800ff00ff437276200004000000010001000300000000009300b800ff00ff00";
const CURV_GRAY_4: &str = "000001000000020004000000000092005c007800b300ea00ff4372762000040000000100010004000000000092005c007800b300ea00ff00";
const CURV_GRAY_2: &str = "0000010000000043727620000400000000000000";
const LEVL_GRAY_1: &str = "0002000000ff000000ff0064002700d9001900ff0098000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff00644c766c730003003e000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff00640000";
const LEVL_GRAY_2: &str = "0002000000ff000000ff0064000000ff004600ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff00644c766c730003003e000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff00640000";

const THRS_E8: &str = "00e80000";
const POST_15: &str = "00150000";

/// A 2 x 2 Grayscale file: a grey layer under one adjustment layer
/// carrying `key`/`block`, inside a Pass Through group. The adjustment
/// record keeps only its transparency and colour-0 channels so the file
/// is clean and the report holds only what the adjustment causes.
fn gray_file(key: &[u8; 4], block: &[u8], tweak: impl FnOnce(&mut TestLayer)) -> TestPsd {
    let block = block.to_vec();
    let key = *key;
    let layer = TestLayer::empty("Adjust 1").with(|l| {
        l.channels.retain(|(id, _)| *id < 1);
        l.blocks.push((key, block));
        tweak(l);
    });
    TestPsd::new(1, 2, 2, 8).with(|p| {
        p.color_mode = 1;
        p.channels = 1;
        p.layers = vec![
            TestLayer::divider(),
            TestLayer::gray(
                "p",
                0,
                0,
                2,
                2,
                8,
                &[[10, 255], [20, 255], [30, 255], [40, 255]],
            ),
            layer,
            TestLayer::group("G", *b"pass"),
        ];
    })
}

fn open(file: &TestPsd) -> PsdDocument {
    ok(read(&file.write()))
}

fn items(document: &PsdDocument) -> String {
    document.report.items.join("\n")
}

fn only_adjustment(document: &PsdDocument) -> Option<Adjustment> {
    let all = adjustments_of(document);
    (all.len() == 1).then(|| all[0].1.clone())
}

fn left_out_line(label: &str) -> String {
    format!("1 {label} layer in this Grayscale file was left out")
}

fn points(curve: &aurora_core::ToneCurve) -> Vec<(f32, f32)> {
    curve.points().iter().map(|p| (p.x, p.y)).collect()
}

fn curves_of(document: &PsdDocument) -> Option<aurora_core::CurvesParams> {
    match only_adjustment(document)? {
        Adjustment::Curves(c) => Some(c),
        _ => None,
    }
}

fn levels_of(document: &PsdDocument) -> Option<LevelsParams> {
    match only_adjustment(document)? {
        Adjustment::Levels(l) => Some(l),
        _ => None,
    }
}

/// Photoshop's `(output, input)` pairs of `curves_grayscale.psd`'s
/// "Curves 1" (channel 1) become the points `(input / 255, output / 255)`.
fn curve_1_points() -> Vec<(f32, f32)> {
    vec![
        (0.0, 0.0),
        (f32::from(0xb8_u8) / 255.0, f32::from(0x93_u8) / 255.0),
        (1.0, 1.0),
    ]
}

// ---------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------

#[test]
fn a_real_grayscale_curve_is_the_same_curve_on_red_green_and_blue() {
    let document = open(&gray_file(b"curv", &unhex(CURV_GRAY_1), |_| {}));
    let Some(curves) = curves_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    for (name, slot) in [
        ("red", &curves.red),
        ("green", &curves.green),
        ("blue", &curves.blue),
    ] {
        let Some(curve) = slot else {
            unreachable!("{name} carries the grey curve")
        };
        assert_eq!(points(curve), curve_1_points(), "{name}");
    }
    assert!(
        curves.composite.is_identity(),
        "no composite curve in a grey file"
    );
    assert!(
        !items(&document).contains("left out"),
        "{}",
        items(&document)
    );
}

#[test]
fn a_four_point_grayscale_curve_keeps_all_its_points() {
    let document = open(&gray_file(b"curv", &unhex(CURV_GRAY_4), |_| {}));
    let Some(curves) = curves_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    let Some(red) = &curves.red else {
        unreachable!("red")
    };
    assert_eq!(red.points().len(), 4);
    assert_eq!(curves.red, curves.green);
    assert_eq!(curves.green, curves.blue);
}

#[test]
fn a_grayscale_curves_layer_with_no_curve_is_the_identity() {
    let document = open(&gray_file(b"curv", &unhex(CURV_GRAY_2), |_| {}));
    let Some(curves) = curves_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    assert!(curves.is_identity());
}

/// `curv(version, count_map, legacy, extra)` of `tests.rs`, rebuilt here
/// for a channel list: a map flag 0, version 1, the channel bitmap, the
/// curves, and the `Crv ` extra data.
fn curv_channels(channels: &[(u16, &[(u16, u16)])]) -> Vec<u8> {
    let mut map = 0_u32;
    for (c, _) in channels {
        map |= 1 << c;
    }
    let points = |out: &mut Vec<u8>, pts: &[(u16, u16)]| {
        out.extend_from_slice(&(pts.len() as u16).to_be_bytes());
        for (o, i) in pts {
            out.extend_from_slice(&o.to_be_bytes());
            out.extend_from_slice(&i.to_be_bytes());
        }
    };
    let mut out = vec![0, 0, 1];
    out.extend_from_slice(&map.to_be_bytes());
    for (_, pts) in channels {
        points(&mut out, pts);
    }
    out.extend_from_slice(b"Crv ");
    out.extend_from_slice(&4_u16.to_be_bytes());
    out.extend_from_slice(&(channels.len() as u32).to_be_bytes());
    for (c, pts) in channels {
        out.extend_from_slice(&c.to_be_bytes());
        points(&mut out, pts);
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

#[test]
fn a_grayscale_curve_on_channel_zero_is_left_out() {
    let block = curv_channels(&[(0, &[(0, 0), (147, 184), (255, 255)])]);
    let document = open(&gray_file(b"curv", &block, |_| {}));
    assert!(adjustments_of(&document).is_empty());
    assert!(
        items(&document).contains(&left_out_line("Curves")),
        "{}",
        items(&document)
    );
}

#[test]
fn channels_two_and_three_do_not_reach_a_grayscale_curve() {
    let block = curv_channels(&[
        (1, &[(0, 0), (147, 184), (255, 255)]),
        (2, &[(0, 50), (255, 255)]),
        (3, &[(10, 0), (255, 200)]),
    ]);
    let document = open(&gray_file(b"curv", &block, |_| {}));
    let Some(curves) = curves_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    for slot in [&curves.red, &curves.green, &curves.blue] {
        let Some(curve) = slot else {
            unreachable!("grey curve")
        };
        assert_eq!(points(curve), curve_1_points());
    }
}

#[test]
fn an_rgb_file_still_maps_channels_one_to_three_to_red_green_blue() {
    // The same bytes in an RGB file: channel 1 is red alone.
    let block = unhex(CURV_GRAY_1);
    let file = super::read_adjust_tests::file_with(b"curv", &block, |_| {});
    let document = open(&file);
    let Some(curves) = curves_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    assert!(curves.red.is_some());
    assert!(curves.green.is_none() && curves.blue.is_none());
}

// ---------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------

fn level_channel(ib: u16, iw: u16, gamma: u16, ob: u16, ow: u16) -> LevelsChannel {
    match LevelsChannel::new(
        f32::from(ib) / 255.0,
        f32::from(iw) / 255.0,
        f32::from(gamma) / 100.0,
        f32::from(ob) / 255.0,
        f32::from(ow) / 255.0,
    ) {
        Ok(c) => c,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// Overwrites record `index` of a `levl` block (version 2 first).
fn set_record(block: &mut [u8], index: usize, record: [u16; 5]) {
    for (k, v) in record.iter().enumerate() {
        let at = 2 + index * 10 + k * 2;
        block[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }
}

#[test]
fn a_real_grayscale_levels_record_one_drives_red_green_and_blue() {
    // levels_grayscale.psd "Levels 1": record 0 the identity, record 1
    // input 39..217, output 25..255, gamma 1.52.
    let document = open(&gray_file(b"levl", &unhex(LEVL_GRAY_1), |_| {}));
    let want = level_channel(0x27, 0xd9, 0x98, 0x19, 0xff);
    assert_eq!(
        levels_of(&document),
        Some(LevelsParams {
            master: LevelsChannel::identity(),
            red: want,
            green: want,
            blue: want,
        }),
        "{}",
        items(&document)
    );
}

#[test]
fn a_real_grayscale_levels_output_floor_only_layer() {
    let document = open(&gray_file(b"levl", &unhex(LEVL_GRAY_2), |_| {}));
    let want = level_channel(0, 255, 100, 0x46, 0xff);
    let Some(levels) = levels_of(&document) else {
        unreachable!("imported: {}", items(&document))
    };
    assert_eq!((levels.red, levels.green, levels.blue), (want, want, want));
    assert!(levels.master.is_identity());
}

#[test]
fn a_non_identity_composite_record_in_a_grayscale_file_is_left_out() {
    let mut block = unhex(LEVL_GRAY_1);
    set_record(&mut block, 0, [10, 240, 0, 255, 100]);
    let document = open(&gray_file(b"levl", &block, |_| {}));
    assert!(adjustments_of(&document).is_empty());
    assert!(
        items(&document).contains(&left_out_line("Levels")),
        "{}",
        items(&document)
    );
}

#[test]
fn records_two_and_three_are_not_read_in_a_grayscale_file() {
    // A reversed range in the green and blue records would make an RGB
    // layer unreadable; a grey file has no such channels.
    let mut block = unhex(LEVL_GRAY_1);
    set_record(&mut block, 2, [200, 100, 0, 255, 100]);
    set_record(&mut block, 3, [0, 255, 0, 255, 0]);
    let document = open(&gray_file(b"levl", &block, |_| {}));
    assert!(levels_of(&document).is_some(), "{}", items(&document));
    // The same block in an RGB file is unreadable.
    let rgb = open(&super::read_adjust_tests::file_with(
        b"levl",
        &block,
        |_| {},
    ));
    assert!(adjustments_of(&rgb).is_empty());
}

#[test]
fn an_unreadable_grayscale_levels_record_one_is_reported_as_unreadable() {
    let mut block = unhex(LEVL_GRAY_1);
    set_record(&mut block, 1, [200, 100, 0, 255, 100]);
    let document = open(&gray_file(b"levl", &block, |_| {}));
    assert!(adjustments_of(&document).is_empty());
    assert!(
        items(&document).contains("could not be read"),
        "{}",
        items(&document)
    );
}

// ---------------------------------------------------------------------
// Invert, Threshold, Posterize, Brightness/Contrast, and the kinds left out
// ---------------------------------------------------------------------

#[test]
fn invert_threshold_and_posterize_import_in_a_grayscale_file() {
    let threshold = match aurora_core::ThresholdParams::new(0xe8) {
        Ok(p) => Adjustment::Threshold(p),
        Err(err) => unreachable!("{err:?}"),
    };
    let posterize = match aurora_core::PosterizeParams::new(0x15) {
        Ok(p) => Adjustment::Posterize(p),
        Err(err) => unreachable!("{err:?}"),
    };
    for (key, block, want) in [
        (b"nvrt", Vec::new(), Adjustment::Invert),
        (b"thrs", unhex(THRS_E8), threshold),
        (b"post", unhex(POST_15), posterize),
    ] {
        let document = open(&gray_file(key, &block, |_| {}));
        assert_eq!(
            only_adjustment(&document),
            Some(want),
            "{key:?}: {}",
            items(&document)
        );
        assert!(!items(&document).contains("left out"), "{key:?}");
    }
}

#[test]
fn the_default_brightness_contrast_imports_and_the_legacy_one_stays_out() {
    let document = open(&gray_file(b"brit", &[0; 8], |l| {
        l.blocks.push((*b"CgEd", unhex(CGED_BC1)));
    }));
    let want = match aurora_core::BrightnessContrastParams::new(-95, -50, false) {
        Ok(p) => Adjustment::BrightnessContrast(p),
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(
        only_adjustment(&document),
        Some(want),
        "{}",
        items(&document)
    );

    let legacy = open(&gray_file(b"brit", &unhex(BRIT_LEGACY), |_| {}));
    assert!(adjustments_of(&legacy).is_empty());
    assert!(
        items(&legacy).contains(&left_out_line("Brightness/Contrast")),
        "{}",
        items(&legacy)
    );
}

#[test]
fn exposure_stays_out_of_a_grayscale_file_because_the_grey_profile_decides_it() {
    let exposure = unhex(super::read_adjust_tests::EXPA_FILL);
    let document = open(&gray_file(b"expA", &exposure, |_| {}));
    assert!(adjustments_of(&document).is_empty());
    assert!(
        items(&document).contains(&left_out_line("Exposure")),
        "{}",
        items(&document)
    );
}

#[test]
fn kinds_without_a_grayscale_fixture_stay_out() {
    use super::read_adjust_tests::{BLNC_FILL, BLWH, PHFL, VIBA_FILL};
    for (key, block, label) in [
        (b"vibA", unhex(VIBA_FILL), "Vibrance"),
        (b"phfl", unhex(PHFL), "Photo Filter"),
        (b"blwh", unhex(BLWH), "Black & White"),
        (b"blnc", unhex(BLNC_FILL), "Color Balance"),
    ] {
        let document = open(&gray_file(key, &block, |_| {}));
        assert!(adjustments_of(&document).is_empty(), "{label}");
        assert!(
            items(&document).contains(&left_out_line(label)),
            "{label}: {}",
            items(&document)
        );
    }
}

// ---------------------------------------------------------------------
// The colour mode note
// ---------------------------------------------------------------------

#[test]
fn a_grayscale_file_says_it_is_held_as_rgb_and_an_rgb_file_does_not() {
    let gray = open(&gray_file(b"nvrt", &[], |_| {}));
    assert!(
        items(&gray).contains("Grayscale file is held as RGB")
            && items(&gray).contains("writes an RGB file"),
        "{}",
        items(&gray)
    );
    let rgb = open(&super::read_adjust_tests::file_with(b"nvrt", &[], |_| {}));
    assert!(!items(&rgb).contains("Grayscale"), "{}", items(&rgb));
}

// ---------------------------------------------------------------------
// Hostile and truncated blocks
// ---------------------------------------------------------------------

#[test]
fn every_truncation_of_the_real_grayscale_blocks_is_safe() {
    let blocks: [(&[u8; 4], Vec<u8>); 7] = [
        (b"curv", unhex(CURV_GRAY_1)),
        (b"curv", unhex(CURV_GRAY_4)),
        (b"levl", unhex(LEVL_GRAY_1)),
        (b"thrs", unhex(THRS_E8)),
        (b"post", unhex(POST_15)),
        (b"brit", vec![0; 8]),
        (b"nvrt", Vec::new()),
    ];
    for (key, block) in blocks {
        for len in 0..=block.len() {
            let truncated = block.get(..len).unwrap_or_default();
            let document = open(&gray_file(key, truncated, |l| {
                if key == b"brit" {
                    let cged = unhex(CGED_BC1);
                    l.blocks.push((
                        *b"CgEd",
                        cged.get(..len.min(cged.len())).unwrap_or_default().to_vec(),
                    ));
                }
            }));
            let count = adjustments_of(&document).len();
            assert!(count <= 1, "{key:?} at {len}");
            // A Levels block shorter than its 29 records is unreadable.
            if key == b"levl" && len < 2 + 29 * 10 {
                assert_eq!(count, 0, "levl truncated to {len}");
                assert!(items(&document).contains("could not be read"), "levl {len}");
            }
        }
    }
}

#[test]
fn a_hostile_curve_count_in_a_grayscale_file_is_bounded() {
    // 0xffffffff channels in the bitmap, no data behind it.
    let mut block = vec![0, 0, 1];
    block.extend_from_slice(&u32::MAX.to_be_bytes());
    block.extend_from_slice(&[0xff; 64]);
    let document = open(&gray_file(b"curv", &block, |_| {}));
    assert!(adjustments_of(&document).is_empty());
}

// ---------------------------------------------------------------------
// The corpus (SKIPPED when absent)
// ---------------------------------------------------------------------

#[test]
fn the_corpus_grayscale_files_import_their_four_layers() {
    for (file, kind, n) in [
        ("curves_grayscale.psd", "Curves", 4),
        ("levels_grayscale.psd", "Levels", 4),
        ("invert_grayscale.psd", "Invert", 1),
        ("threshold_grayscale.psd", "Threshold", 4),
        ("threshold_16bits_grayscale.psd", "Threshold", 4),
        ("posterize_grayscale.psd", "Posterize", 4),
        ("posterize_16bits_grayscale.psd", "Posterize", 4),
        ("brightnesscontrast_grayscale.psd", "Brightness/Contrast", 4),
    ] {
        let Some(document) = corpus(&format!("adjustments/{file}")) else {
            continue;
        };
        let all = adjustments_of(&document);
        assert_eq!(all.len(), n, "{file}");
        assert!(all.iter().all(|(_, a)| a.label() == kind), "{file}");
        assert!(
            !items(&document).contains("left out"),
            "{file}: {}",
            items(&document)
        );
    }
}

#[test]
fn the_corpus_exposure_and_legacy_grayscale_files_stay_out() {
    for (file, kind, n) in [
        ("exposure_grayscale.psd", "Exposure", 4),
        (
            "brightnesscontrast_legacy_grayscale.psd",
            "Brightness/Contrast",
            1,
        ),
    ] {
        let Some(document) = corpus(&format!("adjustments/{file}")) else {
            continue;
        };
        assert!(adjustments_of(&document).is_empty(), "{file}");
        let want = format!("{n} {kind} layer");
        assert!(
            items(&document).contains(&want) && items(&document).contains("Grayscale file"),
            "{file}: {}",
            items(&document)
        );
    }
}

#[test]
fn the_corpus_grayscale_curve_points_match_the_bytes() {
    let Some(document) = corpus("adjustments/curves_grayscale.psd") else {
        return;
    };
    let all = adjustments_of(&document);
    let Some((_, Adjustment::Curves(first))) = all.iter().find(|(n, _)| n == "Curves 1") else {
        unreachable!("Curves 1")
    };
    let Some(red) = &first.red else {
        unreachable!("red")
    };
    assert_eq!(points(red), curve_1_points());
}
