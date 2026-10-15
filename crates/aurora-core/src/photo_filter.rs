//! The parameters of a Photo Filter adjustment (0.239.0): Photoshop's
//! filter colour, Density and Preserve Luminosity.
//!
//! It lives in `aurora-core`, beside [`crate::HueSaturationParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::PhotoFilterOp`)
//!
//! On **straight, encoded** sRGB, per channel, with the filter colour `c`
//! (each channel `0..=1`) and `d = density / 100`:
//!
//! ```text
//! t = 1 + (c - 1) * d                        the per-channel transmission
//! f = in * t                                 multiply toward the colour
//! preserve luminosity:  f = f + (Y(in) - Y(f))   (Y = Rec.709 luma),
//!                       then clamped to [0, 1]
//! ```
//!
//! A white filter (`c = 1,1,1`) is the identity at any density. At density
//! 100 the pixel is multiplied by the filter colour itself. **A choice,
//! not compared with Photoshop**: this is the classic "photo filter"
//! (multiply by a density-weighted colour, then optionally restore the
//! luma by an additive shift); when the shift would leave `[0, 1]` the
//! clamp lets the luma drift a little rather than rescale the colour.
//!
//! # Validation
//!
//! Every value finite; each colour channel in `0..=1`; density in
//! `1..=100`. A value outside is an error, never clamped, including when
//! decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(red, green, blue, density,
//! preserve_luminosity)`, four `f32`s and one `bool`, 17 bytes.

/// Photoshop's preset filter colours, `(name, [r, g, b])` in 8-bit
/// levels: the four the Photoshop Photo Filter dialog lists first among
/// its warming and cooling filters. Values recalled from Photoshop's
/// list, not measured from a file.
pub const PHOTO_FILTER_PRESETS: [(&str, [u8; 3]); 4] = [
    ("Warming Filter (85)", [236, 138, 0]),
    ("Warming Filter (LBA)", [250, 150, 0]),
    ("Cooling Filter (80)", [0, 109, 255]),
    ("Cooling Filter (LBB)", [0, 93, 225]),
];

/// Photoshop's default density.
pub const DEFAULT_DENSITY: f32 = 25.0;

/// Why a [`PhotoFilterParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoFilterError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// A colour channel is outside `0..=1`.
    Colour,
    /// The density is outside `1..=100`.
    Density,
}

impl std::fmt::Display for PhotoFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Photo Filter value is not a finite number",
            Self::Colour => "a Photo Filter colour channel is outside 0..1",
            Self::Density => "the Photo Filter density is outside 1..100",
        })
    }
}

impl std::error::Error for PhotoFilterError {}

/// The wire form: see the module docs.
type Wire = (f32, f32, f32, f32, bool);

/// A Photo Filter adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct PhotoFilterParams {
    colour: [f32; 3],
    density: f32,
    preserve_luminosity: bool,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for PhotoFilterParams {}

impl Default for PhotoFilterParams {
    /// Photoshop's default: the Warming Filter (85) at density 25 with
    /// Preserve Luminosity on.
    fn default() -> Self {
        Self::warming_85()
    }
}

impl TryFrom<Wire> for PhotoFilterParams {
    type Error = PhotoFilterError;

    fn try_from(v: Wire) -> Result<Self, PhotoFilterError> {
        Self::new([v.0, v.1, v.2], v.3, v.4)
    }
}

impl From<PhotoFilterParams> for Wire {
    fn from(p: PhotoFilterParams) -> Self {
        (
            p.colour[0],
            p.colour[1],
            p.colour[2],
            p.density,
            p.preserve_luminosity,
        )
    }
}

