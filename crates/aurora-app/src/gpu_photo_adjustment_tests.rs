//! Vibrance, Exposure, Photo Filter, Black & White and Color Balance on the
//! GPU path (0.242.0): each GPU pass against the CPU compositor
//! (`apply_adjustment_layer`, the reference) on whole composite tiles, plus
//! the dispatch counters that prove the GPU path really ran and the
//! predicate's refusals. The harness (tile patterns, `Conditioning`, the
//! counters) is `gpu_adjustment_tests`' own, extended with the five new
//! `Pass` kinds, so these passes are held to the same tolerances as Levels
//! and Hue/Saturation (0.206.0).
//!
//! Verified on an RTX 3090 (Vulkan) only; every GPU test self-skips
//! without an adapter (`AURORA_REQUIRE_GPU=1` makes that a failure). No
//! test here pins a float hash or a timing.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::too_many_arguments,
    clippy::float_cmp
)]

use crate::gpu_adjustment_tests::{
    GPU_MODES, Pass, QUAD, add_adjustment, cpu_tile, edge_opaque, every_mode, gpu_matches_cpu,
    gpu_tile, hash, max_diff, mode_bound, ok, out_of_gamut, pattern_layer, ramp, ramp_translucent,
    random_opaque, random_translucent, run_cases, store, take_counts, tolerance,
    translucent_tolerance,
};
use crate::tests::real_gpu_context;
use aurora_doc::{Adjustment, BlendMode};

// -- Adjustment constructors ------------------------------------------------

fn vib(vibrance: f32, saturation: f32) -> Adjustment {
    Adjustment::Vibrance(ok(aurora_core::VibranceParams::new(vibrance, saturation)))
}

fn expo(exposure: f32, offset: f32, gamma: f32) -> Adjustment {
    Adjustment::Exposure(ok(aurora_core::ExposureParams::new(
        exposure, offset, gamma,
    )))
}

fn pf(colour: [u8; 3], density: f32, preserve: bool) -> Adjustment {
    Adjustment::PhotoFilter(ok(aurora_core::PhotoFilterParams::new(
        colour.map(|c| f32::from(c) / 255.0),
        density,
        preserve,
    )))
}

fn bw(weights: [f32; 6]) -> Adjustment {
    Adjustment::BlackWhite(ok(aurora_core::BlackWhiteParams::new(
        weights, false, 0.0, 0.0,
    )))
}

fn bw_tint(weights: [f32; 6], hue: f32, saturation: f32) -> Adjustment {
    Adjustment::BlackWhite(ok(aurora_core::BlackWhiteParams::new(
        weights, true, hue, saturation,
    )))
}

fn cbal(ranges: [[f32; 3]; 3], preserve: bool) -> Adjustment {
    Adjustment::ColorBalance(ok(aurora_core::ColorBalanceParams::new(ranges, preserve)))
}

/// Cases whose slope is unbounded (or large enough to carry an adapter's
/// one-`f16`-step `Cb` difference past [`translucent_tolerance`]) on a
/// translucent backdrop: held to the opaque tolerance only, like
/// `gpu_adjustment_tests`' `STEEP_*` lists. Exposure at +-5 stops or more
/// multiplies a shadow's step by 32 or more, a gamma at the ends of its
/// range has an unbounded slope near zero (`x^0.1`) or a slope of 10
/// (`x^10`), saturation +99/+100 divides by `1 - k`, and the Black & White
/// extremes carry slopes of 3 and -2.
pub(crate) const STEEP_CASES: [&str; 12] = [
    "vib sat +99",
    "vib sat +100",
    "vib extremes",
    "exp offset -0.5",
    "exp +5",
    "exp -5",
    "exp +20",
    "exp -20",
    "exp gamma 0.10",
    "exp gamma 9.99",
    "bw extremes high",
    "bw extremes low",
];

fn vibrance_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("vib +50", vib(50.0, 0.0)),
        ("vib +100", vib(100.0, 0.0)),
        ("vib -50", vib(-50.0, 0.0)),
        ("vib -100", vib(-100.0, 0.0)),
        ("vib sat +40", vib(0.0, 40.0)),
        ("vib sat -60", vib(0.0, -60.0)),
        ("vib sat -100", vib(0.0, -100.0)),
        ("vib sat +99", vib(0.0, 99.0)),
        ("vib sat +100", vib(0.0, 100.0)),
        ("vib mixed", vib(70.0, -20.0)),
        ("vib mixed 2", vib(-30.0, 40.0)),
        ("vib extremes", vib(100.0, 100.0)),
        ("vib extremes low", vib(-100.0, -100.0)),
    ]
}

