//! Invert, Threshold, Posterize, Brightness/Contrast, Channel Mixer,
//! Selective Color and Gradient Map on the GPU path (0.250.0): each GPU
//! pass against the CPU compositor (`apply_adjustment_layer`, the reference)
//! on whole composite tiles, plus the dispatch counters that prove the GPU
//! path really ran, the predicate's refusals, the parameter packing and a
//! pairwise run of every pass after every other in ONE compositor (a
//! copy-paste slot slip in `TileCompositor` is caught by more than one
//! test). The harness (tile patterns, `Conditioning`, the counters) is
//! `gpu_adjustment_tests`' own.
//!
//! Threshold, Posterize and the hard-step cases of Brightness/Contrast and
//! Gradient Map are DISCONTINUOUS: on an opaque backdrop (division by
//! exactly 1) GPU and CPU see bit-identical inputs and are held to the
//! ordinary tolerance; on a translucent backdrop the adapters' `straight_
//! backdrop` division can differ from the CPU's by an `f16` step, which can
//! put a texel on the other side of an edge. Those runs count the channels
//! that disagree, bound the fraction at 0.1 % (the `HardMix` rule of
//! 0.115.1/0.242.0) and assert each disagreement is exactly one step of the
//! quantiser (a bin of `1 / (n - 1)` for Posterize, black against white for
//! Threshold, the colour jump of the stop for Gradient Map).
//!
//! Verified on an RTX 3090 (Vulkan) only; every GPU test self-skips without
//! an adapter (`AURORA_REQUIRE_GPU=1` makes that a failure). No test here
//! pins a float hash or a timing.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::too_many_arguments,
    clippy::float_cmp
)]

use crate::gpu_adjustment_tests::{
    COUNTERS, GPU_MODES, Pass, QUAD, add_adjustment, cpu_tile, edge_opaque, every_mode,
    gpu_matches_cpu, gpu_tile, hash, max_diff, ok, out_of_gamut, pattern_layer, ramp,
    ramp_translucent, random_opaque, random_translucent, run_cases, store, take_counts, tolerance,
    translucent_tolerance,
};
use crate::tests::real_gpu_context;
use aurora_doc::{Adjustment, BlendMode};

// -- Adjustment constructors ------------------------------------------------

fn thr(level: u8) -> Adjustment {
    Adjustment::Threshold(ok(aurora_core::ThresholdParams::new(level)))
}

fn post(levels: u8) -> Adjustment {
    Adjustment::Posterize(ok(aurora_core::PosterizeParams::new(levels)))
}

fn bc(brightness: i16, contrast: i16, legacy: bool) -> Adjustment {
    Adjustment::BrightnessContrast(ok(aurora_core::BrightnessContrastParams::new(
        brightness, contrast, legacy,
    )))
}

fn mixer(rows: [[f32; 4]; 3], mono: bool) -> Adjustment {
    Adjustment::ChannelMixer(ok(aurora_core::ChannelMixerParams::new(rows, mono)))
}

fn selective(sets: &[(usize, [f32; 4])], absolute: bool) -> Adjustment {
    let mut ranges = [[0.0_f32; 4]; 9];
    for &(range, sliders) in sets {
        ranges[range] = sliders;
    }
    Adjustment::SelectiveColor(ok(aurora_core::SelectiveColorParams::new(ranges, absolute)))
}

fn gradient(stops: &[(f32, [f32; 3])], reverse: bool) -> Adjustment {
    let stops = stops
        .iter()
        .map(|&(p, c)| aurora_core::GradientStop::new(p, c))
        .collect();
    Adjustment::GradientMap(ok(aurora_core::GradientMapParams::new(
        stops, reverse, false,
    )))
}

// -- Cases ----------------------------------------------------------------------

const NEUTRAL_ROWS: [[f32; 4]; 3] = [
    [100.0, 0.0, 0.0, 0.0],
    [0.0, 100.0, 0.0, 0.0],
    [0.0, 0.0, 100.0, 0.0],
];

/// Cases whose slope is large enough to carry an adapter's one-`f16`-step
/// `Cb` difference past [`translucent_tolerance`] on a translucent backdrop:
/// held to the opaque tolerance only, as `gpu_photo_adjustment_tests`'
/// `STEEP_CASES` are. The legacy line's slope is `1 / (1 - c / 100)`
/// (5 at contrast 80, 100 at 99), a 200 % mixer weight doubles a step per
/// channel, and 64 random stops make a gradient with slopes of about 30.
pub(crate) const STEEP_CASES: [&str; 10] = [
    "gm 5 stops uneven",
    "sel extremes absolute",
    "bc legacy +0/+80",
    "bc legacy +0/+99",
    "mixer 200 %",
    "mixer overdrive",
    "gm 64 stops",
    "bc default 150/+100",
    "bc default -150/+100",
    "gm 8 stops",
];

fn invert_cases() -> Vec<(&'static str, Adjustment)> {
    vec![("invert", Adjustment::Invert)]
}

fn threshold_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("thr 1", thr(1)),
        ("thr 64", thr(64)),
        ("thr 128", thr(128)),
        ("thr 200", thr(200)),
        ("thr 255", thr(255)),
    ]
}

fn posterize_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("post 2", post(2)),
        ("post 3", post(3)),
        ("post 4", post(4)),
        ("post 5", post(5)),
        ("post 8", post(8)),
        ("post 16", post(16)),
        ("post 64", post(64)),
        ("post 100", post(100)),
    ]
}

fn brightness_contrast_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        ("bc default +40/0", bc(40, 0, false)),
        ("bc default 0/+40", bc(0, 40, false)),
        ("bc default -60/+30", bc(-60, 30, false)),
        ("bc default 23/70", bc(23, 70, false)),
        ("bc default -31/47", bc(-31, 47, false)),
        ("bc default 150/+100", bc(150, 100, false)),
        ("bc default -150/+100", bc(-150, 100, false)),
        ("bc default -95/-50", bc(-95, -50, false)),
        ("bc legacy +20/0", bc(20, 0, true)),
        ("bc legacy +0/+57", bc(0, 57, true)),
        ("bc legacy -30/-40", bc(-30, -40, true)),
        ("bc legacy 20/57", bc(20, 57, true)),
        ("bc legacy 50/50", bc(50, 50, true)),
        ("bc legacy -100/0", bc(-100, 0, true)),
        ("bc legacy +0/+80", bc(0, 80, true)),
        ("bc legacy +0/+99", bc(0, 99, true)),
        ("bc legacy +0/-100", bc(0, -100, true)),
    ]
}

