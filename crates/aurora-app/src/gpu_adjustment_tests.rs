//! Levels and Hue/Saturation on the GPU path (0.206.0): the GPU passes
//! against the CPU compositor (`apply_adjustment_layer`, the reference) on
//! whole composite tiles, plus the dispatch counters that prove the GPU
//! path really ran and the predicate's refusals.
//!
//! Verified on an RTX 3090 (Vulkan) only; every GPU test self-skips
//! without an adapter (`AURORA_REQUIRE_GPU=1` makes that a failure).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::too_many_arguments,
    clippy::float_cmp
)]

use crate::tests::real_gpu_context;
use aurora_doc::BlendMode;

pub(crate) const TILE0: aurora_tile::TileId = aurora_tile::TileId { x: 0, y: 0 };
pub(crate) const QUAD: aurora_core::Rect = aurora_core::Rect {
    x: 0,
    y: 0,
    width: 512,
    height: 512,
};

/// The GPU/CPU tolerance on an opaque backdrop: the existing blend-mode
/// differential's `2 * f16::EPSILON` (about half an 8-bit level). The pass
/// rounds `Cb`, `f(Cb)` and the mix to `f16` where the CPU stores them, so
/// what is left is `pack2x16float`'s implementation-defined rounding
/// direction (one `f16` step) and the GPU's own transcendental / division
/// rounding (`pow`, `/`) flipping an `f16` rounding.
pub(crate) fn tolerance() -> f32 {
    2.0 * f32::from(half::f16::EPSILON)
}

/// The bound on a translucent backdrop: `straight_backdrop`'s division
/// (WGSL: 2.5 ULP) can move `Cb` by one `f16` step, which the adjustment's
/// slope scales and the blend term carries, then the final premultiply
/// rounds again. The same derived-per-mode class as the Curves tests
/// (about 6-8 `f16` steps at the top of the range, `2^-11` each); the
/// adjustments tested here have a slope of at most about 2 on these
/// fixtures (Hue/Saturation saturation at most +50, Levels gamma and
/// range stretches kept moderate in these tiles).
///
/// 0.206.1: 10 steps, not 8. Windows' WARP software adapter reached
/// exactly 8 steps on Hue/Saturation hue shifts (no margin) and 3.4e-3..
/// 3.9e-3 elsewhere.
///
/// 0.208.2: 16 steps. The error scales with `1 / alpha` of the backdrop
/// (`straight_backdrop` divides by it; the random tiles reach alpha 0.1, a
/// factor of 10), and GitHub's macOS adapter (Apple Paravirtual) measured
/// 11 steps on Hue/Saturation lightness -50 where the RTX 3090 stays
/// within 6.
///
/// 0.211.1: still 16 steps, now with [`Conditioning`]: the 11- and
/// 17-step failures were not a gap in this bound but texels whose CPU
/// result moves by up to ~0.03 for a one-step backdrop difference.
pub(crate) fn translucent_tolerance() -> f32 {
    16.0 / 2048.0
}

pub(crate) fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

pub(crate) fn store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero");
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

/// A deterministic value in `[0, 1)` for texel `(x, y)` and a `salt`.
pub(crate) fn hash(x: u32, y: u32, salt: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(y.wrapping_mul(0x85EB_CA77))
        .wrapping_add(salt.wrapping_mul(0xC2B2_AE3D));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// Opaque uniform-random colours.
pub(crate) fn random_opaque(x: u32, y: u32) -> [f32; 4] {
    [hash(x, y, 1), hash(x, y, 2), hash(x, y, 3), 1.0]
}

/// Named corner cases (black, white, greys, primaries, secondaries, tints,
/// near-black and near-white near-greys) in the top rows, random colours
/// below; opaque.
pub(crate) fn edge_opaque(x: u32, y: u32) -> [f32; 4] {
    const PALETTE: [[f32; 3]; 16] = [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.5, 0.5, 0.5],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [0.2, 0.4, 0.6],
        [0.9, 0.5, 0.5],
        [0.01, 0.02, 0.015],
        [0.99, 0.98, 0.985],
        [0.25, 0.25, 0.25],
        [0.75, 0.5, 0.25],
        [0.3, 0.9, 0.4],
    ];
    if y < 64 {
        let [r, g, b] = PALETTE[(x as usize / 4) % PALETTE.len()];
        let s = 1.0 - (y % 8) as f32 / 10.0;
        [r * s, g * s, b * s, 1.0]
    } else {
        random_opaque(x, y)
    }
}

/// A smooth colour ramp, opaque.
pub(crate) fn ramp(x: u32, y: u32) -> [f32; 4] {
    [
        x as f32 / 255.0,
        y as f32 / 255.0,
        ((x * 3 + y * 5) % 256) as f32 / 255.0,
        1.0,
    ]
}

/// Random colours at a random alpha in `[0.1, 1)`, a fully transparent band
/// at the top.
/// The lowest backdrop alpha of the translucent tiles. `straight_backdrop`
/// divides by it, so an adapter's one-`f16`-step rounding difference is
/// amplified by up to `1 / ALPHA_FLOOR`. 0.25 (was 0.1 until 0.209.1) keeps
/// that factor at 4: GitHub's Windows (WARP) and macOS (Apple Paravirtual)
/// adapters kept finding new 10x cases at 0.1, each needing a wider bound.
pub(crate) const ALPHA_FLOOR: f32 = 0.25;

pub(crate) fn random_translucent(x: u32, y: u32) -> [f32; 4] {
    let a = if y < 16 {
        0.0
    } else {
        ALPHA_FLOOR + (1.0 - ALPHA_FLOOR) * hash(x, y, 4)
    };
    [hash(x, y, 1), hash(x, y, 2), hash(x, y, 3), a]
}

/// [`ramp`] at an alpha that varies across the tile.
pub(crate) fn ramp_translucent(x: u32, y: u32) -> [f32; 4] {
    let [r, g, b, _] = ramp(x, y);
    let a = if y < 16 {
        0.0
    } else {
        ALPHA_FLOOR + (1.0 - ALPHA_FLOOR) * x as f32 / 255.0
    };
    [r, g, b, a]
}

/// Out-of-gamut straight colours (negative, above one) at alpha 1, which
/// `straight_backdrop` leaves unclamped.
pub(crate) fn out_of_gamut(x: u32, y: u32) -> [f32; 4] {
    [
        hash(x, y, 1) * 3.0 - 1.0,
        hash(x, y, 2) * 2.0 - 0.5,
        hash(x, y, 3) * 4.0 - 2.0,
        1.0,
    ]
}

pub(crate) fn pattern_layer(
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    pattern: impl Fn(u32, u32) -> [f32; 4],
) -> aurora_doc::LayerId {
    let id = ok(layers.add_pixel_layer("pattern", QUAD, None));
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("a pixel layer");
    };
    let tile = ok(store.get_mut(surface, TILE0));
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let texel = (i / 4) as u32;
        let rgba = pattern(texel % aurora_tile::TILE, texel / aurora_tile::TILE);
        *sample = half::f16::from_f32(rgba[i % 4]);
    }
    tile.mark_dirty(aurora_core::Rect {
        x: 0,
        y: 0,
        width: aurora_tile::TILE,
        height: aurora_tile::TILE,
    });
    id
}

pub(crate) fn add_adjustment(
    layers: &mut aurora_doc::LayerTree,
    adjustment: aurora_doc::Adjustment,
) -> aurora_doc::LayerId {
    ok(layers.add_adjustment_layer_at("adjustment", adjustment, None, 0))
}

fn record(ib: f32, iw: f32, g: f32, ob: f32, ow: f32) -> aurora_core::LevelsChannel {
    ok(aurora_core::LevelsChannel::new(ib, iw, g, ob, ow))
}

