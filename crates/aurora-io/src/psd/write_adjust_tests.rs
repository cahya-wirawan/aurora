//! Tests for the PSD writer's adjustment layers (0.205.0): the exact bytes
//! of `curv`, `levl` and `hue2` (against blocks Photoshop itself wrote,
//! taken from psd-tools' fixtures), the records around them, the round
//! trip through the writer's two verifier passes and Aurora's reader, and
//! the rounding the report itemises.

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

use aurora_core::{
    CurvePoint, CurvesParams, HueRange, HueSaturationParams, HueSaturationRangesParams,
    LevelsChannel, LevelsParams, ToneCurve,
};
use aurora_doc::{Adjustment, BlendMode, LayerId, LayerKind, LayerTree};

use super::write::{PsdSaveOptions, PsdSaveRefusal, psd_save_preflight};
use super::write_adjust::encode_adjustment;
use super::write_tests::{Doc, ok, read_back, records, rect, save, shape};
use crate::error::IoError;

// Photoshop's own blocks, from psd-tools' fixtures (`curves_rgb.psd`
// "Curves 2", `levels_rgb.psd` "Levels 4", `huesaturation_colorize_rgb.psd`
// "Hue/Saturation 2"), as psd-tools re-serialises them.
const PHOTOSHOP_CURV: &str = concat!(
    "0000010000000900040003001a0054003700aa00ab00ff00ff00030000000000",
    "6e00a700a600ff43727620000400000002000000040003001a0054003700aa00",
    "ab00ff00ff0003000300000000006e00a700a600ff000000"
);
const PHOTOSHOP_LEVL: &str = concat!(
    "000200300087004a008d0064005100ff006300c0006400000080000000ff0064",
    "000000ff004e00ab00d4000000ff000000ff0064000000ff000000ff00640000",
    "00ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff",
    "000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff0000",
    "00ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff",
    "0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064",
    "000000ff000000ff0064000000ff000000ff0064000000ff000000ff00640000",
    "00ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff",
    "000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff0000",
    "00ff00644c766c730003003e000000ff000000ff0064000000ff000000ff0064",
    "000000ff000000ff0064000000ff000000ff0064000000ff000000ff00640000",
    "00ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff",
    "000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff0000",
    "00ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff",
    "0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff0064",
    "000000ff000000ff0064000000ff000000ff0064000000ff000000ff00640000",
    "00ff000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff",
    "000000ff0064000000ff000000ff0064000000ff000000ff0064000000ff0000",
    "00ff0064000000ff000000ff0064000000ff000000ff0064000000ff000000ff",
    "0064000000ff000000ff0064000000ff000000ff00640000"
);
const PHOTOSHOP_HUE2: &str = concat!(
    "00020100ff8a002d000e000000000000013b0159000f002d000000000000000f",
    "002d004b0069000000000000004b0069008700a5000000000000008700a500c3",
    "00e100000000000000c300e100ff011d00000000000000ff011d013b01590000",
    "00000000"
);

fn be32_at(bytes: &[u8], at: usize) -> usize {
    bytes
        .get(at..at + 4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, |b| u32::from_be_bytes(b) as usize)
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2).unwrap_or("0"), 16).unwrap_or(0))
        .collect()
}

fn curve(levels: &[(u16, u16)]) -> ToneCurve {
    let points: Vec<CurvePoint> = levels
        .iter()
        .map(|&(x, y)| CurvePoint::new(f32::from(x) / 255.0, f32::from(y) / 255.0))
        .collect();
    ok(ToneCurve::new(&points))
}

fn channel(ib: u16, iw: u16, g: u16, ob: u16, ow: u16) -> LevelsChannel {
    ok(LevelsChannel::new(
        f32::from(ib) / 255.0,
        f32::from(iw) / 255.0,
        f32::from(g) / 100.0,
        f32::from(ob) / 255.0,
        f32::from(ow) / 255.0,
    ))
}

fn sample_curves() -> CurvesParams {
    CurvesParams {
        composite: curve(&[(26, 3), (55, 84), (171, 170), (255, 255)]),
        red: None,
        green: None,
        blue: Some(curve(&[(0, 0), (167, 110), (255, 166)])),
    }
}

fn sample_levels() -> LevelsParams {
    LevelsParams {
        master: channel(48, 135, 100, 74, 141),
        red: channel(81, 255, 100, 99, 192),
        green: channel(0, 128, 100, 0, 255),
        blue: channel(0, 255, 212, 78, 171),
    }
}

fn hue_ranges() -> HueSaturationRangesParams {
    let base = ok(HueSaturationParams::new(
        -27.0, 30.0, 10.0, false, 0.0, 25.0, 0.0,
    ));
    let mut ranges = [HueRange::standard(0); 6];
    for (i, slot) in ranges.iter_mut().enumerate() {
        *slot = HueRange::standard(i);
    }
    // Reds with custom wrapping edges, Blues with the standard ones.
    ranges[0] = ok(HueRange::new([207.0, 239.0, 351.0, 37.0], 32.0, 93.0, 0.0));
    ranges[4] = ok(HueRange::new(
        aurora_core::DEFAULT_HUE_RANGE_EDGES[4],
        -12.0,
        0.0,
        40.0,
    ));
    HueSaturationRangesParams::new(base, ranges)
}

fn adjustment_named(tree: &LayerTree, name: &str) -> Option<Adjustment> {
    fn walk(tree: &LayerTree, ids: &[LayerId], name: &str) -> Option<Adjustment> {
        for &id in ids {
            if tree.name(id) == Some(name)
                && let Some(LayerKind::Adjustment(a)) = tree.kind(id)
            {
                return Some(a.clone());
            }
            if let Some(children) = tree.children(id)
                && let Some(found) = walk(tree, children, name)
            {
                return Some(found);
            }
        }
        None
    }
    walk(tree, tree.roots(), name)
}

fn id_named(tree: &LayerTree, name: &str) -> LayerId {
    fn walk(tree: &LayerTree, ids: &[LayerId], name: &str) -> Option<LayerId> {
        for &id in ids {
            if tree.name(id) == Some(name) {
                return Some(id);
            }
            if let Some(children) = tree.children(id)
                && let Some(found) = walk(tree, children, name)
            {
                return Some(found);
            }
        }
        None
    }
    let Some(id) = walk(tree, tree.roots(), name) else {
        unreachable!("layer {name}")
    };
    id
}

/// What Photoshop wrote is what the writer writes, byte for byte, for the
/// same settings (read through Aurora's own reader first).
#[test]
fn the_writer_reproduces_photoshops_own_blocks_byte_for_byte() {
    let curves = encode_adjustment(&Adjustment::Curves(sample_curves()));
    let Ok(curves) = curves else {
        unreachable!("{curves:?}")
    };
    assert_eq!(curves.key, *b"curv");
    assert_eq!(curves.data, unhex(PHOTOSHOP_CURV));
    assert!(!curves.rounding.rounded && !curves.rounding.merged_points);

    let levels = encode_adjustment(&Adjustment::Levels(sample_levels()));
    let Ok(levels) = levels else {
        unreachable!("{levels:?}")
    };
    assert_eq!(levels.key, *b"levl");
    assert_eq!(levels.data, unhex(PHOTOSHOP_LEVL));
    assert!(!levels.rounding.rounded);

    // Colorize on, hue 242 (stored as -118), all six ranges neutral.
    let colorize = ok(HueSaturationParams::new(
        0.0, 0.0, 0.0, true, 242.0, 45.0, 14.0,
    ));
    let hue = encode_adjustment(&Adjustment::HueSaturation(colorize));
    let Ok(hue) = hue else {
        unreachable!("{hue:?}")
    };
    assert_eq!(hue.key, *b"hue2");
    assert_eq!(hue.data, unhex(PHOTOSHOP_HUE2));
    assert!(!hue.rounding.rounded && !hue.rounding.ranges_reset);
}

