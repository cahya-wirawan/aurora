//! The Hue/Saturation adjustment's pixel math (0.203.0).
//!
//! [`HueSaturationOp`] applies an [`aurora_core::HueSaturationParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded values, alpha never
//! an input or output, no 8-bit intermediate (invariant 7.3.1b). The
//! formulas are documented on the parameter type (`aurora-core`'s
//! `hue_saturation` module): HSL with `l = (max + min) / 2`, the master
//! sliders as a hue rotation, a saturation scale and a lightness blend
//! towards black or white, and Colorize as a fixed hue and saturation with
//! the same lightness rule.
//!
//! # Decisions, with reasons
//!
//! - **No lookup table.** The map is three-dimensional (hue depends on all
//!   channels), so it is computed per pixel, in `f32`. It reads nothing
//!   but the pixel: deterministic, and tile boundaries cannot show.
//! - **An identity adjustment is skipped** ([`HueSaturationOp::is_identity`]),
//!   an exact passthrough even for an out-of-gamut value. A non-identity
//!   one clamps its input to `[0, 1]` (`NaN` reads as `0`) and returns
//!   values in `[0, 1]`.
//! - **A grey pixel (`max == min`) has no hue and no saturation**: the
//!   master sliders touch only its lightness; Colorize gives it the
//!   Colorize hue and saturation.
//! - **GPU (0.206.0)** for the master / Colorize path only:
//!   `aurora-render`'s Hue/Saturation pass (`shaders/hue_saturation.wgsl`)
//!   ports [`HueSaturationOp::apply`] line for line
//!   ([`HueSaturationOp::packed_for_gpu`] hands it the settings). An
//!   operation with active colour ranges returns `None` there and stays
//!   on the CPU.

use aurora_core::{
    HUE_RANGE_COUNT, HueRange, HueSaturationParams, HueSaturationRangesParams, forward_gap,
};

/// HSL of a clamped pixel: hue in degrees `[0, 360)`, saturation and
/// lightness in `[0, 1]`.
// `max` is one of `r`, `g`, `b` exactly, so the comparisons are exact.
#[allow(clippy::float_cmp)]
pub(crate) fn to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) * 0.5;
    let delta = max - min;
    if delta <= 0.0 {
        return (0.0, 0.0, lightness);
    }
    let denominator = if lightness <= 0.5 {
        max + min
    } else {
        2.0 - max - min
    };
    let saturation = if denominator > 0.0 {
        (delta / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sector = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    ((sector * 60.0).rem_euclid(360.0), saturation, lightness)
}

/// The RGB of an HSL colour, each channel clamped to `[0, 1]`.
pub(crate) fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector {
        s if s < 1.0 => (chroma, second, 0.0),
        s if s < 2.0 => (second, chroma, 0.0),
        s if s < 3.0 => (0.0, chroma, second),
        s if s < 4.0 => (0.0, second, chroma),
        s if s < 5.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = lightness - chroma * 0.5;
    [
        (r + offset).clamp(0.0, 1.0),
        (g + offset).clamp(0.0, 1.0),
        (b + offset).clamp(0.0, 1.0),
    ]
}

/// The lightness slider: `amount` in `[-1, 1]` blends towards black
/// (negative) or white (positive).
fn adjust_lightness(lightness: f32, amount: f32) -> f32 {
    if amount < 0.0 {
        lightness * (1.0 + amount)
    } else {
        (1.0 - lightness).mul_add(amount, lightness)
    }
}

/// The saturation slider: `amount` in `[-1, 1]`.
pub(crate) fn adjust_saturation(saturation: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        saturation * (1.0 + amount)
    } else if amount >= 1.0 {
        if saturation > 0.0 { 1.0 } else { 0.0 }
    } else {
        (saturation / (1.0 - amount)).min(1.0)
    }
}