fn levels(
    master: aurora_core::LevelsChannel,
    rgb: [aurora_core::LevelsChannel; 3],
) -> aurora_doc::Adjustment {
    let [red, green, blue] = rgb;
    aurora_doc::Adjustment::Levels(aurora_core::LevelsParams {
        master,
        red,
        green,
        blue,
    })
}

fn levels_master(master: aurora_core::LevelsChannel) -> aurora_doc::Adjustment {
    let id = aurora_core::LevelsChannel::identity();
    levels(master, [id; 3])
}

fn hue_sat(h: f32, s: f32, l: f32) -> aurora_doc::Adjustment {
    aurora_doc::Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
        h, s, l, false, 0.0, 25.0, 0.0,
    )))
}

fn colorize(h: f32, s: f32, l: f32) -> aurora_doc::Adjustment {
    aurora_doc::Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
        0.0, 0.0, 0.0, true, h, s, l,
    )))
}

pub(crate) fn cpu_tile(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
) -> Vec<f32> {
    let mut budget = crate::CompositeBudget::for_pass(layers);
    let (texels, _) =
        crate::composite_roots_into_tile(layers, store, TILE0, (0, 0), (0, 0), &mut budget);
    texels.iter().map(|v| v.to_f32()).collect()
}

pub(crate) fn gpu_tile(
    context: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
) -> Option<Vec<f32>> {
    let mut budget = crate::CompositeBudget::for_pass(layers);
    let pending = crate::begin_gpu_composite_tile(
        context,
        compositor,
        layers,
        store,
        TILE0,
        (0, 0),
        (0, 0),
        &mut budget,
    )?;
    let _ = context.device().poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    crate::finish_tile_readback(pending).map(|t| t.iter().map(|v| v.to_f32()).collect())
}

pub(crate) fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// How many dispatch counters [`take_counts`] reads.
pub(crate) const COUNTERS: usize = 15;

/// The passes recorded since the last call, `[curves, levels, hue_sat,
/// vibrance, exposure, photo_filter, black_white, color_balance, invert,
/// threshold, posterize, brightness_contrast, channel_mixer,
/// selective_color, gradient_map]`.
pub(crate) fn take_counts() -> [u64; COUNTERS] {
    [
        crate::take_gpu_curves_dispatch_count(),
        crate::take_gpu_levels_dispatch_count(),
        crate::take_gpu_hue_saturation_dispatch_count(),
        crate::take_gpu_vibrance_dispatch_count(),
        crate::take_gpu_exposure_dispatch_count(),
        crate::take_gpu_photo_filter_dispatch_count(),
        crate::take_gpu_black_white_dispatch_count(),
        crate::take_gpu_color_balance_dispatch_count(),
        crate::take_gpu_invert_dispatch_count(),
        crate::take_gpu_threshold_dispatch_count(),
        crate::take_gpu_posterize_dispatch_count(),
        crate::take_gpu_brightness_contrast_dispatch_count(),
        crate::take_gpu_channel_mixer_dispatch_count(),
        crate::take_gpu_selective_color_dispatch_count(),
        crate::take_gpu_gradient_map_dispatch_count(),
    ]
}

/// Which adjustment pass a test expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    Levels,
    HueSaturation,
    Vibrance,
    Exposure,
    PhotoFilter,
    BlackWhite,
    ColorBalance,
    Invert,
    Threshold,
    Posterize,
    BrightnessContrast,
    ChannelMixer,
    SelectiveColor,
    GradientMap,
}

impl Pass {
    /// Whether this is one of the seven 0.250.0 kinds (steep or binary
    /// sources; see [`source_conditioning`]).
    pub(crate) fn is_newest(self) -> bool {
        matches!(
            self,
            Self::Invert
                | Self::Threshold
                | Self::Posterize
                | Self::BrightnessContrast
                | Self::ChannelMixer
                | Self::SelectiveColor
                | Self::GradientMap
        )
    }

    /// The counters [`take_counts`] must read after `passes` passes of
    /// `self` and none of any other kind.
    pub(crate) fn expected(self, passes: u64) -> [u64; COUNTERS] {
        let mut counts = [0; COUNTERS];
        let slot = match self {
            Self::Levels => 1,
            Self::HueSaturation => 2,
            Self::Vibrance => 3,
            Self::Exposure => 4,
            Self::PhotoFilter => 5,
            Self::BlackWhite => 6,
            Self::ColorBalance => 7,
            Self::Invert => 8,
            Self::Threshold => 9,
            Self::Posterize => 10,
            Self::BrightnessContrast => 11,
            Self::ChannelMixer => 12,
            Self::SelectiveColor => 13,
            Self::GradientMap => 14,
        };
        if let Some(count) = counts.get_mut(slot) {
            *count = passes;
        }
        counts
    }
}

/// One GPU tile that must have run exactly `passes` passes of `pass` (and
/// none of the others), compared against the CPU reference within `bound`.
/// Returns `(cpu, worst |GPU - CPU|)`.
pub(crate) fn gpu_matches_cpu(
    context: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    pass: Pass,
    passes: u64,
    bound: f32,
    what: &str,
) -> (Vec<f32>, f32) {
    gpu_matches_cpu_excluding(
        context, compositor, layers, store, pass, passes, bound, what, None,
    )
}

/// How many `f16` steps of the premultiplied backdrop [`Conditioning`]
/// perturbs each channel by: one, the most an adapter's blend unit or
/// `straight_backdrop` division was ever measured to move it.
const CONDITIONING_STEPS: i32 = 1;

/// Per-texel conditioning of a translucent case (0.211.1): how much the
/// CPU result moves when the premultiplied backdrop moves by one `f16`
/// step per channel. GitHub's macOS (Apple Paravirtual, Metal) and Windows
/// (WARP, Dx12) adapters round the backdrop one step differently from the
/// RTX 3090, and Hue/Saturation's HSL turns that step into up to ~0.03 at
/// a light, barely coloured pixel: with `l` near 1 the saturation
/// `delta / (2 - hi - lo)` divides by ~0.08, and a lightness cut then
/// scales the chroma back up (texel (198, 20) of `random_translucent` at
/// lightness -50 moves 0.0087 on macOS and 0.0174 on Windows, and the same
/// one-step change in the CPU result reproduces the macOS value to the
/// bit). No bound on the GPU/CPU difference holds there, so such texels
/// are reported and held to a looser, sensitivity-relative bound instead.
pub(crate) struct Conditioning {
    /// Texels whose sensitivity exceeds the limit, per texel.
    pub(crate) ill: Vec<bool>,
    /// The sensitivity per texel.
    pub(crate) sensitivity: Vec<f32>,
    /// The unperturbed result, which must equal the CPU tile exactly:
    /// proof that the sensitivity is measured on the very reference the
    /// GPU is compared against.
    pub(crate) base: Vec<f32>,
}

/// The sensitivity above which a texel is excluded from the tight bound:
/// `bound` minus the final readback's own one-step rounding direction
/// (`pack2x16float` may round either way; `1 / ALPHA_FLOOR` of `2^-11`
/// after the un-premultiply).
pub(crate) fn conditioning_limit(bound: f32) -> f32 {
    bound - ALPHA_FLOOR.recip() / 2048.0
}

/// `v` moved by `k` `f16` steps (a premultiplied backdrop channel is never
/// negative; a negative `v` is returned unchanged). Below zero it goes to
/// the negative subnormals, which is what one step down from `0.0` is.
pub(crate) fn f16_step(v: half::f16, k: i32) -> half::f16 {
    let bits = v.to_bits();
    if bits & 0x8000 != 0 {
        return v;
    }
    let moved = (i32::from(bits) + k).min(0x7BFF);
    if moved >= 0 {
        half::f16::from_bits(moved as u16)
    } else {
        half::f16::from_bits(0x8000 | (-moved) as u16)
    }
}

