//! The parameters of a Brightness/Contrast adjustment (0.244.0):
//! brightness, contrast and the Use Legacy switch.
//!
//! # The math (applied by `aurora_filters::BrightnessContrastOp`)
//!
//! See that module: both modes are a per-channel function of the encoded
//! value (the fixtures' output depends on the input channel alone), alpha
//! untouched. **Legacy** is a linear contrast about the mean level 127 plus
//! a brightness shift in 8-bit levels; **the default mode is an
//! approximation** of Photoshop's unpublished curve, fitted on four
//! fixtures.
//!
//! # Validation
//!
//! Integers, as Photoshop's sliders are. The default mode takes brightness
//! `-150..=150` and contrast `-50..=100`; Use Legacy takes both in
//! `-100..=100`. A value outside the mode's range is an error, never
//! clamped, including when decoding a `.aur`. (Switching Use Legacy in the
//! Properties editor clamps the two values into the new mode's range; that
//! is an edit the user made, not a repair of stored data.)
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(brightness: i16, contrast: i16, legacy:
//! bool)`.

/// The default mode's brightness range.
pub const BRIGHTNESS_MAX: i16 = 150;

/// The default mode's contrast range.
pub const CONTRAST_MIN: i16 = -50;

/// The default mode's largest contrast.
pub const CONTRAST_MAX: i16 = 100;

/// The largest magnitude of either slider in Use Legacy mode.
pub const LEGACY_MAX: i16 = 100;

/// The mean level Photoshop pivots the contrast about (the `means` key of
/// the layer's descriptor, and the last field of a legacy `brit` block), on
/// the 0 to 255 scale.
pub const BRIGHTNESS_CONTRAST_MEAN: u8 = 127;

/// Why a [`BrightnessContrastParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrightnessContrastError {
    /// Brightness is outside the mode's range.
    Brightness,
    /// Contrast is outside the mode's range.
    Contrast,
}

impl std::fmt::Display for BrightnessContrastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Brightness => "a Brightness/Contrast brightness is outside its range",
            Self::Contrast => "a Brightness/Contrast contrast is outside its range",
        })
    }
}

impl std::error::Error for BrightnessContrastError {}

/// The wire form: see the module docs.
type Wire = (i16, i16, bool);

/// A Brightness/Contrast adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct BrightnessContrastParams {
    brightness: i16,
    contrast: i16,
    legacy: bool,
}

impl Default for BrightnessContrastParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for BrightnessContrastParams {
    type Error = BrightnessContrastError;

    fn try_from(v: Wire) -> Result<Self, BrightnessContrastError> {
        Self::new(v.0, v.1, v.2)
    }
}

impl From<BrightnessContrastParams> for Wire {
    fn from(p: BrightnessContrastParams) -> Self {
        (p.brightness, p.contrast, p.legacy)
    }
}

