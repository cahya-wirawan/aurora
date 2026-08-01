//! History: reversible operations plus dirtied regions, unlimited
//! undo/redo (§7.3.3). PLAN.md M1.4's fifth piece.
//!
//! [`History`] mirrors every mutating [`LayerTree`] method with one that
//! also records how to undo it. It does not wrap [`LayerTree`] (own it,
//! `Deref` to it, etc.) — there is no `Document` type yet tying a tree,
//! a selection set, and a history together, so `History` and `LayerTree`
//! stay siblings, each call taking `&mut LayerTree` explicitly. A future
//! `Document` can compose them.

use aurora_core::Rect;

use crate::error::DocError;
use crate::layer::{BlendMode, LayerId, LayerKind, LayerLock, LayerMask};
use crate::tree::{LayerTree, RemovedSubtree};

/// One recorded step, always stored as *how to undo the step that's
/// currently on top* — never "what the user did," which would need a
/// separate, parallel "how to undo it" derivation at undo time. Applying
/// an op (see [`apply`]) both performs it and returns its own inverse,
/// which is exactly what the opposite stack needs — so `undo` and `redo`
/// share one function.
///
/// Every variant stores just the one changed value (or, for a structural
/// change, exactly the removed subtree) — never a whole-document
/// snapshot (§7.3.3).
enum LayerOp {
    /// Remove `LayerId` (capturing it fresh at apply time, which becomes
    /// the paired [`LayerOp::Restore`] pushed onto the other stack). The
    /// inverse of adding a layer, and also what an already-removed
    /// layer's own redo/undo cycles through.
    RemoveById(LayerId),
    /// Put a previously captured subtree back. The inverse of removing
    /// one.
    Restore(RemovedSubtree),
    Reparent {
        id: LayerId,
        parent: Option<LayerId>,
        index: usize,
    },
    Rename {
        id: LayerId,
        name: String,
    },
    SetOpacity {
        id: LayerId,
        value: f32,
    },
    SetFillOpacity {
        id: LayerId,
        value: f32,
    },
    SetBlendMode {
        id: LayerId,
        value: BlendMode,
    },
    SetVisible {
        id: LayerId,
        value: bool,
    },
    SetLock {
        id: LayerId,
        value: LayerLock,
    },
    RemoveMask(LayerId),
    RestoreMask(LayerId, LayerMask),
    SetMaskEnabled {
        id: LayerId,
        value: bool,
    },
    SetMaskInverted {
        id: LayerId,
        value: bool,
    },
}

/// The document-space region a step touched, when it's knowable from
/// this crate alone. `None` for a layer whose kind is [`LayerKind::Group`]
/// (a group has no `bounds` of its own — its on-canvas extent is the
/// union of its descendants', which needs subtree-bounds aggregation
/// that doesn't exist anywhere yet, not even for compositing) or for a
/// step with no visual effect at all (`Rename`).
fn layer_dirty_rect(tree: &LayerTree, id: LayerId) -> Option<Rect> {
    match tree.kind(id)? {
        LayerKind::Pixel { bounds } => Some(*bounds),
        LayerKind::Group { .. } => None,
    }
}

/// Same as [`layer_dirty_rect`], but unioned across every pixel layer in
/// a captured subtree — reusing [`Rect::union`]'s own documented
/// empty-rect-as-identity behaviour to fold over an arbitrary number of
/// them, the same accumulation idiom `aurora_tile::Tile::mark_dirty` and
/// `aurora_graph::RenderGraph`'s dirty propagation already use.
fn subtree_dirty_rect(removed: &RemovedSubtree) -> Option<Rect> {
    removed
        .entries
        .iter()
        .filter_map(|(_, entry)| match &entry.kind {
            LayerKind::Pixel { bounds } => Some(*bounds),
            LayerKind::Group { .. } => None,
        })
        .reduce(|a, b| a.union(&b))
}

/// The sibling index `id` currently occupies under `parent` (`None` =
/// root), or `None` if `id` isn't actually there — used to capture the
/// *current* position of a layer being reparented, without adding a new
/// `LayerTree` method: [`LayerTree::roots`]/[`LayerTree::children`]
/// already expose exactly this.
fn current_index(tree: &LayerTree, id: LayerId, parent: Option<LayerId>) -> Option<usize> {
    let siblings = match parent {
        None => tree.roots(),
        Some(p) => tree.children(p)?,
    };
    siblings.iter().position(|&sibling| sibling == id)
}

