//! A tone curve: the pure, widget-free model behind a Curves adjustment
//! and the curve-editor widget (`aurora_widgets`' `curve_editor`).
//!
//! A [`ToneCurve`] maps an input level `x` in `[0, 1]` to an output level
//! `y` in `[0, 1]` through `2..=19` control points ([`MIN_POINTS`],
//! [`MAX_POINTS`] — Photoshop's own Curves limit), interpolated by a
//! **natural cubic spline** whose output is then **clamped to `[0, 1]`**,
//! which is what Photoshop's Curves does (0.157.0). It lives here, in
//! `aurora-core`, so the widget that edits it and the adjustment that
//! applies it (`aurora-filters`, `aurora-doc`, `aurora-io`) share one
//! definition without either depending on the other
//! (`scripts/layering.json`).
//!
//! # Invariants
//!
//! Every [`ToneCurve`] that exists satisfies all of these — the
//! validating constructor [`ToneCurve::new`] refuses anything else, and
//! every editing method builds its candidate and re-validates it, keeping
//! the old curve on failure:
//!
//! - between [`MIN_POINTS`] and [`MAX_POINTS`] points, inclusive;
//! - every coordinate finite, every `x` and every `y` in `[0, 1]`;
//! - the first and last points may sit anywhere in `[0, 1]` (0.158.0,
//!   *movable endpoints*): below the first point's `x` the output is held
//!   at its `y`, above the last one's at that `y` — Photoshop's own rule,
//!   which psd-tools reproduces by clamping the input to `[x0, xn]`;
//! - consecutive `x`s at least [`MIN_POINT_SEPARATION`] apart (so strictly
//!   increasing — the constructor never sorts silently).
//!
//! # Interpolation: Photoshop's, as far as it has been verified
//!
//! **Decided by the design owner (Cahya, PRD FR-027 *Ownership*,
//! 2026-10-09): Aurora matches Photoshop**, so an imported PSD Curves
//! layer looks identical. The model, all of it behind
//! `natural_second_derivatives` and `ToneCurve::spline`, so a later
//! correction touches one place:
//!
//! - a **natural cubic spline** through the points, computed in `f64`:
//!   `C2`-continuous, second derivative exactly `0` at both ends, the
//!   interior second derivatives `M_i` solved from the standard
//!   tridiagonal system (strictly diagonally dominant, so the Thomas
//!   algorithm needs no pivoting and no denominator can be zero);
//! - evaluated per interval as `(1 - t) y_k + t y_{k+1} + h^2 / 6 *
//!   (((1 - t)^3 - (1 - t)) M_k + (t^3 - t) M_{k+1})`, a form that is
//!   exactly `y_k` at `t == 0` and exactly `y_{k+1}` at `t == 1`, and
//!   exactly `x` for the identity curve;
//! - the result **clamped to `[0, 1]`**. Unlike the monotone
//!   Fritsch–Carlson spline this replaced (0.155.0–0.156.0), a natural
//!   spline overshoots between close or uneven points — it can invent a
//!   tone reversal the user did not place — and the clamp is then
//!   load-bearing, not a rounding safety net. That is Photoshop's
//!   behaviour, chosen deliberately.
//!
//! **Evidence (0.157.0):** psd-tools 1.17.4's Curves compositor
//! (`psd_tools/composite/adjustments.py`, `apply_curves`) uses
//! `scipy.interpolate.CubicSpline(..., bc_type="natural")`, clipped to
//! `[0, 1]`, with input clamped to the first/last point. Independently of
//! psd-tools' own code, Photoshop's **own merged composite** stored in
//! the corpus fixture `adjustments/curves_rgb.psd` was compared, region by
//! region, against four candidate models applied to that file's four
//! Curves layers: the natural spline was within one 8-bit level on every
//! level the fixture exercises cleanly (the reference test below), where
//! Fritsch–Carlson missed by up to 10 levels, a zero-end-slope cubic by
//! up to 21 and linear interpolation by up to 17. What it does **not**
//! establish: behaviour at 16 bits per channel (Photoshop may well
//! evaluate a 256-entry table and interpolate it; Aurora evaluates the
//! spline directly), and Photoshop's own source.
//!
//! **Movable endpoints (0.158.0).** Photoshop lets the first and last
//! points move horizontally (a curve may start at input 26) and holds the
//! output flat beyond them; adding flat points at the ends instead is
//! **not** equivalent for a natural spline. Until 0.157.0 this type pinned
//! them to `x == 0` and `x == 1`; the PSD `curv` import relaxed that: the
//! spline is built through the points as given and its **input** is
//! clamped to `[x0, xn]` before it is evaluated, exactly psd-tools'
//! `np.clip(t, x_min, x_max)`. Verified against Photoshop's own merged
//! composite of `adjustments/curves_rgb.psd`, whose `Curves 2` composite
//! curve starts at input 26 (the `aurora-app` corpus differential). The
//! editor still moves an endpoint only vertically
//! ([`ToneCurve::move_point_to`]); an imported interior endpoint keeps its
//! `x`. `.aur` compatibility: a build older than 0.158.0 re-validates every
//! decoded curve with its own `x == 0` / `x == 1` rule, so it **refuses**
//! a file holding a movable-endpoint curve (a typed decode error, the
//! whole file), rather than opening it with a different curve.
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