fn exposure_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("exp +1", expo(1.0, 0.0, 1.0)),
        ("exp -1", expo(-1.0, 0.0, 1.0)),
        ("exp +0.5", expo(0.5, 0.0, 1.0)),
        ("exp -2", expo(-2.0, 0.0, 1.0)),
        ("exp offset +0.1", expo(0.0, 0.1, 1.0)),
        ("exp offset -0.1", expo(0.0, -0.1, 1.0)),
        ("exp offset +0.5", expo(0.0, 0.5, 1.0)),
        ("exp offset -0.5", expo(0.0, -0.5, 1.0)),
        ("exp gamma 2.2", expo(0.0, 0.0, 2.2)),
        ("exp gamma 0.45", expo(0.0, 0.0, 0.45)),
        ("exp all", expo(1.5, 0.05, 1.8)),
        ("exp +5", expo(5.0, 0.0, 1.0)),
        ("exp -5", expo(-5.0, 0.0, 1.0)),
        ("exp +20", expo(20.0, 0.0, 1.0)),
        ("exp -20", expo(-20.0, 0.0, 1.0)),
        ("exp gamma 0.10", expo(0.0, 0.0, 0.10)),
        ("exp gamma 9.99", expo(0.0, 0.0, 9.99)),
    ]
}

fn photo_filter_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("pf warming 85 preserve", pf([236, 138, 0], 50.0, true)),
        ("pf warming 85 plain", pf([236, 138, 0], 50.0, false)),
        (
            "pf cooling 80 d100 preserve",
            pf([0, 109, 255], 100.0, true),
        ),
        ("pf cooling 80 d100 plain", pf([0, 109, 255], 100.0, false)),
        ("pf density 1", pf([0, 0, 0], 1.0, false)),
        ("pf black d100", pf([0, 0, 0], 100.0, false)),
        ("pf black d60 preserve", pf([0, 0, 0], 60.0, true)),
        ("pf green d75 preserve", pf([40, 200, 60], 75.0, true)),
        ("pf one channel off", pf([255, 255, 200], 100.0, true)),
    ]
}

fn black_white_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("bw default", bw(aurora_core::BLACK_WHITE_DEFAULTS)),
        ("bw extremes high", bw([300.0; 6])),
        ("bw extremes low", bw([-200.0; 6])),
        ("bw mixed", bw([100.0, -50.0, 200.0, 0.0, -100.0, 150.0])),
        ("bw reds only", bw([300.0, 0.0, 0.0, 0.0, 0.0, 0.0])),
        ("bw blues only", bw([0.0, 0.0, 0.0, 0.0, 300.0, 0.0])),
        (
            "bw tint sepia",
            bw_tint(aurora_core::BLACK_WHITE_DEFAULTS, 35.0, 25.0),
        ),
        (
            "bw tint blue",
            bw_tint([60.0, 80.0, 40.0, 60.0, 20.0, 70.0], 210.0, 100.0),
        ),
        (
            "bw tint hue 0",
            bw_tint(aurora_core::BLACK_WHITE_DEFAULTS, 0.0, 60.0),
        ),
        (
            "bw tint hue 360",
            bw_tint(aurora_core::BLACK_WHITE_DEFAULTS, 360.0, 60.0),
        ),
    ]
}

fn color_balance_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        (
            "cb shadows",
            cbal([[40.0, 0.0, -20.0], [0.0; 3], [0.0; 3]], false),
        ),
        (
            "cb midtones",
            cbal([[0.0; 3], [-30.0, 20.0, 10.0], [0.0; 3]], false),
        ),
        (
            "cb highlights",
            cbal([[0.0; 3], [0.0; 3], [10.0, -25.0, 35.0]], false),
        ),
        (
            "cb all preserve",
            cbal(
                [[20.0, -10.0, 5.0], [-15.0, 30.0, -20.0], [10.0, 0.0, -40.0]],
                true,
            ),
        ),
        (
            "cb all plain",
            cbal(
                [[20.0, -10.0, 5.0], [-15.0, 30.0, -20.0], [10.0, 0.0, -40.0]],
                false,
            ),
        ),
        (
            "cb extremes preserve",
            cbal(
                [
                    [100.0, -100.0, 100.0],
                    [-100.0, 100.0, -100.0],
                    [100.0, 100.0, -100.0],
                ],
                true,
            ),
        ),
        (
            "cb extremes plain",
            cbal(
                [
                    [100.0, -100.0, 100.0],
                    [-100.0, 100.0, -100.0],
                    [100.0, 100.0, -100.0],
                ],
                false,
            ),
        ),
    ]
}