/// Every block reads back as exactly the adjustment it was made from when
/// nothing needed rounding, in the three kinds and both Hue/Saturation
/// variants.
#[test]
fn encoded_blocks_read_back_as_the_original_adjustment() {
    let originals = [
        Adjustment::Curves(sample_curves()),
        Adjustment::Curves(CurvesParams::identity()),
        Adjustment::Levels(sample_levels()),
        Adjustment::Levels(LevelsParams::identity()),
        Adjustment::HueSaturation(HueSaturationParams::identity()),
        Adjustment::HueSaturation(ok(HueSaturationParams::new(
            -180.0, -100.0, 100.0, false, 360.0, 100.0, -100.0,
        ))),
        Adjustment::from_hue_saturation(hue_ranges()),
    ];
    for original in originals {
        let encoded = encode_adjustment(&original);
        let Ok(encoded) = encoded else {
            unreachable!("{original:?}: {encoded:?}")
        };
        assert_eq!(encoded.data.len() % 4, 0, "{original:?}");
        let expect = match &original {
            // A Colorize hue of 360 is the same angle as 0.
            Adjustment::HueSaturation(p) if p.colorize_hue() >= 360.0 => {
                Adjustment::HueSaturation(ok(p.with_colorize_hue(0.0)))
            }
            other => other.clone(),
        };
        assert_eq!(encoded.readback, expect, "{original:?}");
    }
}

#[test]
fn a_curve_with_no_other_curves_still_writes_its_composite() {
    let encoded = encode_adjustment(&Adjustment::Curves(CurvesParams::identity()));
    let Ok(encoded) = encoded else { unreachable!() };
    // Bitmap: only the composite; one legacy curve and one `Crv ` item.
    assert_eq!(encoded.data.get(3..7), Some(&[0, 0, 0, 1][..]));
    // A red-only curve leaves the identity composite out.
    let red_only = CurvesParams {
        red: Some(curve(&[(0, 10), (255, 200)])),
        ..CurvesParams::identity()
    };
    let encoded = encode_adjustment(&Adjustment::Curves(red_only.clone()));
    let Ok(encoded) = encoded else { unreachable!() };
    assert_eq!(encoded.data.get(3..7), Some(&[0, 0, 0, 2][..]));
    assert_eq!(encoded.readback, Adjustment::Curves(red_only));
}

/// A document with every kind, masked, clipped, hidden, locked, in a group
/// and with its own mode and opacity, through the writer's two verifier
/// passes and back through the reader.
fn adjustments_doc() -> Doc {
    let mut doc = Doc::new((16, 16));
    doc.layer("Background", rect(0, 0, 16, 16), None);
    let curves = ok(doc.tree.add_adjustment_layer_at(
        "Curves A",
        Adjustment::Curves(sample_curves()),
        None,
        0,
    ));
    ok(doc.tree.set_opacity(curves, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(curves, BlendMode::Multiply));
    doc.mask(curves, rect(2, 2, 10, 10), (0, 0, 10, 10), |x, y| {
        f32::from(u16::try_from((x * 20 + y) % 255).unwrap_or(0)) / 255.0
    });
    let group = doc.group("Group", None);
    doc.layer_in("Base", rect(0, 0, 16, 16), None, Some(group));
    let clipped = ok(doc.tree.add_adjustment_layer_at(
        "Hue clipped",
        Adjustment::HueSaturation(ok(HueSaturationParams::new(
            10.0, -20.0, 5.0, false, 0.0, 25.0, 0.0,
        ))),
        Some(group),
        0,
    ));
    ok(doc.tree.set_clipped(clipped, true));
    ok(doc.tree.add_adjustment_layer_at(
        "Levels in group",
        Adjustment::Levels(sample_levels()),
        Some(group),
        0,
    ));
    let ranged = ok(doc.tree.add_adjustment_layer_at(
        "Hue ranges",
        Adjustment::from_hue_saturation(hue_ranges()),
        None,
        0,
    ));
    ok(doc.tree.set_visible(ranged, false));
    doc
}

#[test]
fn adjustment_layers_round_trip_through_write_verify_and_read() {
    let mut doc = adjustments_doc();
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("adjustments.psd");
    // `save` writes, verifies with both passes and swaps in.
    let outcome = ok(save(&mut doc, &target, false));
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    let bytes = ok(std::fs::read(&target));
    let back = read_back(&bytes);
    assert!(back.report.items.is_empty(), "{:?}", back.report.items);
    assert_eq!(
        shape(&back.layers, back.layers.roots()),
        shape(&doc.tree, doc.tree.roots())
    );
    for name in ["Curves A", "Levels in group", "Hue clipped", "Hue ranges"] {
        assert_eq!(
            adjustment_named(&back.layers, name),
            adjustment_named(&doc.tree, name),
            "{name}"
        );
        let (want, got) = (id_named(&doc.tree, name), id_named(&back.layers, name));
        assert_eq!(back.layers.opacity(got), doc.tree.opacity(want), "{name}");
        assert_eq!(back.layers.blend_mode(got), doc.tree.blend_mode(want));
        assert_eq!(back.layers.visible(got), doc.tree.visible(want), "{name}");
        assert_eq!(
            back.layers.clip_base(got).is_some(),
            doc.tree.clip_base(want).is_some(),
            "{name}"
        );
        assert_eq!(
            back.layers.mask(got).map(|m| m.bounds),
            doc.tree.mask(want).map(|m| m.bounds),
            "{name}"
        );
    }
    assert!(
        back.layers
            .mask(id_named(&back.layers, "Curves A"))
            .is_some()
    );
}

/// The records are Photoshop's: an empty rectangle, four empty colour
/// channels (a compression field only), a `-2` channel when masked.
#[test]
fn an_adjustment_record_has_an_empty_rectangle_and_empty_channels() {
    let mut doc = adjustments_doc();
    let (bytes, _) = ok(doc.write());
    let recs = records(&bytes);
    // Bottom to top: Background, Curves A, divider, Levels, Hue clipped,
    // Group, Hue ranges.
    let names: Vec<&str> = recs.iter().map(|r| r.4.as_str()).collect();
    assert_eq!(
        names,
        [
            "Background",
            "Curves A",
            "</Layer group>",
            "Base",
            "Hue clipped",
            "Levels in group",
            "Group",
            "Hue ranges"
        ]
    );
    let curves = recs.get(1).unwrap_or_else(|| unreachable!());
    assert_eq!(curves.1, [0; 4]);
    assert_eq!(curves.2, [-1, 0, 1, 2, -2]);
    let levels = recs.get(5).unwrap_or_else(|| unreachable!());
    assert_eq!(levels.1, [0; 4]);
    assert_eq!(levels.2, [-1, 0, 1, 2]);
    // The adjustment block precedes `luni` among the tagged blocks.
    let at = |needle: &[u8]| bytes.windows(needle.len()).position(|w| w == needle);
    let Some(curv) = at(b"8BIMcurv") else {
        unreachable!("curv block")
    };
    let len = be32_at(&bytes, curv + 8);
    assert_eq!(
        bytes.get(curv + 12 + len..curv + 12 + len + 8),
        Some(&b"8BIMluni"[..]),
        "luni follows the curv block directly"
    );
    assert!(at(b"8BIMlevl").is_some() && at(b"8BIMhue2").is_some());
}

#[test]
fn rounding_is_itemised_and_the_rounded_values_read_back() {
    let mut doc = Doc::new((8, 8));
    doc.layer("Background", rect(0, 0, 8, 8), None);
    // Curves: points off the 8-bit grid.
    let fine = ok(ToneCurve::new(&[
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.5, 0.30),
        CurvePoint::new(1.0, 1.0),
    ]));
    ok(doc.tree.add_adjustment_layer_at(
        "Curves",
        Adjustment::Curves(CurvesParams {
            composite: fine,
            ..CurvesParams::identity()
        }),
        None,
        0,
    ));
    // Levels: a gamma beyond hundredths and fractional levels.
    let level = ok(LevelsChannel::new(0.1, 0.9, 1.234_5, 0.05, 0.95));
    ok(doc.tree.add_adjustment_layer_at(
        "Levels",
        Adjustment::Levels(LevelsParams {
            master: level,
            ..LevelsParams::identity()
        }),
        None,
        0,
    ));
    // Hue/Saturation: fractional values and custom edges on a neutral range.
    let base = ok(HueSaturationParams::new(
        10.4, -20.6, 5.0, false, 0.0, 25.0, 0.0,
    ));
    let mut ranges = [HueRange::standard(0); 6];
    for (i, slot) in ranges.iter_mut().enumerate() {
        *slot = HueRange::standard(i);
    }
    ranges[2] = ok(HueRange::new([80.0, 100.0, 140.0, 160.0], 0.0, 0.0, 0.0));
    ok(doc.tree.add_adjustment_layer_at(
        "Hue",
        Adjustment::from_hue_saturation(HueSaturationRangesParams::new(base, ranges)),
        None,
        0,
    ));
    let report = ok(psd_save_preflight(
        &doc.tree,
        (8, 8),
        PsdSaveOptions::default(),
    ));
    let text = report.items.join("\n");
    assert!(text.contains("1 Curves layer's point positions"), "{text}");
    assert!(text.contains("1 Levels layer's values"), "{text}");
    assert!(
        text.contains("1 Hue/Saturation layer's fractional values"),
        "{text}"
    );
    assert!(text.contains("custom colour-range edges"), "{text}");
    // The writer's own report is the preflight's, and the file reads back
    // as the rounded values.
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.report, report);
    let back = read_back(&bytes);
    let Some(Adjustment::Curves(curves)) = adjustment_named(&back.layers, "Curves") else {
        unreachable!("curves")
    };
    let mid = curves.composite.points().get(1).copied();
    assert_eq!(
        mid,
        Some(CurvePoint::new(128.0 / 255.0, 77.0 / 255.0)),
        "0.5 rounds to level 128 (127.5 away from zero) and 0.30 to 77"
    );
    let Some(Adjustment::Levels(levels)) = adjustment_named(&back.layers, "Levels") else {
        unreachable!("levels")
    };
    assert_eq!(levels.master.gamma().to_bits(), 1.23_f32.to_bits());
    assert_eq!(
        levels.master.input_black().to_bits(),
        (26.0_f32 / 255.0).to_bits()
    );
    let hue = adjustment_named(&back.layers, "Hue").and_then(|a| a.hue_saturation());
    let Some(hue) = hue else { unreachable!("hue") };
    assert_eq!(
        (hue.base().hue(), hue.base().saturation()),
        (10.0, -21.0),
        "10.4 and -20.6 round to whole numbers"
    );
    // The neutral range reads back at the standard edges.
    assert_eq!(hue.ranges()[2], HueRange::standard(2));
}