fn channel_mixer_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        (
            "mixer swap r/b",
            mixer(
                [
                    [0.0, 0.0, 100.0, 0.0],
                    [0.0, 100.0, 0.0, 0.0],
                    [100.0, 0.0, 0.0, 0.0],
                ],
                false,
            ),
        ),
        (
            "mixer mono red",
            mixer(
                [
                    [60.0, 30.0, 10.0, 0.0],
                    [0.0, 100.0, 0.0, 0.0],
                    [0.0, 0.0, 100.0, 0.0],
                ],
                true,
            ),
        ),
        (
            "mixer sepia",
            mixer(
                [
                    [39.3, 76.9, 18.9, 0.0],
                    [34.9, 68.6, 16.8, 0.0],
                    [27.2, 53.4, 13.1, 0.0],
                ],
                false,
            ),
        ),
        (
            "mixer constants",
            mixer(
                [
                    [100.0, 0.0, 0.0, 12.0],
                    [0.0, 100.0, 0.0, -25.0],
                    [0.0, 0.0, 100.0, 40.0],
                ],
                false,
            ),
        ),
        (
            "mixer cross",
            mixer(
                [
                    [80.0, 20.0, 10.0, 0.0],
                    [-10.0, 90.0, 30.0, 0.0],
                    [15.0, -20.0, 85.0, 5.0],
                ],
                false,
            ),
        ),
        (
            "mixer 200 %",
            mixer(
                [
                    [200.0, 0.0, 0.0, 0.0],
                    [0.0, 200.0, 0.0, 0.0],
                    [0.0, 0.0, 200.0, 0.0],
                ],
                false,
            ),
        ),
        (
            "mixer overdrive",
            mixer(
                [
                    [200.0, -100.0, 100.0, 0.0],
                    [-200.0, 200.0, 0.0, 0.0],
                    [0.0, 100.0, -150.0, 50.0],
                ],
                false,
            ),
        ),
    ]
}

fn selective_color_cases() -> Vec<(&'static str, Adjustment)> {
    vec![
        (
            "sel reds relative",
            selective(&[(0, [-40.0, 30.0, 20.0, 10.0])], false),
        ),
        (
            "sel reds absolute",
            selective(&[(0, [-40.0, 30.0, 20.0, 10.0])], true),
        ),
        (
            "sel yellows",
            selective(&[(1, [20.0, -30.0, 50.0, 0.0])], false),
        ),
        (
            "sel greens",
            selective(&[(2, [-50.0, 40.0, -60.0, 15.0])], true),
        ),
        (
            "sel cyans",
            selective(&[(3, [50.0, 0.0, 0.0, -20.0])], false),
        ),
        (
            "sel blues",
            selective(&[(4, [30.0, 30.0, -50.0, 0.0])], true),
        ),
        (
            "sel magentas",
            selective(&[(5, [0.0, 70.0, 0.0, 0.0])], false),
        ),
        (
            "sel whites",
            selective(&[(6, [0.0, 0.0, 30.0, -40.0])], true),
        ),
        (
            "sel neutrals",
            selective(&[(7, [20.0, 0.0, -20.0, 25.0])], false),
        ),
        ("sel blacks", selective(&[(8, [0.0, 0.0, 0.0, 60.0])], true)),
        (
            "sel black only",
            selective(
                &[(0, [0.0, 0.0, 0.0, 80.0]), (8, [0.0, 0.0, 0.0, -80.0])],
                false,
            ),
        ),
        (
            "sel all ranges",
            selective(
                &[
                    (0, [10.0, -10.0, 20.0, 5.0]),
                    (1, [-20.0, 10.0, 10.0, 0.0]),
                    (2, [30.0, 0.0, -30.0, -5.0]),
                    (3, [0.0, 20.0, 0.0, 10.0]),
                    (4, [-10.0, -20.0, 40.0, 0.0]),
                    (5, [15.0, 15.0, -15.0, -10.0]),
                    (6, [0.0, 0.0, 10.0, -10.0]),
                    (7, [10.0, -5.0, 0.0, 5.0]),
                    (8, [0.0, 0.0, 0.0, 20.0]),
                ],
                false,
            ),
        ),
        (
            "sel extremes absolute",
            selective(
                &[
                    (0, [100.0, -100.0, 100.0, 100.0]),
                    (4, [-100.0, 100.0, -100.0, -100.0]),
                    (7, [100.0, 100.0, 100.0, 100.0]),
                ],
                true,
            ),
        ),
    ]
}

fn gradient_map_cases() -> Vec<(&'static str, Adjustment)> {
    let eight: Vec<(f32, [f32; 3])> = (0..8_u32)
        .map(|i| {
            (
                i as f32 / 7.0,
                [hash(i, 0, 9), hash(i, 1, 9), hash(i, 2, 9)],
            )
        })
        .collect();
    let sixty_four: Vec<(f32, [f32; 3])> = (0..64_u32)
        .map(|i| {
            (
                i as f32 / 63.0,
                [hash(i, 0, 7), hash(i, 1, 7), hash(i, 2, 7)],
            )
        })
        .collect();
    vec![
        (
            "gm black to white",
            gradient(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], false),
        ),
        (
            "gm reversed",
            gradient(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], true),
        ),
        (
            "gm duotone",
            gradient(&[(0.0, [0.1, 0.0, 0.4]), (1.0, [1.0, 0.8, 0.2])], false),
        ),
        (
            "gm 3 stops",
            gradient(
                &[
                    (0.0, [0.0, 0.0, 0.2]),
                    (0.4, [0.9, 0.2, 0.2]),
                    (1.0, [1.0, 1.0, 0.6]),
                ],
                false,
            ),
        ),
        (
            "gm interior stops clamp",
            gradient(&[(0.25, [0.9, 0.1, 0.1]), (0.75, [0.1, 0.1, 0.9])], false),
        ),
        (
            "gm 5 stops uneven",
            gradient(
                &[
                    (0.0, [0.0; 3]),
                    (0.1, [0.5, 0.0, 0.5]),
                    (0.5, [0.2, 0.7, 0.3]),
                    (0.55, [0.9, 0.9, 0.0]),
                    (1.0, [1.0; 3]),
                ],
                true,
            ),
        ),
        (
            "gm shared first position",
            gradient(
                &[
                    (0.0, [0.9, 0.1, 0.1]),
                    (0.0, [0.1, 0.1, 0.8]),
                    (1.0, [1.0, 1.0, 0.2]),
                ],
                false,
            ),
        ),
        (
            "gm shared last position",
            gradient(
                &[
                    (0.0, [0.0, 0.2, 0.1]),
                    (1.0, [0.2, 0.9, 0.3]),
                    (1.0, [0.9, 0.3, 0.9]),
                ],
                false,
            ),
        ),
        ("gm 8 stops", gradient(&eight, false)),
        ("gm 64 stops", gradient(&sixty_four, false)),
    ]
}