pub(crate) fn cases_of(pass: Pass) -> Vec<(&'static str, Adjustment)> {
    match pass {
        Pass::Vibrance => vibrance_cases(),
        Pass::Exposure => exposure_cases(),
        Pass::PhotoFilter => photo_filter_cases(),
        Pass::BlackWhite => black_white_cases(),
        Pass::ColorBalance => color_balance_cases(),
        Pass::Levels | Pass::HueSaturation => Vec::new(),
    }
}

const FIVE: [Pass; 5] = [
    Pass::Vibrance,
    Pass::Exposure,
    Pass::PhotoFilter,
    Pass::BlackWhite,
    Pass::ColorBalance,
];

/// A moderate adjustment of each kind, strong enough that every blend mode
/// visibly differs from `Normal` on the ramp.
fn moderate(pass: Pass) -> Adjustment {
    match pass {
        Pass::Vibrance => vib(80.0, 25.0),
        // Darkens the shadows and brightens the highlights, so Darken and
        // Lighten both differ from Normal.
        Pass::Exposure => expo(2.0, 0.0, 0.5),
        Pass::PhotoFilter => pf([236, 138, 0], 70.0, true),
        Pass::BlackWhite => bw_tint(aurora_core::BLACK_WHITE_DEFAULTS, 40.0, 30.0),
        Pass::ColorBalance => cbal(
            [
                [30.0, -10.0, 10.0],
                [-20.0, 25.0, -15.0],
                [10.0, 10.0, 40.0],
            ],
            true,
        ),
        Pass::Levels | Pass::HueSaturation => unreachable!("not one of the five"),
    }
}

/// The identity (neutral) adjustment of each kind, `None` for Black & White
/// (which is never one).
fn neutral(pass: Pass) -> Option<Adjustment> {
    match pass {
        Pass::Vibrance => Some(vib(0.0, 0.0)),
        Pass::Exposure => Some(expo(0.0, 0.0, 1.0)),
        Pass::PhotoFilter => Some(pf([255, 255, 255], 80.0, true)),
        Pass::BlackWhite => None,
        Pass::ColorBalance => Some(cbal([[0.0; 3]; 3], true)),
        Pass::Levels | Pass::HueSaturation => unreachable!("not one of the five"),
    }
}

// -- Patterns -----------------------------------------------------------------

/// Every combination of four levels per channel (ties, greys, primaries,
/// secondaries, exact hue-sector edges), opaque: the sort network, the HSL
/// sectors and the luma weights see every degenerate ordering.
fn tie_grid(x: u32, y: u32) -> [f32; 4] {
    let level = |v: u32| [0.0, 0.25, 0.5, 1.0][(v % 4) as usize];
    [level(x / 4), level(x), level(y / 4 + y), 1.0]
}

/// Every opaque 16-level grey-ish ramp including exact zero and values near
/// the sRGB break points (0.04045 and its linear sibling), which the
/// Exposure transfer branches on.
fn transfer_grid(x: u32, y: u32) -> [f32; 4] {
    const LEVELS: [f32; 16] = [
        0.0, 0.001, 0.01, 0.03, 0.0404, 0.0405, 0.05, 0.1, 0.2, 0.4, 0.6, 0.8, 0.95, 0.999, 1.0,
        0.5,
    ];
    let pick = |v: u32| LEVELS[(v % 16) as usize];
    [pick(x), pick(y), pick(x / 16 + y / 16 * 3), 1.0]
}

// -- Differential: opaque, per kind --------------------------------------------

