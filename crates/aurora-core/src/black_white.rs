//! The parameters of a Black & White adjustment (0.240.0): Photoshop's six
//! colour sliders and its Tint.
//!
//! It lives in `aurora-core`, beside [`crate::HueSaturationParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::BlackWhiteOp`)
//!
//! On **straight, encoded** sRGB, input clamped to `[0, 1]`. A pixel's
//! channels sorted `max >= mid >= min` split it, exactly, into a white part,
//! a primary part and a secondary part:
//!
//! ```text
//! pixel = min * white + (max - mid) * P + (mid - min) * S
//! ```
//!
//! where `P` is the pure primary (red, green or blue) of the largest
//! channel and `S` the pure secondary (yellow = red + green, cyan = green +
//! blue, magenta = red + blue) of the two largest channels. Each of the six
//! sliders is a percentage `w / 100` (Photoshop's: "the grey a pure colour
//! becomes", so Reds 40 turns pure red into 40 % grey). The grey is the
//! same split applied to the weights, white weighing 1:
//!
//! ```text
//! grey = clamp(min + (max - mid) * w(P) + (mid - min) * w(S), 0, 1)
//! ```
//!
//! This is piecewise-linear in hue between the six anchors (0, 60, 120, 180,
//! 240, 300 degrees), continuous (no hue is ever computed), and scaled by
//! chroma: a neutral pixel (`max = mid = min`) keeps its value, white stays
//! white and black stays black whatever the sliders say. A weight above 100
//! or below 0 can overshoot, which the clamp ends. **A choice, not compared
//! with Photoshop**, whose algorithm is unpublished; the anchors (pure
//! colours going to the slider values) follow its documentation.
//!
//! With **Tint** on, the output is the HSL colour with the tint hue
//! (degrees), the tint saturation (percent) and the grey as its lightness.
//! With it off the three channels are the grey.
//!
//! # Validation
//!
//! Every value finite; each weight in `-200..=300`; tint hue in `0..=360`;
//! tint saturation in `0..=100`. A value outside is an error, never
//! clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(weights, tint, hue, saturation)`, six
//! `f32`s in the order reds, yellows, greens, cyans, blues, magentas, one
//! `bool` and two `f32`s, 33 bytes.

/// The six slider names, in storage order.
pub const BLACK_WHITE_NAMES: [&str; 6] =
    ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];

/// Photoshop's default slider values, in storage order.
pub const BLACK_WHITE_DEFAULTS: [f32; 6] = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];

/// The smallest slider value.
pub const BLACK_WHITE_MIN: f32 = -200.0;
/// The largest slider value.
pub const BLACK_WHITE_MAX: f32 = 300.0;

/// A new layer's tint: a warm sepia (an Aurora choice).
pub const DEFAULT_TINT_HUE: f32 = 35.0;
/// A new layer's tint saturation.
pub const DEFAULT_TINT_SATURATION: f32 = 25.0;

/// Why a [`BlackWhiteParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackWhiteError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// A colour slider is outside `-200..=300`.
    Weight,
    /// The tint hue is outside `0..=360`.
    TintHue,
    /// The tint saturation is outside `0..=100`.
    TintSaturation,
}

impl std::fmt::Display for BlackWhiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Black & White value is not a finite number",
            Self::Weight => "a Black & White colour slider is outside -200..300",
            Self::TintHue => "the Black & White tint hue is outside 0..360",
            Self::TintSaturation => "the Black & White tint saturation is outside 0..100",
        })
    }
}

impl std::error::Error for BlackWhiteError {}

/// The wire form: see the module docs.
type Wire = ([f32; 6], bool, f32, f32);

/// A Black & White adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct BlackWhiteParams {
    weights: [f32; 6],
    tint: bool,
    tint_hue: f32,
    tint_saturation: f32,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for BlackWhiteParams {}

impl Default for BlackWhiteParams {
    /// Photoshop's defaults with Tint off.
    fn default() -> Self {
        Self {
            weights: BLACK_WHITE_DEFAULTS,
            tint: false,
            tint_hue: DEFAULT_TINT_HUE,
            tint_saturation: DEFAULT_TINT_SATURATION,
        }
    }
}

