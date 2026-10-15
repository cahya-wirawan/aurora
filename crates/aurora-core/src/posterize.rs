//! The parameter of a Posterize adjustment (0.244.0): how many levels each
//! channel keeps.
//!
//! # The math (applied by `aurora_filters::PosterizeOp`)
//!
//! On **straight, encoded** sRGB, each channel on its own, input clamped to
//! `[0, 1]`. With `n` = [`PosterizeParams::levels`], the channel falls into
//! bin `min(floor(x n), n - 1)` and becomes `bin / (n - 1)`: `n` evenly
//! spaced output values, the first black and the last white. Alpha is
//! untouched. Fitted on `adjustments/posterize_rgb.psd` (8-bit) and
//! `posterize_16bits_rgb.psd` against Photoshop's merged images (levels 21,
//! 13, 7 and 3): the 16-bit file's bin edges are exactly `k / n` (mean error
//! 0.2 levels, from the composite under it being 8-bit); Photoshop's 8-bit
//! mode places them at `k 256 / n` on the 0 to 255 scale and truncates the
//! output, so an 8-bit document differs from Photoshop by one bin edge
//! (one input level in 256 per edge) and under one output level, about 0.4
//! to 1.0 levels on average.
//!
//! **255 levels is treated as no change** (Photoshop's maximum, which
//! moves a pixel by at most one 8-bit level).
//!
//! # Validation
//!
//! `2..=255`; any other value is an error, never clamped, including when
//! decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: one `u8`.

/// The fewest levels.
pub const POSTERIZE_MIN: u8 = 2;

/// The most levels (treated as no change).
pub const POSTERIZE_MAX: u8 = 255;

/// Photoshop's default.
pub const POSTERIZE_DEFAULT: u8 = 4;

/// Why a [`PosterizeParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosterizeError {
    /// The level count is outside `2..=255`.
    Levels,
}

impl std::fmt::Display for PosterizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a Posterize level count is outside 2..255")
    }
}

impl std::error::Error for PosterizeError {}

/// A Posterize adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct PosterizeParams {
    levels: u8,
}

impl Default for PosterizeParams {
    fn default() -> Self {
        Self {
            levels: POSTERIZE_DEFAULT,
        }
    }
}

impl TryFrom<u8> for PosterizeParams {
    type Error = PosterizeError;

    fn try_from(levels: u8) -> Result<Self, PosterizeError> {
        Self::new(levels)
    }
}

impl From<PosterizeParams> for u8 {
    fn from(p: PosterizeParams) -> Self {
        p.levels
    }
}

impl PosterizeParams {
    /// A validated record.
    ///
    /// # Errors
    /// [`PosterizeError::Levels`] for `0` and `1`.
    pub const fn new(levels: u8) -> Result<Self, PosterizeError> {
        if levels < POSTERIZE_MIN {
            return Err(PosterizeError::Levels);
        }
        Ok(Self { levels })
    }

    /// The number of levels, `2..=255`.
    #[must_use]
    pub const fn levels(&self) -> u8 {
        self.levels
    }

    /// Whether the adjustment changes nothing: 255 levels.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.levels == POSTERIZE_MAX
    }
}

#[cfg(test)]
mod tests {
    use super::{PosterizeError as E, PosterizeParams as P};

    #[test]
    fn the_range_is_two_to_two_fifty_five() {
        assert_eq!(P::new(0), Err(E::Levels));
        assert_eq!(P::new(1), Err(E::Levels));
        for n in [2_u8, 3, 4, 254, 255] {
            assert_eq!(P::new(n).map(|p| p.levels()), Ok(n));
        }
        assert_eq!(P::default().levels(), 4);
    }

    #[test]
    fn only_255_is_an_identity() {
        assert!(P::new(255).is_ok_and(|p| p.is_identity()));
        assert!(P::new(254).is_ok_and(|p| !p.is_identity()));
        assert!(!P::default().is_identity());
    }

    #[test]
    fn the_wire_form_is_one_byte_and_refuses_bad_counts() {
        let Ok(p) = P::new(13) else {
            unreachable!("valid");
        };
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes, vec![13]);
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        assert!(postcard::from_bytes::<P>(&[0]).is_err());
        assert!(postcard::from_bytes::<P>(&[1]).is_err());
    }
}
