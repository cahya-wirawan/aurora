//! Tests for the PSD reader's Vibrance, Exposure, Photo Filter, Black &
//! White and Color Balance layers (0.241.0): Photoshop's own blocks (hex
//! taken from the psd-tools corpus), synthetic blocks through the real
//! reader, hostile and truncated blocks, the Lab conversion, and the
//! corpus files themselves (which print `SKIPPED` when absent).

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::trivially_copy_pass_by_ref,
    clippy::bool_assert_comparison,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::type_complexity
)]

use aurora_doc::{Adjustment, BlendMode};

use super::read_adjust::{
    self, ColorBalanceBlock, ExposureBlock, Parsed, PhotoColour, VibranceBlock,
    tint_hue_saturation, tint_rgb,
};
use super::test_writer::{TestLayer, TestPsd};
use super::{Note, PsdDocument, read};
use crate::error::IoError;

// Photoshop's blocks, byte for byte, from the psd-tools corpus (the file
// each came from is named).
/// `fill_adjustments.psd` "Vibrance 1": vibrance -6, Strt 2.
pub(super) const VIBA_FILL: &str = "00000010000000010000000000006e756c6c000000020000000876696272616e63656c6f6e67fffffffa00000000537472746c6f6e67000000020000";
/// `layers/vibrance.psd`: an empty descriptor (both sliders at 0).
pub(super) const VIBA_NEUTRAL: &str = "00000010000000010000000000006e756c6c000000000000";
/// `fill_adjustments.psd` "Exposure 1": -0.3889, 0.0173, 0.9097.
pub(super) const EXPA_FILL: &str = "0001bec7ae143c89a0273f68f5c30000";
/// `layers/exposure.psd`: 0, 0, 1.
pub(super) const EXPA_NEUTRAL: &str = "000100000000000000003f8000000000";
/// `adjustments/exposure_rgb.psd` "Exposure 1".."Exposure 4".
pub(super) const EXPA_RGB: [&str; 4] = [
    "00014001eb853d9eb8523fc28f5c0000",
    "0001bd23d70a3ed42c3d3ee147ae0000",
    "00013fa00000000000003fc28f5c0000",
    "0001bfe8f5c33df65fd93f8000000000",
];
/// `layers/photo-filter.psd` (also in `fill_adjustments.psd`): Lab
/// (67.06, 32.00, 120.00), density 25, preserve luminosity.
pub(super) const PHFL: &str = "000200071a320c802ee000000000001901000000";
/// `fill_adjustments.psd` "Color Balance 1".
pub(super) const BLNC_FILL: &str = "fffc0002fffb000a0004fff70001fff7fffd0100";
/// `clipping-mask2.psd` "Color Balance 1": only the midtones, no preserve.
pub(super) const BLNC_CLIP: &str = "000000000000001c006400350000000000000100";
/// `layers/black-and-white.psd`.
pub(super) const BLWH: &str = concat!(
    "00000010000000010000000000006e756c6c0000000a00000000526420206c6f6e670000002800000000",
    "596c6c776c6f6e670000003c0000000047726e206c6f6e67000000280000000043796e206c6f6e670000",
    "003c00000000426c20206c6f6e6700000014000000004d676e746c6f6e67000000500000000775736554",
    "696e74626f6f6c000000000974696e74436f6c6f724f626a630000000100000000000052474243000000",
    "030000000052642020646f7562406c2003c00000000000000047726e20646f7562406a60058000000000",
    "000000426c2020646f756240666009800000000000000c62775072657365744b696e646c6f6e67000000",
    "010000001b626c61636b416e64576869746550726573657446696c654e616d6554455854000000010000",
    "0000"
);

pub(super) fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2).unwrap_or("0"), 16).unwrap_or(0))
        .collect()
}

fn ok<T>(result: Result<T, IoError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("expected Ok, got {err:?}"),
    }
}

fn parse_block(key: &[u8; 4], data: &[u8]) -> Option<Parsed> {
    read_adjust::parse_block(*key, data)
}

fn parsed(key: &[u8; 4], hex: &str) -> Parsed {
    let Some(parsed) = parse_block(key, &unhex(hex)) else {
        unreachable!("a known key")
    };
    parsed
}

fn adjustment_of(parsed: &Parsed) -> Result<Adjustment, Note> {
    read_adjust::adjustment(parsed, false)
}

// ---------------------------------------------------------------------
// Descriptor builders for synthetic blocks (independent of the writer)
// ---------------------------------------------------------------------

fn key(out: &mut Vec<u8>, k: &[u8]) {
    let n = if k.len() == 4 { 0 } else { k.len() as u32 };
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(k);
}

/// A `vibA`/`blwh`-style block: version 16, name, class, items.
fn descriptor(items: &[(&[u8], &[u8], Vec<u8>)]) -> Vec<u8> {
    let mut out = 16_u32.to_be_bytes().to_vec();
    out.extend_from_slice(&[0, 0, 0, 1, 0, 0]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"null");
    out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    for (k, ty, payload) in items {
        key(&mut out, k);
        out.extend_from_slice(ty);
        out.extend_from_slice(payload);
    }
    out
}