/// Applies `op` to `tree` and returns its own inverse (what undoes what
/// this call just did) plus the region it dirtied, if known. The one
/// place every undo *and* redo step actually happens — `History::undo`/
/// `redo` differ only in which stack they pop from and push to.
fn apply(tree: &mut LayerTree, op: LayerOp) -> Result<(LayerOp, Option<Rect>), DocError> {
    match op {
        LayerOp::RemoveById(id) => {
            let removed = tree.remove_capturing(id)?;
            let dirty = subtree_dirty_rect(&removed);
            Ok((LayerOp::Restore(removed), dirty))
        }
        LayerOp::Restore(removed) => {
            let dirty = subtree_dirty_rect(&removed);
            let id = tree.restore(removed)?;
            Ok((LayerOp::RemoveById(id), dirty))
        }
        LayerOp::Reparent { id, parent, index } => {
            let old_parent = tree.parent(id);
            let old_index =
                current_index(tree, id, old_parent).ok_or(DocError::UnknownLayer(id))?;
            tree.reparent(id, parent, index)?;
            Ok((
                LayerOp::Reparent {
                    id,
                    parent: old_parent,
                    index: old_index,
                },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::Rename { id, name } => {
            let old = tree.name(id).ok_or(DocError::UnknownLayer(id))?.to_owned();
            tree.set_name(id, name)?;
            Ok((LayerOp::Rename { id, name: old }, None))
        }
        LayerOp::SetOpacity { id, value } => {
            let old = tree.opacity(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_opacity(id, value)?;
            Ok((
                LayerOp::SetOpacity { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::SetFillOpacity { id, value } => {
            let old = tree.fill_opacity(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_fill_opacity(id, value)?;
            Ok((
                LayerOp::SetFillOpacity { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::SetBlendMode { id, value } => {
            let old = tree.blend_mode(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_blend_mode(id, value)?;
            Ok((
                LayerOp::SetBlendMode { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::SetVisible { id, value } => {
            let old = tree.visible(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_visible(id, value)?;
            Ok((
                LayerOp::SetVisible { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::SetLock { id, value } => {
            let old = tree.lock(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_lock(id, value)?;
            Ok((
                LayerOp::SetLock { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::RemoveMask(id) => {
            let mask = tree.take_mask(id)?;
            let dirty = layer_dirty_rect(tree, id);
            Ok((LayerOp::RestoreMask(id, mask), dirty))
        }
        LayerOp::RestoreMask(id, mask) => {
            tree.restore_mask(id, mask)?;
            Ok((LayerOp::RemoveMask(id), layer_dirty_rect(tree, id)))
        }
        LayerOp::SetMaskEnabled { id, value } => {
            let old = tree.mask(id).ok_or(DocError::NoMask(id))?.enabled;
            tree.set_mask_enabled(id, value)?;
            Ok((
                LayerOp::SetMaskEnabled { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
        LayerOp::SetMaskInverted { id, value } => {
            let old = tree.mask(id).ok_or(DocError::NoMask(id))?.inverted;
            tree.set_mask_inverted(id, value)?;
            Ok((
                LayerOp::SetMaskInverted { id, value: old },
                layer_dirty_rect(tree, id),
            ))
        }
    }
}

/// Unlimited undo/redo over a [`LayerTree`] (§7.3.3): every mutating
/// `LayerTree` method has a mirror here that performs the same change
/// and records how to reverse it. A step recorded through this type's
/// own methods can always be undone; a `LayerTree` mutation made by
/// calling the tree directly (bypassing `History`) is invisible to it,
/// and mixing the two can leave a recorded step referring to a layer (or
/// position) that direct calls already changed out from under it -- see
/// [`LayerTree::restore`]'s own doc comment for the specific errors that
/// can then surface. Normal use (only ever mutating through one
/// `History`) never hits this.
///
/// New activity through this type's own methods always clears the redo
/// stack, matching every mainstream editor's undo/redo behaviour.
pub struct History {
    undo_stack: Vec<LayerOp>,
    redo_stack: Vec<LayerOp>,
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    fn push(&mut self, op: LayerOp) {
        self.undo_stack.push(op);
        self.redo_stack.clear();
    }

    /// Same as [`LayerTree::add_pixel_layer`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::add_pixel_layer`].
    pub fn add_pixel_layer(
        &mut self,
        tree: &mut LayerTree,
        name: impl Into<String>,
        bounds: Rect,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        let id = tree.add_pixel_layer(name, bounds, parent)?;
        self.push(LayerOp::RemoveById(id));
        Ok(id)
    }

    /// Same as [`LayerTree::add_group`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::add_group`].
    pub fn add_group(
        &mut self,
        tree: &mut LayerTree,
        name: impl Into<String>,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        let id = tree.add_group(name, parent)?;
        self.push(LayerOp::RemoveById(id));
        Ok(id)
    }

    /// Same as [`LayerTree::remove`], recorded for undo. Returns the
    /// region the removal dirtied, if known (see [`layer_dirty_rect`]).
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::remove`].
    pub fn remove(&mut self, tree: &mut LayerTree, id: LayerId) -> Result<Option<Rect>, DocError> {
        let removed = tree.remove_capturing(id)?;
        let dirty = subtree_dirty_rect(&removed);
        self.push(LayerOp::Restore(removed));
        Ok(dirty)
    }

    /// Same as [`LayerTree::reparent`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::reparent`].
    pub fn reparent(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        new_parent: Option<LayerId>,
        index: usize,
    ) -> Result<(), DocError> {
        let old_parent = tree.parent(id);
        let old_index = current_index(tree, id, old_parent).ok_or(DocError::UnknownLayer(id))?;
        tree.reparent(id, new_parent, index)?;
        self.push(LayerOp::Reparent {
            id,
            parent: old_parent,
            index: old_index,
        });
        Ok(())
    }

    /// Same as [`LayerTree::set_name`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_name`].
    pub fn set_name(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        name: impl Into<String>,
    ) -> Result<(), DocError> {
        let old = tree.name(id).ok_or(DocError::UnknownLayer(id))?.to_owned();
        tree.set_name(id, name)?;
        self.push(LayerOp::Rename { id, name: old });
        Ok(())
    }

    /// Same as [`LayerTree::set_opacity`], recorded for undo. Returns the
    /// region the change dirtied, if known.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_opacity`].
    pub fn set_opacity(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: f32,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.opacity(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_opacity(id, value)?;
        self.push(LayerOp::SetOpacity { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_fill_opacity`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_fill_opacity`].
    pub fn set_fill_opacity(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: f32,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.fill_opacity(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_fill_opacity(id, value)?;
        self.push(LayerOp::SetFillOpacity { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_blend_mode`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_blend_mode`].
    pub fn set_blend_mode(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: BlendMode,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.blend_mode(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_blend_mode(id, value)?;
        self.push(LayerOp::SetBlendMode { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_visible`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_visible`].
    pub fn set_visible(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: bool,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.visible(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_visible(id, value)?;
        self.push(LayerOp::SetVisible { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_lock`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_lock`].
    pub fn set_lock(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: LayerLock,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.lock(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_lock(id, value)?;
        self.push(LayerOp::SetLock { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::add_mask`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::add_mask`].
    pub fn add_mask(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        bounds: Rect,
    ) -> Result<Option<Rect>, DocError> {
        tree.add_mask(id, bounds)?;
        self.push(LayerOp::RemoveMask(id));
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::remove_mask`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::remove_mask`].
    pub fn remove_mask(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
    ) -> Result<Option<Rect>, DocError> {
        let mask = tree.take_mask(id)?;
        self.push(LayerOp::RestoreMask(id, mask));
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_mask_enabled`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_mask_enabled`].
    pub fn set_mask_enabled(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: bool,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.mask(id).ok_or(DocError::NoMask(id))?.enabled;
        tree.set_mask_enabled(id, value)?;
        self.push(LayerOp::SetMaskEnabled { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_mask_inverted`], recorded for undo.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_mask_inverted`].
    pub fn set_mask_inverted(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: bool,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.mask(id).ok_or(DocError::NoMask(id))?.inverted;
        tree.set_mask_inverted(id, value)?;
        self.push(LayerOp::SetMaskInverted { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Undoes the most recent step, if any. `None` both when there was
    /// nothing to undo, and when there was but its dirtied region isn't
    /// knowable (see [`layer_dirty_rect`]) — same conflated shape
    /// [`LayerTree::parent`] already documents; callers that need to
    /// tell those apart should check [`Self::can_undo`] first.
    ///
    /// # Errors
    ///
    /// Returns whatever error the underlying `LayerTree` call surfaced —
    /// only reachable by mixing direct `LayerTree` calls with this
    /// `History` (see this type's own doc comment).
    pub fn undo(&mut self, tree: &mut LayerTree) -> Result<Option<Rect>, DocError> {
        let Some(op) = self.undo_stack.pop() else {
            return Ok(None);
        };
        let (inverse, dirty) = apply(tree, op)?;
        self.redo_stack.push(inverse);
        Ok(dirty)
    }

    /// Redoes the most recently undone step, if any. Same conflated
    /// `None` shape as [`Self::undo`] — check [`Self::can_redo`] first if
    /// the distinction matters.
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`].
    pub fn redo(&mut self, tree: &mut LayerTree) -> Result<Option<Rect>, DocError> {
        let Some(op) = self.redo_stack.pop() else {
            return Ok(None);
        };
        let (inverse, dirty) = apply(tree, op)?;
        self.undo_stack.push(inverse);
        Ok(dirty)
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("undo_len", &self.undo_stack.len())
            .field("redo_len", &self.redo_stack.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::History;
    use crate::DocError;
    use crate::layer::{BlendMode, LayerKind, LayerLock};
    use crate::tree::LayerTree;
    use aurora_core::{Id, Rect};

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    fn other_bounds() -> Rect {
        Rect {
            x: 5,
            y: 5,
            width: 20,
            height: 20,
        }
    }

    #[test]
    fn fresh_history_cannot_undo_or_redo() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        assert!(!history.can_undo());
        assert!(!history.can_redo());

        match history.undo(&mut tree) {
            Ok(None) => {}
            other => unreachable!("expected Ok(None), got {other:?}"),
        }
        match history.redo(&mut tree) {
            Ok(None) => {}
            other => unreachable!("expected Ok(None), got {other:?}"),
        }
    }

    #[test]
    fn add_pixel_layer_undo_removes_it_redo_restores_same_id() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(tree.contains(id));
        assert!(history.can_undo());
        assert!(!history.can_redo());

        let dirty = match history.undo(&mut tree) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!tree.contains(id), "undo of an add must remove the layer");
        assert_eq!(dirty, Some(bounds()));
        assert!(!history.can_undo());
        assert!(history.can_redo());

        let dirty = match history.redo(&mut tree) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            tree.contains(id),
            "redo of an add must bring the same id back"
        );
        assert_eq!(dirty, Some(bounds()));
        assert_eq!(tree.kind(id), Some(&LayerKind::Pixel { bounds: bounds() }));
    }

    #[test]
    fn add_group_undo_redo_round_trips_the_same_id() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let id = match history.add_group(&mut tree, "g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // A fresh empty group has no on-canvas extent to dirty.
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(id));

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(id));
        assert_eq!(
            tree.kind(id),
            Some(&LayerKind::Group {
                children: Vec::new()
            })
        );
    }

    #[test]
    fn remove_undo_restores_original_position_redo_removes_again() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let a = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match history.add_pixel_layer(&mut tree, "b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots = [b, a] (newest on top).
        assert_eq!(tree.roots(), [b, a]);

        let dirty = match history.remove(&mut tree, a) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(dirty, Some(bounds()));
        assert!(!tree.contains(a));
        assert_eq!(tree.roots(), [b]);

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(a));
        assert_eq!(
            tree.roots(),
            [b, a],
            "undo of a remove must restore the original position, not just re-add on top"
        );

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(a));
        assert_eq!(tree.roots(), [b]);
    }

    #[test]
    fn remove_undo_restores_a_whole_group_subtree_with_original_ids_and_properties() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let outer = match history.add_group(&mut tree, "outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match history.add_pixel_layer(&mut tree, "inner", bounds(), Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut tree, inner, 0.5) {
            unreachable!("{err:?}");
        }

        if let Err(err) = history.remove(&mut tree, outer) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(outer));
        assert!(!tree.contains(inner));

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(outer));
        assert!(
            tree.contains(inner),
            "the nested child must come back with the same id"
        );
        assert_eq!(tree.parent(inner), Some(outer));
        assert_eq!(
            tree.opacity(inner),
            Some(0.5),
            "a restored layer's own properties must survive, not reset to defaults"
        );
    }

    #[test]
    fn remove_rejects_an_unknown_id() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let bogus: super::LayerId = Id::from_raw(999);
        match history.remove(&mut tree, bogus) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn reparent_undo_redo_round_trips_position() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let a = match history.add_group(&mut tree, "a", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match history.add_group(&mut tree, "b", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match history.add_pixel_layer(&mut tree, "c", bounds(), Some(a)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = history.reparent(&mut tree, child, Some(b), 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.parent(child), Some(b));

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.parent(child),
            Some(a),
            "undo must restore the old parent"
        );

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.parent(child), Some(b));
    }

    #[test]
    fn set_name_undo_redo_round_trips() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = history.set_name(&mut tree, id, "renamed") {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.name(id), Some("renamed"));

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.name(id), Some("a"));

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.name(id), Some("renamed"));
    }

    #[test]
    // Exact-literal round-trip through the op stack, no arithmetic --
    // same reasoning `tree::tests` already documents for its own
    // float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn set_opacity_undo_redo_round_trips_and_dirties_pixel_bounds() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let dirty = match history.set_opacity(&mut tree, id, 0.25) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(dirty, Some(bounds()));
        assert_eq!(tree.opacity(id), Some(0.25));

        let dirty = match history.undo(&mut tree) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(dirty, Some(bounds()));
        assert_eq!(tree.opacity(id), Some(1.0));

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.opacity(id), Some(0.25));
    }

    #[test]
    fn set_opacity_on_a_group_dirties_nothing_knowable() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_group(&mut tree, "g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let dirty = match history.set_opacity(&mut tree, id, 0.5) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            dirty, None,
            "a group has no bounds of its own to report as dirtied"
        );
    }

    #[test]
    fn set_fill_opacity_undo_redo_round_trips() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_fill_opacity(&mut tree, id, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.fill_opacity(id), Some(1.0));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.fill_opacity(id), Some(0.5));
    }

    #[test]
    fn set_blend_mode_undo_redo_round_trips() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_blend_mode(&mut tree, id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.blend_mode(id), Some(BlendMode::Normal));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.blend_mode(id), Some(BlendMode::Multiply));
    }

    #[test]
    fn set_visible_undo_redo_round_trips() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_visible(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.visible(id), Some(true));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.visible(id), Some(false));
    }

    #[test]
    fn set_lock_undo_redo_round_trips() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_lock(&mut tree, id, LayerLock::all()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.lock(id), Some(LayerLock::none()));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.lock(id), Some(LayerLock::all()));
    }

    #[test]
    fn add_mask_undo_removes_it_redo_restores_it_enabled() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = history.add_mask(&mut tree, id, other_bounds()) {
            unreachable!("{err:?}");
        }
        assert!(tree.mask(id).is_some());

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.mask(id).is_none());

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        let mask = tree.mask(id).unwrap_or_else(|| unreachable!("just redone"));
        assert_eq!(mask.bounds, other_bounds());
        assert!(mask.enabled);
        assert!(!mask.inverted);
    }

    #[test]
    fn remove_mask_undo_restores_its_exact_toggled_state_not_the_default() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_mask_enabled(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_mask_inverted(&mut tree, id, true) {
            unreachable!("{err:?}");
        }

        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert!(tree.mask(id).is_none());

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        let mask = tree
            .mask(id)
            .unwrap_or_else(|| unreachable!("just restored by undo"));
        assert!(
            !mask.enabled,
            "restoring a removed mask must bring back its exact toggled state"
        );
        assert!(mask.inverted);
    }

    #[test]
    fn set_mask_enabled_and_inverted_undo_redo_round_trip() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, id, bounds()) {
            unreachable!("{err:?}");
        }

        if let Err(err) = history.set_mask_enabled(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id).map(|m| m.enabled), Some(true));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id).map(|m| m.enabled), Some(false));

        if let Err(err) = history.set_mask_inverted(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id).map(|m| m.inverted), Some(false));
    }

    #[test]
    fn a_new_action_clears_the_redo_stack() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let a = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(history.can_redo());

        // A brand new action, not an undo/redo, must clear the redo stack.
        if let Err(err) = history.add_pixel_layer(&mut tree, "b", bounds(), None) {
            unreachable!("{err:?}");
        }
        assert!(
            !history.can_redo(),
            "new activity must invalidate the old redo path"
        );
        assert!(!tree.contains(a), "the undone layer must still be gone");
    }

    #[test]
    fn multiple_steps_undo_in_lifo_order() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let a = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match history.add_pixel_layer(&mut tree, "b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots = [b, a].

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(b), "the most recent action undoes first");
        assert!(tree.contains(a));

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(a));
        assert!(!history.can_undo());

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(a));
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(b));
    }
}
