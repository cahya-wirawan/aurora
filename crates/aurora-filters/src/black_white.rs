//! The Black & White adjustment's pixel math (0.240.0).
//!
//! [`BlackWhiteOp`] applies an [`aurora_core::BlackWhiteParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded values, alpha never
//! an input or output, no 8-bit intermediate (invariant 7.3.1b). The
//! formula is on the parameter type (`aurora-core`'s `black_white`
//! module): the pixel splits exactly into white, a primary and a secondary
//! part by its sorted channels, and the grey is the same split of the six
//! slider values, `min + (max - mid) * w(primary) + (mid - min) *
//! w(secondary)`.
//!
//! # Decisions, with reasons
//!
//! - **No hue is computed.** The split is the piecewise-linear
//!   interpolation of the six slider values between the hue anchors
//!   (pure colours at 0, 60, ... 300 degrees) written without the hue, so
//!   it is continuous at every sector edge, free of the HSL hue's division
//!   and of `atan2`, and cheap.
//! - **Scaled by chroma**: a neutral pixel keeps its value, black and
//!   white stay black and white, whatever the sliders say; there is no
//!   "luma-like mean" for neutrals because none is needed.
//! - **Never an identity**: even the default sliders turn a coloured pixel
//!   grey, so [`BlackWhiteOp::is_identity`] is `false` and the layer always
//!   runs. Input is clamped to `[0, 1]` (`NaN` reads as `0`); output is in
//!   `[0, 1]`.
//! - **Tint** is the HSL colour with the tint hue and saturation whose
//!   lightness is the grey (so the grey's lightness is kept), via the
//!   Hue/Saturation maths this crate already has.
//! - **No GPU port** (a later task): a visible Black & White layer keeps
//!   the document on the CPU compositing path.

use crate::hue_saturation::from_hsl;
use aurora_core::BlackWhiteParams;

/// A Black & White adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackWhiteOp {
    /// The six sliders as fractions (`value / 100`), reds, yellows, greens,
    /// cyans, blues, magentas.
    weights: [f32; 6],
    /// `Some((hue degrees, saturation 0..1))` with Tint on.
    tint: Option<(f32, f32)>,
}

/// The slider slot of the pure primary whose channel index is `channel`
/// (red 0, green 1, blue 2).
const fn primary_slot(channel: u8) -> usize {
    channel as usize * 2
}

/// The slider slot of the pure secondary made of channels `a` and `b`
/// (distinct): red + green yellow (1), green + blue cyan (3), red + blue
/// magenta (5).
const fn secondary_slot(a: u8, b: u8) -> usize {
    match a + b {
        1 => 1,
        3 => 3,
        _ => 5,
    }
}

