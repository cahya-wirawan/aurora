//! ICC profile loading (ADR 0008: `lcms2`, statically linked).

use crate::error::ColorError;

/// A loaded ICC profile, ready to be used as either side of a
/// [`crate::Transform`].
#[derive(Debug)]
pub struct IccProfile {
    pub(crate) inner: lcms2::Profile,
}

impl IccProfile {
    /// Parses `bytes` as an ICC profile (the whole file's contents, e.g.
    /// read from a real `.icc`/`.icm` file).
    ///
    /// # Errors
    ///
    /// Returns [`ColorError::InvalidProfile`] if `bytes` isn't a valid ICC
    /// profile.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ColorError> {
        let inner = lcms2::Profile::new_icc(bytes).map_err(ColorError::InvalidProfile)?;
        Ok(Self { inner })
    }

    /// A profile representing the standard sRGB space — `lcms2`'s own
    /// built-in definition, not parsed from any file.
    #[must_use]
    pub fn srgb() -> Self {
        Self {
            inner: lcms2::Profile::new_srgb(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IccProfile;

    // CC0-licensed, from the colord-data Debian package -- see
    // corpora/icc/README.md for full provenance.
    const SRGB_ICC: &[u8] = include_bytes!("../../../corpora/icc/sRGB.icc");
    const ECI_RGBV2_ICC: &[u8] = include_bytes!("../../../corpora/icc/ECI-RGBv2.icc");

    #[test]
    fn from_bytes_parses_real_profiles() {
        if let Err(err) = IccProfile::from_bytes(SRGB_ICC) {
            unreachable!("{err:?}");
        }
        if let Err(err) = IccProfile::from_bytes(ECI_RGBV2_ICC) {
            unreachable!("{err:?}");
        }
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        match IccProfile::from_bytes(b"not an icc profile") {
            Err(_) => {}
            Ok(_) => unreachable!("garbage bytes must not parse as a valid ICC profile"),
        }
    }

    #[test]
    fn srgb_builds_a_usable_profile() {
        // Not much to assert on the profile alone -- real coverage is
        // `transform::tests` actually transforming through it.
        let _ = IccProfile::srgb();
    }
}