fn long(v: i32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

fn vib(vibrance: Option<i32>, strt: Option<i32>) -> Vec<u8> {
    let mut items: Vec<(&[u8], &[u8], Vec<u8>)> = Vec::new();
    if let Some(v) = vibrance {
        items.push((b"vibrance", b"long", long(v)));
    }
    if let Some(v) = strt {
        items.push((b"Strt", b"long", long(v)));
    }
    descriptor(&items)
}

fn bw(weights: [i32; 6], tint: Option<bool>, rgb: Option<[f64; 3]>) -> Vec<u8> {
    let mut items: Vec<(&[u8], &[u8], Vec<u8>)> = Vec::new();
    for (k, w) in [&b"Rd  "[..], b"Yllw", b"Grn ", b"Cyn ", b"Bl  ", b"Mgnt"]
        .into_iter()
        .zip(weights)
    {
        items.push((k, b"long", long(w)));
    }
    if let Some(t) = tint {
        items.push((b"useTint", b"bool", vec![u8::from(t)]));
    }
    if let Some(c) = rgb {
        let mut body = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        body.extend_from_slice(b"RGBC");
        body.extend_from_slice(&3_u32.to_be_bytes());
        for (k, v) in [&b"Rd  "[..], b"Grn ", b"Bl  "].into_iter().zip(c) {
            key(&mut body, k);
            body.extend_from_slice(b"doub");
            body.extend_from_slice(&v.to_be_bytes());
        }
        items.push((b"tintColor", b"Objc", body));
    }
    descriptor(&items)
}

fn exp(version: u16, e: f32, o: f32, g: f32) -> Vec<u8> {
    let mut out = version.to_be_bytes().to_vec();
    for v in [e, o, g] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&[0, 0]);
    out
}

fn phfl(version: u16, space: u16, comps: [u16; 4], density: u32, lum: u8) -> Vec<u8> {
    let mut out = version.to_be_bytes().to_vec();
    if version == 3 {
        out.extend_from_slice(&[0; 12]);
    } else {
        out.extend_from_slice(&space.to_be_bytes());
        for c in comps {
            out.extend_from_slice(&c.to_be_bytes());
        }
    }
    out.extend_from_slice(&density.to_be_bytes());
    out.push(lum);
    out
}

fn blnc(values: [i16; 9], lum: u8) -> Vec<u8> {
    let mut out: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
    out.extend_from_slice(&[lum, 0]);
    out
}

// ---------------------------------------------------------------------
// Real blocks
// ---------------------------------------------------------------------

#[test]
fn photoshops_own_vibrance_blocks_parse_to_their_sliders() {
    assert_eq!(
        parsed(b"vibA", VIBA_FILL),
        Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: -6.0,
            saturation: 2.0
        }))
    );
    // An empty descriptor is both sliders at their default, 0.
    assert_eq!(
        parsed(b"vibA", VIBA_NEUTRAL),
        Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: 0.0,
            saturation: 0.0
        }))
    );
    let Ok(Adjustment::Vibrance(p)) = adjustment_of(&parsed(b"vibA", VIBA_FILL)) else {
        unreachable!("vibrance")
    };
    assert_eq!((p.vibrance(), p.saturation()), (-6.0, 2.0));
}

#[test]
fn photoshops_own_exposure_blocks_parse_to_their_floats() {
    let want = |hex: &str, e: u32, o: u32, g: u32| {
        assert_eq!(
            parsed(b"expA", hex),
            Parsed::Exposure(Ok(ExposureBlock {
                exposure: f32::from_bits(e),
                offset: f32::from_bits(o),
                gamma: f32::from_bits(g),
            })),
            "{hex}"
        );
    };
    want(EXPA_NEUTRAL, 0, 0, 0x3f80_0000);
    want(EXPA_FILL, 0xbec7_ae14, 0x3c89_a027, 0x3f68_f5c3);
    want(EXPA_RGB[0], 0x4001_eb85, 0x3d9e_b852, 0x3fc2_8f5c);
    want(EXPA_RGB[1], 0xbd23_d70a, 0x3ed4_2c3d, 0x3ee1_47ae);
    want(EXPA_RGB[2], 0x3fa0_0000, 0, 0x3fc2_8f5c);
    want(EXPA_RGB[3], 0xbfe8_f5c3, 0x3df6_5fd9, 0x3f80_0000);
    // All of them are inside Aurora's ranges, so all import unchanged.
    for hex in EXPA_RGB {
        let Ok(Adjustment::Exposure(p)) = adjustment_of(&parsed(b"expA", hex)) else {
            unreachable!("{hex}")
        };
        let Parsed::Exposure(Ok(b)) = parsed(b"expA", hex) else {
            unreachable!()
        };
        assert_eq!(
            (p.exposure(), p.offset(), p.gamma()),
            (b.exposure, b.offset, b.gamma)
        );
    }
}

#[test]
fn photoshops_own_photo_filter_block_is_the_warming_preset() {
    let Parsed::PhotoFilter(Ok(b)) = parsed(b"phfl", PHFL) else {
        unreachable!("phfl")
    };
    assert_eq!(b.colour, PhotoColour::Lab(67.06, 32.0, 120.0));
    assert_eq!((b.density, b.preserve_luminosity), (25, true));
    let Ok(Adjustment::PhotoFilter(p)) = adjustment_of(&Parsed::PhotoFilter(Ok(b))) else {
        unreachable!("photo filter")
    };
    // Lab (67.06, 32, 120) D50, clipped to sRGB, is #EC8A00 within half a
    // level: Photoshop's "Warming Filter (85)" preset.
    let [r, g, bl] = p.colour();
    assert!((r * 255.0 - 236.0).abs() < 0.5, "{r}");
    assert!((g * 255.0 - 138.0).abs() < 0.5, "{g}");
    assert_eq!(bl, 0.0, "the out-of-gamut blue is clipped");
    assert_eq!((p.density(), p.preserve_luminosity()), (25.0, true));
}

#[test]
fn photoshops_own_color_balance_blocks_parse_to_their_sliders() {
    assert_eq!(
        parsed(b"blnc", BLNC_FILL),
        Parsed::ColorBalance(Ok(ColorBalanceBlock {
            ranges: [[-4, 2, -5], [10, 4, -9], [1, -9, -3]],
            preserve_luminosity: true
        }))
    );
    assert_eq!(
        parsed(b"blnc", BLNC_CLIP),
        Parsed::ColorBalance(Ok(ColorBalanceBlock {
            ranges: [[0, 0, 0], [28, 100, 53], [0, 0, 0]],
            preserve_luminosity: true
        }))
    );
    let Ok(Adjustment::ColorBalance(p)) = adjustment_of(&parsed(b"blnc", BLNC_FILL)) else {
        unreachable!("color balance")
    };
    assert_eq!(
        p.ranges(),
        [[-4.0, 2.0, -5.0], [10.0, 4.0, -9.0], [1.0, -9.0, -3.0]]
    );
    assert!(p.preserve_luminosity());
}