/// Measures [`Conditioning`] of the adjustment layer `id` over the tile
/// whose composite without it is `without` (straight, as [`cpu_tile`]
/// returns it), by running the CPU's own `apply_adjustment_layer` on the
/// premultiplied backdrop and on every combination of its colour channels
/// moved by up to [`CONDITIONING_STEPS`] `f16` steps.
pub(crate) fn conditioning_of(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    id: aurora_doc::LayerId,
    without: &[f32],
    bound: f32,
) -> Conditioning {
    use half::f16;
    let mut backdrop = Vec::with_capacity(without.len());
    for px in without.chunks_exact(4) {
        if let [r, g, b, a] = *px {
            let a = f16::from_f32(a);
            for c in [r, g, b] {
                backdrop.push(f16::from_f32(f16::from_f32(c).to_f32() * a.to_f32()));
            }
            backdrop.push(a);
        }
    }
    let mut run = |acc: Vec<f16>| -> Vec<f32> {
        let mut acc = acc;
        let mut budget = crate::CompositeBudget::for_pass(layers);
        let _ = crate::apply_adjustment_layer(&mut acc, id, layers, store, (0, 0), 0, &mut budget);
        aurora_render::un_premultiply_in_place(&mut acc);
        acc.iter().map(|v| v.to_f32()).collect()
    };
    let base = run(backdrop.clone());
    let mut sensitivity = vec![0.0_f32; without.len() / 4];
    let n = CONDITIONING_STEPS;
    for dr in -n..=n {
        for dg in -n..=n {
            for db in -n..=n {
                let moved: Vec<f16> = backdrop
                    .chunks_exact(4)
                    .flat_map(|t| {
                        [
                            f16_step(t[0], dr),
                            f16_step(t[1], dg),
                            f16_step(t[2], db),
                            t[3],
                        ]
                    })
                    .collect();
                let out = run(moved);
                for (i, s) in sensitivity.iter_mut().enumerate() {
                    for c in 0..3 {
                        *s = s.max((out[i * 4 + c] - base[i * 4 + c]).abs());
                    }
                }
            }
        }
    }
    let limit = conditioning_limit(bound);
    let ill = sensitivity.iter().map(|s| *s > limit).collect();
    Conditioning {
        ill,
        sensitivity,
        base,
    }
}

/// Per-texel conditioning of an OPAQUE tile with respect to the adjustment's
/// own SOURCE colour (`Cs`), for the adjustment layer `id` at its current
/// blend mode and content opacity; `without` is the composite of the tile
/// without the layer (the pattern, opaque), `bound` the bound the caller
/// compares with.
///
/// [`conditioning_of`] perturbs the backdrop, which is the right probe for a
/// translucent tile (the adapters' `straight_backdrop` division moves `Cb`).
/// On an opaque tile `Cb` is bit-identical on both sides and what can differ
/// is `Cs`: the adjustment's result is rounded to `f16` in the shader
/// (`pack2x16float`, whose rounding direction WGSL leaves to the
/// implementation: WARP and Metal differ from the RTX 3090 at exact
/// round-half ties) where the CPU rounds half to even. An inversion
/// `1 - Cb` of a small `f16` is exactly such a tie one time in 2^k. A steep
/// or binary source meets a dividing or thresholding blend term (`ColorDodge`
/// `Cb / (1 - Cs)` with `1 - Cs = Cb`, `ColorBurn`, `VividLight`, `Divide`,
/// `HardMix`) and one `f16` step of `Cs` becomes a large step of the result.
///
/// So the probe moves `Cs` itself: the source tile the CPU builds is moved
/// by up to one `f16` step per colour channel (all 27 combinations), each
/// folded with the very `composite_layer_into` call
/// `apply_adjustment_layer` uses, and a texel's sensitivity is the largest
/// change of its result. A channel is moved unless its result is exactly 0
/// or exactly 1 -- the outputs of a clamp or a constant (Threshold, the
/// steps, a clipped mix), identical on every adapter. Every other channel
/// is moved, EXACTLY-REPRESENTABLE ONES TOO: the GPU evaluates the same
/// arithmetic in a different order or fused, its f32 can sit an ulp below
/// the `f16` the CPU's sits on, and `pack2x16float` is then free to give
/// the lower neighbour (measured on the RTX 3090: Channel Mixer green
/// 0.501953125 landed one step low and flipped a `HardMix` texel whose
/// Cb + Cs was exactly 1).
/// A texel is ill-conditioned when its sensitivity exceeds `bound` less two
/// `f16` steps (`2/2048`, the adapter's own rounding of the result: one step
/// of `Cs` then leaves no room in the bound); callers hold it to
/// `bound + 2 * sensitivity` and every other texel to `bound`, so a shader
/// that is wrong at a well-conditioned texel still fails. The probe is
/// computed from CPU values only, so the same texels are ill-conditioned on
/// every adapter. Opaque tiles only: the result is not weighted by alpha.
pub(crate) fn source_conditioning(
    layers: &aurora_doc::LayerTree,
    id: aurora_doc::LayerId,
    without: &[f32],
    bound: f32,
) -> Conditioning {
    use half::f16;
    let Some(adjustment) = layers.adjustment(id) else {
        unreachable!("an adjustment layer");
    };
    let lut = crate::AdjustmentLut::of(adjustment);
    let opacity = crate::content_opacity(layers, id).unwrap_or(1.0);
    let mode = crate::translate_blend_mode(
        layers
            .blend_mode(id)
            .unwrap_or(aurora_doc::BlendMode::Normal),
    );
    let one = f16::from_f32(1.0);
    // The backdrop as the compositor holds it (opaque: straight = stored).
    let mut backdrop: Vec<f16> = Vec::with_capacity(without.len());
    for px in without.chunks_exact(4) {
        if let [r, g, b, _] = *px {
            backdrop.extend([f16::from_f32(r), f16::from_f32(g), f16::from_f32(b), one]);
        }
    }
    let mut source: Vec<f16> = Vec::with_capacity(backdrop.len());
    let mut rounded: Vec<[bool; 3]> = Vec::with_capacity(backdrop.len() / 4);
    for texel in backdrop.chunks_exact(4) {
        if let [r, g, b, _] = *texel {
            let out = lut.apply([r.to_f32(), g.to_f32(), b.to_f32()]);
            rounded.push(out.map(|v| v != 0.0 && v != 1.0));
            let [r, g, b] = out;
            source.extend([f16::from_f32(r), f16::from_f32(g), f16::from_f32(b), one]);
        }
    }
    let fold = |src: &[f16]| -> Vec<f32> {
        let mut mixed = backdrop.clone();
        aurora_render::composite_layer_into(&mut mixed, src, opacity, mode);
        mixed.iter().map(|v| v.to_f32()).collect()
    };
    let base = fold(&source);
    let mut sensitivity = vec![0.0_f32; base.len() / 4];
    for dr in -1..=1 {
        for dg in -1..=1 {
            for db in -1..=1 {
                let moved: Vec<f16> = source
                    .chunks_exact(4)
                    .zip(&rounded)
                    .flat_map(|(t, r)| {
                        let step = |v: f16, d: i32, free: bool| {
                            if free { f16_step(v, d) } else { v }
                        };
                        [
                            step(t[0], dr, r[0]),
                            step(t[1], dg, r[1]),
                            step(t[2], db, r[2]),
                            t[3],
                        ]
                    })
                    .collect();
                let out = fold(&moved);
                for (i, s) in sensitivity.iter_mut().enumerate() {
                    for c in 0..3 {
                        *s = s.max((out[i * 4 + c] - base[i * 4 + c]).abs());
                    }
                }
            }
        }
    }
    let limit = bound - 2.0 / 2048.0;
    let ill = sensitivity.iter().map(|s| *s > limit).collect();
    Conditioning {
        ill,
        sensitivity,
        base,
    }
}

