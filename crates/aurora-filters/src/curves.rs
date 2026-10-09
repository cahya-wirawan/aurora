//! The Curves adjustment's pixel math (0.155.0).
//!
//! [`CurvesLut`] turns an [`aurora_core::CurvesParams`] into lookup
//! tables and applies them to **straight (un-premultiplied) `f32` RGB**.
//! Alpha is never an input or an output: a caller keeps its own alpha
//! untouched, which is what a Curves adjustment does.
//!
//! # Decisions, with reasons
//!
//! - **Encoded values, not linear light.** The curves map the working
//!   space's stored values directly. Aurora stores colour as the source's
//!   own encoded values promoted to `f16` (an imported sRGB PNG keeps its
//!   sRGB transfer function; nothing in `aurora-io`'s import path
//!   linearises), and every blend mode in `aurora-render` already
//!   operates on those stored values, the way Photoshop's do. Photoshop's
//!   Curves dialog also maps encoded values (its `0..255` axes are the
//!   document's own code values), so a curve drawn against the same
//!   numbers produces the same result only if it is applied to the same
//!   numbers. Linear-light curves would be a different adjustment.
//! - **Per-channel first, then composite**: `out_c = composite(channel_c(in_c))`,
//!   the order psd-tools' Photoshop-checked compositor uses (see
//!   [`aurora_core::curves`]).
//! - **No 8-bit intermediate** (invariant §7.3.1b). Each curve is sampled
//!   at [`CURVES_LUT_INTERVALS`]` + 1` evenly spaced `f32` positions and
//!   read back with linear interpolation in `f32`. The sample positions
//!   `i / 16384` are exact in `f32`. The worst-case interpolation error,
//!   for the steepest curve [`aurora_core::ToneCurve`] can hold (a full
//!   `0 -> 1` rise across one minimum-separation interval of `1/256`,
//!   whose Hermite segment has `|f''| <= 6 * 256^2`), is bounded by
//!   `h^2 / 8 * max|f''| = 6 * 256^2 / (8 * 16384^2) ~= 1.8e-4`, under one
//!   12-bit code value; a typical curve measures below `1e-6` (see the
//!   tests, which measure both against [`aurora_core::ToneCurve::evaluate`]).
//! - **Identity curves are skipped**, not tabulated, so an identity curve
//!   (or an absent channel curve) is an *exact* passthrough, bit for bit,
//!   rather than a passthrough up to interpolation rounding.
//! - **Out-of-range input is clamped to `[0, 1]` by any non-identity
//!   curve** (and `NaN` maps to `0.0`), exactly as
//!   [`aurora_core::ToneCurve::evaluate`] does; a curve's domain is
//!   `[0, 1]`. An out-of-gamut value (an unclamped float-TIFF import) that
//!   meets only identity curves is left as it was.

use aurora_core::{CurvesParams, ToneCurve};

/// How many equal intervals each curve's lookup table divides `[0, 1]`
/// into; each table holds `CURVES_LUT_INTERVALS + 1` samples (64 KiB of
/// `f32`). A power of two, so every sample position is exact in `f32`.
pub const CURVES_LUT_INTERVALS: usize = 1 << 14;

/// One sampled curve, `CURVES_LUT_INTERVALS + 1` values.
#[derive(Debug, Clone, PartialEq)]
struct Table(Vec<f32>);

impl Table {
    /// `None` for an identity curve (skipped, so it passes values through
    /// exactly).
    fn of(curve: Option<&ToneCurve>) -> Option<Self> {
        let curve = curve.filter(|curve| !curve.is_identity())?;
        #[allow(clippy::cast_precision_loss)]
        let last = CURVES_LUT_INTERVALS as f32;
        Some(Self(
            (0..=CURVES_LUT_INTERVALS)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let x = i as f32 / last;
                    curve.evaluate(x)
                })
                .collect(),
        ))
    }

    /// Linear interpolation between the two samples around `x`, which is
    /// clamped to `[0, 1]` first (`NaN` reads as `0.0`).
    fn lookup(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        #[allow(clippy::cast_precision_loss)]
        let position = x * CURVES_LUT_INTERVALS as f32;
        // `position` is in `[0, INTERVALS]`, so the truncation is exact
        // and non-negative; the last sample's own interval is the one
        // before it, at `frac == 1.0`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = (position as usize).min(CURVES_LUT_INTERVALS - 1);
        #[allow(clippy::cast_precision_loss)]
        let frac = position - index as f32;
        match (self.0.get(index), self.0.get(index + 1)) {
            (Some(&low), Some(&high)) => (high - low).mul_add(frac, low),
            // Unreachable: every table holds INTERVALS + 1 samples.
            _ => x,
        }
    }
}

