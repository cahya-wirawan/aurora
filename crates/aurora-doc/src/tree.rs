//! The layer tree itself: identity, nesting, and ordering. PLAN.md M1.4's
//! first deliverable.

use std::collections::HashMap;

use aurora_core::{IdGenerator, Rect};

use crate::error::DocError;
use crate::layer::{BlendMode, Layer, LayerEntry, LayerId, LayerKind, LayerLock, LayerMask};

/// A layer (and, if it was a group, its whole subtree) detached from a
/// [`LayerTree`] by [`LayerTree::remove_capturing`], with enough recorded
/// to put it back exactly via [`LayerTree::restore`] — same id(s),
/// same position, same properties. Not a document snapshot (§7.3.3):
/// scoped to exactly the layer(s) one `remove` call deleted, not the
/// whole tree. [`crate::History`]'s building block for undoing a remove
/// and, symmetrically (`restore` and `remove_capturing` are each other's
/// inverse), for undoing an add.
///
/// `entries` is a flat `(id, LayerEntry)` list — root first, then
/// descendants — rather than a recursive shape, because each captured
/// `LayerEntry` already carries everything needed to reconstruct the
/// tree shape itself: its own `parent` field, and (for a group) its own
/// `children` list. Restoring is just re-inserting every entry under its
/// original id and re-linking the root into its old parent's sibling
/// list; every descendant's own recorded fields already point at the
/// right (also-being-restored) ids.
///
/// `Clone`: [`crate::History`]'s in-memory journal needs its own copy of
/// a captured subtree independent of the one actually consumed by
/// [`LayerTree::restore`] (one is replayed into the tree, the other kept
/// for [`crate::History::replay`]'s later use) — see that type's own doc
/// comment for why replaying it back into a *disk file* is a separate,
/// not-yet-built piece.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RemovedSubtree {
    pub(crate) root: LayerId,
    pub(crate) parent: Option<LayerId>,
    pub(crate) index: usize,
    pub(crate) entries: Vec<(LayerId, LayerEntry)>,
}

/// A forest of layers: pixel layers and groups, nested to any depth.
///
/// **Ordering convention, used throughout this crate**: sibling lists
/// (both [`LayerTree::roots`] and a group's own children) are top-to-bottom
/// as a layers panel displays them — index 0 is the *topmost* layer,
/// painted last (on top) in the final composite. This is the opposite of
/// how PSD stores layers on disk (bottom layer first); `aurora-io` will
/// need to reverse one or the other when it exists. A freshly added layer
/// is inserted at index 0 (on top), matching every mainstream editor's
/// "new layer appears above the current one" behaviour.
///
/// Deliberately just two layer kinds (`Pixel`, `Group`) — see
/// [`LayerKind`]'s own doc comment for why the other nine FR-003 names
/// aren't here yet. Every layer carries opacity, fill opacity, blend mode,
/// visibility, and locking (see [`Self::set_opacity`] and neighbours) —
/// stored state only, since nothing yet composites or paints to actually
/// interpret them. Any layer, pixel or group, may also carry one
/// [`LayerMask`] (see [`Self::add_mask`] and neighbours) — likewise
/// stored state only, no real mask pixels yet.
///
/// `Serialize`/`Deserialize`: a `.aur` file's own manifest entry (ADR
/// 0009) is the whole tree, `postcard`-encoded — every field here
/// (including `ids`, via `IdGenerator`'s own hand-written impl) needs to
/// round-trip so a reloaded document keeps allocating fresh, non-
/// colliding `LayerId`s rather than restarting the counter from `0`.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LayerTree {
    ids: IdGenerator<Layer>,
    layers: HashMap<LayerId, LayerEntry>,
    /// Root-level layers. See this type's own doc comment for the
    /// ordering convention.
    roots: Vec<LayerId>,
}