/// The most points a curve may have: 19, Photoshop's own Curves limit
/// (16 until 0.157.0 — a `.aur` file holding a 17-to-19-point curve is
/// refused by an older build).
pub const MAX_POINTS: usize = 19;

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
    /// A `y` outside `[0, 1]`, or an inserted `x` not strictly between
    /// the first and last points' `x`.
    #[error("tone curve coordinates must lie in [0, 1]")]
    OutOfRange,
    /// A point's `x` lies outside `[0, 1]`. Until 0.157.0 this also meant
    /// "an endpoint not at exactly `0` / `1`"; endpoints may move since
    /// 0.158.0, so only the unit range is enforced.
    #[error("a tone curve's points must sit between x = 0 and x = 1")]
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
    /// One natural-spline second derivative per point (`0.0` at both
    /// ends), recomputed after every edit (a pure function of `points`,
    /// so derived `PartialEq` is exactly "same points").
    second_derivatives: Vec<f64>,
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
        let second_derivatives = natural_second_derivatives(&points);
        Self {
            points,
            second_derivatives,
        }
    }

    /// The control points, in increasing `x`.
    #[must_use]
    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    /// The closed input range the points span, `(x0, xn)` — `(0, 1)`
    /// unless an endpoint was moved. Below `x0` and above `xn` the output
    /// is flat ([`Self::evaluate`]).
    #[must_use]
    pub fn input_range(&self) -> (f32, f32) {
        match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => (a.x, b.x),
            _ => (0.0, 1.0),
        }
    }

    /// Whether this is exactly the two-point identity `(0, 0)`–`(1, 1)`,
    /// which [`Self::evaluate`] maps to itself for every `x` in `[0, 1]`.
    /// A curve with extra points on the diagonal is *not* reported as
    /// identity: its spline is the identity in exact arithmetic, but this
    /// check is about the stored shape, not a numeric property.
    #[must_use]
    #[allow(clippy::float_cmp)]
    pub fn is_identity(&self) -> bool {
        matches!(
            self.points.as_slice(),
            [a, b] if a.x == 0.0 && a.y == 0.0 && b.x == 1.0 && b.y == 1.0
        )
    }

    /// The curve's output at input `x`: the spline clamped to `[0, 1]`
    /// (see this module's own doc comment). `x` is clamped to `[0, 1]`
    /// and then to [`Self::input_range`] (flat beyond a moved endpoint),
    /// and a NaN `x` is treated as `0.0`. Exact (`==`) at every control
    /// point, and exactly `x` for the identity curve.
    #[must_use]
    pub fn evaluate(&self, x: f32) -> f32 {
        (self.spline(x) as f32).clamp(0.0, 1.0)
    }

    /// The spline itself, **before** the output clamp — it may leave
    /// `[0, 1]` between points. For a lookup table that interpolates
    /// between samples and clamps afterwards (`aurora_filters::curves`),
    /// which keeps the table's error bound free of the clamp's kink.
    /// Input handling as [`Self::evaluate`]; always finite.
    #[must_use]
    pub fn evaluate_unclamped(&self, x: f32) -> f32 {
        self.spline(x) as f32
    }

    /// The spline's own end-interval cubics **continued past a moved
    /// endpoint** (0.158.0): input clamped to `[0, 1]` only (NaN reads as
    /// `0.0`), not to [`Self::input_range`], and no output clamp. Inside
    /// the range it equals [`Self::evaluate_unclamped`]. For a lookup table
    /// that clamps its *input* to the range itself before interpolating
    /// (`aurora_filters::curves`), so the flat extension's kink at a moved
    /// endpoint never falls inside a table interval. Always finite: the
    /// continuation is at most one unit away from an interval at least
    /// [`MIN_POINT_SEPARATION`] wide.
    #[must_use]
    pub fn evaluate_extrapolated(&self, x: f32) -> f32 {
        self.spline_at(Self::unit_input(x)) as f32
    }

    fn unit_input(x: f32) -> f64 {
        if x.is_nan() {
            0.0
        } else {
            f64::from(x.clamp(0.0, 1.0))
        }
    }

    /// The natural cubic spline at `x`, in `f64` — the one place the
    /// interpolation formula lives (with `natural_second_derivatives`).
    // The spline formula's own names (`x`, `t`, `u`, `h`, `k`, `a`, `b`).
    #[allow(clippy::many_single_char_names)]
    fn spline(&self, x: f32) -> f64 {
        let (lo, hi) = self.input_range();
        // Flat beyond a moved endpoint: psd-tools' `np.clip(t, x0, xn)`.
        self.spline_at(Self::unit_input(x).clamp(f64::from(lo), f64::from(hi)))
    }

    /// The spline formula at an already-clamped `x` (beyond the knots it
    /// continues the end interval's cubic).
    #[allow(clippy::many_single_char_names)]
    fn spline_at(&self, x: f64) -> f64 {
        // The interval `[k, k + 1]` holding `x`: `x == 1.0` (and any
        // `x` at the last knot) falls in the last interval, at `t == 1`.
        let after = self.points.partition_point(|p| f64::from(p.x) <= x);
        let last = self.points.len().saturating_sub(2);
        let k = after.saturating_sub(1).min(last);
        let (Some(a), Some(b), Some(&ma), Some(&mb)) = (
            self.points.get(k),
            self.points.get(k + 1),
            self.second_derivatives.get(k),
            self.second_derivatives.get(k + 1),
        ) else {
            // Unreachable: every curve holds at least two points.
            return 0.0;
        };
        let (x0, y0) = (f64::from(a.x), f64::from(a.y));
        let (x1, y1) = (f64::from(b.x), f64::from(b.y));
        let h = x1 - x0;
        let t = (x - x0) / h;
        let u = 1.0 - t;
        u * y0 + t * y1 + h * h / 6.0 * ((u * u * u - u) * ma + (t * t * t - t) * mb)
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
    /// [`ToneCurveError::OutOfRange`] unless `x0 < x < xn`
    /// ([`Self::input_range`]; `0 < x < 1` for unmoved endpoints), and
    /// [`ToneCurveError::TooClose`] if `x` is within
    /// [`MIN_POINT_SEPARATION`] of either neighbour.
    pub fn add_point(&mut self, x: f32) -> Result<usize, ToneCurveError> {
        if !x.is_finite() {
            return Err(ToneCurveError::NonFinite);
        }
        if self.points.len() >= MAX_POINTS {
            return Err(ToneCurveError::TooManyPoints);
        }
        let (lo, hi) = self.input_range();
        if x <= lo || x >= hi {
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
    /// `[0, 1]`; an endpoint **ignores `x`** (it keeps its own `x`, `0` or
    /// `1` unless an imported curve moved it); an
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
    // Endpoints may move (0.158.0); every `x` must still lie in `[0, 1]`
    // (with the ordering below, checking the ends is checking them all).
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Err(ToneCurveError::TooFewPoints);
    };
    if first.x < 0.0 || last.x > 1.0 {
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

/// The natural cubic spline's second derivatives `M_i` at each of
/// already-validated `points`: `M_0 = M_{n-1} = 0` (the natural end
/// condition) and, for each interior point,
/// `h_{i-1} M_{i-1} + 2 (h_{i-1} + h_i) M_i + h_i M_{i+1} = 6 (d_i - d_{i-1})`
/// with `h` the interval widths and `d` the secants. Solved by the Thomas
/// algorithm in `f64`: the system is strictly diagonally dominant (each
/// diagonal exceeds its off-diagonals by `h_{i-1} + h_i >= 2/256`), so
/// every elimination denominator is at least `h_{i-1} + 2 h_i > 0` and
/// nothing divides by zero. Two points give all zeros — a straight line.
///
/// The same dominance bounds every `|M_i|` — not each `M_i` by its *own*
/// neighbours' widths, since a large `M` can carry to a knot beside a wide
/// interval, but all of them by the global maximum —
/// `max_j |6 (d_j - d_{j-1})| / (h_{j-1} + h_j) <= max_j 6 / (h_{j-1} h_j)
/// <= 6 * 256^2`, which is what `aurora_filters::curves`' table-accuracy
/// bound rests on.
fn natural_second_derivatives(points: &[CurvePoint]) -> Vec<f64> {
    let n = points.len();
    let mut m = vec![0.0_f64; n];
    if n < 3 {
        return m;
    }
    let widths: Vec<f64> = points
        .windows(2)
        .map(|w| match w {
            [a, b] => f64::from(b.x) - f64::from(a.x),
            _ => 1.0,
        })
        .collect();
    let secants: Vec<f64> = points
        .windows(2)
        .zip(&widths)
        .map(|(w, &h)| match w {
            [a, b] => (f64::from(b.y) - f64::from(a.y)) / h,
            _ => 0.0,
        })
        .collect();
    // Forward elimination over the interior rows `1..n - 1`; row 0 is the
    // known `M_0 = 0`, so its eliminated coefficients are zero too.
    let mut upper = vec![0.0_f64; n];
    let mut rhs = vec![0.0_f64; n];
    for i in 1..n - 1 {
        let (Some(&hl), Some(&hr), Some(&dl), Some(&dr)) = (
            widths.get(i - 1),
            widths.get(i),
            secants.get(i - 1),
            secants.get(i),
        ) else {
            return vec![0.0; n];
        };
        let (prev_upper, prev_rhs) = (
            upper.get(i - 1).copied().unwrap_or(0.0),
            rhs.get(i - 1).copied().unwrap_or(0.0),
        );
        let denom = 2.0 * (hl + hr) - hl * prev_upper;
        if let Some(slot) = upper.get_mut(i) {
            *slot = hr / denom;
        }
        if let Some(slot) = rhs.get_mut(i) {
            *slot = (6.0 * (dr - dl) - hl * prev_rhs) / denom;
        }
    }
    // Back substitution; `M_{n-1} = 0` closes it.
    for i in (1..n - 1).rev() {
        let next = m.get(i + 1).copied().unwrap_or(0.0);
        let value =
            rhs.get(i).copied().unwrap_or(0.0) - upper.get(i).copied().unwrap_or(0.0) * next;
        if let Some(slot) = m.get_mut(i) {
            *slot = value;
        }
    }
    m
}

/// A curve is written as its point list, `(x, y)` pairs in order — the
/// second derivatives are derived data and never stored (0.155.0, for the Curves
/// adjustment layer's `.aur`/journal encoding).
impl serde::Serialize for ToneCurve {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.points.len()))?;
        for point in &self.points {
            seq.serialize_element(&(point.x, point.y))?;
        }
        seq.end()
    }
}