pub(crate) fn cases_of(pass: Pass) -> Vec<(&'static str, Adjustment)> {
    match pass {
        Pass::Invert => invert_cases(),
        Pass::Threshold => threshold_cases(),
        Pass::Posterize => posterize_cases(),
        Pass::BrightnessContrast => brightness_contrast_cases(),
        Pass::ChannelMixer => channel_mixer_cases(),
        Pass::SelectiveColor => selective_color_cases(),
        Pass::GradientMap => gradient_map_cases(),
        _ => Vec::new(),
    }
}

const SEVEN: [Pass; 7] = [
    Pass::Invert,
    Pass::Threshold,
    Pass::Posterize,
    Pass::BrightnessContrast,
    Pass::ChannelMixer,
    Pass::SelectiveColor,
    Pass::GradientMap,
];

/// The kinds whose quantiser is discontinuous (see the module docs).
const DISCONTINUOUS: [Pass; 2] = [Pass::Threshold, Pass::Posterize];

/// A moderate adjustment of each kind, strong enough that every blend mode
/// visibly differs from `Normal` on the ramp.
fn moderate(pass: Pass) -> Adjustment {
    match pass {
        Pass::Invert => Adjustment::Invert,
        Pass::Threshold => thr(120),
        Pass::Posterize => post(4),
        Pass::BrightnessContrast => bc(0, 70, false),
        Pass::ChannelMixer => mixer(
            [
                [80.0, 20.0, 10.0, 0.0],
                [-10.0, 90.0, 30.0, 0.0],
                [15.0, -20.0, 85.0, 5.0],
            ],
            false,
        ),
        Pass::SelectiveColor => selective(
            &[
                (0, [-30.0, 20.0, 10.0, 10.0]),
                (2, [20.0, -30.0, 10.0, 0.0]),
                (6, [0.0, 0.0, 10.0, -20.0]),
                (8, [0.0, 0.0, 0.0, 30.0]),
            ],
            false,
        ),
        Pass::GradientMap => gradient(
            &[
                (0.0, [0.1, 0.0, 0.4]),
                (0.5, [0.9, 0.3, 0.2]),
                (1.0, [1.0, 0.9, 0.5]),
            ],
            false,
        ),
        _ => unreachable!("not one of the seven"),
    }
}

/// The legacy-mode Brightness/Contrast `moderate` is the default (table)
/// mode; this is the line, for the tests that exercise the pass of its own.
fn moderate_legacy_bc() -> Adjustment {
    bc(0, 60, true)
}

/// The identity (neutral) adjustments of each kind, empty for the kinds
/// that are never one (Invert, Threshold, Gradient Map).
fn neutrals(pass: Pass) -> Vec<Adjustment> {
    match pass {
        Pass::Posterize => vec![post(255)],
        Pass::BrightnessContrast => vec![bc(0, 0, false), bc(0, 0, true)],
        Pass::ChannelMixer => vec![mixer(NEUTRAL_ROWS, false)],
        Pass::SelectiveColor => vec![selective(&[], false), selective(&[], true)],
        _ => Vec::new(),
    }
}

// -- Patterns -----------------------------------------------------------------

/// Every combination of four levels per channel: ties, greys, primaries,
/// secondaries and exact range edges, opaque.
fn tie_grid(x: u32, y: u32) -> [f32; 4] {
    let level = |v: u32| [0.0, 0.25, 0.5, 1.0][(v % 4) as usize];
    [level(x / 4), level(x), level(y / 4 + y), 1.0]
}

/// Opaque greys on and around every multiple of `1/16` (a Posterize bin
/// edge for 2, 4, 8 and 16 levels) and `1/3`, one `f16` step either side:
/// the quantiser's edges, where a rounding difference would show.
fn edge_ladder(x: u32, y: u32) -> [f32; 4] {
    let centre = (y % 32) as f32 / 32.0 + if y % 64 < 32 { 0.0 } else { 1.0 / 96.0 };
    let base = half::f16::from_f32(centre);
    let step = (x % 9) as i32 - 4;
    let v = crate::gpu_adjustment_tests::f16_step(base, step).to_f32();
    [v, v, v, 1.0]
}

// -- Differential: opaque, per kind --------------------------------------------

fn opaque_differential(pass: Pass) {
    // The tie grid holds only the levels 0, 1/4, 1/2 and 1, which a 5-level
    // Posterize leaves where they are; the edge ladder is grey, which the
    // colour-dependent passes do not move. Each is used where it bites.
    let mut patterns = vec![
        ("random", random_opaque as fn(u32, u32) -> [f32; 4]),
        ("edge", edge_opaque),
        ("ramp", ramp),
    ];
    if pass != Pass::Posterize {
        patterns.push(("tie grid", tie_grid));
    }
    if DISCONTINUOUS.contains(&pass) {
        patterns.push(("edge ladder", edge_ladder));
    }
    for (label, pattern) in patterns {
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

#[test]
fn gpu_invert_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::Invert);
}

#[test]
fn gpu_threshold_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::Threshold);
}

#[test]
fn gpu_posterize_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::Posterize);
}

#[test]
fn gpu_brightness_contrast_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::BrightnessContrast);
}

#[test]
fn gpu_channel_mixer_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::ChannelMixer);
}