#[test]
fn photoshops_own_black_and_white_block_has_the_default_sliders_and_tint_off() {
    let Parsed::BlackWhite(Ok(b)) = parsed(b"blwh", BLWH) else {
        unreachable!("blwh")
    };
    assert_eq!(b.weights, [40.0, 60.0, 40.0, 60.0, 20.0, 80.0]);
    assert!(!b.tint);
    assert!(
        (b.tint_rgb[0] - 225.000_457_763_671_88).abs() < 1e-9,
        "{:?}",
        b.tint_rgb
    );
    assert!((b.tint_rgb[1] - 211.000_671_386_718_75).abs() < 1e-9);
    assert!((b.tint_rgb[2] - 179.001_159_667_968_75).abs() < 1e-9);
    let Ok(Adjustment::BlackWhite(p)) = adjustment_of(&Parsed::BlackWhite(Ok(b))) else {
        unreachable!("black and white")
    };
    assert_eq!(p.weights(), aurora_core::BLACK_WHITE_DEFAULTS);
    assert!(!p.tint());
    // Photoshop's default tint colour (225, 211, 179) is HSL hue 41.7,
    // saturation 43.4 %.
    assert!((p.tint_hue() - 41.739_13).abs() < 0.01, "{}", p.tint_hue());
    assert!(
        (p.tint_saturation() - 43.396_23).abs() < 0.01,
        "{}",
        p.tint_saturation()
    );
}

// ---------------------------------------------------------------------
// Synthetic blocks
// ---------------------------------------------------------------------

#[test]
fn absent_descriptor_keys_take_photoshops_defaults() {
    assert_eq!(
        parse_block(b"vibA", &vib(Some(30), None)),
        Some(Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: 30.0,
            saturation: 0.0
        })))
    );
    assert_eq!(
        parse_block(b"vibA", &vib(None, Some(-40))),
        Some(Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: 0.0,
            saturation: -40.0
        })))
    );
    // No weights written at all: the defaults, tint off, default colour.
    let Some(Parsed::BlackWhite(Ok(b))) = parse_block(b"blwh", &descriptor(&[])) else {
        unreachable!("blwh")
    };
    assert_eq!(b.weights, [40.0, 60.0, 40.0, 60.0, 20.0, 80.0]);
    assert!(!b.tint);
    assert_eq!(b.tint_rgb, [225.0, 211.0, 179.0]);
}

#[test]
fn ranges_are_exactly_aurora_s_and_outside_them_is_left_out() {
    let edge = |p: Option<Parsed>| adjustment_of(&p.unwrap_or_else(|| unreachable!()));
    // Vibrance -100..100.
    assert!(edge(parse_block(b"vibA", &vib(Some(100), Some(-100)))).is_ok());
    assert_eq!(
        edge(parse_block(b"vibA", &vib(Some(101), None))),
        Err(Note::Unreadable(read_adjust::Kind::Vibrance))
    );
    assert_eq!(
        edge(parse_block(b"vibA", &vib(None, Some(-101)))),
        Err(Note::Unreadable(read_adjust::Kind::Vibrance))
    );
    // Exposure: gamma 0.10..9.99 (Photoshop allows down to 0.01).
    assert!(edge(parse_block(b"expA", &exp(1, 20.0, 0.5, 9.99))).is_ok());
    assert!(edge(parse_block(b"expA", &exp(1, -20.0, -0.5, 0.1))).is_ok());
    for (e, o, g) in [
        (20.5, 0.0, 1.0),
        (0.0, 0.6, 1.0),
        (0.0, 0.0, 0.05),
        (0.0, 0.0, 10.5),
    ] {
        assert_eq!(
            edge(parse_block(b"expA", &exp(1, e, o, g))),
            Err(Note::Unreadable(read_adjust::Kind::Exposure)),
            "{e} {o} {g}"
        );
    }
    assert_eq!(
        edge(parse_block(b"expA", &exp(1, f32::NAN, 0.0, 1.0))),
        Err(Note::Unreadable(read_adjust::Kind::Exposure))
    );
    // Black & White: -200..300.
    assert!(
        edge(parse_block(
            b"blwh",
            &bw([-200, 300, 0, 0, 0, 0], None, None)
        ))
        .is_ok()
    );
    assert_eq!(
        edge(parse_block(b"blwh", &bw([301, 0, 0, 0, 0, 0], None, None))),
        Err(Note::Unreadable(read_adjust::Kind::BlackWhite))
    );
    assert_eq!(
        edge(parse_block(b"blwh", &bw([0, 0, 0, 0, -201, 0], None, None))),
        Err(Note::Unreadable(read_adjust::Kind::BlackWhite))
    );
    // A tint colour channel far outside 0..255 is damage.
    assert_eq!(
        edge(parse_block(
            b"blwh",
            &bw(
                [40, 60, 40, 60, 20, 80],
                Some(true),
                Some([300.0, 0.0, 0.0])
            )
        )),
        Err(Note::Unreadable(read_adjust::Kind::BlackWhite))
    );
    // Color Balance -100..100.
    assert!(
        edge(parse_block(
            b"blnc",
            &blnc([100, -100, 0, 0, 0, 0, 0, 0, 0], 1)
        ))
        .is_ok()
    );
    assert_eq!(
        edge(parse_block(
            b"blnc",
            &blnc([0, 0, 0, 0, 0, 0, 0, 0, 101], 1)
        )),
        Err(Note::Unreadable(read_adjust::Kind::ColorBalance))
    );
    // Photo Filter density 1..100.
    let lab = [5000, 0, 0, 0];
    assert!(edge(parse_block(b"phfl", &phfl(2, 7, lab, 100, 0))).is_ok());
    for density in [0_u32, 101, 4_000_000_000] {
        assert_eq!(
            edge(parse_block(b"phfl", &phfl(2, 7, lab, density, 0))),
            Err(Note::Unreadable(read_adjust::Kind::PhotoFilter)),
            "density {density}"
        );
    }
}

