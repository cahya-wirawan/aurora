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
//! - **GPU (0.242.0)**: `aurora-render`'s Exposure pass
//!   (`shaders/exposure.wgsl`) ports [`ExposureOp::apply`] line for line
//!   ([`ExposureOp::packed_for_gpu`] hands it `2^exposure`, the offset and
//!   `1 / gamma`, computed here once in `f32`). `pow` is guarded against a
//!   non-positive base there, since WGSL leaves that indeterminate.

use aurora_color::{linear_to_srgb, srgb_to_linear};
use aurora_core::ExposureParams;

/// How many `f32`s [`ExposureOp::packed_for_gpu`] returns (0.242.0).
pub const EXPOSURE_PACKED_LEN: usize = 4;

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

    /// The settings flattened for the GPU Exposure pass (0.242.0,
    /// `aurora_render::TileCompositor::composite_exposure_with_opacity`,
    /// `shaders/exposure.wgsl`): `[identity, 2^exposure, offset, 1 /
    /// gamma]`, `identity` as `0`/`1`; the scale and power are the very
    /// `f32`s [`Self::apply`] uses.
    #[must_use]
    pub fn packed_for_gpu(&self) -> [f32; EXPOSURE_PACKED_LEN] {
        [
            f32::from(u8::from(self.is_identity())),
            self.scale,
            self.params.offset(),
            self.power,
        ]
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

/// An Exposure adjustment imported from a Grayscale file, applied in the
/// linear light of the file's grey profile (task 112; see
/// `aurora_core::exposure_trc` for what was measured).
///
/// Per channel: the encoded grey is decoded **through the profile's table**
/// (linear interpolation between entries), `x = linear * 2^exposure +
/// offset`, `y = sign(x) |x|^(1/gamma)` is clamped to `0..=1`, and `y` is
/// encoded with the **inverse of the table** (the first entry reaching `y`,
/// interpolated linearly between neighbours). A neutral layer is an exact
/// passthrough (decode then encode would return the input to within the
/// table's resolution). CPU only: the GPU predicate refuses this
/// adjustment.
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureTrcOp {
    inner: ExposureOp,
    /// The table as linear light in `0..=1`, non-decreasing, first `0` and
    /// last `1`.
    table: Vec<f32>,
}

impl ExposureTrcOp {
    /// Prepares `params` for application.
    #[must_use]
    pub fn new(params: &aurora_core::ExposureTrcParams) -> Self {
        Self {
            inner: ExposureOp::new(&params.exposure()),
            table: params
                .trc()
                .entries()
                .iter()
                .map(|&v| f32::from(v) / f32::from(u16::MAX))
                .collect(),
        }
    }

    /// Whether [`Self::apply`] returns its input unchanged.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.inner.is_identity()
    }

    /// The linear light of an encoded grey `v` (`0..=1`): the table read at
    /// `v * (len - 1)`, interpolated.
    #[must_use]
    pub fn decode(&self, v: f32) -> f32 {
        let v = if v.is_nan() { 0.0 } else { v.max(0.0).min(1.0) };
        let span = (self.table.len() - 1) as f32;
        let pos = v * span;
        // `pos` is in `0..=span`, so the floor is a valid index.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (pos.floor() as usize).min(self.table.len() - 1);
        let lo = self.table.get(i).copied().unwrap_or(0.0);
        let hi = self.table.get(i + 1).copied().unwrap_or(lo);
        lo + (hi - lo) * (pos - i as f32)
    }

    /// The encoded grey whose table value is `y` (`0..=1`).
    #[must_use]
    pub fn encode(&self, y: f32) -> f32 {
        let y = if y.is_nan() { 0.0 } else { y.max(0.0).min(1.0) };
        let i = self.table.partition_point(|&t| t < y);
        if i == 0 {
            return 0.0;
        }
        let (Some(&hi), Some(&lo)) = (self.table.get(i), self.table.get(i - 1)) else {
            return 1.0;
        };
        let span = (self.table.len() - 1) as f32;
        let frac = if hi > lo { (y - lo) / (hi - lo) } else { 0.0 };
        (((i - 1) as f32 + frac) / span).max(0.0).min(1.0)
    }

    fn channel(&self, encoded: f32) -> f32 {
        let linear = self.decode(encoded);
        let x = linear.mul_add(self.inner.scale, self.inner.params.offset());
        let y = if x < 0.0 {
            -(-x).powf(self.inner.power)
        } else {
            x.powf(self.inner.power)
        };
        self.encode(y)
    }

    /// The adjusted colour of one straight RGB pixel (grey held as
    /// R = G = B).
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.is_identity() {
            return rgb;
        }
        rgb.map(|v| {
            let clamped = if v.is_nan() { 0.0 } else { v.max(0.0).min(1.0) };
            self.channel(clamped)
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
                    model(
                        f64::from(v),
                        f64::from(ev),
                        f64::from(offset),
                        f64::from(gamma),
                    ),
                    model(
                        f64::from(1.0 - v),
                        f64::from(ev),
                        f64::from(offset),
                        f64::from(gamma),
                    ),
                    model(
                        f64::from(0.5 * v),
                        f64::from(ev),
                        f64::from(offset),
                        f64::from(gamma),
                    ),
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

    // ------------------------------------------------------------------
    // ExposureTrcOp (task 112)
    // ------------------------------------------------------------------

    use super::ExposureTrcOp;
    use aurora_core::{ExposureTrcParams, GreyTrc};

    fn table(f: impl Fn(f64) -> f64, n: usize) -> GreyTrc {
        let entries = (0..n)
            .map(|i| {
                let v = f(i as f64 / (n - 1) as f64).clamp(0.0, 1.0) * 65535.0;
                v.round() as u16
            })
            .collect();
        match GreyTrc::new(entries) {
            Ok(t) => t,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn trc_op(ev: f32, offset: f32, gamma: f32, trc: GreyTrc) -> ExposureTrcOp {
        match P::new(ev, offset, gamma) {
            Ok(p) => ExposureTrcOp::new(&ExposureTrcParams::new(p, trc)),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn srgb_decode(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// The model in `f64`: the table read by interpolation to decode, and
    /// inverted by brute force to encode.
    fn trc_model(v: f64, ev: f64, offset: f64, gamma: f64, t: &[u16]) -> f64 {
        let at = |i: usize| f64::from(t.get(i).copied().unwrap_or(0)) / 65535.0;
        let pos = v.clamp(0.0, 1.0) * (t.len() - 1) as f64;
        let i = pos.floor() as usize;
        let lin = at(i) + (at((i + 1).min(t.len() - 1)) - at(i)) * (pos - i as f64);
        let x = lin * 2.0_f64.powf(ev) + offset;
        let y = (x.signum() * x.abs().powf(1.0 / gamma)).clamp(0.0, 1.0);
        let n = t.len();
        let value = |i: usize| f64::from(t.get(i).copied().unwrap_or(0)) / 65535.0;
        for i in 0..n {
            if value(i) >= y {
                if i == 0 {
                    return 0.0;
                }
                let (lo, hi) = (value(i - 1), value(i));
                let frac = if hi > lo { (y - lo) / (hi - lo) } else { 0.0 };
                return ((i - 1) as f64 + frac) / (n - 1) as f64;
            }
        }
        1.0
    }

    #[test]
    fn trc_op_matches_an_independent_model_over_a_grid() {
        // A dot-gain-like curve, a power curve and a table with plateaus.
        let tables = [
            table(|x| x.powf(1.8) * 0.8 + x * 0.2, 256),
            table(|x| x.powf(2.2), 1024),
            table(
                |x| {
                    if x < 0.3 {
                        0.0
                    } else if x > 0.7 {
                        1.0
                    } else {
                        (x - 0.3) / 0.4
                    }
                },
                64,
            ),
        ];
        let settings = [
            (2.03, 0.0775, 1.52),
            (-0.04, 0.4144, 0.44),
            (1.25, 0.0, 1.52),
            (-1.82, 0.1203, 1.0),
            (0.0, 0.0, 2.0),
            (-6.0, 0.0, 1.0),
        ];
        for t in &tables {
            for (ev, off, g) in settings {
                let o = trc_op(ev, off, g, t.clone());
                for step in 0..=64 {
                    let v = step as f32 / 64.0;
                    let got = o.apply([v, 1.0 - v, 0.5 * v]);
                    for (c, input) in got.iter().zip([v, 1.0 - v, 0.5 * v]) {
                        let want = trc_model(
                            f64::from(input),
                            f64::from(ev),
                            f64::from(off),
                            f64::from(g),
                            t.entries(),
                        );
                        assert!(
                            (f64::from(*c) - want).abs() < 2e-4,
                            "{ev} {off} {g} v={input}: {c} vs {want}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn trc_op_with_a_srgb_table_is_the_plain_exposure() {
        let t = table(srgb_decode, 256);
        for (ev, off, g) in [(1.0, 0.0, 1.0), (-1.82, 0.1203, 1.0), (0.5, 0.05, 1.4)] {
            let a = trc_op(ev, off, g, t.clone());
            let b = op(ev, off, g);
            for step in 0..=32 {
                let v = step as f32 / 32.0;
                let (x, y) = (a.apply([v; 3]), b.apply([v; 3]));
                for (p, q) in x.iter().zip(y) {
                    assert!((p - q).abs() < 2e-3, "{ev} {off} {g} {v}: {p} vs {q}");
                }
            }
        }
    }

    #[test]
    fn trc_op_decodes_and_encodes_through_the_table() {
        // A gamma 2.2 table: mid grey decodes to 0.5^2.2 = 0.2176 linear.
        // Neutral sliders apart from gamma 2 give y = sqrt(0.2176) = 0.4665
        // and the table encodes that as 0.4665^(1/2.2) = 0.7068. Decoding
        // with sRGB instead (0.2140 linear) would give 0.7056 - close - so
        // also check a point where the curves differ strongly.
        let t = table(|x| x.powf(2.2), 1024);
        let out = trc_op(0.0, 0.0, 2.0, t.clone()).apply([0.5; 3]);
        let want = 0.5_f64.powf(2.2).sqrt().powf(1.0 / 2.2);
        for c in out {
            assert!((f64::from(c) - want).abs() < 1e-3, "{c} vs {want}");
        }
        // A table far from sRGB (linear): decode and encode are the
        // identity, so +1 stop doubles the *encoded* value.
        let lin = table(|x| x, 256);
        let out = trc_op(1.0, 0.0, 1.0, lin).apply([0.25; 3]);
        for c in out {
            assert!((c - 0.5).abs() < 1e-3, "{c}");
        }
        // The sRGB-decode alternative would give a different answer there.
        let srgb_alt = op(1.0, 0.0, 1.0).apply([0.25; 3]);
        assert!((srgb_alt[0] - 0.5).abs() > 0.05);
    }

    #[test]
    fn trc_decode_interpolates_and_clamps() {
        let o = trc_op(0.5, 0.0, 1.0, table(|x| x * x, 5));
        // Entries: 0, 1/16, 1/4, 9/16, 1 at 0, .25, .5, .75, 1.
        assert!(o.decode(0.0).abs() < 1e-6);
        assert!((o.decode(1.0) - 1.0).abs() < 1e-6);
        assert!((o.decode(0.25) - 0.0625).abs() < 1e-4);
        assert!((o.decode(0.375) - (0.0625 + 0.25) / 2.0).abs() < 1e-4);
        assert!((o.decode(-4.0)).abs() < 1e-6);
        assert!((o.decode(9.0) - 1.0).abs() < 1e-6);
        assert!(o.decode(f32::NAN).abs() < 1e-6);
    }

    #[test]
    fn trc_op_identity_is_an_exact_passthrough() {
        let o = trc_op(0.0, 0.0, 1.0, table(|x| x.powf(2.2), 256));
        assert!(o.is_identity());
        for rgb in [[0.1, 0.5, 0.9], [1.5, -0.25, 0.3], [f32::NAN, 0.0, 2.0]] {
            let out = o.apply(rgb);
            for (a, b) in out.iter().zip(rgb) {
                assert!(a.to_bits() == b.to_bits());
            }
        }
        assert!(!trc_op(0.01, 0.0, 1.0, table(|x| x, 2)).is_identity());
    }

    #[test]
    fn trc_encode_handles_plateaus_ends_and_bad_input() {
        let o = trc_op(
            0.5,
            0.0,
            1.0,
            table(|x| if x < 0.5 { 0.0 } else { (x - 0.5) * 2.0 }, 11),
        );
        // y = 0 sits on the plateau: the first entry reaching it, 0.
        assert!(o.encode(0.0).abs() < 1e-6);
        assert!(o.encode(-3.0).abs() < 1e-6);
        assert!(o.encode(f32::NAN).abs() < 1e-6);
        assert!((o.encode(1.0) - 1.0).abs() < 1e-6);
        assert!((o.encode(7.0) - 1.0).abs() < 1e-6);
        // Entry 5 is 0 and entry 6 is 0.2: y = 0.1 is half way.
        assert!((o.encode(0.1) - 0.55).abs() < 1e-4, "{}", o.encode(0.1));
        // Monotone.
        let mut last = 0.0;
        for i in 0..=200 {
            let e = o.encode(i as f32 / 200.0);
            assert!(e >= last - 1e-7);
            last = e;
        }
        // A two-entry table is the identity encode.
        let id = trc_op(1.0, 0.0, 1.0, table(|x| x, 2));
        assert!((id.encode(0.3) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn trc_op_output_is_always_in_range() {
        let o = trc_op(20.0, 0.5, 0.1, table(|x| x.powf(1.5), 256));
        for v in [-1.0, 0.0, 0.3, 1.0, 5.0, f32::NAN, f32::INFINITY] {
            for c in o.apply([v; 3]) {
                assert!((0.0..=1.0).contains(&c), "{v}: {c}");
            }
        }
    }
}
