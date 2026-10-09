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
//!   `i / 16384` are exact in `f32`.
//! - **The table holds the spline before its clamp, and the lookup
//!   clamps** (0.157.0). [`aurora_core::ToneCurve`] is a natural cubic
//!   spline clamped to `[0, 1]` (Photoshop's model), and the clamp is
//!   load-bearing: a curve may cross `0` or `1` with a slope of several
//!   hundred. Interpolating samples of the *clamped* curve would put that
//!   kink inside one table interval, an error of up to `slope * h / 4` —
//!   measured at `3.1e-3` (most of an 8-bit level) on the
//!   minimum-separation zigzag in the tests. Clamping after
//!   interpolation instead is `1`-Lipschitz, so the error is the smooth
//!   spline's own: `h^2 / 8 * max|f''|` with `h = 1/16384`. The natural
//!   spline's second derivative is linear on each interval between its
//!   knot values `M_i`, and diagonal dominance of its tridiagonal system
//!   bounds every `|M_i|` by `max_j 6 / (h_{j-1} h_j) <= 6 * 256^2` for points
//!   at least `1/256` apart with outputs in `[0, 1]` (see
//!   `aurora_core::tone_curve`'s `natural_second_derivatives`). So the
//!   bound is `6 * 256^2 / (8 * 16384^2) ~= 1.83e-4` — numerically the
//!   same figure 0.155.0 derived for Fritsch–Carlson's Hermite segments,
//!   by a different argument — plus `f32` rounding of samples that may
//!   lie far outside `[0, 1]` (an overshoot of up to a few hundred,
//!   rounding at `~1e-5` of it at worst). Under one 12-bit code value; a
//!   typical curve measures below `1e-6` (see the tests, which measure
//!   both against [`aurora_core::ToneCurve::evaluate`]).
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

/// One sampled curve, `CURVES_LUT_INTERVALS + 1` values, plus the
/// curve's own input range (0.158.0, [`ToneCurve::input_range`]).
///
/// **Moved endpoints.** A curve whose first or last point is interior is
/// flat beyond it. The samples are the spline *continued* past the
/// endpoints ([`ToneCurve::evaluate_extrapolated`]) and the lookup clamps
/// its input to the range before interpolating, so the flat extension's
/// kink sits exactly at the clamp and never inside a table interval —
/// sampling the flat curve itself would put a slope discontinuity of up
/// to several hundred inside one interval (`slope * h / 4`, most of an
/// 8-bit level). The interval straddling a moved endpoint interpolates
/// the continued cubic, which is within one interval width of its knot,
/// so the error bound in the module docs holds unchanged.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    samples: Vec<f32>,
    range: (f32, f32),
}