#[test]
fn curve_points_on_the_same_level_are_merged_and_reported() {
    // Three points; the middle two are one 8-bit level apart in the model
    // (1/256 apart) but round to the same level.
    let x1 = 102.5_f32 / 255.0;
    let x2 = x1 + 1.0 / 256.0;
    let merged = ok(ToneCurve::new(&[
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(x1, 0.3),
        CurvePoint::new(x2, 0.4),
        CurvePoint::new(1.0, 1.0),
    ]));
    let encoded = encode_adjustment(&Adjustment::Curves(CurvesParams {
        composite: merged,
        ..CurvesParams::identity()
    }));
    let Ok(encoded) = encoded else {
        unreachable!("{encoded:?}")
    };
    assert!(encoded.rounding.merged_points);
    let Adjustment::Curves(back) = encoded.readback else {
        unreachable!("curves")
    };
    assert_eq!(back.composite.points().len(), 3);
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "Curves",
        Adjustment::Curves(CurvesParams {
            composite: ok(ToneCurve::new(&[
                CurvePoint::new(0.0, 0.0),
                CurvePoint::new(x1, 0.3),
                CurvePoint::new(x2, 0.4),
                CurvePoint::new(1.0, 1.0),
            ])),
            ..CurvesParams::identity()
        }),
        None,
        0,
    ));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    assert!(
        report
            .items
            .iter()
            .any(|i| i.contains("less than one level apart")),
        "{:?}",
        report.items
    );
}

#[test]
fn a_curve_that_collapses_to_one_level_is_refused_with_its_reason() {
    let x1 = 102.5_f32 / 255.0;
    let collapsing = ok(ToneCurve::new(&[
        CurvePoint::new(x1, 0.2),
        CurvePoint::new(x1 + 1.0 / 256.0, 0.8),
    ]));
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "Squeezed",
        Adjustment::Curves(CurvesParams {
            composite: collapsing,
            ..CurvesParams::identity()
        }),
        None,
        0,
    ));
    let refusal = psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default());
    let Err(refusal) = refusal else {
        unreachable!("refused")
    };
    assert!(
        matches!(
            refusal,
            PsdSaveRefusal::AdjustmentNotRepresentable { kind: "Curves", .. }
        ),
        "{refusal:?}"
    );
    let text = refusal.to_string();
    assert!(
        text.contains("Squeezed") && text.contains("fewer than two"),
        "{text}"
    );
}

/// And a document without them still saves: the refusal is per layer.
#[test]
fn a_document_with_only_older_adjustments_still_preflights() {
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "Hue",
        Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
        None,
        0,
    ));
    assert!(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()).is_ok());
}

/// `adjustments.psd` / `adjustments.json` for `scripts/verify_psd_write.py
/// check_adjustments` (psd-tools): the document of
/// [`adjustments_doc`] and the integers each block should hold.
///
/// ```text
/// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
///     adjustments_fixture_for_psd_tools -- --ignored
/// ```
#[test]
#[ignore = "writes the fixture scripts/verify_psd_write.py reads"]
fn adjustments_fixture_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let mut doc = adjustments_doc();
    let (bytes, _) = ok(doc.write());
    ok(std::fs::write(out.join("adjustments.psd"), &bytes));
    let level = |v: f32| (v * 255.0).round() as i64;
    let curve_json = |c: &ToneCurve| -> String {
        let pts: Vec<String> = c
            .points()
            .iter()
            .map(|p| format!("[{}, {}]", level(p.x), level(p.y)))
            .collect();
        format!("[{}]", pts.join(", "))
    };
    let curves = sample_curves();
    let blue = curves.blue.as_ref().map_or_else(String::new, curve_json);
    let levels = sample_levels();
    let records: Vec<String> = [levels.master, levels.red, levels.green, levels.blue]
        .iter()
        .map(|c| {
            format!(
                "[{}, {}, {}, {}, {}]",
                level(c.input_black()),
                level(c.input_white()),
                level(c.output_black()),
                level(c.output_white()),
                (c.gamma() * 100.0).round() as i64
            )
        })
        .collect();
    let hue = hue_ranges();
    let ranges: Vec<String> = hue
        .ranges()
        .iter()
        .map(|r| {
            let e = r.edges();
            format!(
                "[[{}, {}, {}, {}], [{}, {}, {}]]",
                e[0] as i64,
                e[1] as i64,
                e[2] as i64,
                e[3] as i64,
                r.hue() as i64,
                r.saturation() as i64,
                r.lightness() as i64
            )
        })
        .collect();
    let json = format!(
        "{{\"curves\": {{\"composite\": {}, \"blue\": {}}}, \"levels\": [{}], \
         \"hue\": {{\"master\": [{}, {}, {}], \"ranges\": [{}]}}}}\n",
        curve_json(&curves.composite),
        blue,
        records.join(", "),
        hue.base().hue() as i64,
        hue.base().saturation() as i64,
        hue.base().lightness() as i64,
        ranges.join(", ")
    );
    ok(std::fs::write(out.join("adjustments.json"), json));
}

/// The writer's verifier compares an adjustment's block bytes (first pass)
/// and what the reader makes of them (second pass).
#[test]
fn the_verifier_catches_a_wrong_adjustment_block_or_readback() {
    let mut doc = adjustments_doc();
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("adjustments.psd");
    let (bytes, outcome) = ok(doc.write());
    ok(std::fs::write(&path, &bytes));
    ok(super::write::verify_psd_file(&path, &outcome.expected));

    let curves_at = |e: &super::write::PsdExpectation| {
        e.layers
            .iter()
            .position(|l| l.adjustment.as_ref().is_some_and(|a| a.key == *b"curv"))
    };
    let mut wrong_bytes = outcome.expected.clone();
    let Some(i) = curves_at(&wrong_bytes) else {
        unreachable!("curves record")
    };
    if let Some(a) = wrong_bytes
        .layers
        .get_mut(i)
        .and_then(|l| l.adjustment.as_mut())
        && let Some(byte) = a.data.get_mut(8)
    {
        *byte ^= 1;
    }
    let err = super::write::verify_psd_file(&path, &wrong_bytes);
    assert!(
        matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment blocks")),
        "{err:?}"
    );

    let mut wrong_readback = outcome.expected.clone();
    if let Some(a) = wrong_readback
        .layers
        .get_mut(i)
        .and_then(|l| l.adjustment.as_mut())
    {
        a.readback = Adjustment::Curves(CurvesParams::identity());
    }
    let err = super::write::verify_psd_file(&path, &wrong_readback);
    assert!(
        matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment")),
        "{err:?}"
    );
}