/// The weight in `[0, 1]` of hue `hue` (degrees) in a colour range with
/// `edges`: a linear ramp up from edge 1 to edge 2, 1 up to edge 3, a
/// linear ramp down to edge 4, 0 elsewhere, all measured forward from
/// edge 1 around the circle. A zero-width ramp is a hard edge.
///
/// This is a **fitted model**, not Photoshop's published algorithm: it
/// reproduces the ramps in psd-tools' `huesaturation_saturation_rgb.psd`
/// against Photoshop's merged image (see PLAN.md, 0.204.0).
#[must_use]
pub fn range_weight(edges: [f32; 4], hue: f32) -> f32 {
    let [e1, e2, e3, e4] = edges;
    let t = (hue - e1).rem_euclid(360.0);
    let up = forward_gap(e1, e2);
    let full = forward_gap(e2, e3);
    let down = forward_gap(e3, e4);
    if t < up {
        t / up
    } else if t < up + full {
        1.0
    } else if t < up + full + down {
        1.0 - (t - up - full) / down
    } else {
        0.0
    }
}

/// A saturation slider position `amount` in `[-1, 1]` as a factor on the
/// saturation (`1 + k` below zero, `1 / (1 - k)` above; the pole at `+1`
/// is capped, since a weighted share of an infinite factor has no meaning).
fn saturation_factor(amount: f32) -> f32 {
    if amount <= 0.0 {
        1.0 + amount
    } else {
        1.0 / (1.0 - amount).max(1e-6)
    }
}

/// How many `f32`s [`HueSaturationOp::packed_for_gpu`] returns (0.206.0).
pub const HUE_SATURATION_PACKED_LEN: usize = 8;

/// A Hue/Saturation adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueSaturationOp {
    params: HueSaturationParams,
    /// The colour ranges, only when Colorize is off and one of them
    /// adjusts something; otherwise the 0.203.0 path runs unchanged.
    ranges: Option<[HueRange; HUE_RANGE_COUNT]>,
}

impl HueSaturationOp {
    /// Prepares `params` for application (nothing to precompute; kept as a
    /// type so the caller's lookup-table enum treats every adjustment alike).
    #[must_use]
    pub const fn new(params: &HueSaturationParams) -> Self {
        Self {
            params: *params,
            ranges: None,
        }
    }

    /// Prepares master/Colorize settings plus colour ranges. With Colorize
    /// on, or every range adjusting nothing, this is [`Self::new`] of the
    /// base settings exactly (bit for bit).
    #[must_use]
    pub fn with_ranges(params: &HueSaturationRangesParams) -> Self {
        let ranged = !params.base().colorize() && !params.ranges_adjust_nothing();
        Self {
            params: *params.base(),
            ranges: ranged.then(|| *params.ranges()),
        }
    }

