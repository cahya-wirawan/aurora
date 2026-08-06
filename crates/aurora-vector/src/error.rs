//! Errors from `aurora-vector`.

/// `#[non_exhaustive]`: more variants land as this crate grows past
/// fill/stroke tessellation (boolean operations, text-on-path).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VectorError {
    /// [`crate::fill`]/[`crate::stroke`] failed inside `lyon`'s own
    /// tessellator — not expected for a well-formed path built via
    /// [`crate::PathBuilder`] (every real UI shape this crate builds
    /// today is a closed, non-self-intersecting outline), but a real,
    /// checked possibility rather than an assumption.
    #[error("tessellation failed: {0}")]
    Tessellation(#[from] lyon::tessellation::TessellationError),
}
