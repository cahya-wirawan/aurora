//! The Vibrance adjustment's pixel math (0.239.0).
//!
//! [`VibranceOp`] applies an [`aurora_core::VibranceParams`] to **straight
//! (un-premultiplied) `f32` RGB**, encoded values, alpha never an input or
//! output, no 8-bit intermediate (invariant 7.3.1b). The formulas are on
//! the parameter type (`aurora-core`'s `vibrance` module): the pixel's HSL
//! saturation is raised more the duller the colour (and less for skin-tone
//! hues), hue and lightness stay, then the Saturation slider applies the
//! Hue/Saturation master rule.
//!
//! # Decisions, with reasons
//!
//! - **Per pixel, no table**: the map reads three channels (hue), so a 1D
//!   table cannot hold it. Deterministic, and tile boundaries cannot show.
//! - **An identity adjustment is skipped** ([`VibranceOp::is_identity`]), an
//!   exact passthrough even for an out-of-gamut value. A non-identity one
//!   clamps its input to `[0, 1]` (`NaN` reads as `0`) and returns values
//!   in `[0, 1]`.
//! - **A grey pixel (`max == min`) has no hue and no saturation** and is
//!   unchanged: there is nothing to make more vivid.
//! - **No GPU port** (a later task): a visible Vibrance layer keeps the
//!   document on the CPU compositing path.

use crate::hue_saturation::{adjust_saturation, from_hsl, to_hsl};
use aurora_core::VibranceParams;

/// How much the skin-tone guard (hues around 30 degrees) shrinks a
/// positive vibrance at its centre: half.
const SKIN_PROTECTION: f32 = 0.5;
/// The skin-tone guard's centre hue and half-width, degrees.
const SKIN_CENTRE: f32 = 30.0;
const SKIN_HALF_WIDTH: f32 = 30.0;

/// The factor in `[1 - SKIN_PROTECTION, 1]` a positive vibrance is scaled
/// by at `hue` degrees: a raised cosine dip centred on [`SKIN_CENTRE`].
fn skin_guard(hue: f32) -> f32 {
    let d = hue - SKIN_CENTRE;
    if d.abs() >= SKIN_HALF_WIDTH {
        return 1.0;
    }
    let bump = 0.5 * (1.0 + (std::f32::consts::PI * d / SKIN_HALF_WIDTH).cos());
    SKIN_PROTECTION.mul_add(-bump, 1.0)
}

/// The vibrance slider (`amount` in `[-1, 1]`) on saturation `s`.
fn vibrance_saturation(s: f32, amount: f32, hue: f32) -> f32 {
    if amount > 0.0 {
        s * (1.0 + amount * (1.0 - s) * skin_guard(hue))
    } else {
        s * (1.0 + amount)
    }
}

/// A Vibrance adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VibranceOp {
    params: VibranceParams,
}