#[test]
fn gpu_selective_color_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::SelectiveColor);
}

#[test]
fn gpu_gradient_map_matches_the_cpu_on_opaque_tiles() {
    opaque_differential(Pass::GradientMap);
}

// -- Differential: translucent, continuous kinds ----------------------------------

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
fn gpu_invert_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::Invert);
}

#[test]
fn gpu_brightness_contrast_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::BrightnessContrast);
}

#[test]
fn gpu_channel_mixer_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::ChannelMixer);
}

#[test]
fn gpu_selective_color_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::SelectiveColor);
}

#[test]
fn gpu_gradient_map_matches_the_cpu_on_translucent_backdrops() {
    translucent_differential(Pass::GradientMap);
}

// -- Differential: translucent, discontinuous quantisers ----------------------------

/// One GPU tile of `adjustment` over `pattern` at `Normal`, opacity 1, against
/// the CPU: the pass must have run once; returns the number of disagreeing
/// colour channels (|GPU - CPU| over [`translucent_tolerance`]) and the
/// number of colour channels. Every disagreeing channel must be exactly one
/// quantiser step (`step` times the texel's alpha, the tile being
/// premultiplied) within `tolerance()` -- or, with `exact` false, at most
/// that (a step whose size differs per channel); alpha must agree to
/// `tolerance()`.
fn flip_differential(
    context: &aurora_gpu::GpuContext,
    pass: Pass,
    adjustment: Adjustment,
    pattern: fn(u32, u32) -> [f32; 4],
    step: f32,
    exact: bool,
    what: &str,
) -> (usize, usize) {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, pattern);
    let without = cpu_tile(&layers, &mut store);
    let _ = add_adjustment(&mut layers, adjustment);
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    assert!(crate::document_qualifies_for_gpu_compositing(&layers));
    let _ = take_counts();
    let Some(gpu) = gpu_tile(context, &mut compositor, &layers, &mut store) else {
        unreachable!("{what}: fell back");
    };
    assert_eq!(take_counts(), pass.expected(1), "{what}: pass recorded");
    let cpu = cpu_tile(&layers, &mut store);
    assert!(
        max_diff(&cpu, &without) > 0.002,
        "{what}: the adjustment must change the composite"
    );
    let mut flipped = 0;
    let mut channels = 0;
    for (i, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
        let d = (g - c).abs();
        if i % 4 == 3 {
            assert!(d <= tolerance(), "{what}: alpha {g} vs {c}");
            continue;
        }
        channels += 1;
        if d > translucent_tolerance() {
            flipped += 1;
            let alpha = cpu[(i / 4) * 4 + 3];
            let ok_step = if exact {
                // A flip moves the channel by one quantiser step; the texel's
                // alpha weights it, but the backdrop's own rounding can put
                // the stored difference a little above that weighted step
                // (measured on Apple M2 Pro / Metal: 1.0 against 0.969).
                d >= step * alpha - tolerance() && d <= step + tolerance()
            } else {
                d <= step * alpha + tolerance()
            };
            assert!(
                ok_step,
                "{what}: texel {} channel {}: a disagreement of {d} is not one step ({step} x alpha {alpha})",
                i / 4,
                i % 4
            );
        } else {
            assert!(d <= translucent_tolerance());
        }
    }
    println!("{what}: {flipped} of {channels} channels flipped one step");
    (flipped, channels)
}

fn flip_fraction_ok(flipped: usize, channels: usize) -> bool {
    flipped * 1000 <= channels
}

#[test]
fn gpu_threshold_flips_at_most_one_step_on_translucent_backdrops() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (name, adjustment) in threshold_cases() {
        for (label, pattern) in [
            (
                "random translucent",
                random_translucent as fn(u32, u32) -> [f32; 4],
            ),
            ("ramp translucent", ramp_translucent),
        ] {
            let (flipped, channels) = flip_differential(
                &context,
                Pass::Threshold,
                adjustment.clone(),
                pattern,
                1.0,
                true,
                &format!("{name} / {label}"),
            );
            assert!(flip_fraction_ok(flipped, channels), "{name} {label}");
        }
    }
}

#[test]
fn gpu_posterize_flips_at_most_one_bin_on_translucent_backdrops() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for levels in [2_u8, 3, 4, 5, 8, 16] {
        let adjustment = post(levels);
        for (label, pattern) in [
            (
                "random translucent",
                random_translucent as fn(u32, u32) -> [f32; 4],
            ),
            ("ramp translucent", ramp_translucent),
        ] {
            let (flipped, channels) = flip_differential(
                &context,
                Pass::Posterize,
                adjustment.clone(),
                pattern,
                1.0 / (f32::from(levels) - 1.0),
                true,
                &format!("post {levels} / {label}"),
            );
            assert!(
                flip_fraction_ok(flipped, channels),
                "post {levels} {label}: {flipped} of {channels}"
            );
        }
    }
}

/// The hard steps of the other two kinds: Brightness/Contrast's contrast-100
/// step (black against white) and a Gradient Map with two stops at one
/// position (the colour jump of the step).
#[test]
fn gpu_hard_steps_flip_at_most_one_step_on_translucent_backdrops() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (label, pattern) in [
        (
            "random translucent",
            random_translucent as fn(u32, u32) -> [f32; 4],
        ),
        ("ramp translucent", ramp_translucent),
    ] {
        let (flipped, channels) = flip_differential(
            &context,
            Pass::BrightnessContrast,
            bc(0, 100, true),
            pattern,
            1.0,
            true,
            &format!("bc legacy contrast 100 step / {label}"),
        );
        assert!(flip_fraction_ok(flipped, channels));
        let jump = 0.9; // |0.95 - 0.05|, the steepest channel of the step below
        let (flipped, channels) = flip_differential(
            &context,
            Pass::GradientMap,
            gradient(
                &[
                    (0.0, [0.05, 0.1, 0.2]),
                    (0.5, [0.05, 0.3, 0.4]),
                    (0.5, [0.95, 0.9, 0.8]),
                    (1.0, [0.95, 1.0, 0.9]),
                ],
                false,
            ),
            pattern,
            jump,
            false,
            &format!("gm hard step / {label}"),
        );
        // The three channels jump by 0.9, 0.7 and 0.6: a disagreement is
        // one of those, never more than the biggest.
        assert!(flip_fraction_ok(flipped, channels));
    }
}