fn opaque_differential(pass: Pass) {
    for (label, pattern) in [
        ("random", random_opaque as fn(u32, u32) -> [f32; 4]),
        ("edge", edge_opaque),
        ("ramp", ramp),
        ("tie grid", tie_grid),
        ("transfer grid", transfer_grid),
    ] {
        let worst = run_cases(
            pass,
            pattern,
            1.0,
            BlendMode::Normal,
            tolerance(),
            &format!("{pass:?} {label}"),
        );
        println!("{pass:?} opaque {label}: worst {worst:e}");
    }
    let worst = run_cases(
        pass,
        random_opaque,
        0.35,
        BlendMode::Normal,
        tolerance(),
        &format!("{pass:?} random @35%"),
    );
    println!("{pass:?} opaque random @35%: worst {worst:e}");
}

fn translucent_differential(pass: Pass) {
    let worst = run_cases(
        pass,
        random_translucent,
        1.0,
        BlendMode::Normal,
        translucent_tolerance(),
        &format!("{pass:?} random translucent"),
    );
    println!("{pass:?} translucent random: worst {worst:e}");
    let worst = run_cases(
        pass,
        ramp_translucent,
        0.6,
        BlendMode::Normal,
        translucent_tolerance(),
        &format!("{pass:?} ramp translucent @60%"),
    );
    println!("{pass:?} translucent ramp @60%: worst {worst:e}");
}

#[test]
fn gpu_vibrance_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::Vibrance);
}

#[test]
fn gpu_exposure_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::Exposure);
}

#[test]
fn gpu_photo_filter_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::PhotoFilter);
}

#[test]
fn gpu_black_white_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::BlackWhite);
}

#[test]
fn gpu_color_balance_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::ColorBalance);
}

#[test]
fn gpu_vibrance_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::Vibrance);
}

#[test]
fn gpu_exposure_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::Exposure);
}

#[test]
fn gpu_photo_filter_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::PhotoFilter);
}

#[test]
fn gpu_black_white_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::BlackWhite);
}

#[test]
fn gpu_color_balance_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::ColorBalance);
}

// -- Every blend mode, per kind -------------------------------------------------

#[test]
fn gpu_five_adjustments_match_the_cpu_at_every_gpu_blend_mode() {
    for pass in FIVE {
        every_mode(pass, moderate(pass), ramp);
    }
}

/// The same modes on random tiles at the stable modes (the ones that do not
/// amplify a one-step input difference), as the 0.206.0 test does.
#[test]
fn gpu_five_adjustments_match_the_cpu_at_the_stable_modes_on_random_tiles() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let stable = [
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::LinearDodge,
        BlendMode::Darken,
    ];
    for pass in FIVE {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, moderate(pass));
        ok(layers.set_opacity(id, 0.8));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        for mode in stable {
            ok(layers.set_blend_mode(id, mode));
            let _ = gpu_matches_cpu(
                &context,
                &mut compositor,
                &layers,
                &mut store,
                pass,
                1,
                tolerance(),
                &format!("random {pass:?} {mode:?}"),
            );
        }
    }
}

#[test]
fn gpu_five_adjustments_apply_opacity_times_fill() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in FIVE {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, moderate(pass));
        ok(layers.set_opacity(id, 0.8));
        ok(layers.set_fill_opacity(id, 0.5));
        assert_eq!(crate::content_opacity(&layers, id), Some(0.4));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let _ = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            1,
            tolerance(),
            &format!("{pass:?} opacity x fill"),
        );
    }
}

/// Out-of-gamut and negative backdrops (`straight_backdrop` is unclamped):
/// a non-identity adjustment clamps its input to [0, 1] on both paths; an
/// identity one under a blend mode passes the value through unchanged.
#[test]
fn gpu_five_adjustments_handle_out_of_gamut_backdrops_like_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in FIVE {
        let mut cases = vec![(moderate(pass), BlendMode::Normal)];
        if let Some(identity) = neutral(pass) {
            cases.push((identity, BlendMode::Multiply));
        }
        for (i, (adjustment, mode)) in cases.into_iter().enumerate() {
            let (_dir, mut store) = store();
            let mut layers = aurora_doc::LayerTree::new();
            let _ = pattern_layer(&mut layers, &mut store, out_of_gamut);
            let id = add_adjustment(&mut layers, adjustment);
            ok(layers.set_blend_mode(id, mode));
            let mut compositor = aurora_render::TileCompositor::new(context.device());
            assert!(crate::document_qualifies_for_gpu_compositing(&layers));
            let _ = take_counts();
            let Some(gpu) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
                unreachable!("{pass:?} case {i}: fell back");
            };
            assert_eq!(
                take_counts(),
                pass.expected(1),
                "{pass:?} case {i}: pass recorded"
            );
            let cpu = cpu_tile(&layers, &mut store);
            let mut worst = 0.0_f32;
            for (g, c) in gpu.iter().zip(&cpu) {
                let allowed = tolerance().max(c.abs() * 4.0 / 2048.0);
                assert!(
                    (g - c).abs() <= allowed,
                    "{pass:?} case {i} ({mode:?}): GPU {g} vs CPU {c}"
                );
                worst = worst.max((g - c).abs() / allowed);
            }
            println!("out of gamut {pass:?} case {i} ({mode:?}): worst fraction {worst}");
        }
    }
}