/// Decoding re-validates through [`ToneCurve::new`], so a hostile or
/// corrupt file can never produce a curve that breaks this module's
/// invariants. At most [`MAX_POINTS`] elements are ever read: a longer
/// sequence is refused at element `MAX_POINTS + 1` rather than buffered,
/// whatever length prefix it claims.
impl<'de> serde::Deserialize<'de> for ToneCurve {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PointsVisitor;

        impl<'de> serde::de::Visitor<'de> for PointsVisitor {
            type Value = ToneCurve;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(
                    f,
                    "a sequence of {MIN_POINTS}..={MAX_POINTS} (x, y) tone-curve points"
                )
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<ToneCurve, A::Error> {
                let mut points = Vec::with_capacity(MAX_POINTS);
                while let Some((x, y)) = seq.next_element::<(f32, f32)>()? {
                    if points.len() == MAX_POINTS {
                        return Err(serde::de::Error::custom(ToneCurveError::TooManyPoints));
                    }
                    points.push(CurvePoint::new(x, y));
                }
                ToneCurve::new(&points).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_seq(PointsVisitor)
    }
}

#[cfg(test)]
// Exact float equality is the claim under test (knots, LUT ends, clamps,
// the identity); the formulas' own single-letter names; a pseudo-random
// fraction in `[0, 1)` cast to an index.
#[allow(
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::cast_sign_loss
)]
mod tests {
    use super::{
        CurvePoint, MAX_LUT_LEN, MAX_POINTS, MIN_POINT_SEPARATION, ToneCurve, ToneCurveError,
        natural_second_derivatives,
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

    fn zigzag(n: usize, step: f32) -> Vec<CurvePoint> {
        (0..n)
            .map(|i| {
                let x = if i + 1 == n { 1.0 } else { i as f32 * step };
                p(x, if i % 2 == 0 { 0.0 } else { 1.0 })
            })
            .collect()
    }

    /// The identity is exactly the identity — bit for bit, not within a
    /// tolerance — over a `2^16`-step sweep of `[0, 1]` plus a 1000-step
    /// one: both its second derivatives are `0`, and the evaluation form
    /// reduces to `0 * (1 - x) + 1 * x`.
    #[test]
    fn identity_is_the_identity_bit_for_bit() {
        let c = ToneCurve::identity();
        assert_eq!(natural_second_derivatives(c.points()), vec![0.0, 0.0]);
        let fine = (0..=(1_u32 << 16)).map(|i| i as f32 / (1_u32 << 16) as f32);
        let sweep = (0..=1000_u32).map(|i| i as f32 / 1000.0);
        for x in fine.chain(sweep) {
            assert_eq!(c.evaluate(x).to_bits(), x.to_bits(), "{x}");
        }
        let lut = lut(&c, 256);
        assert_eq!(lut.len(), 256);
        for (i, y) in lut.iter().enumerate() {
            assert_eq!(*y, (i as f64 / 255.0) as f32, "{i}");
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

    /// Two points are a straight line: no curvature to solve for.
    #[test]
    fn two_points_give_a_straight_line() {
        let c = curve(&[p(0.0, 0.25), p(1.0, 0.75)]);
        assert_eq!(natural_second_derivatives(c.points()), vec![0.0, 0.0]);
        for i in 0..=1024_u32 {
            let x = i as f32 / 1024.0;
            let want = 0.25 + 0.5 * f64::from(x);
            assert!((f64::from(c.evaluate(x)) - want).abs() <= 1e-7, "{x}");
        }
        let falling = curve(&[p(0.0, 1.0), p(1.0, 0.0)]);
        assert_eq!(falling.evaluate(0.25), 0.75);
        assert_eq!(falling.evaluate(0.5), 0.5);
    }

    /// A hand-solved natural spline: through `(0, 0)`, `(0.5, 0.75)`,
    /// `(1, 1)` the one interior second derivative is
    /// `6 (0.5 - 1.5) / (2 * 1) = -3`, so the interval midpoints are
    /// `0.375 + 0.25 / 6 * 0.375 * 3 = 0.421875` and
    /// `0.875 + 0.046875 = 0.921875` — all exact in binary. A clamped
    /// (zero-end-slope) or Fritsch–Carlson spline gives other values.
    #[test]
    fn a_three_point_curve_matches_the_hand_solved_natural_spline() {
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.75), p(1.0, 1.0)]);
        assert_eq!(natural_second_derivatives(c.points()), vec![0.0, -3.0, 0.0]);
        assert_eq!(c.evaluate(0.25), 0.421_875);
        assert_eq!(c.evaluate(0.75), 0.921_875);
    }