/// The Colorize hue is stored signed, as Photoshop does: 180 stays, 181 is
/// written as -179, and both read back as the angle they were.
#[test]
fn the_colorize_hue_is_signed_across_the_180_boundary() {
    for (hue, stored) in [(0.0_f32, 0_i16), (180.0, 180), (181.0, -179), (359.0, -1)] {
        let params = ok(HueSaturationParams::new(
            0.0, 0.0, 0.0, true, hue, 50.0, 0.0,
        ));
        let encoded = encode_adjustment(&Adjustment::HueSaturation(params));
        let Ok(encoded) = encoded else {
            unreachable!("{hue}: {encoded:?}")
        };
        // Header (version, colorize byte, pad), then the Colorize hue.
        let at = encoded
            .data
            .get(4..6)
            .and_then(|b| <[u8; 2]>::try_from(b).ok());
        assert_eq!(at.map(i16::from_be_bytes), Some(stored), "hue {hue}");
        assert!(!encoded.rounding.rounded, "hue {hue}");
        assert_eq!(
            encoded.readback,
            Adjustment::HueSaturation(params),
            "hue {hue}"
        );
    }
}

/// Colorize on with colour-range adjustments: the ranges are written (the
/// file keeps them) but Aurora's reader drops them, so the save report says
/// so; a Colorize layer with neutral ranges, and a ranged layer without
/// Colorize, say nothing.
#[test]
fn colorize_with_range_adjustments_is_reported_as_not_kept_on_reopen() {
    let ranged = |colorize: bool| {
        let base = ok(HueSaturationParams::new(
            0.0, 0.0, 0.0, colorize, 200.0, 40.0, 0.0,
        ));
        let mut ranges = [HueRange::standard(0); 6];
        for (i, slot) in ranges.iter_mut().enumerate() {
            *slot = HueRange::standard(i);
        }
        ranges[1] = ok(HueRange::new(
            aurora_core::DEFAULT_HUE_RANGE_EDGES[1],
            20.0,
            -10.0,
            0.0,
        ));
        HueSaturationRangesParams::new(base, ranges)
    };
    let report_of = |adjustment: Adjustment| {
        let mut tree = LayerTree::new();
        ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
        ok(tree.add_adjustment_layer_at("H", adjustment, None, 0));
        ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default())).items
    };
    let items = report_of(Adjustment::from_hue_saturation(ranged(true)));
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(
        items
            .iter()
            .any(|i| i.contains("Aurora ignores them while Colorize is on")),
        "{items:?}"
    );
    assert_eq!(
        report_of(Adjustment::from_hue_saturation(ranged(false))),
        Vec::<String>::new()
    );
    let neutral = HueSaturationRangesParams::plain(ok(HueSaturationParams::new(
        0.0, 0.0, 0.0, true, 200.0, 40.0, 0.0,
    )));
    assert_eq!(
        report_of(Adjustment::from_hue_saturation(neutral)),
        Vec::<String>::new()
    );

    // The file does hold the range, and Aurora reads the layer back without it.
    let encoded = encode_adjustment(&Adjustment::from_hue_saturation(ranged(true)));
    let Ok(encoded) = encoded else { unreachable!() };
    assert!(encoded.rounding.colorize_ranges);
    let Adjustment::HueSaturation(plain) = encoded.readback else {
        unreachable!("Colorize reads back without ranges")
    };
    assert!(plain.colorize());
    // Yellows' record: edges 15 45 75 105 then hue 20, saturation -10.
    let tail = [0, 15, 0, 45, 0, 75, 0, 105, 0, 20, 255, 246];
    assert!(encoded.data.windows(tail.len()).any(|w| w == tail));
}

// ---------------------------------------------------------------------
// Vibrance, Exposure, Photo Filter, Black & White, Color Balance (0.241.0)
// ---------------------------------------------------------------------

use super::read_adjust_tests::{
    BLNC_CLIP, BLNC_FILL, BLWH, EXPA_FILL, EXPA_NEUTRAL, EXPA_RGB, PHFL, VIBA_FILL, VIBA_NEUTRAL,
};
use aurora_core::{
    BlackWhiteParams, ColorBalanceParams, ExposureParams, PhotoFilterParams, VibranceParams,
};

fn encoded(adjustment: &Adjustment) -> super::write_adjust::EncodedAdjustment {
    match encode_adjustment(adjustment) {
        Ok(e) => e,
        Err(reason) => unreachable!("{adjustment:?}: {reason}"),
    }
}

/// `data` is `hex` (Photoshop's block, padded as Photoshop pads it).
fn assert_photoshop_block(adjustment: &Adjustment, key: &[u8; 4], hex: &str) {
    let e = encoded(adjustment);
    assert_eq!(e.key, *key);
    let want = unhex(hex);
    assert_eq!(e.data, want, "{adjustment:?}");
    assert!(!e.rounding.rounded, "{adjustment:?}");
}

#[test]
fn the_writer_reproduces_photoshops_own_blocks_for_vibrance_exposure_and_color_balance() {
    let vibrance = |v, s| Adjustment::Vibrance(ok(VibranceParams::new(v, s)));
    assert_photoshop_block(&vibrance(-6.0, 2.0), b"vibA", VIBA_FILL);
    // Both sliders at 0: Photoshop's empty descriptor, no keys.
    assert_photoshop_block(&vibrance(0.0, 0.0), b"vibA", VIBA_NEUTRAL);
    let exposure = |e: u32, o: u32, g: u32| {
        Adjustment::Exposure(ok(ExposureParams::new(
            f32::from_bits(e),
            f32::from_bits(o),
            f32::from_bits(g),
        )))
    };
    assert_photoshop_block(
        &exposure(0xbec7_ae14, 0x3c89_a027, 0x3f68_f5c3),
        b"expA",
        EXPA_FILL,
    );
    assert_photoshop_block(&exposure(0, 0, 0x3f80_0000), b"expA", EXPA_NEUTRAL);
    for (hex, bits) in EXPA_RGB.iter().zip([
        (0x4001_eb85, 0x3d9e_b852, 0x3fc2_8f5c),
        (0xbd23_d70a, 0x3ed4_2c3d, 0x3ee1_47ae),
        (0x3fa0_0000, 0, 0x3fc2_8f5c),
        (0xbfe8_f5c3, 0x3df6_5fd9, 0x3f80_0000),
    ]) {
        assert_photoshop_block(&exposure(bits.0, bits.1, bits.2), b"expA", hex);
    }
    let balance = |v: [f32; 9], lum: bool| {
        let r = [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]];
        Adjustment::ColorBalance(ok(ColorBalanceParams::new(r, lum)))
    };
    assert_photoshop_block(
        &balance([-4.0, 2.0, -5.0, 10.0, 4.0, -9.0, 1.0, -9.0, -3.0], true),
        b"blnc",
        BLNC_FILL,
    );
    assert_photoshop_block(
        &balance([0.0, 0.0, 0.0, 28.0, 100.0, 53.0, 0.0, 0.0, 0.0], true),
        b"blnc",
        BLNC_CLIP,
    );
}

