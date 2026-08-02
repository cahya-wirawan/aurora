//! Error types for `aurora-widgets`.

use crate::WidgetId;

/// Errors from building or editing a [`crate::WidgetTree`].
///
/// `#[non_exhaustive]`: more variants will be added as this crate grows
/// (focus/input routing, layout); downstream `match`es must already
/// handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WidgetError {
    /// A [`WidgetId`] passed to a `WidgetTree` method doesn't exist in
    /// that tree.
    #[error("widget {0:?} does not exist")]
    UnknownWidget(WidgetId),
    /// [`crate::WidgetTree::remove`] was called on a tree's own root —
    /// a tree always has exactly one root (unlike
    /// `aurora_doc::LayerTree`'s multiple top-level layers), so removing
    /// it would leave the tree without one rather than a smaller tree.
    #[error("cannot remove {0:?}: it is this tree's root")]
    CannotRemoveRoot(WidgetId),
}