impl VibranceOp {
    /// Prepares `params` for application.
    #[must_use]
    pub const fn new(params: &VibranceParams) -> Self {
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
        if self.is_identity() {
            return rgb;
        }
        let clamped = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let (hue, saturation, lightness) = to_hsl(clamped);
        let boosted = vibrance_saturation(saturation, self.params.vibrance() / 100.0, hue);
        let adjusted = adjust_saturation(boosted, self.params.saturation() / 100.0);
        from_hsl(hue, adjusted.clamp(0.0, 1.0), lightness)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{VibranceOp, skin_guard};
    use aurora_core::VibranceParams as P;

    fn op(vibrance: f32, saturation: f32) -> VibranceOp {
        match P::new(vibrance, saturation) {
            Ok(p) => VibranceOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// An independent textbook HSL (no code shared with the op): hue
    /// degrees, saturation, lightness.
    fn hsl(rgb: [f32; 3]) -> (f32, f32, f32) {
        let [r, g, b] = rgb.map(f64::from);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if max == min {
            return (0.0, 0.0, l as f32);
        }
        let d = max - min;
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * (((g - b) / d).rem_euclid(6.0))
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h as f32, s as f32, l as f32)
    }

    /// RGB of an HSL colour by the textbook chroma construction.
    fn rgb_of(h: f32, s: f32, l: f32) -> [f32; 3] {
        let (h, s, l) = (f64::from(h), f64::from(s), f64::from(l));
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h / 60.0).floor() as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [(r + m) as f32, (g + m) as f32, (b + m) as f32]
    }

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn identity_is_an_exact_passthrough_even_out_of_gamut() {
        let identity = op(0.0, 0.0);
        assert!(identity.is_identity());
        for rgb in [
            [0.1, 0.5, 0.9],
            [1.5, -0.25, 0.3],
            [f32::NAN, 0.0, 2.0],
            [0.5, 0.5, 0.5],
        ] {
            let out = identity.apply(rgb);
            for (a, b) in out.iter().zip(rgb) {
                assert!(a.to_bits() == b.to_bits(), "{rgb:?} -> {out:?}");
            }
        }
        assert!(!op(1.0, 0.0).is_identity());
        assert!(!op(0.0, -1.0).is_identity());
    }

    /// Positive vibrance follows `s' = s (1 + k (1 - s) g(h))`, keeps hue
    /// and lightness, over a grid of hues (outside the skin guard so
    /// `g = 1`), saturations and lightnesses.
    #[test]
    fn positive_vibrance_matches_the_closed_form_and_keeps_hue_and_lightness() {
        for vibrance in [10.0_f32, 50.0, 100.0] {
            let k = vibrance / 100.0;
            for hue in [90.0_f32, 150.0, 210.0, 270.0, 330.0] {
                for s in [0.05_f32, 0.2, 0.5, 0.8, 0.95] {
                    for l in [0.3_f32, 0.5, 0.7] {
                        let input = rgb_of(hue, s, l);
                        let out = op(vibrance, 0.0).apply(input);
                        let (h2, s2, l2) = hsl(out);
                        let want = s * (1.0 + k * (1.0 - s));
                        assert!(near(s2, want, 2e-4), "{hue} {s} {l} {vibrance}: {s2} {want}");
                        assert!(near(l2, l, 2e-4), "lightness {hue} {s} {l}");
                        let dh = (h2 - hue + 540.0).rem_euclid(360.0) - 180.0;
                        assert!(dh.abs() < 0.2, "hue {hue} {s} {l}: {h2}");
                    }
                }
            }
        }
    }

    #[test]
    fn dull_colours_gain_more_than_vivid_ones_and_full_saturation_is_untouched() {
        let hue = 200.0;
        let gain = |s: f32| {
            let (_, s2, _) = hsl(op(80.0, 0.0).apply(rgb_of(hue, s, 0.5)));
            s2 - s
        };
        let relative = |s: f32| (gain(s) + s) / s;
        assert!(relative(0.1) > relative(0.4) && relative(0.4) > relative(0.8));
        assert!(relative(0.1) > 1.5, "a dull colour gains a lot");
        assert!(gain(0.1) > 0.0 && gain(0.5) > 0.0);
        // A fully saturated colour is untouched by a positive vibrance.
        let pure = [0.0_f32, 0.5, 1.0];
        let out = op(100.0, 0.0).apply(pure);
        for (a, b) in out.iter().zip(pure) {
            assert!(near(*a, b, 1e-5), "{out:?}");
        }
    }

    #[test]
    fn saturation_is_monotone_in_the_input_saturation() {
        for vibrance in [-100.0_f32, -40.0, 30.0, 100.0] {
            for hue in [10.0_f32, 30.0, 120.0, 250.0] {
                let mut last = -1.0_f32;
                for step in 0..=40 {
                    let s = step as f32 / 40.0;
                    let (_, s2, _) = hsl(op(vibrance, 0.0).apply(rgb_of(hue, s, 0.5)));
                    assert!(s2 >= last - 3e-4, "{vibrance} {hue} {s}: {s2} < {last}");
                    last = s2;
                }
            }
        }
    }

    #[test]
    fn skin_tone_hues_gain_less_than_other_hues_at_the_same_saturation() {
        let gain_at = |hue: f32| {
            let (_, s2, _) = hsl(op(100.0, 0.0).apply(rgb_of(hue, 0.3, 0.5)));
            s2 - 0.3
        };
        let skin = gain_at(30.0);
        let blue = gain_at(220.0);
        assert!(skin > 0.0 && skin < blue, "skin {skin} blue {blue}");
        // The dip is half at its centre: s (1 + k (1 - s) / 2).
        assert!(near(skin, 0.3 * 0.7 * 0.5, 2e-4), "{skin}");
        assert!(near(blue, 0.3 * 0.7, 2e-4), "{blue}");
        // And the guard is exactly 1 at and outside its edges, 0.5 at its centre.
        assert_eq!(skin_guard(0.0), 1.0);
        assert_eq!(skin_guard(60.0), 1.0);
        assert_eq!(skin_guard(200.0), 1.0);
        assert!(near(skin_guard(30.0), 0.5, 1e-6));
        assert!(skin_guard(15.0) > 0.5 && skin_guard(15.0) < 1.0);
    }

    #[test]
    fn negative_vibrance_and_negative_saturation_scale_the_saturation() {
        let input = rgb_of(200.0, 0.6, 0.5);
        let (_, s_vib, _) = hsl(op(-50.0, 0.0).apply(input));
        assert!(near(s_vib, 0.3, 2e-4), "{s_vib}");
        let (_, s_sat, _) = hsl(op(0.0, -50.0).apply(input));
        assert!(near(s_sat, 0.3, 2e-4), "{s_sat}");
        // -100 reaches grey, keeping the lightness.
        let out = op(-100.0, 0.0).apply(input);
        assert!(near(out[0], out[1], 1e-5) && near(out[1], out[2], 1e-5));
        assert!(near(out[0], 0.5, 1e-5));
    }

    #[test]
    fn the_saturation_slider_applies_after_vibrance() {
        let input = rgb_of(120.0, 0.3, 0.5);
        // Vibrance 100 gives s1 = 0.3 * 1.7 = 0.51; saturation +50 gives
        // s1 / (1 - 0.5) = 1.02, capped at 1.
        let (_, s2, _) = hsl(op(100.0, 50.0).apply(input));
        assert!(near(s2, 1.0, 2e-4), "{s2}");
        // Vibrance 100 then saturation -50 gives 0.51 * 0.5.
        let (_, s3, _) = hsl(op(100.0, -50.0).apply(input));
        assert!(near(s3, 0.255, 2e-4), "{s3}");
    }

    #[test]
    fn grey_pixels_are_unchanged_and_output_stays_in_gamut() {
        for v in [0.0_f32, 0.25, 0.5, 1.0] {
            let out = op(100.0, 100.0).apply([v, v, v]);
            assert_eq!(out, [v, v, v]);
        }
        let wild = [
            [2.0_f32, -1.0, 0.5],
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
            [0.0, 0.0, 1.0],
        ];
        for rgb in wild {
            for (v, s) in [(100.0, 100.0), (-100.0, 100.0), (50.0, -100.0)] {
                let out = op(v, s).apply(rgb);
                assert!(out.iter().all(|c| (0.0..=1.0).contains(c)), "{rgb:?} {out:?}");
            }
        }
    }

    #[test]
    fn the_map_is_a_pure_per_pixel_function() {
        let a = op(60.0, 20.0);
        let pixel = [0.7, 0.3, 0.4];
        assert_eq!(a.apply(pixel), a.apply(pixel));
        assert_eq!(a.apply(pixel), op(60.0, 20.0).apply(pixel));
    }
}