// -- Identity / neutral layers ----------------------------------------------------

/// Neutral layers: under `Normal` the CPU skips the layer and the GPU
/// records no pass (bit-identical to the layer-less composite); under any
/// other mode the pass runs and passes the value through. Black & White is
/// never neutral: its default sliders still run a pass under `Normal`.
#[test]
fn gpu_neutral_layers_record_no_pass_under_normal_and_one_otherwise() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in FIVE {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_translucent);
        let without = cpu_tile(&layers, &mut store);
        let adjustment = neutral(pass).unwrap_or_else(|| moderate(pass));
        let is_neutral = neutral(pass).is_some();
        let id = add_adjustment(&mut layers, adjustment);
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let (cpu, _) = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            u64::from(!is_neutral),
            if is_neutral {
                tolerance()
            } else {
                translucent_tolerance()
            },
            &format!("{pass:?} neutral @Normal"),
        );
        if is_neutral {
            assert_eq!(cpu, without, "the CPU skips a neutral layer under Normal");
        } else {
            assert!(
                max_diff(&cpu, &without) > 0.01,
                "Black & White is never an identity"
            );
        }
        ok(layers.set_blend_mode(id, BlendMode::Multiply));
        let _ = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            1,
            translucent_tolerance(),
            &format!("{pass:?} neutral @Multiply"),
        );
    }
}

// -- Mixed document, refusals, predicate -------------------------------------------

/// A document with a pixel layer, one adjustment of each of the five kinds
/// (and a blended pixel layer between them and one on top) composites on
/// the GPU with exactly five passes and equals the CPU composite.
#[test]
fn a_document_with_all_five_adjustments_composites_on_the_gpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, random_opaque);
    for (i, pass) in FIVE.into_iter().enumerate() {
        let id = add_adjustment(&mut layers, moderate(pass));
        ok(layers.set_opacity(id, 0.9 - i as f32 * 0.1));
        if i == 1 {
            ok(layers.set_blend_mode(id, BlendMode::Overlay));
        }
        if i == 2 {
            let mid = pattern_layer(&mut layers, &mut store, ramp_translucent);
            ok(layers.set_blend_mode(mid, BlendMode::Multiply));
            ok(layers.set_opacity(mid, 0.7));
        }
    }
    let top = pattern_layer(&mut layers, &mut store, edge_opaque);
    ok(layers.set_opacity(top, 0.25));
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    assert!(crate::document_qualifies_for_gpu_compositing(&layers));
    let _ = take_counts();
    let Some(gpu) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
        unreachable!("the tile fell back");
    };
    assert_eq!(
        take_counts(),
        [0, 0, 0, 1, 1, 1, 1, 1],
        "one pass of each of the five kinds"
    );
    let cpu = cpu_tile(&layers, &mut store);
    let worst = max_diff(&gpu, &cpu);
    println!("five adjustments in one document: worst |GPU - CPU| = {worst:e}");
    assert!(worst <= translucent_tolerance(), "worst {worst}");
}

