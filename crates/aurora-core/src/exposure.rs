//! The parameters of an Exposure adjustment (0.239.0): Photoshop's
//! Exposure, Offset and Gamma Correction sliders.
//!
//! It lives in `aurora-core`, beside [`crate::LevelsParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::ExposureOp`)
//!
//! Per channel, on **straight RGB converted from encoded sRGB to linear
//! light** (IEC 61966-2-1, `aurora_color::srgb_to_linear`), so the
//! exposure is a real change of stops:
//!
//! ```text
//! x = linear * 2^exposure + offset
//! y = sign(x) * |x|^(1 / gamma)         (negative x gives a negative y)
//! out = linear_to_srgb(clamp(y, 0, 1))
//! ```
//!
//! A negative `y` therefore clamps to black. Gamma works as in Levels:
//! above 1 it brightens the midtones, below 1 it darkens them. The
//! neutral settings (exposure 0, offset 0, gamma 1) are an exact
//! passthrough (the pixel is not converted at all). **A choice, not
//! compared with Photoshop**: the sign of the gamma exponent and the
//! linear-light placement of the offset follow Photoshop's description
//! of the 32-bit Exposure control, not a measurement.
//!
//! # Validation
//!
//! Every value finite; exposure in `-20..=20` stops, offset in
//! `-0.5..=0.5`, gamma in `0.10..=9.99` (the Levels gamma range). A value
//! outside is an error, never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple of three `f32`s `(exposure, offset,
//! gamma)`, 12 bytes.

use crate::levels::{GAMMA_MAX, GAMMA_MIN};

/// The largest exposure change in stops, either way.
pub const EXPOSURE_MAX: f32 = 20.0;

/// The largest offset, either way.
pub const OFFSET_MAX: f32 = 0.5;

/// Why an [`ExposureParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// The exposure is outside `-20..=20`.
    Exposure,
    /// The offset is outside `-0.5..=0.5`.
    Offset,
    /// The gamma is outside `0.10..=9.99`.
    Gamma,
}

impl std::fmt::Display for ExposureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "an Exposure value is not a finite number",
            Self::Exposure => "the Exposure is outside -20..20",
            Self::Offset => "the Exposure offset is outside -0.5..0.5",
            Self::Gamma => "the Exposure gamma is outside 0.10..9.99",
        })
    }
}

impl std::error::Error for ExposureError {}

/// The wire form: see the module docs.
type Wire = (f32, f32, f32);

/// An Exposure adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct ExposureParams {
    exposure: f32,
    offset: f32,
    gamma: f32,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for ExposureParams {}

impl Default for ExposureParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for ExposureParams {
    type Error = ExposureError;

    fn try_from(v: Wire) -> Result<Self, ExposureError> {
        Self::new(v.0, v.1, v.2)
    }
}

impl From<ExposureParams> for Wire {
    fn from(p: ExposureParams) -> Self {
        (p.exposure, p.offset, p.gamma)
    }
}

