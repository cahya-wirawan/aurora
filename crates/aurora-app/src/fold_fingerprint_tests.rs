//! A fingerprint of the CPU fold as `aurora-app` runs it (60 FPS round 7):
//! two root layers composited by [`crate::composite_roots_into_tile`]
//! (the fold, the straightening pass and everything around them, in this
//! crate's release binary with its own thin-LTO inlining), over every
//! `f16` bit pattern in each of the eight input samples, every blend mode
//! and three opacities; one FNV-1a hash per configuration, written to the
//! file named by `AURORA_APP_FOLD_FINGERPRINT`. Run in two builds and diff
//! the files to compare them bit for bit, NaN payloads included.

#![allow(clippy::cast_possible_truncation)]

use crate::occlusion_cull_tests::{ok, rect, store_with_budget};

fn fnv(samples: &[half::f16]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// One tile whose texel `i` is `base` with sample `position` (0..4)
/// replaced by the `f16` of bit pattern `i`, or `base` everywhere when
/// `position` is `None`.
fn tile(base: [f32; 4], position: Option<usize>) -> Vec<half::f16> {
    let mut out = Vec::with_capacity(aurora_tile::SAMPLES);
    for pattern in 0..=u16::MAX {
        let mut texel = base.map(half::f16::from_f32);
        if let Some(slot) = position.and_then(|p| texel.get_mut(p)) {
            *slot = half::f16::from_bits(pattern);
        }
        out.extend_from_slice(&texel);
    }
    out
}

#[test]
#[ignore = "writes a fingerprint file; run with --ignored"]
fn write_the_app_fold_fingerprint() {
    let Some(path) = std::env::var_os("AURORA_APP_FOLD_FINGERPRINT") else {
        return;
    };
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let full = rect(0, 0, aurora_tile::TILE, aurora_tile::TILE);
    let below = ok(layers.add_pixel_layer("below", full, None));
    let above = ok(layers.add_pixel_layer("above", full, None));
    let (Some(below_surface), Some(above_surface)) =
        (layers.surface_id(below), layers.surface_id(above))
    else {
        unreachable!("pixel layers");
    };
    let id = aurora_tile::TileId { x: 0, y: 0 };
    let modes: Vec<_> = aurora_doc::BlendMode::ALL
        .into_iter()
        .filter(|m| *m != aurora_doc::BlendMode::PassThrough)
        .collect();
    let mut lines = Vec::new();
    for backdrop in [[0.2, 0.35, 0.55, 1.0], [0.15, 0.05, 0.1, 0.5]] {
        for source in [[0.9, 0.6, 0.25, 0.75], [0.3, 0.8, 0.1, 1.0]] {
            for position in 0..8 {
                let (b, s) = if position < 4 {
                    (tile(backdrop, Some(position)), tile(source, None))
                } else {
                    (tile(backdrop, None), tile(source, Some(position - 4)))
                };
                ok(store.get_mut(below_surface, id))
                    .texels_mut()
                    .copy_from_slice(&b);
                ok(store.get_mut(above_surface, id))
                    .texels_mut()
                    .copy_from_slice(&s);
                for &mode in &modes {
                    ok(layers.set_blend_mode(above, mode));
                    for opacity in [1.0, 0.5, 0.0] {
                        ok(layers.set_opacity(above, opacity));
                        let mut budget = crate::CompositeBudget::for_pass(&layers);
                        let (out, _) = crate::composite_roots_into_tile(
                            &layers,
                            &mut store,
                            id,
                            (0, 0),
                            (0, 0),
                            &mut budget,
                        );
                        lines.push(format!(
                            "{backdrop:?} {source:?} {position} {mode:?} {opacity} {:016x}",
                            fnv(&out)
                        ));
                    }
                }
            }
        }
    }
    let _ = std::fs::write(path, lines.join("\n"));
}

/// The adjustment layers' CPU path (60 FPS round 9): a pixel layer whose
/// tile holds every `f16` bit pattern in one sample (the others at a
/// base), under a Curves or Levels layer (with and without per-channel
/// curves, a mask, several modes and opacities), composited by
/// [`crate::composite_roots_into_tile`]; one FNV-1a hash per
/// configuration, written to the file named by
/// `AURORA_APP_ADJUST_FINGERPRINT`. Run in two builds and diff the files.
#[test]
#[ignore = "writes a fingerprint file; run with --ignored"]
fn write_the_app_adjustment_fingerprint() {
    let Some(path) = std::env::var_os("AURORA_APP_ADJUST_FINGERPRINT") else {
        return;
    };
    let point = aurora_core::CurvePoint::new;
    let curve = |points: &[aurora_core::CurvePoint]| ok(aurora_core::ToneCurve::new(points));
    let s_curve = curve(&[
        point(0.0, 0.0),
        point(0.3, 0.2),
        point(0.7, 0.85),
        point(1.0, 1.0),
    ]);
    let lift = curve(&[point(0.0, 0.1), point(1.0, 0.9)]);
    let levels = |b: f32, w: f32, g: f32| ok(aurora_core::LevelsChannel::new(b, w, g, 0.02, 0.97));
    let adjustments = [
        aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
            composite: s_curve.clone(),
            ..aurora_core::CurvesParams::default()
        }),
        aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
            composite: s_curve,
            red: Some(lift.clone()),
            green: None,
            blue: Some(lift),
        }),
        aurora_doc::Adjustment::Levels(aurora_core::LevelsParams {
            master: levels(0.05, 0.9, 1.3),
            ..aurora_core::LevelsParams::default()
        }),
        aurora_doc::Adjustment::Levels(aurora_core::LevelsParams {
            master: levels(0.0, 1.0, 0.8),
            red: levels(0.1, 0.95, 1.0),
            green: levels(0.1, 0.95, 1.0),
            blue: levels(0.0, 0.8, 2.0),
        }),
    ];
    let modes = [
        aurora_doc::BlendMode::Normal,
        aurora_doc::BlendMode::Multiply,
        aurora_doc::BlendMode::Overlay,
        aurora_doc::BlendMode::Luminosity,
        aurora_doc::BlendMode::Dissolve,
    ];
    let mut lines = Vec::new();
    for (which, adjustment) in adjustments.iter().enumerate() {
        for masked in [false, true] {
            let (_d, mut store) = store_with_budget(64);
            let mut layers = aurora_doc::LayerTree::new();
            let full = rect(0, 0, aurora_tile::TILE, aurora_tile::TILE);
            let below = ok(layers.add_pixel_layer("below", full, None));
            let adjust = ok(layers.add_adjustment_layer_at("adjust", adjustment.clone(), None, 0));
            if masked {
                ok(layers.add_mask(adjust, full));
                let Some(mask) = layers.mask_surface_id(adjust) else {
                    unreachable!("a mask");
                };
                let _ = ok(aurora_doc::write_mask_coverage_region(
                    &mut store,
                    mask,
                    (0, 0),
                    aurora_tile::TILE,
                    aurora_tile::TILE,
                    |x, y| ((x * 7 + y * 3) % 256) as f32 / 255.0,
                ));
            }
            let Some(below_surface) = layers.surface_id(below) else {
                unreachable!("a pixel layer");
            };
            let id = aurora_tile::TileId { x: 0, y: 0 };
            for base in [[0.2, 0.35, 0.55, 1.0], [0.15, 0.05, 0.1, 0.5]] {
                for position in 0..4 {
                    ok(store.get_mut(below_surface, id))
                        .texels_mut()
                        .copy_from_slice(&tile(base, Some(position)));
                    for &mode in &modes {
                        ok(layers.set_blend_mode(adjust, mode));
                        for opacity in [1.0, 0.6] {
                            ok(layers.set_opacity(adjust, opacity));
                            let mut budget = crate::CompositeBudget::for_pass(&layers);
                            let (out, _) = crate::composite_roots_into_tile(
                                &layers,
                                &mut store,
                                id,
                                (0, 0),
                                (0, 0),
                                &mut budget,
                            );
                            lines.push(format!(
                                "{which} {masked} {base:?} {position} {mode:?} {opacity} {:016x}",
                                fnv(&out)
                            ));
                        }
                    }
                }
            }
        }
    }
    let _ = std::fs::write(path, lines.join("\n"));
}
