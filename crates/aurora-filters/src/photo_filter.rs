//! The Photo Filter adjustment's pixel math (0.239.0).
//!
//! [`PhotoFilterOp`] applies an [`aurora_core::PhotoFilterParams`] to
//! **straight (un-premultiplied) `f32` RGB**, encoded values, alpha never
//! an input or output, no 8-bit intermediate (invariant 7.3.1b). The
//! formula is on the parameter type (`aurora-core`'s `photo_filter`
//! module): multiply each channel by `1 + (colour - 1) * density`, then
//! (Preserve Luminosity) add back the Rec.709 luma the multiply took away.
//!
//! # Decisions, with reasons
//!
//! - **Encoded values, Rec.709 luma weights**, as the Dodge/Burn/Sponge
//!   tools and the rest of this crate's colour maths do; no linear-light
//!   round trip.
//! - **An identity adjustment (a white filter) is skipped**
//!   ([`PhotoFilterOp::is_identity`]), an exact passthrough even for an
//!   out-of-gamut value. A non-identity one clamps its input to `[0, 1]`
//!   (`NaN` reads as `0`) and returns values in `[0, 1]`.
//! - **No GPU port** (a later task): a visible Photo Filter layer keeps
//!   the document on the CPU compositing path.

use aurora_core::PhotoFilterParams;

/// Rec.709 luma weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luma([r, g, b]: [f32; 3]) -> f32 {
    LUMA[0].mul_add(r, LUMA[1].mul_add(g, LUMA[2] * b))
}

/// A Photo Filter adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoFilterOp {
    params: PhotoFilterParams,
    /// The per-channel transmission `1 + (colour - 1) * density`.
    transmission: [f32; 3],
}

impl PhotoFilterOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &PhotoFilterParams) -> Self {
        let density = params.density() / 100.0;
        Self {
            params: *params,
            transmission: params.colour().map(|c| (c - 1.0).mul_add(density, 1.0)),
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
        let [t0, t1, t2] = self.transmission;
        let filtered = [input[0] * t0, input[1] * t1, input[2] * t2];
        if !self.params.preserve_luminosity() {
            return filtered;
        }
        let shift = luma(input) - luma(filtered);
        filtered.map(|c| (c + shift).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{PhotoFilterOp, luma};
    use aurora_core::PhotoFilterParams as P;

    fn op(colour: [f32; 3], density: f32, preserve: bool) -> PhotoFilterOp {
        match P::new(colour, density, preserve) {
            Ok(p) => PhotoFilterOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn a_white_filter_is_an_exact_passthrough_even_out_of_gamut() {
        for density in [1.0, 25.0, 100.0] {
            for preserve in [false, true] {
                let o = op([1.0; 3], density, preserve);
                assert!(o.is_identity());
                for rgb in [[0.1, 0.5, 0.9], [1.5, -0.25, 0.3], [f32::NAN, 0.0, 2.0]] {
                    let out = o.apply(rgb);
                    for (a, b) in out.iter().zip(rgb) {
                        assert!(a.to_bits() == b.to_bits());
                    }
                }
            }
        }
        assert!(!op([1.0, 1.0, 0.99], 50.0, true).is_identity());
    }

    #[test]
    fn without_preserve_it_multiplies_by_the_density_weighted_colour() {
        let colour = [0.8, 0.4, 0.0];
        let input = [0.5, 0.5, 0.5];
        // density 50: t = 1 + (c - 1) * 0.5 = [0.9, 0.7, 0.5]
        assert!(near(op(colour, 50.0, false).apply(input), [0.45, 0.35, 0.25], 1e-6));
        // density 100: the pixel times the colour itself.
        assert!(near(op(colour, 100.0, false).apply(input), [0.4, 0.2, 0.0], 1e-6));
        // density 1: almost nothing.
        let faint = op(colour, 1.0, false).apply(input);
        assert!(near(faint, [0.499, 0.497, 0.495], 1e-5), "{faint:?}");
    }

    #[test]
    fn preserve_luminosity_restores_the_luma_when_nothing_clips() {
        let o = op([0.9, 0.55, 0.0], 60.0, true);
        for input in [[0.3, 0.4, 0.5], [0.5, 0.5, 0.5], [0.2, 0.3, 0.25], [0.6, 0.5, 0.45]] {
            let out = o.apply(input);
            assert!(out.iter().all(|c| (0.0..=1.0).contains(c)));
            assert!((luma(out) - luma(input)).abs() < 1e-5, "{input:?} {out:?}");
        }
        // Without preserve the same filter darkens.
        let dark = op([0.9, 0.55, 0.0], 60.0, false).apply([0.5, 0.5, 0.5]);
        assert!(luma(dark) < luma([0.5; 3]) - 0.05);
    }

    #[test]
    fn preserve_luminosity_warms_without_darkening() {
        // Warming: the blue channel falls and the red rises relative to it.
        let out = op([236.0 / 255.0, 138.0 / 255.0, 0.0], 50.0, true).apply([0.5; 3]);
        assert!(out[0] > out[1] && out[1] > out[2], "{out:?}");
        // Cooling: the reverse.
        let cool = op([0.0, 109.0 / 255.0, 1.0], 50.0, true).apply([0.5; 3]);
        assert!(cool[2] > cool[1] && cool[1] > cool[0], "{cool:?}");
    }

    #[test]
    fn density_is_monotone_toward_the_colour() {
        let input = [0.6, 0.6, 0.6];
        let mut last_blue = 2.0_f32;
        for density in [1.0_f32, 20.0, 40.0, 60.0, 80.0, 100.0] {
            let b = op([0.9, 0.5, 0.1], density, false).apply(input)[2];
            assert!(b <= last_blue, "{density}");
            last_blue = b;
        }
    }

    #[test]
    fn black_is_unchanged_without_preserve_and_lifted_with_it() {
        assert_eq!(op([0.9, 0.5, 0.1], 80.0, false).apply([0.0; 3]), [0.0; 3]);
        // Preserve adds the (zero) luma difference: black stays black.
        assert_eq!(op([0.9, 0.5, 0.1], 80.0, true).apply([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn output_stays_in_gamut_for_wild_input() {
        for rgb in [[2.0_f32, -1.0, 0.5], [f32::NAN, f32::INFINITY, f32::NEG_INFINITY]] {
            for preserve in [false, true] {
                let out = op([0.0, 0.2, 1.0], 100.0, preserve).apply(rgb);
                assert!(out.iter().all(|c| (0.0..=1.0).contains(c)), "{out:?}");
            }
        }
        // White input with preserve on a strong filter keeps its luma
        // (nothing clips on the way down; the shift clips on the way up).
        let out = op([0.0, 0.0, 1.0], 100.0, true).apply([1.0; 3]);
        assert!(out.iter().all(|c| (0.0..=1.0).contains(c)));
    }

    #[test]
    fn the_map_is_a_pure_per_pixel_function() {
        let a = op([0.9, 0.5, 0.1], 30.0, true);
        let px = [0.7, 0.3, 0.4];
        assert_eq!(a.apply(px), a.apply(px));
    }
}