impl BlackWhiteOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &BlackWhiteParams) -> Self {
        Self {
            weights: params.weights().map(|w| w / 100.0),
            tint: params
                .tint()
                .then(|| (params.tint_hue(), params.tint_saturation() / 100.0)),
        }
    }

    /// Whether [`Self::apply`] returns its input unchanged: never.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        false
    }

    /// The grey of one straight, encoded RGB pixel, in `[0, 1]`.
    #[must_use]
    pub fn grey(&self, rgb: [f32; 3]) -> f32 {
        let [r, g, b] = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        // Sort descending with a three-step network; the strict `<` keeps
        // equal channels in index order (red, green, blue).
        let (mut first, mut second, mut third) = ((r, 0_u8), (g, 1_u8), (b, 2_u8));
        if first.0 < second.0 {
            std::mem::swap(&mut first, &mut second);
        }
        if second.0 < third.0 {
            std::mem::swap(&mut second, &mut third);
        }
        if first.0 < second.0 {
            std::mem::swap(&mut first, &mut second);
        }
        let primary = self
            .weights
            .get(primary_slot(first.1))
            .copied()
            .unwrap_or(0.0);
        let secondary = self
            .weights
            .get(secondary_slot(first.1, second.1))
            .copied()
            .unwrap_or(0.0);
        let grey =
            (first.0 - second.0).mul_add(primary, third.0) + (second.0 - third.0) * secondary;
        grey.clamp(0.0, 1.0)
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let grey = self.grey(rgb);
        match self.tint {
            None => [grey; 3],
            Some((hue, saturation)) => from_hsl(hue, saturation, grey),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
mod tests {
    use super::BlackWhiteOp;
    use crate::hue_saturation::to_hsl;
    use aurora_core::{BLACK_WHITE_DEFAULTS, BlackWhiteParams as P};

    fn params(weights: [f32; 6], tint: bool, hue: f32, sat: f32) -> P {
        match P::new(weights, tint, hue, sat) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn op(weights: [f32; 6]) -> BlackWhiteOp {
        BlackWhiteOp::new(&params(weights, false, 0.0, 0.0))
    }

    fn tinted(weights: [f32; 6], hue: f32, sat: f32) -> BlackWhiteOp {
        BlackWhiteOp::new(&params(weights, true, hue, sat))
    }

    /// An independent model sharing no code with the op: the textbook hue
    /// angle, then a linear interpolation of the slider values between the
    /// anchors at 0, 60, ... 300 degrees, `grey = min + chroma * w(hue)`.
    fn model_grey(weights: [f32; 6], rgb: [f32; 3]) -> f64 {
        let [r, g, b] = rgb.map(f64::from);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        if chroma == 0.0 {
            return max;
        }
        let hue = if max == r {
            ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        } * 60.0;
        let sector = (hue / 60.0).floor();
        let t = hue / 60.0 - sector;
        let slot = |i: f64| f64::from(weights[(i as usize) % 6]) / 100.0;
        let w = slot(sector) * (1.0 - t) + slot(sector + 1.0) * t;
        (min + chroma * w).clamp(0.0, 1.0)
    }

    /// A deterministic spread of pixels, including ties and extremes.
    fn pixels() -> Vec<[f32; 3]> {
        let mut out = Vec::new();
        let levels = [0.0_f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
        for &r in &levels {
            for &g in &levels {
                for &b in &levels {
                    out.push([r, g, b]);
                }
            }
        }
        let mut state = 0x1234_5678_u32;
        for _ in 0..3000 {
            let mut next = || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / 16_777_216.0
            };
            out.push([next(), next(), next()]);
        }
        out
    }

    #[test]
    fn the_default_sliders_send_each_pure_colour_to_its_grey() {
        let o = op(BLACK_WHITE_DEFAULTS);
        let cases = [
            ([1.0, 0.0, 0.0], 0.4),
            ([1.0, 1.0, 0.0], 0.6),
            ([0.0, 1.0, 0.0], 0.4),
            ([0.0, 1.0, 1.0], 0.6),
            ([0.0, 0.0, 1.0], 0.2),
            ([1.0, 0.0, 1.0], 0.8),
        ];
        for (rgb, want) in cases {
            let out = o.apply(rgb);
            for c in out {
                assert!((c - want).abs() < 1e-6, "{rgb:?} -> {out:?}, want {want}");
            }
        }
        // A pale red keeps its floor of white: min + chroma * 0.4.
        let pale = o.apply([0.8, 0.5, 0.5]);
        assert!((pale[0] - (0.5 + 0.3 * 0.4)).abs() < 1e-6, "{pale:?}");
    }

    #[test]
    fn neutrals_black_and_white_are_untouched_whatever_the_sliders() {
        for weights in [
            BLACK_WHITE_DEFAULTS,
            [-200.0; 6],
            [300.0; 6],
            [300.0, -200.0, 300.0, -200.0, 300.0, -200.0],
        ] {
            let o = op(weights);
            for v in [0.0_f32, 0.003, 0.25, 0.5, 0.731, 1.0] {
                assert_eq!(o.apply([v; 3]), [v; 3], "{weights:?} {v}");
            }
        }
    }

    #[test]
    fn it_matches_an_independent_hue_interpolation_model() {
        for weights in [
            BLACK_WHITE_DEFAULTS,
            [-200.0, 300.0, 0.0, 100.0, -50.0, 250.0],
            [100.0; 6],
            [0.0; 6],
            [300.0, 300.0, -200.0, -200.0, 50.0, 150.0],
        ] {
            let o = op(weights);
            for rgb in pixels() {
                let want = model_grey(weights, rgb);
                let got = f64::from(o.grey(rgb));
                assert!(
                    (got - want).abs() < 2e-6,
                    "{weights:?} {rgb:?}: {got} vs {want}"
                );
                assert_eq!(o.apply(rgb), [o.grey(rgb); 3]);
            }
        }
    }

    #[test]
    fn swapping_red_and_blue_swaps_their_sliders_and_the_secondaries() {
        let w = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
        // red <-> blue: reds <-> blues, yellows <-> cyans, greens and
        // magentas stay.
        let swapped = [w[4], w[3], w[2], w[1], w[0], w[5]];
        let (a, b) = (op(w), op(swapped));
        for [r, g, bl] in pixels() {
            let x = a.grey([r, g, bl]);
            let y = b.grey([bl, g, r]);
            assert!((x - y).abs() < 1e-6, "{r} {g} {bl}");
        }
    }

    #[test]
    fn a_slider_moves_only_the_hues_within_sixty_degrees_of_its_anchor() {
        let base = BLACK_WHITE_DEFAULTS;
        for slot in 0..6 {
            let mut raised = base;
            raised[slot] += 20.0;
            let (a, b) = (op(base), op(raised));
            // Hue h at full saturation, value 1: rgb of hue degrees.
            for step in 0..360 {
                let h = f64::from(step);
                let rgb = hue_rgb(h);
                let d = (b.grey(rgb) - a.grey(rgb)).abs();
                let anchor = (slot * 60) as f64;
                let dist = (h - anchor + 180.0).rem_euclid(360.0) - 180.0;
                if dist.abs() >= 60.0 {
                    assert!(d < 1e-6, "slot {slot} hue {h}: {d}");
                } else {
                    // Triangular: 1 - |dist| / 60 of the 0.2 raise.
                    let want = 0.2 * (1.0 - dist.abs() / 60.0);
                    assert!(
                        (f64::from(d) - want).abs() < 2e-5,
                        "slot {slot} hue {h}: {d}"
                    );
                }
            }
        }
    }

    fn hue_rgb(h: f64) -> [f32; 3] {
        let x = 1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs();
        let (r, g, b) = match (h / 60.0).floor() as i32 {
            0 => (1.0, x, 0.0),
            1 => (x, 1.0, 0.0),
            2 => (0.0, 1.0, x),
            3 => (0.0, x, 1.0),
            4 => (x, 0.0, 1.0),
            _ => (1.0, 0.0, x),
        };
        [r as f32, g as f32, b as f32]
    }

    #[test]
    fn the_grey_is_continuous_and_monotone_in_every_slider() {
        let o = op([-120.0, 250.0, 10.0, 90.0, -30.0, 200.0]);
        // Continuity along the hue circle: the grey of a fully saturated
        // colour moves by at most the largest slider step per degree.
        let mut last = o.grey(hue_rgb(0.0));
        for step in 1..=3600 {
            let now = o.grey(hue_rgb(f64::from(step) / 10.0));
            assert!((now - last).abs() < 0.0127, "step {step}: {now} vs {last}");
            last = now;
        }
        // Monotone: a larger slider never gives a smaller grey.
        for rgb in pixels() {
            for slot in 0..6 {
                let mut lo = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];
                let mut hi = lo;
                lo[slot] = -50.0;
                hi[slot] = 150.0;
                assert!(
                    op(hi).grey(rgb) >= op(lo).grey(rgb) - 1e-6,
                    "{slot} {rgb:?}"
                );
            }
        }
    }

    #[test]
    fn the_output_is_clamped_and_wild_input_stays_in_gamut() {
        let o = op([300.0, 300.0, 300.0, 300.0, 300.0, 300.0]);
        assert_eq!(o.apply([1.0, 0.0, 0.0]), [1.0; 3], "300 % of white clamps");
        let dark = op([-200.0; 6]);
        assert_eq!(
            dark.apply([1.0, 0.0, 0.0]),
            [0.0; 3],
            "-200 % clamps to black"
        );
        for rgb in [
            [2.0_f32, -1.0, 0.5],
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
            [1e30, -1e30, 0.0],
        ] {
            for o in [
                op(BLACK_WHITE_DEFAULTS),
                tinted(BLACK_WHITE_DEFAULTS, 35.0, 25.0),
                o,
                dark,
            ] {
                let out = o.apply(rgb);
                assert!(
                    out.iter().all(|c| (0.0..=1.0).contains(c)),
                    "{rgb:?} {out:?}"
                );
            }
        }
        assert!(!o.is_identity() && !op(BLACK_WHITE_DEFAULTS).is_identity());
    }

    #[test]
    fn tint_puts_the_hue_and_saturation_at_the_greys_lightness() {
        let weights = BLACK_WHITE_DEFAULTS;
        let plain = op(weights);
        let sepia = tinted(weights, 35.0, 25.0);
        for rgb in [
            [0.8, 0.4, 0.2],
            [0.3, 0.6, 0.9],
            [0.5, 0.5, 0.5],
            [0.2, 0.9, 0.4],
        ] {
            let grey = plain.grey(rgb);
            let out = sepia.apply(rgb);
            let (hue, s, l) = to_hsl(out);
            assert!((l - grey).abs() < 1e-5, "lightness is the grey: {rgb:?}");
            if grey > 0.05 && grey < 0.95 {
                assert!((s - 0.25).abs() < 1e-4, "saturation {s}");
                assert!((hue - 35.0).abs() < 0.05, "hue {hue}");
            }
        }
        // Black and white have no room for a tint.
        assert_eq!(sepia.apply([0.0; 3]), [0.0; 3]);
        assert_eq!(sepia.apply([1.0; 3]), [1.0; 3]);
        // Zero saturation is the plain grey; the tint flag off ignores hue.
        assert_eq!(
            tinted(weights, 200.0, 0.0).apply([0.8, 0.4, 0.2]),
            plain.apply([0.8, 0.4, 0.2])
        );
        let off = BlackWhiteOp::new(&params(weights, false, 200.0, 90.0));
        assert_eq!(off.apply([0.8, 0.4, 0.2]), plain.apply([0.8, 0.4, 0.2]));
        // Another hue gives another colour.
        let blue = tinted(weights, 220.0, 25.0).apply([0.8, 0.4, 0.2]);
        assert!(blue[2] > blue[0], "{blue:?}");
        let warm = sepia.apply([0.8, 0.4, 0.2]);
        assert!(warm[0] > warm[2], "{warm:?}");
    }

    #[test]
    fn the_map_is_a_pure_per_pixel_function() {
        let a = tinted([1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 10.0, 50.0);
        let px = [0.7, 0.3, 0.4];
        assert_eq!(a.apply(px), a.apply(px));
    }
}
