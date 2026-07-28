//! Error types for `aurora-core`'s own fallible operations.

/// Errors from `aurora-core`'s validating constructors.
///
/// `#[non_exhaustive]`: more variants will be added as this crate grows;
/// downstream `match`es must already handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A `Size` was constructed past the ADR 0002 document ceiling.
    #[error("document dimensions {width}x{height} exceed the {max}px ceiling (ADR 0002)")]
    DocumentTooLarge { width: u32, height: u32, max: u32 },
    /// A `Size` was constructed with a zero width or height.
    #[error("document dimensions must be non-zero")]
    EmptyDocument,
}