    /// The natural end condition and `C2` continuity, measured on the
    /// curve itself rather than read off the solver: the second
    /// difference vanishes at both ends, and one-sided slopes agree across
    /// every interior knot (a sign or index slip in the solver breaks
    /// both).
    #[test]
    fn the_spline_is_natural_at_the_ends_and_smooth_across_knots() {
        let pts = [
            p(0.0, 0.1),
            p(0.2, 0.3),
            p(0.35, 0.32),
            p(0.6, 0.7),
            p(1.0, 0.8),
        ];
        let c = curve(&pts);
        let f = |x: f64| f64::from(c.evaluate_unclamped(x as f32));
        let e = 1.0 / 1024.0;
        for (x0, dir) in [(0.0, 1.0), (1.0, -1.0)] {
            let second = (f(x0) - 2.0 * f(x0 + dir * e) + f(x0 + dir * 2.0 * e)) / (e * e);
            assert!(second.abs() < 0.05, "end {x0}: f'' ~= {second}");
        }
        for q in pts.iter().skip(1).take(pts.len() - 2) {
            let x = f64::from(q.x);
            let left = (f(x) - f(x - e)) / e;
            let right = (f(x + e) - f(x)) / e;
            assert!((left - right).abs() < 0.02, "knot {x}: {left} vs {right}");
        }
    }

