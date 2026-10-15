//! The parameter of a Threshold adjustment (0.244.0): one level on
//! Photoshop's 8-bit scale.
//!
//! # The math (applied by `aurora_filters::ThresholdOp`)
//!
//! On **straight, encoded** sRGB. The pixel's grey is
//! `Y = 0.30 R + 0.59 G + 0.11 B` (Photoshop's own weights, fitted on the
//! psd-tools fixtures `adjustments/threshold_*.psd`: 3 mismatching pixels of
//! 40,000 against Photoshop's merged image, against 30 for Rec.601 and 553
//! for Rec.709), on the 0 to 255 scale. A pixel whose `255 Y` reaches
//! [`ThresholdParams::level`] minus one half becomes white, any other
//! black; alpha is untouched. The half level is Photoshop's 8-bit
//! rounding (`round(255 Y) >= level`): the 16-bit fixture reaches the
//! level itself, so a 16-bit document differs from Photoshop by under half
//! a level of grey at the edge of the white region.
//!
//! # Validation
//!
//! The level is `1..=255` (Photoshop's slider); `0` is an error, never
//! clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: one `u8`.

/// The smallest level.
pub const THRESHOLD_MIN: u8 = 1;

/// The largest level.
pub const THRESHOLD_MAX: u8 = 255;

/// Photoshop's default level.
pub const THRESHOLD_DEFAULT: u8 = 128;

/// Why a [`ThresholdParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdError {
    /// The level is outside `1..=255`.
    Level,
}

impl std::fmt::Display for ThresholdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a Threshold level is outside 1..255")
    }
}

impl std::error::Error for ThresholdError {}

/// A Threshold adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct ThresholdParams {
    level: u8,
}

impl Default for ThresholdParams {
    fn default() -> Self {
        Self {
            level: THRESHOLD_DEFAULT,
        }
    }
}

impl TryFrom<u8> for ThresholdParams {
    type Error = ThresholdError;

    fn try_from(level: u8) -> Result<Self, ThresholdError> {
        Self::new(level)
    }
}

impl From<ThresholdParams> for u8 {
    fn from(p: ThresholdParams) -> Self {
        p.level
    }
}

impl ThresholdParams {
    /// A validated record.
    ///
    /// # Errors
    /// [`ThresholdError::Level`] for `0`.
    pub const fn new(level: u8) -> Result<Self, ThresholdError> {
        if level < THRESHOLD_MIN {
            return Err(ThresholdError::Level);
        }
        Ok(Self { level })
    }

    /// The level, `1..=255`.
    #[must_use]
    pub const fn level(&self) -> u8 {
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::{ThresholdError as E, ThresholdParams as P};

    #[test]
    fn the_level_range_is_one_to_two_fifty_five() {
        assert_eq!(P::new(0), Err(E::Level));
        for level in [1_u8, 2, 127, 128, 254, 255] {
            assert_eq!(P::new(level).map(|p| p.level()), Ok(level));
        }
        assert_eq!(P::default().level(), 128);
    }

    #[test]
    fn the_wire_form_is_one_byte_and_refuses_zero() {
        let Ok(p) = P::new(172) else {
            unreachable!("valid");
        };
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes, vec![172]);
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        assert!(postcard::from_bytes::<P>(&[0]).is_err());
        assert!(postcard::from_bytes::<P>(&[]).is_err());
    }
}