impl ExposureParams {
    /// Exposure 0, offset 0, gamma 1: changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            exposure: 0.0,
            offset: 0.0,
            gamma: 1.0,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`ExposureError`] naming the first rule broken.
    pub fn new(exposure: f32, offset: f32, gamma: f32) -> Result<Self, ExposureError> {
        if !exposure.is_finite() || !offset.is_finite() || !gamma.is_finite() {
            return Err(ExposureError::NotFinite);
        }
        if !(-EXPOSURE_MAX..=EXPOSURE_MAX).contains(&exposure) {
            return Err(ExposureError::Exposure);
        }
        if !(-OFFSET_MAX..=OFFSET_MAX).contains(&offset) {
            return Err(ExposureError::Offset);
        }
        if !(GAMMA_MIN..=GAMMA_MAX).contains(&gamma) {
            return Err(ExposureError::Gamma);
        }
        Ok(Self {
            exposure,
            offset,
            gamma,
        })
    }

    /// The exposure in stops, `-20..=20`.
    #[must_use]
    pub const fn exposure(&self) -> f32 {
        self.exposure
    }

    /// The offset, `-0.5..=0.5`.
    #[must_use]
    pub const fn offset(&self) -> f32 {
        self.offset
    }

    /// The gamma correction, `0.10..=9.99`.
    #[must_use]
    pub const fn gamma(&self) -> f32 {
        self.gamma
    }

    /// Whether the adjustment changes nothing: exposure and offset exactly
    /// zero and gamma exactly one.
    #[must_use]
    // Exact comparison on purpose: only the neutral settings are skipped.
    #[allow(clippy::float_cmp)]
    pub fn is_identity(&self) -> bool {
        self.exposure == 0.0 && self.offset == 0.0 && self.gamma == 1.0
    }

    /// A copy with the exposure replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_exposure(&self, exposure: f32) -> Result<Self, ExposureError> {
        Self::new(exposure, self.offset, self.gamma)
    }

    /// A copy with the offset replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_offset(&self, offset: f32) -> Result<Self, ExposureError> {
        Self::new(self.exposure, offset, self.gamma)
    }

    /// A copy with the gamma replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_gamma(&self, gamma: f32) -> Result<Self, ExposureError> {
        Self::new(self.exposure, self.offset, gamma)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{ExposureError as E, ExposureParams as P};

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
        assert!(!ok(P::new(0.01, 0.0, 1.0)).is_identity());
        assert!(!ok(P::new(0.0, 0.001, 1.0)).is_identity());
        assert!(!ok(P::new(0.0, 0.0, 1.01)).is_identity());
        assert!(!ok(P::new(0.0, 0.0, 0.99)).is_identity());
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new(-20.0, -0.5, 0.10).is_ok());
        assert!(P::new(20.0, 0.5, 9.99).is_ok());
        assert_eq!(P::new(20.01, 0.0, 1.0), Err(E::Exposure));
        assert_eq!(P::new(-20.01, 0.0, 1.0), Err(E::Exposure));
        assert_eq!(P::new(0.0, 0.51, 1.0), Err(E::Offset));
        assert_eq!(P::new(0.0, -0.51, 1.0), Err(E::Offset));
        assert_eq!(P::new(0.0, 0.0, 0.09), Err(E::Gamma));
        assert_eq!(P::new(0.0, 0.0, 10.0), Err(E::Gamma));
        assert_eq!(P::new(0.0, 0.0, 0.0), Err(E::Gamma));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(P::new(bad, 0.0, 1.0), Err(E::NotFinite));
            assert_eq!(P::new(0.0, bad, 1.0), Err(E::NotFinite));
            assert_eq!(P::new(0.0, 0.0, bad), Err(E::NotFinite));
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(-2.5, 0.125, 2.2));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes.len(), 12, "three f32s, positional, no framing");
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        let Ok(bad) = postcard::to_allocvec(&(0.0_f32, 0.0_f32, 0.0_f32)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&bad).is_err(), "gamma 0 refused");
        let Ok(nan) = postcard::to_allocvec(&(0.0_f32, f32::NAN, 1.0_f32)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&nan).is_err());
        assert!(postcard::from_bytes::<P>(bytes.get(..11).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_keep_the_other_values_and_validate() {
        let p = ok(P::new(1.0, 0.25, 2.0));
        let q = ok(p.with_exposure(2.0));
        assert_eq!((q.exposure(), q.offset(), q.gamma()), (2.0, 0.25, 2.0));
        let r = ok(p.with_offset(-0.25));
        assert_eq!((r.exposure(), r.offset(), r.gamma()), (1.0, -0.25, 2.0));
        let s = ok(p.with_gamma(3.0));
        assert_eq!((s.exposure(), s.offset(), s.gamma()), (1.0, 0.25, 3.0));
        assert!(p.with_exposure(99.0).is_err());
        assert!(p.with_offset(1.0).is_err());
        assert!(p.with_gamma(0.0).is_err());
    }
}