/// [`gpu_matches_cpu`] with the texels that are ill-conditioned with respect
/// to the adjustment's source colour (see [`source_conditioning`]) held to
/// `bound + 2 * sensitivity`, at most `max_ill_percent` of the tile. The
/// layer `id` is the adjustment under test.
pub(crate) fn gpu_matches_cpu_source_conditioned(
    context: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    id: aurora_doc::LayerId,
    pass: Pass,
    passes: u64,
    bound: f32,
    what: &str,
    max_ill_percent: usize,
) -> (Vec<f32>, f32) {
    let visible = layers.visible(id).unwrap_or(true);
    ok(layers.set_visible(id, false));
    let without = cpu_tile(layers, store);
    ok(layers.set_visible(id, visible));
    let conditioning = source_conditioning(layers, id, &without, bound);
    gpu_matches_cpu_excluding_capped(
        context,
        compositor,
        layers,
        store,
        pass,
        passes,
        bound,
        what,
        Some(&conditioning),
        max_ill_percent,
    )
}

/// [`gpu_matches_cpu`] with the texels `exclude` marks ill-conditioned
/// held to `bound + 2 * sensitivity` instead of `bound` (and the count of
/// them printed and bounded to 1 % of the tile).
pub(crate) fn gpu_matches_cpu_excluding(
    context: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    pass: Pass,
    passes: u64,
    bound: f32,
    what: &str,
    exclude: Option<&Conditioning>,
) -> (Vec<f32>, f32) {
    gpu_matches_cpu_excluding_capped(
        context, compositor, layers, store, pass, passes, bound, what, exclude, 1,
    )
}

/// [`gpu_matches_cpu_excluding`] with the share of texels that may be
/// ill-conditioned set by `max_ill_percent` (1 by default). A steep or
/// binary source under a dividing blend mode makes a large share of the
/// tile sensitive (see [`source_conditioning`]); the cap keeps "mostly
/// excluded" from passing as "tested".
pub(crate) fn gpu_matches_cpu_excluding_capped(
    context: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    pass: Pass,
    passes: u64,
    bound: f32,
    what: &str,
    exclude: Option<&Conditioning>,
    max_ill_percent: usize,
) -> (Vec<f32>, f32) {
    assert!(
        crate::document_qualifies_for_gpu_compositing(layers),
        "{what}: setup, must take the GPU path"
    );
    let _ = take_counts();
    let Some(gpu) = gpu_tile(context, compositor, layers, store) else {
        unreachable!("{what}: the GPU tile fell back");
    };
    assert_eq!(
        take_counts(),
        pass.expected(passes),
        "{what}: GPU passes recorded -- fewer means a silent CPU fallback"
    );
    let cpu = cpu_tile(layers, store);
    assert_eq!(gpu.len(), cpu.len(), "{what}");
    let mut worst = 0.0_f32;
    let mut worst_excluded = 0.0_f32;
    for (i, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
        let d = (g - c).abs();
        let texel = i / 4;
        let (limit, excluded) = match exclude {
            Some(e) if e.ill.get(texel).copied().unwrap_or(false) => (
                bound + 2.0 * e.sensitivity.get(texel).copied().unwrap_or(0.0),
                true,
            ),
            _ => (bound, false),
        };
        assert!(
            d <= limit,
            "{what}: texel ({}, {}) channel {}: GPU {g} vs CPU {c} (bound {limit}, excluded {excluded})",
            texel % aurora_tile::TILE as usize,
            texel / aurora_tile::TILE as usize,
            i % 4
        );
        if excluded {
            worst_excluded = worst_excluded.max(d);
        } else {
            worst = worst.max(d);
        }
    }
    if let Some(e) = exclude {
        assert!(
            e.base == cpu,
            "{what}: the conditioning probe must reproduce the CPU tile exactly"
        );
        let count = e.ill.iter().filter(|b| **b).count();
        println!(
            "{what}: {count} of {} texels ill-conditioned (sensitivity > {:e}), their worst {worst_excluded:e}",
            e.ill.len(),
            conditioning_limit(bound)
        );
        assert!(
            count * 100 < e.ill.len() * max_ill_percent,
            "{what}: {count} ill-conditioned texels is more than {max_ill_percent} % of the tile"
        );
    }
    println!("{what}: worst |GPU - CPU| = {worst:e} (bound {bound:e})");
    (cpu, worst)
}

pub(crate) const GPU_MODES: [BlendMode; 19] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::Screen,
    BlendMode::Difference,
    BlendMode::LinearDodge,
    BlendMode::LinearBurn,
    BlendMode::ColorBurn,
    BlendMode::ColorDodge,
    BlendMode::Overlay,
    BlendMode::HardLight,
    BlendMode::LinearLight,
    BlendMode::VividLight,
    BlendMode::HardMix,
    BlendMode::PinLight,
    BlendMode::SoftLight,
    BlendMode::Subtract,
    BlendMode::Divide,
];

const STEEP_LEVELS_CASES: [&str; 2] = ["gamma min 0.10", "narrow input span"];

/// Hue/Saturation cases whose slope is also unbounded on a translucent
/// backdrop: saturation +99/+100 divides by `1 - k` (a factor of 100 at
/// +99), so a one-step `Cb` difference from the GPU's `straight_backdrop`
/// division flips a pixel between grey and coloured. Held to the opaque
/// tolerance only. The macOS and Windows CI adapters (Apple Paravirtual,
/// WARP) rounded differently from the RTX 3090 and exposed this in 0.206.0.
const STEEP_HUE_SAT_CASES: [&str; 3] = ["saturation +99", "saturation +100", "extremes"];

/// Blend terms that divide or threshold (`ColorBurn`, `ColorDodge`,
/// `VividLight`, `Divide`, `HardMix`) amplify a one-`f16`-step difference
/// in `Cs` (the same class as the Curves tests' derived per-mode bound of
/// about 6-8 steps on the RTX 3090; Windows' WARP adapter measured 9 steps
/// for Hue/Saturation under `ColorDodge`, 13 under `VividLight`): 20 steps of `2^-11` instead of
/// `2 * f16::EPSILON`.
pub(crate) fn mode_bound(mode: BlendMode) -> f32 {
    match mode {
        BlendMode::ColorBurn
        | BlendMode::ColorDodge
        | BlendMode::VividLight
        | BlendMode::Divide
        | BlendMode::HardMix => 20.0 / 2048.0,
        _ => tolerance(),
    }
}

// -- Levels parameter sets ------------------------------------------------

