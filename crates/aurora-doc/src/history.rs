//! History: reversible operations plus dirtied regions, unlimited
//! undo/redo (§7.3.3), and an in-memory crash-recovery journal.
//! PLAN.md M1.4's fifth and sixth pieces.
//!
//! [`History`] mirrors every mutating [`LayerTree`] method with one that
//! also records how to undo it. It does not wrap [`LayerTree`] (own it,
//! `Deref` to it, etc.) — there is no `Document` type yet tying a tree,
//! a selection set, and a history together, so `History` and `LayerTree`
//! stay siblings, each call taking `&mut LayerTree` explicitly. A future
//! `Document` can compose them.
//!
//! **The journal, and what's deliberately not built yet.** Every op
//! `History` ever applies — whether from a fresh call, an undo, or a
//! redo — is also appended, in real chronological order, to an
//! ever-growing in-memory log. [`History::replay`] rebuilds a fresh
//! `LayerTree` purely from that log, proving it's a sufficient,
//! order-correct record of the *current* state (not the undo stack's
//! shape — undoing something and never redoing it means the journal's
//! replay reflects the undone state, matching what the user actually has
//! open). This is the risky, easy-to-get-subtly-wrong half of "crash
//! recovery journal" (§7.3.3), and it's done and tested. **What's
//! deliberately not here: writing this journal to disk**, which is what
//! would actually let it survive the crash it's named for. That needs a
//! chosen on-disk encoding for `LayerOp`'s recursive shape (nested
//! entries, strings, ids) — a real, first-party format decision, same
//! *kind* of choice as `aurora-tile`'s own hand-rolled tile codec, but
//! this pass didn't reach it, and forcing one without evidence is exactly
//! the mistake `spike/raw-icc/FINDINGS.md` already caught once (a
//! "small, fast" persistence detail turning out to need its own real
//! design pass). Tracked as the next step on this bullet, not silently
//! skipped.

use aurora_core::Rect;

use crate::error::DocError;
use crate::layer::{BlendMode, LayerEntry, LayerId, LayerKind, LayerLock, LayerMask};
use crate::tree::{LayerTree, RemovedSubtree};

/// One recorded step. On the undo/redo stacks, stored as *how to undo the
/// step that's currently on top* — never "what the user did," which
/// would need a separate, parallel "how to undo it" derivation at undo
/// time. Applying an op (see [`apply`]) both performs it and returns its
/// own inverse, which is exactly what the opposite stack needs — so
/// `undo` and `redo` share one function. In `History`'s own journal
/// (a separate, ever-growing `Vec`, not one of the two stacks), the same
/// type instead records *what was just applied*, in real chronological
/// order — see [`History::replay`].
///
/// Every variant stores just the one changed value (or, for a structural
/// change, exactly the removed subtree) — never a whole-document
/// snapshot (§7.3.3).
///
/// `Clone`: the journal needs its own independent copy of each op (the
/// stacks' copies get consumed by [`apply`]).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
    /// Appended last, not alongside the other `Set*` variants above —
    /// `LayerOp` is `postcard`-serialized (ADR 0009, the crash-recovery
    /// journal/autosave), and postcard encodes an enum variant by its
    /// ordinal position; inserting a new variant in the *middle* would
    /// silently reinterpret every later variant in an old, already-
    /// written journal as the wrong op. Appending is always safe.
    SetBounds {
        id: LayerId,
        value: Rect,
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
// One match arm per `LayerOp` variant, each following the exact same
// "read the old value, apply, return the inverse" shape -- splitting
// this up would just relocate the same lines into several
// same-length-total functions, not reduce real complexity.
#[allow(clippy::too_many_lines)]
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
        LayerOp::SetBounds { id, value } => {
            let old = tree.bounds(id).ok_or(DocError::UnknownLayer(id))?;
            tree.set_bounds(id, value)?;
            // The dirty region must cover *both* the old and new bounds
            // -- undoing/redoing a move needs whatever the layer used to
            // cover repainted too, not just where it ends up, or the
            // canvas would show a stale copy left behind at the old
            // position.
            Ok((
                LayerOp::SetBounds { id, value: old },
                Some(old.union(&value)),
            ))
        }
    }
}

