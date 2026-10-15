//! The parameters of a Vibrance adjustment (0.239.0): Photoshop's
//! Vibrance and Saturation sliders.
//!
//! It lives in `aurora-core`, beside [`crate::HueSaturationParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::VibranceOp`)
//!
//! On **straight, encoded** RGB converted to HSL (hue in degrees,
//! saturation `s` and lightness in `[0, 1]`; the Hue/Saturation model):
//!
//! ```text
//! k = vibrance / 100
//! k > 0:  s1 = s * (1 + k * (1 - s) * protect(h))     boost the dull colours
//! k <= 0: s1 = s * (1 + k)                            plain proportional cut
//! m = saturation / 100
//! s2 = s1 * (1 + m)                  m <= 0
//!    = min(1, s1 / (1 - m))          0 < m < 1
//!    = 1 if s1 > 0, else 0           m = 1
//! hue and lightness unchanged; a grey pixel (no hue) is unchanged.
//! protect(h) = 1 - 0.5 * bump(h),  bump = (1 + cos(pi * (h - 30) / 30)) / 2
//!              for h in [0, 60], else 0                skin tones
//! ```
//!
//! So a positive vibrance multiplies a colour's saturation by up to
//! `1 + k` for a nearly grey colour, by nothing for a fully saturated one
//! (`s1 <= 1` always: `s + s(1 - s) = 1 - (1 - s)^2`), and by at most half
//! as much for orange-red hues around 30 degrees (the skin-tone guard).
//! `s1` is monotone in `s` for every `k`. **A choice, not compared with
//! Photoshop**: Photoshop's algorithm is unpublished; the shape (boost the
//! least saturated most, spare saturated colours and skin) follows its
//! documented intent. The Saturation slider is the Hue/Saturation master
//! saturation rule, applied after Vibrance.
//!
//! # Validation
//!
//! Both values finite and in `-100..=100`; a value outside is an error,
//! never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple of two `f32`s `(vibrance, saturation)`,
//! 8 bytes.

/// Why a [`VibranceParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibranceError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// The vibrance is outside `-100..=100`.
    Vibrance,
    /// The saturation is outside `-100..=100`.
    Saturation,
}

impl std::fmt::Display for VibranceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Vibrance value is not a finite number",
            Self::Vibrance => "the Vibrance is outside -100..100",
            Self::Saturation => "the Vibrance saturation is outside -100..100",
        })
    }
}

impl std::error::Error for VibranceError {}

/// The wire form: see the module docs.
type Wire = (f32, f32);

/// A Vibrance adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct VibranceParams {
    vibrance: f32,
    saturation: f32,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for VibranceParams {}

impl Default for VibranceParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for VibranceParams {
    type Error = VibranceError;

    fn try_from(v: Wire) -> Result<Self, VibranceError> {
        Self::new(v.0, v.1)
    }
}

impl From<VibranceParams> for Wire {
    fn from(p: VibranceParams) -> Self {
        (p.vibrance, p.saturation)
    }
}

impl VibranceParams {
    /// Both sliders at zero: changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            vibrance: 0.0,
            saturation: 0.0,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`VibranceError`] naming the first rule broken.
    pub fn new(vibrance: f32, saturation: f32) -> Result<Self, VibranceError> {
        if !vibrance.is_finite() || !saturation.is_finite() {
            return Err(VibranceError::NotFinite);
        }
        if !(-100.0..=100.0).contains(&vibrance) {
            return Err(VibranceError::Vibrance);
        }
        if !(-100.0..=100.0).contains(&saturation) {
            return Err(VibranceError::Saturation);
        }
        Ok(Self {
            vibrance,
            saturation,
        })
    }

    /// The vibrance, `-100..=100`.
    #[must_use]
    pub const fn vibrance(&self) -> f32 {
        self.vibrance
    }

    /// The saturation, `-100..=100`.
    #[must_use]
    pub const fn saturation(&self) -> f32 {
        self.saturation
    }

    /// Whether the adjustment changes nothing: both sliders exactly zero.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.vibrance == 0.0 && self.saturation == 0.0
    }

    /// A copy with the vibrance replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_vibrance(&self, vibrance: f32) -> Result<Self, VibranceError> {
        Self::new(vibrance, self.saturation)
    }

    /// A copy with the saturation replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_saturation(&self, saturation: f32) -> Result<Self, VibranceError> {
        Self::new(self.vibrance, saturation)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{VibranceError as E, VibranceParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_detected() {
        assert!(P::identity().is_identity());
        assert!(P::default().is_identity());
        assert!(ok(P::new(-0.0, 0.0)).is_identity());
        assert!(!ok(P::new(0.5, 0.0)).is_identity());
        assert!(!ok(P::new(0.0, -0.5)).is_identity());
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new(-100.0, 100.0).is_ok());
        assert!(P::new(100.0, -100.0).is_ok());
        assert_eq!(P::new(100.5, 0.0), Err(E::Vibrance));
        assert_eq!(P::new(-100.5, 0.0), Err(E::Vibrance));
        assert_eq!(P::new(0.0, 100.5), Err(E::Saturation));
        assert_eq!(P::new(0.0, -100.5), Err(E::Saturation));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(P::new(bad, 0.0), Err(E::NotFinite));
            assert_eq!(P::new(0.0, bad), Err(E::NotFinite));
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(-27.0, 64.0));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes.len(), 8, "two f32s, positional, no framing");
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        let Ok(bad) = postcard::to_allocvec(&(200.0_f32, 0.0_f32)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&bad).is_err(), "decode validates");
        let Ok(nan) = postcard::to_allocvec(&(f32::NAN, 0.0_f32)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&nan).is_err());
        assert!(postcard::from_bytes::<P>(bytes.get(..7).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_keep_the_other_value_and_validate() {
        let p = ok(P::new(10.0, 20.0));
        let q = ok(p.with_vibrance(11.0));
        assert_eq!((q.vibrance(), q.saturation()), (11.0, 20.0));
        let r = ok(p.with_saturation(21.0));
        assert_eq!((r.vibrance(), r.saturation()), (10.0, 21.0));
        assert!(p.with_vibrance(999.0).is_err());
        assert!(p.with_saturation(f32::NAN).is_err());
    }
}
