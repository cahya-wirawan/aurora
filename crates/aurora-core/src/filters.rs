//! Editable layer filters (task 176): the data behind Photoshop's "Smart
//! Filters" idea. A filter is *not* baked into a layer's pixels (invariant
//! §7.3.2); it sits on the layer as an ordered list, is computed when the
//! layer is drawn, and can be edited, hidden or removed at any time.
//!
//! This module is data and the numbers every consumer must agree on: the
//! validated list ([`LayerFilters`]), one entry ([`LayerFilter`] with a
//! [`FilterKind`]) and, for the Gaussian blur, the exact kernel it is drawn
//! with ([`GaussianSpec`]) and how far it reaches ([`FilterKind::reach`]).
//! The pixel maths lives in `aurora-filters`; the document wiring in
//! `aurora-doc`.
//!
//! **Wire format.** `FilterKind` is a positional `postcard` enum: a new
//! kind (the Unsharp Mask of task 177) is appended after the last, never
//! inserted, and a change to a variant's fields needs a new variant.
//! `LayerFilter` and `LayerFilters` are positional structs on the wire in
//! `.aur`'s optional `layer-filters` entry and in `LayerOp::SetFilters`.

/// The most filters one layer can carry.
pub const MAX_LAYER_FILTERS: usize = 8;

/// The largest Gaussian blur radius, in pixels. The radius is the standard
/// deviation of the Gaussian, so the blur reaches three times that far;
/// 85 px keeps one filter's reach within a single tile (256 px), which is
/// what bounds the work of drawing it (the layer's neighbouring tiles are
/// read, never the whole layer). Photoshop allows up to 1000 px.
pub const MAX_GAUSSIAN_RADIUS: f32 = 85.0;

/// The smallest Gaussian blur radius, in pixels (Photoshop's is 0.1).
pub const MIN_GAUSSIAN_RADIUS: f32 = 0.1;

/// The smallest Unsharp Mask amount, in percent (Photoshop's is 1).
pub const MIN_UNSHARP_AMOUNT: f32 = 1.0;

/// The largest Unsharp Mask amount, in percent (Photoshop's is 500).
pub const MAX_UNSHARP_AMOUNT: f32 = 500.0;

/// The largest Reduce Noise strength (Photoshop's is 10).
pub const MAX_NOISE_STRENGTH: f32 = 10.0;

/// The largest Reduce Noise percentage (Preserve Details, Reduce Color
/// Noise, Sharpen Details; Photoshop's are 0..=100 %).
pub const MAX_NOISE_PERCENT: f32 = 100.0;

/// The standard deviation, in pixels, of the chroma blur at Reduce Color
/// Noise 100 % (Aurora's own choice; exact-kernel range, reach 12 px).
pub const REDUCE_NOISE_MAX_CHROMA_SIGMA: f32 = 4.0;

/// The standard deviation, in pixels, of the blur Sharpen Details
/// sharpens against (Aurora's own choice; reach 3 px).
pub const REDUCE_NOISE_SHARPEN_SIGMA: f32 = 1.0;

/// The largest half-width, in pixels, of Reduce Noise's bilateral window.
pub const REDUCE_NOISE_MAX_RADIUS: u32 = 5;

/// The largest Dust & Scratches radius, in pixels (Photoshop's is 100; a
/// median over a disc costs about `pi r^2` samples a pixel: one 256 px tile
/// measures 11 ms at radius 1, 110 at 5 and 340 at 10 (dev profile), so Aurora
/// stops where one tile still draws in a fraction of a second).
pub const MAX_DUST_RADIUS: u32 = 10;

/// The smallest Dust & Scratches radius, in pixels (Photoshop's is 1).
pub const MIN_DUST_RADIUS: u32 = 1;

/// The most pixels all of one layer's shown filters may reach together
/// (hidden filters are not counted; showing one is checked again).
/// One tile's side: the window drawn for a tile is then at most 3 x 3
/// tiles, however the filters are chained.
pub const MAX_FILTER_REACH: u32 = 256;

