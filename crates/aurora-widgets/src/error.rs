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
    /// [`crate::FocusManager::focus`] was asked to focus a widget whose
    /// own `accesskit::Node` doesn't declare `Action::Focus` — reusing
    /// `accesskit`'s own vocabulary for "focusable" rather than a second,
    /// parallel flag.
    #[error("widget {0:?} does not support the accesskit Focus action")]
    NotFocusable(WidgetId),
    /// A concrete widget function (`widgets::toggle_checkbox`,
    /// `widgets::set_slider_value`, ...) was called on a real, existing
    /// widget that isn't the kind that function operates on.
    #[error("widget {0:?} exists but is not the expected widget kind")]
    WrongWidgetKind(WidgetId),
    /// A concrete widget function was asked to interact with (click,
    /// toggle, drag) a widget whose own state marks it disabled.
    #[error("widget {0:?} is disabled")]
    WidgetDisabled(WidgetId),
    /// A widget constructor or mutator was handed a numeric range that
    /// cannot describe a real position: a non-finite bound, or `min >
    /// max`. Returned rather than letting `f64::clamp`'s own internal
    /// `assert!(min <= max)` fire — that assertion lives in `core`, not
    /// in this workspace, so the root `Cargo.toml`'s `panic = "deny"`
    /// lint cannot see it, and a panic in a crate holding a
    /// professional's unsaved work is exactly what that lint exists to
    /// prevent.
    #[error("invalid numeric range: min={min}, max={max} (both must be finite, with min <= max)")]
    InvalidRange { min: f64, max: f64 },
    /// [`crate::paint_widget`] failed to tessellate a widget's own paint
    /// geometry into a `Mesh`. In practice unreachable from this crate's
    /// own callers today: `rounded_rect`'s own cubic-Bézier output never
    /// triggers a `lyon` tessellation failure in this module's tests.
    #[error("failed to tessellate a widget's own paint geometry: {0}")]
    Paint(#[source] aurora_vector::VectorError),
}