impl LayerTree {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ids: IdGenerator::new(),
            layers: HashMap::new(),
            roots: Vec::new(),
        }
    }

    /// Adds a pixel layer named `name`, positioned at `bounds` in
    /// document space, as the new topmost child of `parent` (or a new
    /// topmost root, if `parent` is `None`).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `parent` is `Some` and
    /// doesn't exist, or [`DocError::NotAGroup`] if `parent` names a
    /// pixel layer. Nothing is added when this happens.
    pub fn add_pixel_layer(
        &mut self,
        name: impl Into<String>,
        bounds: Rect,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        self.insert(name.into(), parent, LayerKind::Pixel { bounds })
    }

    /// Adds an empty group named `name`, as the new topmost child of
    /// `parent` (or a new topmost root, if `parent` is `None`).
    ///
    /// # Errors
    ///
    /// Same as [`Self::add_pixel_layer`].
    pub fn add_group(
        &mut self,
        name: impl Into<String>,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        self.insert(
            name.into(),
            parent,
            LayerKind::Group {
                children: Vec::new(),
            },
        )
    }

    fn insert(
        &mut self,
        name: String,
        parent: Option<LayerId>,
        kind: LayerKind,
    ) -> Result<LayerId, DocError> {
        // Validate `parent` before touching `self.layers` at all, so a
        // failed call adds nothing -- same "all or nothing" discipline
        // `aurora_graph::RenderGraph::add_node` uses for its own inputs.
        if let Some(parent_id) = parent {
            match self.layers.get(&parent_id) {
                None => return Err(DocError::UnknownLayer(parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(parent_id));
                }
                Some(_) => {}
            }
        }

        let id = self.ids.next_id();
        self.layers.insert(id, LayerEntry::new(name, parent, kind));

        let siblings = match self.sibling_list_mut(parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!(
                    "parent's existence and group-ness were already validated above: {err:?}"
                )
            }
        };
        siblings.insert(0, id);
        Ok(id)
    }

    /// Removes `id` from the tree. If `id` is a group, every descendant
    /// is removed too — a plain delete removes a group's contents along
    /// with it, matching every mainstream editor's actual behaviour
    /// (there's no implicit "flatten children up a level" on delete).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn remove(&mut self, id: LayerId) -> Result<(), DocError> {
        self.remove_capturing(id)?;
        Ok(())
    }

    /// Same as [`Self::remove`], but keeps every removed entry (root plus,
    /// for a group, its whole subtree) instead of discarding it —
    /// [`crate::History`]'s own building block for undoing a remove (and,
    /// symmetrically, for undoing an add: see [`Self::restore`]).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub(crate) fn remove_capturing(&mut self, id: LayerId) -> Result<RemovedSubtree, DocError> {
        let parent = self
            .layers
            .get(&id)
            .ok_or(DocError::UnknownLayer(id))?
            .parent;

        let siblings = match self.sibling_list_mut(parent) {
            Ok(list) => list,
            Err(err) => unreachable!("id's own recorded parent must be valid: {err:?}"),
        };
        let Some(index) = siblings.iter().position(|&sibling| sibling == id) else {
            unreachable!("id's own recorded parent must list id as one of its children");
        };
        siblings.remove(index);

        let mut entries = Vec::new();
        self.capture_subtree(id, &mut entries);

        Ok(RemovedSubtree {
            root: id,
            parent,
            index,
            entries,
        })
    }

    /// Removes `id` and, recursively, every descendant, appending each
    /// `(id, LayerEntry)` pair to `out` — the flat capture
    /// [`Self::remove_capturing`]/[`Self::restore`] round-trip through. A
    /// descendant's own recorded `parent`/(for a group) `children` fields
    /// are already exactly what's needed to reconstruct the subtree, so
    /// no separate tree shape needs to be recorded alongside the entries.
    fn capture_subtree(&mut self, id: LayerId, out: &mut Vec<(LayerId, LayerEntry)>) {
        let Some(entry) = self.layers.remove(&id) else {
            unreachable!("a group's recorded children must exist in the tree by construction");
        };
        let children = match &entry.kind {
            LayerKind::Group { children } => children.clone(),
            LayerKind::Pixel { .. } => Vec::new(),
        };
        out.push((id, entry));
        for child in children {
            self.capture_subtree(child, out);
        }
    }

    /// Restores a subtree previously detached by [`Self::remove_capturing`]
    /// — every layer comes back at its original id, so anything outside
    /// this tree that already referenced those ids (a saved selection, a
    /// pending [`crate::History`] redo entry) stays valid. Returns the
    /// restored root's id (same as [`RemovedSubtree::root`], returned for
    /// convenience).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if the subtree's recorded parent
    /// no longer exists, or [`DocError::NotAGroup`] if it now names a
    /// pixel layer — both only reachable if something removed or replaced
    /// that parent after this subtree was captured, since normal
    /// [`crate::History`] undo/redo never reaches here out of order.
    /// Nothing is changed when this happens.
    pub(crate) fn restore(&mut self, removed: RemovedSubtree) -> Result<LayerId, DocError> {
        let RemovedSubtree {
            root,
            parent,
            index,
            entries,
        } = removed;

        // Validate before mutating anything -- same "all or nothing"
        // discipline `insert` uses for its own `parent` argument.
        if let Some(parent_id) = parent {
            match self.layers.get(&parent_id) {
                None => return Err(DocError::UnknownLayer(parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(parent_id));
                }
                Some(_) => {}
            }
        }

        for (id, entry) in entries {
            self.layers.insert(id, entry);
        }

        let siblings = match self.sibling_list_mut(parent) {
            Ok(list) => list,
            Err(err) => unreachable!("just validated above: {err:?}"),
        };
        let clamped = index.min(siblings.len());
        siblings.insert(clamped, root);
        Ok(root)
    }

    /// Moves `id` (and, if it's a group, its whole subtree) to be a child
    /// of `new_parent` at sibling position `index`, clamped to the valid
    /// range — an out-of-range `index` lands at the end rather than
    /// erroring, the same forgiving behaviour a UI drag-and-drop drop
    /// target needs.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` or `new_parent` (when
    /// `Some`) doesn't exist, [`DocError::NotAGroup`] if `new_parent`
    /// names a pixel layer, or [`DocError::CycleDetected`] if
    /// `new_parent` is `id` itself or one of `id`'s own descendants.
    /// Nothing is changed when this happens.
    pub fn reparent(
        &mut self,
        id: LayerId,
        new_parent: Option<LayerId>,
        index: usize,
    ) -> Result<(), DocError> {
        let old_parent = match self.layers.get(&id) {
            Some(entry) => entry.parent,
            None => return Err(DocError::UnknownLayer(id)),
        };

        if let Some(new_parent_id) = new_parent {
            if new_parent_id == id || self.is_descendant(new_parent_id, id) {
                return Err(DocError::CycleDetected {
                    id,
                    new_parent: new_parent_id,
                });
            }
            match self.layers.get(&new_parent_id) {
                None => return Err(DocError::UnknownLayer(new_parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(new_parent_id));
                }
                Some(_) => {}
            }
        }

        // Everything validated -- detach from the old position...
        let old_siblings = match self.sibling_list_mut(old_parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!("id's current parent, just read above, must be valid: {err:?}")
            }
        };
        old_siblings.retain(|&sibling| sibling != id);

        // ...then attach at the new one.
        let new_siblings = match self.sibling_list_mut(new_parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!(
                    "new_parent's existence and group-ness were already validated: {err:?}"
                )
            }
        };
        let clamped = index.min(new_siblings.len());
        new_siblings.insert(clamped, id);

        let Some(entry) = self.layers.get_mut(&id) else {
            unreachable!("id's existence was already confirmed above");
        };
        entry.parent = new_parent;
        Ok(())
    }

    /// Whether `descendant` is nested anywhere inside `ancestor`'s
    /// subtree — [`Self::reparent`]'s cycle guard. Walks upward from
    /// `descendant` through its own chain of parents (bounded by tree
    /// depth) rather than downward through `ancestor`'s whole subtree
    /// (which could be large), since the answer only needs one path, not
    /// an exhaustive search.
    fn is_descendant(&self, descendant: LayerId, ancestor: LayerId) -> bool {
        let Some(entry) = self.layers.get(&descendant) else {
            return false;
        };
        match entry.parent {
            Some(parent) if parent == ancestor => true,
            Some(parent) => self.is_descendant(parent, ancestor),
            None => false,
        }
    }

    /// The sibling list `parent` names: [`Self::roots`] if `None`, or a
    /// group's own children if `Some`. The single place every
    /// insert/remove/reparent path goes through to find "the list `id`
    /// lives in."
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] or [`DocError::NotAGroup`] —
    /// see [`Self::add_pixel_layer`]'s doc comment.
    fn sibling_list_mut(&mut self, parent: Option<LayerId>) -> Result<&mut Vec<LayerId>, DocError> {
        match parent {
            None => Ok(&mut self.roots),
            Some(parent_id) => {
                let entry = self
                    .layers
                    .get_mut(&parent_id)
                    .ok_or(DocError::UnknownLayer(parent_id))?;
                match &mut entry.kind {
                    LayerKind::Group { children } => Ok(children),
                    LayerKind::Pixel { .. } => Err(DocError::NotAGroup(parent_id)),
                }
            }
        }
    }

    #[must_use]
    pub fn contains(&self, id: LayerId) -> bool {
        self.layers.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// `None` both when `id` doesn't exist and when it's a root layer —
    /// callers that need to tell those apart should check
    /// [`Self::contains`] first, matching `aurora_graph::RenderGraph`'s
    /// own `payload`/`dependencies` convention.
    #[must_use]
    pub fn parent(&self, id: LayerId) -> Option<LayerId> {
        self.layers.get(&id).and_then(|entry| entry.parent)
    }

    #[must_use]
    pub fn kind(&self, id: LayerId) -> Option<&LayerKind> {
        self.layers.get(&id).map(|entry| &entry.kind)
    }

    /// The [`aurora_tile::SurfaceId`] `id`'s own pixel content is
    /// addressed under in the document's shared `aurora_tile::TileStore`
    /// ([ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)) —
    /// reused directly from `id`'s own raw value
    /// (`SurfaceId::from_raw(id.to_raw())`), not independently
    /// allocated, so there is no second id-allocation scheme to keep in
    /// sync with this tree's own [`IdGenerator`].
    ///
    /// Returns `None` for an unknown `id`, or one that names a
    /// [`LayerKind::Group`] — a group has no pixels of its own to store,
    /// so it never needs a surface.
    #[must_use]
    pub fn surface_id(&self, id: LayerId) -> Option<aurora_tile::SurfaceId> {
        match self.kind(id)? {
            LayerKind::Pixel { .. } => Some(aurora_tile::SurfaceId::from_raw(id.to_raw())),
            LayerKind::Group { .. } => None,
        }
    }

    #[must_use]
    pub fn name(&self, id: LayerId) -> Option<&str> {
        self.layers.get(&id).map(|entry| entry.name.as_str())
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn set_name(&mut self, id: LayerId, name: impl Into<String>) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.name = name.into();
        Ok(())
    }

    #[must_use]
    pub fn opacity(&self, id: LayerId) -> Option<f32> {
        self.layers.get(&id).map(|entry| entry.opacity)
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist, or
    /// [`DocError::OpacityOutOfRange`] if `opacity` is outside
    /// `0.0..=1.0`. Nothing is changed when this happens.
    pub fn set_opacity(&mut self, id: LayerId, opacity: f32) -> Result<(), DocError> {
        if !(0.0..=1.0).contains(&opacity) {
            return Err(DocError::OpacityOutOfRange(opacity));
        }
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.opacity = opacity;
        Ok(())
    }

    /// The layer's *fill* opacity — distinct from [`Self::opacity`] in
    /// exactly the way Photoshop's own "Fill" slider is: it fades the
    /// layer's own pixels but, unlike [`Self::opacity`], does not fade
    /// layer styles applied on top (a distinction this crate stores now
    /// and a future compositor/layer-style consumer gives meaning to).
    #[must_use]
    pub fn fill_opacity(&self, id: LayerId) -> Option<f32> {
        self.layers.get(&id).map(|entry| entry.fill_opacity)
    }

    /// # Errors
    ///
    /// Same as [`Self::set_opacity`].
    pub fn set_fill_opacity(&mut self, id: LayerId, fill_opacity: f32) -> Result<(), DocError> {
        if !(0.0..=1.0).contains(&fill_opacity) {
            return Err(DocError::OpacityOutOfRange(fill_opacity));
        }
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.fill_opacity = fill_opacity;
        Ok(())
    }

    #[must_use]
    pub fn blend_mode(&self, id: LayerId) -> Option<BlendMode> {
        self.layers.get(&id).map(|entry| entry.blend_mode)
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn set_blend_mode(&mut self, id: LayerId, blend_mode: BlendMode) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.blend_mode = blend_mode;
        Ok(())
    }

    /// A pixel layer's own `bounds`, in document space — `None` for an
    /// unknown `id` or one that names a group (a group has no `bounds`
    /// of its own; see [`LayerKind::Group`]).
    #[must_use]
    pub fn bounds(&self, id: LayerId) -> Option<Rect> {
        match self.kind(id)? {
            LayerKind::Pixel { bounds } => Some(*bounds),
            LayerKind::Group { .. } => None,
        }
    }

    /// Repositions/resizes a pixel layer: sets `id`'s own [`LayerKind::Pixel`]
    /// `bounds` to `bounds` — the Move tool's own document-model support.
    /// Until now a pixel layer's `bounds` could only ever be set once, at
    /// [`Self::add_pixel_layer`] time; nothing could move a layer
    /// afterward.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist, or
    /// [`DocError::NotAPixelLayer`] if it names a group.
    pub fn set_bounds(&mut self, id: LayerId, bounds: Rect) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        match &mut entry.kind {
            LayerKind::Pixel { bounds: current } => {
                *current = bounds;
                Ok(())
            }
            LayerKind::Group { .. } => Err(DocError::NotAPixelLayer(id)),
        }
    }

    /// This layer's *own* visibility flag — not whether it actually shows
    /// up in the final composite, which also depends on every ancestor
    /// group's own visibility. Computing that combined answer needs a
    /// concrete tree-walking consumer this crate doesn't have yet (same
    /// reasoning `spike/psd-write/FINDINGS.md` already recorded: an
    /// invisible group hides its whole subtree).
    #[must_use]
    pub fn visible(&self, id: LayerId) -> Option<bool> {
        self.layers.get(&id).map(|entry| entry.visible)
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn set_visible(&mut self, id: LayerId, visible: bool) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.visible = visible;
        Ok(())
    }

    #[must_use]
    pub fn lock(&self, id: LayerId) -> Option<LayerLock> {
        self.layers.get(&id).map(|entry| entry.lock)
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn set_lock(&mut self, id: LayerId, lock: LayerLock) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.lock = lock;
        Ok(())
    }

    /// `None` both when `id` doesn't exist and when it exists but has no
    /// mask — same conflated shape [`Self::parent`] already documents;
    /// callers that need to tell those apart should check
    /// [`Self::contains`] first.
    #[must_use]
    pub fn mask(&self, id: LayerId) -> Option<&LayerMask> {
        self.layers.get(&id)?.mask.as_ref()
    }

    /// Adds a mask to `id`, enabled and not inverted, covering `bounds` in
    /// document space.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist, or
    /// [`DocError::MaskAlreadyExists`] if it already has a mask. Nothing
    /// is changed when this happens.
    pub fn add_mask(&mut self, id: LayerId, bounds: Rect) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        if entry.mask.is_some() {
            return Err(DocError::MaskAlreadyExists(id));
        }
        entry.mask = Some(LayerMask {
            bounds,
            enabled: true,
            inverted: false,
        });
        Ok(())
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist, or
    /// [`DocError::NoMask`] if it has none. Nothing is changed when this
    /// happens.
    pub fn remove_mask(&mut self, id: LayerId) -> Result<(), DocError> {
        self.take_mask(id)?;
        Ok(())
    }

    /// Same as [`Self::remove_mask`], but returns the removed
    /// [`LayerMask`] instead of discarding it — [`crate::History`]'s own
    /// building block for undoing a `remove_mask` (and, symmetrically,
    /// for undoing an `add_mask`: see [`Self::restore_mask`]).
    ///
    /// # Errors
    ///
    /// Same as [`Self::remove_mask`].
    pub(crate) fn take_mask(&mut self, id: LayerId) -> Result<LayerMask, DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.mask.take().ok_or(DocError::NoMask(id))
    }

    /// Puts back a mask previously removed by [`Self::take_mask`], with
    /// its exact `enabled`/`inverted` state — unlike [`Self::add_mask`],
    /// which always creates a fresh, enabled, uninverted one, this is for
    /// restoring one that may have been toggled before it was removed.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub(crate) fn restore_mask(&mut self, id: LayerId, mask: LayerMask) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.mask = Some(mask);
        Ok(())
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist, or
    /// [`DocError::NoMask`] if it has none. Nothing is changed when this
    /// happens.
    pub fn set_mask_enabled(&mut self, id: LayerId, enabled: bool) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        let mask = entry.mask.as_mut().ok_or(DocError::NoMask(id))?;
        mask.enabled = enabled;
        Ok(())
    }

    /// # Errors
    ///
    /// Same as [`Self::set_mask_enabled`].
    pub fn set_mask_inverted(&mut self, id: LayerId, inverted: bool) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        let mask = entry.mask.as_mut().ok_or(DocError::NoMask(id))?;
        mask.inverted = inverted;
        Ok(())
    }

    /// Root-level layers, top-to-bottom (see this type's own doc comment
    /// for the ordering convention).
    #[must_use]
    pub fn roots(&self) -> &[LayerId] {
        &self.roots
    }

    /// `None` if `id` doesn't exist, or if it exists but is a pixel layer
    /// (which structurally has no children). `Some(&[])` is a real,
    /// expected result for an empty group — callers that need to
    /// distinguish "doesn't exist" from "is a pixel layer" should check
    /// [`Self::contains`]/[`Self::kind`] directly.
    #[must_use]
    pub fn children(&self, id: LayerId) -> Option<&[LayerId]> {
        match self.kind(id)? {
            LayerKind::Group { children } => Some(children.as_slice()),
            LayerKind::Pixel { .. } => None,
        }
    }

    /// Every pixel layer a compositor should actually draw, bottom-to-top
    /// (paint order — later entries draw over earlier ones), recursing
    /// into groups at any depth: for each sibling list (starting at
    /// [`Self::roots`], then, recursively, a visible group's own
    /// [`Self::children`]), siblings are walked bottom-to-top (reversed
    /// from their stored top-to-bottom order — see this type's own doc
    /// comment for that convention) and a [`LayerKind::Pixel`] entry is
    /// pushed directly, while a [`LayerKind::Group`] entry "unpacks" in
    /// place at its own stacking position by recursing into its
    /// children with the same logic. This correctly interleaves a
    /// group's contents among its siblings rather than, say, appending
    /// them all at the end.
    ///
    /// **Ancestor-gated visibility**: a layer only appears if it and
    /// *every* group in its ancestor chain up to the root has its own
    /// `visible` flag `true` — an invisible group hides its whole
    /// subtree regardless of any individual descendant's own `visible`
    /// flag, matching every mainstream layer-based editor's "folder
    /// visibility gates its contents" behaviour. A hidden or invisible
    /// layer (pixel or group) is skipped entirely, not composited at
    /// reduced strength — visibility is binary, matching `set_visible`'s
    /// own semantics.
    ///
    /// [`LayerKind::Group`] entries themselves are never pushed to the
    /// result — a group isn't a compositable object, only its pixel-
    /// layer contents are.
    ///
    /// **Scope, stated honestly — what this does *not* do**: a group's
    /// own `opacity`/`blend_mode`/mask are **not** aggregated into its
    /// children's effective compositing. A child nested in a group at
    /// 50% opacity still composites using only its *own* opacity/blend
    /// mode, exactly as if it were a root layer — a group set to 50%
    /// opacity does not (yet) make its contents appear at half strength.
    /// That aggregation, and the same subtree-bounds/effective-
    /// visibility aggregation this crate's own private
    /// `layer_dirty_rect` helper (`history.rs`) still lacks for groups,
    /// are separate, still-open gaps this method does not attempt to
    /// close.
    #[must_use]
    pub fn paint_order(&self) -> Vec<LayerId> {
        let mut out = Vec::new();
        self.paint_order_into(&self.roots, &mut out);
        out
    }

    /// [`Self::paint_order`]'s own recursive worker: walks `siblings`
    /// (a sibling list in its stored top-to-bottom order — either
    /// [`Self::roots`] or a group's own [`Self::children`]) bottom-to-top,
    /// appending each visible [`LayerKind::Pixel`] directly to `out` and
    /// recursing into each visible [`LayerKind::Group`]'s own children.
    /// An invisible layer, pixel or group, is skipped — for a group,
    /// that skips its whole subtree without recursing into it at all,
    /// which is what makes ancestor visibility gate transitively rather
    /// than needing a separately tracked "is some ancestor hidden"
    /// flag.
    fn paint_order_into(&self, siblings: &[LayerId], out: &mut Vec<LayerId>) {
        for &id in siblings.iter().rev() {
            if self.visible(id) != Some(true) {
                continue;
            }
            match self.kind(id) {
                Some(LayerKind::Pixel { .. }) => out.push(id),
                Some(LayerKind::Group { children }) => self.paint_order_into(children, out),
                None => {}
            }
        }
    }
}