/// A one-line, human-readable description of one journal entry — see
/// [`History::journal_descriptions`]'s own doc comment for why this
/// deliberately doesn't take a `&LayerTree` to resolve names beyond
/// what an entry itself already captured.
fn describe(op: &LayerOp) -> String {
    match op {
        LayerOp::RemoveById(id) => format!("Removed layer #{}", id.to_raw()),
        LayerOp::Restore(removed) => {
            let name = removed
                .entries
                .iter()
                .find(|(entry_id, _)| *entry_id == removed.root)
                .map_or("layer", |(_, entry)| entry.name.as_str());
            format!("Added layer \"{name}\"")
        }
        LayerOp::Reparent { id, .. } => format!("Moved layer #{}", id.to_raw()),
        LayerOp::Rename { id, name } => {
            format!("Renamed layer #{} to \"{name}\"", id.to_raw())
        }
        LayerOp::SetOpacity { id, value } => {
            format!(
                "Set opacity of layer #{} to {}%",
                id.to_raw(),
                percent(*value)
            )
        }
        LayerOp::SetFillOpacity { id, value } => {
            format!(
                "Set fill opacity of layer #{} to {}%",
                id.to_raw(),
                percent(*value)
            )
        }
        LayerOp::SetBlendMode { id, value } => {
            format!("Set blend mode of layer #{} to {value:?}", id.to_raw())
        }
        LayerOp::SetVisible { id, value } => {
            let verb = if *value { "Shown" } else { "Hidden" };
            format!("{verb} layer #{}", id.to_raw())
        }
        LayerOp::SetLock { id, .. } => {
            format!("Changed lock settings for layer #{}", id.to_raw())
        }
        LayerOp::RemoveMask(id) => format!("Removed mask from layer #{}", id.to_raw()),
        LayerOp::RestoreMask(id, _) => format!("Added mask to layer #{}", id.to_raw()),
        LayerOp::SetMaskEnabled { id, value } => {
            let verb = if *value { "Enabled" } else { "Disabled" };
            format!("{verb} mask on layer #{}", id.to_raw())
        }
        LayerOp::SetMaskInverted { id, value } => {
            let verb = if *value { "Inverted" } else { "Un-inverted" };
            format!("{verb} mask on layer #{}", id.to_raw())
        }
        // "Repositioned," not "Moved" -- `Reparent` already claims that
        // verb for changing a layer's place in the tree/z-order, a
        // different operation from changing its own on-canvas bounds.
        LayerOp::SetBounds { id, value } => {
            format!(
                "Repositioned layer #{} to ({}, {})",
                id.to_raw(),
                value.x,
                value.y
            )
        }
    }
}

/// `0.0..=1.0` as a rounded whole percentage, for a description string.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percent(value: f32) -> i32 {
    (value * 100.0).round() as i32
}

/// Unlimited undo/redo over a [`LayerTree`] (§7.3.3): every mutating
/// `LayerTree` method has a mirror here that performs the same change
/// and records how to reverse it. A step recorded through this type's
/// own methods can always be undone; a `LayerTree` mutation made by
/// calling the tree directly (bypassing `History`) is invisible to it,
/// and mixing the two can leave a recorded step referring to a layer (or
/// position) that direct calls already changed out from under it -- see
/// `LayerTree::restore`'s own doc comment for the specific errors that
/// can then surface. Normal use (only ever mutating through one
/// `History`) never hits this.
///
/// New activity through this type's own methods always clears the redo
/// stack, matching every mainstream editor's undo/redo behaviour.
///
/// Also keeps the in-memory crash-recovery journal described in this
/// module's own doc comment — a separate, ever-growing log, unaffected by
/// undo/redo clearing the redo stack.
pub struct History {
    undo_stack: Vec<LayerOp>,
    redo_stack: Vec<LayerOp>,
    journal: Vec<LayerOp>,
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            journal: Vec::new(),
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

    /// Discards every redo entry without touching `undo_stack` or the
    /// journal — the same "new activity invalidates redo" clearing this
    /// type's own private `push` helper already does internally for a
    /// structural edit
    /// recorded *through* this type, exposed here for a caller that
    /// needs to invalidate this history's own redo stack in response to
    /// activity recorded somewhere else entirely (`aurora-app`'s own
    /// unified undo ordering across this type and
    /// `aurora_brush::PixelHistory`, PLAN.md's Undo/Redo bullet: doing a
    /// pixel edit must invalidate a pending structural redo too, not
    /// just this history's own).
    pub fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    /// How many ops the journal has recorded so far — mostly useful for
    /// tests; see [`Self::journal_descriptions`] for reading individual
    /// entries.
    #[must_use]
    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    /// One human-readable, one-line description per journal entry, in
    /// the same chronological order [`Self::replay`] itself uses — what
    /// a History panel actually shows (PLAN.md M1.8's own "History...
    /// panels" bullet). Not a `Display`/`Debug` impl on `LayerOp`
    /// itself, which stays private (its exact shape is this crate's own
    /// implementation detail, not something a caller should match on)
    /// — this is the one, deliberate seam through which its content
    /// becomes externally visible.
    ///
    /// Deliberately self-contained (no `&LayerTree` parameter): a
    /// description only ever names the one layer a `Rename`/`Restore`
    /// entry itself captured a name for, falling back to a numeric
    /// `layer #N` reference otherwise — accepting less friendly text
    /// over the alternative of resolving names against a live tree
    /// (which layer names have changed since is genuinely ambiguous —
    /// Photoshop's own History panel shows a name as of when the step
    /// happened, not retroactively updated later ones — and `History`
    /// deliberately doesn't hold a tree reference of its own; see this
    /// module's own doc comment).
    #[must_use]
    pub fn journal_descriptions(&self) -> Vec<String> {
        self.journal.iter().map(describe).collect()
    }