    /// Photoshop's output level for each input level, flattened
    /// `(input, output)` pairs — see
    /// `matches_photoshop_on_curves_from_its_own_merged_composite`.
    const PS_CURVES3_RED: &[u8] = &[
        0, 0, 2, 1, 3, 1, 4, 1, 6, 2, 7, 2, 8, 3, 9, 3, 11, 4, 12, 4, 13, 4, 14, 5, 15, 5, 16, 5,
        17, 6, 18, 6, 19, 7, 20, 7, 22, 8, 23, 8, 24, 8, 25, 9, 27, 10, 28, 10, 32, 12, 35, 13, 39,
        15, 44, 18, 45, 19, 50, 22, 51, 22, 52, 23, 53, 24, 55, 25, 56, 26, 57, 27, 58, 27, 59, 28,
        60, 29, 62, 31, 63, 31, 64, 32, 65, 33, 66, 34, 67, 35, 68, 36, 69, 37, 70, 38, 71, 39, 72,
        40, 73, 41, 74, 42, 75, 43, 76, 44, 77, 45, 78, 46, 79, 47, 80, 48, 81, 50, 82, 51, 83, 52,
        84, 53, 85, 55, 86, 56, 87, 57, 88, 58, 89, 60, 90, 61, 91, 63, 92, 64, 93, 65, 94, 67, 95,
        68, 96, 70, 97, 72, 98, 73, 99, 75, 100, 76, 101, 78, 102, 80, 103, 81, 104, 83, 105, 85,
        106, 87, 107, 88, 108, 90, 109, 92, 110, 94, 111, 96, 112, 97, 113, 99, 114, 101, 115, 103,
        116, 105, 117, 107, 118, 109, 119, 110, 120, 112, 121, 114, 122, 116, 123, 118, 124, 120,
        125, 122, 126, 124, 127, 126, 128, 128, 129, 129, 130, 131, 131, 133, 132, 135, 133, 137,
        134, 139, 135, 141, 136, 143, 137, 145, 138, 146, 139, 148, 140, 150, 141, 152, 142, 154,
        143, 155, 144, 157, 145, 159, 146, 161, 147, 162, 148, 164, 149, 166, 150, 167, 151, 169,
        152, 171, 153, 172, 154, 174, 155, 175, 156, 177, 157, 178, 158, 180, 159, 181, 160, 183,
        161, 184, 162, 185, 163, 187, 164, 188, 165, 189, 166, 191, 167, 192, 168, 193, 169, 195,
        170, 196, 171, 197, 172, 198, 173, 199, 174, 201, 175, 202, 176, 203, 177, 204, 178, 205,
        179, 206, 180, 207, 181, 208, 182, 209, 183, 210, 184, 211, 185, 212, 186, 213, 187, 214,
        188, 215, 189, 216, 190, 217, 191, 218, 192, 219, 193, 219, 194, 220, 195, 221, 196, 222,
        197, 223, 198, 224, 199, 224, 200, 225, 201, 226, 202, 227, 203, 227, 204, 228, 205, 229,
        206, 230, 207, 230, 208, 231, 209, 232, 210, 232, 211, 233, 212, 234, 213, 234, 214, 235,
        215, 235, 216, 236, 217, 237, 218, 237, 219, 238, 220, 238, 221, 239, 222, 239, 223, 240,
        224, 241, 225, 241, 226, 242, 227, 242, 228, 243, 229, 243, 230, 244, 231, 244, 232, 245,
        233, 245, 234, 246, 235, 246, 236, 247, 237, 247, 238, 247, 239, 248, 240, 248, 241, 249,
        242, 249, 243, 250, 244, 250, 245, 251, 246, 251, 247, 252, 248, 252, 250, 253, 252, 254,
        254, 255, 255, 255,
    ];

    /// Photoshop's output level for each input level, flattened
    /// `(input, output)` pairs — see
    /// `matches_photoshop_on_curves_from_its_own_merged_composite`.
    const PS_CURVES1_GREEN_THEN_RGB: &[u8] = &[
        0, 0, 2, 1, 5, 3, 6, 3, 7, 4, 9, 5, 12, 7, 14, 8, 16, 9, 19, 12, 20, 12, 22, 14, 24, 15,
        25, 16, 26, 17, 27, 18, 28, 19, 29, 20, 30, 21, 31, 21, 32, 22, 33, 23, 34, 25, 35, 26, 36,
        27, 37, 28, 38, 30, 39, 31, 40, 32, 41, 33, 42, 34, 43, 37, 44, 38, 46, 41, 47, 43, 48, 44,
        49, 46, 50, 47, 51, 49, 52, 50, 53, 52, 54, 54, 55, 57, 56, 59, 57, 60, 58, 62, 59, 64, 60,
        66, 61, 68, 62, 69, 63, 71, 64, 73, 65, 75, 66, 77, 67, 79, 68, 80, 69, 82, 70, 84, 71, 86,
        72, 88, 73, 90, 74, 92, 75, 94, 76, 96, 77, 98, 78, 99, 79, 101, 80, 103, 81, 105, 82, 107,
        83, 109, 84, 111, 85, 113, 86, 114, 87, 116, 88, 118, 89, 120, 90, 122, 91, 123, 92, 125,
        93, 127, 94, 128, 95, 130, 96, 132, 97, 134, 98, 134, 99, 135, 100, 137, 101, 138, 102,
        140, 103, 142, 104, 143, 105, 145, 106, 146, 107, 148, 108, 149, 109, 151, 110, 152, 111,
        154, 112, 155, 113, 155, 114, 157, 115, 158, 116, 160, 117, 161, 118, 162, 119, 164, 120,
        165, 121, 166, 122, 168, 123, 169, 124, 169, 125, 170, 126, 172, 127, 173, 128, 174, 129,
        176, 130, 177, 131, 178, 132, 179, 133, 180, 134, 182, 135, 182, 136, 183, 137, 184, 138,
        185, 139, 186, 140, 187, 141, 189, 142, 190, 143, 191, 144, 192, 145, 192, 146, 193, 147,
        194, 148, 195, 149, 196, 150, 197, 151, 198, 152, 199, 153, 200, 154, 201, 155, 201, 156,
        202, 157, 203, 158, 204, 159, 205, 160, 206, 161, 207, 162, 208, 163, 208, 164, 209, 165,
        209, 166, 210, 167, 211, 168, 212, 169, 213, 170, 213, 171, 214, 172, 215, 173, 216, 174,
        217, 175, 217, 176, 218, 177, 218, 178, 219, 179, 220, 180, 220, 181, 221, 182, 222, 183,
        223, 184, 223, 185, 224, 186, 225, 187, 225, 188, 226, 189, 227, 190, 227, 191, 228, 192,
        228, 193, 228, 194, 229, 195, 230, 196, 230, 197, 231, 198, 231, 199, 232, 200, 233, 201,
        233, 202, 234, 203, 234, 205, 235, 207, 236, 208, 237, 210, 238, 211, 238, 212, 239, 214,
        240, 217, 241, 218, 241, 219, 242, 221, 243, 222, 243, 224, 244, 226, 245, 227, 245, 229,
        246, 230, 246, 231, 247, 234, 248, 236, 249, 239, 250, 243, 251, 246, 252, 248, 253, 250,
        253, 251, 254, 252, 254, 253, 254, 255, 255,
    ];

