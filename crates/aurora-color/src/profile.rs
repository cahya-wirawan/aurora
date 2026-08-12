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

    /// Re-serializes this profile back to raw ICC bytes — [`Self::from_bytes`]'s
    /// own inverse, what a caller needs to embed a real profile into a
    /// file (a `.aur` document's own manifest, or eventually a PNG/TIFF
    /// chunk) instead of just tagging pixels with it in memory. Works
    /// for a profile built via [`Self::srgb`] too, not just one loaded
    /// from real file bytes — `lcms2` can always re-encode a profile it
    /// holds, built-in or parsed.
    ///
    /// # Errors
    ///
    /// Returns [`ColorError::SerializeFailed`] if `lcms2` fails to
    /// re-encode the profile — not expected in practice, but checked
    /// rather than assumed.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ColorError> {
        self.inner.icc().map_err(ColorError::SerializeFailed)
    }

    /// Whether this profile is *exactly* the built-in [`Self::srgb`]
    /// profile — deliberately narrow: a byte-exact match of
    /// [`Self::to_bytes`] output against `Self::srgb().to_bytes()`,
    /// confirmed deterministic (two independent `Self::srgb()`
    /// instances re-serialize to identical bytes — `lcms2`'s built-in
    /// sRGB definition carries no per-instance timestamp/ID the way a
    /// freshly-authored profile might), not any attempt at real
    /// colorimetric equivalence between differently-encoded sRGB-ish
    /// profiles. A file carrying some *other* sRGB-labeled ICC profile
    /// with different bytes correctly returns `false` here — Aurora
    /// can't cheaply prove that's colorimetrically identical, so it
    /// gets full profile treatment rather than being silently
    /// reclassified.
    ///
    /// Exists here rather than in a format crate (e.g. `aurora-io`)
    /// because it's a colour-domain concept: any format with its own
    /// "lightweight sRGB tag" convention (PNG's `sRGB` chunk today,
    /// potentially others later) can reuse it without reaching back up
    /// the crate layering.
    ///
    /// # Errors
    ///
    /// Returns [`ColorError::SerializeFailed`] if either profile fails
    /// to serialize via [`Self::to_bytes`].
    pub fn is_exactly_srgb(&self) -> Result<bool, ColorError> {
        Ok(self.to_bytes()? == Self::srgb().to_bytes()?)
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

    #[test]
    fn to_bytes_round_trips_a_real_loaded_profile() {
        let profile = match IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        let bytes = match profile.to_bytes() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!bytes.is_empty());
        // Re-parsing lcms2's own re-encoding must succeed -- the actual
        // round trip a `.aur` save/reopen cycle depends on. Not asserted
        // byte-for-byte against the original file: lcms2 is free to
        // re-encode with a different profile ID/timestamp than whatever
        // the original file happened to carry.
        if let Err(err) = IccProfile::from_bytes(&bytes) {
            unreachable!("re-parsing a just-serialized profile must succeed: {err:?}");
        }
    }

    #[test]
    fn to_bytes_also_works_for_the_built_in_srgb_profile() {
        let bytes = match IccProfile::srgb().to_bytes() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!bytes.is_empty());
        if let Err(err) = IccProfile::from_bytes(&bytes) {
            unreachable!("{err:?}");
        }
    }

    #[test]
    fn is_exactly_srgb_is_true_for_the_built_in_srgb_profile() {
        match IccProfile::srgb().is_exactly_srgb() {
            Ok(is_srgb) => assert!(is_srgb, "the built-in sRGB profile must match itself"),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn is_exactly_srgb_is_false_for_a_real_different_profile() {
        let profile = match IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        match profile.is_exactly_srgb() {
            Ok(is_srgb) => assert!(
                !is_srgb,
                "a real, different embedded profile must not match sRGB"
            ),
            Err(err) => unreachable!("{err:?}"),
        }
    }
}