/// A Curves adjustment ready to apply: one optional table per curve.
/// See the module docs for the model, order and precision.
#[derive(Debug, Clone, PartialEq)]
pub struct CurvesLut {
    composite: Option<Table>,
    channels: [Option<Table>; 3],
}

impl CurvesLut {
    /// Samples every non-identity curve of `params`.
    #[must_use]
    pub fn new(params: &CurvesParams) -> Self {
        Self {
            composite: Table::of(Some(&params.composite)),
            channels: [
                Table::of(params.red.as_ref()),
                Table::of(params.green.as_ref()),
                Table::of(params.blue.as_ref()),
            ],
        }
    }

    /// Whether applying this changes nothing (every curve was identity).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.composite.is_none() && self.channels.iter().all(Option::is_none)
    }

    /// Maps one straight RGB colour: each channel through its own curve,
    /// then all three through the composite curve. Alpha is the caller's
    /// and is never touched.
    #[must_use]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb;
        [
            self.map_channel(0, r),
            self.map_channel(1, g),
            self.map_channel(2, b),
        ]
    }

    /// Maps one value of colour channel `index` (`0` red, `1` green,
    /// `2` blue): its channel curve, then the composite curve.
    #[must_use]
    pub fn map_channel(&self, index: usize, value: f32) -> f32 {
        let after_channel = match self.channels.get(index) {
            Some(Some(table)) => table.lookup(value),
            _ => value,
        };
        match &self.composite {
            Some(table) => table.lookup(after_channel),
            None => after_channel,
        }
    }
}

impl From<&CurvesParams> for CurvesLut {
    fn from(params: &CurvesParams) -> Self {
        Self::new(params)
    }
}

#[cfg(test)]
mod tests {
    use super::{CURVES_LUT_INTERVALS, CurvesLut};
    use aurora_core::{CurvePoint, CurvesParams, ToneCurve};