// -- Edges of the quantisers, opaque: exact ------------------------------------------

/// Threshold's greys exactly on and around the edge, for the levels whose
/// edge `(level - 0.5) / 255` is a round number (128 -> 0.5) and others.
/// Opaque, so both sides see bit-identical inputs and both use a true `fma`:
/// the decision is the same texel for texel, the tie included.
#[test]
fn gpu_threshold_agrees_with_the_cpu_on_greys_at_and_around_the_edge() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for level in [1_u8, 2, 77, 128, 129, 254, 255] {
        let edge = (f32::from(level) - 0.5) / 255.0;
        let centre = half::f16::from_f32(edge);
        // Grey ramps of f16 neighbours of the edge: x picks the offset, the
        // three channels take slightly different offsets so the weighted sum
        // lands on both sides of the edge and exactly on it for some texels.
        let pattern = move |x: u32, y: u32| {
            let o = (x % 64) as i32 - 32;
            let q = (y % 8) as i32 - 4;
            let c = |k: i32| crate::gpu_adjustment_tests::f16_step(centre, k).to_f32();
            [c(o), c(o + q), c(o - q), 1.0]
        };
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, pattern);
        let _ = add_adjustment(&mut layers, thr(level));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let (cpu, _) = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            Pass::Threshold,
            1,
            tolerance(),
            &format!("threshold level {level} at the edge"),
        );
        let whites = cpu.chunks_exact(4).filter(|t| t[0] > 0.5).count();
        assert!(
            whites > 100 && whites < cpu.len() / 4 - 100,
            "level {level}: the edge must split the tile ({whites} white)"
        );
    }
}

/// Posterize's channel values exactly on every bin edge (`k / n` for 2, 4,
/// 8 and 16 levels, dyadic so exactly representable) and one `f16` step
/// either side. Opaque: `floor(x n)` is exact on both sides, so the bin is
/// identical for every texel.
#[test]
fn gpu_posterize_agrees_with_the_cpu_on_values_at_every_bin_edge() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for levels in [2_u8, 4, 8, 16] {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, edge_ladder);
        let _ = add_adjustment(&mut layers, post(levels));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let _ = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            Pass::Posterize,
            1,
            tolerance(),
            &format!("posterize {levels} at the bin edges"),
        );
    }
}

// -- Every blend mode, per kind -------------------------------------------------

#[test]
fn gpu_seven_adjustments_match_the_cpu_at_every_gpu_blend_mode() {
    for pass in SEVEN {
        every_mode(pass, moderate(pass), ramp);
    }
    // The legacy line of Brightness/Contrast is its own pass; the default
    // mode above goes through the Curves pass.
    every_mode(Pass::BrightnessContrast, moderate_legacy_bc(), ramp);
}