    /// The settings flattened for the GPU Hue/Saturation pass (0.206.0,
    /// `aurora_render::TileCompositor::composite_hue_saturation_with_opacity`,
    /// `shaders/hue_saturation.wgsl`): `[identity, colorize, hue,
    /// saturation, lightness, colorize_hue, colorize_saturation,
    /// colorize_lightness]`, flags as `0`/`1`, hues in degrees, the
    /// saturation and lightness sliders divided by `100` in `f32` exactly
    /// as [`Self::apply`] divides them.
    ///
    /// `None` when this operation has active colour ranges: the GPU pass
    /// has no per-range model, so the layer stays on the CPU.
    #[must_use]
    pub fn packed_for_gpu(&self) -> Option<[f32; HUE_SATURATION_PACKED_LEN]> {
        if self.ranges.is_some() {
            return None;
        }
        let p = &self.params;
        Some([
            f32::from(u8::from(self.is_identity())),
            f32::from(u8::from(p.colorize())),
            p.hue(),
            p.saturation() / 100.0,
            p.lightness() / 100.0,
            p.colorize_hue(),
            p.colorize_saturation() / 100.0,
            p.colorize_lightness() / 100.0,
        ])
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.params.is_identity() && self.ranges.is_none()
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.is_identity() {
            return rgb;
        }
        let clamped = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let (hue, saturation, lightness) = to_hsl(clamped);
        let p = &self.params;
        if p.colorize() {
            return from_hsl(
                p.colorize_hue(),
                p.colorize_saturation() / 100.0,
                adjust_lightness(lightness, p.colorize_lightness() / 100.0),
            );
        }
        let Some(ranges) = &self.ranges else {
            return from_hsl(
                hue + p.hue(),
                adjust_saturation(saturation, p.saturation() / 100.0),
                adjust_lightness(lightness, p.lightness() / 100.0),
            );
        };
        // Ranged: the master first (exactly as above), then the weighted
        // sum of the ranges, weights read from the pixel's ORIGINAL hue.
        let (mut hue_shift, mut saturation_gain, mut lightness_sum) = (0.0_f32, 0.0_f32, 0.0_f32);
        for range in ranges.iter().filter(|r| !r.adjusts_nothing()) {
            let w = range_weight(range.edges(), hue);
            if w <= 0.0 {
                continue;
            }
            hue_shift += w * range.hue();
            saturation_gain += w * (saturation_factor(range.saturation() / 100.0) - 1.0);
            lightness_sum += w * range.lightness() / 100.0;
        }
        let master_saturation = adjust_saturation(saturation, p.saturation() / 100.0);
        let rgb = from_hsl(
            hue + p.hue() + hue_shift,
            (master_saturation * (1.0 + saturation_gain)).clamp(0.0, 1.0),
            adjust_lightness(lightness, p.lightness() / 100.0),
        );
        pull_lightness(rgb, lightness_sum.clamp(-1.0, 1.0))
    }
}