/// Radii up to this use the exact truncated Gaussian; larger ones three
/// box blurs (see [`GaussianSpec`]).
const EXACT_MAX_SIGMA: f32 = 4.0;

/// Fixed-point scale of kernel weights: a kernel's weights sum to exactly
/// this.
pub const KERNEL_ONE: i64 = 1 << 16;

/// Why a [`LayerFilters`] value is refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FiltersError {
    #[error("{count} filters on one layer; at most {MAX_LAYER_FILTERS} are allowed")]
    TooMany { count: usize },
    #[error(
        "Unsharp Mask amount is not a number from {MIN_UNSHARP_AMOUNT} to {MAX_UNSHARP_AMOUNT} percent"
    )]
    AmountOutOfRange,
    #[error(
        "filter radius is not a number from {MIN_GAUSSIAN_RADIUS} to {MAX_GAUSSIAN_RADIUS} pixels"
    )]
    RadiusOutOfRange,
    #[error("Reduce Noise strength is not a number from 0 to {MAX_NOISE_STRENGTH}")]
    NoiseStrengthOutOfRange,
    #[error(
        "Reduce Noise Preserve Details, Reduce Color Noise and Sharpen Details are not numbers from 0 to {MAX_NOISE_PERCENT} percent"
    )]
    NoisePercentOutOfRange,
    #[error(
        "Dust & Scratches radius is not a whole number from {MIN_DUST_RADIUS} to {MAX_DUST_RADIUS} pixels"
    )]
    DustRadiusOutOfRange,
    #[error(
        "the layer's shown filters together reach {reach} pixels; at most {MAX_FILTER_REACH} are allowed"
    )]
    ReachTooLarge { reach: u32 },
}

/// One kind of filter and its settings. Appended-only, like every
/// `postcard` enum.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FilterKind {
    /// A Gaussian blur whose `radius` (pixels) is the Gaussian's standard
    /// deviation.
    GaussianBlur { radius: f32 },
    /// An Unsharp Mask (task 177), appended after the blur. `amount` is
    /// the percentage of the difference between a pixel's colour and its
    /// Gaussian-blurred colour that is added back (1..=500), `radius` the
    /// blur's standard deviation in pixels (the blur's own range) and
    /// `threshold` the smallest per-channel difference, in 8-bit levels
    /// (0..=255), that is sharpened; a smaller difference is left alone.
    UnsharpMask {
        amount: f32,
        radius: f32,
        threshold: u8,
    },
    /// Reduce Noise (task 178), appended after the Unsharp Mask. All four
    /// are Photoshop's controls with Aurora's own maths (see
    /// `aurora_filters::reduce_noise`): `strength` 0..=10 (the luminance
    /// smoothing; 0 = none), `preserve_details` 0..=100 % (how much edge
    /// contrast survives the smoothing), `color_noise` 0..=100 % (blur of
    /// the colour-only part of the picture; 0 = none) and `sharpen_details`
    /// 0..=100 % (a final unsharp boost; 0 = none). All four at 0 is the
    /// identity.
    ReduceNoise {
        strength: f32,
        preserve_details: f32,
        color_noise: f32,
        sharpen_details: f32,
    },
    /// Dust & Scratches (task 179), appended after Reduce Noise: a median
    /// filter over the disc of `radius` pixels (1..=10) that replaces a
    /// channel only where it differs from the median by more than
    /// `threshold` 8-bit levels (0..=255; 0 replaces every difference).
    /// See `aurora_filters::dust_scratches`.
    DustAndScratches { radius: u32, threshold: u8 },
}