impl BrightnessContrastParams {
    /// Brightness 0, contrast 0, the default mode: changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            brightness: 0,
            contrast: 0,
            legacy: false,
        }
    }

    /// The allowed `(brightness, contrast)` ranges of a mode, inclusive.
    #[must_use]
    pub const fn ranges(legacy: bool) -> ((i16, i16), (i16, i16)) {
        if legacy {
            ((-LEGACY_MAX, LEGACY_MAX), (-LEGACY_MAX, LEGACY_MAX))
        } else {
            ((-BRIGHTNESS_MAX, BRIGHTNESS_MAX), (CONTRAST_MIN, CONTRAST_MAX))
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`BrightnessContrastError`] naming the first rule broken.
    pub const fn new(
        brightness: i16,
        contrast: i16,
        legacy: bool,
    ) -> Result<Self, BrightnessContrastError> {
        let ((b_lo, b_hi), (c_lo, c_hi)) = Self::ranges(legacy);
        if brightness < b_lo || brightness > b_hi {
            return Err(BrightnessContrastError::Brightness);
        }
        if contrast < c_lo || contrast > c_hi {
            return Err(BrightnessContrastError::Contrast);
        }
        Ok(Self {
            brightness,
            contrast,
            legacy,
        })
    }

    /// The brightness slider.
    #[must_use]
    pub const fn brightness(&self) -> i16 {
        self.brightness
    }

    /// The contrast slider.
    #[must_use]
    pub const fn contrast(&self) -> i16 {
        self.contrast
    }

    /// Whether Use Legacy is on.
    #[must_use]
    pub const fn legacy(&self) -> bool {
        self.legacy
    }

    /// Whether the adjustment changes nothing: both sliders at zero (in
    /// either mode).
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.brightness == 0 && self.contrast == 0
    }

    /// A copy with brightness replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub const fn with_brightness(&self, brightness: i16) -> Result<Self, BrightnessContrastError> {
        Self::new(brightness, self.contrast, self.legacy)
    }

    /// A copy with contrast replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub const fn with_contrast(&self, contrast: i16) -> Result<Self, BrightnessContrastError> {
        Self::new(self.brightness, contrast, self.legacy)
    }

    /// A copy with Use Legacy switched; both sliders are clamped into the
    /// new mode's range (the only place a value is clamped, and only on
    /// this edit).
    #[must_use]
    pub fn with_legacy(&self, legacy: bool) -> Self {
        let ((b_lo, b_hi), (c_lo, c_hi)) = Self::ranges(legacy);
        Self {
            brightness: self.brightness.clamp(b_lo, b_hi),
            contrast: self.contrast.clamp(c_lo, c_hi),
            legacy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrightnessContrastError as E, BrightnessContrastParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_default_mode_ranges_are_checked_at_both_ends() {
        assert!(P::new(150, 100, false).is_ok());
        assert!(P::new(-150, -50, false).is_ok());
        assert_eq!(P::new(151, 0, false), Err(E::Brightness));
        assert_eq!(P::new(-151, 0, false), Err(E::Brightness));
        assert_eq!(P::new(0, 101, false), Err(E::Contrast));
        assert_eq!(P::new(0, -51, false), Err(E::Contrast));
    }

    #[test]
    fn the_legacy_ranges_are_checked_at_both_ends() {
        assert!(P::new(100, 100, true).is_ok());
        assert!(P::new(-100, -100, true).is_ok());
        assert_eq!(P::new(101, 0, true), Err(E::Brightness));
        assert_eq!(P::new(-101, 0, true), Err(E::Brightness));
        assert_eq!(P::new(0, 101, true), Err(E::Contrast));
        assert_eq!(P::new(0, -101, true), Err(E::Contrast));
    }

    #[test]
    fn identity_is_both_sliders_at_zero_in_either_mode() {
        assert!(P::identity().is_identity() && P::default().is_identity());
        assert!(ok(P::new(0, 0, true)).is_identity());
        assert!(!ok(P::new(1, 0, false)).is_identity());
        assert!(!ok(P::new(0, 1, true)).is_identity());
    }

    #[test]
    fn setters_edit_one_field_and_switching_the_mode_clamps() {
        let p = ok(P::new(140, 90, false));
        assert_eq!(ok(p.with_brightness(-3)).contrast(), 90);
        assert_eq!(ok(p.with_contrast(-3)).brightness(), 140);
        assert_eq!(p.with_brightness(151), Err(E::Brightness));
        let legacy = p.with_legacy(true);
        assert_eq!(
            (legacy.brightness(), legacy.contrast(), legacy.legacy()),
            (100, 90, true)
        );
        let back = ok(P::new(-80, -100, true)).with_legacy(false);
        assert_eq!(
            (back.brightness(), back.contrast(), back.legacy()),
            (-80, -50, false)
        );
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(-95, -50, false));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        for bad in [(151_i16, 0_i16, false), (0, 101, false), (101, 0, true), (0, -101, true)] {
            let Ok(wire) = postcard::to_allocvec(&bad) else {
                unreachable!("encodes");
            };
            assert!(postcard::from_bytes::<P>(&wire).is_err(), "{bad:?}");
        }
        assert!(postcard::from_bytes::<P>(&bytes[..1]).is_err());
    }
}