fn levels_cases() -> Vec<(&'static str, aurora_doc::Adjustment)> {
    let id = aurora_core::LevelsChannel::identity();
    vec![
        ("stretch", levels_master(record(0.1, 0.9, 1.0, 0.0, 1.0))),
        ("gamma 2.2", levels_master(record(0.0, 1.0, 2.2, 0.0, 1.0))),
        (
            "gamma min 0.10",
            levels_master(record(0.0, 1.0, 0.10, 0.0, 1.0)),
        ),
        (
            "gamma max 9.99",
            levels_master(record(0.0, 1.0, 9.99, 0.0, 1.0)),
        ),
        ("gamma 0.5", levels_master(record(0.0, 1.0, 0.5, 0.0, 1.0))),
        (
            "output range",
            levels_master(record(0.0, 1.0, 1.0, 0.2, 0.8)),
        ),
        (
            "inverted output",
            levels_master(record(0.0, 1.0, 1.0, 1.0, 0.0)),
        ),
        (
            "everything",
            levels_master(record(0.05, 0.95, 1.7, 0.1, 0.9)),
        ),
        (
            "narrow input span",
            levels_master(record(0.4, 0.4 + 1.0 / 255.0, 1.0, 0.0, 1.0)),
        ),
        (
            "per channel + master",
            levels(
                record(0.0, 1.0, 1.4, 0.0, 1.0),
                [
                    record(0.1, 0.8, 1.0, 0.0, 1.0),
                    record(0.0, 1.0, 0.6, 0.0, 1.0),
                    record(0.0, 1.0, 1.0, 0.3, 1.0),
                ],
            ),
        ),
        (
            "red only",
            levels(id, [record(0.2, 0.7, 1.3, 0.0, 1.0), id, id]),
        ),
    ]
}

fn hue_sat_cases() -> Vec<(&'static str, aurora_doc::Adjustment)> {
    vec![
        ("hue +60", hue_sat(60.0, 0.0, 0.0)),
        ("hue -120", hue_sat(-120.0, 0.0, 0.0)),
        ("hue +180", hue_sat(180.0, 0.0, 0.0)),
        ("hue -180", hue_sat(-180.0, 0.0, 0.0)),
        ("hue +1", hue_sat(1.0, 0.0, 0.0)),
        ("saturation -100", hue_sat(0.0, -100.0, 0.0)),
        ("saturation -40", hue_sat(0.0, -40.0, 0.0)),
        ("saturation +30", hue_sat(0.0, 30.0, 0.0)),
        ("saturation +50", hue_sat(0.0, 50.0, 0.0)),
        ("saturation +99", hue_sat(0.0, 99.0, 0.0)),
        ("saturation +100", hue_sat(0.0, 100.0, 0.0)),
        ("lightness -100", hue_sat(0.0, 0.0, -100.0)),
        ("lightness -50", hue_sat(0.0, 0.0, -50.0)),
        ("lightness +50", hue_sat(0.0, 0.0, 50.0)),
        ("lightness +100", hue_sat(0.0, 0.0, 100.0)),
        ("all three", hue_sat(75.0, 35.0, -20.0)),
        ("extremes", hue_sat(-180.0, 100.0, 100.0)),
        ("extremes low", hue_sat(180.0, -100.0, -100.0)),
        ("colorize", colorize(30.0, 80.0, 0.0)),
        ("colorize hue 0", colorize(0.0, 100.0, 0.0)),
        ("colorize hue 360", colorize(360.0, 50.0, 0.0)),
        ("colorize saturation 0", colorize(200.0, 0.0, 0.0)),
        ("colorize lightness -100", colorize(120.0, 100.0, -100.0)),
        ("colorize lightness +100", colorize(120.0, 100.0, 100.0)),
        ("colorize lightness +30", colorize(280.0, 60.0, 30.0)),
        (
            "colorize ignores master",
            aurora_doc::Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
                90.0, 100.0, 100.0, true, 30.0, 80.0, 0.0,
            ))),
        ),
    ]
}

/// The pass (Levels or Hue/Saturation) of the case list `which`.
fn cases_of(pass: Pass) -> Vec<(&'static str, aurora_doc::Adjustment)> {
    match pass {
        Pass::Levels => levels_cases(),
        Pass::HueSaturation => hue_sat_cases(),
        Pass::Vibrance
        | Pass::Exposure
        | Pass::PhotoFilter
        | Pass::BlackWhite
        | Pass::ColorBalance => crate::gpu_photo_adjustment_tests::cases_of(pass),
        Pass::Invert
        | Pass::Threshold
        | Pass::Posterize
        | Pass::BrightnessContrast
        | Pass::ChannelMixer
        | Pass::SelectiveColor
        | Pass::GradientMap => crate::gpu_newest_adjustment_tests::cases_of(pass),
    }
}

/// Runs every case of `pass` over `pattern` at `opacity` and `mode`,
/// returning the worst difference. The adjustment must change the
/// composite (or be an identity-like case).
pub(crate) fn run_cases(
    pass: Pass,
    pattern: impl Fn(u32, u32) -> [f32; 4] + Copy,
    opacity: f32,
    mode: BlendMode,
    bound: f32,
    label: &str,
) -> f32 {
    let Some(context) = real_gpu_context() else {
        return 0.0;
    };
    let mut worst = 0.0_f32;
    for (name, adjustment) in cases_of(pass) {
        // Slopes of 10 (gamma 0.10) and 255 (a one-level input span) scale a
        // translucent backdrop's one-step `Cb` difference past any fixed
        // bound; they are held to the opaque tolerance above instead.
        if bound > tolerance()
            && (STEEP_LEVELS_CASES.contains(&name)
                || STEEP_HUE_SAT_CASES.contains(&name)
                || crate::gpu_photo_adjustment_tests::STEEP_CASES.contains(&name)
                || crate::gpu_newest_adjustment_tests::STEEP_CASES.contains(&name))
        {
            continue;
        }
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, pattern);
        let without = cpu_tile(&layers, &mut store);
        let id = add_adjustment(&mut layers, adjustment);
        ok(layers.set_opacity(id, opacity));
        ok(layers.set_blend_mode(id, mode));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let what = format!("{label} / {name}");
        // A translucent run (bound above the opaque one): measure which
        // texels the CPU result is too sensitive to a one-step backdrop
        // difference for any adapter-independent bound to hold.
        let conditioning = (bound > tolerance())
            .then(|| conditioning_of(&layers, &mut store, id, &without, bound));
        let (cpu, w) = gpu_matches_cpu_excluding(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            1,
            bound,
            &what,
            conditioning.as_ref(),
        );
        assert!(
            max_diff(&cpu, &without) > 0.002,
            "{what}: the adjustment must really change the composite"
        );
        worst = worst.max(w);
    }
    worst
}

#[test]
fn gpu_levels_matches_the_cpu_on_random_and_edge_tiles() {
    for (label, pattern) in [
        ("random", random_opaque as fn(u32, u32) -> [f32; 4]),
        ("edge", edge_opaque),
        ("ramp", ramp),
    ] {
        let worst = run_cases(
            Pass::Levels,
            pattern,
            1.0,
            BlendMode::Normal,
            tolerance(),
            label,
        );
        println!("levels opaque {label}: worst {worst:e}");
    }
    // Partial opacity, so the lerp with the backdrop is observable.
    let worst = run_cases(
        Pass::Levels,
        random_opaque,
        0.35,
        BlendMode::Normal,
        tolerance(),
        "random @35%",
    );
    println!("levels opaque random @35%: worst {worst:e}");
}

#[test]
fn gpu_hue_saturation_matches_the_cpu_on_random_and_edge_tiles() {
    for (label, pattern) in [
        ("random", random_opaque as fn(u32, u32) -> [f32; 4]),
        ("edge", edge_opaque),
        ("ramp", ramp),
    ] {
        let worst = run_cases(
            Pass::HueSaturation,
            pattern,
            1.0,
            BlendMode::Normal,
            tolerance(),
            label,
        );
        println!("hue/sat opaque {label}: worst {worst:e}");
    }
    let worst = run_cases(
        Pass::HueSaturation,
        random_opaque,
        0.35,
        BlendMode::Normal,
        tolerance(),
        "random @35%",
    );
    println!("hue/sat opaque random @35%: worst {worst:e}");
}