/// The Black & White block is Photoshop's byte for byte but for the three
/// tint-colour doubles, which come from the tint hue and saturation.
#[test]
fn the_black_and_white_block_is_photoshops_but_for_the_tint_colour() {
    let params = ok(BlackWhiteParams::new(
        aurora_core::BLACK_WHITE_DEFAULTS,
        false,
        200.0,
        60.0,
    ));
    let e = encoded(&Adjustment::BlackWhite(params));
    assert_eq!(e.key, *b"blwh");
    assert!(!e.rounding.rounded);
    let mut want = unhex(BLWH);
    let at = want
        .windows(4)
        .position(|w| w == b"RGBC")
        .unwrap_or_else(|| unreachable!("RGBC"));
    let rgb = super::read_adjust::tint_rgb(200.0, 60.0);
    for (i, v) in rgb.iter().enumerate() {
        // After "RGBC" and the u32 count: per item a zero key length, the
        // four-character key, "doub", then the value.
        let value = at + 4 + 4 + i * 20 + 12;
        want.splice(value..value + 8, v.to_be_bytes());
    }
    assert_eq!(e.data, want);
    // Tint on only flips the bool byte (right after "useTint" + "bool").
    let on = encoded(&Adjustment::BlackWhite(params.with_tint(true)));
    let flag = want
        .windows(11)
        .position(|w| w == b"useTintbool")
        .unwrap_or_else(|| unreachable!("useTint"))
        + 11;
    let mut want_on = want;
    want_on[flag] = 1;
    assert_eq!(on.data, want_on);
    assert_eq!(on.data.len() % 4, 0);
}

/// The Photo Filter block has Photoshop's structure; the Lab components
/// are the colour's own, in-gamut (Photoshop's preset stores an
/// out-of-gamut Lab for the same sRGB colour, so the bytes differ there by
/// design).
#[test]
fn the_photo_filter_block_has_photoshops_layout_and_the_colours_own_lab() {
    let warming = ok(PhotoFilterParams::new(
        [236.0 / 255.0, 138.0 / 255.0, 0.0],
        25.0,
        true,
    ));
    let e = encoded(&Adjustment::PhotoFilter(warming));
    let photoshop = unhex(PHFL);
    assert_eq!(e.key, *b"phfl");
    assert_eq!(e.data.len(), photoshop.len());
    // Version 2, colour space 7 (Lab), the zero fourth component, density
    // 25 (u32), preserve luminosity, padding: all Photoshop's.
    assert_eq!(e.data.get(..4), photoshop.get(..4));
    assert_eq!(e.data.get(10..), photoshop.get(10..));
    let comp = |i: usize| {
        e.data
            .get(4 + 2 * i..6 + 2 * i)
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .map_or(0, i16::from_be_bytes)
    };
    // sRGB (236, 138, 0) in D50 Lab: L 67.19, a 33.14, b 72.79.
    assert!((f64::from(comp(0)) - 6719.0).abs() <= 2.0, "{}", comp(0));
    assert!((f64::from(comp(1)) - 3314.0).abs() <= 3.0, "{}", comp(1));
    assert!((f64::from(comp(2)) - 7279.0).abs() <= 3.0, "{}", comp(2));
    // And it reads back as the same colour within a third of a level.
    assert!(!e.rounding.rounded);
    let Adjustment::PhotoFilter(back) = e.readback else {
        unreachable!("photo filter")
    };
    for (a, b) in back.colour().iter().zip(warming.colour()) {
        assert!((a - b).abs() < 1.0 / 765.0, "{a} {b}");
    }
    assert_eq!((back.density(), back.preserve_luminosity()), (25.0, true));
}

/// What Photoshop wrote reads, is written, and reads back identically, for
/// every real block of the five kinds.
#[test]
fn photoshops_blocks_survive_read_write_read() {
    for (key, hex) in [
        (b"vibA", VIBA_FILL),
        (b"vibA", VIBA_NEUTRAL),
        (b"expA", EXPA_FILL),
        (b"expA", EXPA_RGB[0]),
        (b"expA", EXPA_RGB[1]),
        (b"phfl", PHFL),
        (b"blwh", BLWH),
        (b"blnc", BLNC_FILL),
        (b"blnc", BLNC_CLIP),
    ] {
        let Some(parsed) = super::read_adjust::parse_block(*key, &unhex(hex)) else {
            unreachable!()
        };
        let Ok(first) = super::read_adjust::adjustment(&parsed, false) else {
            unreachable!("{key:?} reads")
        };
        let e = encoded(&first);
        // The colour of a Photo Filter and the tint of a Black & White
        // come back within their own tolerance; everything else exactly.
        match (&first, &e.readback) {
            (Adjustment::PhotoFilter(a), Adjustment::PhotoFilter(b)) => {
                for (x, y) in a.colour().iter().zip(b.colour()) {
                    assert!((x - y).abs() < 1.0 / 765.0);
                }
                assert_eq!(
                    (a.density(), a.preserve_luminosity()),
                    (b.density(), b.preserve_luminosity())
                );
            }
            (Adjustment::BlackWhite(a), Adjustment::BlackWhite(b)) => {
                assert_eq!((a.weights(), a.tint()), (b.weights(), b.tint()));
                assert!((a.tint_saturation() - b.tint_saturation()).abs() < 0.01);
                assert!((a.tint_hue() - b.tint_hue()).abs() < 0.01);
            }
            (a, b) => assert_eq!(a, b, "{key:?}"),
        }
        assert_eq!(e.data.len() % 4, 0, "{key:?}");
    }
}

#[test]
fn extreme_values_of_the_five_kinds_read_back_as_themselves() {
    let originals = [
        Adjustment::Vibrance(ok(VibranceParams::new(-100.0, 100.0))),
        Adjustment::Vibrance(ok(VibranceParams::new(100.0, -100.0))),
        Adjustment::Exposure(ok(ExposureParams::new(-20.0, -0.5, 0.1))),
        Adjustment::Exposure(ok(ExposureParams::new(20.0, 0.5, 9.99))),
        Adjustment::ColorBalance(ok(ColorBalanceParams::new(
            [[-100.0, 100.0, -100.0], [100.0; 3], [-100.0; 3]],
            false,
        ))),
        Adjustment::BlackWhite(ok(BlackWhiteParams::new(
            [-200.0, 300.0, -200.0, 300.0, 0.0, 1.0],
            true,
            0.0,
            100.0,
        ))),
        Adjustment::PhotoFilter(ok(PhotoFilterParams::new([0.0; 3], 1.0, false))),
        Adjustment::PhotoFilter(ok(PhotoFilterParams::new([1.0; 3], 100.0, true))),
        Adjustment::PhotoFilter(ok(PhotoFilterParams::new([0.0, 0.0, 1.0], 50.0, true))),
    ];
    for original in originals {
        let e = encoded(&original);
        assert_eq!(e.data.len() % 4, 0, "{original:?}");
        assert!(!e.rounding.rounded, "{original:?}");
        match (&original, &e.readback) {
            (Adjustment::PhotoFilter(a), Adjustment::PhotoFilter(b)) => {
                for (x, y) in a.colour().iter().zip(b.colour()) {
                    assert!((x - y).abs() < 1.0 / 765.0, "{a:?} {b:?}");
                }
            }
            (Adjustment::BlackWhite(a), Adjustment::BlackWhite(b)) => {
                assert_eq!((a.weights(), a.tint()), (b.weights(), b.tint()));
                assert!((a.tint_saturation() - b.tint_saturation()).abs() < 0.01);
            }
            (a, b) => assert_eq!(a, b),
        }
    }
}