#[test]
fn gpu_seven_adjustments_match_the_cpu_at_the_stable_modes_on_random_tiles() {
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
    for pass in SEVEN {
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
fn gpu_seven_adjustments_apply_opacity_times_fill() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in SEVEN {
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

/// Out-of-gamut and negative backdrops (`straight_backdrop` is unclamped): a
/// non-identity adjustment clamps its input to [0, 1] on both paths; an
/// identity one under a blend mode passes the value through (Posterize 255
/// and Brightness/Contrast 0/0 exactly, Channel Mixer and Selective Color
/// clamped, which is what the CPU returns for them).
#[test]
fn gpu_seven_adjustments_handle_out_of_gamut_backdrops_like_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in SEVEN {
        let mut cases = vec![(moderate(pass), BlendMode::Normal)];
        if pass == Pass::BrightnessContrast {
            cases.push((moderate_legacy_bc(), BlendMode::Normal));
        }
        for identity in neutrals(pass) {
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
/// other mode the pass runs and passes the value through. Invert, Threshold
/// and Gradient Map are never neutral: they run a pass under `Normal`.
#[test]
fn gpu_neutral_layers_record_no_pass_under_normal_and_one_otherwise() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in SEVEN {
        let neutral = neutrals(pass);
        let candidates = if neutral.is_empty() {
            vec![moderate(pass)]
        } else {
            neutral.clone()
        };
        for (i, adjustment) in candidates.into_iter().enumerate() {
            let is_neutral = !neutral.is_empty();
            // A neutral layer is checked on a translucent tile; a layer that
            // is never neutral (Invert, Threshold, Gradient Map) on an opaque
            // one, where even the quantiser is bit-exact.
            let pattern: fn(u32, u32) -> [f32; 4] = if is_neutral {
                random_translucent
            } else {
                random_opaque
            };
            let (_dir, mut store) = store();
            let mut layers = aurora_doc::LayerTree::new();
            let _ = pattern_layer(&mut layers, &mut store, pattern);
            let without = cpu_tile(&layers, &mut store);
            let id = add_adjustment(&mut layers, adjustment);
            let mut compositor = aurora_render::TileCompositor::new(context.device());
            let (cpu, _) = gpu_matches_cpu(
                &context,
                &mut compositor,
                &layers,
                &mut store,
                pass,
                u64::from(!is_neutral),
                tolerance(),
                &format!("{pass:?} #{i} neutral @Normal"),
            );
            if is_neutral {
                assert_eq!(cpu, without, "the CPU skips a neutral layer under Normal");
            } else {
                assert!(
                    max_diff(&cpu, &without) > 0.01,
                    "{pass:?} is never an identity"
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
                if is_neutral {
                    translucent_tolerance()
                } else {
                    tolerance()
                },
                &format!("{pass:?} #{i} neutral @Multiply"),
            );
        }
    }
}

// -- Brightness/Contrast's default mode rides the Curves pass ------------------------

/// The default mode is a 16,385-sample table: it uses the compositor's
/// Curves pass (one upload per parameter set, shared with a real Curves
/// layer of equal parameters), counts as a Brightness/Contrast dispatch and
/// not a Curves one, and the legacy line uses the pass of its own (no
/// upload).
#[test]
fn brightness_contrast_default_mode_uploads_its_table_once_through_the_curves_pass() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, random_opaque);
    let id = add_adjustment(&mut layers, bc(40, 20, false));
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    assert_eq!(compositor.curves_lut_uploads(), 0);
    let _ = gpu_matches_cpu(
        &context,
        &mut compositor,
        &layers,
        &mut store,
        Pass::BrightnessContrast,
        1,
        tolerance(),
        "bc default first tile",
    );
    assert_eq!(compositor.curves_lut_uploads(), 1, "the table is uploaded");
    // Same parameters again (a second tile of the same document): a cache hit.
    let _ = gpu_matches_cpu(
        &context,
        &mut compositor,
        &layers,
        &mut store,
        Pass::BrightnessContrast,
        1,
        tolerance(),
        "bc default again",
    );
    assert_eq!(compositor.curves_lut_uploads(), 1, "an unchanged set hits");
    // An edit is a new parameter set.
    let _ = add_adjustment(&mut layers, bc(-40, 20, false));
    ok(layers.set_visible(id, false));
    let _ = gpu_matches_cpu(
        &context,
        &mut compositor,
        &layers,
        &mut store,
        Pass::BrightnessContrast,
        1,
        tolerance(),
        "bc default edited",
    );
    assert_eq!(compositor.curves_lut_uploads(), 2, "an edit uploads once");
    // The legacy line has its own pass and uploads nothing.
    let (_dir2, mut store2) = self::store();
    let mut layers2 = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers2, &mut store2, random_opaque);
    let _ = add_adjustment(&mut layers2, bc(40, 20, true));
    let mut compositor2 = aurora_render::TileCompositor::new(context.device());
    let _ = gpu_matches_cpu(
        &context,
        &mut compositor2,
        &layers2,
        &mut store2,
        Pass::BrightnessContrast,
        1,
        tolerance(),
        "bc legacy",
    );
    assert_eq!(compositor2.curves_lut_uploads(), 0, "the line has no table");
}

// -- Pairwise: every pass after every other, one compositor ---------------------------

/// Channels over `tolerance()` between a GPU tile and the CPU one.
fn disagreeing_channels(gpu: &[f32], cpu: &[f32]) -> usize {
    gpu.iter()
        .zip(cpu)
        .filter(|(g, c)| (*g - *c).abs() > tolerance())
        .count()
}

/// Every ordered pair (P, Q) of the seven passes, P then Q on top of an
/// opaque pixel layer, all in ONE `TileCompositor`: the second kind finds
/// the first kind's pipeline slot already built, so a pass that was wired to
/// another's slot (or shader, or uniform layout) fails in several pairs, not
/// one. Counters must read exactly one pass of each kind. A pair is held to
/// `tolerance()` when neither pass is a discontinuous quantiser; a
/// quantiser's flips (a texel whose first-pass output differs by one `f16`
/// step) are bounded at 0.5 % of the channels.
#[test]
fn every_ordered_pair_of_the_seven_passes_matches_the_cpu_in_one_compositor() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    let mut pairs = 0;
    for p in SEVEN {
        for q in SEVEN {
            let (_dir, mut store) = store();
            let mut layers = aurora_doc::LayerTree::new();
            let _ = pattern_layer(&mut layers, &mut store, random_opaque);
            let _ = add_adjustment(&mut layers, moderate(p));
            let _ = add_adjustment(&mut layers, moderate(q));
            assert!(crate::document_qualifies_for_gpu_compositing(&layers));
            let _ = take_counts();
            let Some(gpu) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
                unreachable!("{p:?} then {q:?}: fell back");
            };
            let mut expected = p.expected(1);
            for (slot, add) in expected.iter_mut().zip(q.expected(1)) {
                *slot += add;
            }
            assert_eq!(take_counts(), expected, "{p:?} then {q:?}: passes recorded");
            let cpu = cpu_tile(&layers, &mut store);
            let flips = disagreeing_channels(&gpu, &cpu);
            let quantised = DISCONTINUOUS.contains(&p) || DISCONTINUOUS.contains(&q);
            if quantised {
                assert!(
                    flips * 200 <= gpu.len(),
                    "{p:?} then {q:?}: {flips} of {} channels disagree",
                    gpu.len()
                );
            } else {
                assert_eq!(flips, 0, "{p:?} then {q:?}");
            }
            pairs += 1;
        }
    }
    assert_eq!(pairs, 49);
}

// -- Mixed document, refusals, predicate -------------------------------------------

fn earlier_kinds() -> Vec<Adjustment> {
    let curve = ok(aurora_core::ToneCurve::new(&[
        aurora_core::CurvePoint::new(0.0, 0.05),
        aurora_core::CurvePoint::new(0.5, 0.6),
        aurora_core::CurvePoint::new(1.0, 0.95),
    ]));
    let level = |ib, iw, g, ob, ow| ok(aurora_core::LevelsChannel::new(ib, iw, g, ob, ow));
    vec![
        Adjustment::Curves(aurora_core::CurvesParams {
            composite: curve,
            red: None,
            green: None,
            blue: None,
        }),
        Adjustment::Levels(aurora_core::LevelsParams {
            master: level(0.05, 0.95, 1.2, 0.0, 1.0),
            red: aurora_core::LevelsChannel::identity(),
            green: aurora_core::LevelsChannel::identity(),
            blue: aurora_core::LevelsChannel::identity(),
        }),
        Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
            20.0, 10.0, 0.0, false, 0.0, 25.0, 0.0,
        ))),
        Adjustment::Vibrance(ok(aurora_core::VibranceParams::new(30.0, 10.0))),
        Adjustment::Exposure(ok(aurora_core::ExposureParams::new(0.5, 0.0, 1.0))),
        Adjustment::PhotoFilter(ok(aurora_core::PhotoFilterParams::new(
            [0.9, 0.5, 0.0],
            40.0,
            true,
        ))),
        Adjustment::BlackWhite(aurora_core::BlackWhiteParams::default()),
        Adjustment::ColorBalance(ok(aurora_core::ColorBalanceParams::new(
            [[10.0, 0.0, -10.0], [0.0; 3], [0.0, 10.0, 0.0]],
            true,
        ))),
    ]
}

