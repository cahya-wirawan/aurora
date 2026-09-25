//! A tone curve: the pure, widget-free model behind a Curves adjustment
//! and the curve-editor widget (`aurora_widgets`' `curve_editor`).
//!
//! A [`ToneCurve`] maps an input level `x` in `[0, 1]` to an output level
//! `y` in `[0, 1]` through `2..=16` control points ([`MIN_POINTS`],
//! [`MAX_POINTS`]), interpolated by a **monotone cubic Hermite spline**
//! (Fritsch–Carlson). It lives here, in `aurora-core`, so the widget that
//! edits it and the future adjustment that applies it (`aurora-filters`,
//! `aurora-doc`, `aurora-io`) share one definition without either
//! depending on the other (`scripts/layering.json`).
//!
//! # Invariants
//!
//! Every [`ToneCurve`] that exists satisfies all of these — the
//! validating constructor [`ToneCurve::new`] refuses anything else, and
//! every editing method builds its candidate and re-validates it, keeping
//! the old curve on failure:
//!
//! - between [`MIN_POINTS`] and [`MAX_POINTS`] points, inclusive;
//! - every coordinate finite, every `y` in `[0, 1]`;
//! - the first point's `x` is exactly `0.0` and the last one's exactly
//!   `1.0` (the endpoints can move vertically, never horizontally);
//! - consecutive `x`s at least [`MIN_POINT_SEPARATION`] apart (so strictly
//!   increasing — the constructor never sorts silently).
//!
//! # Interpolation
//!
//! Fritsch–Carlson, computed in `f64`: secants `d_k`; an interior
//! tangent is the mean of its two neighbouring secants when both have
//! the same strict sign and `0` otherwise (so a local extremum of the
//! data is a flat extremum of the curve); each end tangent is its
//! adjacent secant; then, per interval, a flat interval zeroes both its
//! tangents and any interval whose `(m_k / d_k, m_{k+1} / d_k)` lies
//! outside the radius-3 circle has both scaled back onto it. The result
//! is **monotone within every interval**: each segment stays between its
//! own two knots' `y`s and never overshoots, so no curve ever leaves
//! `[0, 1]` by construction. [`ToneCurve::evaluate`] still clamps its
//! result to `[0, 1]`, but only as a rounding safety net.
//!
//! **This deliberately differs from Photoshop**, whose Curves dialog
//! interpolates with a natural cubic spline that overshoots between
//! close points (and then clips). Which one Aurora should match is a
//! design-owner decision (PRD FR-027 *Ownership*), flagged, not settled
//! here: a monotone spline was chosen because it cannot invent tone
//! reversals the user did not place.
//!
//! A second, smaller design-owner question inside that choice: each
//! interior tangent starts as the **unweighted** mean of its two
//! neighbouring secants (Fritsch and Carlson's original 1980 form),
//! not the interval-width-weighted three-point estimate (e.g. Fritsch
//! and Butland's weighted harmonic mean) that bends less towards a much
//! shorter neighbouring interval. Both keep the monotonicity guarantee;
//! they differ only in shape between unevenly spaced points.
//!
//! [`ToneCurve::build_lut`] allocates exactly the length it is asked for,
//! up to [`MAX_LUT_LEN`] samples (4 MiB of `f32`); a longer request is
//! refused with [`ToneCurveError::LutTooLong`] rather than attempted, so
//! a caller-supplied `usize::MAX` is an error and never a capacity-
//! overflow panic or an out-of-memory abort.

/// The fewest points a curve may have: its two endpoints.
pub const MIN_POINTS: usize = 2;

/// The longest table [`ToneCurve::build_lut`] builds: `1 << 20` samples
/// (4 MiB of `f32`) — 64x the 16-bit input range, far past any real LUT,
/// and small enough that the allocation cannot plausibly fail.
pub const MAX_LUT_LEN: usize = 1 << 20;

/// The most points a curve may have (Photoshop's own Curves limit).
pub const MAX_POINTS: usize = 16;

/// The smallest horizontal gap between two consecutive points: `1/256`,
/// exact in `f32`. Deliberately **smaller** than the `1/255` step an
/// 8-bit input level (and the curve editor's fine key step) moves by, so
/// two points one 8-bit level apart are always legal.
pub const MIN_POINT_SEPARATION: f32 = 1.0 / 256.0;

/// One control point: input level `x`, output level `y`, both in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