#[test]
fn a_photo_filter_that_is_not_lab_version_2_is_left_out_with_its_own_reason() {
    for bytes in [
        phfl(3, 0, [0; 4], 25, 1),
        phfl(2, 0, [65535, 0, 0, 0], 25, 1),
        phfl(2, 2, [0; 4], 25, 1),
    ] {
        let p = parse_block(b"phfl", &bytes).unwrap_or_else(|| unreachable!());
        assert_eq!(adjustment_of(&p), Err(Note::PhotoFilterColour), "{bytes:?}");
    }
    // An unknown version is just unreadable.
    let p = parse_block(b"phfl", &phfl(4, 7, [0; 4], 25, 1)).unwrap_or_else(|| unreachable!());
    assert_eq!(p, Parsed::PhotoFilter(Err(())));
}

#[test]
fn wrong_versions_and_wrong_kinds_of_item_are_unreadable() {
    assert_eq!(
        parse_block(b"expA", &exp(0, 0.0, 0.0, 1.0)),
        Some(Parsed::Exposure(Err(())))
    );
    assert_eq!(
        parse_block(b"expA", &exp(2, 0.0, 0.0, 1.0)),
        Some(Parsed::Exposure(Err(())))
    );
    // A descriptor version other than 16.
    let mut bad = vib(Some(1), None);
    bad[3] = 15;
    assert_eq!(parse_block(b"vibA", &bad), Some(Parsed::Vibrance(Err(()))));
    // A slider that is text, a tint flag that is a number, a colour that is
    // not an object.
    let text = descriptor(&[(b"vibrance", b"TEXT", vec![0, 0, 0, 1, 0, 0])]);
    assert_eq!(parse_block(b"vibA", &text), Some(Parsed::Vibrance(Err(()))));
    let flag = descriptor(&[(b"useTint", b"long", long(1))]);
    assert_eq!(
        parse_block(b"blwh", &flag),
        Some(Parsed::BlackWhite(Err(())))
    );
    let colour = descriptor(&[(b"tintColor", b"long", long(1))]);
    assert_eq!(
        parse_block(b"blwh", &colour),
        Some(Parsed::BlackWhite(Err(())))
    );
    // Doubles are accepted for the sliders; a NaN is not.
    let d = descriptor(&[(b"vibrance", b"doub", 12.5_f64.to_be_bytes().to_vec())]);
    assert_eq!(
        parse_block(b"vibA", &d),
        Some(Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: 12.5,
            saturation: 0.0
        })))
    );
    let nan = descriptor(&[(b"vibrance", b"doub", f64::NAN.to_be_bytes().to_vec())]);
    assert_eq!(parse_block(b"vibA", &nan), Some(Parsed::Vibrance(Err(()))));
    // Unknown keys are ignored.
    let extra = descriptor(&[(b"Atlm", b"bool", vec![1]), (b"vibrance", b"long", long(7))]);
    assert_eq!(
        parse_block(b"vibA", &extra),
        Some(Parsed::Vibrance(Ok(VibranceBlock {
            vibrance: 7.0,
            saturation: 0.0
        })))
    );
}

#[test]
fn the_last_block_of_a_record_wins() {
    // Exercised through the real reader below; here the parse of each is
    // independent of order.
    assert_ne!(parsed(b"vibA", VIBA_FILL), parsed(b"vibA", VIBA_NEUTRAL));
}

// ---------------------------------------------------------------------
// Hostile and truncated blocks
// ---------------------------------------------------------------------

/// Every prefix of every real block, and every single-byte mutation of
/// it, parses to something (Ok or Err) without panicking.
#[test]
fn truncated_and_mutated_blocks_never_panic() {
    let blocks: [(&[u8; 4], Vec<u8>); 8] = [
        (b"vibA", unhex(VIBA_FILL)),
        (b"expA", unhex(EXPA_FILL)),
        (b"phfl", unhex(PHFL)),
        (b"blwh", unhex(BLWH)),
        (b"blnc", unhex(BLNC_FILL)),
        (b"vibA", unhex(VIBA_NEUTRAL)),
        (
            b"blwh",
            bw([1, 2, 3, 4, 5, 6], Some(true), Some([1.0, 2.0, 3.0])),
        ),
        (b"phfl", phfl(3, 0, [0; 4], 25, 1)),
    ];
    for (key, bytes) in &blocks {
        for len in 0..=bytes.len() {
            let _ = parse_block(key, bytes.get(..len).unwrap_or(&[]));
        }
        for i in 0..bytes.len() {
            for flip in [0x01_u8, 0x80, 0xFF] {
                let mut m = bytes.clone();
                if let Some(b) = m.get_mut(i) {
                    *b ^= flip;
                }
                let _ = parse_block(key, &m);
            }
        }
    }
    // The truncations of the real descriptors are not readable (short
    // data never reads as a different valid layer).
    let blwh = unhex(BLWH);
    for len in 0..290 {
        let p = parse_block(b"blwh", blwh.get(..len).unwrap_or(&[]));
        assert_eq!(p, Some(Parsed::BlackWhite(Err(()))), "len {len}");
    }
    let blnc = unhex(BLNC_FILL);
    for len in 0..19 {
        let p = parse_block(b"blnc", blnc.get(..len).unwrap_or(&[]));
        assert_eq!(p, Some(Parsed::ColorBalance(Err(()))), "len {len}");
    }
    let expa = unhex(EXPA_FILL);
    for len in 0..14 {
        assert_eq!(
            parse_block(b"expA", expa.get(..len).unwrap_or(&[])),
            Some(Parsed::Exposure(Err(()))),
            "len {len}"
        );
    }
    let phfl_bytes = unhex(PHFL);
    for len in 0..17 {
        assert_eq!(
            parse_block(b"phfl", phfl_bytes.get(..len).unwrap_or(&[])),
            Some(Parsed::PhotoFilter(Err(()))),
            "len {len}"
        );
    }
}