/// Masks, `Dissolve`, `Exclusion` and a non-separable mode keep every pass
/// on the CPU, and a group refuses the whole document; a disabled mask is
/// ignored.
#[test]
fn unsupported_five_adjustment_layers_fall_back_to_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in FIVE {
        let adjustment = moderate(pass);
        for refusal in ["mask", "Dissolve", "Exclusion", "Luminosity"] {
            let (_dir, mut store) = store();
            let mut layers = aurora_doc::LayerTree::new();
            let _ = pattern_layer(&mut layers, &mut store, random_opaque);
            let id = add_adjustment(&mut layers, adjustment.clone());
            match refusal {
                "mask" => ok(layers.add_mask(id, QUAD)),
                "Dissolve" => ok(layers.set_blend_mode(id, BlendMode::Dissolve)),
                "Exclusion" => ok(layers.set_blend_mode(id, BlendMode::Exclusion)),
                _ => ok(layers.set_blend_mode(id, BlendMode::Luminosity)),
            }
            assert!(
                !crate::adjustment_layer_qualifies_for_gpu(&layers, id),
                "{pass:?} {refusal}: the layer does not qualify"
            );
            assert!(
                !crate::document_qualifies_for_gpu_compositing(&layers),
                "{pass:?} {refusal}: the document stays on the CPU"
            );
            let mut compositor = aurora_render::TileCompositor::new(context.device());
            let _ = take_counts();
            assert!(
                gpu_tile(&context, &mut compositor, &layers, &mut store).is_none(),
                "{pass:?} {refusal}: the tile must fall back, not drop the layer"
            );
            assert_eq!(take_counts(), [0; 8], "{pass:?} {refusal}");
        }
        // A disabled mask is ignored on both paths.
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, adjustment.clone());
        ok(layers.add_mask(id, QUAD));
        ok(layers.set_mask_enabled(id, false));
        assert!(
            crate::adjustment_layer_qualifies_for_gpu(&layers, id),
            "{pass:?}: a disabled mask does not refuse"
        );
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        // A group refuses, an adjustment inside it or not.
        let (_dir, mut store) = self::store();
        let mut grouped = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut grouped, &mut store, random_opaque);
        let group = ok(grouped.add_group("Group 1", None));
        let _ = ok(grouped.add_adjustment_layer_at("inner", adjustment, Some(group), 0));
        assert!(
            !crate::document_qualifies_for_gpu_compositing(&grouped),
            "{pass:?}: an adjustment in a group keeps the document on the CPU"
        );
    }
}

/// A visible layer of each kind qualifies at the root; a hidden one never
/// disqualifies (and records no pass).
#[test]
fn visible_five_adjustment_layers_qualify_and_hidden_ones_are_ignored() {
    let (_dir, mut store) = store();
    for pass in FIVE {
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, moderate(pass));
        assert!(crate::adjustment_layer_qualifies_for_gpu(&layers, id));
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        ok(layers.set_visible(id, false));
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
    }
}

/// A hidden layer of each kind records no pass on the GPU and the tile
/// equals the layer-less composite.
#[test]
fn hidden_five_adjustment_layers_record_no_pass() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in FIVE {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let without = cpu_tile(&layers, &mut store);
        let id = add_adjustment(&mut layers, moderate(pass));
        ok(layers.set_visible(id, false));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let (cpu, _) = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            0,
            tolerance(),
            &format!("{pass:?} hidden"),
        );
        assert_eq!(cpu, without);
    }
}

// -- Packing -------------------------------------------------------------------

#[test]
fn the_packed_parameter_lengths_agree_between_filters_and_render() {
    assert_eq!(
        aurora_filters::VIBRANCE_PACKED_LEN,
        aurora_render::VIBRANCE_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::EXPOSURE_PACKED_LEN,
        aurora_render::EXPOSURE_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::PHOTO_FILTER_PACKED_LEN,
        aurora_render::PHOTO_FILTER_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::BLACK_WHITE_PACKED_LEN,
        aurora_render::BLACK_WHITE_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::COLOR_BALANCE_PACKED_LEN,
        aurora_render::COLOR_BALANCE_GPU_PARAMS_LEN
    );
}

#[test]
fn vibrance_packed_for_gpu_is_the_settings_in_shader_order() {
    let p = ok(aurora_core::VibranceParams::new(40.0, -25.0));
    let packed = aurora_filters::VibranceOp::new(&p).packed_for_gpu();
    assert_eq!(packed, [0.0, 0.4, -0.25, 0.0]);
    let identity = aurora_filters::VibranceOp::new(&aurora_core::VibranceParams::identity());
    assert_eq!(identity.packed_for_gpu()[0], 1.0);
}