impl PhotoFilterParams {
    /// A white filter: changes nothing at any density.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            colour: [1.0, 1.0, 1.0],
            density: DEFAULT_DENSITY,
            preserve_luminosity: true,
        }
    }

    /// The Warming Filter (85) at the default density, Preserve Luminosity
    /// on: what a new Photo Filter layer holds (Photoshop's default).
    #[must_use]
    pub const fn warming_85() -> Self {
        Self {
            colour: [236.0 / 255.0, 138.0 / 255.0, 0.0],
            density: DEFAULT_DENSITY,
            preserve_luminosity: true,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`PhotoFilterError`] naming the first rule broken.
    pub fn new(
        colour: [f32; 3],
        density: f32,
        preserve_luminosity: bool,
    ) -> Result<Self, PhotoFilterError> {
        if colour.iter().any(|c| !c.is_finite()) || !density.is_finite() {
            return Err(PhotoFilterError::NotFinite);
        }
        if colour.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err(PhotoFilterError::Colour);
        }
        if !(1.0..=100.0).contains(&density) {
            return Err(PhotoFilterError::Density);
        }
        Ok(Self {
            colour,
            density,
            preserve_luminosity,
        })
    }

    /// The filter colour, straight encoded sRGB, each channel `0..=1`.
    #[must_use]
    pub const fn colour(&self) -> [f32; 3] {
        self.colour
    }

    /// The density, `1..=100` percent.
    #[must_use]
    pub const fn density(&self) -> f32 {
        self.density
    }

    /// Whether the pixel's luma is restored after the filter.
    #[must_use]
    pub const fn preserve_luminosity(&self) -> bool {
        self.preserve_luminosity
    }

    /// Whether the adjustment changes nothing: a white filter.
    #[must_use]
    // Exact comparison on purpose: only a bit-exact white is skipped.
    #[allow(clippy::float_cmp)]
    pub fn is_identity(&self) -> bool {
        self.colour == [1.0, 1.0, 1.0]
    }

    /// A copy with the filter colour replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_colour(&self, colour: [f32; 3]) -> Result<Self, PhotoFilterError> {
        Self::new(colour, self.density, self.preserve_luminosity)
    }

    /// A copy with the density replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_density(&self, density: f32) -> Result<Self, PhotoFilterError> {
        Self::new(self.colour, density, self.preserve_luminosity)
    }

    /// A copy with Preserve Luminosity switched.
    #[must_use]
    pub const fn with_preserve_luminosity(&self, preserve: bool) -> Self {
        Self {
            preserve_luminosity: preserve,
            ..*self
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{PHOTO_FILTER_PRESETS, PhotoFilterError as E, PhotoFilterParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_a_white_filter_at_any_density_and_flag() {
        assert!(P::identity().is_identity());
        assert!(ok(P::new([1.0, 1.0, 1.0], 100.0, false)).is_identity());
        assert!(ok(P::new([1.0, 1.0, 1.0], 1.0, true)).is_identity());
        assert!(!P::default().is_identity());
        assert!(!ok(P::new([1.0, 1.0, 0.99], 50.0, true)).is_identity());
        assert!(!ok(P::new([0.99, 1.0, 1.0], 50.0, true)).is_identity());
    }

    #[test]
    fn the_default_is_the_warming_85_filter_at_density_25() {
        let d = P::default();
        assert_eq!(d.density(), 25.0);
        assert!(d.preserve_luminosity());
        let [r, g, b] = d.colour();
        assert_eq!(
            (
                (r * 255.0).round(),
                (g * 255.0).round(),
                (b * 255.0).round()
            ),
            (236.0, 138.0, 0.0)
        );
        assert_eq!(
            PHOTO_FILTER_PRESETS.first().map(|p| p.1),
            Some([236, 138, 0])
        );
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new([0.0, 0.0, 0.0], 1.0, false).is_ok());
        assert!(P::new([1.0, 1.0, 1.0], 100.0, true).is_ok());
        assert_eq!(P::new([1.01, 0.0, 0.0], 50.0, true), Err(E::Colour));
        assert_eq!(P::new([0.0, -0.01, 0.0], 50.0, true), Err(E::Colour));
        assert_eq!(P::new([0.0, 0.0, 1.01], 50.0, true), Err(E::Colour));
        assert_eq!(P::new([0.5; 3], 0.99, true), Err(E::Density));
        assert_eq!(P::new([0.5; 3], 100.01, true), Err(E::Density));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(P::new([bad, 0.5, 0.5], 50.0, true), Err(E::NotFinite));
            assert_eq!(P::new([0.5, bad, 0.5], 50.0, true), Err(E::NotFinite));
            assert_eq!(P::new([0.5, 0.5, bad], 50.0, true), Err(E::NotFinite));
            assert_eq!(P::new([0.5; 3], bad, true), Err(E::NotFinite));
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new([0.25, 0.5, 0.75], 40.0, false));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(bytes.len(), 4 * 4 + 1, "four f32s and a bool");
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        let Ok(bad) = postcard::to_allocvec(&(2.0_f32, 0.0_f32, 0.0_f32, 50.0_f32, true)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&bad).is_err(), "decode validates");
        let Ok(nan) = postcard::to_allocvec(&(0.0_f32, 0.0_f32, 0.0_f32, f32::NAN, true)) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&nan).is_err());
        assert!(postcard::from_bytes::<P>(bytes.get(..16).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_keep_the_other_values_and_validate() {
        let p = ok(P::new([0.1, 0.2, 0.3], 30.0, true));
        assert_eq!(ok(p.with_colour([0.4, 0.5, 0.6])).colour(), [0.4, 0.5, 0.6]);
        assert_eq!(ok(p.with_colour([0.4, 0.5, 0.6])).density(), 30.0);
        assert_eq!(ok(p.with_density(31.0)).density(), 31.0);
        assert_eq!(ok(p.with_density(31.0)).colour(), [0.1, 0.2, 0.3]);
        assert!(!p.with_preserve_luminosity(false).preserve_luminosity());
        assert_eq!(p.with_preserve_luminosity(false).density(), 30.0);
        assert!(p.with_colour([2.0, 0.0, 0.0]).is_err());
        assert!(p.with_density(0.0).is_err());
    }
}