impl CurvePoint {
    /// A point at `(x, y)` — unvalidated; [`ToneCurve::new`] validates.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Why a [`ToneCurve`] construction or edit was refused. The curve is
/// unchanged whenever one of these is returned.
///
/// `#[non_exhaustive]`, like [`crate::CoreError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToneCurveError {
    /// Fewer than [`MIN_POINTS`] points.
    #[error("a tone curve needs at least {MIN_POINTS} points")]
    TooFewPoints,
    /// More than [`MAX_POINTS`] points (or an insert past that limit).
    #[error("a tone curve holds at most {MAX_POINTS} points")]
    TooManyPoints,
    /// A coordinate was NaN or infinite.
    #[error("tone curve coordinates must be finite")]
    NonFinite,
    /// A `y` outside `[0, 1]`, or an inserted `x` not strictly inside
    /// `(0, 1)`.
    #[error("tone curve coordinates must lie in [0, 1]")]
    OutOfRange,
    /// The first point's `x` is not `0.0` or the last one's is not `1.0`.
    #[error("a tone curve's endpoints must sit at x = 0 and x = 1")]
    EndpointX,
    /// Two consecutive points are closer than [`MIN_POINT_SEPARATION`]
    /// horizontally (or out of order).
    #[error("tone curve points must be at least 1/256 apart horizontally")]
    TooClose,
    /// An attempt to remove the first or last point.
    #[error("a tone curve's endpoints cannot be removed")]
    EndpointNotRemovable,
    /// A point index past the end of the curve.
    #[error("no tone curve point at that index")]
    IndexOutOfRange,
    /// A [`ToneCurve::build_lut`] length above [`MAX_LUT_LEN`].
    #[error("a tone curve lookup table holds at most {MAX_LUT_LEN} samples")]
    LutTooLong,
}

/// A validated tone curve — see this module's own doc comment for the
/// invariants every value satisfies and the interpolation it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurve {
    points: Vec<CurvePoint>,
    /// One Fritsch–Carlson tangent per point, recomputed after every
    /// edit (a pure function of `points`, so derived `PartialEq` is
    /// exactly "same points").
    tangents: Vec<f64>,
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self::identity()
    }
}