#[test]
fn exposure_packed_for_gpu_is_the_scale_offset_and_power_the_cpu_uses() {
    let p = ok(aurora_core::ExposureParams::new(1.5, -0.125, 2.0));
    let packed = aurora_filters::ExposureOp::new(&p).packed_for_gpu();
    assert_eq!(packed, [0.0, 1.5_f32.exp2(), -0.125, 0.5]);
    let identity = aurora_filters::ExposureOp::new(&aurora_core::ExposureParams::identity());
    assert_eq!(identity.packed_for_gpu(), [1.0, 1.0, 0.0, 1.0]);
}

#[test]
fn photo_filter_packed_for_gpu_is_the_flags_and_transmission() {
    let p = ok(aurora_core::PhotoFilterParams::new(
        [0.5, 0.25, 0.0],
        50.0,
        true,
    ));
    let packed = aurora_filters::PhotoFilterOp::new(&p).packed_for_gpu();
    // t = 1 + (c - 1) * 0.5.
    assert_eq!(packed, [0.0, 1.0, 0.0, 0.0, 0.75, 0.625, 0.5, 0.0]);
    let plain = ok(aurora_core::PhotoFilterParams::new(
        [0.5, 0.25, 0.0],
        100.0,
        false,
    ));
    let packed = aurora_filters::PhotoFilterOp::new(&plain).packed_for_gpu();
    assert_eq!(packed, [0.0, 0.0, 0.0, 0.0, 0.5, 0.25, 0.0, 0.0]);
    let white = ok(aurora_core::PhotoFilterParams::new([1.0; 3], 30.0, false));
    assert_eq!(
        aurora_filters::PhotoFilterOp::new(&white).packed_for_gpu()[0],
        1.0
    );
}

#[test]
fn black_white_packed_for_gpu_is_the_weights_and_tint() {
    let plain = ok(aurora_core::BlackWhiteParams::new(
        [40.0, 60.0, 80.0, -20.0, 20.0, 300.0],
        false,
        90.0,
        50.0,
    ));
    let packed = aurora_filters::BlackWhiteOp::new(&plain).packed_for_gpu();
    assert_eq!(
        packed,
        [0.4, 0.6, 0.8, -0.2, 0.2, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        "a tint that is off packs as zeros"
    );
    let tinted = ok(aurora_core::BlackWhiteParams::new(
        [40.0, 60.0, 80.0, -20.0, 20.0, 300.0],
        true,
        210.0,
        75.0,
    ));
    let packed = aurora_filters::BlackWhiteOp::new(&tinted).packed_for_gpu();
    assert_eq!(
        packed,
        [
            0.4, 0.6, 0.8, -0.2, 0.2, 3.0, 1.0, 210.0, 0.75, 0.0, 0.0, 0.0
        ]
    );
}

#[test]
fn color_balance_packed_for_gpu_is_the_flags_and_channel_shifts() {
    let p = ok(aurora_core::ColorBalanceParams::new(
        [[100.0, -50.0, 20.0], [0.0, 40.0, -100.0], [10.0, 0.0, 80.0]],
        true,
    ));
    let packed = aurora_filters::ColorBalanceOp::new(&p).packed_for_gpu();
    assert_eq!(
        packed,
        [
            0.0, 1.0, 0.0, 0.0, // flags
            0.5, -0.25, 0.1, 0.0, // shadows
            0.0, 0.2, -0.5, 0.0, // midtones
            0.05, 0.0, 0.4, 0.0, // highlights
        ]
    );
    let identity =
        aurora_filters::ColorBalanceOp::new(&aurora_core::ColorBalanceParams::identity());
    assert_eq!(identity.packed_for_gpu()[0], 1.0);
    let plain = ok(aurora_core::ColorBalanceParams::new([[10.0; 3]; 3], false));
    assert_eq!(
        aurora_filters::ColorBalanceOp::new(&plain).packed_for_gpu()[1],
        0.0
    );
}

// -- Exposure's domain edges ----------------------------------------------------

/// Exposure's `pow` guards (WGSL's `pow` is indeterminate for a
/// non-positive base): black and near-black with a negative offset (the
/// `x < 0` branch), a positive offset on black, an exact `x == 0` (stops
/// that cancel an offset), the sRGB break points, and white.
#[test]
fn gpu_exposure_matches_the_cpu_at_the_domain_edges() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (name, adjustment) in [
        ("negative offset", expo(0.0, -0.5, 1.0)),
        ("negative offset, gamma 0.45", expo(0.0, -0.1, 0.45)),
        ("negative offset, gamma 9.99", expo(2.0, -0.3, 9.99)),
        ("positive offset on black", expo(-20.0, 0.5, 1.0)),
        ("tiny offset", expo(0.0, 0.0001, 1.0)),
        ("exact zero", expo(1.0, -0.0, 1.0)),
        ("gamma 0.10 and offset", expo(0.0, 0.2, 0.10)),
    ] {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, transfer_grid);
        let _ = add_adjustment(&mut layers, adjustment);
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let (_, worst) = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            Pass::Exposure,
            1,
            tolerance(),
            &format!("exposure edge / {name}"),
        );
        assert!(worst.is_finite());
    }
}