/// The half-width of Reduce Noise's bilateral window for `strength`:
/// `0` for none (strength 0 or not a number), else `ceil(strength / 2)`,
/// 1..=5.
#[must_use]
pub fn reduce_noise_radius(strength: f32) -> u32 {
    if !(strength.is_finite() && strength > 0.0) {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1..=5 after the clamp
    let r = (strength.min(MAX_NOISE_STRENGTH) / 2.0).ceil() as u32;
    r.clamp(1, REDUCE_NOISE_MAX_RADIUS)
}

/// The chroma blur's spec for `color_noise` percent, or `None` for none
/// (0 % or not a number). Sigma is `color_noise / 100 * 4` px, at least
/// the blur's 0.1 px minimum.
#[must_use]
pub fn reduce_noise_chroma_spec(color_noise: f32) -> Option<GaussianSpec> {
    if !(color_noise.is_finite() && color_noise > 0.0) {
        return None;
    }
    let sigma = (color_noise.min(MAX_NOISE_PERCENT) / MAX_NOISE_PERCENT
        * REDUCE_NOISE_MAX_CHROMA_SIGMA)
        .max(MIN_GAUSSIAN_RADIUS);
    GaussianSpec::new(sigma)
}

/// The sharpen stage's blur spec, or `None` when `sharpen_details` is 0
/// (or not a number).
#[must_use]
pub fn reduce_noise_sharpen_spec(sharpen_details: f32) -> Option<GaussianSpec> {
    if !(sharpen_details.is_finite() && sharpen_details > 0.0) {
        return None;
    }
    GaussianSpec::new(REDUCE_NOISE_SHARPEN_SIGMA)
}

impl FilterKind {
    /// A short name for menus, history rows and accessibility.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::GaussianBlur { .. } => "Gaussian Blur",
            Self::UnsharpMask { .. } => "Unsharp Mask",
            Self::ReduceNoise { .. } => "Reduce Noise",
            Self::DustAndScratches { .. } => "Dust & Scratches",
        }
    }

    /// Whether the settings are in range.
    ///
    /// # Errors
    ///
    /// [`FiltersError::RadiusOutOfRange`] for a non-finite or out-of-range
    /// radius, [`FiltersError::AmountOutOfRange`] for an Unsharp Mask
    /// amount.
    pub fn validate(&self) -> Result<(), FiltersError> {
        match self {
            Self::GaussianBlur { radius } => {
                if radius.is_finite()
                    && (MIN_GAUSSIAN_RADIUS..=MAX_GAUSSIAN_RADIUS).contains(radius)
                {
                    Ok(())
                } else {
                    Err(FiltersError::RadiusOutOfRange)
                }
            }
            Self::UnsharpMask { amount, radius, .. } => {
                if !(amount.is_finite()
                    && (MIN_UNSHARP_AMOUNT..=MAX_UNSHARP_AMOUNT).contains(amount))
                {
                    Err(FiltersError::AmountOutOfRange)
                } else if radius.is_finite()
                    && (MIN_GAUSSIAN_RADIUS..=MAX_GAUSSIAN_RADIUS).contains(radius)
                {
                    Ok(())
                } else {
                    Err(FiltersError::RadiusOutOfRange)
                }
            }
            Self::ReduceNoise {
                strength,
                preserve_details,
                color_noise,
                sharpen_details,
            } => {
                if !(strength.is_finite() && (0.0..=MAX_NOISE_STRENGTH).contains(strength)) {
                    return Err(FiltersError::NoiseStrengthOutOfRange);
                }
                if [preserve_details, color_noise, sharpen_details]
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=MAX_NOISE_PERCENT).contains(*v))
                {
                    Ok(())
                } else {
                    Err(FiltersError::NoisePercentOutOfRange)
                }
            }
            Self::DustAndScratches { radius, .. } => {
                if (MIN_DUST_RADIUS..=MAX_DUST_RADIUS).contains(radius) {
                    Ok(())
                } else {
                    Err(FiltersError::DustRadiusOutOfRange)
                }
            }
        }
    }

    /// How many pixels beyond a pixel this filter reads: the output at `p`
    /// depends only on input within this distance. `0` for an invalid
    /// setting (nothing valid ever asks).
    #[must_use]
    pub fn reach(&self) -> u32 {
        match self {
            // The unsharp mask reads exactly the blur it subtracts.
            Self::GaussianBlur { radius } | Self::UnsharpMask { radius, .. } => {
                GaussianSpec::new(*radius).map_or(0, |s| s.reach())
            }
            // The three stages run one after the other, each reading the
            // one before: their reaches add. A stage that is off (its
            // setting 0) reads nothing.
            Self::ReduceNoise {
                strength,
                color_noise,
                sharpen_details,
                ..
            } => reduce_noise_radius(*strength)
                .saturating_add(reduce_noise_chroma_spec(*color_noise).map_or(0, |s| s.reach()))
                .saturating_add(
                    reduce_noise_sharpen_spec(*sharpen_details).map_or(0, |s| s.reach()),
                ),
            // The disc of `radius` pixels fits in the square of that
            // half-width.
            Self::DustAndScratches { radius, .. } => {
                if (MIN_DUST_RADIUS..=MAX_DUST_RADIUS).contains(radius) {
                    *radius
                } else {
                    0
                }
            }
        }
    }
}