impl ToneCurve {
    /// The identity curve: `(0, 0)` to `(1, 1)`, `evaluate(x) == x`.
    #[must_use]
    pub fn identity() -> Self {
        Self::from_valid(vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)])
    }

    /// A curve through `points`, which must already satisfy every
    /// invariant in this module's own doc comment — **no silent
    /// sorting**, clamping or de-duplication.
    ///
    /// # Errors
    ///
    /// The first violated invariant, checked in this order:
    /// [`ToneCurveError::TooFewPoints`], [`ToneCurveError::TooManyPoints`],
    /// [`ToneCurveError::NonFinite`], [`ToneCurveError::OutOfRange`],
    /// [`ToneCurveError::EndpointX`], [`ToneCurveError::TooClose`].
    pub fn new(points: &[CurvePoint]) -> Result<Self, ToneCurveError> {
        validate(points)?;
        Ok(Self::from_valid(points.to_vec()))
    }

    fn from_valid(points: Vec<CurvePoint>) -> Self {
        let tangents = tangents(&points);
        Self { points, tangents }
    }

    /// The control points, in increasing `x`.
    #[must_use]
    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    /// The curve's output at input `x`. `x` is clamped to `[0, 1]`, and a
    /// NaN `x` is treated as `0.0`. Exact (`==`) at every control point.
    /// The result is clamped to `[0, 1]` — a rounding safety net only;
    /// the spline itself never leaves its own knots' range.
    #[must_use]
    // The Hermite formula's own names (`x`, `t`, `h`, `k`, `a`, `b`, `y`).
    #[allow(clippy::many_single_char_names)]
    pub fn evaluate(&self, x: f32) -> f32 {
        let x = if x.is_nan() {
            0.0
        } else {
            f64::from(x.clamp(0.0, 1.0))
        };
        // The interval `[k, k + 1]` holding `x`: `x == 1.0` (and any
        // `x` at the last knot) falls in the last interval, at `t == 1`.
        let after = self.points.partition_point(|p| f64::from(p.x) <= x);
        let last = self.points.len().saturating_sub(2);
        let k = after.saturating_sub(1).min(last);
        let (Some(a), Some(b), Some(&ma), Some(&mb)) = (
            self.points.get(k),
            self.points.get(k + 1),
            self.tangents.get(k),
            self.tangents.get(k + 1),
        ) else {
            // Unreachable: every curve holds at least two points.
            return 0.0;
        };
        let (x0, y0) = (f64::from(a.x), f64::from(a.y));
        let (x1, y1) = (f64::from(b.x), f64::from(b.y));
        let h = x1 - x0;
        let t = (x - x0) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        let y = h00 * y0 + h10 * h * ma + h01 * y1 + h11 * h * mb;
        (y as f32).clamp(0.0, 1.0)
    }

    /// `n` evenly spaced samples: sample `i` is [`Self::evaluate`] at
    /// `i / (n - 1)` (the division done in `f64`), so the first sample
    /// is exactly `evaluate(0.0)` and the last exactly `evaluate(1.0)`.
    /// `n == 0` is empty and `n == 1` is `[evaluate(0.0)]`. Allocates
    /// exactly `n` floats.
    ///
    /// # Errors
    ///
    /// [`ToneCurveError::LutTooLong`] if `n` exceeds [`MAX_LUT_LEN`];
    /// nothing is allocated.
    pub fn build_lut(&self, n: usize) -> Result<Vec<f32>, ToneCurveError> {
        if n > MAX_LUT_LEN {
            return Err(ToneCurveError::LutTooLong);
        }
        Ok(match n {
            0 => Vec::new(),
            1 => vec![self.evaluate(0.0)],
            _ => {
                let last = (n - 1) as f64;
                (0..n)
                    .map(|i| self.evaluate((i as f64 / last) as f32))
                    .collect()
            }
        })
    }

    /// Adds a point at input `x`, **on the current curve** (`y` is
    /// [`Self::evaluate`]`(x)` before the insert), and returns its index.
    ///
    /// # Errors
    ///
    /// [`ToneCurveError::NonFinite`] for a non-finite `x`,
    /// [`ToneCurveError::TooManyPoints`] at [`MAX_POINTS`],
    /// [`ToneCurveError::OutOfRange`] unless `0 < x < 1`, and
    /// [`ToneCurveError::TooClose`] if `x` is within
    /// [`MIN_POINT_SEPARATION`] of either neighbour.
    pub fn add_point(&mut self, x: f32) -> Result<usize, ToneCurveError> {
        if !x.is_finite() {
            return Err(ToneCurveError::NonFinite);
        }
        if self.points.len() >= MAX_POINTS {
            return Err(ToneCurveError::TooManyPoints);
        }
        if x <= 0.0 || x >= 1.0 {
            return Err(ToneCurveError::OutOfRange);
        }
        let index = self.points.partition_point(|p| p.x < x);
        let mut next = self.points.clone();
        next.insert(index, CurvePoint::new(x, self.evaluate(x)));
        self.replace(next)?;
        Ok(index)
    }

    /// Removes the interior point at `index` and returns it.
    ///
    /// # Errors
    ///
    /// [`ToneCurveError::IndexOutOfRange`] past the end, or
    /// [`ToneCurveError::EndpointNotRemovable`] for the first or last
    /// point (so a curve never drops below [`MIN_POINTS`]).
    pub fn remove_point(&mut self, index: usize) -> Result<CurvePoint, ToneCurveError> {
        let Some(&removed) = self.points.get(index) else {
            return Err(ToneCurveError::IndexOutOfRange);
        };
        if index == 0 || index + 1 == self.points.len() {
            return Err(ToneCurveError::EndpointNotRemovable);
        }
        let mut next = self.points.clone();
        next.remove(index);
        self.replace(next)?;
        Ok(removed)
    }

    /// Moves the point at `index` towards `(x, y)`: `y` is clamped to
    /// `[0, 1]`; an endpoint **ignores `x`** (it stays at `0` or `1`); an
    /// interior point's `x` is clamped to stay [`MIN_POINT_SEPARATION`]
    /// from both neighbours. Returns whether the point actually moved.
    ///
    /// # Errors
    ///
    /// [`ToneCurveError::IndexOutOfRange`] past the end, or
    /// [`ToneCurveError::NonFinite`] if `x` or `y` is not finite (checked
    /// for an endpoint too, even though its `x` is then ignored).
    pub fn move_point_to(&mut self, index: usize, x: f32, y: f32) -> Result<bool, ToneCurveError> {
        let Some(&current) = self.points.get(index) else {
            return Err(ToneCurveError::IndexOutOfRange);
        };
        if !(x.is_finite() && y.is_finite()) {
            return Err(ToneCurveError::NonFinite);
        }
        let last = self.points.len() - 1;
        let x = if index == 0 || index == last {
            current.x
        } else {
            let (Some(before), Some(after)) =
                (self.points.get(index - 1), self.points.get(index + 1))
            else {
                return Err(ToneCurveError::IndexOutOfRange);
            };
            let (lo, hi) = interior_x_range(before.x, after.x);
            x.clamp(lo, hi.max(lo))
        };
        let moved = CurvePoint::new(x, y.clamp(0.0, 1.0));
        if moved == current {
            return Ok(false);
        }
        let mut next = self.points.clone();
        if let Some(slot) = next.get_mut(index) {
            *slot = moved;
        }
        self.replace(next)?;
        Ok(true)
    }

    /// The point nearest `(x, y)` within the axis-aligned ellipse of radii
    /// `(rx, ry)` around it (distance `((px - x) / rx)² + ((py - y) / ry)²
    /// <= 1`), the lower index on a tie. `None` if no point is inside, or
    /// if any argument is non-finite or a radius is not positive.
    #[must_use]
    pub fn nearest_point_within(&self, x: f32, y: f32, rx: f32, ry: f32) -> Option<usize> {
        let finite = [x, y, rx, ry].iter().all(|v| v.is_finite());
        if !finite || rx <= 0.0 || ry <= 0.0 {
            return None;
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in self.points.iter().enumerate() {
            let dx = (f64::from(p.x) - f64::from(x)) / f64::from(rx);
            let dy = (f64::from(p.y) - f64::from(y)) / f64::from(ry);
            let d = dx * dx + dy * dy;
            if d <= 1.0 && best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Validates `next` and, only if it passes, makes it this curve.
    fn replace(&mut self, next: Vec<CurvePoint>) -> Result<(), ToneCurveError> {
        validate(&next)?;
        *self = Self::from_valid(next);
        Ok(())
    }
}

/// The closed range an interior point between `before` and `after` may
/// occupy: at least [`MIN_POINT_SEPARATION`] from each, as measured by
/// [`validate`]'s own `f32` subtraction. `x + SEP` is rounded in `f32`,
/// so each bound is nudged one ulp inward if the rounding landed it too
/// close.
fn interior_x_range(before: f32, after: f32) -> (f32, f32) {
    let mut lo = before + MIN_POINT_SEPARATION;
    if lo - before < MIN_POINT_SEPARATION {
        lo = lo.next_up();
    }
    let mut hi = after - MIN_POINT_SEPARATION;
    if after - hi < MIN_POINT_SEPARATION {
        hi = hi.next_down();
    }
    (lo, hi)
}

/// Every invariant in this module's own doc comment, in the order
/// [`ToneCurve::new`] documents.
// Exact endpoint comparisons are the invariant itself (`x == 0.0` and
// `x == 1.0` exactly, both exact in `f32`).
#[allow(clippy::float_cmp)]
fn validate(points: &[CurvePoint]) -> Result<(), ToneCurveError> {
    if points.len() < MIN_POINTS {
        return Err(ToneCurveError::TooFewPoints);
    }
    if points.len() > MAX_POINTS {
        return Err(ToneCurveError::TooManyPoints);
    }
    if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return Err(ToneCurveError::NonFinite);
    }
    if points.iter().any(|p| !(0.0..=1.0).contains(&p.y)) {
        return Err(ToneCurveError::OutOfRange);
    }
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Err(ToneCurveError::TooFewPoints);
    };
    if first.x != 0.0 || last.x != 1.0 {
        return Err(ToneCurveError::EndpointX);
    }
    if points
        .windows(2)
        .any(|w| matches!(w, [a, b] if b.x - a.x < MIN_POINT_SEPARATION))
    {
        return Err(ToneCurveError::TooClose);
    }
    Ok(())
}