/// A document with a pixel layer, one adjustment of each of the eight earlier
/// kinds and each of the seven new ones (and blended pixel layers between and
/// on top) composites on the GPU with exactly fifteen passes, one per
/// counter, and equals the CPU composite (flips of the two quantisers
/// bounded at 0.5 %).
#[test]
fn a_document_with_all_fifteen_adjustment_kinds_composites_on_the_gpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, random_opaque);
    let mut all: Vec<Adjustment> = earlier_kinds();
    all.extend(SEVEN.map(moderate));
    assert_eq!(all.len(), 15);
    for (i, adjustment) in all.into_iter().enumerate() {
        let id = add_adjustment(&mut layers, adjustment);
        ok(layers.set_opacity(id, 0.95 - i as f32 * 0.02));
        if i % 4 == 1 {
            ok(layers.set_blend_mode(id, BlendMode::Overlay));
        }
        if i == 5 || i == 11 {
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
        [1; COUNTERS],
        "one pass of each of the fifteen kinds"
    );
    let cpu = cpu_tile(&layers, &mut store);
    let worst = max_diff(&gpu, &cpu);
    let flips = disagreeing_channels(&gpu, &cpu);
    println!(
        "fifteen adjustments in one document: worst |GPU - CPU| = {worst:e}, {flips} of {} channels over the tolerance",
        gpu.len()
    );
    assert!(flips * 200 <= gpu.len(), "{flips} channels disagree");
}

/// Masks, `Dissolve`, `Exclusion` and a non-separable mode keep every pass
/// on the CPU, and a group refuses the whole document; a disabled mask is
/// ignored.
#[test]
fn unsupported_newest_adjustment_layers_fall_back_to_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in SEVEN {
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
            assert_eq!(take_counts(), [0; COUNTERS], "{pass:?} {refusal}");
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

/// A visible layer of each kind (neutral or not) qualifies at the root; a
/// hidden one never disqualifies.
#[test]
fn visible_newest_adjustment_layers_qualify_and_hidden_ones_are_ignored() {
    let (_dir, mut store) = store();
    for pass in SEVEN {
        let mut all = vec![moderate(pass)];
        all.extend(neutrals(pass));
        for adjustment in all {
            let mut layers = aurora_doc::LayerTree::new();
            let _ = pattern_layer(&mut layers, &mut store, random_opaque);
            let id = add_adjustment(&mut layers, adjustment);
            assert!(crate::adjustment_layer_qualifies_for_gpu(&layers, id));
            assert!(crate::document_qualifies_for_gpu_compositing(&layers));
            ok(layers.set_visible(id, false));
            assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        }
    }
}

/// A hidden layer of each kind records no pass on the GPU and the tile
/// equals the layer-less composite.
#[test]
fn hidden_newest_adjustment_layers_record_no_pass() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for pass in SEVEN {
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
        aurora_filters::THRESHOLD_PACKED_LEN,
        aurora_render::THRESHOLD_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::POSTERIZE_PACKED_LEN,
        aurora_render::POSTERIZE_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::BRIGHTNESS_CONTRAST_PACKED_LEN,
        aurora_render::BRIGHTNESS_CONTRAST_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::CHANNEL_MIXER_PACKED_LEN,
        aurora_render::CHANNEL_MIXER_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::SELECTIVE_COLOR_PACKED_LEN,
        aurora_render::SELECTIVE_COLOR_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::GRADIENT_MAP_PACKED_LEN,
        aurora_render::GRADIENT_MAP_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::GRADIENT_MAP_PACKED_LEN,
        4 + 4 * aurora_core::GRADIENT_MAP_MAX_STOPS
    );
}

/// std140: the uniform is a 16-byte header then `vec4`s, so every pass's
/// parameter list must be a whole number of `vec4`s or the buffer is short
/// of the struct the shader declares.
#[test]
fn every_new_pass_parameter_list_is_a_whole_number_of_vec4s() {
    for len in [
        aurora_render::THRESHOLD_GPU_PARAMS_LEN,
        aurora_render::POSTERIZE_GPU_PARAMS_LEN,
        aurora_render::BRIGHTNESS_CONTRAST_GPU_PARAMS_LEN,
        aurora_render::CHANNEL_MIXER_GPU_PARAMS_LEN,
        aurora_render::SELECTIVE_COLOR_GPU_PARAMS_LEN,
        aurora_render::GRADIENT_MAP_GPU_PARAMS_LEN,
    ] {
        assert_eq!(len % 4, 0, "{len}");
    }
}

#[test]
fn threshold_packed_for_gpu_is_the_edge_and_the_photoshop_weights() {
    let p = ok(aurora_core::ThresholdParams::new(128));
    let packed = aurora_filters::ThresholdOp::new(&p).packed_for_gpu();
    assert_eq!(packed, [127.5 / 255.0, 0.30, 0.59, 0.11]);
    let other = ok(aurora_core::ThresholdParams::new(1));
    assert_eq!(
        aurora_filters::ThresholdOp::new(&other).packed_for_gpu()[0],
        0.5 / 255.0
    );
}

#[test]
fn posterize_packed_for_gpu_is_the_flag_count_and_top() {
    let p = ok(aurora_core::PosterizeParams::new(5));
    assert_eq!(
        aurora_filters::PosterizeOp::new(&p).packed_for_gpu(),
        [0.0, 5.0, 4.0, 0.0]
    );
    let identity = ok(aurora_core::PosterizeParams::new(255));
    assert_eq!(
        aurora_filters::PosterizeOp::new(&identity).packed_for_gpu(),
        [1.0, 255.0, 254.0, 0.0]
    );
}