impl TryFrom<Wire> for BlackWhiteParams {
    type Error = BlackWhiteError;

    fn try_from(v: Wire) -> Result<Self, BlackWhiteError> {
        Self::new(v.0, v.1, v.2, v.3)
    }
}

impl From<BlackWhiteParams> for Wire {
    fn from(p: BlackWhiteParams) -> Self {
        (p.weights, p.tint, p.tint_hue, p.tint_saturation)
    }
}

impl BlackWhiteParams {
    /// A validated record.
    ///
    /// # Errors
    /// [`BlackWhiteError`] naming the first rule broken.
    pub fn new(
        weights: [f32; 6],
        tint: bool,
        tint_hue: f32,
        tint_saturation: f32,
    ) -> Result<Self, BlackWhiteError> {
        if weights.iter().any(|w| !w.is_finite())
            || !tint_hue.is_finite()
            || !tint_saturation.is_finite()
        {
            return Err(BlackWhiteError::NotFinite);
        }
        if weights
            .iter()
            .any(|w| !(BLACK_WHITE_MIN..=BLACK_WHITE_MAX).contains(w))
        {
            return Err(BlackWhiteError::Weight);
        }
        if !(0.0..=360.0).contains(&tint_hue) {
            return Err(BlackWhiteError::TintHue);
        }
        if !(0.0..=100.0).contains(&tint_saturation) {
            return Err(BlackWhiteError::TintSaturation);
        }
        Ok(Self {
            weights,
            tint,
            tint_hue,
            tint_saturation,
        })
    }

    /// The six slider values, reds first (see [`BLACK_WHITE_NAMES`]).
    #[must_use]
    pub const fn weights(&self) -> [f32; 6] {
        self.weights
    }

    /// Whether Tint is on.
    #[must_use]
    pub const fn tint(&self) -> bool {
        self.tint
    }

    /// The tint hue in degrees, `0..=360`.
    #[must_use]
    pub const fn tint_hue(&self) -> f32 {
        self.tint_hue
    }

    /// The tint saturation, `0..=100`.
    #[must_use]
    pub const fn tint_saturation(&self) -> f32 {
        self.tint_saturation
    }

    /// A copy with slider `index` (`0..6`) replaced.
    ///
    /// # Errors
    /// As [`Self::new`]; an `index` past the last slider is
    /// [`BlackWhiteError::Weight`].
    pub fn with_weight(&self, index: usize, value: f32) -> Result<Self, BlackWhiteError> {
        let mut weights = self.weights;
        *weights.get_mut(index).ok_or(BlackWhiteError::Weight)? = value;
        Self::new(weights, self.tint, self.tint_hue, self.tint_saturation)
    }

    /// A copy with Tint switched.
    #[must_use]
    pub const fn with_tint(&self, tint: bool) -> Self {
        Self { tint, ..*self }
    }