impl Default for LayerTree {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LayerTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerTree")
            .field("layer_count", &self.layers.len())
            .field("root_count", &self.roots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::LayerTree;
    use crate::DocError;
    use crate::layer::{BlendMode, Layer, LayerKind, LayerLock, LayerMask};
    use aurora_core::{Id, Rect};

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    // Exact-literal round-trip through real postcard bytes, no
    // arithmetic -- same reasoning this crate's other float_cmp allows
    // already document.
    #[allow(clippy::float_cmp)]
    fn layer_tree_round_trips_through_real_postcard_bytes() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_opacity(child, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_blend_mode(child, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.add_mask(child, bounds()) {
            unreachable!("{err:?}");
        }

        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut restored: LayerTree = match postcard::from_bytes(&bytes) {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(restored.roots(), &[group]);
        assert_eq!(restored.children(group), Some([child].as_slice()));
        assert_eq!(restored.opacity(child), Some(0.5));
        assert_eq!(restored.blend_mode(child), Some(BlendMode::Multiply));
        assert_eq!(
            restored.mask(child),
            Some(&LayerMask {
                bounds: bounds(),
                enabled: true,
                inverted: false,
            })
        );

        // The restored tree's own id generator must keep counting from
        // where the saved one left off -- not restart from 0 and
        // eventually hand out an id (`group`'s or `child`'s) that
        // already exists.
        let fresh = match restored.add_pixel_layer("new", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(fresh, group);
        assert_ne!(fresh, child);
    }

    #[test]
    fn new_tree_is_empty() {
        let tree = LayerTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert!(tree.roots().is_empty());
    }

    #[test]
    fn add_pixel_layer_and_group_at_root_newest_on_top() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_group("b", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // b was added after a, so it must be on top (index 0).
        assert_eq!(tree.roots(), [b, a]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.name(a), Some("a"));
        assert_eq!(tree.name(b), Some("b"));
    }

    #[test]
    fn add_pixel_layer_records_its_bounds_via_kind() {
        let mut tree = LayerTree::new();
        let rect = bounds();
        let id = match tree.add_pixel_layer("a", rect, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.kind(id), Some(&LayerKind::Pixel { bounds: rect }));
    }

    #[test]
    fn surface_id_of_a_pixel_layer_reuses_its_own_raw_layer_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.surface_id(id),
            Some(aurora_tile::SurfaceId::from_raw(id.to_raw()))
        );
    }

    #[test]
    fn surface_id_is_none_for_a_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.surface_id(group), None);
    }