#[test]
fn values_finer_than_the_file_are_rounded_and_itemised() {
    let mut doc = Doc::new((8, 8));
    doc.layer("Background", rect(0, 0, 8, 8), None);
    let add = |doc: &mut Doc, name: &str, a: Adjustment| {
        ok(doc.tree.add_adjustment_layer_at(name, a, None, 0));
    };
    add(
        &mut doc,
        "V",
        Adjustment::Vibrance(ok(VibranceParams::new(10.4, -20.6))),
    );
    add(
        &mut doc,
        "E",
        Adjustment::Exposure(ok(ExposureParams::new(1.234_567, 0.012_345, 1.789_01))),
    );
    add(
        &mut doc,
        "P",
        Adjustment::PhotoFilter(ok(PhotoFilterParams::new([0.9, 0.5, 0.1], 33.4, true))),
    );
    add(
        &mut doc,
        "B",
        Adjustment::BlackWhite(ok(BlackWhiteParams::new(
            [40.4, 60.0, 40.0, 60.0, 20.0, 80.6],
            false,
            35.0,
            25.0,
        ))),
    );
    add(
        &mut doc,
        "C",
        Adjustment::ColorBalance(ok(ColorBalanceParams::new(
            [[1.5, 0.0, 0.0], [0.0; 3], [0.0; 3]],
            true,
        ))),
    );
    let report = ok(psd_save_preflight(
        &doc.tree,
        (8, 8),
        PsdSaveOptions::default(),
    ));
    let text = report.items.join("\n");
    assert!(
        text.contains("1 Vibrance layer's fractional values will be rounded"),
        "{text}"
    );
    assert!(
        text.contains("1 Photo Filter layer's density will be rounded"),
        "{text}"
    );
    assert!(
        text.contains("1 Black & White layer's fractional slider values"),
        "{text}"
    );
    assert!(
        text.contains("1 Color Balance layer's fractional values"),
        "{text}"
    );
    // Exposure is f32 in the file: nothing to report for it.
    assert!(!text.contains("Exposure"), "{text}");
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.report, report);
    let back = read_back(&bytes);
    let Some(Adjustment::Vibrance(v)) = adjustment_named(&back.layers, "V") else {
        unreachable!()
    };
    assert_eq!((v.vibrance(), v.saturation()), (10.0, -21.0));
    let Some(Adjustment::PhotoFilter(p)) = adjustment_named(&back.layers, "P") else {
        unreachable!()
    };
    assert_eq!(p.density(), 33.0);
    let Some(Adjustment::BlackWhite(b)) = adjustment_named(&back.layers, "B") else {
        unreachable!()
    };
    assert_eq!(b.weights(), [40.0, 60.0, 40.0, 60.0, 20.0, 81.0]);
    let Some(Adjustment::ColorBalance(c)) = adjustment_named(&back.layers, "C") else {
        unreachable!()
    };
    assert_eq!(
        c.ranges()[0],
        [2.0, 0.0, 0.0],
        "1.5 rounds half away from zero"
    );
    let Some(Adjustment::Exposure(e)) = adjustment_named(&back.layers, "E") else {
        unreachable!()
    };
    assert_eq!(e.exposure().to_bits(), 1.234_567_f32.to_bits());
}

/// Exact values report nothing (a lossless save stays lossless).
#[test]
fn exact_values_of_the_five_kinds_are_a_lossless_save() {
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    for (name, a) in [
        (
            "V",
            Adjustment::Vibrance(ok(VibranceParams::new(10.0, -20.0))),
        ),
        ("E", Adjustment::Exposure(ExposureParams::identity())),
        (
            "P",
            Adjustment::PhotoFilter(PhotoFilterParams::warming_85()),
        ),
        ("B", Adjustment::BlackWhite(BlackWhiteParams::default())),
        (
            "C",
            Adjustment::ColorBalance(ColorBalanceParams::identity()),
        ),
    ] {
        ok(tree.add_adjustment_layer_at(name, a, None, 0));
    }
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    // Only Black & White is itemised: its tint colour is stored as a colour
    // and its preset name is cleared.
    assert_eq!(report.items.len(), 1, "{:?}", report.items);
    assert!(
        report
            .items
            .iter()
            .all(|i| i.contains("Black & White layer's tint colour"))
    );
}

fn five_doc() -> Doc {
    let mut doc = Doc::new((16, 16));
    doc.layer("Background", rect(0, 0, 16, 16), None);
    let vibrance = ok(doc.tree.add_adjustment_layer_at(
        "Vibrance A",
        Adjustment::Vibrance(ok(VibranceParams::new(-6.0, 2.0))),
        None,
        0,
    ));
    ok(doc.tree.set_opacity(vibrance, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(vibrance, BlendMode::Multiply));
    doc.mask(vibrance, rect(2, 2, 10, 10), (0, 0, 10, 10), |x, y| {
        f32::from(u16::try_from((x * 20 + y) % 255).unwrap_or(0)) / 255.0
    });
    let group = doc.group("Group", None);
    doc.layer_in("Base", rect(0, 0, 16, 16), None, Some(group));
    let clipped = ok(doc.tree.add_adjustment_layer_at(
        "Balance clipped",
        Adjustment::ColorBalance(ok(ColorBalanceParams::new(
            [[-4.0, 2.0, -5.0], [10.0, 4.0, -9.0], [1.0, -9.0, -3.0]],
            true,
        ))),
        Some(group),
        0,
    ));
    ok(doc.tree.set_clipped(clipped, true));
    ok(doc.tree.add_adjustment_layer_at(
        "Exposure in group",
        Adjustment::Exposure(ok(ExposureParams::new(2.03, 0.0775, 1.52))),
        Some(group),
        0,
    ));
    ok(doc.tree.add_adjustment_layer_at(
        "Filter",
        Adjustment::PhotoFilter(PhotoFilterParams::warming_85()),
        None,
        0,
    ));
    let bw = ok(doc.tree.add_adjustment_layer_at(
        "Mono",
        Adjustment::BlackWhite(ok(BlackWhiteParams::new(
            [10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
            true,
            200.0,
            40.0,
        ))),
        None,
        0,
    ));
    ok(doc.tree.set_visible(bw, false));
    doc
}

#[test]
fn the_five_kinds_round_trip_through_write_verify_and_read() {
    let mut doc = five_doc();
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("five.psd");
    let outcome = ok(save(&mut doc, &target, false));
    // Only the Black & White layer is itemised (tint colour, preset name).
    assert_eq!(outcome.report.items.len(), 1, "{:?}", outcome.report);
    assert!(outcome.report.items[0].contains("Black & White layer's tint colour"));
    let back = read_back(&ok(std::fs::read(&target)));
    assert!(back.report.items.is_empty(), "{:?}", back.report.items);
    assert_eq!(
        shape(&back.layers, back.layers.roots()),
        shape(&doc.tree, doc.tree.roots())
    );
    for name in [
        "Vibrance A",
        "Balance clipped",
        "Exposure in group",
        "Filter",
        "Mono",
    ] {
        let (want, got) = (id_named(&doc.tree, name), id_named(&back.layers, name));
        let (a, b) = (
            adjustment_named(&doc.tree, name),
            adjustment_named(&back.layers, name),
        );
        match (a, b) {
            (Some(Adjustment::PhotoFilter(a)), Some(Adjustment::PhotoFilter(b))) => {
                for (x, y) in a.colour().iter().zip(b.colour()) {
                    assert!((x - y).abs() < 1.0 / 765.0);
                }
            }
            (Some(Adjustment::BlackWhite(a)), Some(Adjustment::BlackWhite(b))) => {
                assert_eq!((a.weights(), a.tint()), (b.weights(), b.tint()));
                assert!((a.tint_hue() - b.tint_hue()).abs() < 0.01);
                assert!((a.tint_saturation() - b.tint_saturation()).abs() < 0.01);
            }
            (a, b) => assert_eq!(a, b, "{name}"),
        }
        assert_eq!(back.layers.opacity(got), doc.tree.opacity(want), "{name}");
        assert_eq!(
            back.layers.blend_mode(got),
            doc.tree.blend_mode(want),
            "{name}"
        );
        assert_eq!(back.layers.visible(got), doc.tree.visible(want), "{name}");
        assert_eq!(
            back.layers.clip_base(got).is_some(),
            doc.tree.clip_base(want).is_some(),
            "{name}"
        );
        assert_eq!(
            back.layers.mask(got).map(|m| m.bounds),
            doc.tree.mask(want).map(|m| m.bounds),
            "{name}"
        );
    }
}

/// Each record has an empty rectangle and the key's block is the first
/// tagged block, before `luni`.
#[test]
fn the_five_records_have_empty_rectangles_and_their_block_first() {
    let mut doc = five_doc();
    let (bytes, _) = ok(doc.write());
    let recs = records(&bytes);
    for name in [
        "Vibrance A",
        "Balance clipped",
        "Exposure in group",
        "Filter",
        "Mono",
    ] {
        let rec = recs
            .iter()
            .find(|r| r.4 == name)
            .unwrap_or_else(|| unreachable!("{name}"));
        assert_eq!(rec.1, [0; 4], "{name}");
    }
    for key in [&b"vibA"[..], b"expA", b"phfl", b"blwh", b"blnc"] {
        let mut needle = b"8BIM".to_vec();
        needle.extend_from_slice(key);
        let Some(at) = bytes.windows(8).position(|w| w == needle.as_slice()) else {
            unreachable!("{key:?} block")
        };
        let len = be32_at(&bytes, at + 8);
        assert_eq!(
            bytes.get(at + 12 + len..at + 12 + len + 8),
            Some(&b"8BIMluni"[..]),
            "luni follows the {key:?} block directly"
        );
    }
}

/// The verifier compares the five kinds' bytes and read-back too.
#[test]
fn the_verifier_catches_a_wrong_block_or_readback_for_each_new_kind() {
    let mut doc = five_doc();
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("five.psd");
    let (bytes, outcome) = ok(doc.write());
    ok(std::fs::write(&path, &bytes));
    ok(super::write::verify_psd_file(&path, &outcome.expected));
    for key in [b"vibA", b"expA", b"phfl", b"blwh", b"blnc"] {
        let at = |e: &super::write::PsdExpectation| {
            e.layers
                .iter()
                .position(|l| l.adjustment.as_ref().is_some_and(|a| a.key == *key))
        };
        let Some(i) = at(&outcome.expected) else {
            unreachable!("{key:?}")
        };
        let mut wrong_bytes = outcome.expected.clone();
        if let Some(a) = wrong_bytes
            .layers
            .get_mut(i)
            .and_then(|l| l.adjustment.as_mut())
            && let Some(byte) = a.data.get_mut(4)
        {
            *byte ^= 1;
        }
        let err = super::write::verify_psd_file(&path, &wrong_bytes);
        assert!(
            matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment blocks")),
            "{key:?}: {err:?}"
        );
        let mut wrong_readback = outcome.expected.clone();
        if let Some(a) = wrong_readback
            .layers
            .get_mut(i)
            .and_then(|l| l.adjustment.as_mut())
        {
            a.readback = Adjustment::Exposure(ExposureParams::identity());
            if *key == *b"expA" {
                a.readback = Adjustment::Vibrance(VibranceParams::identity());
            }
        }
        let err = super::write::verify_psd_file(&path, &wrong_readback);
        assert!(
            matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment")),
            "{key:?}: {err:?}"
        );
    }
}

/// `adjustments5.psd` / `adjustments5.json` for `scripts/verify_psd_write.py
/// check_adjustments5` (psd-tools).
///
/// ```text
/// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
///     five_adjustments_fixture_for_psd_tools -- --ignored
/// ```
#[test]
#[ignore = "writes the fixture scripts/verify_psd_write.py reads"]
fn five_adjustments_fixture_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let mut doc = five_doc();
    let (bytes, _) = ok(doc.write());
    ok(std::fs::write(out.join("adjustments5.psd"), &bytes));
    let json = "{\"vibrance\": [-6, 2], \"exposure\": [2.03, 0.0775, 1.52], \
        \"balance\": [[-4, 2, -5], [10, 4, -9], [1, -9, -3], 1], \
        \"bw\": [10, 20, 30, 40, 50, 60], \"bw_tint\": true, \"density\": 25, \"luminosity\": 1}\n";
    ok(std::fs::write(out.join("adjustments5.json"), json));
}

/// Every Black & White layer written is itemised (its tint colour becomes a
/// colour made from hue and saturation, its preset name is cleared) so the
/// "Save With Changes?" dialog names it; no Black & White layer, no line.
#[test]
fn black_and_white_layers_are_itemised_in_the_save_report() {
    let line = |tree: &LayerTree| {
        ok(psd_save_preflight(tree, (4, 4), PsdSaveOptions::default()))
            .items
            .into_iter()
            .filter(|i| i.contains("Black & White"))
            .collect::<Vec<_>>()
    };
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "Hue",
        Adjustment::HueSaturation(HueSaturationParams::identity()),
        None,
        0,
    ));
    assert!(line(&tree).is_empty(), "no Black & White layer, no line");
    ok(tree.add_adjustment_layer_at(
        "Mono",
        Adjustment::BlackWhite(BlackWhiteParams::default()),
        None,
        0,
    ));
    let one = line(&tree);
    assert_eq!(one.len(), 1, "{one:?}");
    assert!(
        one[0].starts_with("1 Black & White layer's tint colour")
            && one[0].contains("preset name will be cleared"),
        "{one:?}"
    );
    ok(tree.add_adjustment_layer_at(
        "Mono 2",
        Adjustment::BlackWhite(BlackWhiteParams::default()),
        None,
        0,
    ));
    let two = line(&tree);
    assert!(
        two[0].starts_with("2 Black & White layers' tint colour"),
        "{two:?}"
    );
}