    /// A copy with the tint hue replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_tint_hue(&self, hue: f32) -> Result<Self, BlackWhiteError> {
        Self::new(self.weights, self.tint, hue, self.tint_saturation)
    }

    /// A copy with the tint saturation replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_tint_saturation(&self, saturation: f32) -> Result<Self, BlackWhiteError> {
        Self::new(self.weights, self.tint, self.tint_hue, saturation)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{BLACK_WHITE_DEFAULTS, BlackWhiteError as E, BlackWhiteParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_default_is_photoshops_six_values_with_tint_off() {
        let d = P::default();
        assert_eq!(d.weights(), [40.0, 60.0, 40.0, 60.0, 20.0, 80.0]);
        assert_eq!(d.weights(), BLACK_WHITE_DEFAULTS);
        assert!(!d.tint());
        assert_eq!((d.tint_hue(), d.tint_saturation()), (35.0, 25.0));
        assert!(P::new(d.weights(), false, 35.0, 25.0) == Ok(d));
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        let w = BLACK_WHITE_DEFAULTS;
        assert!(P::new([-200.0, 300.0, -200.0, 300.0, 0.0, 0.0], true, 0.0, 0.0).is_ok());
        assert!(P::new(w, true, 360.0, 100.0).is_ok());
        for i in 0..6 {
            let mut bad = w;
            if let Some(slot) = bad.get_mut(i) {
                *slot = 300.5;
            }
            assert_eq!(P::new(bad, false, 0.0, 0.0), Err(E::Weight), "{i}");
            if let Some(slot) = bad.get_mut(i) {
                *slot = -200.5;
            }
            assert_eq!(P::new(bad, false, 0.0, 0.0), Err(E::Weight), "{i}");
            for not_finite in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                if let Some(slot) = bad.get_mut(i) {
                    *slot = not_finite;
                }
                assert_eq!(P::new(bad, false, 0.0, 0.0), Err(E::NotFinite));
            }
        }
        assert_eq!(P::new(w, true, 360.5, 0.0), Err(E::TintHue));
        assert_eq!(P::new(w, true, -0.5, 0.0), Err(E::TintHue));
        assert_eq!(P::new(w, true, 0.0, 100.5), Err(E::TintSaturation));
        assert_eq!(P::new(w, true, 0.0, -0.5), Err(E::TintSaturation));
        for bad in [f32::NAN, f32::INFINITY] {
            assert_eq!(P::new(w, true, bad, 0.0), Err(E::NotFinite));
            assert_eq!(P::new(w, true, 0.0, bad), Err(E::NotFinite));
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(
            [-200.0, 12.5, 300.0, 0.0, 99.0, 1.0],
            true,
            211.0,
            64.0,
        ));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes.len(), 6 * 4 + 1 + 2 * 4, "positional, no framing");
        assert_eq!(bytes.get(24), Some(&1), "the tint flag follows the weights");
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        let wire = |w: [f32; 6], hue: f32, sat: f32| postcard::to_allocvec(&(w, true, hue, sat));
        for (w, hue, sat) in [
            ([301.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0, 0.0),
            ([0.0, 0.0, 0.0, 0.0, 0.0, -201.0], 0.0, 0.0),
            ([f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0, 0.0),
            ([0.0; 6], 361.0, 0.0),
            ([0.0; 6], 0.0, 101.0),
            ([0.0; 6], f32::INFINITY, 0.0),
        ] {
            let Ok(bad) = wire(w, hue, sat) else {
                unreachable!("encodes");
            };
            assert!(
                postcard::from_bytes::<P>(&bad).is_err(),
                "{w:?} {hue} {sat}"
            );
        }
        assert!(postcard::from_bytes::<P>(bytes.get(..32).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_keep_the_other_values_and_validate() {
        let p = ok(P::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0], true, 10.0, 20.0));
        for i in 0..6 {
            let q = ok(p.with_weight(i, 77.0));
            let mut want = p.weights();
            if let Some(slot) = want.get_mut(i) {
                *slot = 77.0;
            }
            assert_eq!(q.weights(), want);
            assert!(q.tint());
            assert_eq!((q.tint_hue(), q.tint_saturation()), (10.0, 20.0));
        }
        assert_eq!(p.with_weight(6, 0.0), Err(E::Weight));
        assert!(p.with_weight(0, 301.0).is_err());
        let q = p.with_tint(false);
        assert!(!q.tint());
        assert_eq!(q.weights(), p.weights());
        assert_eq!(ok(p.with_tint_hue(99.0)).tint_hue(), 99.0);
        assert_eq!(ok(p.with_tint_hue(99.0)).tint_saturation(), 20.0);
        assert_eq!(ok(p.with_tint_saturation(55.0)).tint_saturation(), 55.0);
        assert_eq!(ok(p.with_tint_saturation(55.0)).tint_hue(), 10.0);
        assert!(p.with_tint_hue(400.0).is_err());
        assert!(p.with_tint_saturation(f32::NAN).is_err());
    }
}