/// Translucent backdrops: moderate-slope cases only (see
/// [`translucent_tolerance`]); the +99/+100 saturation, +/-100 lightness
/// extremes and Colorize are covered on opaque backdrops above and on a
/// translucent one in the full-alpha-band test below.
#[test]
fn gpu_levels_matches_the_cpu_on_translucent_backdrops() {
    let worst = run_cases(
        Pass::Levels,
        random_translucent,
        1.0,
        BlendMode::Normal,
        translucent_tolerance(),
        "random translucent",
    );
    println!("levels translucent random: worst {worst:e}");
    let worst = run_cases(
        Pass::Levels,
        ramp_translucent,
        0.6,
        BlendMode::Normal,
        translucent_tolerance(),
        "ramp translucent @60%",
    );
    println!("levels translucent ramp @60%: worst {worst:e}");
}

#[test]
fn gpu_hue_saturation_matches_the_cpu_on_translucent_backdrops() {
    let worst = run_cases(
        Pass::HueSaturation,
        random_translucent,
        1.0,
        BlendMode::Normal,
        translucent_tolerance(),
        "random translucent",
    );
    println!("hue/sat translucent random: worst {worst:e}");
    let worst = run_cases(
        Pass::HueSaturation,
        ramp_translucent,
        0.6,
        BlendMode::Normal,
        translucent_tolerance(),
        "ramp translucent @60%",
    );
    println!("hue/sat translucent ramp @60%: worst {worst:e}");
}

/// Every admitted blend mode, one adjustment each, on the smooth ramp (as
/// the Curves test): the GPU must differ from Normal for every other mode,
/// or a dropped mode would pass.
/// Pass and mode pairs whose blend term is the source itself (see
/// [`every_mode`]).
fn coincides_with_normal(pass: Pass, mode: BlendMode) -> bool {
    match pass {
        Pass::Invert => mode == BlendMode::LinearLight,
        Pass::Threshold => matches!(
            mode,
            BlendMode::HardLight
                | BlendMode::LinearLight
                | BlendMode::VividLight
                | BlendMode::PinLight
        ),
        _ => false,
    }
}

pub(crate) fn every_mode(
    pass: Pass,
    adjustment: aurora_doc::Adjustment,
    pattern: fn(u32, u32) -> [f32; 4],
) {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, pattern);
    let id = add_adjustment(&mut layers, adjustment);
    ok(layers.set_opacity(id, 0.75));
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    let mut normal = Vec::new();
    let mut worst = 0.0_f32;
    for mode in GPU_MODES {
        ok(layers.set_blend_mode(id, mode));
        let what = format!("{pass:?} {mode:?}");
        if pass == Pass::Invert && mode == BlendMode::HardMix {
            // HardMix switches at Cb + Cs = 1, and an inversion has Cs =
            // 1 - Cb: EVERY texel sits on the switch (up to rounding), so
            // no value or flip bound exists. Only the dispatch is checked.
            let _ = take_counts();
            let Some(_) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
                unreachable!("{what}: fell back");
            };
            assert_eq!(take_counts(), pass.expected(1), "{what}: pass recorded");
            continue;
        }
        let (cpu, w) = if pass.is_newest() {
            // The seven 0.250.0 kinds meet the dividing and thresholding
            // blend terms with steep or binary sources: the texels whose
            // result moves by more than the bound allows for one `f16` step
            // of the source colour (see `source_conditioning`) are held to
            // `bound + 2 * sensitivity`, every other texel to the tight
            // bound. The share of ill-conditioned texels is capped per mode.
            // It is computed from CPU values alone, so it is the same on
            // every adapter: measured on the RTX 3090 at most 8 % (Invert
            // under ColorDodge, 5.7 % under VividLight; ColorBurn, Divide and
            // the rest below 1 %: VividLight, ColorBurn, ColorDodge and
            // Divide divide by a quantity that a step of the source can bring
            // near zero), HardMix 0.14 % (it flips at its switch only).
            let max_ill_percent = match mode {
                BlendMode::ColorBurn
                | BlendMode::ColorDodge
                | BlendMode::VividLight
                | BlendMode::Divide => 12,
                // HardMix (0.14 % measured) and the rest: 1 %.
                _ => 1,
            };
            gpu_matches_cpu_source_conditioned(
                &context,
                &mut compositor,
                &mut layers,
                &mut store,
                id,
                pass,
                1,
                mode_bound(mode),
                &what,
                max_ill_percent,
            )
        } else {
            if mode == BlendMode::HardMix {
                // HardMix thresholds `VividLight` at 0.5: a one-`f16`-step
                // difference in `Cs` near the threshold flips a channel between
                // 0 and 1 (disclosed since 0.115.1), so no value bound exists.
                // Hold it to a bound on how many channels may flip instead.
                let _ = take_counts();
                let Some(gpu) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
                    unreachable!("{what}: fell back");
                };
                assert_eq!(take_counts(), pass.expected(1), "{what}: pass recorded");
                let cpu = cpu_tile(&layers, &mut store);
                let flipped = gpu
                    .iter()
                    .zip(&cpu)
                    .filter(|(g, c)| (*g - *c).abs() > mode_bound(mode))
                    .count();
                println!("{what}: {flipped} of {} channels flipped", gpu.len());
                assert!(
                    flipped * 1000 <= gpu.len(),
                    "{what}: {flipped} channels flipped"
                );
                continue;
            }
            gpu_matches_cpu(
                &context,
                &mut compositor,
                &layers,
                &mut store,
                pass,
                1,
                mode_bound(mode),
                &what,
            )
        };
        worst = worst.max(w);
        if mode == BlendMode::Normal {
            normal = cpu;
        } else if coincides_with_normal(pass, mode) {
            // For a binary source (Threshold: 0 or 1) HardLight, LinearLight,
            // VividLight and PinLight return the source itself, and
            // LinearLight(Cb, 1 - Cb) = clamp(Cb + 2 (1 - Cb) - 1) = 1 - Cb
            // for Invert: these combinations ARE Normal, mathematically, so
            // they cannot differ from it (the comparison against the CPU
            // above still holds).
            assert!(
                max_diff(&cpu, &normal) < 0.01,
                "{what}: this mode of this pass is Normal"
            );
        } else {
            assert!(
                max_diff(&cpu, &normal) > 0.01,
                "{what}: must differ from Normal, or a dropped mode would pass"
            );
        }
    }
    println!("{pass:?} every mode: worst {worst:e}");
}

#[test]
fn gpu_levels_matches_the_cpu_at_every_gpu_blend_mode() {
    every_mode(
        Pass::Levels,
        levels(
            record(0.0, 1.0, 1.4, 0.0, 1.0),
            [
                record(0.1, 0.9, 1.0, 0.0, 1.0),
                aurora_core::LevelsChannel::identity(),
                record(0.0, 1.0, 1.0, 0.2, 1.0),
            ],
        ),
        ramp,
    );
}

#[test]
fn gpu_hue_saturation_matches_the_cpu_at_every_gpu_blend_mode() {
    every_mode(Pass::HueSaturation, hue_sat(100.0, 30.0, -15.0), ramp);
    every_mode(Pass::HueSaturation, colorize(210.0, 70.0, 10.0), ramp);
}

/// The same modes on random tiles, where the blend terms meet saturated
/// and degenerate operands: `Divide`, `ColorBurn`, `ColorDodge` and
/// `VividLight` amplify a one-step input difference, so only the stable
/// modes are held to the opaque tolerance here.
#[test]
fn gpu_adjustments_match_the_cpu_at_the_stable_modes_on_random_tiles() {
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
    for (pass, adjustment) in [
        (
            Pass::Levels,
            levels_master(record(0.05, 0.95, 1.7, 0.1, 0.9)),
        ),
        (Pass::HueSaturation, hue_sat(75.0, 35.0, -20.0)),
    ] {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, adjustment);
        ok(layers.set_opacity(id, 0.8));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        for mode in stable {
            ok(layers.set_blend_mode(id, mode));
            let what = format!("random {pass:?} {mode:?}");
            let _ = gpu_matches_cpu(
                &context,
                &mut compositor,
                &layers,
                &mut store,
                pass,
                1,
                tolerance(),
                &what,
            );
        }
    }
}

