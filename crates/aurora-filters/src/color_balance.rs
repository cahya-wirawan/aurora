//! The Color Balance adjustment's pixel math (0.240.0).
//!
//! [`ColorBalanceOp`] applies an [`aurora_core::ColorBalanceParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded values, alpha never
//! an input or output, no 8-bit intermediate (invariant 7.3.1b). The
//! formula is on the parameter type (`aurora-core`'s `color_balance`
//! module): the pixel's Rec.709 luma weighs three tonal ranges (the Dodge
//! and Burn tools' shadow, midtone and highlight weights), each range's
//! three sliders move the red, green and blue channel, and Preserve
//! Luminosity subtracts the luma the shift added.
//!
//! # Decisions, with reasons
//!
//! - **Encoded values, Rec.709 luma weights**, as the Dodge/Burn/Sponge
//!   tools and the rest of this crate's colour maths do; no linear-light
//!   round trip.
//! - **The shift is computed first and clamped last.** With Preserve
//!   Luminosity the unclamped result has exactly the input's luma
//!   (`out_c = in_c + d_c - sum(luma_k * d_k)`); the clamp may then move
//!   it, as the Photo Filter's does.
//! - **An identity adjustment (every slider zero) is skipped**
//!   ([`ColorBalanceOp::is_identity`]), an exact passthrough even for an
//!   out-of-gamut value. A non-identity one clamps its input to `[0, 1]`
//!   (`NaN` reads as `0`) and returns values in `[0, 1]`.
//! - **No GPU port** (a later task): a visible Color Balance layer keeps
//!   the document on the CPU compositing path.

use aurora_core::ColorBalanceParams;

/// Rec.709 luma weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// How far a slider at 100, in the range with full weight, moves its
/// channel: half the scale.
pub const SHIFT_AT_100: f32 = 0.5;

fn luma([r, g, b]: [f32; 3]) -> f32 {
    LUMA[0].mul_add(r, LUMA[1].mul_add(g, LUMA[2] * b))
}

/// The weights of the shadows, midtones and highlights at luma `y` in
/// `[0, 1]`: `(1 - y)^2`, `4 y (1 - y)`, `y^2`.
#[must_use]
pub fn range_weights(y: f32) -> [f32; 3] {
    let inverse = 1.0 - y;
    [inverse * inverse, 4.0 * y * inverse, y * y]
}

/// A Color Balance adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorBalanceOp {
    params: ColorBalanceParams,
    /// The sliders as channel shifts at full weight: `slider / 100 *
    /// SHIFT_AT_100`, `[range][red, green, blue]`.
    shifts: [[f32; 3]; 3],
}

