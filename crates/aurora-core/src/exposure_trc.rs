//! Exposure in a Grayscale file's grey profile (task 112): the settings of
//! an Exposure layer imported from a Grayscale PSD together with the
//! profile's tone curve.
//!
//! # What was measured
//!
//! `adjustments/exposure_grayscale.psd` embeds the grey ICC profile "Dot
//! Gain 20%" (resource 1039; a `GRAY` profile whose `kTRC` tag is a
//! 256-entry `curv` table, device value to linear Y). Against Photoshop's
//! own merged image (four Exposure layers with different settings, one
//! with a non-trivial mask) the model that fits to 0.27-0.37 levels mean
//! (8-bit rounding noise is 0.29) is: the encoded grey is decoded with
//! the **sRGB** curve, `x = linear * 2^exposure + offset`, `y = sign(x)
//! |x|^(1/gamma)` clamped to `0..1`, and `y` is encoded with the
//! **inverse of the profile's table**, not with sRGB. Decoding with the
//! table, or encoding with sRGB, are 5 to 40 levels off. The asymmetry is
//! what Photoshop's files show; why it does so is not known. With a table
//! equal to the sRGB curve the model is the plain Exposure.
//!
//! **Not measured**: a neutral layer (exposure 0, offset 0, gamma 1) is
//! left an exact passthrough here, as in the RGB Exposure; no fixture says
//! what Photoshop does with one. Every other gray profile (no profile, a
//! gamma curve, a parametric curve, a table of another length) has no
//! fixture and is not imported.
//!
//! # Validation
//!
//! The table has `2..=4096` entries, is non-decreasing, starts at `0` and
//! ends at `65535` (so the inverse is defined on all of `0..=1`). A value
//! outside is an error, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(ExposureParams, Vec<u16>)`.

use crate::exposure::ExposureParams;

/// The fewest entries a table may have.
pub const GREY_TRC_MIN_ENTRIES: usize = 2;

/// The most entries a table may have (an ICC `curv` table in practice has
/// 256 or 1024).
pub const GREY_TRC_MAX_ENTRIES: usize = 4096;

/// Why a [`GreyTrc`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreyTrcError {
    /// Fewer than 2 or more than 4096 entries.
    Length,
    /// An entry is lower than the one before it.
    NotMonotone,
    /// The first entry is not `0` or the last is not `65535`.
    Ends,
}

impl std::fmt::Display for GreyTrcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Length => "a grey tone curve needs 2 to 4096 entries",
            Self::NotMonotone => "a grey tone curve must not decrease",
            Self::Ends => "a grey tone curve must run from 0 to 65535",
        })
    }
}

impl std::error::Error for GreyTrcError {}

/// A grey profile's tone curve: encoded grey (entry index over `len - 1`)
/// to linear light, as 16-bit values. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Vec<u16>", into = "Vec<u16>")]
pub struct GreyTrc {
    entries: Vec<u16>,
}

impl TryFrom<Vec<u16>> for GreyTrc {
    type Error = GreyTrcError;

    fn try_from(entries: Vec<u16>) -> Result<Self, GreyTrcError> {
        Self::new(entries)
    }
}

impl From<GreyTrc> for Vec<u16> {
    fn from(t: GreyTrc) -> Self {
        t.entries
    }
}

impl GreyTrc {
    /// A validated table.
    ///
    /// # Errors
    /// [`GreyTrcError`] naming the first rule broken.
    pub fn new(entries: Vec<u16>) -> Result<Self, GreyTrcError> {
        if !(GREY_TRC_MIN_ENTRIES..=GREY_TRC_MAX_ENTRIES).contains(&entries.len()) {
            return Err(GreyTrcError::Length);
        }
        if entries.windows(2).any(|w| match w {
            [a, b] => b < a,
            _ => false,
        }) {
            return Err(GreyTrcError::NotMonotone);
        }
        if entries.first() != Some(&0) || entries.last() != Some(&u16::MAX) {
            return Err(GreyTrcError::Ends);
        }
        Ok(Self { entries })
    }

    /// The table, device value to linear light.
    #[must_use]
    pub fn entries(&self) -> &[u16] {
        &self.entries
    }
}

/// The wire form: see the module docs.
type Wire = (ExposureParams, GreyTrc);

