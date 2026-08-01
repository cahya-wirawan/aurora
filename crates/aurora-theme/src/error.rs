//! Error types for `aurora-theme`.

/// Errors from parsing a palette, a theme, or resolving tokens between
/// them.
///
/// `#[non_exhaustive]`: more variants will be added as this crate grows
/// (hot reload, scales); downstream `match`es must already handle
/// "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThemeError {
    /// The TOML itself didn't parse.
    #[error("failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// A theme or palette reference (e.g. `"neutral.100"`) didn't lead to
    /// a value of the expected shape.
    #[error("{reference:?} does not resolve to a {expected}")]
    WrongType {
        reference: String,
        expected: &'static str,
    },
    /// A palette reference (`"<ramp>.<step>"`, ...) named a path that
    /// doesn't exist in the palette at all.
    #[error("{0:?} does not exist in the palette")]
    UnknownPaletteReference(String),
    /// A hex color string wasn't `#RRGGBB`.
    #[error("{0:?} is not a valid #RRGGBB color")]
    InvalidColor(String),
    /// A theme's flattened, inheritance-resolved token set was missing a
    /// key the vocabulary requires (`vocabulary.md`), after walking its
    /// whole `extends` chain.
    #[error("theme is missing required token {0:?} after resolving its whole extends chain")]
    MissingToken(String),
    /// A theme's `extends` named a parent that was never registered via
    /// [`crate::ThemeSet::register`].
    #[error("theme extends {0:?}, which is not a registered theme")]
    UnknownParent(String),
    /// A theme's `extends` chain refers back to itself.
    #[error("cyclic extends chain: {0:?} (eventually) extends itself")]
    CyclicExtends(String),
}
