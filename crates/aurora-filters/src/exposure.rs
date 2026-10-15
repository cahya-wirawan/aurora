//! The Exposure adjustment's pixel math (0.239.0).
//!
//! [`ExposureOp`] applies an [`aurora_core::ExposureParams`] to **straight
//! (un-premultiplied) `f32` RGB**, encoded sRGB in and out, alpha never an
//! input or output, no 8-bit intermediate (invariant 7.3.1b). The formula
//! is on the parameter type (`aurora-core`'s `exposure` module): each
//! channel goes to linear light (`aurora_color::srgb_to_linear`), becomes
//! `x = linear * 2^exposure + offset`, `y = sign(x) |x|^(1/gamma)`, is
//! clamped to `[0, 1]` and encoded again.
//!
//! # Decisions, with reasons
//!
//! - **Per channel, no table**: the map is one-dimensional, but it is
//!   computed directly in `f32` (three `powf` per channel) rather than
//!   tabulated, so the neutral case and every stop are exact to `f32`
//!   rounding. A table is a possible later optimisation (the cost is in
//!   PLAN.md's 0.239.0 entry).
//! - **An identity adjustment is skipped** ([`ExposureOp::is_identity`]), an
//!   exact passthrough even for an out-of-gamut value. A non-identity one
//!   clamps its input to `[0, 1]` (`NaN` reads as `0`) and returns values
//!   in `[0, 1]`, so a brightening exposure clips highlights as an
//!   8-bit-referred display would.
//! - **No GPU port** (a later task): a visible Exposure layer keeps the
//!   document on the CPU compositing path.

use aurora_color::{linear_to_srgb, srgb_to_linear};
use aurora_core::ExposureParams;

/// An Exposure adjustment ready to apply to pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureOp {
    params: ExposureParams,
    /// `2^exposure`, computed once.
    scale: f32,
    /// `1 / gamma`, computed once.
    power: f32,
}