/// A descriptor claiming four billion items in a few bytes, a deeply
/// nested one and an over-long block are refused before they allocate.
#[test]
fn hostile_descriptors_are_refused_with_bounded_work() {
    // Item count 0xFFFFFFFF in a block of ~30 bytes.
    let mut huge = descriptor(&[]);
    let n = huge.len();
    huge[n - 4..].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(parse_block(b"vibA", &huge), Some(Parsed::Vibrance(Err(()))));
    assert_eq!(
        parse_block(b"blwh", &huge),
        Some(Parsed::BlackWhite(Err(())))
    );

    // A tint colour nested 200 deep.
    let mut nested = Vec::new();
    for _ in 0..200 {
        nested.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        nested.extend_from_slice(b"RGBC");
        nested.extend_from_slice(&1_u32.to_be_bytes());
        key(&mut nested, b"Rd  ");
        nested.extend_from_slice(b"Objc");
    }
    let deep = descriptor(&[(b"tintColor", b"Objc", nested)]);
    assert_eq!(
        parse_block(b"blwh", &deep),
        Some(Parsed::BlackWhite(Err(())))
    );

    // More items than the per-block budget (256): each is a 14-byte long.
    let many: Vec<(&[u8], &[u8], Vec<u8>)> = (0..300)
        .map(|_| (&b"Xxxx"[..], &b"long"[..], long(0)))
        .collect();
    assert_eq!(
        parse_block(b"vibA", &descriptor(&many)),
        Some(Parsed::Vibrance(Err(()))),
        "the item budget stops a 300-item block"
    );

    // A block over 64 KiB is refused without being parsed.
    let mut long_block = vib(Some(1), None);
    long_block.resize(70_000, 0);
    assert_eq!(
        parse_block(b"vibA", &long_block),
        Some(Parsed::Vibrance(Err(())))
    );

    // An unknown value type.
    let unknown = descriptor(&[(b"vibrance", b"zzzz", vec![0; 8])]);
    assert_eq!(
        parse_block(b"vibA", &unknown),
        Some(Parsed::Vibrance(Err(())))
    );
}

// ---------------------------------------------------------------------
// Through the real reader
// ---------------------------------------------------------------------

const KEYS: [&[u8; 4]; 5] = [b"vibA", b"expA", b"phfl", b"blwh", b"blnc"];

fn file_with(key: &[u8; 4], block: &[u8], tweak: impl FnOnce(&mut TestLayer)) -> TestPsd {
    let block = block.to_vec();
    let key = *key;
    let layer = TestLayer::empty("Adjust 1").with(|l| {
        l.blocks.push((key, block));
        l.mask = Some((0, 0, 2, 1, 255, 0));
        l.channels.push((-2, vec![0, 255]));
        tweak(l);
    });
    TestPsd::new(1, 2, 2, 8).with(|p| {
        p.layers = vec![
            TestLayer::divider(),
            TestLayer::pixels("p", 0, 0, 2, 2, 8, &[[255, 0, 0, 255]; 4]),
            layer,
            TestLayer::group("G", *b"pass"),
        ];
    })
}

fn top_adjustment(document: &PsdDocument) -> Option<(aurora_doc::LayerId, Adjustment)> {
    let group = *document.layers.roots().first()?;
    let top = *document.layers.children(group)?.first()?;
    document.layers.adjustment(top).cloned().map(|a| (top, a))
}

fn children_count(document: &PsdDocument) -> usize {
    document
        .layers
        .roots()
        .first()
        .and_then(|&g| document.layers.children(g))
        .map_or(0, <[_]>::len)
}

#[test]
fn each_kind_opens_as_an_adjustment_with_its_properties_and_is_not_reported() {
    let blocks: [(&[u8; 4], Vec<u8>, &str); 5] = [
        (b"vibA", unhex(VIBA_FILL), "Vibrance"),
        (b"expA", unhex(EXPA_FILL), "Exposure"),
        (b"phfl", unhex(PHFL), "Photo Filter"),
        (b"blwh", unhex(BLWH), "Black & White"),
        (b"blnc", unhex(BLNC_FILL), "Color Balance"),
    ];
    for (key, block, label) in blocks {
        let bytes = file_with(key, &block, |l| {
            l.opacity = 128;
            l.blend = *b"mul ";
            l.flags = 0x02;
            l.clipping = 1;
        })
        .write();
        let document = ok(read(&bytes));
        let Some((top, adjustment)) = top_adjustment(&document) else {
            unreachable!("{label}: the top child is the adjustment")
        };
        assert_eq!(adjustment.label(), label);
        assert_eq!(document.layers.opacity(top), Some(128.0 / 255.0), "{label}");
        assert_eq!(document.layers.blend_mode(top), Some(BlendMode::Multiply));
        assert_eq!(document.layers.visible(top), Some(false), "{label}");
        assert!(document.layers.mask(top).is_some(), "{label}");
        assert!(document.layers.clip_base(top).is_some(), "{label}: clipped");
        assert!(
            document.report.is_empty(),
            "{label}: {:?}",
            document.report.items
        );
    }
}

#[test]
fn a_record_with_two_blocks_of_one_kind_uses_the_last() {
    let bytes = file_with(b"blnc", &unhex(BLNC_FILL), |l| {
        l.blocks.push((*b"blnc", unhex(BLNC_CLIP)));
    })
    .write();
    let document = ok(read(&bytes));
    let Some((_, Adjustment::ColorBalance(p))) = top_adjustment(&document) else {
        unreachable!("color balance")
    };
    assert_eq!(p.ranges()[1], [28.0, 100.0, 53.0]);
}