impl ColorBalanceOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &ColorBalanceParams) -> Self {
        Self {
            params: *params,
            shifts: params
                .ranges()
                .map(|range| range.map(|v| v / 100.0 * SHIFT_AT_100)),
        }
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
        let input = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        let [shadows, midtones, highlights] = range_weights(luma(input));
        let [s, m, h] = self.shifts;
        let delta = [0, 1, 2].map(|c| {
            let at = |range: [f32; 3]| range.get(c).copied().unwrap_or(0.0);
            shadows.mul_add(at(s), midtones.mul_add(at(m), highlights * at(h)))
        });
        let restore = if self.params.preserve_luminosity() {
            -luma(delta)
        } else {
            0.0
        };
        [0, 1, 2].map(|c| {
            let base = input.get(c).copied().unwrap_or(0.0);
            let moved = delta.get(c).copied().unwrap_or(0.0);
            (base + moved + restore).clamp(0.0, 1.0)
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::{ColorBalanceOp, SHIFT_AT_100, luma, range_weights};
    use aurora_core::ColorBalanceParams as P;

    fn params(ranges: [[f32; 3]; 3], preserve: bool) -> P {
        match P::new(ranges, preserve) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn op(ranges: [[f32; 3]; 3], preserve: bool) -> ColorBalanceOp {
        ColorBalanceOp::new(&params(ranges, preserve))
    }

    /// An independent f64 model written from the module docs.
    fn model(ranges: [[f32; 3]; 3], preserve: bool, rgb: [f32; 3]) -> [f64; 3] {
        let input = rgb.map(|v| f64::from(v).clamp(0.0, 1.0));
        let y = 0.2126 * input[0] + 0.7152 * input[1] + 0.0722 * input[2];
        let w = [(1.0 - y) * (1.0 - y), 4.0 * y * (1.0 - y), y * y];
        let mut d = [0.0_f64; 3];
        for c in 0..3 {
            for r in 0..3 {
                d[c] += w[r] * f64::from(ranges[r][c]) / 100.0 * f64::from(SHIFT_AT_100);
            }
        }
        let lum = 0.2126 * d[0] + 0.7152 * d[1] + 0.0722 * d[2];
        let mut out = [0.0; 3];
        for c in 0..3 {
            out[c] = (input[c] + d[c] - if preserve { lum } else { 0.0 }).clamp(0.0, 1.0);
        }
        out
    }

    fn pixels() -> Vec<[f32; 3]> {
        let mut out = Vec::new();
        let levels = [0.0_f32, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0];
        for &r in &levels {
            for &g in &levels {
                for &b in &levels {
                    out.push([r, g, b]);
                }
            }
        }
        out
    }

    const MIXED: [[f32; 3]; 3] = [
        [30.0, -20.0, 10.0],
        [-40.0, 60.0, -80.0],
        [100.0, 5.0, -100.0],
    ];

    #[test]
    fn all_zero_sliders_are_an_exact_passthrough_even_out_of_gamut() {
        for preserve in [false, true] {
            let o = op([[0.0; 3]; 3], preserve);
            assert!(o.is_identity());
            for rgb in [[0.1, 0.5, 0.9], [1.5, -0.25, 0.3], [f32::NAN, 0.0, 2.0]] {
                let out = o.apply(rgb);
                for (a, b) in out.iter().zip(rgb) {
                    assert_eq!(a.to_bits(), b.to_bits());
                }
            }
        }
        for r in 0..3 {
            for s in 0..3 {
                let mut ranges = [[0.0; 3]; 3];
                ranges[r][s] = 1.0;
                assert!(!op(ranges, true).is_identity(), "{r} {s}");
            }
        }
    }

    #[test]
    fn the_weights_are_the_dodge_burn_range_curves() {
        assert_eq!(range_weights(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(range_weights(1.0), [0.0, 0.0, 1.0]);
        assert_eq!(range_weights(0.5), [0.25, 1.0, 0.25]);
        for step in 0..=100 {
            let y = step as f32 / 100.0;
            let [s, m, h] = range_weights(y);
            assert!((s - (1.0 - y).powi(2)).abs() < 1e-6);
            assert!((m - 4.0 * y * (1.0 - y)).abs() < 1e-6);
            assert!((h - y * y).abs() < 1e-6);
        }
    }

    #[test]
    fn it_matches_an_independent_model() {
        for ranges in [MIXED, [[100.0; 3]; 3], [[-100.0, 0.0, 100.0]; 3]] {
            for preserve in [false, true] {
                let o = op(ranges, preserve);
                for rgb in pixels() {
                    let got = o.apply(rgb);
                    let want = model(ranges, preserve, rgb);
                    for c in 0..3 {
                        assert!(
                            (f64::from(got[c]) - want[c]).abs() < 2e-6,
                            "{ranges:?} {preserve} {rgb:?}: {got:?} vs {want:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn midtone_cyan_red_moves_red_by_the_documented_amount() {
        // Mid grey: y = 0.5, midtone weight 1, shadows and highlights 0.25.
        let mut ranges = [[0.0; 3]; 3];
        ranges[1][0] = 100.0;
        let out = op(ranges, false).apply([0.5; 3]);
        assert!((out[0] - 1.0).abs() < 1e-6, "0.5 + 0.5 * 1.0: {out:?}");
        assert_eq!((out[1], out[2]), (0.5, 0.5));
        ranges[1][0] = -40.0;
        let out = op(ranges, false).apply([0.5; 3]);
        assert!((out[0] - 0.3).abs() < 1e-6, "{out:?}");
        // Shadows barely touch a bright pixel, highlights barely a dark one.
        let mut s_only = [[0.0; 3]; 3];
        s_only[0][2] = 100.0;
        let dark = op(s_only, false).apply([0.0, 0.0, 0.0])[2];
        let bright = op(s_only, false).apply([1.0, 1.0, 1.0])[2];
        assert!((dark - 0.5).abs() < 1e-6 && (bright - 1.0).abs() < 1e-6);
        let mut h_only = [[0.0; 3]; 3];
        h_only[2][1] = -100.0;
        assert_eq!(op(h_only, false).apply([0.0; 3]), [0.0; 3]);
        let lit = op(h_only, false).apply([1.0; 3]);
        assert!((lit[1] - 0.5).abs() < 1e-6, "{lit:?}");
    }

    #[test]
    fn each_slider_moves_only_its_own_channel_monotonically() {
        for range in 0..3 {
            for channel in 0..3 {
                let mut last = -1.0_f32;
                for value in [-100.0_f32, -50.0, 0.0, 25.0, 100.0] {
                    let mut ranges = [[0.0; 3]; 3];
                    ranges[range][channel] = value;
                    let out = op(ranges, false).apply([0.4, 0.5, 0.6]);
                    for other in 0..3 {
                        if other != channel {
                            assert_eq!(out[other], [0.4, 0.5, 0.6][other], "{range} {channel}");
                        }
                    }
                    assert!(out[channel] >= last, "{range} {channel} {value}");
                    last = out[channel];
                }
            }
        }
    }

    #[test]
    fn preserve_luminosity_restores_the_luma_when_nothing_clips() {
        let ranges = [
            [20.0, -10.0, 15.0],
            [30.0, 25.0, -20.0],
            [-15.0, 10.0, 12.0],
        ];
        let kept = op(ranges, true);
        let free = op(ranges, false);
        for input in [
            [0.3, 0.4, 0.5],
            [0.5, 0.5, 0.5],
            [0.2, 0.3, 0.25],
            [0.6, 0.5, 0.45],
            [0.45, 0.5, 0.4],
        ] {
            let out = kept.apply(input);
            assert!(out.iter().all(|c| (0.0..=1.0).contains(c)));
            assert!((luma(out) - luma(input)).abs() < 1e-5, "{input:?} {out:?}");
            // Without it the luma moves.
            assert!(
                (luma(free.apply(input)) - luma(input)).abs() > 1e-3,
                "{input:?}"
            );
        }
    }

    #[test]
    fn the_output_stays_in_gamut_for_wild_input_and_saturated_sliders() {
        for preserve in [false, true] {
            for rgb in [
                [2.0_f32, -1.0, 0.5],
                [f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
                [1.0, 1.0, 1.0],
                [0.0, 0.0, 0.0],
            ] {
                let out = op([[100.0; 3], [-100.0; 3], [100.0; 3]], preserve).apply(rgb);
                assert!(out.iter().all(|c| (0.0..=1.0).contains(c)), "{out:?}");
            }
        }
    }

    #[test]
    fn the_map_is_a_pure_per_pixel_function() {
        let a = op(MIXED, true);
        let px = [0.7, 0.3, 0.4];
        assert_eq!(a.apply(px), a.apply(px));
    }
}