#[test]
fn brightness_contrast_packed_for_gpu_is_the_line_the_step_or_a_table() {
    let op = |b, c, legacy| {
        aurora_filters::BrightnessContrastOp::new(&ok(aurora_core::BrightnessContrastParams::new(
            b, c, legacy,
        )))
    };
    // Identity: kind 0.
    assert_eq!(op(0, 0, true).packed_for_gpu(), Some([0.0; 4]));
    assert_eq!(op(0, 0, false).packed_for_gpu(), Some([0.0; 4]));
    // The line: kind 1, shift = b / 255, mean = 127 / 255, slope = 1 / (1 - c / 100).
    let line = op(51, 50, true).packed_for_gpu();
    assert_eq!(line, Some([1.0, 51.0 / 255.0, 127.0 / 255.0, 2.0]));
    let negative = op(0, -50, true).packed_for_gpu();
    assert_eq!(negative, Some([1.0, 0.0, 127.0 / 255.0, 0.5]));
    // The step: kind 2 and a finite slope (infinity never reaches the GPU).
    let step = op(10, 100, true).packed_for_gpu();
    assert_eq!(step, Some([2.0, 10.0 / 255.0, 127.0 / 255.0, 0.0]));
    // The default mode is a table, handed to the Curves pass.
    let default = op(40, 20, false);
    assert_eq!(default.packed_for_gpu(), None);
    let Some((params, table)) = default.curve_for_gpu() else {
        unreachable!("the default mode has a curve");
    };
    assert_eq!(
        table.packed_for_gpu().len(),
        aurora_filters::CURVES_PACKED_LEN
    );
    assert!(params.red.is_none() && params.green.is_none() && params.blue.is_none());
    assert!(op(20, 0, true).curve_for_gpu().is_none());
    assert!(op(0, 0, false).curve_for_gpu().is_none());
}

#[test]
fn channel_mixer_packed_for_gpu_is_the_flag_and_the_rows_as_fractions() {
    let rows = [
        [10.0, 20.0, 30.0, 40.0],
        [-50.0, 60.0, 70.0, -80.0],
        [90.0, 100.0, -110.0, 5.0],
    ];
    let packed =
        aurora_filters::ChannelMixerOp::new(&ok(aurora_core::ChannelMixerParams::new(rows, true)))
            .packed_for_gpu();
    assert_eq!(
        packed,
        [
            1.0, 0.0, 0.0, 0.0, // flags
            0.1, 0.2, 0.3, 0.4, // red output
            -0.5, 0.6, 0.7, -0.8, // green output
            0.9, 1.0, -1.1, 0.05, // blue output
        ]
    );
    let plain =
        aurora_filters::ChannelMixerOp::new(&ok(aurora_core::ChannelMixerParams::new(rows, false)))
            .packed_for_gpu();
    assert_eq!(plain[0], 0.0);
}

#[test]
fn selective_color_packed_for_gpu_is_the_flag_and_nine_ink_rows() {
    let mut ranges = [[0.0_f32; 4]; 9];
    for (i, row) in ranges.iter_mut().enumerate() {
        *row = [i as f32, -(i as f32), 2.0 * i as f32, 100.0 - i as f32];
    }
    let packed = aurora_filters::SelectiveColorOp::new(&ok(
        aurora_core::SelectiveColorParams::new(ranges, true),
    ))
    .packed_for_gpu();
    assert_eq!(packed.len(), 40);
    assert_eq!(packed[..4], [1.0, 0.0, 0.0, 0.0]);
    for i in 0..9 {
        let row = &packed[4 + i * 4..8 + i * 4];
        let want = [
            i as f32 / 100.0,
            -(i as f32) / 100.0,
            2.0 * i as f32 / 100.0,
            (100.0 - i as f32) / 100.0,
        ];
        assert_eq!(row, want, "range {i}");
    }
    let relative = aurora_filters::SelectiveColorOp::new(&ok(
        aurora_core::SelectiveColorParams::new(ranges, false),
    ))
    .packed_for_gpu();
    assert_eq!(relative[0], 0.0);
}

#[test]
fn gradient_map_packed_for_gpu_is_the_header_the_stops_and_a_last_stop_pad() {
    let stops = [
        (0.0, [0.1, 0.2, 0.3]),
        (0.25, [0.4, 0.5, 0.6]),
        (0.25, [0.7, 0.8, 0.9]),
        (1.0, [1.0, 0.0, 0.5]),
    ];
    let make = |reverse| {
        let Adjustment::GradientMap(p) = gradient(&stops, reverse) else {
            unreachable!("a gradient map");
        };
        aurora_filters::GradientMapOp::new(&p).packed_for_gpu()
    };
    let packed = make(false);
    assert_eq!(packed.len(), aurora_filters::GRADIENT_MAP_PACKED_LEN);
    assert_eq!(packed[..4], [0.0, 4.0, 0.0, 0.0]);
    for (i, (pos, c)) in stops.iter().enumerate() {
        assert_eq!(
            packed[4 + i * 4..8 + i * 4],
            [*pos, c[0], c[1], c[2]],
            "stop {i}"
        );
    }
    // Past the count the last stop repeats.
    for slot in 4..64 {
        assert_eq!(
            packed[4 + slot * 4..8 + slot * 4],
            [1.0, 1.0, 0.0, 0.5],
            "slot {slot}"
        );
    }
    assert_eq!(make(true)[0], 1.0);
    // 64 stops fill every slot.
    let many: Vec<(f32, [f32; 3])> = (0..64).map(|i| (i as f32 / 63.0, [0.5; 3])).collect();
    let Adjustment::GradientMap(p) = gradient(&many, false) else {
        unreachable!("a gradient map");
    };
    let full = aurora_filters::GradientMapOp::new(&p).packed_for_gpu();
    assert_eq!(full[1], 64.0);
    assert_eq!(full[4 + 63 * 4], 1.0);
    assert!(full.iter().all(|v| v.is_finite()));
}

// -- Cost -----------------------------------------------------------------------

/// CPU against GPU composite time for a 4096 x 4096 document (256 tiles):
/// one opaque pixel layer, then the document with each of the seven
/// adjustments (moderate settings) on top, CPU path against GPU path.
/// `#[ignore]`d measurement; run with
/// `cargo test -p aurora-app --release -- --ignored --nocapture measure_seven`.
#[test]
#[ignore = "measurement"]
fn measure_seven_adjustments_composite_cpu_against_gpu_on_a_4096_document() {
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
    for pass in SEVEN {
        let id = add_adjustment(&mut layers, moderate(pass));
        report(&format!("+ {pass:?}"), &layers, &mut store);
        ok(layers.set_visible(id, false));
    }
}

/// The nineteen modes the harness iterates are the admitted ones (so
/// `every_mode` over the seven kinds covers `curves_mode_code`).
#[test]
fn the_newest_passes_are_exercised_at_all_nineteen_gpu_modes() {
    assert_eq!(GPU_MODES.len(), 19);
    for mode in GPU_MODES {
        assert!(aurora_render::curves_mode_code(crate::translate_blend_mode(mode)).is_some());
    }
}