/// An Exposure adjustment whose result is encoded through a grey profile's
/// tone curve. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "Wire", into = "Wire")]
pub struct ExposureTrcParams {
    exposure: ExposureParams,
    trc: GreyTrc,
}

impl From<Wire> for ExposureTrcParams {
    fn from(v: Wire) -> Self {
        Self::new(v.0, v.1)
    }
}

impl From<ExposureTrcParams> for Wire {
    fn from(p: ExposureTrcParams) -> Self {
        (p.exposure, p.trc)
    }
}

impl ExposureTrcParams {
    /// The Exposure settings with the table.
    #[must_use]
    pub const fn new(exposure: ExposureParams, trc: GreyTrc) -> Self {
        Self { exposure, trc }
    }

    /// The Exposure settings.
    #[must_use]
    pub const fn exposure(&self) -> ExposureParams {
        self.exposure
    }

    /// The grey profile's tone curve.
    #[must_use]
    pub const fn trc(&self) -> &GreyTrc {
        &self.trc
    }

    /// A copy with the Exposure settings replaced; the table stays.
    #[must_use]
    pub fn with_exposure_params(&self, exposure: ExposureParams) -> Self {
        Self {
            exposure,
            trc: self.trc.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExposureTrcParams, GreyTrc, GreyTrcError as E};
    use crate::ExposureParams;

    fn ramp(n: usize) -> Vec<u16> {
        (0..n)
            .map(|i| {
                let v = (i as f64 / (n - 1) as f64 * 65535.0).round();
                v as u16
            })
            .collect()
    }

    #[test]
    fn a_valid_table_is_accepted_at_its_length_bounds() {
        assert!(GreyTrc::new(ramp(2)).is_ok());
        assert!(GreyTrc::new(ramp(256)).is_ok());
        assert!(GreyTrc::new(ramp(4096)).is_ok());
    }

    #[test]
    fn length_ends_and_monotonicity_are_validated() {
        assert_eq!(GreyTrc::new(Vec::new()), Err(E::Length));
        assert_eq!(GreyTrc::new(vec![0]), Err(E::Length));
        assert_eq!(GreyTrc::new(ramp(4097)), Err(E::Length));
        let mut down = ramp(256);
        if let Some(v) = down.get_mut(100) {
            *v = 0;
        }
        assert_eq!(GreyTrc::new(down), Err(E::NotMonotone));
        let mut low = ramp(256);
        if let Some(v) = low.first_mut() {
            *v = 1;
        }
        assert_eq!(GreyTrc::new(low), Err(E::Ends));
        let mut high = ramp(256);
        if let Some(v) = high.last_mut() {
            *v = 65534;
        }
        assert_eq!(GreyTrc::new(high), Err(E::Ends));
        // Flat runs are legal.
        assert!(GreyTrc::new(vec![0, 0, 5, 5, 65535, 65535]).is_ok());
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_a_bad_table() {
        let Ok(trc) = GreyTrc::new(ramp(256)) else {
            unreachable!("valid");
        };
        let Ok(e) = ExposureParams::new(1.0, 0.1, 1.5) else {
            unreachable!("valid");
        };
        let p = ExposureTrcParams::new(e, trc);
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(postcard::from_bytes::<ExposureTrcParams>(&bytes), Ok(p));
        // The same bytes with the table replaced by a descending one.
        let Ok(bad) = postcard::to_allocvec(&(e, vec![9_u16, 3])) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<ExposureTrcParams>(&bad).is_err());
        // Truncation at every length is an error, never a panic.
        for n in 0..bytes.len() {
            assert!(
                postcard::from_bytes::<ExposureTrcParams>(bytes.get(..n).unwrap_or(&[])).is_err(),
                "{n}"
            );
        }
    }

    #[test]
    fn setting_the_exposure_keeps_the_table() {
        let Ok(trc) = GreyTrc::new(ramp(256)) else {
            unreachable!("valid");
        };
        let p = ExposureTrcParams::new(ExposureParams::identity(), trc.clone());
        let Ok(e) = ExposureParams::new(2.0, 0.0, 1.0) else {
            unreachable!("valid");
        };
        let q = p.with_exposure_params(e);
        assert_eq!(q.exposure(), e);
        assert_eq!(q.trc(), &trc);
    }
}