// ---------------------------------------------------------------------
// Invert, Threshold, Posterize, Brightness/Contrast (0.244.0)
// ---------------------------------------------------------------------

mod four_kinds {
    use super::super::read_adjust_tests::{BRIT_LEGACY, CGED_BC1};
    use super::{
        Adjustment, Doc, IoError, LayerTree, encode_adjustment, id_named, ok, psd_save_preflight,
        read_back, records, rect, save, shape, unhex, PsdSaveOptions,
    };
    use aurora_core::{BrightnessContrastParams, PosterizeParams, ThresholdParams};

    pub(super) fn threshold(level: u8) -> Adjustment {
        Adjustment::Threshold(ok(ThresholdParams::new(level)))
    }

    pub(super) fn posterize(levels: u8) -> Adjustment {
        Adjustment::Posterize(ok(PosterizeParams::new(levels)))
    }

    pub(super) fn bc(b: i16, c: i16, legacy: bool) -> Adjustment {
        Adjustment::BrightnessContrast(ok(BrightnessContrastParams::new(b, c, legacy)))
    }

    fn encoded(a: &Adjustment) -> super::super::write_adjust::EncodedAdjustment {
        match encode_adjustment(a) {
            Ok(e) => e,
            Err(reason) => unreachable!("{a:?}: {reason}"),
        }
    }

    #[test]
    fn the_writer_reproduces_photoshops_own_blocks_byte_for_byte() {
        let e = encoded(&Adjustment::Invert);
        assert_eq!((e.key, e.data.len(), e.extra.is_none()), (*b"nvrt", 0, true));
        for (a, key, hex) in [
            (threshold(232), b"thrs", "00e80000"),
            (threshold(60), b"thrs", "003c0000"),
            (posterize(13), b"post", "000d0000"),
            (posterize(3), b"post", "00030000"),
            (bc(20, 57, true), b"brit", BRIT_LEGACY),
        ] {
            let e = encoded(&a);
            assert_eq!(e.key, *key, "{a:?}");
            assert_eq!(e.data, unhex(hex), "{a:?}");
            assert!(e.extra.is_none() && !e.rounding.rounded, "{a:?}");
            assert_eq!(e.readback, a);
        }
        // The default mode: an all-zero brit and Photoshop's CgEd, 132 bytes.
        let e = encoded(&bc(-95, -50, false));
        assert_eq!((e.key, e.data.clone()), (*b"brit", vec![0; 8]));
        let Some((key, data)) = e.extra else {
            unreachable!("CgEd")
        };
        assert_eq!(key, *b"CgEd");
        assert_eq!(data, unhex(CGED_BC1));
        assert_eq!(data.len(), 132);
    }

    #[test]
    fn every_value_of_every_kind_reads_back_as_itself() {
        let mut all = vec![Adjustment::Invert];
        all.extend((1..=255_u8).map(threshold));
        all.extend((2..=255_u8).map(posterize));
        for legacy in [false, true] {
            let ((b0, b1), (c0, c1)) = BrightnessContrastParams::ranges(legacy);
            for b in [b0, b0 + 1, -1, 0, 1, b1 - 1, b1] {
                for c in [c0, c0 + 1, -1, 0, 1, c1 - 1, c1] {
                    all.push(bc(b, c, legacy));
                }
            }
        }
        for a in all {
            let e = encoded(&a);
            assert_eq!(e.readback, a, "{a:?}");
            assert!(!e.rounding.rounded && !e.rounding.merged_points, "{a:?}");
            assert_eq!(e.data.len() % 4, 0);
        }
    }

    #[test]
    fn these_kinds_are_a_lossless_save_with_no_report_line() {
        let mut tree = LayerTree::new();
        ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
        for (name, a) in [
            ("I", Adjustment::Invert),
            ("T", threshold(77)),
            ("P", posterize(6)),
            ("B", bc(-31, 47, false)),
            ("L", bc(20, 57, true)),
        ] {
            ok(tree.add_adjustment_layer_at(name, a, None, 0));
        }
        let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
        assert!(report.items.is_empty(), "{:?}", report.items);
    }