impl ExposureOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &ExposureParams) -> Self {
        Self {
            params: *params,
            scale: params.exposure().exp2(),
            power: 1.0 / params.gamma(),
        }
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.params.is_identity()
    }

    /// One encoded channel in `[0, 1]` through the model.
    fn channel(&self, encoded: f32) -> f32 {
        let linear = srgb_to_linear(encoded);
        let x = linear.mul_add(self.scale, self.params.offset());
        let y = if x < 0.0 {
            -(-x).powf(self.power)
        } else {
            x.powf(self.power)
        };
        let y = y.clamp(0.0, 1.0);
        // `linear_to_srgb(1.0)` lands one ulp under 1.0; a clipped highlight
        // must be exactly white.
        if y >= 1.0 { 1.0 } else { linear_to_srgb(y) }
    }

    /// The adjusted colour of one straight, encoded RGB pixel.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.is_identity() {
            return rgb;
        }
        rgb.map(|v| {
            let clamped = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
            self.channel(clamped).clamp(0.0, 1.0)
        })
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::ExposureOp;
    use aurora_core::ExposureParams as P;

    fn op(exposure: f32, offset: f32, gamma: f32) -> ExposureOp {
        match P::new(exposure, offset, gamma) {
            Ok(p) => ExposureOp::new(&p),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// An independent model in `f64` with its own sRGB curve.
    fn model(v: f64, ev: f64, offset: f64, gamma: f64) -> f64 {
        let lin = if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        };
        let x = lin * 2.0_f64.powf(ev) + offset;
        let y = (x.signum() * x.abs().powf(1.0 / gamma)).clamp(0.0, 1.0);
        if y <= 0.003_130_8 {
            y * 12.92
        } else {
            1.055 * y.powf(1.0 / 2.4) - 0.055
        }
    }

    #[test]
    fn identity_is_an_exact_passthrough_even_out_of_gamut() {
        let identity = op(0.0, 0.0, 1.0);
        assert!(identity.is_identity());
        for rgb in [[0.1, 0.5, 0.9], [1.5, -0.25, 0.3], [f32::NAN, 0.0, 2.0]] {
            let out = identity.apply(rgb);
            for (a, b) in out.iter().zip(rgb) {
                assert!(a.to_bits() == b.to_bits(), "{rgb:?} -> {out:?}");
            }
        }
        assert!(!op(0.01, 0.0, 1.0).is_identity());
        assert!(!op(0.0, 0.001, 1.0).is_identity());
        assert!(!op(0.0, 0.0, 1.01).is_identity());
    }

    #[test]
    fn a_stop_doubles_linear_light() {
        // Mid grey 0.5 encoded is 0.21404 linear; +1 stop is 0.42808 linear,
        // which encodes to 0.68578.
        let out = op(1.0, 0.0, 1.0).apply([0.5, 0.5, 0.5]);
        for c in out {
            assert!((c - 0.685_78).abs() < 2e-4, "{c}");
        }
        // -1 stop is 0.10702 linear, 0.36078 encoded.
        let down = op(-1.0, 0.0, 1.0).apply([0.5, 0.5, 0.5]);
        for c in down {
            assert!((c - 0.360_78).abs() < 2e-4, "{c}");
        }
        // Black stays black under any exposure without an offset.
        assert_eq!(op(5.0, 0.0, 1.0).apply([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn it_matches_an_independent_f64_model_over_a_grid() {
        let settings = [
            (1.0, 0.0, 1.0),
            (-2.0, 0.0, 1.0),
            (0.5, 0.1, 1.0),
            (0.0, -0.2, 1.0),
            (0.0, 0.0, 2.2),
            (0.0, 0.0, 0.45),
            (1.5, 0.05, 0.8),
            (-3.0, 0.3, 3.0),
            (20.0, 0.0, 1.0),
            (-20.0, 0.5, 9.99),
        ];
        for (ev, offset, gamma) in settings {
            let o = op(ev, offset, gamma);
            for step in 0..=32 {
                let v = step as f32 / 32.0;
                let got = o.apply([v, 1.0 - v, 0.5 * v]);
                let want = [
                    model(f64::from(v), f64::from(ev), f64::from(offset), f64::from(gamma)),
                    model(f64::from(1.0 - v), f64::from(ev), f64::from(offset), f64::from(gamma)),
                    model(f64::from(0.5 * v), f64::from(ev), f64::from(offset), f64::from(gamma)),
                ];
                for (g, w) in got.iter().zip(want) {
                    assert!(
                        (f64::from(*g) - w).abs() < 3e-4,
                        "ev {ev} off {offset} g {gamma} v {v}: {g} vs {w}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_control_is_monotone() {
        let channel = |o: &ExposureOp, v: f32| o.apply([v, v, v])[0];
        // Brighter with more exposure, a higher offset, a higher gamma.
        for v in [0.1_f32, 0.3, 0.6, 0.9] {
            let mut last = -1.0;
            for ev in [-4.0_f32, -2.0, -1.0, 0.0, 1.0, 2.0] {
                let c = channel(&op(ev, 0.0, 1.0), v);
                assert!(c >= last, "exposure {ev} {v}");
                last = c;
            }
            let mut last = -1.0;
            for off in [-0.4_f32, -0.1, 0.0, 0.1, 0.4] {
                let c = channel(&op(0.0, off, 1.0), v);
                assert!(c >= last, "offset {off} {v}");
                last = c;
            }
            let mut last = -1.0;
            for gamma in [0.3_f32, 0.7, 1.0, 1.5, 3.0, 9.0] {
                let c = channel(&op(0.0, 0.0, gamma), v);
                assert!(c >= last, "gamma {gamma} {v}");
                last = c;
            }
        }
        // And monotone in the input for a fixed setting.
        let o = op(0.7, -0.05, 1.4);
        let mut last = -1.0;
        for step in 0..=255 {
            let c = channel(&o, step as f32 / 255.0);
            assert!(c >= last);
            last = c;
        }
    }

    #[test]
    fn gamma_brightens_above_one_and_darkens_below_and_fixes_black_and_white() {
        let mid = [0.5_f32; 3];
        assert!(op(0.0, 0.0, 2.0).apply(mid)[0] > 0.5);
        assert!(op(0.0, 0.0, 0.5).apply(mid)[0] < 0.5);
        // Linear 0 and 1 are fixed points of the power.
        for gamma in [0.2_f32, 0.5, 2.0, 9.0] {
            let o = op(0.0, 0.0, gamma);
            assert_eq!(o.apply([0.0; 3]), [0.0; 3]);
            for c in o.apply([1.0; 3]) {
                assert!((c - 1.0).abs() < 1e-5, "{gamma}: {c}");
            }
        }
    }

    #[test]
    fn a_negative_offset_clips_to_black_and_a_positive_one_lifts_black() {
        // Linear 0.01 (encoded about 0.0998) minus 0.5 is negative: black.
        let out = op(0.0, -0.5, 1.0).apply([0.1, 0.1, 0.1]);
        assert_eq!(out, [0.0; 3]);
        // Offset 0.25 lifts black to linear 0.25, encoded 0.5370.
        let lifted = op(0.0, 0.25, 1.0).apply([0.0; 3]);
        for c in lifted {
            assert!((c - 0.537).abs() < 1e-3, "{c}");
        }
    }

    #[test]
    fn extremes_clip_in_gamut_and_nan_reads_as_black() {
        for rgb in [[0.9_f32, 0.5, 0.01], [f32::NAN, 2.0, -1.0]] {
            for (ev, off, g) in [(20.0, 0.5, 9.99), (-20.0, -0.5, 0.1), (20.0, -0.5, 0.1)] {
                let out = op(ev, off, g).apply(rgb);
                assert!(
                    out.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)),
                    "{rgb:?} {out:?}"
                );
            }
        }
        let out = op(2.0, 0.0, 1.0).apply([f32::NAN, 0.0, 0.0]);
        assert_eq!(out, [0.0; 3]);
        // +20 stops pushes anything non-black to white.
        assert_eq!(op(20.0, 0.0, 1.0).apply([0.2, 0.2, 0.2]), [1.0; 3]);
    }

    #[test]
    fn channels_are_independent() {
        let o = op(1.0, 0.1, 1.3);
        let all = o.apply([0.2, 0.5, 0.8]);
        assert_eq!(all[0], o.apply([0.2, 0.0, 0.0])[0]);
        assert_eq!(all[1], o.apply([0.0, 0.5, 0.0])[1]);
        assert_eq!(all[2], o.apply([0.0, 0.0, 0.8])[2]);
    }
}