/// One filter of a layer's list.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerFilter {
    pub kind: FilterKind,
    /// Whether the filter is drawn (the eye); a hidden filter keeps its
    /// settings.
    pub visible: bool,
}

impl LayerFilter {
    /// A visible filter.
    #[must_use]
    pub fn new(kind: FilterKind) -> Self {
        Self {
            kind,
            visible: true,
        }
    }
}

/// A pixel layer's ordered filters, bottom to top: the first filter reads
/// the layer's stored pixels and each later one the result of the one
/// before. Never empty when attached (an empty list is `None` on the
/// layer).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct LayerFilters {
    pub filters: Vec<LayerFilter>,
}

impl LayerFilters {
    /// Checks the count, every setting and the total reach of the shown
    /// filters.
    ///
    /// # Errors
    ///
    /// The first [`FiltersError`] found.
    pub fn validate(&self) -> Result<(), FiltersError> {
        if self.filters.len() > MAX_LAYER_FILTERS {
            return Err(FiltersError::TooMany {
                count: self.filters.len(),
            });
        }
        for filter in &self.filters {
            filter.kind.validate()?;
        }
        // Only the shown filters are drawn, and every draw, cache key and
        // invalidation reads the shown reach, so only it is bounded.
        // Showing a hidden filter is validated again at that moment.
        let reach = self.visible_reach();
        if reach > MAX_FILTER_REACH {
            return Err(FiltersError::ReachTooLarge { reach });
        }
        Ok(())
    }

    /// The filters that are drawn, in order.
    pub fn visible(&self) -> impl Iterator<Item = &LayerFilter> {
        self.filters.iter().filter(|f| f.visible)
    }

    /// Whether any filter is drawn.
    #[must_use]
    pub fn any_visible(&self) -> bool {
        self.visible().next().is_some()
    }

    /// The pixels the *visible* filters together reach: the apron of
    /// neighbouring pixels needed to draw a tile.
    #[must_use]
    pub fn visible_reach(&self) -> u32 {
        self.visible()
            .map(|f| f.kind.reach())
            .fold(0u32, u32::saturating_add)
    }
}

/// The exact arithmetic of a Gaussian blur of one radius, integers only so
/// the result at a pixel does not depend on which window it is computed in
/// (tiles are seamless bit for bit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GaussianSpec {
    /// Radii up to 4 px: the Gaussian `exp(-x^2 / 2 sigma^2)` truncated at
    /// `ceil(3 sigma)` and normalised, `weights[i]` the weight of offset
    /// `i - weights.len() / 2` in units of `1 / KERNEL_ONE`, summing to
    /// exactly [`KERNEL_ONE`].
    Exact { weights: Vec<i64> },
    /// Larger radii: three successive box blurs (Kovesi's ideal widths),
    /// each of odd width `2 r + 1`, the standard fast approximation of a
    /// Gaussian (its profile is within 6 % of the Gaussian's peak value).
    Boxes { radii: [u32; 3] },
}