/// Fritsch–Carlson tangents for already-validated `points` — see
/// "Interpolation" in this module's own doc comment.
// Fritsch–Carlson's own names (`n`, `m`, `d`, `a`, `b`, `s`).
#[allow(clippy::many_single_char_names)]
fn tangents(points: &[CurvePoint]) -> Vec<f64> {
    let secants: Vec<f64> = points
        .windows(2)
        .map(|w| match w {
            [a, b] => (f64::from(b.y) - f64::from(a.y)) / (f64::from(b.x) - f64::from(a.x)),
            _ => 0.0,
        })
        .collect();
    let n = points.len();
    let mut m = vec![0.0_f64; n];
    for (k, slot) in m.iter_mut().enumerate() {
        let left = k.checked_sub(1).and_then(|i| secants.get(i)).copied();
        let right = secants.get(k).copied();
        *slot = match (left, right) {
            (Some(l), Some(r)) => {
                if (l > 0.0 && r > 0.0) || (l < 0.0 && r < 0.0) {
                    f64::midpoint(l, r)
                } else {
                    0.0
                }
            }
            (None, Some(r)) => r,
            (Some(l), None) => l,
            (None, None) => 0.0,
        };
    }
    for (k, &d) in secants.iter().enumerate() {
        // Redundant with the sign rule above — both tangents of a flat
        // interval are already `0` (an interior one has a zero secant
        // beside it; an end one *is* that secant) — and measured as such:
        // deleting this branch survives every test, the skipped `0 / 0`
        // below being `NaN`, which fails `> 9.0`. Kept so no `NaN` is
        // ever computed and the step reads as the published algorithm.
        if d == 0.0 {
            if let Some(slot) = m.get_mut(k) {
                *slot = 0.0;
            }
            if let Some(slot) = m.get_mut(k + 1) {
                *slot = 0.0;
            }
            continue;
        }
        let (Some(&mk), Some(&mk1)) = (m.get(k), m.get(k + 1)) else {
            continue;
        };
        let a = mk / d;
        let b = mk1 / d;
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            if let Some(slot) = m.get_mut(k) {
                *slot = tau * a * d;
            }
            if let Some(slot) = m.get_mut(k + 1) {
                *slot = tau * b * d;
            }
        }
    }
    m
}

#[cfg(test)]
// Exact float equality is the claim under test (knots, LUT ends, clamps);
// the formulas' own single-letter names; a pseudo-random fraction in
// `[0, 1)` cast to an index.
#[allow(
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::cast_sign_loss
)]
mod tests {
    use super::{
        CurvePoint, MAX_LUT_LEN, MAX_POINTS, MIN_POINT_SEPARATION, ToneCurve, ToneCurveError,
        tangents,
    };

