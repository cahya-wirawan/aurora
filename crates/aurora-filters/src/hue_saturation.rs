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
//! - **No GPU port.** `aurora-app` composites Hue/Saturation layers on
//!   the CPU only (a visible one refuses the GPU path).

use aurora_core::HueSaturationParams;

/// HSL of a clamped pixel: hue in degrees `[0, 360)`, saturation and
/// lightness in `[0, 1]`.
// `max` is one of `r`, `g`, `b` exactly, so the comparisons are exact.
#[allow(clippy::float_cmp)]
fn to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
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
fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
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
fn adjust_saturation(saturation: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        saturation * (1.0 + amount)
    } else if amount >= 1.0 {
        if saturation > 0.0 { 1.0 } else { 0.0 }
    } else {
        (saturation / (1.0 - amount)).min(1.0)
    }
}

/// A Hue/Saturation adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueSaturationOp {
    params: HueSaturationParams,
}

impl HueSaturationOp {
    /// Prepares `params` for application (nothing to precompute; kept as a
    /// type so the caller's lookup-table enum treats every adjustment alike).
    #[must_use]
    pub const fn new(params: &HueSaturationParams) -> Self {
        Self { params: *params }
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.params.is_identity()
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.params.is_identity() {
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
        from_hsl(
            hue + p.hue(),
            adjust_saturation(saturation, p.saturation() / 100.0),
            adjust_lightness(lightness, p.lightness() / 100.0),
        )
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
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
}