    /// Photoshop's output level for each input level, flattened
    /// `(input, output)` pairs — see
    /// `matches_photoshop_on_curves_from_its_own_merged_composite`.
    const PS_CURVES1_BLUE_THEN_RGB: &[u8] = &[
        0, 7, 1, 8, 3, 10, 7, 13, 8, 14, 9, 14, 10, 15, 12, 17, 16, 21, 18, 23, 19, 24, 20, 26, 21,
        27, 22, 28, 26, 34, 27, 36, 29, 38, 30, 40, 31, 43, 32, 44, 33, 46, 34, 47, 35, 49, 36, 50,
        37, 52, 38, 55, 39, 57, 40, 59, 41, 60, 42, 62, 43, 64, 44, 66, 46, 69, 47, 71, 48, 73, 49,
        77, 50, 79, 51, 80, 52, 82, 53, 84, 54, 86, 55, 88, 56, 90, 57, 92, 58, 94, 59, 96, 60, 98,
        61, 98, 62, 99, 63, 101, 64, 103, 65, 105, 66, 107, 67, 109, 68, 111, 69, 113, 70, 114, 71,
        114, 72, 116, 73, 118, 74, 120, 75, 122, 76, 122, 77, 123, 78, 125, 79, 127, 80, 127, 81,
        128, 82, 130, 83, 132, 84, 132, 85, 134, 86, 135, 87, 135, 88, 137, 89, 138, 90, 138, 91,
        140, 92, 142, 93, 142, 94, 143, 95, 145, 96, 145, 97, 146, 98, 148, 99, 148, 100, 149, 101,
        151, 102, 151, 103, 152, 104, 152, 105, 154, 106, 155, 107, 155, 108, 157, 109, 158, 110,
        158, 111, 160, 112, 160, 113, 161, 114, 162, 115, 162, 116, 164, 117, 165, 118, 165, 119,
        166, 120, 168, 121, 168, 122, 169, 123, 170, 124, 170, 125, 172, 126, 173, 127, 173, 128,
        174, 129, 176, 130, 177, 131, 177, 132, 178, 133, 179, 134, 180, 135, 180, 136, 182, 137,
        183, 138, 184, 139, 184, 140, 185, 141, 186, 142, 187, 143, 189, 144, 189, 145, 190, 146,
        191, 147, 192, 148, 193, 149, 193, 150, 194, 151, 195, 152, 196, 153, 197, 154, 198, 155,
        199, 156, 199, 157, 200, 158, 201, 159, 202, 160, 203, 161, 204, 162, 205, 163, 206, 164,
        206, 165, 207, 166, 208, 167, 208, 168, 209, 169, 210, 170, 211, 171, 212, 172, 213, 173,
        213, 174, 214, 175, 215, 176, 216, 177, 216, 178, 217, 179, 217, 180, 218, 181, 219, 182,
        220, 183, 220, 184, 221, 185, 222, 186, 223, 187, 223, 188, 224, 189, 225, 190, 225, 191,
        226, 192, 227, 193, 227, 194, 228, 195, 228, 196, 229, 197, 230, 198, 230, 199, 231, 200,
        231, 201, 232, 202, 233, 203, 233, 204, 234, 205, 234, 206, 235, 207, 235, 208, 236, 209,
        237, 210, 237, 211, 238, 212, 238, 213, 239, 215, 240, 216, 240, 218, 241, 220, 242, 221,
        242, 222, 243, 226, 245, 227, 245, 228, 245, 229, 246, 233, 247, 235, 248, 237, 249, 238,
        249, 239, 250, 240, 250, 242, 251, 243, 251, 246, 252, 247, 252, 248, 253, 249, 253, 252,
        254, 255, 255,
    ];

    /// **Photoshop's own rendering as the oracle.** Each list is flattened
    /// `(input level, Photoshop's output level)` pairs read from the
    /// merged composite Photoshop stored in the corpus fixture
    /// `corpora/psd/reference/psd-tools-fixtures/adjustments/curves_rgb.psd`
    /// (read with psd-tools 1.17.4, 0.157.0): for one Curves layer, every
    /// pixel where that layer's mask is fully on and every other Curves
    /// layer's fully off, the layers below composited by psd-tools with
    /// all Curves layers hidden, grouped by input level and kept only
    /// where at least 90% of at least three pixels agree. The point lists
    /// are the layer's own stored points. Red of "Curves 3" is a channel
    /// curve alone; green and blue of "Curves 1" go through their channel
    /// curve and then the RGB curve, rounded to an 8-bit value between the
    /// two — that rounding models the 8-bit *file* only; Aurora's own
    /// `aurora_filters::curves` path (same order) has no 8-bit step. This
    /// checks `ToneCurve` alone, not `CurvesLut` end to end. Reproduce the
    /// pairs with `scripts/oracles/psd_curves_oracle.py`. Every level
    /// must be within one 8-bit level, and at least 98% exact;
    /// Fritsch–Carlson misses these by up to 10 levels.
    #[test]
    fn matches_photoshop_on_curves_from_its_own_merged_composite() {
        let pts = |raw: &[(u8, u8)]| -> ToneCurve {
            let points: Vec<CurvePoint> = raw
                .iter()
                .map(|&(x, y)| p(f32::from(x) / 255.0, f32::from(y) / 255.0))
                .collect();
            curve(&points)
        };
        let level = |c: &ToneCurve, v: f32| (c.evaluate(v / 255.0) * 255.0).round();
        let red3 = pts(&[(0, 0), (96, 70), (151, 169), (255, 255)]);
        let rgb1 = pts(&[(0, 0), (49, 37), (95, 118), (197, 232), (255, 255)]);
        let green1 = pts(&[(0, 0), (65, 72), (213, 211), (255, 255)]);
        let blue1 = pts(&[(0, 15), (73, 95), (122, 128), (255, 255)]);
        let cases: [(&str, &[u8], Option<&ToneCurve>, &ToneCurve); 3] = [
            ("Curves 3 red", PS_CURVES3_RED, None, &red3),
            (
                "Curves 1 green, then RGB",
                PS_CURVES1_GREEN_THEN_RGB,
                Some(&rgb1),
                &green1,
            ),
            (
                "Curves 1 blue, then RGB",
                PS_CURVES1_BLUE_THEN_RGB,
                Some(&rgb1),
                &blue1,
            ),
        ];
        for (name, pairs, composite, channel) in cases {
            let n = pairs.len() / 2;
            assert!(n >= 200, "{name}: {n} levels");
            let mut exact = 0_usize;
            for pair in pairs.chunks_exact(2) {
                let [input, want] = pair else { unreachable!() };
                let mut out = level(channel, f32::from(*input));
                if let Some(rgb) = composite {
                    out = level(rgb, out);
                }
                let want = f32::from(*want);
                assert!(
                    (out - want).abs() <= 1.0,
                    "{name}: {input} -> {out}, Photoshop {want}"
                );
                exact += usize::from(out == want);
            }
            assert!(exact * 100 >= n * 98, "{name}: only {exact} of {n} exact");
        }
    }