// -- Cost -----------------------------------------------------------------------

/// CPU against GPU composite time for a 4096 x 4096 document (256 tiles):
/// one opaque pixel layer, then the document with each of the five
/// adjustments (moderate settings) on top, CPU path against GPU path.
/// `#[ignore]`d measurement; run with
/// `cargo test -p aurora-app --release -- --ignored --nocapture measure_five`.
#[test]
#[ignore = "measurement"]
fn measure_five_adjustments_composite_cpu_against_gpu_on_a_4096_document() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let side = 4096 / aurora_tile::TILE;
    let tiles: Vec<_> = (0..side)
        .flat_map(|y| (0..side).map(move |x| aurora_tile::TileId { x, y }))
        .collect();
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4096,
        height: 4096,
    };
    let base = ok(layers.add_pixel_layer("base", bounds, None));
    let Some(surface) = layers.surface_id(base) else {
        unreachable!("a pixel layer");
    };
    for &tile_id in &tiles {
        let tile = ok(store.get_mut(surface, tile_id));
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            let texel = (i / 4) as u32;
            let x = texel % aurora_tile::TILE + tile_id.x * aurora_tile::TILE;
            let y = texel / aurora_tile::TILE + tile_id.y * aurora_tile::TILE;
            let rgba = [hash(x, y, 1), hash(x, y, 2), hash(x, y, 3), 1.0];
            *sample = half::f16::from_f32(rgba[i % 4]);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    let mut report =
        |label: &str, layers: &aurora_doc::LayerTree, store: &mut aurora_tile::TileStore| {
            let mut cpu_ms = Vec::new();
            let mut gpu_ms = Vec::new();
            for _ in 0..5 {
                let start = std::time::Instant::now();
                for &tile in &tiles {
                    let mut budget = crate::CompositeBudget::for_pass(layers);
                    let _ = crate::composite_roots_into_tile(
                        layers,
                        store,
                        tile,
                        (0, 0),
                        (0, 0),
                        &mut budget,
                    );
                }
                cpu_ms.push(start.elapsed().as_secs_f64() * 1e3);
                let _ = take_counts();
                let start = std::time::Instant::now();
                let mut pending = Vec::new();
                for &tile in &tiles {
                    let mut budget = crate::CompositeBudget::for_pass(layers);
                    if let Some(p) = crate::begin_gpu_composite_tile(
                        &context,
                        &mut compositor,
                        layers,
                        store,
                        tile,
                        (0, 0),
                        (0, 0),
                        &mut budget,
                    ) {
                        pending.push(p);
                    }
                }
                let _ = context.device().poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                let done = pending
                    .into_iter()
                    .filter_map(crate::finish_tile_readback)
                    .count();
                gpu_ms.push(start.elapsed().as_secs_f64() * 1e3);
                assert_eq!(done, tiles.len(), "{label}: every tile on the GPU");
            }
            println!("4096^2 {label}: CPU ms per pass {cpu_ms:.1?}");
            println!("4096^2 {label}: GPU ms per pass {gpu_ms:.1?}");
        };
    report("pixel layer only", &layers, &mut store);
    for pass in FIVE {
        let id = add_adjustment(&mut layers, moderate(pass));
        report(&format!("+ {pass:?}"), &layers, &mut store);
        ok(layers.set_visible(id, false));
    }
}

/// A sanity check on the mode list the harness iterates: the nineteen
/// admitted modes, so `every_mode` really covers `curves_mode_code`.
#[test]
fn the_gpu_mode_list_is_the_nineteen_admitted_modes() {
    assert_eq!(GPU_MODES.len(), 19);
    for mode in GPU_MODES {
        assert!(aurora_render::curves_mode_code(crate::translate_blend_mode(mode)).is_some());
        let _ = mode_bound(mode);
    }
}
