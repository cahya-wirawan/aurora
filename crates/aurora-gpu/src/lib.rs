//! wgpu device management, shader library, and GPU tile residency.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! This crate is a skeleton: no functionality is implemented yet.

/// Placeholder so the crate compiles and CI has something to check.
///
/// Remove once real types land.
#[must_use]
pub const fn crate_name() -> &'static str {
    "aurora-gpu"
}

#[cfg(test)]
mod tests {
    use super::crate_name;

    #[test]
    fn reports_its_name() {
        assert_eq!(crate_name(), "aurora-gpu");
    }
}