#[test]
fn gpu_adjustments_apply_opacity_times_fill() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (pass, adjustment) in [
        (Pass::Levels, levels_master(record(0.1, 0.9, 1.6, 0.0, 1.0))),
        (Pass::HueSaturation, hue_sat(90.0, 40.0, 0.0)),
    ] {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, adjustment);
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

/// Out-of-gamut and negative backdrops (`straight_backdrop` is unclamped).
/// A non-identity adjustment clamps its input to [0, 1] on both paths; an
/// identity one under a blend mode passes the value through unchanged.
#[test]
fn gpu_adjustments_handle_out_of_gamut_backdrops_like_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let identity_levels = aurora_doc::Adjustment::Levels(aurora_core::LevelsParams::identity());
    let identity_hs =
        aurora_doc::Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity());
    let cases = [
        (
            Pass::Levels,
            levels_master(record(0.1, 0.9, 1.6, 0.0, 1.0)),
            BlendMode::Normal,
        ),
        (Pass::Levels, identity_levels, BlendMode::Multiply),
        (
            Pass::HueSaturation,
            hue_sat(90.0, 40.0, 10.0),
            BlendMode::Normal,
        ),
        (
            Pass::HueSaturation,
            colorize(40.0, 60.0, 0.0),
            BlendMode::Normal,
        ),
        (Pass::HueSaturation, identity_hs, BlendMode::Multiply),
    ];
    for (i, (pass, adjustment, mode)) in cases.into_iter().enumerate() {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, out_of_gamut);
        let id = add_adjustment(&mut layers, adjustment);
        ok(layers.set_blend_mode(id, mode));
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        // Out-of-gamut operands push blend terms far out of range; compare
        // relative to the CPU value (f16 has 11 significant bits), with the
        // absolute tolerance as a floor.
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        let _ = take_counts();
        let Some(gpu) = gpu_tile(&context, &mut compositor, &layers, &mut store) else {
            unreachable!("case {i}: fell back");
        };
        assert_eq!(take_counts(), pass.expected(1), "case {i}: pass recorded");
        let cpu = cpu_tile(&layers, &mut store);
        let mut worst = 0.0_f32;
        for (g, c) in gpu.iter().zip(&cpu) {
            let allowed = tolerance().max(c.abs() * 4.0 / 2048.0);
            assert!(
                (g - c).abs() <= allowed,
                "case {i} ({pass:?} {mode:?}): GPU {g} vs CPU {c}"
            );
            worst = worst.max((g - c).abs() / allowed);
        }
        println!("out of gamut case {i} ({pass:?} {mode:?}): worst fraction of allowance {worst}");
    }
}

/// Identity adjustments: under `Normal` the CPU skips the layer and the GPU
/// records no pass (bit-identical to the layer-less composite); under any
/// other mode the pass runs and passes the value through exactly.
#[test]
fn gpu_identity_adjustments_record_no_pass_under_normal_and_one_otherwise() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (pass, adjustment) in [
        (
            Pass::Levels,
            aurora_doc::Adjustment::Levels(aurora_core::LevelsParams::identity()),
        ),
        (
            Pass::HueSaturation,
            aurora_doc::Adjustment::HueSaturation(aurora_core::HueSaturationParams::identity()),
        ),
    ] {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_translucent);
        let without = cpu_tile(&layers, &mut store);
        let id = add_adjustment(&mut layers, adjustment);
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let (cpu, _) = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            0,
            tolerance(),
            &format!("{pass:?} identity @Normal"),
        );
        assert_eq!(cpu, without, "the CPU skips an identity layer under Normal");
        ok(layers.set_blend_mode(id, BlendMode::Multiply));
        let _ = gpu_matches_cpu(
            &context,
            &mut compositor,
            &layers,
            &mut store,
            pass,
            1,
            translucent_tolerance(),
            &format!("{pass:?} identity @Multiply"),
        );
    }
}

/// A document with a pixel layer, an adjustment of each kind (and Curves),
/// a blended pixel layer between them and one on top composites on the GPU
/// and equals the CPU composite.
#[test]
fn a_document_with_levels_and_hue_saturation_layers_composites_on_the_gpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let _ = pattern_layer(&mut layers, &mut store, random_opaque);
    let lv = add_adjustment(
        &mut layers,
        levels_master(record(0.05, 0.95, 1.5, 0.0, 1.0)),
    );
    ok(layers.set_opacity(lv, 0.9));
    let mid = pattern_layer(&mut layers, &mut store, ramp_translucent);
    ok(layers.set_blend_mode(mid, BlendMode::Multiply));
    ok(layers.set_opacity(mid, 0.7));
    let hs = add_adjustment(&mut layers, hue_sat(40.0, 20.0, -10.0));
    ok(layers.set_blend_mode(hs, BlendMode::Overlay));
    ok(layers.set_opacity(hs, 0.8));
    let lv2 = add_adjustment(&mut layers, colorize(200.0, 50.0, 0.0));
    ok(layers.set_opacity(lv2, 0.3));
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
        [0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        "Levels once, Hue/Saturation twice"
    );
    let cpu = cpu_tile(&layers, &mut store);
    let worst = max_diff(&gpu, &cpu);
    println!("mixed document: worst |GPU - CPU| = {worst:e}");
    assert!(worst <= translucent_tolerance(), "worst {worst}");
}

/// Hue/Saturation with colour ranges stays on the CPU; a ranges layer whose
/// ranges are neutral (or that is Colorize) is the master path and runs on
/// the GPU.
#[test]
fn hue_saturation_with_active_ranges_stays_on_the_cpu_and_neutral_ranges_do_not() {
    let base = ok(aurora_core::HueSaturationParams::new(
        20.0, 10.0, 0.0, false, 0.0, 25.0, 0.0,
    ));
    let plain = aurora_core::HueSaturationRangesParams::plain(base);
    // A range that adjusts something.
    let reds = ok(aurora_core::HueRange::new(
        aurora_core::HueRange::standard(0).edges(),
        30.0,
        20.0,
        0.0,
    ));
    let Some(ranged) = plain.with_range(0, reds) else {
        unreachable!("index 0 exists");
    };
    // The same, with Colorize on: ranges are ignored.
    let colorized_base = ok(aurora_core::HueSaturationParams::new(
        0.0, 0.0, 0.0, true, 120.0, 60.0, 0.0,
    ));
    let colorized = ranged.with_base(colorized_base);
    // Custom edges that adjust nothing.
    let quiet = ok(aurora_core::HueRange::new(
        [10.0, 20.0, 30.0, 40.0],
        0.0,
        0.0,
        0.0,
    ));
    let Some(neutral_edges) = plain.with_range(1, quiet) else {
        unreachable!("index 1 exists");
    };

    let Some(context) = real_gpu_context() else {
        return;
    };
    let cases = [
        ("ranged", ranged, false),
        ("colorize with ranges", colorized, true),
        ("neutral custom edges", neutral_edges, true),
        ("plain", plain, true),
    ];
    for (name, params, gpu_ok) in cases {
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let _ = add_adjustment(
            &mut layers,
            aurora_doc::Adjustment::HueSaturationRanges(params),
        );
        assert_eq!(
            crate::document_qualifies_for_gpu_compositing(&layers),
            gpu_ok,
            "{name}: predicate"
        );
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        let _ = take_counts();
        let gpu = gpu_tile(&context, &mut compositor, &layers, &mut store);
        if gpu_ok {
            let Some(gpu) = gpu else {
                unreachable!("{name}: should run on the GPU");
            };
            assert_eq!(take_counts(), Pass::HueSaturation.expected(1), "{name}");
            let cpu = cpu_tile(&layers, &mut store);
            assert!(max_diff(&gpu, &cpu) <= tolerance(), "{name}");
        } else {
            assert!(
                gpu.is_none(),
                "{name}: the tile must fall back, not drop it"
            );
            assert_eq!(take_counts(), [0; COUNTERS], "{name}: no pass recorded");
        }
    }
}