impl Table {
    /// `None` for an identity curve (skipped, so it passes values through
    /// exactly).
    fn of(curve: Option<&ToneCurve>) -> Option<Self> {
        let curve = curve.filter(|curve| !curve.is_identity())?;
        #[allow(clippy::cast_precision_loss)]
        let last = CURVES_LUT_INTERVALS as f32;
        Some(Self {
            samples: (0..=CURVES_LUT_INTERVALS)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let x = i as f32 / last;
                    curve.evaluate_extrapolated(x)
                })
                .collect(),
            range: curve.input_range(),
        })
    }

    /// Linear interpolation between the two samples around `x`, which is
    /// clamped to `[0, 1]` first (`NaN` reads as `0.0`), and the result
    /// clamped to `[0, 1]` after — the curve's own output clamp, applied
    /// once the interpolation is done (see the module docs).
    fn lookup(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        // Flat beyond a moved endpoint (0.158.0): clamped to the curve's
        // own range before the table is read, see [`Table`].
        let (lo, hi) = self.range;
        let x = x.clamp(lo, hi.max(lo));
        #[allow(clippy::cast_precision_loss)]
        let position = x * CURVES_LUT_INTERVALS as f32;
        // `position` is in `[0, INTERVALS]`, so the truncation is exact
        // and non-negative; the last sample's own interval is the one
        // before it, at `frac == 1.0`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = (position as usize).min(CURVES_LUT_INTERVALS - 1);
        #[allow(clippy::cast_precision_loss)]
        let frac = position - index as f32;
        match (self.samples.get(index), self.samples.get(index + 1)) {
            (Some(&low), Some(&high)) => (high - low).mul_add(frac, low).clamp(0.0, 1.0),
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

/// How many `f32`s lead [`CurvesLut::packed_for_gpu`]'s output: four
/// `[present, range_lo, range_hi, 0.0]` records, red, green, blue,
/// composite (0.159.0).
pub const CURVES_PACKED_HEADER: usize = 16;

/// The length of [`CurvesLut::packed_for_gpu`]'s output: the header plus
/// four tables of [`CURVES_LUT_INTERVALS`]` + 1` samples (0.159.0) —
/// 65,556 `f32`s, 262,224 bytes.
pub const CURVES_PACKED_LEN: usize = CURVES_PACKED_HEADER + 4 * (CURVES_LUT_INTERVALS + 1);

impl CurvesLut {
    /// The tables flattened into one `f32` array for the GPU Curves pass
    /// (0.159.0, `aurora_render::TileCompositor::composite_curves_with_opacity`,
    /// which documents and checks the same layout from its side — this
    /// crate sits beside `aurora-render`, not below it, so neither can
    /// name the other's constants; `aurora-app` asserts they agree).
    ///
    /// Layout, every value `f32`:
    /// - `[0, 16)`: four records `[present, lo, hi, 0.0]` for red, green,
    ///   blue and composite in that order. `present` is `1.0` for a
    ///   tabulated curve and `0.0` for an identity one (the shader then
    ///   passes the value through exactly, as [`Self::map_channel`]
    ///   does); `lo`/`hi` are the table's own input range.
    /// - `[16 + t * 16385, 16 + (t + 1) * 16385)`: table `t`'s samples,
    ///   all zero for an identity curve.
    ///
    /// The samples are this table's own, bit for bit — no re-sampling and
    /// no narrowing (invariant §7.3.1b: no 8-bit, and here no `f16`
    /// either).
    ///
    /// **Empty** if any table does not hold exactly
    /// [`CURVES_LUT_INTERVALS`]` + 1` samples (unreachable through
    /// [`Self::new`]; 0.159.0 review). The GPU side refuses any length but
    /// [`CURVES_PACKED_LEN`], so the caller composites on the CPU rather
    /// than the GPU reading a flagged-present table of zeros (which would
    /// black that channel out).
    #[must_use]
    pub fn packed_for_gpu(&self) -> Vec<f32> {
        let tables = [
            self.channels.first().and_then(Option::as_ref),
            self.channels.get(1).and_then(Option::as_ref),
            self.channels.get(2).and_then(Option::as_ref),
            self.composite.as_ref(),
        ];
        let mut out = Vec::with_capacity(CURVES_PACKED_LEN);
        for table in tables {
            match table {
                Some(table) => out.extend([1.0, table.range.0, table.range.1, 0.0]),
                None => out.extend([0.0; 4]),
            }
        }
        for table in tables {
            match table {
                Some(table) if table.samples.len() == CURVES_LUT_INTERVALS + 1 => {
                    out.extend_from_slice(&table.samples);
                }
                Some(_) => return Vec::new(),
                None => out.extend(std::iter::repeat_n(0.0, CURVES_LUT_INTERVALS + 1)),
            }
        }
        out
    }
}

impl From<&CurvesParams> for CurvesLut {
    fn from(params: &CurvesParams) -> Self {
        Self::new(params)
    }
}

#[cfg(test)]
mod tests {
    use super::{CURVES_LUT_INTERVALS, CURVES_PACKED_HEADER, CURVES_PACKED_LEN, CurvesLut, Table};
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

    /// Monotone *for this data*: a natural spline is not monotone in
    /// general (0.157.0), but this one is, and the table must not add a
    /// reversal the curve does not have.
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
        // A full rise across one 1/256 interval: the natural spline
        // overshoots both ways around it, so the clamp is crossed steeply.
        let step = 1.0 / 256.0;
        let steepest = max_lut_error(&[(0.0, 0.0), (0.5, 0.0), (0.5 + step, 1.0), (1.0, 1.0)]);
        // The curve with the largest second derivatives the invariants
        // allow: 19 points alternating 0 / 1 at minimum separation, every
        // `|M_i|` near its `6 * 256^2` ceiling and the clamp crossed
        // dozens of times.
        #[allow(clippy::cast_precision_loss)]
        let zigzag: Vec<(f32, f32)> = (0..aurora_core::MAX_POINTS)
            .map(|i| {
                let x = if i + 1 == aurora_core::MAX_POINTS {
                    1.0
                } else {
                    i as f32 * step
                };
                (x, if i % 2 == 0 { 0.0 } else { 1.0 })
            })
            .collect();
        let zigzag = max_lut_error(&zigzag);
        println!(
            "CurvesLut ({CURVES_LUT_INTERVALS} intervals) max |lut - evaluate|: typical \
             S-curve {typical:e}, one-step rise {steepest:e}, minimum-separation zigzag \
             {zigzag:e}"
        );
        assert!(typical < 1e-6, "typical curve error {typical}");
        for (name, error) in [("one-step rise", steepest), ("zigzag", zigzag)] {
            assert!(
                error < 1.9e-4,
                "{name} error {error} exceeds the derived 1.83e-4 bound"
            );
        }
    }

    #[test]
    fn from_params_matches_new() {
        let params = composite_only(&[(0.0, 0.2), (1.0, 0.7)]);
        assert_eq!(CurvesLut::from(&params), CurvesLut::new(&params));
        assert!(!CurvesLut::new(&params).is_identity());
    }

    /// 0.158.0: a curve with moved endpoints — the steepest possible
    /// rise right at a moved first point, and `curves_rgb.psd`'s own
    /// `Curves 4` red — is flat beyond them and stays within the smooth
    /// bound everywhere, the kink included (sampling the flat curve would
    /// miss by `slope * h / 4` at the kink, ~4e-3 here).
    #[test]
    #[allow(clippy::float_cmp)] // flat means bit-for-bit the endpoint's own `y`
    fn moved_endpoints_are_flat_beyond_and_within_the_bound_at_the_kink() {
        let steep = [(0.3, 0.0), (0.3 + 1.0 / 256.0, 1.0), (0.7, 1.0)];
        let psd = [
            (49.0 / 255.0, 60.0 / 255.0),
            (94.0 / 255.0, 218.0 / 255.0),
            (102.0 / 255.0, 0.0),
            (185.0 / 255.0, 229.0 / 255.0),
            (195.0 / 255.0, 36.0 / 255.0),
        ];
        for points in [&steep[..], &psd[..]] {
            let error = max_lut_error(points);
            assert!(error < 1.9e-4, "{points:?}: {error}");
            let lut = CurvesLut::new(&composite_only(points));
            let (Some(first), Some(last)) = (points.first(), points.last()) else {
                unreachable!("two points");
            };
            for i in 0..=100_u16 {
                let x = f32::from(i) / 100.0;
                let [got, _, _] = lut.apply([x, x, x]);
                if x <= first.0 {
                    assert_eq!(got, first.1, "below x0 at {x}");
                }
                if x >= last.0 {
                    assert_eq!(got, last.1, "above xn at {x}");
                }
            }
        }
    }

    /// 0.159.0: the GPU packing is the CPU tables bit for bit, in the
    /// documented order, with identity curves flagged absent.
    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn packed_for_gpu_is_the_cpu_tables_in_order_with_identity_flagged_absent() {
        let params = CurvesParams {
            composite: curve(&[(0.1, 0.0), (0.5, 0.7), (0.9, 1.0)]),
            green: Some(curve(&[(0.0, 1.0), (1.0, 0.0)])),
            ..CurvesParams::identity()
        };
        let lut = CurvesLut::new(&params);
        let packed = lut.packed_for_gpu();
        assert_eq!(packed.len(), CURVES_PACKED_LEN);
        let header = packed.get(..CURVES_PACKED_HEADER).unwrap_or_default();
        assert_eq!(header.first(), Some(&0.0), "red is identity");
        assert_eq!(header.get(4), Some(&1.0), "green is tabulated");
        assert_eq!(header.get(8), Some(&0.0), "blue is identity");
        assert_eq!(header.get(12), Some(&1.0), "composite is tabulated");
        assert_eq!(
            header.get(13),
            Some(&0.1),
            "composite range starts at its first point"
        );
        assert_eq!(header.get(14), Some(&0.9), "and ends at its last");
        let samples = CURVES_LUT_INTERVALS + 1;
        let table = |t: usize| {
            packed
                .get(CURVES_PACKED_HEADER + t * samples..CURVES_PACKED_HEADER + (t + 1) * samples)
                .unwrap_or_default()
        };
        assert!(table(0).iter().all(|&v| v == 0.0));
        // Reading the packed table back the way the shader does gives
        // `map_channel`'s own answer, bit for bit.
        for &x in &[0.0_f32, 0.05, 0.3, 0.5, 0.77, 0.95, 1.0] {
            let green = table(1);
            let position = x * CURVES_LUT_INTERVALS as f32;
            let index = (position as usize).min(CURVES_LUT_INTERVALS - 1);
            let frac = position - index as f32;
            let (Some(&low), Some(&high)) = (green.get(index), green.get(index + 1)) else {
                unreachable!("in range");
            };
            let got = (high - low).mul_add(frac, low).clamp(0.0, 1.0);
            let want = CurvesLut::new(&CurvesParams {
                green: params.green.clone(),
                ..CurvesParams::identity()
            })
            .map_channel(1, x);
            assert_eq!(got.to_bits(), want.to_bits(), "x = {x}");
        }
    }

    /// 0.159.0 review (I6): a malformed table packs to nothing, so the GPU
    /// side refuses it, rather than to a present-flagged table of zeros.
    #[test]
    fn packed_for_gpu_is_empty_for_a_malformed_table() {
        let lut = CurvesLut {
            composite: Some(Table {
                samples: vec![0.5; 3],
                range: (0.0, 1.0),
            }),
            channels: [None, None, None],
        };
        assert!(lut.packed_for_gpu().is_empty());
    }
}
