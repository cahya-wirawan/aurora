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
    /// An opacity or fill-opacity value passed to `LayerTree` was outside
    /// the valid `0.0..=1.0` range.
    #[error("opacity {0} is out of range: must be within 0.0..=1.0")]
    OpacityOutOfRange(f32),
    /// [`crate::LayerTree::add_mask`] was called on a layer that already
    /// has one — matching Photoshop's own UI, which replaces "Add Layer
    /// Mask" with "Delete Layer Mask" once one exists rather than letting
    /// a second one silently overwrite it.
    #[error("layer {0:?} already has a mask")]
    MaskAlreadyExists(LayerId),
    /// A mask-only operation ([`crate::LayerTree::remove_mask`],
    /// [`crate::LayerTree::set_mask_enabled`],
    /// [`crate::LayerTree::set_mask_inverted`]) was called on a layer that
    /// exists but has no mask.
    #[error("layer {0:?} has no mask")]
    NoMask(LayerId),
    /// A [`crate::SelectionSet`] operation ([`crate::SelectionSet::invert`],
    /// [`crate::SelectionSet::save_active`]) needed an active selection,
    /// but none exists.
    #[error("no active selection")]
    NoActiveSelection,
    /// [`crate::SelectionSet::load`] or
    /// [`crate::SelectionSet::delete_saved`] named a selection that was
    /// never saved (or was already deleted).
    #[error("no selection saved under {0:?}")]
    UnknownSavedSelection(String),
}
