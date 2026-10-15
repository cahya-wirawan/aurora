//! Filter and adjustment node implementations.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! As of 0.155.0 this crate holds one real adjustment, Curves
//! ([`curves`]); Levels ([`levels`]) joined in 0.202.0 and Hue/Saturation
//! ([`hue_saturation`]) in 0.203.0; Vibrance ([`vibrance`]), Exposure
//! ([`exposure`]) and Photo Filter ([`photo_filter`]) joined in 0.239.0
//! (CPU only); every other filter and adjustment is still to come.

pub mod curves;
pub mod exposure;
pub mod hue_saturation;
pub mod levels;
pub mod photo_filter;
pub mod shadow;
pub mod stroke;
pub mod vibrance;

pub use curves::{CURVES_LUT_INTERVALS, CURVES_PACKED_HEADER, CURVES_PACKED_LEN, CurvesLut};
pub use exposure::ExposureOp;
pub use hue_saturation::{HUE_SATURATION_PACKED_LEN, HueSaturationOp, range_weight};
pub use levels::{EXACT_INTERVALS, LEVELS_LUT_INTERVALS, LEVELS_PACKED_LEN, LevelsLut};
pub use photo_filter::PhotoFilterOp;
pub use shadow::shadow_coverage;
pub use stroke::{StrokeSide, stroke_coverage, stroke_reach};
pub use vibrance::VibranceOp;

/// The crate's own name, kept from the skeleton so CI's crate-name check
/// still has something to check.
#[must_use]
pub const fn crate_name() -> &'static str {
    "aurora-filters"
}

#[cfg(test)]
mod tests {
    use super::crate_name;

    #[test]
    fn reports_its_name() {
        assert_eq!(crate_name(), "aurora-filters");
    }
}
