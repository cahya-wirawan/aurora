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
        "Gaussian blur radius is not a number from {MIN_GAUSSIAN_RADIUS} to {MAX_GAUSSIAN_RADIUS} pixels"
    )]
    RadiusOutOfRange,
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
}

impl FilterKind {
    /// A short name for menus, history rows and accessibility.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::GaussianBlur { .. } => "Gaussian Blur",
        }
    }

    /// Whether the settings are in range.
    ///
    /// # Errors
    ///
    /// [`FiltersError::RadiusOutOfRange`] for a non-finite or out-of-range
    /// radius.
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
        }
    }

    /// How many pixels beyond a pixel this filter reads: the output at `p`
    /// depends only on input within this distance. `0` for an invalid
    /// setting (nothing valid ever asks).
    #[must_use]
    pub fn reach(&self) -> u32 {
        match self {
            Self::GaussianBlur { radius } => GaussianSpec::new(*radius).map_or(0, |s| s.reach()),
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