#[test]
fn out_of_range_and_unreadable_layers_are_left_out_with_their_own_line() {
    let cases: [(&[u8; 4], Vec<u8>, &str); 7] = [
        (
            b"vibA",
            vib(Some(500), None),
            "1 Vibrance layer was left out",
        ),
        (
            b"expA",
            exp(1, 0.0, 0.0, 0.01),
            "1 Exposure layer was left out",
        ),
        (b"expA", vec![0, 1, 0], "1 Exposure layer was left out"),
        (
            b"phfl",
            phfl(2, 7, [100, 0, 0, 0], 0, 0),
            "1 Photo Filter layer was left out",
        ),
        (
            b"blwh",
            bw([999, 0, 0, 0, 0, 0], None, None),
            "1 Black & White layer was left out",
        ),
        (
            b"blnc",
            blnc([0, 0, 0, 0, 0, 0, 0, 0, -120], 1),
            "1 Color Balance layer was left out",
        ),
        (
            b"phfl",
            phfl(3, 0, [0; 4], 25, 1),
            "colour is not stored as Lab",
        ),
    ];
    for (key, block, needle) in cases {
        let document = ok(read(&file_with(key, &block, |_| {}).write()));
        let items = document.report.items.join("\n");
        assert!(items.contains(needle), "{key:?}: {items}");
        assert_eq!(
            children_count(&document),
            1,
            "{key:?}: only the pixel layer remains"
        );
    }
}

#[test]
fn a_grayscale_file_s_layers_are_left_out_and_say_so() {
    for (key, block, label) in [
        (b"vibA", unhex(VIBA_FILL), "Vibrance"),
        (b"expA", unhex(EXPA_FILL), "Exposure"),
        (b"phfl", unhex(PHFL), "Photo Filter"),
        (b"blwh", unhex(BLWH), "Black & White"),
        (b"blnc", unhex(BLNC_FILL), "Color Balance"),
    ] {
        let layer = TestLayer::empty("Adjust 1").with(|l| l.blocks.push((*key, block.clone())));
        let bytes = TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.color_mode = 1;
                p.layers = vec![TestLayer::divider(), layer, TestLayer::group("G", *b"pass")];
            })
            .write();
        let document = ok(read(&bytes));
        let items = document.report.items.join("\n");
        assert!(
            items.contains(&format!(
                "1 {label} layer in this Grayscale file was left out"
            )),
            "{label}: {items}"
        );
    }
}

/// A record that also carries another, unmodelled adjustment key keeps
/// the old behaviour: left out as "other kind", never half-imported.
#[test]
fn a_record_with_an_unmodelled_adjustment_key_is_still_left_out() {
    let document = ok(read(
        &file_with(b"blnc", &unhex(BLNC_FILL), |l| {
            l.blocks.push((*b"brit", vec![0; 4]));
        })
        .write(),
    ));
    let items = document.report.items.join("\n");
    assert!(items.contains("1 adjustment layer"), "{items}");
    assert_eq!(children_count(&document), 1);
}

#[test]
fn every_key_is_recognised_and_only_these_five() {
    for k in KEYS {
        assert!(read_adjust::is_key(*k));
        assert!(parse_block(k, &[]).is_some());
    }
    for k in [
        b"curv", b"levl", b"hue2", b"brit", b"mixr", b"hue ", b"nvrt",
    ] {
        assert!(!read_adjust::is_key(*k));
        assert_eq!(parse_block(k, &[]), None);
    }
}

// ---------------------------------------------------------------------
// The tint colour and Lab maths
// ---------------------------------------------------------------------

#[test]
fn the_tint_colour_maps_to_hsl_hue_and_saturation() {
    // Pure and half-saturated colours, computed by hand.
    for (rgb, hue, sat) in [
        ([255.0, 0.0, 0.0], 0.0, 100.0),
        ([255.0, 255.0, 0.0], 60.0, 100.0),
        ([0.0, 255.0, 0.0], 120.0, 100.0),
        ([0.0, 255.0, 255.0], 180.0, 100.0),
        ([0.0, 0.0, 255.0], 240.0, 100.0),
        ([255.0, 0.0, 255.0], 300.0, 100.0),
        ([191.25, 63.75, 63.75], 0.0, 50.0),
    ] {
        let (h, s) = tint_hue_saturation(rgb);
        assert!(
            (h - hue).abs() < 1e-3 && (s - sat).abs() < 1e-3,
            "{rgb:?}: {h} {s}"
        );
    }
    // A grey has no hue: Aurora's default tint hue, zero saturation.
    assert_eq!(tint_hue_saturation([128.0, 128.0, 128.0]), (35.0, 0.0));
    assert_eq!(tint_hue_saturation([0.0, 0.0, 0.0]), (35.0, 0.0));
    assert_eq!(tint_hue_saturation([255.0, 255.0, 255.0]), (35.0, 0.0));
}

#[test]
fn tint_rgb_is_the_inverse_of_the_hue_and_saturation() {
    for hue in (0..360).step_by(7) {
        for sat in [1.0_f32, 25.0, 50.0, 99.0, 100.0] {
            let rgb = tint_rgb(hue as f32, sat);
            assert!(rgb.iter().all(|c| (0.0..=255.0).contains(c)), "{rgb:?}");
            let (h, s) = tint_hue_saturation(rgb);
            assert!((h - hue as f32).abs() < 1e-2, "{hue} {sat}: {h}");
            assert!((s - sat).abs() < 1e-2, "{hue} {sat}: {s}");
        }
    }
    // Zero saturation is mid grey.
    assert_eq!(tint_rgb(200.0, 0.0), [127.5; 3]);
}