    fn p(x: f32, y: f32) -> CurvePoint {
        CurvePoint::new(x, y)
    }

    fn curve(points: &[CurvePoint]) -> ToneCurve {
        match ToneCurve::new(points) {
            Ok(curve) => curve,
            Err(err) => unreachable!("{err:?} for {points:?}"),
        }
    }

    fn lut(c: &ToneCurve, n: usize) -> Vec<f32> {
        match c.build_lut(n) {
            Ok(lut) => lut,
            Err(err) => unreachable!("{err:?} for n = {n}"),
        }
    }

    /// Asserts every sample in every interval stays within that
    /// interval's own knot range (`±1e-6`) and inside `[0, 1]` — the
    /// tighter, per-interval bound, not just the final clamp.
    fn assert_bounded_per_interval(c: &ToneCurve, samples_per_interval: u32) {
        for w in c.points().windows(2) {
            let [a, b] = w else { unreachable!() };
            let (lo, hi) = (a.y.min(b.y) - 1e-6, a.y.max(b.y) + 1e-6);
            for s in 0..=samples_per_interval {
                let t = f64::from(s) / f64::from(samples_per_interval);
                let x = (f64::from(a.x) + t * (f64::from(b.x) - f64::from(a.x))) as f32;
                let y = c.evaluate(x);
                // A sample rounding onto the neighbouring interval's
                // knot is still that knot's value — exact there.
                assert!(
                    (lo..=hi).contains(&y) && (0.0..=1.0).contains(&y),
                    "{x} -> {y} outside [{lo}, {hi}] for {:?}",
                    c.points()
                );
            }
        }
    }

    /// The unclamped spline value inside interval `k`, for tests that
    /// must see past `evaluate`'s safety-net clamp.
    fn raw(c: &ToneCurve, x: f64) -> f64 {
        let pts = c.points();
        let k = pts
            .partition_point(|q| f64::from(q.x) <= x)
            .saturating_sub(1)
            .min(pts.len() - 2);
        let m = tangents(pts);
        let (Some(a), Some(b), Some(&ma), Some(&mb)) =
            (pts.get(k), pts.get(k + 1), m.get(k), m.get(k + 1))
        else {
            unreachable!()
        };
        let h = f64::from(b.x) - f64::from(a.x);
        let t = (x - f64::from(a.x)) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * f64::from(a.y)
            + (t3 - 2.0 * t2 + t) * h * ma
            + (-2.0 * t3 + 3.0 * t2) * f64::from(b.y)
            + (t3 - t2) * h * mb
    }

    fn zigzag(n: usize, step: f32) -> Vec<CurvePoint> {
        (0..n)
            .map(|i| {
                let x = if i + 1 == n { 1.0 } else { i as f32 * step };
                p(x, if i % 2 == 0 { 0.0 } else { 1.0 })
            })
            .collect()
    }

    #[test]
    fn identity_is_the_identity_within_1e_6() {
        let c = ToneCurve::identity();
        for i in 0..=1000_u32 {
            let x = i as f32 / 1000.0;
            assert!((c.evaluate(x) - x).abs() <= 1e-6, "{x}");
        }
        let lut = lut(&c, 256);
        assert_eq!(lut.len(), 256);
        for (i, y) in lut.iter().enumerate() {
            assert!((y - i as f32 / 255.0).abs() <= 1e-6, "{i}");
        }
        assert_eq!(ToneCurve::default(), c);
        assert_eq!(c.points(), [p(0.0, 0.0), p(1.0, 1.0)]);
    }

    #[test]
    fn every_knot_is_reproduced_exactly() {
        let sets: [&[CurvePoint]; 4] = [
            &[p(0.0, 0.25), p(1.0, 0.75)],
            &[p(0.0, 0.0), p(0.25, 0.6), p(0.5, 0.1), p(1.0, 1.0)],
            &[
                p(0.0, 1.0),
                p(0.3, 0.3),
                p(0.31, 0.9),
                p(0.7, 0.9),
                p(1.0, 0.0),
            ],
            &[
                p(0.0, 0.1),
                p(0.2, 0.2),
                p(0.4, 0.4),
                p(0.6, 0.65),
                p(1.0, 0.7),
            ],
        ];
        for set in sets {
            let c = curve(set);
            for q in set {
                assert_eq!(c.evaluate(q.x), q.y, "{set:?} at {q:?}");
            }
        }
    }

    #[test]
    fn monotone_data_gives_a_monotone_curve() {
        let increasing = curve(&[
            p(0.0, 0.0),
            p(0.1, 0.5),
            p(0.2, 0.55),
            p(0.5, 0.6),
            p(0.52, 0.95),
            p(1.0, 1.0),
        ]);
        let decreasing = curve(&[p(0.0, 1.0), p(0.05, 0.2), p(0.6, 0.19), p(1.0, 0.0)]);
        let samples = lut(&increasing, 4097);
        assert!(
            samples
                .windows(2)
                .all(|w| matches!(w, [a, b] if *b >= *a - 1e-7))
        );
        let samples = lut(&decreasing, 4097);
        assert!(
            samples
                .windows(2)
                .all(|w| matches!(w, [a, b] if *b <= *a + 1e-7))
        );
    }