    fn curve(points: &[(f32, f32)]) -> ToneCurve {
        let points: Vec<CurvePoint> = points.iter().map(|&(x, y)| CurvePoint::new(x, y)).collect();
        match ToneCurve::new(&points) {
            Ok(curve) => curve,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn composite_only(points: &[(f32, f32)]) -> CurvesParams {
        CurvesParams {
            composite: curve(points),
            ..CurvesParams::identity()
        }
    }

    /// Every `f16` value in `[0, 1]`, plus out-of-range and non-finite
    /// probes: the values a straight `f16` tile can actually hand the LUT.
    fn probes() -> Vec<f32> {
        let mut values: Vec<f32> = (0_u16..=0x3C00)
            .map(|bits| half::f16::from_bits(bits).to_f32())
            .collect();
        values.extend([-0.5, 1.5, 7.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        values
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn identity_params_are_an_exact_passthrough_bit_for_bit() {
        let lut = CurvesLut::new(&CurvesParams::identity());
        assert!(lut.is_identity());
        for x in probes() {
            let [r, g, b] = lut.apply([x, x, x]);
            for out in [r, g, b] {
                assert_eq!(out.to_bits(), x.to_bits(), "identity must not move {x}");
            }
        }
        let explicit = CurvesParams {
            red: Some(ToneCurve::identity()),
            green: Some(ToneCurve::identity()),
            blue: Some(ToneCurve::identity()),
            ..CurvesParams::identity()
        };
        assert!(CurvesLut::new(&explicit).is_identity());
    }

    #[test]
    fn known_points_map_to_their_own_outputs() {
        let lut = CurvesLut::new(&composite_only(&[(0.0, 0.1), (0.25, 0.5), (1.0, 0.9)]));
        for (x, y) in [(0.0, 0.1), (0.25, 0.5), (1.0, 0.9)] {
            for out in lut.apply([x, x, x]) {
                assert!((out - y).abs() < 1e-6, "{x} -> {out}, want {y}");
            }
        }
    }

    #[test]
    fn an_inverting_curve_inverts() {
        let lut = CurvesLut::new(&composite_only(&[(0.0, 1.0), (1.0, 0.0)]));
        for x in [0.0_f32, 0.125, 0.25, 0.5, 0.75, 1.0] {
            let [r, ..] = lut.apply([x, 0.0, 0.0]);
            assert!((r - (1.0 - x)).abs() < 1e-6, "{x} -> {r}");
        }
        // Out of range clamps into the curve's own domain first.
        let [r, g, b] = lut.apply([-3.0, 4.0, f32::NAN]);
        assert!((r - 1.0).abs() < 1e-6 && g.abs() < 1e-6 && (b - 1.0).abs() < 1e-6);
    }

    #[test]
    fn per_channel_curves_are_independent() {
        let params = CurvesParams {
            red: Some(curve(&[(0.0, 0.0), (0.5, 0.8), (1.0, 1.0)])),
            ..CurvesParams::identity()
        };
        let lut = CurvesLut::new(&params);
        let [r, g, b] = lut.apply([0.5, 0.5, 0.5]);
        assert!((r - 0.8).abs() < 1e-6, "red bent: {r}");
        assert_eq!(g.to_bits(), 0.5_f32.to_bits(), "green untouched");
        assert_eq!(b.to_bits(), 0.5_f32.to_bits(), "blue untouched");

        let params = CurvesParams {
            blue: Some(curve(&[(0.0, 1.0), (1.0, 0.0)])),
            ..CurvesParams::identity()
        };
        let [r, g, b] = CurvesLut::new(&params).apply([0.25, 0.5, 0.25]);
        assert_eq!((r, g), (0.25, 0.5), "only blue moves");
        assert!((b - 0.75).abs() < 1e-6);
    }

    #[test]
    fn channel_curve_applies_before_the_composite_curve() {
        // Two non-commuting curves: red inverts, composite lifts the
        // shadows to 0.5 at x = 0. Channel-then-composite gives
        // composite(1 - 0) = composite(1) = 1; composite-then-channel
        // would give 1 - composite(0) = 0.5.
        let params = CurvesParams {
            composite: curve(&[(0.0, 0.5), (1.0, 1.0)]),
            red: Some(curve(&[(0.0, 1.0), (1.0, 0.0)])),
            ..CurvesParams::identity()
        };
        let [r, g, _] = CurvesLut::new(&params).apply([0.0, 0.0, 0.0]);
        assert!((r - 1.0).abs() < 1e-6, "channel first, then composite: {r}");
        assert!((g - 0.5).abs() < 1e-6, "composite alone on green: {g}");
    }

    #[test]
    fn a_monotone_curve_stays_monotone_through_the_lut() {
        let lut = CurvesLut::new(&composite_only(&[
            (0.0, 0.0),
            (0.2, 0.05),
            (0.5, 0.6),
            (0.8, 0.95),
            (1.0, 1.0),
        ]));
        let mut previous = f32::NEG_INFINITY;
        for x in probes().into_iter().filter(|x| (0.0..=1.0).contains(x)) {
            let [out, ..] = lut.apply([x, 0.0, 0.0]);
            assert!(out >= previous, "not monotone at {x}: {out} < {previous}");
            assert!((0.0..=1.0).contains(&out));
            previous = out;
        }
    }

    /// The LUT's worst error against direct evaluation over every `f16`
    /// in `[0, 1]` and a dense `f32` sweep.
    fn max_lut_error(points: &[(f32, f32)]) -> f32 {
        let tone = curve(points);
        let lut = CurvesLut::new(&composite_only(points));
        let dense = (0..=(1_u32 << 20)).map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let x = i as f32 / (1_u32 << 20) as f32;
            x
        });
        probes()
            .into_iter()
            .filter(|x| (0.0..=1.0).contains(x))
            .chain(dense)
            .map(|x| (lut.apply([x, 0.0, 0.0])[0] - tone.evaluate(x)).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn lut_accuracy_against_direct_evaluation_is_measured_and_bounded() {
        let typical = max_lut_error(&[(0.0, 0.0), (0.25, 0.18), (0.75, 0.84), (1.0, 1.0)]);
        // The steepest legal curve: a full rise across one 1/256 interval.
        let step = 1.0 / 256.0;
        let steepest = max_lut_error(&[(0.0, 0.0), (0.5, 0.0), (0.5 + step, 1.0), (1.0, 1.0)]);
        println!(
            "CurvesLut ({CURVES_LUT_INTERVALS} intervals) max |lut - evaluate|: typical \
             S-curve {typical:e}, steepest legal curve {steepest:e}"
        );
        assert!(typical < 1e-6, "typical curve error {typical}");
        assert!(
            steepest < 1.9e-4,
            "steepest curve error {steepest} exceeds the derived bound"
        );
    }

    #[test]
    fn from_params_matches_new() {
        let params = composite_only(&[(0.0, 0.2), (1.0, 0.7)]);
        assert_eq!(CurvesLut::from(&params), CurvesLut::new(&params));
        assert!(!CurvesLut::new(&params).is_identity());
    }
}