    /// A natural spline overshoots monotone data between uneven points —
    /// it is *not* monotone, by design (Photoshop's behaviour) — and
    /// `evaluate` clamps what leaves `[0, 1]`. Replaces 0.155.0's
    /// `monotone_data_gives_a_monotone_curve` and
    /// `the_unclamped_spline_never_overshoots_its_knots`, whose claims
    /// were properties of the Fritsch–Carlson spline this replaced.
    #[test]
    fn an_overshoot_leaves_the_unit_range_and_is_clamped() {
        let c = curve(&[p(0.0, 0.0), p(0.45, 0.0), p(0.5, 1.0), p(1.0, 1.0)]);
        let (mut below, mut above) = (false, false);
        for i in 0..=4096_u32 {
            let x = i as f32 / 4096.0;
            let raw = c.evaluate_unclamped(x);
            assert!(raw.is_finite(), "{x}");
            below |= raw < -0.01;
            above |= raw > 1.01;
            assert_eq!(c.evaluate(x), raw.clamp(0.0, 1.0), "{x}");
        }
        assert!(below && above, "the spline must overshoot both ways here");
        assert_eq!(c.evaluate(0.6), 1.0);
        assert!(c.evaluate_unclamped(0.6) > 1.0);
    }

    /// A plateau between two rises is no longer exactly flat (it was under
    /// Fritsch–Carlson, 0.155.0's `a_plateau_is_exactly_flat`): the
    /// natural spline rings through it, as Photoshop's does.
    #[test]
    fn a_plateau_between_rises_rings_rather_than_staying_flat() {
        let c = curve(&[p(0.0, 0.0), p(0.3, 0.4), p(0.7, 0.4), p(1.0, 1.0)]);
        assert_eq!(c.evaluate(0.3), 0.4);
        assert_eq!(c.evaluate(0.7), 0.4);
        let dip = (1..400_u32)
            .map(|i| c.evaluate(0.3 + 0.4 * i as f32 / 400.0))
            .fold(f32::INFINITY, f32::min);
        assert!(
            dip < 0.39,
            "expected the spline to dip below the plateau: {dip}"
        );
    }