    #[test]
    fn adversarial_curves_stay_within_each_intervals_own_knots() {
        let sep = MIN_POINT_SEPARATION;
        let sets = vec![
            zigzag(MAX_POINTS, sep),
            zigzag(MAX_POINTS, 1.0 / 16.0),
            zigzag(3, 0.5),
            // Plateaus between steep rises.
            vec![
                p(0.0, 0.0),
                p(0.2, 0.5),
                p(0.4, 0.5),
                p(0.41, 1.0),
                p(0.8, 1.0),
                p(1.0, 1.0),
            ],
            // A one-point spike at minimum separation either side.
            vec![
                p(0.0, 0.0),
                p(0.5 - sep, 0.0),
                p(0.5, 1.0),
                p(0.5 + sep, 0.0),
                p(1.0, 0.0),
            ],
            // Steep then shallow — the case the radius-3 rescale exists for.
            vec![p(0.0, 0.0), p(sep, 0.9), p(1.0 - sep, 0.95), p(1.0, 1.0)],
            vec![p(0.0, 0.0), p(0.9, 0.01), p(0.9 + sep, 0.99), p(1.0, 1.0)],
        ];
        for set in &sets {
            assert_bounded_per_interval(&curve(set), 256);
        }
    }

    /// The per-interval bound must hold for the *unclamped* spline too,
    /// or `evaluate`'s final clamp would be hiding an overshoot.
    #[test]
    fn the_unclamped_spline_never_overshoots_its_knots() {
        let sets = [
            vec![
                p(0.0, 0.0),
                p(0.9, 0.01),
                p(0.9 + MIN_POINT_SEPARATION, 0.99),
                p(1.0, 1.0),
            ],
            vec![
                p(0.0, 0.2),
                p(0.1, 0.3),
                p(0.11, 0.9),
                p(0.5, 0.95),
                p(1.0, 0.96),
            ],
            zigzag(MAX_POINTS, MIN_POINT_SEPARATION),
        ];
        for set in &sets {
            let c = curve(set);
            for w in set.windows(2) {
                let [a, b] = w else { unreachable!() };
                let (lo, hi) = (f64::from(a.y.min(b.y)), f64::from(a.y.max(b.y)));
                for s in 0..=512_u32 {
                    let x =
                        f64::from(a.x) + f64::from(s) / 512.0 * (f64::from(b.x) - f64::from(a.x));
                    let y = raw(&c, x);
                    assert!(y >= lo - 1e-9 && y <= hi + 1e-9, "{x} -> {y}");
                }
            }
        }
    }