#[test]
fn lab_conversion_hits_known_colours_and_round_trips() {
    use super::lab::{lab_to_srgb, srgb_to_lab};
    // White and black.
    let (l, a, b) = srgb_to_lab([1.0, 1.0, 1.0]);
    assert!(
        (l - 100.0).abs() < 1e-3 && a.abs() < 2e-3 && b.abs() < 2e-3,
        "{l} {a} {b}"
    );
    assert_eq!(srgb_to_lab([0.0; 3]).0.abs() < 1e-9, true);
    assert_eq!(
        lab_to_srgb(100.0, 0.0, 0.0).map(|c| (c * 255.0).round()),
        [255.0; 3]
    );
    assert_eq!(lab_to_srgb(0.0, 0.0, 0.0), [0.0; 3]);
    // Pure sRGB red in D50 Lab (Lindbloom): L 54.29, a 80.80, b 69.89.
    let (l, a, b) = srgb_to_lab([1.0, 0.0, 0.0]);
    assert!((l - 54.29).abs() < 0.02 && (a - 80.80).abs() < 0.05 && (b - 69.89).abs() < 0.05);
    // The corpus preset, in both directions.
    let rgb = lab_to_srgb(67.06, 32.0, 120.0);
    assert!((rgb[0] * 255.0 - 236.0).abs() < 0.5 && (rgb[1] * 255.0 - 138.0).abs() < 0.5);
    // Round trip over a grid.
    for r in 0..=8 {
        for g in 0..=8 {
            for bl in 0..=8 {
                let c = [r as f32 / 8.0, g as f32 / 8.0, bl as f32 / 8.0];
                let (l, a, b) = srgb_to_lab(c);
                let back = lab_to_srgb(l, a, b);
                for (x, y) in c.iter().zip(back) {
                    assert!((x - y).abs() < 1e-4, "{c:?} -> {back:?}");
                }
            }
        }
    }
    // Non-finite input is black, not NaN.
    assert_eq!(lab_to_srgb(f64::NAN, 0.0, 0.0), [0.0; 3]);
    assert!(
        lab_to_srgb(1e300, 1e300, -1e300)
            .iter()
            .all(|c| (0.0..=1.0).contains(c))
    );
}

// ---------------------------------------------------------------------
// The corpus (SKIPPED when absent)
// ---------------------------------------------------------------------

fn corpus(relative: &str) -> Option<PsdDocument> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(relative);
    let Ok(bytes) = std::fs::read(&path) else {
        println!("SKIPPED: corpus file not present at {}", path.display());
        return None;
    };
    Some(ok(read(&bytes)))
}

