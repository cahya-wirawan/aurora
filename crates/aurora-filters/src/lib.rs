//! Filter and adjustment node implementations.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! As of 0.155.0 this crate holds one real adjustment, Curves
//! ([`curves`]); every other filter and adjustment is still to come.

pub mod curves;

pub use curves::{CURVES_LUT_INTERVALS, CurvesLut};

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