    /// Rebuilds a fresh [`LayerTree`] purely by replaying this history's
    /// own journal from empty, in the exact order every op was actually
    /// applied — see this module's own doc comment.
    ///
    /// The rebuilt tree is checked before it is handed back, with the
    /// *same* validator `LayerTree`'s own `Deserialize` runs on a `.aur`
    /// manifest (`LayerTree::validate`). To be clear about what that
    /// does and does not close today: **nothing outside this crate's own
    /// tests calls `replay` yet**, so this is not a currently-live hole
    /// being plugged. It will matter as soon as undo/redo can be seeded
    /// from a file — a journal loaded by [`Self::load_journal`] is
    /// untrusted bytes from exactly the same file the manifest came
    /// from, and replay reaches a live `LayerTree` *without* going
    /// through that `Deserialize` impl (it starts from
    /// [`LayerTree::new`] and applies recorded ops), so validating only
    /// the manifest would leave that second door open. The bar is set
    /// now, while the path is still short, rather than retrofitted onto
    /// the first caller.
    ///
    /// # Errors
    ///
    /// Returns an error if replaying the journal fails, or if the tree
    /// it produces is not structurally a tree — neither should happen
    /// for a `History` only ever mutated through its own methods (see
    /// this type's own doc comment about mixing in direct `LayerTree`
    /// calls); both are reachable from a crafted journal.
    pub fn replay(&self) -> Result<LayerTree, DocError> {
        let mut tree = LayerTree::new();
        for op in self.journal.clone() {
            apply(&mut tree, op)?;
        }
        tree.validate()?;
        Ok(tree)
    }

    /// Serializes this history's own journal — via `postcard`, ADR
    /// 0009's own decision for `.aur`'s manifest/history encoding — the
    /// on-disk encoding this crate's own doc comment named as the
    /// missing piece of crash-recovery persistence. Deliberately just
    /// the journal, not the undo/redo stacks: [`Self::replay`]'s own
    /// doc comment already proves the journal alone is a sufficient,
    /// order-correct record of *current* state, and a crash doesn't
    /// need to preserve exactly how many times the user pressed undo
    /// along the way.
    ///
    /// **Scope, stated honestly**: this returns plain `postcard` bytes,
    /// not a full `.aur` file — ADR 0009's real ZIP container (with a
    /// `mimetype` sentinel, a manifest, and per-tile entries) has
    /// nothing else to hold yet (no layer owns real pixel storage, and
    /// there is no document-level manifest beyond what the journal
    /// itself already encodes), so wrapping one entry in a container
    /// with no siblings would be premature scaffolding. Building the
    /// real container is separate, follow-on work for whenever a
    /// manifest/tile data exist to go alongside this.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::JournalSerialization`] if `postcard` itself
    /// fails — not expected for this crate's own, entirely
    /// serializable `LayerOp` shape, but a real, checked possibility
    /// rather than an assumption.
    pub fn save_journal(&self) -> Result<Vec<u8>, DocError> {
        postcard::to_allocvec(&self.journal)
            .map_err(|source| DocError::JournalSerialization(source.to_string()))
    }