impl GaussianSpec {
    /// The spec for a standard deviation `sigma` in pixels, or `None` if
    /// it is not a finite number in range.
    #[must_use]
    pub fn new(sigma: f32) -> Option<Self> {
        if !(sigma.is_finite() && (MIN_GAUSSIAN_RADIUS..=MAX_GAUSSIAN_RADIUS).contains(&sigma)) {
            return None;
        }
        let sigma = f64::from(sigma);
        if sigma <= f64::from(EXACT_MAX_SIGMA) {
            Some(Self::exact(sigma))
        } else {
            Some(Self::boxes(sigma))
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn exact(sigma: f64) -> Self {
        let r = (3.0 * sigma).ceil().max(1.0) as i64;
        let raw: Vec<f64> = (-r..=r)
            .map(|x| (-((x * x) as f64) / (2.0 * sigma * sigma)).exp())
            .collect();
        let total: f64 = raw.iter().sum();
        let mut weights: Vec<i64> = raw
            .iter()
            .map(|w| (w / total * KERNEL_ONE as f64).round() as i64)
            .collect();
        // Rounding leaves the sum a few units off; the centre absorbs it so
        // a flat area stays exactly flat.
        let sum: i64 = weights.iter().sum();
        if let Some(centre) = weights.get_mut(r as usize) {
            *centre += KERNEL_ONE - sum;
        }
        Self::Exact { weights }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn boxes(sigma: f64) -> Self {
        let n = 3.0_f64;
        let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
        let mut lower = ideal.floor() as i64;
        if lower % 2 == 0 {
            lower -= 1;
        }
        let lower = lower.max(1);
        let upper = lower + 2;
        let l = lower as f64;
        let m = ((12.0 * sigma * sigma - n * l * l - 4.0 * n * l - 3.0 * n) / (-4.0 * l - 4.0))
            .round()
            .clamp(0.0, n) as usize;
        let mut radii = [0u32; 3];
        for (i, r) in radii.iter_mut().enumerate() {
            let width = if i < m { lower } else { upper };
            *r = u32::try_from((width - 1) / 2).unwrap_or(0);
        }
        Self::Boxes { radii }
    }

    /// How far the blur reads from a pixel, in pixels.
    #[must_use]
    pub fn reach(&self) -> u32 {
        match self {
            Self::Exact { weights } => u32::try_from(weights.len() / 2).unwrap_or(u32::MAX),
            Self::Boxes { radii } => radii.iter().copied().fold(0, u32::saturating_add),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_weights_sum_to_one_and_are_symmetric() {
        for sigma in [0.1_f32, 0.5, 1.0, 2.5, 4.0] {
            let Some(GaussianSpec::Exact { weights }) = GaussianSpec::new(sigma) else {
                unreachable!("exact for {sigma}");
            };
            assert_eq!(weights.iter().sum::<i64>(), KERNEL_ONE);
            assert_eq!(weights.len() % 2, 1);
            // Symmetric except for the centre's rounding absorption.
            let n = weights.len();
            for i in 0..n / 2 {
                assert_eq!(weights.get(i), weights.get(n - 1 - i), "sigma {sigma}");
            }
        }
    }

    #[test]
    fn large_radii_use_boxes_within_one_tile() {
        let Some(GaussianSpec::Boxes { radii }) = GaussianSpec::new(MAX_GAUSSIAN_RADIUS) else {
            unreachable!("boxes");
        };
        let reach: u32 = radii.iter().sum();
        assert!(reach < MAX_FILTER_REACH, "{reach}");
        // Box variance approximates sigma^2.
        let var: f64 = radii
            .iter()
            .map(|r| {
                let w = f64::from(2 * r + 1);
                (w * w - 1.0) / 12.0
            })
            .sum();
        let sigma = var.sqrt();
        assert!((sigma - 85.0).abs() < 1.5, "{sigma}");
    }

    #[test]
    fn spec_refuses_out_of_range() {
        assert!(GaussianSpec::new(f32::NAN).is_none());
        assert!(GaussianSpec::new(0.0).is_none());
        assert!(GaussianSpec::new(85.1).is_none());
        assert!(GaussianSpec::new(-1.0).is_none());
    }

    #[test]
    fn validate_checks_count_range_and_reach() {
        let blur = |radius| LayerFilter::new(FilterKind::GaussianBlur { radius });
        assert!(
            LayerFilters {
                filters: vec![blur(10.0)]
            }
            .validate()
            .is_ok()
        );
        assert_eq!(
            LayerFilters {
                filters: vec![blur(f32::NAN)]
            }
            .validate(),
            Err(FiltersError::RadiusOutOfRange)
        );
        assert!(matches!(
            LayerFilters {
                filters: vec![blur(2.0); MAX_LAYER_FILTERS + 1]
            }
            .validate(),
            Err(FiltersError::TooMany { .. })
        ));
        assert!(matches!(
            LayerFilters {
                filters: vec![blur(85.0), blur(85.0)]
            }
            .validate(),
            Err(FiltersError::ReachTooLarge { .. })
        ));
        // 85 alone is within one tile.
        assert!(
            LayerFilters {
                filters: vec![blur(85.0)]
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn only_the_shown_filters_count_toward_the_reach_cap() {
        let mut f = LayerFilters {
            filters: vec![
                LayerFilter::new(FilterKind::GaussianBlur { radius: 50.0 }),
                LayerFilter::new(FilterKind::GaussianBlur { radius: 50.0 }),
            ],
        };
        assert!(f.validate().is_err(), "both shown reach too far");
        if let Some(f1) = f.filters.get_mut(1) {
            f1.visible = false;
        }
        // One shown, one hidden: allowed, and the hidden one is not drawn.
        assert!(f.validate().is_ok());
        let one = LayerFilters {
            filters: vec![LayerFilter::new(FilterKind::GaussianBlur { radius: 50.0 })],
        };
        assert_eq!(f.visible_reach(), one.visible_reach());
        assert!(f.any_visible());
        // Showing the hidden one again is refused by the same check.
        if let Some(f1) = f.filters.get_mut(1) {
            f1.visible = true;
        }
        assert!(matches!(
            f.validate(),
            Err(FiltersError::ReachTooLarge { .. })
        ));
        for filter in &mut f.filters {
            filter.visible = false;
        }
        assert!(!f.any_visible());
        assert_eq!(f.visible_reach(), 0);
        assert!(f.validate().is_ok());
    }

    #[test]
    fn unsharp_mask_validates_reaches_like_the_blur_and_is_appended() {
        let unsharp = |amount, radius| FilterKind::UnsharpMask {
            amount,
            radius,
            threshold: 3,
        };
        assert_eq!(unsharp(150.0, 2.0).validate(), Ok(()));
        assert_eq!(unsharp(1.0, 0.1).validate(), Ok(()));
        assert_eq!(unsharp(500.0, 85.0).validate(), Ok(()));
        for amount in [0.9, 500.1, f32::NAN, f32::INFINITY, -5.0] {
            assert_eq!(
                unsharp(amount, 2.0).validate(),
                Err(FiltersError::AmountOutOfRange),
                "{amount}"
            );
        }
        for radius in [0.09, 85.1, f32::NAN] {
            assert_eq!(
                unsharp(150.0, radius).validate(),
                Err(FiltersError::RadiusOutOfRange),
                "{radius}"
            );
        }
        assert_eq!(
            unsharp(100.0, 7.0).reach(),
            FilterKind::GaussianBlur { radius: 7.0 }.reach()
        );
        assert_eq!(unsharp(100.0, 7.0).name(), "Unsharp Mask");
        // It counts toward the shared reach cap like a blur.
        let list = LayerFilters {
            filters: vec![
                LayerFilter::new(unsharp(100.0, 85.0)),
                LayerFilter::new(FilterKind::GaussianBlur { radius: 85.0 }),
            ],
        };
        assert!(matches!(
            list.validate(),
            Err(FiltersError::ReachTooLarge { .. })
        ));
    }

    /// The wire format: the blur keeps variant 0 and its bytes; the Unsharp
    /// Mask is variant 1. A build that knows only the blur (mirrored here)
    /// refuses the new variant instead of misreading it.
    #[test]
    fn wire_format_blur_bytes_unchanged_and_old_readers_refuse_unsharp() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        enum Old {
            GaussianBlur { radius: f32 },
        }
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct OldFilter {
            kind: Old,
            visible: bool,
        }
        let blur = LayerFilter::new(FilterKind::GaussianBlur { radius: 2.5 });
        let bytes = postcard::to_allocvec(&blur).unwrap_or_default();
        // variant 0, f32 2.5 little-endian, visible = true.
        assert_eq!(bytes, [0, 0, 0, 0x20, 0x40, 1]);
        assert_eq!(
            postcard::from_bytes::<OldFilter>(&bytes).ok(),
            Some(OldFilter {
                kind: Old::GaussianBlur { radius: 2.5 },
                visible: true
            })
        );
        let unsharp = LayerFilter::new(FilterKind::UnsharpMask {
            amount: 150.0,
            radius: 2.0,
            threshold: 4,
        });
        let bytes = postcard::to_allocvec(&unsharp).unwrap_or_default();
        assert_eq!(bytes.first(), Some(&1), "variant 1");
        assert!(postcard::from_bytes::<OldFilter>(&bytes).is_err());
        assert_eq!(
            postcard::from_bytes::<LayerFilter>(&bytes).ok(),
            Some(unsharp)
        );
    }

    fn noise(strength: f32, preserve: f32, colour: f32, sharpen: f32) -> FilterKind {
        FilterKind::ReduceNoise {
            strength,
            preserve_details: preserve,
            color_noise: colour,
            sharpen_details: sharpen,
        }
    }

    #[test]
    fn reduce_noise_validates_names_and_reaches_by_stage() {
        assert_eq!(noise(0.0, 0.0, 0.0, 0.0).validate(), Ok(()));
        assert_eq!(noise(10.0, 100.0, 100.0, 100.0).validate(), Ok(()));
        for bad in [-0.1, 10.1, f32::NAN, f32::INFINITY] {
            assert_eq!(
                noise(bad, 50.0, 50.0, 50.0).validate(),
                Err(FiltersError::NoiseStrengthOutOfRange),
                "{bad}"
            );
        }
        for bad in [-1.0, 100.5, f32::NAN, f32::NEG_INFINITY] {
            for slot in 0..3 {
                let mut v = [50.0_f32; 3];
                if let Some(v) = v.get_mut(slot) {
                    *v = bad;
                }
                assert_eq!(
                    noise(
                        5.0,
                        v.first().copied().unwrap_or(0.0),
                        v.get(1).copied().unwrap_or(0.0),
                        v.get(2).copied().unwrap_or(0.0)
                    )
                    .validate(),
                    Err(FiltersError::NoisePercentOutOfRange),
                    "{bad} slot {slot}"
                );
            }
        }
        assert_eq!(noise(1.0, 0.0, 0.0, 0.0).name(), "Reduce Noise");
        // Stage reaches: off stages read nothing, the rest add.
        assert_eq!(noise(0.0, 80.0, 0.0, 0.0).reach(), 0);
        assert_eq!(reduce_noise_radius(0.01), 1);
        assert_eq!(reduce_noise_radius(2.0), 1);
        assert_eq!(reduce_noise_radius(2.1), 2);
        assert_eq!(reduce_noise_radius(10.0), 5);
        assert_eq!(noise(10.0, 0.0, 0.0, 0.0).reach(), 5);
        assert_eq!(noise(0.0, 0.0, 100.0, 0.0).reach(), 12);
        assert_eq!(noise(0.0, 0.0, 0.0, 1.0).reach(), 3);
        assert_eq!(noise(10.0, 0.0, 100.0, 100.0).reach(), 5 + 12 + 3);
        // It shares the cap with the other kinds.
        let list = LayerFilters {
            filters: vec![
                LayerFilter::new(noise(10.0, 0.0, 100.0, 100.0)),
                LayerFilter::new(FilterKind::GaussianBlur { radius: 85.0 }),
            ],
        };
        assert!(matches!(
            list.validate(),
            Err(FiltersError::ReachTooLarge { .. })
        ));
    }

    /// Task 178: variant 2; a build that knows only the first two kinds
    /// (the 0.310.0 shape, mirrored here) refuses it.
    #[test]
    fn wire_format_reduce_noise_is_variant_two_and_0_310_readers_refuse_it() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        enum Old {
            GaussianBlur {
                radius: f32,
            },
            UnsharpMask {
                amount: f32,
                radius: f32,
                threshold: u8,
            },
        }
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct OldFilter {
            kind: Old,
            visible: bool,
        }
        let unsharp = LayerFilter::new(FilterKind::UnsharpMask {
            amount: 150.0,
            radius: 2.0,
            threshold: 4,
        });
        let bytes = postcard::to_allocvec(&unsharp).unwrap_or_default();
        assert!(postcard::from_bytes::<OldFilter>(&bytes).is_ok(), "control");
        let reduce = LayerFilter::new(noise(4.0, 60.0, 30.0, 20.0));
        let bytes = postcard::to_allocvec(&reduce).unwrap_or_default();
        assert_eq!(bytes.first(), Some(&2), "variant 2");
        assert_eq!(bytes.len(), 1 + 16 + 1);
        assert!(postcard::from_bytes::<OldFilter>(&bytes).is_err());
        assert_eq!(
            postcard::from_bytes::<LayerFilter>(&bytes).ok(),
            Some(reduce)
        );
    }

    #[test]
    fn dust_and_scratches_validates_names_and_reaches_its_radius() {
        let dust = |radius| FilterKind::DustAndScratches {
            radius,
            threshold: 0,
        };
        assert_eq!(dust(1).validate(), Ok(()));
        assert_eq!(dust(MAX_DUST_RADIUS).validate(), Ok(()));
        for bad in [0, MAX_DUST_RADIUS + 1, u32::MAX] {
            assert_eq!(
                dust(bad).validate(),
                Err(FiltersError::DustRadiusOutOfRange)
            );
            assert_eq!(dust(bad).reach(), 0);
        }
        assert_eq!(dust(1).name(), "Dust & Scratches");
        assert_eq!(dust(7).reach(), 7);
        // It shares the cap with the other kinds.
        let list = LayerFilters {
            filters: vec![
                LayerFilter::new(dust(MAX_DUST_RADIUS)),
                LayerFilter::new(FilterKind::GaussianBlur { radius: 85.0 }),
                LayerFilter::new(FilterKind::GaussianBlur { radius: 85.0 }),
            ],
        };
        assert!(matches!(
            list.validate(),
            Err(FiltersError::ReachTooLarge { .. })
        ));
    }

    #[test]
    fn wire_format_dust_and_scratches_is_variant_three_and_0_311_readers_refuse_it() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        enum Old {
            GaussianBlur {
                radius: f32,
            },
            UnsharpMask {
                amount: f32,
                radius: f32,
                threshold: u8,
            },
            ReduceNoise {
                strength: f32,
                preserve_details: f32,
                color_noise: f32,
                sharpen_details: f32,
            },
        }
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct OldFilter {
            kind: Old,
            visible: bool,
        }
        let reduce = LayerFilter::new(noise(4.0, 60.0, 30.0, 20.0));
        let bytes = postcard::to_allocvec(&reduce).unwrap_or_default();
        assert!(postcard::from_bytes::<OldFilter>(&bytes).is_ok(), "control");
        let dust = LayerFilter::new(FilterKind::DustAndScratches {
            radius: 3,
            threshold: 12,
        });
        let bytes = postcard::to_allocvec(&dust).unwrap_or_default();
        assert_eq!(
            bytes,
            vec![3, 3, 12, 1],
            "variant 3, radius, threshold, visible"
        );
        assert!(postcard::from_bytes::<OldFilter>(&bytes).is_err());
        assert_eq!(postcard::from_bytes::<LayerFilter>(&bytes).ok(), Some(dust));
    }

    #[test]
    fn postcard_round_trip() {
        let f = LayerFilters {
            filters: vec![LayerFilter {
                kind: FilterKind::GaussianBlur { radius: 3.5 },
                visible: false,
            }],
        };
        let bytes = postcard::to_allocvec(&f).unwrap_or_default();
        assert_eq!(postcard::from_bytes::<LayerFilters>(&bytes).ok(), Some(f));
    }
}