    /// The most adversarial curves the invariants allow — a full-range
    /// zigzag at minimum separation, a one-step spike, steep-then-flat —
    /// stay finite unclamped, inside `[0, 1]` clamped, and still exact at
    /// their knots. Replaces 0.155.0's
    /// `adversarial_curves_stay_within_each_intervals_own_knots`: a
    /// natural spline does not stay within an interval's own knots.
    #[test]
    fn adversarial_curves_stay_finite_and_inside_the_unit_range() {
        let sep = MIN_POINT_SEPARATION;
        let sets = vec![
            zigzag(MAX_POINTS, sep),
            zigzag(MAX_POINTS, 1.0 / 19.0),
            zigzag(3, 0.5),
            vec![
                p(0.0, 0.0),
                p(0.5 - sep, 0.0),
                p(0.5, 1.0),
                p(0.5 + sep, 0.0),
                p(1.0, 0.0),
            ],
            vec![p(0.0, 0.0), p(sep, 0.9), p(1.0 - sep, 0.95), p(1.0, 1.0)],
        ];
        for set in &sets {
            let c = curve(set);
            for i in 0..=8192_u32 {
                let x = i as f32 / 8192.0;
                assert!(c.evaluate_unclamped(x).is_finite(), "{x} for {set:?}");
                assert!((0.0..=1.0).contains(&c.evaluate(x)), "{x} for {set:?}");
            }
            for q in set {
                assert_eq!(c.evaluate(q.x), q.y, "knot {q:?}");
            }
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
            (&[p(-0.01, 0.0), p(1.0, 1.0)], E::EndpointX),
            (&[p(0.0, 0.0), p(1.01, 1.0)], E::EndpointX),
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

    /// Photoshop's limit, literally: 19 points are a curve and 20 are
    /// not, whether constructed or inserted (16 until 0.157.0; replaces
    /// `a_seventeenth_point_is_refused`).
    #[test]
    fn nineteen_points_are_accepted_and_a_twentieth_is_refused() {
        assert_eq!(MAX_POINTS, 19);
        let even = |n: usize| -> Vec<CurvePoint> {
            (0..n).map(|i| p(i as f32 / (n - 1) as f32, 0.5)).collect()
        };
        assert!(ToneCurve::new(&even(19)).is_ok());
        assert_eq!(
            ToneCurve::new(&even(20)),
            Err(ToneCurveError::TooManyPoints)
        );
        let mut c = ToneCurve::identity();
        for i in 1..18_u16 {
            assert!(c.add_point(f32::from(i) / 18.0).is_ok(), "{i}");
        }
        assert_eq!(c.points().len(), 19);
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

    /// The two movable-endpoint curves `adjustments/curves_rgb.psd`
    /// carries, as the file's `(input, output)` levels.
    fn psd_levels(levels: &[(u16, u16)]) -> ToneCurve {
        let points: Vec<CurvePoint> = levels
            .iter()
            .map(|&(i, o)| p(f32::from(i) / 255.0, f32::from(o) / 255.0))
            .collect();
        curve(&points)
    }

    /// 0.158.0: a curve whose endpoints are interior is valid, holds its
    /// output flat beyond them, and matches an independent natural-spline
    /// solve (numpy `linalg.solve` of the full tridiagonal system, input
    /// clamped to `[x0, xn]` as psd-tools does) to `1e-6`.
    #[test]
    fn movable_endpoints_hold_the_output_flat_beyond_them() {
        // `Curves 2`'s composite: starts at input 26.
        let start = psd_levels(&[(26, 3), (55, 84), (171, 170), (255, 255)]);
        assert_eq!(start.input_range(), (26.0 / 255.0, 1.0));
        assert!(!start.is_identity());
        // `Curves 4`'s red: moved at both ends, 49 and 195.
        let both = psd_levels(&[(49, 60), (94, 218), (102, 0), (185, 229), (195, 36)]);
        for (c, reference) in [
            (
                &start,
                [
                    (0.0, 0.011_764_7),
                    (0.1, 0.011_764_7),
                    (0.3, 0.482_915_1),
                    (0.5, 0.619_479_3),
                    (0.7, 0.683_161_5),
                    (0.75, 0.719_544_1),
                    (0.9, 0.875_275_9),
                    (1.0, 1.0),
                ],
            ),
            (
                &both,
                [
                    (0.0, 0.235_294_1),
                    (0.1, 0.235_294_1),
                    (0.3, 1.0),
                    (0.5, 0.0),
                    (0.7, 1.0),
                    (0.75, 0.456_463_8),
                    (0.9, 0.141_176_5),
                    (1.0, 0.141_176_5),
                ],
            ),
        ] {
            for (x, want) in reference {
                let got = c.evaluate(x);
                assert!((got - want).abs() <= 1e-6, "{x}: {got} vs {want}");
            }
            // Flat, bit for bit, on both sides of the moved ends.
            let (lo, hi) = c.input_range();
            let (Some(first), Some(last)) = (c.points().first(), c.points().last()) else {
                unreachable!("two points at least");
            };
            for i in 0..=64_u16 {
                let x = f32::from(i) / 64.0;
                if x <= lo {
                    assert_eq!(c.evaluate(x), first.y, "below x0 at {x}");
                }
                if x >= hi {
                    assert_eq!(c.evaluate(x), last.y, "above xn at {x}");
                }
            }
            for point in c.points() {
                assert_eq!(c.evaluate(point.x), point.y);
            }
        }
    }

    /// The extrapolated form agrees with the clamped one inside the range,
    /// continues past it (not flat), and stays finite everywhere.
    #[test]
    fn evaluate_extrapolated_continues_past_moved_endpoints() {
        let c = psd_levels(&[(26, 3), (55, 84), (171, 170), (255, 255)]);
        for i in 0..=255_u16 {
            let x = f32::from(i) / 255.0;
            let e = c.evaluate_extrapolated(x);
            assert!(e.is_finite());
            if x >= c.input_range().0 {
                assert_eq!(e, c.evaluate_unclamped(x), "{x}");
            }
        }
        // Below 26 the end interval's line continues downwards.
        assert!(c.evaluate_extrapolated(0.0) < c.evaluate_unclamped(0.0));
        assert_eq!(
            c.evaluate_extrapolated(f32::NAN),
            c.evaluate_extrapolated(0.0)
        );
        // The steepest valid end interval, continued a whole unit: finite.
        let steep = curve(&[p(0.5, 0.0), p(0.5 + MIN_POINT_SEPARATION, 1.0), p(0.6, 0.0)]);
        for x in [0.0, 1.0] {
            assert!(steep.evaluate_extrapolated(x).is_finite());
        }
    }

    /// Editing a moved-endpoint curve: inserts only between the ends, an
    /// endpoint drag keeps its own `x`, and the result still validates.
    #[test]
    fn editing_respects_moved_endpoints() {
        let mut c = psd_levels(&[(26, 3), (255, 255)]);
        assert_eq!(c.add_point(0.05), Err(ToneCurveError::OutOfRange));
        assert_eq!(c.add_point(0.5), Ok(1));
        assert_eq!(c.move_point_to(0, 0.0, 0.5), Ok(true));
        assert_eq!(c.points().first(), Some(&p(26.0 / 255.0, 0.5)));
        let mut end = psd_levels(&[(0, 0), (238, 255)]);
        assert_eq!(end.add_point(0.95), Err(ToneCurveError::OutOfRange));
        assert_eq!(
            end.remove_point(1),
            Err(ToneCurveError::EndpointNotRemovable)
        );
        assert_eq!(end.move_point_to(1, 1.0, 0.9), Ok(true));
        assert_eq!(end.points().last(), Some(&p(238.0 / 255.0, 0.9)));
    }

    /// A moved-endpoint curve survives the `.aur` encoding (its point
    /// list) unchanged, and a point outside `[0, 1]` is still refused on
    /// decode.
    #[test]
    fn a_moved_endpoint_curve_round_trips_and_out_of_range_x_is_refused() {
        let c = psd_levels(&[(49, 60), (94, 218), (102, 0), (185, 229), (195, 36)]);
        let bytes = match postcard::to_allocvec(&c) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(postcard::from_bytes::<ToneCurve>(&bytes).ok(), Some(c));
        let outside = match postcard::to_allocvec(&vec![(-0.5_f32, 0.0_f32), (1.0, 1.0)]) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(postcard::from_bytes::<ToneCurve>(&outside).is_err());
    }
}
