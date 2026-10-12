//! Tests for the PSD writer's adjustment layers (0.205.0): the exact bytes
//! of `curv`, `levl` and `hue2` (against blocks Photoshop itself wrote,
//! taken from psd-tools' fixtures), the records around them, the round
//! trip through the writer's two verifier passes and Aurora's reader, and
//! the rounding the report itemises.

use aurora_core::{
    CurvePoint, CurvesParams, HueRange, HueSaturationParams, HueSaturationRangesParams,
    LevelsChannel, LevelsParams, ToneCurve,
};
use aurora_doc::{Adjustment, BlendMode, LayerId, LayerKind, LayerTree};

use super::write::{PsdSaveOptions, PsdSaveRefusal, psd_save_preflight};
use super::write_adjust::encode_adjustment;
use super::write_tests::{Doc, ok, read_back, records, rect, save, shape};

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