/// Masks, `Dissolve`, `Exclusion` and the non-separable modes keep both
/// new passes on the CPU, and a group refuses the whole document.
#[test]
fn unsupported_levels_and_hue_saturation_layers_fall_back_to_the_cpu() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for adjustment in [
        levels_master(record(0.1, 0.9, 1.5, 0.0, 1.0)),
        hue_sat(30.0, 0.0, 0.0),
    ] {
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
                !crate::document_qualifies_for_gpu_compositing(&layers),
                "{refusal}: the document stays on the CPU"
            );
            let mut compositor = aurora_render::TileCompositor::new(context.device());
            let _ = take_counts();
            assert!(
                gpu_tile(&context, &mut compositor, &layers, &mut store).is_none(),
                "{refusal}: the tile must fall back, not drop the layer"
            );
            assert_eq!(take_counts(), [0; COUNTERS], "{refusal}");
        }
        // A disabled mask is ignored on both paths.
        let (_dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, adjustment.clone());
        ok(layers.add_mask(id, QUAD));
        ok(layers.set_mask_enabled(id, false));
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        // A group refuses, an adjustment inside it or not.
        let (_dir, mut store) = self::store();
        let mut grouped = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut grouped, &mut store, random_opaque);
        let group = ok(grouped.add_group("Group 1", None));
        let _ = ok(grouped.add_adjustment_layer_at("inner", adjustment, Some(group), 0));
        assert!(!crate::document_qualifies_for_gpu_compositing(&grouped));
    }
}

/// A hidden Levels / Hue/Saturation layer never disqualifies, and a visible
/// one now qualifies at the root.
#[test]
fn visible_levels_and_hue_saturation_layers_qualify_and_hidden_ones_are_ignored() {
    let (_dir, mut store) = store();
    for adjustment in [
        levels_master(record(0.1, 0.9, 1.5, 0.0, 1.0)),
        hue_sat(30.0, 0.0, 0.0),
    ] {
        let mut layers = aurora_doc::LayerTree::new();
        let _ = pattern_layer(&mut layers, &mut store, random_opaque);
        let id = add_adjustment(&mut layers, adjustment);
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
        ok(layers.set_visible(id, false));
        assert!(crate::document_qualifies_for_gpu_compositing(&layers));
    }
}

/// The CPU-side and render-side layouts agree.
#[test]
fn the_packed_parameter_layouts_agree_between_filters_and_render() {
    assert_eq!(
        aurora_filters::LEVELS_PACKED_LEN,
        aurora_render::LEVELS_GPU_PARAMS_LEN
    );
    assert_eq!(
        aurora_filters::HUE_SATURATION_PACKED_LEN,
        aurora_render::HUE_SATURATION_GPU_PARAMS_LEN
    );
}

/// `packed_for_gpu` carries exactly the record fields, in the shader's
/// order: red, green, blue, master.
#[test]
fn levels_packed_for_gpu_is_the_records_in_shader_order() {
    let params = aurora_core::LevelsParams {
        master: record(0.1, 0.9, 2.0, 0.2, 0.8),
        red: record(0.3, 0.7, 1.0, 0.0, 1.0),
        green: aurora_core::LevelsChannel::identity(),
        blue: record(0.0, 1.0, 0.5, 0.0, 1.0),
    };
    let packed = aurora_filters::LevelsLut::new(&params).packed_for_gpu();
    assert_eq!(
        &packed[0..8],
        &[1.0, 0.3, 0.7, 1.0, 0.0, 1.0, 0.0, 0.0],
        "red"
    );
    assert_eq!(&packed[8..16], &[0.0; 8], "green is an identity record");
    assert_eq!(
        &packed[16..24],
        &[1.0, 0.0, 1.0, 2.0, 0.0, 1.0, 1.0, 0.0],
        "blue"
    );
    assert_eq!(
        &packed[24..32],
        &[1.0, 0.1, 0.9, 0.5, 0.2, 0.8, 1.0, 0.0],
        "master"
    );
}

#[test]
fn hue_saturation_packed_for_gpu_is_the_settings_in_shader_order() {
    let p = ok(aurora_core::HueSaturationParams::new(
        -45.0, 30.0, -60.0, false, 200.0, 80.0, 20.0,
    ));
    let packed = aurora_filters::HueSaturationOp::new(&p).packed_for_gpu();
    assert_eq!(packed, Some([0.0, 0.0, -45.0, 0.3, -0.6, 200.0, 0.8, 0.2]),);
    let identity =
        aurora_filters::HueSaturationOp::new(&aurora_core::HueSaturationParams::identity());
    assert_eq!(identity.packed_for_gpu().map(|v| v[0]), Some(1.0));
}

/// The 0.211.1 finding, headless (no adapter needed): texel (198, 20) of
/// `random_translucent` is a light, barely coloured pixel
/// (`l` ~ 0.96, `2 - hi - lo` ~ 0.08), and under Hue/Saturation lightness
/// -50 one `f16` step of its premultiplied backdrop moves the CPU's own
/// result by up to ~0.03. The macOS CI adapter's 0.23547363 at channel 1
/// (CPU 0.22680664) is exactly the CPU result for the blue channel one
/// step lower, i.e. a one-step backdrop difference, not a shader fault.
#[test]
fn a_light_pixel_amplifies_a_one_step_backdrop_difference_under_lightness() {
    use half::f16;
    let op = aurora_filters::HueSaturationOp::new(&ok(aurora_core::HueSaturationParams::new(
        0.0, 0.0, -50.0, false, 0.0, 25.0, 0.0,
    )));
    let pixel = random_translucent(198, 20);
    let alpha = f16::from_f32(pixel[3]);
    let premultiplied =
        [0, 1, 2].map(|c| f16::from_f32(f16::from_f32(pixel[c]).to_f32() * alpha.to_f32()));
    let green = |steps: [i32; 3]| -> f32 {
        let straight = [0, 1, 2].map(|c| {
            let v = f16_step(premultiplied[c], steps[c]);
            f16::from_f32(v.to_f32() / alpha.to_f32()).to_f32()
        });
        let out = op.apply(straight)[1];
        let stored = f16::from_f32(f16::from_f32(out).to_f32() * alpha.to_f32());
        f16::from_f32(stored.to_f32() / alpha.to_f32()).to_f32()
    };
    assert_eq!(green([0, 0, 0]), 0.226_806_64, "the CPU reference");
    assert_eq!(green([0, 0, -1]), 0.235_473_63, "macOS CI's GPU value");
    let mut spread = 0.0_f32;
    for dr in -1..=1 {
        for dg in -1..=1 {
            for db in -1..=1 {
                spread = spread.max((green([dr, dg, db]) - 0.226_806_64).abs());
            }
        }
    }
    assert!(spread > 0.02, "one step moves the result by {spread}");
    assert!(spread > 3.0 * translucent_tolerance());
}
