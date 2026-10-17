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

    /// A synthetic RGB matrix/gamma profile: `white` and the three
    /// `primaries` are CIE xy chromaticities, `gamma` a pure power curve
    /// (`0` means the sRGB parametric curve). Lets tests, and tools that
    /// need one, make an Adobe-RGB- or Display-P3-like profile without a
    /// file.
    ///
    /// # Errors
    ///
    /// [`ColorError::InvalidProfile`] if `lcms2` rejects the chromaticities.
    pub fn synthetic_rgb(
        white: [f64; 2],
        primaries: [[f64; 2]; 3],
        gamma: f64,
    ) -> Result<Self, ColorError> {
        let xyy = |p: [f64; 2]| lcms2::CIExyY {
            x: p[0],
            y: p[1],
            Y: 1.0,
        };
        let curve = if gamma == 0.0 {
            lcms2::ToneCurve::new_parametric(
                4,
                &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
            )
            .map_err(ColorError::InvalidProfile)?
        } else {
            lcms2::ToneCurve::new(gamma)
        };
        let triple = lcms2::CIExyYTRIPLE {
            Red: xyy(primaries[0]),
            Green: xyy(primaries[1]),
            Blue: xyy(primaries[2]),
        };
        let inner = lcms2::Profile::new_rgb(&xyy(white), &triple, &[&curve, &curve, &curve])
            .map_err(ColorError::InvalidProfile)?;
        Ok(Self { inner })
    }

    /// A synthetic grayscale profile (D65 white, a pure power `gamma`
    /// curve): for tests of the wrong-colour-space refusal and of grey
    /// imports.
    ///
    /// # Errors
    ///
    /// [`ColorError::InvalidProfile`] if `lcms2` rejects it.
    pub fn synthetic_gray(gamma: f64) -> Result<Self, ColorError> {
        let curve = lcms2::ToneCurve::new(gamma);
        let white = lcms2::CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        };
        let inner = lcms2::Profile::new_gray(&white, &curve).map_err(ColorError::InvalidProfile)?;
        Ok(Self { inner })
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

    /// The built-in sRGB profile's serialised bytes, made once per
    /// process and reused (0.177.0 review M-2). `lcms2` stamps the current
    /// date and time (and may compute a profile ID) into every
    /// serialisation's header, so two calls a second apart differ in
    /// those bytes; a caller that writes the bytes and later checks them
    /// (the PSD writer and its verifier) needs one stable copy.
    ///
    /// # Errors
    ///
    /// [`ColorError::SerializeFailed`] if `lcms2` cannot serialise it.
    pub fn srgb_bytes() -> Result<Vec<u8>, ColorError> {
        static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        if let Some(bytes) = BYTES.get() {
            return Ok(bytes.clone());
        }
        let made = Self::srgb().to_bytes()?;
        Ok(BYTES.get_or_init(|| made).clone())
    }

    /// Whether this profile is the built-in [`Self::srgb`] profile:
    /// its [`Self::to_bytes`] equal [`Self::srgb_bytes`] in every byte
    /// but the ones `lcms2` stamps per serialisation — the header's
    /// creation date and time (bytes 24..36) and profile ID (84..100)
    /// ([`icc_bytes_match_ignoring_stamps`]). Deliberately narrow: no
    /// attempt at colorimetric equivalence, so some *other* sRGB-labelled
    /// profile with different bytes is `false` and gets full profile
    /// treatment.
    ///
    /// Until 0.177.0 this compared the raw bytes and claimed that was
    /// deterministic; it was not — the stamped date made it `false`
    /// whenever the two serialisations straddled a second, which could
    /// make a PNG export embed an `iCCP` profile instead of its `sRGB`
    /// chunk.
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
        Ok(icc_bytes_match_ignoring_stamps(
            &self.to_bytes()?,
            &Self::srgb_bytes()?,
        ))
    }
}

/// The ICC header ranges a serialiser stamps: the creation date and time
/// (24..36) and the profile ID, an MD5 of the profile (84..100).
const STAMPED: [std::ops::Range<usize>; 2] = [24..36, 84..100];

/// Whether two serialised ICC profiles are the same profile: equal
/// length and equal bytes everywhere outside the header's creation date
/// and time and profile ID (0.177.0 review M-2).
#[must_use]
pub fn icc_bytes_match_ignoring_stamps(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .enumerate()
            .all(|(i, (x, y))| x == y || STAMPED.iter().any(|r| r.contains(&i)))
}

#[cfg(test)]
mod tests {
    use super::{IccProfile, icc_bytes_match_ignoring_stamps};

    #[test]
    fn is_exactly_srgb_ignores_a_different_creation_stamp() {
        let Ok(bytes) = IccProfile::srgb().to_bytes() else {
            unreachable!("serialises")
        };
        // Another moment's serialisation: a different date and time, and
        // a different profile ID.
        let mut later = bytes.clone();
        for i in (24..36).chain(84..100) {
            if let Some(b) = later.get_mut(i) {
                *b = b.wrapping_add(1);
            }
        }
        assert!(icc_bytes_match_ignoring_stamps(&bytes, &later));
        let Ok(parsed) = IccProfile::from_bytes(&later) else {
            unreachable!("still a valid profile")
        };
        assert!(matches!(parsed.is_exactly_srgb(), Ok(true)));
        // Any byte outside the stamps still counts.
        let mut other = bytes.clone();
        if let Some(b) = other.get_mut(40) {
            *b = b.wrapping_add(1);
        }
        assert!(!icc_bytes_match_ignoring_stamps(&bytes, &other));
        assert!(!icc_bytes_match_ignoring_stamps(
            &bytes,
            bytes.get(..bytes.len() - 1).unwrap_or_default()
        ));
        // The cached bytes are stable within the process.
        assert_eq!(IccProfile::srgb_bytes().ok(), IccProfile::srgb_bytes().ok());
    }

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