/// The ranges' net lightness `t` in `[-1, 1]`: positive pulls every
/// channel towards the pixel's largest channel by that fraction (towards
/// grey at the peak's level), negative pulls them towards the smallest.
/// Fitted: Photoshop's per-range lightness barely moves HSL lightness, it
/// desaturates (PLAN.md, 0.204.0).
fn pull_lightness(rgb: [f32; 3], t: f32) -> [f32; 3] {
    if t == 0.0 {
        return rgb;
    }
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    rgb.map(|c| {
        if t > 0.0 {
            t.mul_add(max - c, c)
        } else {
            (-t).mul_add(-(c - min), c)
        }
        .clamp(0.0, 1.0)
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
mod tests {
    use super::{HueSaturationOp, from_hsl, to_hsl};
    use aurora_core::HueSaturationParams as P;

    fn params(h: f32, s: f32, l: f32) -> P {
        match P::new(h, s, l, false, 0.0, 25.0, 0.0) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn colorize(h: f32, s: f32, l: f32) -> P {
        match P::new(0.0, 0.0, 0.0, true, h, s, l) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    fn apply(p: &P, rgb: [f32; 3]) -> [f32; 3] {
        HueSaturationOp::new(p).apply(rgb)
    }

    #[test]
    fn hsl_round_trips_on_a_grid() {
        for r in 0..=8 {
            for g in 0..=8 {
                for b in 0..=8 {
                    let c = [r as f32 / 8.0, g as f32 / 8.0, b as f32 / 8.0];
                    let (h, s, l) = to_hsl(c);
                    assert!(near(from_hsl(h, s, l), c, 2e-6), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn identity_is_an_exact_passthrough_even_out_of_gamut() {
        let op = HueSaturationOp::new(&P::identity());
        assert!(op.is_identity());
        for px in [[0.2, 1.5, -0.25], [f32::NAN, 0.0, 1.0], [0.3, 0.3, 0.3]] {
            let out = op.apply(px);
            for (a, b) in out.iter().zip(px) {
                assert_eq!(a.to_bits(), b.to_bits());
            }
        }
        // Colorize values never matter while it is off.
        let off = match P::new(0.0, 0.0, 0.0, false, 200.0, 90.0, 40.0) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(HueSaturationOp::new(&off).is_identity());
    }

    #[test]
    fn hue_rotates_and_wraps() {
        let red = [1.0, 0.0, 0.0];
        assert!(near(
            apply(&params(120.0, 0.0, 0.0), red),
            [0.0, 1.0, 0.0],
            1e-6
        ));
        assert!(near(
            apply(&params(-120.0, 0.0, 0.0), red),
            [0.0, 0.0, 1.0],
            1e-6
        ));
        // +180 and -180 are the same rotation.
        assert!(near(
            apply(&params(180.0, 0.0, 0.0), red),
            apply(&params(-180.0, 0.0, 0.0), red),
            1e-6
        ));
        assert!(near(
            apply(&params(180.0, 0.0, 0.0), red),
            [0.0, 1.0, 1.0],
            1e-6
        ));
        // Two rotations that sum to 360 compose to the identity.
        let c = [0.8, 0.3, 0.5];
        let once = apply(&params(150.0, 0.0, 0.0), c);
        assert!(near(apply(&params(-150.0, 0.0, 0.0), once), c, 2e-6));
        // Lightness and HSL saturation survive a rotation.
        let (_, s0, l0) = to_hsl(c);
        let (_, s1, l1) = to_hsl(once);
        assert!((s0 - s1).abs() < 2e-6 && (l0 - l1).abs() < 2e-6);
    }

    #[test]
    fn grey_pixels_ignore_hue_and_saturation_exactly() {
        for v in [0.0_f32, 0.1, 0.5, 0.731, 1.0] {
            for (h, s) in [(90.0, 0.0), (-33.0, 100.0), (180.0, -100.0), (0.0, 50.0)] {
                let out = apply(&params(h, s, 0.0), [v, v, v]);
                assert_eq!(out, [v, v, v], "grey {v} hue {h} sat {s}");
            }
        }
    }

    #[test]
    fn saturation_extremes() {
        let c = [0.8, 0.4, 0.4];
        let (_, _, l) = to_hsl(c);
        // -100: grey at the same HSL lightness.
        assert!(near(apply(&params(0.0, -100.0, 0.0), c), [l, l, l], 1e-6));
        // +100: fully saturated, same lightness.
        let full = apply(&params(0.0, 100.0, 0.0), c);
        let (_, s, l2) = to_hsl(full);
        assert!((s - 1.0).abs() < 1e-5 && (l2 - l).abs() < 1e-5, "{full:?}");
        // +100 saturates even a barely tinted pixel; a grey stays grey.
        let (_, s2, _) = to_hsl(apply(&params(0.0, 100.0, 0.0), [0.5, 0.501, 0.5]));
        assert!((s2 - 1.0).abs() < 1e-3);
        // Halfway down halves the saturation.
        let (_, sh, _) = to_hsl(apply(&params(0.0, -50.0, 0.0), c));
        let (_, s0, _) = to_hsl(c);
        assert!((sh - s0 * 0.5).abs() < 1e-5);
        // Positive saturation is monotone and never above 1.
        let mut last = 0.0;
        for k in 0..=20 {
            let (_, sk, _) = to_hsl(apply(&params(0.0, k as f32 * 5.0, 0.0), c));
            assert!(sk + 1e-6 >= last && sk <= 1.0 + 1e-6, "{k}");
            last = sk;
        }
        // Past full saturation the scale is capped at 1 (not clamped per
        // channel afterwards, which would shift the colour): +50 on a
        // light pixel with s = 0.6 (1.2 uncapped) equals +100.
        let light = [0.9, 0.5, 0.5];
        assert!(to_hsl(light).1 > 0.5 && to_hsl(light).2 > 0.65);
        assert_eq!(
            apply(&params(0.0, 50.0, 0.0), light),
            apply(&params(0.0, 100.0, 0.0), light)
        );
        // The curve is s / (1 - k): +50 doubles it.
        let (_, s50, _) = to_hsl(apply(&params(0.0, 50.0, 0.0), [0.6, 0.5, 0.5]));
        let (_, s00, _) = to_hsl([0.6, 0.5, 0.5]);
        assert!((s50 - 2.0 * s00).abs() < 1e-5);
    }

    #[test]
    fn lightness_extremes_blend_to_black_and_white() {
        let c = [0.8, 0.4, 0.2];
        assert_eq!(apply(&params(0.0, 0.0, -100.0), c), [0.0, 0.0, 0.0]);
        assert_eq!(apply(&params(0.0, 0.0, 100.0), c), [1.0, 1.0, 1.0]);
        let (_, s, l) = to_hsl(c);
        let (_, s_half, l_half) = to_hsl(apply(&params(0.0, 0.0, -50.0), c));
        assert!((l_half - l * 0.5).abs() < 1e-5 && (s_half - s).abs() < 1e-4);
        let (_, _, l_up) = to_hsl(apply(&params(0.0, 0.0, 50.0), c));
        assert!((l_up - (l + (1.0 - l) * 0.5)).abs() < 1e-5);
    }

    #[allow(clippy::many_single_char_names)]
    #[test]
    fn colorize_sets_hue_and_saturation_and_keeps_the_pixels_lightness() {
        let c = [0.2, 0.5, 0.9];
        let (_, _, l) = to_hsl(c);
        let out = apply(&colorize(30.0, 80.0, 0.0), c);
        let (h, s, l2) = to_hsl(out);
        assert!((h - 30.0).abs() < 1e-3 && (s - 0.8).abs() < 1e-5 && (l2 - l).abs() < 1e-5);
        // A grey pixel is coloured too, and the master sliders are ignored.
        let both = match P::new(90.0, 100.0, 100.0, true, 30.0, 80.0, 0.0) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(near(apply(&both, c), out, 1e-6));
        let grey = apply(&colorize(120.0, 100.0, 0.0), [0.5, 0.5, 0.5]);
        let (hg, sg, lg) = to_hsl(grey);
        assert!((hg - 120.0).abs() < 1e-3 && (sg - 1.0).abs() < 1e-5 && (lg - 0.5).abs() < 1e-6);
        // Colorize saturation 0 makes grey at the pixel's lightness.
        assert!(near(apply(&colorize(77.0, 0.0, 0.0), c), [l, l, l], 1e-6));
        // Colorize lightness: -100 black, +100 white.
        assert_eq!(apply(&colorize(77.0, 50.0, -100.0), c), [0.0, 0.0, 0.0]);
        assert_eq!(apply(&colorize(77.0, 50.0, 100.0), c), [1.0, 1.0, 1.0]);
        // Colorize hue 360 is hue 0.
        assert!(near(
            apply(&colorize(360.0, 60.0, 10.0), c),
            apply(&colorize(0.0, 60.0, 10.0), c),
            1e-6
        ));
    }

    #[test]
    fn out_of_range_and_nan_input_is_clamped_and_output_stays_in_gamut() {
        let p = params(40.0, 30.0, 10.0);
        for px in [
            [2.0, -1.0, 0.5],
            [f32::NAN, f32::NAN, f32::NAN],
            [f32::INFINITY, 0.0, f32::NEG_INFINITY],
        ] {
            let out = apply(&p, px);
            assert!(
                out.iter().all(|v| (0.0..=1.0).contains(v)),
                "{px:?} {out:?}"
            );
        }
        assert_eq!(
            apply(&p, [2.0, -1.0, 0.5]),
            apply(&p, [1.0, 0.0, 0.5]),
            "clamped before the map"
        );
    }

    /// Pixels do not depend on their neighbours: applying to a pixel alone
    /// and inside a longer run agrees bit for bit (the tile-seam guarantee).
    #[test]
    fn the_map_is_a_pure_per_pixel_function() {
        let p = params(-27.0, 12.0, -20.0);
        let px: Vec<[f32; 3]> = (0..64)
            .map(|i| {
                [
                    i as f32 / 63.0,
                    (i * 7 % 64) as f32 / 63.0,
                    (i * 13 % 64) as f32 / 63.0,
                ]
            })
            .collect();
        let op = HueSaturationOp::new(&p);
        let batch: Vec<[f32; 3]> = px.iter().map(|&c| op.apply(c)).collect();
        for (c, want) in px.iter().zip(&batch) {
            assert_eq!(&op.apply(*c), want);
        }
    }

    // ---- 0.204.0: colour ranges ----------------------------------------

    use super::range_weight;
    use aurora_core::{HueRange, HueSaturationRangesParams as RP};

    fn base(h: f32, s: f32, l: f32) -> P {
        params(h, s, l)
    }

    fn with_range(b: P, index: usize, edges: [f32; 4], h: f32, s: f32, l: f32) -> RP {
        let range = match HueRange::new(edges, h, s, l) {
            Ok(r) => r,
            Err(err) => unreachable!("{err:?}"),
        };
        match RP::plain(b).with_range(index, range) {
            Some(p) => p,
            None => unreachable!("index"),
        }
    }

    fn grid() -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for r in 0..=6 {
            for g in 0..=6 {
                for b in 0..=6 {
                    v.push([r as f32 / 6.0, g as f32 / 6.0, b as f32 / 6.0]);
                }
            }
        }
        v
    }

    #[test]
    fn weights_ramp_hold_ramp_down_and_wrap() {
        let e = [315.0, 345.0, 15.0, 45.0]; // reds: wraps through 0
        let w = |h: f32| range_weight(e, h);
        assert_eq!(w(315.0), 0.0);
        assert!((w(330.0) - 0.5).abs() < 1e-6);
        assert_eq!(w(345.0), 1.0);
        assert_eq!(w(0.0), 1.0);
        assert_eq!(w(359.999), 1.0);
        assert_eq!(w(15.0 - 1e-3), 1.0);
        assert!((w(30.0) - 0.5).abs() < 1e-6);
        assert_eq!(w(45.0), 0.0);
        assert_eq!(w(180.0), 0.0);
        assert_eq!(w(314.9), 0.0);
        // Hue 360 is hue 0; hue -30 is 330.
        assert_eq!(w(360.0), w(0.0));
        assert!((w(-30.0) - w(330.0)).abs() < 1e-6);
        // Hard edges: (100,100,200,200) is a box.
        let b = [100.0, 100.0, 200.0, 200.0];
        assert_eq!(range_weight(b, 99.9), 0.0);
        assert_eq!(range_weight(b, 100.0), 1.0);
        assert_eq!(range_weight(b, 199.9), 1.0);
        assert_eq!(range_weight(b, 200.0), 0.0);
        // A corpus range that wraps in its ramp-up (303 -> 4).
        let c = [303.0, 4.0, 82.0, 175.0];
        assert_eq!(range_weight(c, 303.0), 0.0);
        assert!((range_weight(c, 333.5) - 0.5).abs() < 0.01);
        assert_eq!(range_weight(c, 40.0), 1.0);
        assert!((range_weight(c, 128.5) - 0.5).abs() < 0.01);
        assert_eq!(range_weight(c, 175.0), 0.0);
        assert_eq!(range_weight(c, 250.0), 0.0);
    }

    #[test]
    fn neutral_ranges_are_bit_identical_to_the_plain_adjustment() {
        let plain = base(-27.0, 12.0, -20.0);
        let a = HueSaturationOp::new(&plain);
        // Default ranges, and ranges whose edges were edited but adjust nothing.
        let edited = with_range(plain, 2, [80.0, 100.0, 140.0, 160.0], 0.0, 0.0, 0.0);
        for ranged in [RP::plain(plain), edited] {
            let b = HueSaturationOp::with_ranges(&ranged);
            for px in grid() {
                let (x, y) = (a.apply(px), b.apply(px));
                for (u, v) in x.iter().zip(y) {
                    assert_eq!(u.to_bits(), v.to_bits(), "{px:?}");
                }
            }
        }
        assert!(HueSaturationOp::with_ranges(&RP::plain(P::identity())).is_identity());
        // Colorize ignores the ranges.
        let col = P::new(0.0, 0.0, 0.0, true, 120.0, 100.0, 0.0);
        let col = match col {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        };
        let ranged = with_range(col, 0, [315.0, 345.0, 15.0, 45.0], 50.0, 50.0, 50.0);
        let (x, y) = (
            HueSaturationOp::new(&col),
            HueSaturationOp::with_ranges(&ranged),
        );
        for px in grid() {
            assert_eq!(x.apply(px), y.apply(px));
        }
    }

    #[test]
    fn a_ranges_hue_shift_applies_by_weight_and_leaves_other_hues_alone() {
        let red_range = [315.0, 345.0, 15.0, 45.0];
        let p = with_range(P::identity(), 0, red_range, 120.0, 0.0, 0.0);
        let op = HueSaturationOp::with_ranges(&p);
        // Pure red (hue 0, weight 1) rotates by 120: green.
        assert!(near(op.apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0], 1e-5));
        // Pure blue (hue 240, weight 0) is untouched.
        assert_eq!(op.apply([0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]);
        // Halfway down the ramp (hue 30, weight .5) rotates by 60.
        let c = from_hsl(30.0, 1.0, 0.5);
        let (h, _, _) = to_hsl(op.apply(c));
        assert!((h - 90.0).abs() < 1e-3, "{h}");
        // Grey is untouched by a hue shift.
        assert_eq!(op.apply([0.4, 0.4, 0.4]), [0.4, 0.4, 0.4]);
    }

    #[test]
    fn a_ranges_saturation_blends_the_factor_by_weight() {
        let red_range = [315.0, 345.0, 15.0, 45.0];
        // +50 is a factor of 2; at weight 0.5 the blended factor is 1.5.
        let p = with_range(P::identity(), 0, red_range, 0.0, 50.0, 0.0);
        let op = HueSaturationOp::with_ranges(&p);
        let full = from_hsl(0.0, 0.2, 0.5);
        let (_, s, _) = to_hsl(op.apply(full));
        assert!((s - 0.4).abs() < 1e-5, "{s}");
        let half = from_hsl(30.0, 0.2, 0.5);
        let (_, s, _) = to_hsl(op.apply(half));
        assert!((s - 0.3).abs() < 1e-5, "{s}");
        // -100 greys the range completely, at its plateau only.
        let g = with_range(P::identity(), 0, red_range, 0.0, -100.0, 0.0);
        let (_, s, _) = to_hsl(HueSaturationOp::with_ranges(&g).apply(full));
        assert!(s < 1e-5);
        // Two overlapping ranges add their (factor - 1) shares.
        let Ok(second) = HueRange::new(red_range, 0.0, 50.0, 0.0) else {
            unreachable!("valid range");
        };
        let Some(two) = p.with_range(1, second) else {
            unreachable!("index");
        };
        let (_, s, _) = to_hsl(HueSaturationOp::with_ranges(&two).apply(full));
        assert!((s - 0.6).abs() < 1e-5, "factor 1 + 1 + 1 = 3: {s}");
    }

    #[test]
    fn master_applies_first_and_the_ranges_weight_the_original_hue() {
        // Master hue +120 moves red to green; the red range (weights from
        // the ORIGINAL hue) then still applies to it.
        let red_range = [315.0, 345.0, 15.0, 45.0];
        let p = with_range(base(120.0, 0.0, 0.0), 0, red_range, 60.0, 0.0, 0.0);
        let out = HueSaturationOp::with_ranges(&p).apply([1.0, 0.0, 0.0]);
        let (h, _, _) = to_hsl(out);
        assert!((h - 180.0).abs() < 1e-3, "120 + 60: {h}");
        // A pixel that master moves INTO the range is not affected by it.
        let blue = [0.0, 0.0, 1.0]; // hue 240; +120 -> hue 0 (red-ish)
        let (h, _, _) = to_hsl(HueSaturationOp::with_ranges(&p).apply(blue));
        assert!((h - 0.0).abs() < 1e-3 || (h - 360.0).abs() < 1e-3, "{h}");
    }

    #[test]
    fn a_ranges_lightness_pulls_channels_towards_the_peak_or_the_floor() {
        let red_range = [315.0, 345.0, 15.0, 45.0];
        let up = with_range(P::identity(), 0, red_range, 0.0, 0.0, 50.0);
        let down = with_range(P::identity(), 0, red_range, 0.0, 0.0, -50.0);
        let c = [0.8, 0.3, 0.2];
        let (u, d) = (
            HueSaturationOp::with_ranges(&up).apply(c),
            HueSaturationOp::with_ranges(&down).apply(c),
        );
        // c has hue 10, inside the plateau (weight 1): +50 halves the gap to 0.8.
        assert!(near(u, [0.8, 0.55, 0.5], 1e-5), "{u:?}");
        // -50 halves the gap to the floor 0.2.
        assert!(near(d, [0.5, 0.25, 0.2], 1e-5), "{d:?}");
        // +100 is grey at the peak, -100 grey at the floor.
        let full = with_range(P::identity(), 0, red_range, 0.0, 0.0, 100.0);
        assert!(near(
            HueSaturationOp::with_ranges(&full).apply(c),
            [0.8; 3],
            1e-5
        ));
        let low = with_range(P::identity(), 0, red_range, 0.0, 0.0, -100.0);
        assert!(near(
            HueSaturationOp::with_ranges(&low).apply(c),
            [0.2; 3],
            1e-5
        ));
        // Hue 340 sits at weight 5/6 on the ramp: the pull is scaled by it.
        let ramp = HueSaturationOp::with_ranges(&up).apply([0.8, 0.2, 0.4]);
        assert!(
            (ramp[1] - (0.2 + 0.6 * 0.5 * 5.0 / 6.0)).abs() < 1e-4,
            "{ramp:?}"
        );
        // Outside the range nothing happens.
        let blue = [0.1, 0.2, 0.9];
        assert!(near(
            HueSaturationOp::with_ranges(&up).apply(blue),
            blue,
            1e-6
        ));
    }

    #[test]
    fn ranged_output_stays_in_gamut_and_is_a_pure_per_pixel_function() {
        let mut p = with_range(
            base(40.0, 30.0, 10.0),
            0,
            [315.0, 345.0, 15.0, 45.0],
            90.0,
            100.0,
            100.0,
        );
        p = match p.with_range(
            3,
            match HueRange::new([135.0, 165.0, 195.0, 225.0], -90.0, -100.0, -100.0) {
                Ok(r) => r,
                Err(err) => unreachable!("{err:?}"),
            },
        ) {
            Some(p) => p,
            None => unreachable!("index"),
        };
        let op = HueSaturationOp::with_ranges(&p);
        for px in grid() {
            let out = op.apply(px);
            assert!(
                out.iter().all(|v| (0.0..=1.0).contains(v)),
                "{px:?} {out:?}"
            );
            assert_eq!(op.apply(px), out);
        }
        for px in [[f32::NAN, 2.0, -1.0], [f32::INFINITY, 0.0, 0.3]] {
            assert!(op.apply(px).iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}
