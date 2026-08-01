//! Error types for `aurora-doc`.

use crate::layer::LayerId;

/// Errors from building or editing a [`crate::LayerTree`].
///
/// `#[non_exhaustive]`: more variants will be added as this crate grows
/// (masks, selections, history); downstream `match`es must already
/// handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DocError {
    /// A [`LayerId`] passed to a `LayerTree` method doesn't exist in
    /// that tree — either it's from a different tree, or nothing created
    /// it yet.
    #[error("layer {0:?} does not exist")]
    UnknownLayer(LayerId),
    /// A [`LayerId`] was used where a group was required (e.g. as the
    /// `parent` of a new layer, or the target of a reparent), but it
    /// names a pixel layer instead.
    #[error("layer {0:?} is not a group and cannot contain other layers")]
    NotAGroup(LayerId),
    /// Reparenting `id` under `new_parent` would make `id` its own
    /// ancestor — `new_parent` is `id` itself, or one of `id`'s
    /// descendants.
    #[error(
        "cannot move layer {id:?} under {new_parent:?}: {new_parent:?} is {id:?} or one of its own descendants"
    )]
    CycleDetected { id: LayerId, new_parent: LayerId },
}