    pub(super) fn four_doc() -> Doc {
        let mut doc = Doc::new((16, 16));
        doc.layer("Background", rect(0, 0, 16, 16), None);
        let invert = ok(doc.tree.add_adjustment_layer_at("Invert A", Adjustment::Invert, None, 0));
        ok(doc.tree.set_opacity(invert, 128.0 / 255.0));
        ok(doc.tree.set_blend_mode(invert, aurora_doc::BlendMode::Multiply));
        doc.mask(invert, rect(2, 2, 10, 10), (0, 0, 10, 10), |x, y| {
            f32::from(u16::try_from((x * 20 + y) % 255).unwrap_or(0)) / 255.0
        });
        let group = doc.group("Group", None);
        doc.layer_in("Base", rect(0, 0, 16, 16), None, Some(group));
        let clipped =
            ok(doc.tree.add_adjustment_layer_at("Posterize clipped", posterize(7), Some(group), 0));
        ok(doc.tree.set_clipped(clipped, true));
        ok(doc.tree.add_adjustment_layer_at("Threshold in group", threshold(172), Some(group), 0));
        ok(doc.tree.add_adjustment_layer_at("Bright", bc(23, 70, false), None, 0));
        let legacy = ok(doc.tree.add_adjustment_layer_at("Legacy", bc(20, 57, true), None, 0));
        ok(doc.tree.set_visible(legacy, false));
        doc
    }

    const NAMES: [&str; 5] = [
        "Invert A",
        "Posterize clipped",
        "Threshold in group",
        "Bright",
        "Legacy",
    ];

    #[test]
    fn the_four_kinds_round_trip_through_write_verify_and_read() {
        let mut doc = four_doc();
        let dir = ok(tempfile::tempdir());
        let target = dir.path().join("four.psd");
        let outcome = ok(save(&mut doc, &target, false));
        assert!(outcome.report.items.is_empty(), "{:?}", outcome.report);
        let back = read_back(&ok(std::fs::read(&target)));
        // Only the default-mode Brightness/Contrast layer is noted on reopen.
        assert_eq!(back.report.items.len(), 1, "{:?}", back.report.items);
        assert!(back.report.items[0].contains("approximation"));
        assert_eq!(
            shape(&back.layers, back.layers.roots()),
            shape(&doc.tree, doc.tree.roots())
        );
        for name in NAMES {
            let (want, got) = (id_named(&doc.tree, name), id_named(&back.layers, name));
            assert_eq!(back.layers.adjustment(got), doc.tree.adjustment(want), "{name}");
            assert_eq!(back.layers.opacity(got), doc.tree.opacity(want), "{name}");
            assert_eq!(back.layers.blend_mode(got), doc.tree.blend_mode(want), "{name}");
            assert_eq!(back.layers.visible(got), doc.tree.visible(want), "{name}");
            assert_eq!(
                back.layers.clip_base(got).is_some(),
                doc.tree.clip_base(want).is_some(),
                "{name}"
            );
            assert_eq!(
                back.layers.mask(got).map(|m| m.bounds),
                doc.tree.mask(want).map(|m| m.bounds),
                "{name}"
            );
        }
    }

    #[test]
    fn the_records_have_empty_rectangles_and_the_default_mode_writes_brit_then_cged() {
        let mut doc = four_doc();
        let (bytes, _) = ok(doc.write());
        let recs = records(&bytes);
        for name in NAMES {
            let rec = recs
                .iter()
                .find(|r| r.4 == name)
                .unwrap_or_else(|| unreachable!("{name}"));
            assert_eq!(rec.1, [0; 4], "{name}");
        }
        let at = |needle: &[u8]| bytes.windows(needle.len()).position(|w| w == needle);
        let Some(brit_default) = at(b"8BIMbrit\x00\x00\x00\x08\x00\x00\x00\x00\x00\x00\x00\x00") else {
            unreachable!("the default mode's zero brit")
        };
        assert_eq!(
            bytes.get(brit_default + 20..brit_default + 28),
            Some(&b"8BIMCgEd"[..]),
            "CgEd directly after brit"
        );
        assert!(at(b"8BIMbrit\x00\x00\x00\x08\x00\x14\x00\x39\x00\x7f\x00\x00").is_some());
        let cged_len = 132;
        assert_eq!(
            bytes.get(brit_default + 28 + 4 + cged_len..brit_default + 28 + 4 + cged_len + 8),
            Some(&b"8BIMluni"[..])
        );
        for key in [&b"nvrt"[..], b"thrs", b"post"] {
            let mut needle = b"8BIM".to_vec();
            needle.extend_from_slice(key);
            assert!(at(&needle).is_some(), "{key:?}");
        }
    }

    #[test]
    fn the_verifier_catches_a_wrong_block_a_wrong_extra_block_or_a_wrong_readback() {
        let mut doc = four_doc();
        let dir = ok(tempfile::tempdir());
        let path = dir.path().join("four.psd");
        let (bytes, outcome) = ok(doc.write());
        ok(std::fs::write(&path, &bytes));
        ok(super::super::write::verify_psd_file(&path, &outcome.expected));
        for key in [b"thrs", b"post", b"brit"] {
            let Some(i) = outcome.expected.layers.iter().position(|l| {
                l.adjustment
                    .as_ref()
                    .is_some_and(|a| a.key == *key && (key != b"brit" || a.extra.is_some()))
            }) else {
                unreachable!("{key:?}")
            };
            let mut wrong = outcome.expected.clone();
            if let Some(a) = wrong.layers.get_mut(i).and_then(|l| l.adjustment.as_mut())
                && let Some(byte) = a.data.get_mut(0)
            {
                *byte ^= 1;
            }
            let err = super::super::write::verify_psd_file(&path, &wrong);
            assert!(
                matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment blocks")),
                "{key:?}: {err:?}"
            );
            let mut wrong = outcome.expected.clone();
            if let Some(a) = wrong.layers.get_mut(i).and_then(|l| l.adjustment.as_mut()) {
                a.readback = Adjustment::Invert;
            }
            let err = super::super::write::verify_psd_file(&path, &wrong);
            assert!(
                matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment")),
                "{key:?}: {err:?}"
            );
        }
        // The extra CgEd block is checked too.
        let Some(i) = outcome
            .expected
            .layers
            .iter()
            .position(|l| l.adjustment.as_ref().is_some_and(|a| a.extra.is_some()))
        else {
            unreachable!("a default-mode layer")
        };
        let mut wrong = outcome.expected.clone();
        if let Some(a) = wrong.layers.get_mut(i).and_then(|l| l.adjustment.as_mut())
            && let Some((_, data)) = a.extra.as_mut()
            && let Some(byte) = data.get_mut(60)
        {
            *byte ^= 1;
        }
        let err = super::super::write::verify_psd_file(&path, &wrong);
        assert!(
            matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment blocks")),
            "{err:?}"
        );
        let mut missing = outcome.expected.clone();
        if let Some(a) = missing.layers.get_mut(i).and_then(|l| l.adjustment.as_mut()) {
            a.extra = None;
        }
        assert!(super::super::write::verify_psd_file(&path, &missing).is_err());
    }

    /// `adjustments4.psd` / `adjustments4.json` for `scripts/verify_psd_write.py
    /// check_adjustments4` (psd-tools).
    ///
    /// ```text
    /// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
    ///     four_adjustments_fixture_for_psd_tools -- --ignored
    /// ```
    #[test]
    #[ignore = "writes the fixture scripts/verify_psd_write.py reads"]
    fn four_adjustments_fixture_for_psd_tools() {
        let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
            unreachable!("set AURORA_PSD_FIXTURE_DIR");
        };
        let out = std::path::Path::new(&out);
        let mut doc = four_doc();
        let (bytes, _) = ok(doc.write());
        ok(std::fs::write(out.join("adjustments4.psd"), &bytes));
        let json = "{\"threshold\": 172, \"posterize\": 7, \"default\": [23, 70], \
            \"legacy\": [20, 57]}\n";
        ok(std::fs::write(out.join("adjustments4.json"), json));
    }
}