    #[test]
    fn surface_id_is_none_for_an_unknown_id() {
        let tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        assert_eq!(tree.surface_id(bogus), None);
    }

    #[test]
    fn surface_id_is_distinct_for_two_different_pixel_layers() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(tree.surface_id(a), tree.surface_id(b));
    }

    #[test]
    fn paint_order_is_bottom_to_top_the_reverse_of_roots() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots() is top-to-bottom, newest on top: [b, a].
        assert_eq!(tree.roots(), [b, a]);
        assert_eq!(
            tree.paint_order(),
            [a, b],
            "paint order is bottom-to-top: a first, b drawn over it"
        );
    }

    #[test]
    fn paint_order_skips_a_hidden_root() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_visible(a, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.paint_order(), [b]);
    }

    #[test]
    fn paint_order_skips_a_root_level_group() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_group("g", None) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.paint_order(), [a]);
    }

    #[test]
    fn paint_order_includes_a_layer_nested_inside_a_visible_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.paint_order(),
            [child],
            "a layer nested inside a visible group must now appear in paint order"
        );
    }

    #[test]
    fn paint_order_interleaves_a_groups_contents_at_its_own_stacking_position() {
        // Tree shape (roots, top-to-bottom): [top, group, bottom], with
        // group containing [g_top, g_bottom] (also top-to-bottom).
        //
        // Expected bottom-to-top paint order: the group "unpacks" at its
        // own position among its root-level siblings, so `bottom` (below
        // the group) comes first, then the group's own contents
        // bottom-to-top (g_bottom, g_top), then `top` (above the group)
        // last.
        let mut tree = LayerTree::new();
        let bottom = match tree.add_pixel_layer("bottom", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match tree.add_pixel_layer("top", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots() is top-to-bottom, newest on top: [top, group, bottom].
        assert_eq!(tree.roots(), [top, group, bottom]);

        let g_bottom = match tree.add_pixel_layer("g_bottom", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let g_top = match tree.add_pixel_layer("g_top", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // children(group) is top-to-bottom, newest on top: [g_top, g_bottom].
        assert_eq!(tree.children(group), Some([g_top, g_bottom].as_slice()));

        assert_eq!(tree.paint_order(), [bottom, g_bottom, g_top, top]);
    }

    #[test]
    fn paint_order_excludes_a_layer_nested_inside_an_invisible_group_even_if_the_layer_itself_is_visible()
     {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // The child's own visible flag stays true (the default) -- only
        // the group is hidden.
        assert_eq!(tree.visible(child), Some(true));
        if let Err(err) = tree.set_visible(group, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.paint_order(),
            [],
            "an invisible group must hide its whole subtree regardless of a child's own visible flag"
        );
    }

    #[test]
    fn paint_order_recurses_two_levels_deep() {
        // outer(group) -> inner(group) -> leaf(pixel), all visible.
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_group("inner", Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.add_pixel_layer("leaf", bounds(), Some(inner)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.paint_order(),
            [leaf],
            "recursion must not be hardcoded to one level of nesting"
        );
    }

    #[test]
    fn paint_order_of_a_mixed_visible_and_hidden_children_group_only_includes_the_visible_ones() {
        // group -> [visible_a (top), hidden_b, visible_c (bottom)],
        // plus a second, entirely hidden group -> hidden_group_child,
        // proving a hidden group also transitively hides its own
        // visible children.
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let visible_c = match tree.add_pixel_layer("visible_c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let hidden_b = match tree.add_pixel_layer("hidden_b", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let visible_a = match tree.add_pixel_layer("visible_a", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_visible(hidden_b, false) {
            unreachable!("{err:?}");
        }
        // children(group) top-to-bottom, newest on top: [visible_a, hidden_b, visible_c].
        assert_eq!(
            tree.children(group),
            Some([visible_a, hidden_b, visible_c].as_slice())
        );

        let hidden_group = match tree.add_group("hidden_group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let hidden_group_child =
            match tree.add_pixel_layer("hidden_group_child", bounds(), Some(hidden_group)) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
        // hidden_group_child's own visible flag stays true -- only the
        // parent group is hidden.
        assert_eq!(tree.visible(hidden_group_child), Some(true));
        if let Err(err) = tree.set_visible(hidden_group, false) {
            unreachable!("{err:?}");
        }

        // roots() top-to-bottom, newest on top: [hidden_group, group].
        assert_eq!(tree.roots(), [hidden_group, group]);

        assert_eq!(
            tree.paint_order(),
            [visible_c, visible_a],
            "only the visible children appear, in their own bottom-to-top order, \
             and the hidden group's own visible child never appears at all"
        );
    }

    #[test]
    fn paint_order_never_includes_a_group_entry_itself() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let order = tree.paint_order();
        assert_eq!(order, [child]);
        assert!(
            !order.contains(&group),
            "a group is walked during recursion but must never itself be pushed"
        );
    }

    #[test]
    fn paint_order_is_empty_for_a_fresh_tree() {
        let tree = LayerTree::new();
        assert_eq!(tree.paint_order(), []);
    }

    #[test]
    fn add_nested_layer_inside_a_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(group), Some([child].as_slice()));
        assert_eq!(tree.parent(child), Some(group));
        assert_eq!(tree.roots(), [group], "child must not also be a root");
    }

    #[test]
    fn add_rejects_an_unknown_parent() {
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(41);
        match tree.add_pixel_layer("a", bounds(), Some(bogus)) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
        assert!(tree.is_empty(), "a failed add must add nothing");
    }

    #[test]
    fn add_rejects_a_non_group_parent() {
        let mut tree = LayerTree::new();
        let pixel = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.add_pixel_layer("b", bounds(), Some(pixel)) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, pixel),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "a failed add must add nothing");
    }

    #[test]
    fn kind_and_children_are_none_for_an_unknown_id() {
        let tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        assert_eq!(tree.kind(bogus), None);
        assert_eq!(tree.children(bogus), None);
        assert_eq!(tree.parent(bogus), None);
        assert_eq!(tree.name(bogus), None);
        assert!(!tree.contains(bogus));
    }

    #[test]
    fn children_is_none_for_a_pixel_layer() {
        let mut tree = LayerTree::new();
        let pixel = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.children(pixel),
            None,
            "a pixel layer has no children slot to return, unlike an empty group"
        );
    }

    #[test]
    fn children_is_some_empty_for_a_fresh_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(group), Some([].as_slice()));
    }

    #[test]
    fn set_name_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_name(id, "renamed") {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.name(id), Some("renamed"));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_name(bogus, "x") {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn remove_detaches_a_root_layer() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(a));
        assert_eq!(tree.roots(), [b]);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn remove_updates_the_parent_group_children_list() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(child) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.children(group), Some([].as_slice()));
    }

    #[test]
    fn remove_cascades_into_every_descendant() {
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_group("inner", Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.add_pixel_layer("leaf", bounds(), Some(inner)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.remove(outer) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(outer));
        assert!(!tree.contains(inner), "nested group must also be removed");
        assert!(
            !tree.contains(leaf),
            "leaf two levels down must also be removed"
        );
        assert!(tree.is_empty());
    }

    #[test]
    fn remove_rejects_an_unknown_id() {
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        match tree.remove(bogus) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn reparent_moves_a_layer_between_groups() {
        let mut tree = LayerTree::new();
        let a = match tree.add_group("a", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_group("b", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(a)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.reparent(child, Some(b), 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.children(a), Some([].as_slice()));
        assert_eq!(tree.children(b), Some([child].as_slice()));
        assert_eq!(tree.parent(child), Some(b));
    }

    #[test]
    fn reparent_can_move_a_layer_to_root() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.reparent(child, None, 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.parent(child), None);
        assert_eq!(tree.roots(), [child, group]);
    }

    #[test]
    fn reparent_reorders_within_the_same_parent() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let c = match tree.add_pixel_layer("c", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Insertion order puts newest on top: roots = [c, b, a].
        assert_eq!(tree.roots(), [c, b, a]);

        // Move a (currently bottom) to the very top.
        if let Err(err) = tree.reparent(a, None, 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.roots(), [a, c, b]);
    }

    #[test]
    fn reparent_clamps_an_out_of_range_index_to_the_end() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots = [b, a]; move b far past the end.
        if let Err(err) = tree.reparent(b, None, 999) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.roots(), [a, b]);
    }

    #[test]
    fn reparent_rejects_a_cycle_under_self() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(group, Some(group), 0) {
            Err(DocError::CycleDetected { id, new_parent }) => {
                assert_eq!(id, group);
                assert_eq!(new_parent, group);
            }
            other => unreachable!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn reparent_rejects_a_cycle_under_a_descendant() {
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_group("inner", Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(outer, Some(inner), 0) {
            Err(DocError::CycleDetected { id, new_parent }) => {
                assert_eq!(id, outer);
                assert_eq!(new_parent, inner);
            }
            other => unreachable!("expected CycleDetected, got {other:?}"),
        }
        // Must be unchanged.
        assert_eq!(tree.parent(inner), Some(outer));
        assert_eq!(tree.parent(outer), None);
    }

    #[test]
    fn reparent_rejects_a_non_group_target() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(a, Some(b), 0) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, b),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
    }

    #[test]
    fn reparent_rejects_unknown_ids() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let bogus: super::LayerId = Id::from_raw(999);

        match tree.reparent(bogus, None, 0) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
        match tree.reparent(a, Some(bogus), 0) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn default_is_an_empty_tree() {
        let tree = LayerTree::default();
        assert!(tree.is_empty());
    }

    #[test]
    fn layer_kind_is_group_distinguishes_the_two_kinds() {
        assert!(!LayerKind::Pixel { bounds: bounds() }.is_group());
        assert!(
            LayerKind::Group {
                children: Vec::new()
            }
            .is_group()
        );
    }

    #[test]
    fn fresh_layer_has_the_documented_defaults() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.opacity(id), Some(1.0));
        assert_eq!(tree.fill_opacity(id), Some(1.0));
        assert_eq!(tree.blend_mode(id), Some(BlendMode::Normal));
        assert_eq!(tree.visible(id), Some(true));
        assert_eq!(tree.lock(id), Some(LayerLock::none()));
    }

    #[test]
    // The values under test are the exact literals passed a few lines
    // above, round-tripped through `DocError::OpacityOutOfRange` with no
    // arithmetic in between -- exact comparison is correct here, not the
    // "accumulated rounding error" case clippy::float_cmp warns about
    // (same reasoning `aurora_tile::store`'s own tests already document).
    #[allow(clippy::float_cmp)]
    fn set_opacity_updates_and_rejects_out_of_range() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.set_opacity(id, 0.5) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.opacity(id), Some(0.5));

        match tree.set_opacity(id, 1.5) {
            Err(DocError::OpacityOutOfRange(v)) => assert_eq!(v, 1.5),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
        // A rejected value must not have been applied.
        assert_eq!(tree.opacity(id), Some(0.5));

        match tree.set_opacity(id, -0.1) {
            Err(DocError::OpacityOutOfRange(v)) => assert_eq!(v, -0.1),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_opacity(bogus, 0.5) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    // Same exact-literal-round-trip reasoning as
    // `set_opacity_updates_and_rejects_out_of_range` above.
    #[allow(clippy::float_cmp)]
    fn set_fill_opacity_updates_independently_of_opacity() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.set_fill_opacity(id, 0.25) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.fill_opacity(id), Some(0.25));
        assert_eq!(tree.opacity(id), Some(1.0), "must not affect layer opacity");

        match tree.set_fill_opacity(id, 2.0) {
            Err(DocError::OpacityOutOfRange(v)) => assert_eq!(v, 2.0),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn set_blend_mode_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_blend_mode(id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.blend_mode(id), Some(BlendMode::Multiply));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_blend_mode(bogus, BlendMode::Screen) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn bounds_reads_a_pixel_layers_own_bounds_and_none_for_a_group() {
        let mut tree = LayerTree::new();
        let pixel = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.bounds(pixel), Some(bounds()));
        assert_eq!(tree.bounds(group), None);
    }

    #[test]
    fn set_bounds_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let moved = Rect {
            x: 100,
            y: 200,
            width: 30,
            height: 40,
        };
        if let Err(err) = tree.set_bounds(id, moved) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.kind(id), Some(&LayerKind::Pixel { bounds: moved }));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_bounds(bogus, moved) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn set_bounds_rejects_a_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.set_bounds(group, bounds()) {
            Err(DocError::NotAPixelLayer(got)) => assert_eq!(got, group),
            other => unreachable!("expected NotAPixelLayer, got {other:?}"),
        }
    }

    #[test]
    fn set_visible_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_visible(id, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.visible(id), Some(false));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_visible(bogus, true) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn set_lock_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_lock(id, LayerLock::all()) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.lock(id), Some(LayerLock::all()));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_lock(bogus, LayerLock::none()) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn layer_lock_none_and_all_and_is_any() {
        assert!(!LayerLock::none().is_any());
        assert_eq!(LayerLock::none(), LayerLock::default());
        let all = LayerLock::all();
        assert!(all.transparency && all.pixels && all.position);
        assert!(all.is_any());

        let partial = LayerLock {
            transparency: true,
            pixels: false,
            position: false,
        };
        assert!(partial.is_any());
    }

    #[test]
    fn fresh_layer_has_no_mask() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.mask(id), None);
    }

    #[test]
    fn add_mask_creates_an_enabled_uninverted_mask_and_rejects_duplicates() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_bounds = bounds();

        if let Err(err) = tree.add_mask(id, mask_bounds) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.mask(id),
            Some(&LayerMask {
                bounds: mask_bounds,
                enabled: true,
                inverted: false,
            })
        );

        match tree.add_mask(id, mask_bounds) {
            Err(DocError::MaskAlreadyExists(got)) => assert_eq!(got, id),
            other => unreachable!("expected MaskAlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn add_mask_works_on_a_group_too() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_mask(group, bounds()) {
            unreachable!("{err:?}");
        }
        assert!(tree.mask(group).is_some());
    }

    #[test]
    fn add_mask_rejects_an_unknown_layer() {
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(999);
        match tree.add_mask(bogus, bounds()) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn remove_mask_clears_it_and_rejects_when_absent() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match tree.remove_mask(id) {
            Err(DocError::NoMask(got)) => assert_eq!(got, id),
            other => unreachable!("expected NoMask, got {other:?}"),
        }

        if let Err(err) = tree.add_mask(id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.remove_mask(id) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id), None);

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.remove_mask(bogus) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn removing_a_layer_takes_its_mask_with_it() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_mask(id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.remove(id) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(id));
    }

    #[test]
    fn set_mask_enabled_updates_and_rejects_when_absent_or_unknown() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match tree.set_mask_enabled(id, false) {
            Err(DocError::NoMask(got)) => assert_eq!(got, id),
            other => unreachable!("expected NoMask, got {other:?}"),
        }

        if let Err(err) = tree.add_mask(id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_mask_enabled(id, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id).map(|m| m.enabled), Some(false));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_mask_enabled(bogus, true) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn set_mask_inverted_updates_and_rejects_when_absent_or_unknown() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match tree.set_mask_inverted(id, true) {
            Err(DocError::NoMask(got)) => assert_eq!(got, id),
            other => unreachable!("expected NoMask, got {other:?}"),
        }

        if let Err(err) = tree.add_mask(id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_mask_inverted(id, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.mask(id).map(|m| m.inverted), Some(true));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_mask_inverted(bogus, true) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    /// Not a functional test -- `Layer` is a zero-variant marker type only
    /// ever named via `PhantomData` (see `aurora_core::Id`'s own doc
    /// comment). This just confirms the type is actually exported and
    /// usable as `Id<Layer>` from outside the crate, the way `LayerId`
    /// itself is defined.
    #[test]
    fn layer_marker_type_is_exported() {
        let _id: Id<Layer> = Id::from_raw(0);
    }
}