fn adjustments_of(document: &PsdDocument) -> Vec<(String, Adjustment)> {
    fn walk(
        tree: &aurora_doc::LayerTree,
        ids: &[aurora_doc::LayerId],
        out: &mut Vec<(String, Adjustment)>,
    ) {
        for &id in ids {
            if let Some(a) = tree.adjustment(id) {
                out.push((tree.name(id).unwrap_or_default().to_owned(), a.clone()));
            }
            if let Some(children) = tree.children(id) {
                walk(tree, children, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&document.layers, document.layers.roots(), &mut out);
    out
}

#[test]
fn corpus_single_layer_fixtures_import_their_neutral_adjustment() {
    for (file, label) in [
        ("layers/vibrance.psd", "Vibrance"),
        ("layers/exposure.psd", "Exposure"),
        ("layers/photo-filter.psd", "Photo Filter"),
        ("layers/black-and-white.psd", "Black & White"),
        ("layers/color-balance.psd", "Color Balance"),
    ] {
        let Some(document) = corpus(file) else {
            continue;
        };
        let found = adjustments_of(&document);
        assert_eq!(found.len(), 1, "{file}: {found:?}");
        assert_eq!(found[0].1.label(), label, "{file}");
        assert!(
            !document
                .report
                .items
                .join("\n")
                .contains("adjustment layer"),
            "{file}: {:?}",
            document.report.items
        );
    }
    let Some(document) = corpus("layers/exposure.psd") else {
        return;
    };
    let found = adjustments_of(&document);
    assert_eq!(
        found[0].1,
        Adjustment::Exposure(aurora_core::ExposureParams::identity())
    );
    let Some(document) = corpus("layers/color-balance.psd") else {
        return;
    };
    let found = adjustments_of(&document);
    assert_eq!(
        found[0].1,
        Adjustment::ColorBalance(aurora_core::ColorBalanceParams::identity())
    );
}

#[test]
fn corpus_fill_adjustments_imports_all_five_kinds_with_photoshop_s_values() {
    let Some(document) = corpus("fill_adjustments.psd") else {
        return;
    };
    let found = adjustments_of(&document);
    let by = |label: &str| {
        found
            .iter()
            .find(|(_, a)| a.label() == label)
            .map(|(_, a)| a.clone())
    };
    let Some(Adjustment::Vibrance(v)) = by("Vibrance") else {
        unreachable!("vibrance")
    };
    assert_eq!((v.vibrance(), v.saturation()), (-6.0, 2.0));
    let Some(Adjustment::Exposure(e)) = by("Exposure") else {
        unreachable!("exposure")
    };
    assert_eq!(e.exposure().to_bits(), 0xbec7_ae14);
    let Some(Adjustment::ColorBalance(c)) = by("Color Balance") else {
        unreachable!("balance")
    };
    assert_eq!(
        c.ranges(),
        [[-4.0, 2.0, -5.0], [10.0, 4.0, -9.0], [1.0, -9.0, -3.0]]
    );
    let Some(Adjustment::PhotoFilter(p)) = by("Photo Filter") else {
        unreachable!("filter")
    };
    assert_eq!(p.density(), 25.0);
    let Some(Adjustment::BlackWhite(b)) = by("Black & White") else {
        unreachable!("bw")
    };
    assert_eq!(b.weights(), aurora_core::BLACK_WHITE_DEFAULTS);
    // The Black & White layer is hidden in the file, and stays hidden.
    let hidden = found.iter().any(|(name, a)| {
        a.label() == "Black & White"
            && document.layers.roots().iter().any(|&id| {
                document.layers.name(id) == Some(name) && document.layers.visible(id) == Some(false)
            })
    });
    assert!(hidden);
}

#[test]
fn corpus_exposure_rgb_imports_four_masked_exposures() {
    let Some(document) = corpus("adjustments/exposure_rgb.psd") else {
        return;
    };
    let found = adjustments_of(&document);
    assert_eq!(found.len(), 4, "{found:?}");
    for (i, hex) in EXPA_RGB.iter().enumerate() {
        let name = format!("Exposure {}", i + 1);
        let Some((_, Adjustment::Exposure(p))) = found.iter().find(|(n, _)| *n == name) else {
            unreachable!("{name}")
        };
        let Parsed::Exposure(Ok(b)) = parsed(b"expA", hex) else {
            unreachable!()
        };
        assert_eq!(
            (p.exposure(), p.offset(), p.gamma()),
            (b.exposure, b.offset, b.gamma)
        );
        let id = document
            .layers
            .roots()
            .iter()
            .copied()
            .find(|&id| document.layers.name(id) == Some(&name));
        assert!(
            id.is_some_and(|id| document.layers.mask(id).is_some()),
            "{name} has its mask"
        );
    }
    assert!(
        !document
            .report
            .items
            .join("\n")
            .contains("adjustment layer")
    );
}

#[test]
fn corpus_clipping_mask2_imports_its_clipped_color_balance() {
    let Some(document) = corpus("clipping-mask2.psd") else {
        return;
    };
    let found = adjustments_of(&document);
    let Some((name, Adjustment::ColorBalance(p))) =
        found.iter().find(|(_, a)| a.label() == "Color Balance")
    else {
        unreachable!("color balance")
    };
    assert_eq!(p.ranges()[1], [28.0, 100.0, 53.0]);
    let _ = name;
}

// ---------------------------------------------------------------------
// Reports for adjustments Aurora shows stronger than Photoshop
// ---------------------------------------------------------------------

/// A group (Pass Through) holding a pixel layer and an adjustment, with the
/// group's Fill byte set to `fill` and the adjustment clipped or not.
fn grouped_file(fill: u8, clipped: bool, with_pixels_below: bool) -> TestPsd {
    let adjustment = TestLayer::empty("Adjust 1").with(|l| {
        l.blocks.push((*b"blnc", unhex(BLNC_FILL)));
        l.clipping = u8::from(clipped);
    });
    TestPsd::new(1, 2, 2, 8).with(|p| {
        let mut layers = vec![TestLayer::divider()];
        if with_pixels_below {
            layers.push(TestLayer::pixels(
                "p",
                0,
                0,
                2,
                2,
                8,
                &[[255, 0, 0, 255]; 4],
            ));
        }
        layers.push(adjustment);
        layers.push(TestLayer::group("G", *b"pass").with(|g| {
            g.blocks.push((*b"iOpa", vec![fill]));
        }));
        p.layers = layers;
    })
}

#[test]
fn an_adjustment_under_a_pass_through_group_with_reduced_fill_is_reported_as_stronger() {
    let document = ok(read(&grouped_file(102, false, true).write()));
    let items = document.report.items.join("\n");
    assert!(
        items.contains(
            "1 adjustment layer sits inside a Pass Through group whose Fill is below 100%"
        ) && items.contains("may look much stronger than in Photoshop"),
        "{items}"
    );
    // The layer stays imported.
    assert!(top_adjustment(&document).is_some());
    // Full Fill: no such line.
    let document = ok(read(&grouped_file(255, false, true).write()));
    let items = document.report.items.join("\n");
    assert!(!items.contains("much stronger"), "{items}");
}

#[test]
fn a_clipped_adjustment_shown_unclipped_is_reported_as_stronger() {
    // Clipped with nothing beneath it in its group: shown unclipped.
    let document = ok(read(&grouped_file(255, true, false).write()));
    let items = document.report.items.join("\n");
    assert!(
        items.contains("1 clipped adjustment layer is shown unclipped")
            && items.contains("may look much stronger than in Photoshop"),
        "{items}"
    );
    assert!(top_adjustment(&document).is_some(), "still imported");
    // Clipped to a pixel layer: really clipped, nothing to report.
    let document = ok(read(&grouped_file(255, true, true).write()));
    let items = document.report.items.join("\n");
    assert!(!items.contains("much stronger"), "{items}");
}

#[test]
fn the_left_out_note_calls_four_kinds_approximations() {
    let document = ok(read(
        &file_with(b"blnc", &unhex(BLNC_FILL), |l| {
            l.blocks.push((*b"brit", vec![0; 4]));
        })
        .write(),
    ));
    let items = document.report.items.join("\n");
    assert!(
        items.contains("approximations that have not been compared with Photoshop"),
        "{items}"
    );
}

#[test]
fn corpus_files_where_aurora_shows_the_adjustment_stronger_say_so() {
    if let Some(document) = corpus("passthrough_fill_adjustment.psd") {
        let items = document.report.items.join("\n");
        assert!(
            items.contains("sits inside a Pass Through group whose Fill is below 100%"),
            "{items}"
        );
        assert!(!adjustments_of(&document).is_empty());
    }
    if let Some(document) = corpus("passthrough_clipping_mask_adjustment.psd") {
        let items = document.report.items.join("\n");
        // The clipped layer here is a pixel layer whose base is a group, not
        // a clipped adjustment: the existing clipping note names it, and no
        // false claim about a clipped adjustment is made. (Aurora's error on
        // this file is dominated by that unclipped layer.)
        assert!(
            items.contains("1 clipped layer is shown unclipped"),
            "{items}"
        );
        assert!(!items.contains("clipped adjustment layer"), "{items}");
        assert_eq!(adjustments_of(&document).len(), 2);
    }
}