    /// Reconstructs a `History` from a journal previously produced by
    /// [`Self::save_journal`]. The undo/redo stacks start empty — the
    /// same "can't undo past the recovery point" behaviour real
    /// applications already have after crash recovery — with the
    /// recovered journal itself intact for [`Self::replay`]/
    /// [`Self::journal_descriptions`].
    ///
    /// **This deserializes, it does not validate.** `bytes` are
    /// untrusted — a `.aur` file's own `history` entry, from whoever
    /// sent the file — and the ops they decode into are taken at face
    /// value here; nothing checks that replaying them yields a coherent
    /// tree. [`Self::replay`] is where that bar is enforced, so a
    /// caller that loads a journal and never replays it has checked
    /// nothing beyond "these bytes are well-formed `postcard`".
    ///
    /// # Errors
    ///
    /// Returns [`DocError::JournalDeserialization`] if `bytes` isn't a
    /// valid, `postcard`-encoded journal (e.g. corrupted, truncated, or
    /// from an incompatible future version — see ADR 0009's own
    /// forward-compatibility policy for the container format this will
    /// eventually be embedded in).
    pub fn load_journal(bytes: &[u8]) -> Result<Self, DocError> {
        let journal: Vec<LayerOp> = postcard::from_bytes(bytes)
            .map_err(|source| DocError::JournalDeserialization(source.to_string()))?;
        Ok(Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            journal,
        })
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
        let name = name.into();
        let id = tree.add_pixel_layer(name.clone(), bounds, parent)?;
        self.journal.push(LayerOp::Restore(RemovedSubtree {
            root: id,
            parent,
            index: 0,
            entries: vec![(
                id,
                LayerEntry::new(name, parent, LayerKind::Pixel { bounds }),
            )],
        }));
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
        let name = name.into();
        let id = tree.add_group(name.clone(), parent)?;
        self.journal.push(LayerOp::Restore(RemovedSubtree {
            root: id,
            parent,
            index: 0,
            entries: vec![(
                id,
                LayerEntry::new(
                    name,
                    parent,
                    LayerKind::Group {
                        children: Vec::new(),
                    },
                ),
            )],
        }));
        self.push(LayerOp::RemoveById(id));
        Ok(id)
    }

    /// Same as [`LayerTree::remove`], recorded for undo. Returns the
    /// region the removal dirtied, if known (see `layer_dirty_rect`).
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::remove`].
    pub fn remove(&mut self, tree: &mut LayerTree, id: LayerId) -> Result<Option<Rect>, DocError> {
        let removed = tree.remove_capturing(id)?;
        let dirty = subtree_dirty_rect(&removed);
        self.journal.push(LayerOp::RemoveById(id));
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
        self.journal.push(LayerOp::Reparent {
            id,
            parent: new_parent,
            index,
        });
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
        let name = name.into();
        let old = tree.name(id).ok_or(DocError::UnknownLayer(id))?.to_owned();
        tree.set_name(id, name.clone())?;
        self.journal.push(LayerOp::Rename { id, name });
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
        self.journal.push(LayerOp::SetOpacity { id, value });
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
        self.journal.push(LayerOp::SetFillOpacity { id, value });
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
        self.journal.push(LayerOp::SetBlendMode { id, value });
        self.push(LayerOp::SetBlendMode { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Same as [`LayerTree::set_bounds`], recorded for undo — the Move
    /// tool's own document-model support.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::set_bounds`].
    pub fn set_bounds(
        &mut self,
        tree: &mut LayerTree,
        id: LayerId,
        value: Rect,
    ) -> Result<Option<Rect>, DocError> {
        let old = tree.bounds(id).ok_or(DocError::UnknownLayer(id))?;
        tree.set_bounds(id, value)?;
        self.journal.push(LayerOp::SetBounds { id, value });
        self.push(LayerOp::SetBounds { id, value: old });
        Ok(Some(old.union(&value)))
    }

    /// Records a bounds change for `id` from `old` to whatever `tree`
    /// currently shows, as a single undo step — unlike [`Self::set_bounds`],
    /// this never touches `tree` itself. For a caller that already
    /// applied the real change directly (`aurora-app`'s own Move drag:
    /// live visual feedback on every pointer-move event, without
    /// recording an undo step for each one) and wants exactly one undo
    /// entry for the whole gesture, retroactively, once it completes.
    /// Journals `SetBounds { value: tree.bounds(id) }` — the tree's own
    /// current, real state, matching every other journal entry's own
    /// "what was actually applied" meaning — and pushes its inverse,
    /// `SetBounds { value: old }`, onto the undo stack, so undoing
    /// restores `old` in one step regardless of how many intermediate
    /// positions the live gesture actually passed through.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't currently name
    /// a real layer in `tree`.
    pub fn record_bounds_change(
        &mut self,
        tree: &LayerTree,
        id: LayerId,
        old: Rect,
    ) -> Result<(), DocError> {
        let current = tree.bounds(id).ok_or(DocError::UnknownLayer(id))?;
        self.journal.push(LayerOp::SetBounds { id, value: current });
        self.push(LayerOp::SetBounds { id, value: old });
        Ok(())
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
        self.journal.push(LayerOp::SetVisible { id, value });
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
        self.journal.push(LayerOp::SetLock { id, value });
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
        self.journal.push(LayerOp::RestoreMask(
            id,
            LayerMask {
                bounds,
                enabled: true,
                inverted: false,
            },
        ));
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
        self.journal.push(LayerOp::RemoveMask(id));
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
        self.journal.push(LayerOp::SetMaskEnabled { id, value });
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
        self.journal.push(LayerOp::SetMaskInverted { id, value });
        self.push(LayerOp::SetMaskInverted { id, value: old });
        Ok(layer_dirty_rect(tree, id))
    }

    /// Undoes the most recent step, if any. `None` both when there was
    /// nothing to undo, and when there was but its dirtied region isn't
    /// knowable (see `layer_dirty_rect`) — same conflated shape
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
        let forward = op.clone();
        // A failed `apply` puts the step back where it came from. The
        // `?` this replaced dropped it from the undo stack *and* never
        // pushed it to the redo stack, so a single refused undo silently
        // cost the user that step forever -- and this round adds several
        // new ways for `apply` to refuse (see `LayerTree::reparent`'s
        // and `restore`'s own error lists), which widens a window that
        // was previously only reachable by mixing direct `LayerTree`
        // calls into a `History`-managed tree. Every `LayerTree` call
        // `apply` makes is all-or-nothing, so the tree is untouched too.
        let (inverse, dirty) = match apply(tree, op) {
            Ok(applied) => applied,
            Err(err) => {
                self.undo_stack.push(forward);
                return Err(err);
            }
        };
        self.journal.push(forward);
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
        let forward = op.clone();
        // Same restore-on-failure as `undo` above, for the same reason.
        let (inverse, dirty) = match apply(tree, op) {
            Ok(applied) => applied,
            Err(err) => {
                self.redo_stack.push(forward);
                return Err(err);
            }
        };
        self.journal.push(forward);
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
            .field("journal_len", &self.journal.len())
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
    fn replay_of_an_empty_history_is_an_empty_tree() {
        let history = History::new();
        assert_eq!(history.journal_len(), 0);
        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(replayed.is_empty());
    }

    #[test]
    fn replay_reconstructs_a_simple_add_with_the_same_id_and_bounds() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(replayed.contains(id));
        assert_eq!(
            replayed.kind(id),
            Some(&LayerKind::Pixel { bounds: bounds() })
        );
        assert_eq!(replayed.roots(), tree.roots());
    }

    #[test]
    fn replay_reflects_current_state_after_an_undo_not_the_original_history() {
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
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        // Live tree now has only `a` -- `b` was undone.
        assert!(!tree.contains(b));

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            replayed.contains(a),
            "the surviving layer must still be there"
        );
        assert!(
            !replayed.contains(b),
            "replay must reflect the undone state, not resurrect what the user undid"
        );
        assert_eq!(replayed.roots(), tree.roots());
    }

    #[test]
    fn replay_reflects_a_redo_bringing_a_layer_back() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(replayed.contains(id));
    }

    #[test]
    // Exact-literal round-trip, no arithmetic -- same reasoning as
    // `tree::tests`' own float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn replay_reconstructs_property_changes() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut tree, id, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_blend_mode(&mut tree, id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_visible(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_lock(&mut tree, id, LayerLock::all()) {
            unreachable!("{err:?}");
        }

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(replayed.opacity(id), Some(0.5));
        assert_eq!(replayed.blend_mode(id), Some(BlendMode::Multiply));
        assert_eq!(replayed.visible(id), Some(false));
        assert_eq!(replayed.lock(id), Some(LayerLock::all()));
    }

    #[test]
    fn replay_reconstructs_a_nested_group_with_correct_parent_links() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let group = match history.add_group(&mut tree, "g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match history.add_pixel_layer(&mut tree, "c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(replayed.parent(child), Some(group));
        assert_eq!(replayed.children(group), Some([child].as_slice()));
    }

    #[test]
    fn replay_reconstructs_a_mask_with_its_exact_state() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, id, other_bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_mask_inverted(&mut tree, id, true) {
            unreachable!("{err:?}");
        }

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask = replayed
            .mask(id)
            .unwrap_or_else(|| unreachable!("mask must survive replay"));
        assert_eq!(mask.bounds, other_bounds());
        assert!(mask.enabled);
        assert!(mask.inverted);
    }

    #[test]
    fn journal_len_grows_with_every_action_undo_and_redo() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        assert_eq!(history.journal_len(), 0);

        if let Err(err) = history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            unreachable!("{err:?}");
        }
        assert_eq!(history.journal_len(), 1);

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(history.journal_len(), 2, "undo is itself a journaled step");

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(history.journal_len(), 3, "so is redo");
    }

    #[test]
    fn journal_descriptions_names_added_layers_and_reports_state_changes() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let id = match history.add_pixel_layer(&mut tree, "Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut tree, id, 0.8) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_blend_mode(&mut tree, id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.set_visible(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }

        let descriptions = history.journal_descriptions();
        assert_eq!(descriptions.len(), 5, "4 actions plus the undo itself");
        let Some(added) = descriptions.first() else {
            unreachable!("just asserted len() == 5");
        };
        assert_eq!(added, "Added layer \"Background\"");
        let Some(opacity) = descriptions.get(1) else {
            unreachable!("just asserted len() == 5");
        };
        assert!(opacity.contains("80%"), "{descriptions:?}");
        let Some(blend) = descriptions.get(2) else {
            unreachable!("just asserted len() == 5");
        };
        assert!(blend.contains("Multiply"), "{descriptions:?}");
        let Some(hidden) = descriptions.get(3) else {
            unreachable!("just asserted len() == 5");
        };
        assert_eq!(hidden, &format!("Hidden layer #{}", id.to_raw()));
        // The undo of "Hidden" is itself journaled as the inverse action
        // actually applied -- "Shown", not a second "Hidden" entry and
        // not some special "undo" marker (History's own doc comment:
        // the journal records what was *applied*, whichever stack it
        // came from).
        let Some(shown) = descriptions.get(4) else {
            unreachable!("just asserted len() == 5");
        };
        assert_eq!(shown, &format!("Shown layer #{}", id.to_raw()));
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
    fn clear_redo_discards_a_pending_redo_without_touching_undo() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(history.can_redo());
        assert!(!tree.contains(id));

        history.clear_redo();

        assert!(!history.can_redo(), "the pending redo must be discarded");
        match history.redo(&mut tree) {
            Ok(None) => {}
            other => unreachable!("expected Ok(None), got {other:?}"),
        }
        assert!(
            !tree.contains(id),
            "clear_redo must not have reapplied anything"
        );
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
    fn set_bounds_undo_redo_round_trips_and_dirties_the_union_of_old_and_new() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let moved = Rect {
            x: 100,
            y: 100,
            width: 10,
            height: 10,
        };

        let dirty = match history.set_bounds(&mut tree, id, moved) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            dirty,
            Some(bounds().union(&moved)),
            "must dirty both where the layer used to be and where it ends up"
        );
        assert_eq!(tree.bounds(id), Some(moved));

        let dirty = match history.undo(&mut tree) {
            Ok(dirty) => dirty,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(dirty, Some(bounds().union(&moved)));
        assert_eq!(tree.bounds(id), Some(bounds()));

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.bounds(id), Some(moved));
    }

    #[test]
    fn set_bounds_rejects_a_group() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_group(&mut tree, "g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // `UnknownLayer`, not `NotAPixelLayer` -- this method reads the
        // *old* bounds first (`tree.bounds(id)`, `None` for a group) and
        // rejects there, before ever reaching `LayerTree::set_bounds`'s
        // own `NotAPixelLayer` check (already covered directly by
        // `tree::tests::set_bounds_rejects_a_group`).
        match history.set_bounds(&mut tree, id, bounds()) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, id),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn record_bounds_change_records_one_undo_step_covering_a_change_already_applied() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let moved = Rect {
            x: 100,
            y: 100,
            width: 10,
            height: 10,
        };
        // Applied directly, bypassing History entirely -- the same
        // "live drag feedback" shape `aurora-app` uses.
        if let Err(err) = tree.set_bounds(id, moved) {
            unreachable!("{err:?}");
        }
        let journal_len_before = history.journal_len();

        if let Err(err) = history.record_bounds_change(&tree, id, bounds()) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            history.journal_len(),
            journal_len_before + 1,
            "exactly one journal entry for the whole gesture, not one per intermediate position"
        );
        assert!(history.can_undo());
        assert!(!history.can_redo());

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.bounds(id),
            Some(bounds()),
            "undo must restore the pre-gesture bounds in one step"
        );

        if let Err(err) = history.redo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.bounds(id), Some(moved), "redo must reapply the move");
    }

    #[test]
    fn record_bounds_change_rejects_an_unknown_id() {
        let tree = LayerTree::new();
        let mut history = History::new();
        let bogus: super::LayerId = Id::from_raw(999);
        match history.record_bounds_change(&tree, bogus, bounds()) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn journal_describes_set_bounds_distinctly_from_reparent() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_bounds(
            &mut tree,
            id,
            Rect {
                x: 5,
                y: 7,
                width: 10,
                height: 10,
            },
        ) {
            unreachable!("{err:?}");
        }
        let descriptions = history.journal_descriptions();
        let Some(last) = descriptions.last() else {
            unreachable!("just recorded one action");
        };
        assert!(
            last.starts_with("Repositioned"),
            "must use a verb distinct from Reparent's own \"Moved\": {last:?}"
        );
        assert!(last.contains("(5, 7)"), "{last:?}");
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

    // -- save_journal / load_journal (ADR 0009) --

    #[test]
    fn save_then_load_journal_round_trips_journal_len_and_descriptions() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_blend_mode(&mut tree, id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }

        let bytes = match history.save_journal() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let recovered = match History::load_journal(&bytes) {
            Ok(history) => history,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(recovered.journal_len(), history.journal_len());
        assert_eq!(
            recovered.journal_descriptions(),
            history.journal_descriptions()
        );
    }

    #[test]
    fn load_journal_replays_into_the_same_tree_shape_as_the_original() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut tree, id, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.add_mask(&mut tree, id, other_bounds()) {
            unreachable!("{err:?}");
        }

        let bytes = match history.save_journal() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let recovered = match History::load_journal(&bytes) {
            Ok(history) => history,
            Err(err) => unreachable!("{err:?}"),
        };
        let replayed = match recovered.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };

        assert!(replayed.contains(id));
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(replayed.opacity(id), Some(0.5));
        }
        let Some(mask) = replayed.mask(id) else {
            unreachable!("mask must survive save_journal/load_journal/replay");
        };
        assert_eq!(mask.bounds, other_bounds());
    }

    #[test]
    fn load_journal_starts_with_empty_undo_redo_stacks() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        if let Err(err) = history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            unreachable!("{err:?}");
        }
        assert!(history.can_undo(), "sanity check: the original can undo");

        let bytes = match history.save_journal() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let recovered = match History::load_journal(&bytes) {
            Ok(history) => history,
            Err(err) => unreachable!("{err:?}"),
        };

        assert!(
            !recovered.can_undo(),
            "recovery cannot undo past the point it recovered from"
        );
        assert!(!recovered.can_redo());
    }

    #[test]
    fn load_journal_rejects_garbage() {
        match History::load_journal(b"not a real journal") {
            Err(DocError::JournalDeserialization(_)) => {}
            other => unreachable!("expected JournalDeserialization, got {other:?}"),
        }
    }

    #[test]
    fn save_journal_of_an_empty_history_round_trips_to_an_empty_journal() {
        let history = History::new();
        let bytes = match history.save_journal() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let recovered = match History::load_journal(&bytes) {
            Ok(history) => history,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(recovered.journal_len(), 0);
    }

    // --- a crafted journal is untrusted input too ---------------------
    //
    // `LayerTree`'s own `Deserialize` validates a `.aur` manifest, but a
    // journal never goes through it: `replay` starts from
    // `LayerTree::new()` and applies recorded ops, so a crafted
    // `history` entry in the same file was a second, unvalidated way to
    // reach a live `LayerTree`. These craft the journals that used to
    // get through.
    //
    // None of this is reachable from the app today -- nothing in the UI
    // calls `remove`/`reparent` yet -- so this closes a latent gap
    // before it goes live rather than fixing an active exploit.

    fn pixel_entry(name: &str, parent: Option<super::LayerId>) -> super::LayerEntry {
        super::LayerEntry::new(
            name.to_owned(),
            parent,
            LayerKind::Pixel { bounds: bounds() },
        )
    }

    fn group_entry(
        name: &str,
        parent: Option<super::LayerId>,
        children: Vec<super::LayerId>,
    ) -> super::LayerEntry {
        super::LayerEntry::new(name.to_owned(), parent, LayerKind::Group { children })
    }

    /// Encodes `journal` exactly as `save_journal` would, loads it back
    /// through the real public entry point, and replays it.
    fn replay_crafted_journal(journal: &[super::LayerOp]) -> Result<LayerTree, DocError> {
        let bytes = match postcard::to_allocvec(journal) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let recovered = match History::load_journal(&bytes) {
            Ok(history) => history,
            Err(err) => unreachable!("a well-formed postcard journal must load: {err:?}"),
        };
        recovered.replay()
    }

    #[test]
    fn replaying_a_journal_whose_restored_root_records_the_wrong_parent_errors() {
        let root: super::LayerId = Id::from_raw(0);
        let elsewhere: super::LayerId = Id::from_raw(7);
        // The op says "put this back at the top level", the entry says
        // "I live inside layer 7". `remove_capturing` would later look
        // for it among layer 7's children.
        let journal = vec![super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries: vec![(root, pixel_entry("a", Some(elsewhere)))],
        })];
        match replay_crafted_journal(&journal) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, root),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_whose_restored_subtree_hides_an_unreachable_entry_errors() {
        let root: super::LayerId = Id::from_raw(0);
        let stowaway: super::LayerId = Id::from_raw(1);
        // `stowaway` rides along in `entries` but no `children` list
        // names it: it would land in the tree's map while being
        // invisible to every traversal.
        let journal = vec![super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries: vec![
                (root, group_entry("g", None, Vec::new())),
                (stowaway, pixel_entry("stowaway", None)),
            ],
        })];
        match replay_crafted_journal(&journal) {
            Err(DocError::OrphanedLayer(id)) => assert_eq!(id, stowaway),
            other => unreachable!("expected OrphanedLayer, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_whose_restored_subtree_carries_the_same_id_twice_errors() {
        let root: super::LayerId = Id::from_raw(0);
        let twice: super::LayerId = Id::from_raw(1);
        let journal = vec![super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries: vec![
                (root, group_entry("g", None, vec![twice])),
                (twice, pixel_entry("first", Some(root))),
                (twice, pixel_entry("second", Some(root))),
            ],
        })];
        match replay_crafted_journal(&journal) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, twice),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_whose_restored_subtree_is_missing_its_own_root_errors() {
        let root: super::LayerId = Id::from_raw(0);
        let only: super::LayerId = Id::from_raw(1);
        let journal = vec![super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries: vec![(only, pixel_entry("only", None))],
        })];
        match replay_crafted_journal(&journal) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, root),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_whose_ops_are_each_valid_but_whose_result_is_not_errors() {
        // Each op here is internally coherent, and the damage only shows
        // up in the *merged* tree: the second subtree's `children` names
        // a layer the first op already placed at the top level, so once
        // the two are merged that layer is reachable from two parents.
        //
        // `restore` used to walk only the incoming subtree, where that
        // id looks like a harmless dangling reference, and this case was
        // caught solely by `replay`'s own closing `tree.validate()`.
        // `restore` now checks the incoming `children` against the
        // *live* map before merging, so it is refused one op earlier and
        // the live tree is never in the broken state at all -- which is
        // what matters for `undo`/`redo`, which call `restore` directly
        // and have no closing validate of their own. `replay`'s
        // `tree.validate()` stays as the outer net for anything a
        // pre-merge check cannot see.
        let stolen: super::LayerId = Id::from_raw(0);
        let thief: super::LayerId = Id::from_raw(1);
        let journal = vec![
            super::LayerOp::Restore(super::RemovedSubtree {
                root: stolen,
                parent: None,
                index: 0,
                entries: vec![(stolen, pixel_entry("stolen", None))],
            }),
            super::LayerOp::Restore(super::RemovedSubtree {
                root: thief,
                parent: None,
                index: 0,
                entries: vec![(thief, group_entry("thief", None, vec![stolen]))],
            }),
        ];
        match replay_crafted_journal(&journal) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, stolen),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_restoring_an_id_the_tree_already_holds_errors() {
        // The `MalformedRemovedSubtree` branch the other crafted-journal
        // tests here miss: a second `Restore` naming an id the first one
        // already made live. Merging it would replace that layer
        // outright.
        let clash: super::LayerId = Id::from_raw(0);
        let subtree = |name: &str| {
            super::LayerOp::Restore(super::RemovedSubtree {
                root: clash,
                parent: None,
                index: 0,
                entries: vec![(clash, pixel_entry(name, None))],
            })
        };
        let journal = vec![subtree("first"), subtree("second")];
        match replay_crafted_journal(&journal) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, clash),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
    }

    #[test]
    fn a_replayed_document_keeps_allocating_ids_past_the_ones_it_restored() {
        // `replay` rebuilds from `LayerTree::new()`, whose counter starts
        // at 0, while every layer it restores keeps its original id.
        // Unless something advances the counter, the next edit on a
        // recovered document hands out an id a restored layer already
        // holds -- silently aliasing two layers and (via
        // `LayerTree::surface_id`, which reuses the raw id) their tile
        // storage with it. `LayerTree::restore` is where that is closed.
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let mut existing = Vec::new();
        for name in ["a", "b", "c"] {
            match history.add_pixel_layer(&mut tree, name, bounds(), None) {
                Ok(id) => existing.push(id),
                Err(err) => unreachable!("{err:?}"),
            }
        }
        let mut replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        let fresh = match replayed.add_pixel_layer("fresh", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            !existing.contains(&fresh),
            "a recovered document must not re-issue an id it already restored: \
             {fresh:?} collides with one of {existing:?}"
        );
        assert_eq!(replayed.len(), 4, "nothing may have been overwritten");
    }

    #[test]
    fn a_refused_undo_keeps_the_step_on_the_undo_stack() {
        // `undo` pops before it applies. When `apply` then fails, the
        // step used to be gone from both stacks -- silently costing the
        // user that step forever. Reached here the documented way (see
        // `History`'s own doc comment): a direct `LayerTree` call mixed
        // into a `History`-managed tree, so the recorded inverse no
        // longer matches reality.
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "layer", bounds(), None) {
            Ok(added) => added,
            Err(err) => unreachable!("{err:?}"),
        };
        // Behind `History`'s back, so undoing the add cannot work.
        if let Err(err) = tree.remove(id) {
            unreachable!("{err:?}");
        }
        assert!(history.can_undo());
        match history.undo(&mut tree) {
            Err(DocError::UnknownLayer(missing)) => assert_eq!(missing, id),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
        assert!(
            history.can_undo(),
            "a refused undo must leave the step where it was, not consume it"
        );
        assert!(!history.can_redo(), "and must not half-move it to redo");
    }

    #[test]
    fn a_refused_redo_keeps_the_step_on_the_redo_stack() {
        // The mirror of the test above.
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "layer", bounds(), None) {
            Ok(added) => added,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(history.can_redo());
        // Behind `History`'s back again: something else now occupies the
        // id the pending redo wants to restore.
        let mut clash = LayerTree::new();
        std::mem::swap(&mut tree, &mut clash);
        if let Err(err) = tree.add_pixel_layer("squatter", bounds(), None) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.roots().first().copied(), Some(id));
        match history.redo(&mut tree) {
            Err(DocError::MalformedRemovedSubtree(clashing)) => assert_eq!(clashing, id),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
        assert!(
            history.can_redo(),
            "a refused redo must leave the step where it was, not consume it"
        );
    }

    #[test]
    fn replaying_an_honest_crafted_journal_still_succeeds() {
        // The positive control for the five above: a hand-built journal
        // that *is* coherent must still replay, so the new checks are
        // not simply refusing every journal that did not come from
        // `save_journal`.
        let group: super::LayerId = Id::from_raw(0);
        let child: super::LayerId = Id::from_raw(1);
        let journal = vec![
            super::LayerOp::Restore(super::RemovedSubtree {
                root: group,
                parent: None,
                index: 0,
                entries: vec![(group, group_entry("g", None, Vec::new()))],
            }),
            super::LayerOp::Restore(super::RemovedSubtree {
                root: child,
                parent: Some(group),
                index: 0,
                entries: vec![(child, pixel_entry("c", Some(group)))],
            }),
        ];
        let tree = match replay_crafted_journal(&journal) {
            Ok(tree) => tree,
            Err(err) => unreachable!("a coherent journal must still replay: {err:?}"),
        };
        assert_eq!(tree.roots(), &[group]);
        assert_eq!(tree.children(group), Some([child].as_slice()));
    }
}