    #[test]
    fn a_local_extremum_is_flat_and_not_overshot() {
        // Signs differ either side of the middle point: its tangent is 0,
        // so the curve peaks exactly at the knot.
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.8), p(1.0, 0.0)]);
        for i in 0..=1000_u32 {
            let x = i as f32 / 1000.0;
            assert!(c.evaluate(x) <= 0.8, "{x}");
        }
        assert_eq!(tangents(c.points()).get(1), Some(&0.0));
    }

    #[test]
    fn a_plateau_is_exactly_flat() {
        let c = curve(&[p(0.0, 0.0), p(0.3, 0.4), p(0.7, 0.4), p(1.0, 1.0)]);
        for i in 0..=400_u32 {
            let x = 0.3 + 0.4 * i as f32 / 400.0;
            assert!((c.evaluate(x) - 0.4).abs() <= 1e-7, "{x}");
        }
    }

    #[test]
    fn construction_reports_each_error() {
        use ToneCurveError as E;
        let many: Vec<CurvePoint> = (0..=MAX_POINTS)
            .map(|i| p(i as f32 / MAX_POINTS as f32, 0.5))
            .collect();
        let cases: [(&[CurvePoint], E); 11] = [
            (&[], E::TooFewPoints),
            (&[p(0.0, 0.0)], E::TooFewPoints),
            (&many, E::TooManyPoints),
            (&[p(0.0, f32::NAN), p(1.0, 1.0)], E::NonFinite),
            (&[p(0.0, 0.0), p(f32::INFINITY, 1.0)], E::NonFinite),
            (&[p(0.0, -0.01), p(1.0, 1.0)], E::OutOfRange),
            (&[p(0.0, 0.0), p(1.0, 1.01)], E::OutOfRange),
            (&[p(0.01, 0.0), p(1.0, 1.0)], E::EndpointX),
            (&[p(0.0, 0.0), p(0.99, 1.0)], E::EndpointX),
            (
                &[p(0.0, 0.0), p(0.6, 0.5), p(0.4, 0.5), p(1.0, 1.0)],
                E::TooClose,
            ),
            (
                &[p(0.0, 0.0), p(0.5, 0.5), p(0.5, 0.6), p(1.0, 1.0)],
                E::TooClose,
            ),
        ];
        for (points, want) in cases {
            assert_eq!(ToneCurve::new(points), Err(want), "{points:?}");
        }
        let full: Vec<CurvePoint> = (0..MAX_POINTS)
            .map(|i| p(i as f32 / (MAX_POINTS - 1) as f32, 0.5))
            .collect();
        assert!(ToneCurve::new(&full).is_ok());
    }

    #[test]
    fn separation_of_exactly_min_is_accepted_and_one_ulp_less_is_not() {
        let sep = MIN_POINT_SEPARATION;
        assert!(ToneCurve::new(&[p(0.0, 0.0), p(sep, 0.5), p(1.0, 1.0)]).is_ok());
        assert!(
            ToneCurve::new(&[p(0.0, 0.0), p(0.5, 0.5), p(0.5 + sep, 0.5), p(1.0, 1.0)]).is_ok()
        );
        assert_eq!(
            ToneCurve::new(&[p(0.0, 0.0), p(sep.next_down(), 0.5), p(1.0, 1.0)]),
            Err(ToneCurveError::TooClose)
        );
        assert_eq!(
            ToneCurve::new(&[p(0.0, 0.0), p((1.0 - sep).next_up(), 0.5), p(1.0, 1.0)]),
            Err(ToneCurveError::TooClose)
        );
    }

    #[test]
    fn add_point_lands_on_the_curve_and_keeps_order() {
        let mut c = curve(&[p(0.0, 0.1), p(0.5, 0.7), p(1.0, 0.9)]);
        let before = c.evaluate(0.25);
        assert_eq!(c.add_point(0.25), Ok(1));
        assert_eq!(c.points().get(1), Some(&p(0.25, before)));
        assert_eq!(c.add_point(0.75), Ok(3));
        assert!(
            c.points()
                .windows(2)
                .all(|w| matches!(w, [a, b] if a.x < b.x))
        );
        assert_eq!(c.add_point(0.0), Err(ToneCurveError::OutOfRange));
        assert_eq!(c.add_point(1.0), Err(ToneCurveError::OutOfRange));
        assert_eq!(c.add_point(f32::NAN), Err(ToneCurveError::NonFinite));
        let snapshot = c.clone();
        assert_eq!(
            c.add_point((0.5 + MIN_POINT_SEPARATION).next_down()),
            Err(ToneCurveError::TooClose)
        );
        assert_eq!(c.add_point(0.5), Err(ToneCurveError::TooClose));
        assert_eq!(c, snapshot);
        assert_eq!(c.add_point(0.5 + MIN_POINT_SEPARATION).map(|_| ()), Ok(()));
    }

    #[test]
    fn a_seventeenth_point_is_refused() {
        let mut c = ToneCurve::identity();
        for i in 1..(MAX_POINTS - 1) {
            assert!(
                c.add_point(i as f32 / (MAX_POINTS - 1) as f32).is_ok(),
                "{i}"
            );
        }
        assert_eq!(c.points().len(), MAX_POINTS);
        let snapshot = c.clone();
        assert_eq!(c.add_point(0.03), Err(ToneCurveError::TooManyPoints));
        assert_eq!(c, snapshot);
    }

    #[test]
    fn remove_point_refuses_endpoints_and_bad_indices() {
        let mut c = curve(&[p(0.0, 0.0), p(0.4, 0.6), p(1.0, 1.0)]);
        assert_eq!(c.remove_point(0), Err(ToneCurveError::EndpointNotRemovable));
        assert_eq!(c.remove_point(2), Err(ToneCurveError::EndpointNotRemovable));
        assert_eq!(c.remove_point(3), Err(ToneCurveError::IndexOutOfRange));
        assert_eq!(c.remove_point(1), Ok(p(0.4, 0.6)));
        assert_eq!(c, ToneCurve::identity());
        assert_eq!(c.remove_point(1), Err(ToneCurveError::EndpointNotRemovable));
    }

    #[test]
    fn move_point_to_clamps_and_endpoints_keep_their_x() {
        let sep = MIN_POINT_SEPARATION;
        let mut c = curve(&[p(0.0, 0.0), p(0.4, 0.6), p(0.6, 0.7), p(1.0, 1.0)]);
        assert_eq!(c.move_point_to(0, 0.3, 0.2), Ok(true));
        assert_eq!(c.points().first(), Some(&p(0.0, 0.2)));
        assert_eq!(c.move_point_to(3, -5.0, 2.0), Ok(false));
        assert_eq!(c.move_point_to(3, 0.5, 0.25), Ok(true));
        assert_eq!(c.points().last(), Some(&p(1.0, 0.25)));
        assert_eq!(c.move_point_to(1, 0.9, -1.0), Ok(true));
        let moved = c.points().get(1).copied();
        assert_eq!(moved.map(|q| q.y), Some(0.0));
        assert!(moved.is_some_and(|q| 0.6 - q.x >= sep && q.x < 0.6));
        assert_eq!(c.move_point_to(1, -1.0, 0.0), Ok(true));
        assert!(
            c.points()
                .get(1)
                .is_some_and(|q| q.x - 0.0 >= sep && q.x <= sep.next_up())
        );
        assert_eq!(
            c.move_point_to(9, 0.5, 0.5),
            Err(ToneCurveError::IndexOutOfRange)
        );
        let snapshot = c.clone();
        assert_eq!(
            c.move_point_to(0, f32::NAN, 0.5),
            Err(ToneCurveError::NonFinite)
        );
        assert_eq!(
            c.move_point_to(1, 0.5, f32::INFINITY),
            Err(ToneCurveError::NonFinite)
        );
        assert_eq!(c, snapshot);
    }

    #[test]
    fn nearest_point_within_uses_the_ellipse_and_breaks_ties_low() {
        // Binary-exact positions, so the tie at 0.5625 is a real tie.
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.5), p(0.625, 0.5), p(1.0, 1.0)]);
        assert_eq!(c.nearest_point_within(0.52, 0.5, 0.05, 0.05), Some(1));
        assert_eq!(c.nearest_point_within(0.6, 0.5, 0.05, 0.05), Some(2));
        assert_eq!(c.nearest_point_within(0.5625, 0.5, 0.0625, 0.0625), Some(1));
        assert_eq!(c.nearest_point_within(0.5625, 0.5, 0.05, 0.5), None);
        assert_eq!(c.nearest_point_within(0.5, 0.6, 0.01, 0.2), Some(1));
        assert_eq!(c.nearest_point_within(0.5, 0.6, 0.2, 0.01), None);
        assert_eq!(c.nearest_point_within(f32::NAN, 0.5, 0.1, 0.1), None);
        assert_eq!(c.nearest_point_within(0.5, 0.5, 0.0, 0.1), None);
        assert_eq!(c.nearest_point_within(0.5, 0.5, 0.1, f32::INFINITY), None);
    }

    #[test]
    fn lut_lengths_and_ends_are_exact() {
        let c = curve(&[p(0.0, 0.2), p(0.3, 0.9), p(1.0, 0.4)]);
        assert!(lut(&c, 0).is_empty());
        assert_eq!(lut(&c, 1), vec![0.2]);
        for n in [2_usize, 3, 256, 1000, 4096, MAX_LUT_LEN] {
            let lut = lut(&c, n);
            assert_eq!(lut.len(), n);
            assert_eq!(lut.first(), Some(&0.2));
            assert_eq!(lut.last(), Some(&0.4));
        }
    }

    /// Past [`MAX_LUT_LEN`] the request is refused before any
    /// allocation — `usize::MAX` included, which would otherwise be a
    /// capacity-overflow panic in a workspace that denies panics.
    #[test]
    fn a_lut_longer_than_the_cap_is_refused_not_attempted() {
        let c = curve(&[p(0.0, 0.2), p(0.3, 0.9), p(1.0, 0.4)]);
        assert_eq!(lut(&c, MAX_LUT_LEN).len(), MAX_LUT_LEN);
        for n in [MAX_LUT_LEN + 1, usize::MAX / 4, usize::MAX] {
            assert_eq!(c.build_lut(n), Err(ToneCurveError::LutTooLong), "n = {n}");
        }
    }

    #[test]
    fn evaluate_clamps_x_and_treats_nan_as_zero() {
        let c = curve(&[p(0.0, 0.2), p(1.0, 0.8)]);
        assert_eq!(c.evaluate(f32::NAN), 0.2);
        assert_eq!(c.evaluate(-3.0), 0.2);
        assert_eq!(c.evaluate(f32::NEG_INFINITY), 0.2);
        assert_eq!(c.evaluate(7.0), 0.8);
        assert_eq!(c.evaluate(f32::INFINITY), 0.8);
    }

    /// A deterministic pseudo-random edit sequence never breaks an
    /// invariant: every intermediate curve re-validates.
    #[test]
    fn random_edit_sequences_keep_every_invariant() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1_u64 << 24) as f32
        };
        let mut c = ToneCurve::identity();
        for _ in 0..20_000 {
            let op = next();
            let n = c.points().len();
            let index = ((next() * n as f32) as usize).min(n - 1);
            if op < 0.4 {
                let _ = c.add_point(next());
            } else if op < 0.6 {
                let _ = c.remove_point(index);
            } else {
                // A finite move of an existing point never fails: it clamps.
                assert!(
                    c.move_point_to(index, next() * 1.2 - 0.1, next() * 1.2 - 0.1)
                        .is_ok()
                );
            }
            assert_eq!(ToneCurve::new(c.points()).as_ref(), Ok(&c));
        }
    }
}
