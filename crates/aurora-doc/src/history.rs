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

use std::collections::HashSet;

use aurora_core::Rect;

use crate::error::DocError;
use crate::layer::{BlendMode, LayerEntry, LayerId, LayerKind, LayerLock, LayerMask};
use crate::text_safety::sanitize_display_name;
use crate::tree::{LayerTree, RemovedSubtree, validate_origin};

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

/// The most entries [`History::journal_descriptions`] will ever return.
/// Photoshop's own History-states maximum is 1000; matching that number
/// keeps the panel's worst case in the same range a professional already
/// expects, rather than growing with an untrusted file's journal length.
const MAX_DESCRIPTIONS: usize = 1000;

/// How far into a `RemovedSubtree`'s own `entries` list [`describe`]
/// will look for the root's recorded name.
///
/// The root is at index 0 for every subtree this crate produces —
/// `LayerTree::remove_capturing` captures it first (its
/// `capture_subtree` walks "root first, then each child's own subtree in
/// stored order"), and [`History::add_pixel_layer`]/[`History::add_group`]
/// each build a one-element list holding only it. So a limit of any size
/// ≥ 1 is behaviour-preserving for every journal Aurora itself wrote.
///
/// The limit exists for the other case: [`History::load_journal`]
/// deliberately performs zero structural validation, so a crafted or
/// foreign journal can carry an arbitrarily long `entries` list whose
/// root is nowhere near the front — and an unbounded `find` there is an
/// unbounded scan *per described entry*, on the UI thread that is
/// drawing the History panel. 64 is generous next to the 1 every real
/// subtree needs, and small enough that
/// `MAX_DESCRIPTIONS × MAX_ROOT_SEARCH_ENTRIES` is a fixed, trivial
/// ceiling on the whole panel's work.
const MAX_ROOT_SEARCH_ENTRIES: usize = 64;

/// A one-line, human-readable description of one journal entry — see
/// [`History::journal_descriptions`]'s own doc comment for why this
/// deliberately doesn't take a `&LayerTree` to resolve names beyond
/// what an entry itself already captured.
///
/// The two arms that embed a caller-supplied name (`Restore`, `Rename`)
/// put it through [`sanitize_display_name`] first; the other twelve of
/// [`LayerOp`]'s fourteen variants format only numeric ids, verbs,
/// percentages, coordinates, or a `Debug`-formatted [`BlendMode`], none
/// of which carry unbounded text.
///
/// **The `Restore` arm's search for the root's name is bounded** to the
/// first [`MAX_ROOT_SEARCH_ENTRIES`] entries — see that constant for why
/// that is invisible to every subtree this crate builds, all of which
/// put the root at index 0. Past the bound the arm falls back to the
/// same `"layer"` placeholder it already uses when `entries` doesn't
/// name the root at all, so the degradation is display-only text on a
/// crafted or foreign journal. It changes nothing about what a journal
/// is *accepted* as: [`History::load_journal`]'s deliberate
/// zero-structural-validation doctrine is untouched, and
/// [`History::replay`] still holds the full subtree to the real
/// validator.
fn describe(op: &LayerOp) -> String {
    match op {
        LayerOp::RemoveById(id) => format!("Removed layer #{}", id.to_raw()),
        LayerOp::Restore(removed) => {
            let name = removed
                .entries
                .iter()
                .take(MAX_ROOT_SEARCH_ENTRIES)
                .find(|(entry_id, _)| *entry_id == removed.root)
                .map_or("layer", |(_, entry)| entry.name.as_str());
            format!("Added layer \"{}\"", sanitize_display_name(name))
        }
        LayerOp::Reparent { id, .. } => format!("Moved layer #{}", id.to_raw()),
        LayerOp::Rename { id, name } => {
            format!(
                "Renamed layer #{} to \"{}\"",
                id.to_raw(),
                sanitize_display_name(name)
            )
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
    /// entries. This is the untruncated count, so it can exceed
    /// `journal_descriptions().len()`, which caps what it returns.
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
    ///
    /// **Bounded on both axes, because these strings become `accesskit`
    /// labels.** `aurora_ui`'s History panel hands each one straight to
    /// `node.set_label`, so an unbounded description crosses a process
    /// boundary into an assistive technology that did not ask for it —
    /// and layer names on the `.aur` path come from a file.
    ///
    /// - **Per entry**: an embedded layer name goes through
    ///   [`sanitize_display_name`] — control, bidi-formatting,
    ///   separator, and invisible-format characters removed, then capped
    ///   at `MAX_NAME_CHARS` (128) characters plus an `…`. That function
    ///   bounds its *input* as well as its output (at most 1024
    ///   characters of a name are examined), which is what keeps a name
    ///   made entirely of invisible characters from costing its own
    ///   full length here — the output cap alone never stopped that
    ///   walk, and 0.64.0's claim to be bounded "regardless of how large
    ///   the journal on disk was" was wrong until 0.64.2. The longest
    ///   description any arm can produce is therefore
    ///   `Renamed layer #{u64} to "{name}"` — about 15 characters of
    ///   fixed text, up to 20 digits of id, and 129 characters of name
    ///   between quotes: roughly 170 characters worst case. **That is a
    ///   character bound, not a byte bound**: `char`s are up to 4 bytes
    ///   in UTF-8, so the same worst case is ~540 bytes for one
    ///   description and ~540 KB for a full, capped panel. That is the
    ///   honest ceiling — small enough for an `accesskit` label, but not
    ///   the tighter number the character count alone reads as.
    /// - **In total**: at most `MAX_DESCRIPTIONS` (1000) real
    ///   entries, matching Photoshop's own History-states maximum. The
    ///   *most recent* 1000 are kept; when anything was dropped, index 0
    ///   is a synthetic `"… {n} earlier steps omitted"` entry (singular
    ///   `step` when `n == 1`) sitting in the oldest-of-what's-shown
    ///   position this method's chronological order puts it in.
    /// - **Per-entry *compute*, not just per-entry output**: the one arm
    ///   that searches rather than formats — `Restore`, looking through a
    ///   removed subtree's `entries` for the root's recorded name — stops
    ///   after `MAX_ROOT_SEARCH_ENTRIES` (64) comparisons. Without
    ///   that, a single crafted journal entry carrying a million-element
    ///   `entries` list was a million comparisons *per described entry*,
    ///   so total work grew with the file rather than with this method's
    ///   own caps. That search is now bounded by
    ///   `MAX_DESCRIPTIONS × MAX_ROOT_SEARCH_ENTRIES`, and the name
    ///   sanitizer each described entry runs is separately bounded to
    ///   1024 input characters (`MAX_SCANNED_CHARS`) — the two together
    ///   are what make this method's total work a function of its own
    ///   caps rather than of the journal's size. Between 0.64.0 and
    ///   0.64.2 only the first of the two held: a review round measured
    ///   a journal of 200 names × 200,000 invisible characters at 158 ms
    ///   through the sanitizer, against 170 µs for ordinary names.
    ///
    /// All three bounds are **display-only**. The journal, the stored
    /// layer name ([`LayerTree::name`]), and [`Self::replay`] all still
    /// carry the full, unmodified name and the full op sequence; nothing
    /// here edits what an undo or a replay will reproduce, and nothing
    /// here changes which journals [`Self::load_journal`] accepts.
    #[must_use]
    pub fn journal_descriptions(&self) -> Vec<String> {
        let omitted = self.journal.len().saturating_sub(MAX_DESCRIPTIONS);
        let mut out = Vec::new();
        if omitted > 0 {
            out.push(format!(
                "… {omitted} earlier step{} omitted",
                if omitted == 1 { "" } else { "s" }
            ));
        }
        out.extend(self.journal.iter().skip(omitted).map(describe));
        out
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
    /// a real layer in `tree`, or [`DocError::LayerOriginOutOfRange`] if
    /// `old`'s own origin is one [`LayerTree::set_bounds`] would refuse.
    /// Nothing is recorded when either happens — neither the journal nor
    /// the undo stack is touched.
    ///
    /// **Why `old` is range-checked here even though this method never
    /// writes to `tree`.** Until 0.57.14 it was not, on the argument
    /// that the value reaches the journal rather than the tree, and that
    /// the first thing to put it *in* the tree would be an ordinary
    /// [`Self::undo`], which delegates to [`LayerTree::set_bounds`] and
    /// is refused there. The premise held; the conclusion did not. That
    /// refusal happens *after* the entry is on the undo stack and with
    /// nothing popping it, so the same `undo()` fails the same way on
    /// every attempt while [`Self::can_undo`] keeps reporting `true` —
    /// undo permanently wedged, and every step beneath it unreachable.
    /// Checking up front, with the same predicate `set_bounds` uses,
    /// turns that into an ordinary rejected call.
    pub fn record_bounds_change(
        &mut self,
        tree: &LayerTree,
        id: LayerId,
        old: Rect,
    ) -> Result<(), DocError> {
        let current = tree.bounds(id).ok_or(DocError::UnknownLayer(id))?;
        // Before the first push, so a refusal records nothing at all --
        // the same "all or nothing" discipline `set_bounds` follows.
        validate_origin(old)?;
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

    /// Same as [`LayerTree::add_mask`], recorded for undo — **and, unlike
    /// it, this clears any coverage a previous mask on the same layer
    /// left behind** (via [`crate::mask::forget_mask_coverage`]), which
    /// is why it needs the store. Mask surface ids are derived rather
    /// than allocated, so without the clear a fresh mask would open
    /// wearing a deleted one's pixels; **[`crate::mask`]'s lifecycle
    /// notes are the canonical account** of why the clear belongs here
    /// and not in [`Self::remove_mask`], and are the one place that
    /// reasoning is kept.
    ///
    /// Two things that follow from it, both stated in full there:
    ///
    /// - **Undoing back past this does not restore the old mask's
    ///   coverage, and can leave the restored old mask reading the
    ///   *newer* mask's coverage, shifted** — the same defect shape
    ///   this call fixes going forward, still present going backward,
    ///   for the same reason. Accepted and tested, not lost silently.
    /// - **[`crate::mask::forget_mask_coverage`] is a whole-store scan,
    ///   and this runs it on every successful mask add** — including
    ///   the common case of a layer that never had a mask, where it
    ///   matches nothing. See that function's own "Cost" section for
    ///   what that means and the named follow-on it would need.
    ///
    /// # Errors
    ///
    /// Same as [`LayerTree::add_mask`]. Nothing in the store is touched
    /// when it refuses — in particular a
    /// [`DocError::MaskAlreadyExists`] refusal leaves the live mask's
    /// coverage entirely alone, and so does an out-of-range rectangle
    /// on a maskless layer still carrying residue from a prior removal.
    pub fn add_mask(
        &mut self,
        tree: &mut LayerTree,
        store: &mut aurora_tile::TileStore,
        id: LayerId,
        bounds: Rect,
    ) -> Result<Option<Rect>, DocError> {
        // Order is load-bearing: the tree edit comes first, so a refusal
        // (unknown layer, `MaskAlreadyExists`, an out-of-range rectangle)
        // returns before anything can destroy the pixels of a mask that
        // is still attached.
        tree.add_mask(id, bounds)?;
        // Only now, with a genuinely new mask committed, is the old
        // occupant of this derived surface unreachable and safe to free.
        crate::mask::forget_mask_coverage(tree, store, id);
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
    /// # Why this does *not* free the mask's tiles
    ///
    /// It takes no store, deliberately: this removal is undoable via
    /// `LayerOp::RestoreMask`, so freeing the coverage here would make
    /// Ctrl+Z bring the mask back blank. The residue is on purpose, and
    /// is cleared instead by [`Self::add_mask`] and by
    /// [`crate::forget_document_surfaces`], the two points where it
    /// really is unreachable. **[`crate::mask`]'s lifecycle notes carry
    /// the full reasoning**; `history.rs`'s own
    /// `undo_of_a_remove_mask_still_finds_its_painted_coverage` is what
    /// keeps it true.
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

/// Frees every `aurora_tile::TileStore` surface a discarded document
/// could still be addressing — its live layers' content and mask
/// surfaces, *plus* every layer captured on either undo/redo stack —
/// and returns how many tiles were actually forgotten.
///
/// Without this, a document that goes away takes nothing with it. A
/// surface id is derived from a `LayerId` rather than allocated, and
/// `aurora_tile::TileStore` has no notion of which document a surface
/// belongs to, so every tile the discarded document ever painted stays
/// resident (or paged out to the scratch disk) for the lifetime of the
/// process with nothing left able to name it.
///
/// # Why this takes both by value
///
/// Because the alternative signature is dangerous in a way review
/// cannot reliably catch. Taking `&LayerTree`/`&History` would let a
/// caller sweep a *live* document — silently destroying the pixels of
/// every layer the user still has open, with no error and no way back
/// (see `aurora_tile::TileStore::forget_surface`). By value, that is a
/// compile error instead: the caller has given the document up, and
/// cannot use it afterwards. It is not an ergonomic accident, and it
/// should not be relaxed to references.
///
/// # What is walked
///
/// Three sources, and the third was added in 0.80.1:
///
/// - every live layer's content and mask surface, via this crate's own
///   crate-private `LayerTree::all_surfaces`;
/// - every `RemovedSubtree` captured on the undo stack
///   **or the redo stack** — an added-then-undone layer's subtree sits
///   on the latter and nowhere else, and its tiles are exactly as
///   orphaned as any other's;
/// - every `Restore` entry in [`History`]'s own crash-recovery
///   journal.
///
/// The journal used to be skipped, on the stated grounds that a
/// surface named only there "names nothing anyone can still reach" —
/// which is a description of this leak, not a reason to leave it. The
/// case is concrete: [`History::add_pixel_layer`]/[`History::add_group`]
/// push the same `LayerOp::Restore` onto the journal as onto the undo
/// stack, so once that stack entry is consumed by an undo and the
/// resulting redo entry is dropped (see the gap below), the journal
/// can be the last place naming that subtree's surfaces. Sweeping it
/// too is a strict superset at the cost of one more `Restore` scan,
/// and this function's whole contract is that the caller has given
/// the document up.
///
/// One caveat that does not change the decision, but is worth knowing:
/// unlike the two stacks, a journal can come from *outside* this
/// process — [`History::load_journal`] deserializes one whole, with
/// deliberately no structural validation. It cannot make the sweep
/// name `aurora-app`'s reserved composite surface (both guards in
/// `RemovedSubtree::surfaces` exclude it), but it can name arbitrary
/// layer ids. That is the same cross-document aliasing the section
/// below on wiring this into the app describes, not a new one.
///
/// # What is still not covered
///
/// Two gaps, each one a surface this sweep can no longer name (a
/// third, separate limitation — which of the app's own open paths can
/// call this at all — is the section further down):
///
/// 1. **A redo entry dropped mid-session.** `History`'s private `push`
///    helper clears the redo stack on any new structural activity, and
///    so does the *public* [`History::clear_redo`] — which
///    `aurora_app::UndoOrder::record` calls on every committed edit, to
///    keep this history's redo stack and `aurora_brush::PixelHistory`'s
///    invalidating each other. That makes this the one leak path here
///    that is live and reachable in the shipped app today. The captured
///    subtrees go with the cleared stack; the journal sweep above
///    recovers the ones that came from an *add* (whose `Restore` is
///    also journalled), but not a redo entry that arrived any other
///    way. Freeing at that point needs a store handle neither `push`
///    nor `clear_redo` has, which is a wider change than this round.
/// 2. **A removal that bypassed [`History`] entirely.**
///    [`LayerTree::remove`] — as opposed to `remove_capturing` — drops
///    the subtree on the floor rather than handing it back, so no
///    `RemovedSubtree` reaches either stack or the journal and the
///    removed layer's surfaces are beyond even this sweep's reach.
///    Mixing direct `LayerTree` calls with `History` is a discouraged
///    but supported shape (see [`History`]'s own doc comments), so this
///    is reachable by construction, not merely in theory.
///
/// # Wired into the app's flat-image open; `.aur` still is not
///
/// `aurora_app::App::open_file`'s flat-image path calls this as of
/// 0.82.0, through its own `replace_document_pixels`: it sweeps the
/// outgoing document **before** writing the incoming image's pixels.
/// That order is forced by the same aliasing described above — both
/// documents derive surface ids from `LayerId`s that restart counting
/// from zero, so the incoming document's first layer claims exactly
/// the surface the outgoing one's first layer owns. Sweeping first is
/// also what makes the incoming layer's surface genuinely empty, which
/// `aurora_io::write_into_store` already assumes ("the rest of a
/// freshly allocated tile is already zero") when it writes only the
/// region the image covers. It is safe to sweep first there because
/// every fallible step of that open — read, decode, panel rebuild —
/// is already behind it.
///
/// `aurora_app::App::open_aur_file` remains blocked, and the reason is
/// specifically an *ordering* problem rather than a residue one:
/// `aurora_io::read_aur` fills the store with the new document's tiles
/// before the caller holds any tree to sweep against, so there is no
/// point in that path where the outgoing document can be swept without
/// either destroying the document just loaded (sweeping after) or
/// destroying the live one on an open that can still fail (sweeping
/// before). The residue half is already handled: that reader rolls
/// back its own partial writes on failure. Solving the ordering half
/// (a per-document surface-id namespace, or a staging store the read
/// fills before an atomic swap) is real follow-on work and is not done
/// here.
// Deliberate: see "Why this takes both by value" above. Taking these by
// reference would make sweeping a live document compile.
#[allow(clippy::needless_pass_by_value)]
pub fn forget_document_surfaces(
    layers: LayerTree,
    history: History,
    store: &mut aurora_tile::TileStore,
) -> usize {
    let mut surfaces: HashSet<aurora_tile::SurfaceId> = layers.all_surfaces().into_iter().collect();
    for op in history
        .undo_stack
        .iter()
        .chain(history.redo_stack.iter())
        .chain(history.journal.iter())
    {
        if let LayerOp::Restore(removed) = op {
            surfaces.extend(removed.surfaces());
        }
    }
    // One batched call, not one `forget_surface` per surface: each of
    // those does its own full scan of everything the store holds, so
    // the loop would be O(surfaces × tiles held) at exactly the moment
    // the store is fullest.
    store.forget_surfaces(&surfaces)
}

#[cfg(test)]
mod tests {
    use super::History;
    use crate::DocError;
    use crate::layer::{BlendMode, LayerKind, LayerLock, LayerMask};
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
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
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

    /// An ordinary short, clean name -- including a non-ASCII em dash,
    /// which is neither a control nor a bidi-formatting character --
    /// comes through the sanitizer untouched, so the descriptions a real
    /// in-session history produces are byte-identical to what they were
    /// before any capping existed.
    #[test]
    fn an_ordinary_name_passes_through_a_description_unchanged() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let id = match history.add_pixel_layer(&mut tree, "Retouch — skin", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_name(&mut tree, id, "Café 😀‍🔥 background") {
            unreachable!("{err:?}");
        }

        let descriptions = history.journal_descriptions();
        assert_eq!(descriptions.len(), 2, "{descriptions:?}");
        let Some(added) = descriptions.first() else {
            unreachable!("just asserted len() == 2");
        };
        assert_eq!(added, "Added layer \"Retouch — skin\"");
        let Some(renamed) = descriptions.get(1) else {
            unreachable!("just asserted len() == 2");
        };
        // The zero-width joiner inside the emoji sequence is category
        // `Cf` but deliberately *not* stripped -- it is load-bearing.
        assert_eq!(
            renamed,
            &format!("Renamed layer #{} to \"Café 😀‍🔥 background\"", id.to_raw())
        );
    }

    #[test]
    fn journal_description_of_a_huge_layer_name_is_capped() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let huge = "a".repeat(500_000);
        let id = match history.add_pixel_layer(&mut tree, huge.clone(), bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_name(&mut tree, id, huge.clone()) {
            unreachable!("{err:?}");
        }

        let descriptions = history.journal_descriptions();
        assert_eq!(descriptions.len(), 2, "{descriptions:?}");
        // The `Restore` (added) arm.
        let Some(added) = descriptions.first() else {
            unreachable!("just asserted len() == 2");
        };
        assert!(
            added.chars().count() <= 200,
            "{} chars",
            added.chars().count()
        );
        assert!(added.contains('\u{2026}'), "{added}");
        // The `Rename` arm.
        let Some(renamed) = descriptions.get(1) else {
            unreachable!("just asserted len() == 2");
        };
        assert!(
            renamed.chars().count() <= 200,
            "{} chars",
            renamed.chars().count()
        );
        assert!(renamed.contains('\u{2026}'), "{renamed}");
    }

    #[test]
    fn journal_description_strips_control_and_bidi_characters() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let hostile = "safe\u{202E}txet\u{0007}";
        let id = match history.add_pixel_layer(&mut tree, hostile, bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_name(&mut tree, id, hostile) {
            unreachable!("{err:?}");
        }

        for description in history.journal_descriptions() {
            assert!(!description.contains('\u{202E}'), "{description:?}");
            assert!(!description.contains('\u{0007}'), "{description:?}");
            assert!(description.contains("safe"), "{description:?}");
            // Nothing here is anywhere near the 128-character cap, so
            // stripping alone must not add an ellipsis: the `…` means
            // "visible text was cut", not "something was removed".
            assert!(!description.contains('\u{2026}'), "{description:?}");
        }
    }

    /// The exact character-cap boundary, seen end to end through
    /// `journal_descriptions` rather than only at `sanitize_display_name`
    /// (which has its own boundary test): 128 visible characters is not
    /// truncated, 129 is.
    #[test]
    fn journal_description_name_cap_boundary_is_exact() {
        for (name_chars, wants_ellipsis) in [(128_usize, false), (129, true)] {
            let mut tree = LayerTree::new();
            let mut history = History::new();
            let name = "a".repeat(name_chars);
            let id = match history.add_pixel_layer(&mut tree, name.clone(), bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let descriptions = history.journal_descriptions();
            let Some(added) = descriptions.first() else {
                unreachable!("one op was journaled: {descriptions:?}");
            };
            assert_eq!(
                added.contains('\u{2026}'),
                wants_ellipsis,
                "{name_chars} chars: {added:?}"
            );
            if !wants_ellipsis {
                assert_eq!(*added, format!("Added layer \"{name}\""));
            }
            let _ = id;
        }
    }

    /// A legitimate multi-byte name *under* the character cap but well
    /// over it in bytes must reach the description byte-identical --
    /// `sanitize_display_name`'s byte-length fast-path check is a
    /// conservative pre-filter, not the cap itself.
    #[test]
    fn a_long_cjk_name_under_the_char_cap_reaches_the_description_intact() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let name = "漢".repeat(100);
        assert!(name.len() > 128, "{} bytes", name.len());
        if let Err(err) = history.add_pixel_layer(&mut tree, name.clone(), bounds(), None) {
            unreachable!("{err:?}");
        }
        let descriptions = history.journal_descriptions();
        let Some(added) = descriptions.first() else {
            unreachable!("one op was journaled: {descriptions:?}");
        };
        assert_eq!(*added, format!("Added layer \"{name}\""));
        assert!(!added.contains('\u{2026}'), "{added:?}");
    }

    #[test]
    fn truncating_a_multibyte_name_does_not_panic() {
        // Each name is far past the 128-character cap, and every cap
        // boundary lands inside a multi-byte character -- 3 bytes for
        // the Han ideograph, 4 for the emoji. Reaching the assertions
        // at all is the "does not panic" half; `String`/`char`
        // operations throughout are what make the result valid UTF-8.
        for name in [&"漢".repeat(200), &"🎨".repeat(200)] {
            let mut tree = LayerTree::new();
            let mut history = History::new();
            let id = match history.add_pixel_layer(&mut tree, name.clone(), bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = history.set_name(&mut tree, id, name.clone()) {
                unreachable!("{err:?}");
            }

            let descriptions = history.journal_descriptions();
            assert_eq!(descriptions.len(), 2, "{descriptions:?}");
            for description in descriptions {
                assert!(
                    description.chars().count() <= 200,
                    "{} chars",
                    description.chars().count()
                );
                assert!(description.contains('\u{2026}'), "{description}");
            }
        }
    }

    #[test]
    fn journal_descriptions_caps_entry_count_with_an_omission_notice() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let id = match history.add_pixel_layer(&mut tree, "Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // One `Restore` plus 1004 visibility flips == 1005 journal ops.
        for step in 0..1004 {
            if let Err(err) = history.set_visible(&mut tree, id, step % 2 == 0) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(history.journal_len(), 1005);

        let descriptions = history.journal_descriptions();
        assert_eq!(descriptions.len(), 1001, "1000 kept plus the notice");
        let Some(notice) = descriptions.first() else {
            unreachable!("just asserted len() == 1001");
        };
        assert_eq!(notice, "… 5 earlier steps omitted");
        // Index 1 is a real op description, not a second notice. The
        // *most recent* 1000 are what survive, so the five dropped are
        // the oldest -- which includes the `Restore` that named the
        // layer.
        let Some(first_kept) = descriptions.get(1) else {
            unreachable!("just asserted len() == 1001");
        };
        assert!(
            first_kept.contains(&format!("layer #{}", id.to_raw())),
            "{first_kept:?}"
        );
        assert!(!first_kept.starts_with('…'), "{first_kept:?}");
    }

    /// The single most important guard here: capping and sanitizing are
    /// *display-only*. A name that `journal_descriptions` truncates must
    /// still round-trip through `replay` in full -- proof the sanitizer
    /// never ran inside `apply`'s `Restore`/`Rename` arms.
    #[test]
    fn replay_preserves_a_name_that_display_truncates() {
        let mut tree = LayerTree::new();
        let mut history = History::new();

        let huge = "a".repeat(500_000);
        let id = match history.add_pixel_layer(&mut tree, huge.clone(), bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let renamed = format!("{huge}\u{202E}");
        if let Err(err) = history.set_name(&mut tree, id, renamed.clone()) {
            unreachable!("{err:?}");
        }

        let replayed = match history.replay() {
            Ok(tree) => tree,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(replayed.name(id), Some(renamed.as_str()));
        assert_eq!(tree.name(id), Some(renamed.as_str()));

        let descriptions = history.journal_descriptions();
        let Some(shown) = descriptions.get(1) else {
            unreachable!("two ops were journaled: {descriptions:?}");
        };
        assert!(shown.chars().count() <= 200, "{shown:?}");
        assert!(!shown.contains('\u{202E}'), "{shown:?}");
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

    // Before 0.57.14 this call succeeded, and the entry it pushed made
    // undo permanently unusable: `undo` delegates to
    // `LayerTree::set_bounds`, which refuses the out-of-range origin,
    // but the refusal happens with the entry still on the stack and
    // nothing popping it -- so every later `undo()` failed identically
    // while `can_undo()` kept saying `true`. Checking `old` before the
    // first push is what turns that into an ordinary rejected call.
    #[test]
    fn record_bounds_change_rejects_an_old_origin_outside_the_document_range() {
        let mut tree = LayerTree::new();
        let mut history = History::new();
        // Built on the tree directly, so the undo stack starts genuinely
        // empty and `can_undo()` is a clean signal for the wedge.
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let journal_len_before = history.journal_len();
        assert!(!history.can_undo(), "nothing to undo before the bad call");

        match history.record_bounds_change(&tree, id, out_of_range_origin()) {
            Err(DocError::LayerOriginOutOfRange { x, y: _, max }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }

        assert_eq!(
            history.journal_len(),
            journal_len_before,
            "a refused call must record nothing in the journal"
        );
        assert!(
            !history.can_undo(),
            "a refused call must leave the undo stack untouched -- the wedge this closes"
        );
        assert!(!history.can_redo());

        // And the tree, which this method never writes to anyway, is
        // still exactly where the caller left it.
        assert_eq!(tree.bounds(id), Some(bounds()));

        // The layer is still perfectly editable afterwards: a legitimate
        // `record_bounds_change` on the same id still works, and its undo
        // succeeds -- proving nothing was left half-recorded.
        let moved = Rect {
            x: 100,
            y: 100,
            width: 10,
            height: 10,
        };
        if let Err(err) = tree.set_bounds(id, moved) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.record_bounds_change(&tree, id, bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.bounds(id), Some(bounds()));
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
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
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
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
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
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
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
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut tree, "a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut tree, id, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
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

    /// An origin one step past `aurora_core::MAX_DOCUMENT_ORIGIN`, with
    /// a small extent so only the origin is at fault.
    fn out_of_range_origin() -> Rect {
        Rect {
            x: aurora_core::MAX_DOCUMENT_ORIGIN + 1,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn replaying_a_journal_whose_restored_layer_sits_outside_the_document_range_errors() {
        // `set_bounds` and `insert_unchecked` both run `validate_origin`
        // on a caller-supplied `Rect`, but `restore` deliberately does
        // not (it puts back a value the tree already accepted), so a
        // crafted `Restore` op was a way to splice an origin past
        // `MAX_DOCUMENT_ORIGIN` into a live tree. `replay`'s closing
        // `tree.validate()` is what refuses it now.
        let root: super::LayerId = Id::from_raw(0);
        let entry = super::LayerEntry::new(
            "far".to_owned(),
            None,
            LayerKind::Pixel {
                bounds: out_of_range_origin(),
            },
        );
        let journal = vec![super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries: vec![(root, entry)],
        })];
        match replay_crafted_journal(&journal) {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn replaying_a_journal_whose_restored_mask_sits_outside_the_document_range_errors() {
        // The mask half of the same door: `add_mask` runs
        // `validate_origin`, `restore_mask` does not. The layer this
        // mask lands on is itself in range, so only the mask is at
        // fault.
        let root: super::LayerId = Id::from_raw(0);
        let journal = vec![
            super::LayerOp::Restore(super::RemovedSubtree {
                root,
                parent: None,
                index: 0,
                entries: vec![(root, pixel_entry("host", None))],
            }),
            super::LayerOp::RestoreMask(
                root,
                LayerMask {
                    bounds: out_of_range_origin(),
                    enabled: true,
                    inverted: false,
                },
            ),
        ];
        match replay_crafted_journal(&journal) {
            Err(DocError::LayerOriginOutOfRange { x, max, .. }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
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

    /// A `RemovedSubtree` whose `entries` list is `size` long, with the
    /// root's own `"Retouch"` entry at `root_index` and short filler
    /// everywhere else — the shared fixture for the two
    /// [`super::MAX_ROOT_SEARCH_ENTRIES`] tests below.
    ///
    /// The filler entries carry empty names and ids that start well past
    /// the root's, so nothing but the root can ever match and the memory
    /// this holds stays close to the bare `LayerEntry` size.
    fn restore_with_root_at(size: usize, root_index: usize) -> super::LayerOp {
        let root: super::LayerId = Id::from_raw(0);
        let mut entries = Vec::with_capacity(size);
        // `+ 1` so no filler id can ever collide with the root's 0.
        let mut next_filler = 1_u64;
        for position in 0..size {
            if position == root_index {
                entries.push((root, pixel_entry("Retouch", None)));
            } else {
                let filler: super::LayerId = Id::from_raw(next_filler);
                next_filler += 1;
                entries.push((filler, pixel_entry("", None)));
            }
        }
        super::LayerOp::Restore(super::RemovedSubtree {
            root,
            parent: None,
            index: 0,
            entries,
        })
    }

    /// The real case the bound must not break: the root leads the list,
    /// as it does for every subtree this crate produces, and the list is
    /// far longer than the bound.
    ///
    /// `capture_subtree` visits root-first, and
    /// `add_pixel_layer`/`add_group` build a one-element list, so a
    /// journal Aurora itself wrote never has the root anywhere but index
    /// 0 — and the description must still be the layer's real name no
    /// matter how many entries follow it.
    #[test]
    fn describe_names_a_restore_root_that_leads_a_huge_entry_list() {
        let op = restore_with_root_at(50_000, 0);
        assert_eq!(super::describe(&op), "Added layer \"Retouch\"");
    }

    /// The structural proof of the bound: the same list, the same size,
    /// the root moved to the very end.
    ///
    /// Against the unpatched, unbounded `find` this fails — it walks all
    /// 50,000 entries and reports the real `"Retouch"`. Against the bound
    /// it stops after `MAX_ROOT_SEARCH_ENTRIES` and falls back to the
    /// `"layer"` placeholder the arm already used when `entries` names no
    /// root at all. No timing is involved, so it is deterministic on any
    /// machine.
    ///
    /// **This is the deliberate, disclosed degradation**, and it is
    /// display-only: only a crafted or foreign journal can reach it (see
    /// `MAX_ROOT_SEARCH_ENTRIES`), it costs a placeholder name in the
    /// History panel, and it does not change which journals
    /// `History::load_journal` accepts — that method's
    /// zero-structural-validation doctrine is untouched, and `replay`
    /// still holds the same subtree to the real validator.
    #[test]
    fn describe_stops_searching_a_restore_entry_list_after_a_bounded_prefix() {
        let size = 50_000;
        let op = restore_with_root_at(size, size - 1);
        assert_eq!(super::describe(&op), "Added layer \"layer\"");
    }

    /// The bound's own boundary, both sides, one entry apart.
    ///
    /// The two tests above prove *a* bound exists somewhere between 1
    /// and 50,000; a review round showed by mutation that setting
    /// `MAX_ROOT_SEARCH_ENTRIES` to 1 or to 49,998 left both of them
    /// green. This pins where it actually falls: the entry at index
    /// `MAX_ROOT_SEARCH_ENTRIES - 1` is the last one searched, and the
    /// one at `MAX_ROOT_SEARCH_ENTRIES` is the first one missed. An
    /// off-by-one in the `take` fails one half or the other.
    #[test]
    fn describe_searches_exactly_the_first_max_root_search_entries() {
        let size = super::MAX_ROOT_SEARCH_ENTRIES + 1;

        let last_searched = restore_with_root_at(size, super::MAX_ROOT_SEARCH_ENTRIES - 1);
        assert_eq!(
            super::describe(&last_searched),
            "Added layer \"Retouch\"",
            "the entry at MAX_ROOT_SEARCH_ENTRIES - 1 is still inside the bound"
        );

        let first_missed = restore_with_root_at(size, super::MAX_ROOT_SEARCH_ENTRIES);
        assert_eq!(
            super::describe(&first_missed),
            "Added layer \"layer\"",
            "the entry at MAX_ROOT_SEARCH_ENTRIES is the first one past the bound"
        );
    }

    /// The *magnitude*, which the boundary test above cannot pin: it is
    /// written in terms of the constant, so it stays green at any value,
    /// including a useless one.
    ///
    /// Both ends carry a reason. Every subtree this crate produces puts
    /// the root at index 0, so anything ≥ 1 is behaviour-preserving for
    /// Aurora's own journals — but a bound that tight would make a
    /// foreign journal's *first sibling* enough to lose the name, so the
    /// floor here is real headroom rather than the bare minimum. The
    /// ceiling keeps the panel's total work trivial:
    /// `MAX_DESCRIPTIONS` (1000) × this is the whole of it.
    #[test]
    fn the_root_search_bound_is_generous_but_still_trivial() {
        assert!(
            (16..=1024).contains(&super::MAX_ROOT_SEARCH_ENTRIES),
            "{} is outside the band this bound is justified in",
            super::MAX_ROOT_SEARCH_ENTRIES
        );
        assert!(
            super::MAX_DESCRIPTIONS.saturating_mul(super::MAX_ROOT_SEARCH_ENTRIES) <= 1_000_000,
            "the panel's whole worst case must stay a trivial number of comparisons"
        );
    }

    // ---- `forget_document_surfaces` -------------------------------

    /// A real, scratch-disk-backed store, the same fixture shape
    /// `mask.rs`'s own round-trip tests use. These tests are about what
    /// actually happens to tiles, so a mock would prove nothing.
    fn real_tile_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(8) else {
            unreachable!("8 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("a freshly created tempdir must be usable: {err:?}"),
        };
        (dir, store)
    }

    fn tile() -> aurora_tile::TileId {
        aurora_tile::TileId { x: 0, y: 0 }
    }

    /// Fills tile `(0, 0)` of `surface` with a pattern derived from
    /// `seed` -- distinctive per surface, and not uniform within the
    /// tile, so a byte-exact comparison later is a real one.
    fn paint(store: &mut aurora_tile::TileStore, surface: aurora_tile::SurfaceId, seed: u32) {
        let Ok(entry) = store.get_mut(surface, tile()) else {
            unreachable!("a real store must serve this tile");
        };
        for (index, sample) in entry.texels_mut().iter_mut().enumerate() {
            let value = f32::from((index as u32 % 97 + seed) as u16) / 512.0;
            *sample = half::f16::from_f32(value);
        }
    }

    fn texels(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
    ) -> Vec<half::f16> {
        let Ok(entry) = store.get(surface, tile()) else {
            unreachable!("a real store must serve this tile");
        };
        entry.texels().to_vec()
    }

    fn expected_pattern(seed: u32) -> Vec<half::f16> {
        (0..aurora_tile::SAMPLES)
            .map(|index| {
                let value = f32::from((index as u32 % 97 + seed) as u16) / 512.0;
                half::f16::from_f32(value)
            })
            .collect()
    }

    fn content_surface(tree: &LayerTree, id: crate::LayerId) -> aurora_tile::SurfaceId {
        let Some(surface) = tree.surface_id(id) else {
            unreachable!("a pixel layer in the tree has a content surface");
        };
        surface
    }

    fn mask_surface(tree: &LayerTree, id: crate::LayerId) -> aurora_tile::SurfaceId {
        let Some(surface) = tree.mask_surface_id(id) else {
            unreachable!("a layer in the tree has a mask surface");
        };
        surface
    }

    #[test]
    // The leak this exists to close, at its smallest: removing a layer
    // deliberately keeps its tiles (undo needs them), and only
    // discarding the whole document frees them.
    fn forget_document_surfaces_frees_a_removed_pixel_layers_content_tiles() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Paint", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        let surface = content_surface(&tree, id);
        paint(&mut store, surface, 1);
        assert!(store.contains_tile(surface, tile()));

        if let Err(err) = history.remove(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert!(
            store.contains_tile(surface, tile()),
            "removing a layer must NOT free its tiles -- undo still needs them"
        );

        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            1
        );
        assert!(
            !store.contains_tile(surface, tile()),
            "discarding the document must free the removed layer's tiles"
        );
        assert!(
            texels(&mut store, surface)
                .iter()
                .all(|sample| sample.to_f32() == 0.0),
            "a swept surface must read back blank, not carrying its old pixels"
        );
    }

    #[test]
    // Masks double the number of surfaces a layer can orphan, and the
    // mask surface is derived rather than stored -- so the sweep must
    // not gate on the `LayerMask` struct still being there.
    fn forget_document_surfaces_frees_both_content_and_mask_tiles() {
        // Case one: the mask is still attached when the layer goes.
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let content = content_surface(&tree, id);
        let mask = mask_surface(&tree, id);
        paint(&mut store, content, 1);
        if let Err(err) = crate::write_mask_coverage(&mut store, mask, tile(), 3, 4, 0.25) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.remove(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            2
        );
        assert!(!store.contains_tile(content, tile()), "content freed");
        assert!(!store.contains_tile(mask, tile()), "mask coverage freed");

        // Case two: the mask was *removed* before the layer was, so the
        // captured entry carries no `LayerMask` at all -- but
        // `remove_mask` leaves the coverage behind (see `crate::mask`'s
        // own lifecycle notes), so the sweep must still reach it.
        let (_dir2, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Was masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let content = content_surface(&tree, id);
        let mask = mask_surface(&tree, id);
        paint(&mut store, content, 2);
        if let Err(err) = crate::write_mask_coverage(&mut store, mask, tile(), 1, 1, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert!(
            store.contains_tile(mask, tile()),
            "removing the mask struct leaves its coverage behind -- the residue this must find"
        );
        if let Err(err) = history.remove(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            2
        );
        assert!(!store.contains_tile(content, tile()));
        assert!(
            !store.contains_tile(mask, tile()),
            "residual mask coverage must be freed even with no LayerMask left"
        );
    }

    #[test]
    // One `remove` of a group detaches a whole subtree, and every
    // descendant owns surfaces of its own. A sweep that only looked at
    // the captured root would free one surface and leak the rest.
    fn forget_document_surfaces_frees_every_descendants_surfaces_not_just_the_groups() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(group) = history.add_group(&mut tree, "Group", None) else {
            unreachable!("an empty tree accepts a root group");
        };
        let Ok(plain) = history.add_pixel_layer(&mut tree, "Plain", bounds(), Some(group)) else {
            unreachable!("a group accepts a child");
        };
        let Ok(masked) = history.add_pixel_layer(&mut tree, "Masked", bounds(), Some(group)) else {
            unreachable!("a group accepts a child");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, masked, bounds()) {
            unreachable!("{err:?}");
        }
        let Ok(nested) = history.add_group(&mut tree, "Nested", Some(group)) else {
            unreachable!("a group accepts a child group");
        };
        let Ok(deep) = history.add_pixel_layer(&mut tree, "Deep", bounds(), Some(nested)) else {
            unreachable!("a group accepts a child");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, deep, bounds()) {
            unreachable!("{err:?}");
        }

        // Every surface the subtree owns, painted for real: five layers
        // (two of them groups, which have masks but no content).
        let mut painted = Vec::new();
        for (seed, id) in [plain, masked, deep].into_iter().enumerate() {
            let surface = content_surface(&tree, id);
            paint(&mut store, surface, seed as u32 + 1);
            painted.push(surface);
        }
        for id in [group, plain, masked, nested, deep] {
            let surface = mask_surface(&tree, id);
            if let Err(err) = crate::write_mask_coverage(&mut store, surface, tile(), 0, 0, 0.5) {
                unreachable!("{err:?}");
            }
            painted.push(surface);
        }

        // `aurora-app`'s reserved composite surface must never be one
        // of the ids this machinery emits -- both as a set membership
        // check and, below, as a tile that survives the sweep.
        let composite = aurora_tile::SurfaceId::from_raw(u64::MAX);
        assert!(
            !tree.all_surfaces().contains(&composite),
            "no layer may derive the reserved composite surface"
        );
        paint(&mut store, composite, 9);

        if let Err(err) = history.remove(&mut tree, group) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            painted.len(),
            "every content and mask surface in the subtree must be swept"
        );
        for surface in painted {
            assert!(
                !store.contains_tile(surface, tile()),
                "a descendant's surface must be freed too, not just the group's own"
            );
        }
        assert!(
            store.contains_tile(composite, tile()),
            "the reserved composite surface must survive a document sweep"
        );
    }

    #[test]
    /// The anti-naive-implementation test, and the reason the sweep
    /// lives at document-discard time rather than inside
    /// `LayerTree::remove_capturing`.
    ///
    /// **This test fails against a "free the tiles when the layer is
    /// removed" implementation**, which is the obvious shape and a
    /// strictly worse regression than the leak it would fix: Ctrl+Z
    /// after deleting a layer would restore it blank, silently, with
    /// the user's pixels already gone. Deleting a layer must keep its
    /// tiles; only discarding the document may free them.
    fn undo_of_a_remove_still_finds_the_removed_layers_painted_pixels() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Precious", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        let surface = content_surface(&tree, id);
        paint(&mut store, surface, 7);
        let before = texels(&mut store, surface);
        assert_eq!(
            before,
            expected_pattern(7),
            "the fixture painted what it meant to"
        );

        if let Err(err) = history.remove(&mut tree, id) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }

        assert!(
            tree.contains(id),
            "undo must put the layer back in the tree"
        );
        assert_eq!(
            content_surface(&tree, id),
            surface,
            "the restored layer must address the same surface it painted into"
        );
        assert_eq!(
            texels(&mut store, surface),
            before,
            "the restored layer's pixels must come back byte-exact, not blank"
        );
    }

    #[test]
    // Undoing an *add* leaves the layer's subtree on the redo stack and
    // nowhere else. Scanning only `undo_stack` would leak it.
    fn forget_document_surfaces_frees_a_layer_that_was_added_then_undone() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Undone", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        let content = content_surface(&tree, id);
        let mask = mask_surface(&tree, id);
        paint(&mut store, content, 3);
        if let Err(err) = crate::write_mask_coverage(&mut store, mask, tile(), 2, 2, 0.75) {
            unreachable!("{err:?}");
        }

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(id), "the add was undone");
        assert!(history.can_redo(), "its subtree is on the redo stack");
        assert!(
            !history.can_undo(),
            "and on neither the undo stack nor the tree"
        );

        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            2
        );
        assert!(!store.contains_tile(content, tile()));
        assert!(!store.contains_tile(mask, tile()));
    }

    #[test]
    /// The journal as *last* namer, which is why 0.80.1 started
    /// sweeping it.
    ///
    /// Add a layer, undo it (its subtree moves to the redo stack), then
    /// `clear_redo()` — which the shipped app's own
    /// `aurora_app::UndoOrder::record` calls on every committed edit.
    /// After that the tree does not hold the layer, neither stack holds
    /// its subtree, and the crash-recovery journal's `Restore` entry is
    /// the only thing left in the whole document that can name its
    /// surfaces. Against the pre-0.80.1 body, which chained only the
    /// two stacks, this frees 0 tiles instead of 2.
    fn forget_document_surfaces_frees_a_surface_only_the_journal_still_names() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Journalled", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        let content = content_surface(&tree, id);
        let mask = mask_surface(&tree, id);
        paint(&mut store, content, 5);
        if let Err(err) = crate::write_mask_coverage(&mut store, mask, tile(), 4, 4, 0.5) {
            unreachable!("{err:?}");
        }

        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }
        history.clear_redo();

        assert!(!tree.contains(id), "the add was undone");
        assert!(!history.can_undo(), "nothing left on the undo stack");
        assert!(!history.can_redo(), "and clear_redo emptied the other one");
        assert!(
            history.journal_len() > 0,
            "the journal is the only place left naming this subtree"
        );
        assert!(store.contains_tile(content, tile()));
        assert!(store.contains_tile(mask, tile()));

        assert_eq!(
            super::forget_document_surfaces(tree, history, &mut store),
            2,
            "a journal-only-reachable subtree's surfaces must still be swept"
        );
        assert!(!store.contains_tile(content, tile()));
        assert!(!store.contains_tile(mask, tile()));
    }

    // ---- `History::add_mask` clears stale mask coverage ------------

    /// Coverage the fixtures paint. Exactly representable as `f16`, so
    /// a read-back comparison is genuinely exact rather than close.
    const PAINTED: f32 = 0.25;

    /// What a *second* mask on the same layer paints, where a fixture
    /// needs to tell the two apart. Same exactness as [`PAINTED`], and
    /// deliberately neither that value nor the unpainted `1.0` default,
    /// so an assertion naming it cannot pass by accident.
    const REPAINTED: f32 = 0.75;

    /// Exact float equality expressed as bit equality -- the same shape
    /// (and the same reason) `mask.rs`'s own tests use: this workspace
    /// denies `clippy::float_cmp`, and these round trips really are
    /// exact.
    fn exactly(actual: f32, expected: f32) -> bool {
        actual.to_bits() == expected.to_bits()
    }

    /// Mask coverage at tile-local `(x, y)` of tile `(0, 0)`, read
    /// through the module that owns the storage convention.
    ///
    /// Note this *materializes* the tile if the store does not hold it,
    /// so a "was it freed?" assertion must use `contains_tile` and must
    /// come first.
    fn coverage_at(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
        x: usize,
        y: usize,
    ) -> f32 {
        let Ok(entry) = store.get(surface, tile()) else {
            unreachable!("a real store must serve this tile");
        };
        let base = (y * aurora_tile::TILE as usize + x) * aurora_tile::CHANNELS;
        let Some(texel) = entry.texels().get(base..base + aurora_tile::CHANNELS) else {
            unreachable!("(x, y) constructed in range for a whole tile");
        };
        crate::read_mask_coverage(texel)
    }

    fn paint_coverage(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
        x: usize,
        y: usize,
        coverage: f32,
    ) {
        if let Err(err) = crate::write_mask_coverage(store, surface, tile(), x, y, coverage) {
            unreachable!("{err:?}");
        }
    }

    #[test]
    /// The defect this round fixes, end to end.
    ///
    /// A mask surface id is derived from its layer's id, so a second
    /// mask on the same layer lands on the same surface the first one
    /// painted into. Before 0.81.0 the new mask opened wearing the old
    /// one's coverage, shifted by the offset between the two `bounds`
    /// origins -- which is why the re-add below deliberately uses a
    /// *different* rectangle.
    ///
    /// The first half of this test pins the other side of the same
    /// rule: `remove_mask` must NOT free those tiles, because undo
    /// needs them (see `undo_of_a_remove_mask_still_finds_its_painted_coverage`).
    fn add_mask_after_a_remove_starts_from_unpainted_coverage() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), PAINTED),
            "the fixture painted what it meant to"
        );

        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert!(
            store.contains_tile(mask, tile()),
            "removing the mask struct must leave its coverage behind -- undo needs it"
        );

        // A different rectangle, so the origin shift the old bug
        // produced is actually exercised rather than hidden by the two
        // masks happening to line up.
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
            unreachable!("{err:?}");
        }
        assert!(
            !store.contains_tile(mask, tile()),
            "a genuinely new mask must not inherit the removed one's tiles"
        );
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), 1.0),
            "and its coverage must read the never-painted default"
        );
    }

    #[test]
    // The blast radius. Clearing one layer's stale mask coverage must
    // not reach that layer's own pixel content, nor anything at all
    // belonging to another layer.
    fn add_mask_clears_only_that_layers_mask_surface() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(a) = history.add_pixel_layer(&mut tree, "A", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        let Ok(b) = history.add_pixel_layer(&mut tree, "B", bounds(), None) else {
            unreachable!("a tree accepts a second root layer");
        };
        for id in [a, b] {
            if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
                unreachable!("{err:?}");
            }
        }
        let (a_content, a_mask) = (content_surface(&tree, a), mask_surface(&tree, a));
        let (b_content, b_mask) = (content_surface(&tree, b), mask_surface(&tree, b));
        paint(&mut store, a_content, 1);
        paint(&mut store, b_content, 2);
        paint_coverage(&mut store, a_mask, 3, 4, PAINTED);
        paint_coverage(&mut store, b_mask, 5, 6, PAINTED);
        let a_content_before = texels(&mut store, a_content);
        let b_content_before = texels(&mut store, b_content);
        let b_mask_before = texels(&mut store, b_mask);

        if let Err(err) = history.remove_mask(&mut tree, a) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.add_mask(&mut tree, &mut store, a, other_bounds()) {
            unreachable!("{err:?}");
        }

        assert!(
            !store.contains_tile(a_mask, tile()),
            "A's stale mask coverage is the one thing that must go"
        );
        assert_eq!(
            texels(&mut store, a_content),
            a_content_before,
            "A's own pixel content must be untouched"
        );
        assert_eq!(
            texels(&mut store, b_content),
            b_content_before,
            "B's pixel content must be untouched"
        );
        assert_eq!(
            texels(&mut store, b_mask),
            b_mask_before,
            "B's mask coverage must be untouched"
        );
    }

    #[test]
    /// The anti-naive-implementation test for this round, and the
    /// reason the clear lives in `add_mask` rather than `remove_mask`.
    ///
    /// **This test fails against a "free the tiles when the mask is
    /// removed" implementation** -- the obvious shape, and a strictly
    /// worse regression than the resurrection it would fix: Ctrl+Z
    /// after removing a mask would bring the mask back blank, with the
    /// user's painted coverage already destroyed. It is the mask-shaped
    /// twin of
    /// `undo_of_a_remove_still_finds_the_removed_layers_painted_pixels`.
    fn undo_of_a_remove_mask_still_finds_its_painted_coverage() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Precious", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);

        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.undo(&mut tree) {
            unreachable!("{err:?}");
        }

        assert!(
            tree.mask(id).is_some(),
            "undo must put the mask back on the layer"
        );
        assert_eq!(
            mask_surface(&tree, id),
            mask,
            "the restored mask must address the same surface it painted into"
        );
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), PAINTED),
            "and its coverage must come back at the value written, not the default"
        );
    }

    #[test]
    /// The accepted, documented consequence of deriving a mask surface
    /// id from its layer's id, pinned here rather than left to be
    /// discovered.
    ///
    /// Once a genuinely new mask has been *committed* on a layer, the
    /// previous mask's coverage is gone for good: undoing back past the
    /// add restores the old `LayerMask` struct -- bounds, `enabled`,
    /// `inverted`, all exact -- but not its pixels, because the two
    /// masks share one surface and only one set of tiles can live
    /// there. Holding both would need a surface per mask *instance*,
    /// i.e. allocated rather than derived ids, which is a separate
    /// decision and deliberately not taken here.
    ///
    /// **This fixture leaves the replacement mask unpainted, and that
    /// makes it the mild half of the consequence.** Read alone it
    /// suggests the restored mask merely reads the unpainted default;
    /// paint the replacement first and it reads the *replacement's*
    /// coverage instead, shifted. `add_mask_undone_leaves_the_old_mask_reading_the_new_masks_coverage`
    /// below is that sibling, and is the honest statement of this
    /// residual's real shape.
    fn add_mask_makes_the_removed_masks_coverage_unrecoverable_by_undo() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);

        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
            unreachable!("{err:?}");
        }
        // Undo the add, then the remove: mask A's struct comes back.
        for _ in 0..2 {
            if let Err(err) = history.undo(&mut tree) {
                unreachable!("{err:?}");
            }
        }

        let Some(restored) = tree.mask(id) else {
            unreachable!("two undos must put the original mask back");
        };
        assert_eq!(
            restored.bounds,
            bounds(),
            "the struct restored is the *original* mask, not the replacement"
        );
        assert!(restored.enabled);
        assert!(!restored.inverted);
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), 1.0),
            "but its coverage is gone -- the accepted cost of a derived surface id"
        );
    }

    #[test]
    // Ordering, and why it is load-bearing: the tree edit runs first,
    // so a refusal returns before anything can touch the store. A
    // `MaskAlreadyExists` call must leave the *live* mask's coverage
    // completely alone -- clearing first and validating second would
    // destroy a user's mask on a call that did nothing else at all.
    fn add_mask_refused_as_already_existing_leaves_its_coverage_alone() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);
        let before = texels(&mut store, mask);

        match history.add_mask(&mut tree, &mut store, id, other_bounds()) {
            Err(DocError::MaskAlreadyExists(refused)) => assert_eq!(refused, id),
            other => unreachable!("expected MaskAlreadyExists, got {other:?}"),
        }

        assert!(
            store.contains_tile(mask, tile()),
            "a refused add must not free the live mask's tiles"
        );
        assert_eq!(
            texels(&mut store, mask),
            before,
            "and must leave its coverage byte-exact"
        );
        assert!(exactly(coverage_at(&mut store, mask, 3, 4), PAINTED));
    }

    #[test]
    /// The residual above, at full strength: undoing past an `add_mask`
    /// can restore an old mask that now reads a **newer** mask's
    /// coverage, shifted by the offset between the two `bounds` origins.
    ///
    /// This is character-for-character the defect shape 0.81.0 fixes
    /// going *forward* — one derived surface, two masks, coverage
    /// addressed relative to whichever origin is currently attached —
    /// still reachable going *backward* through undo, and reachable
    /// entirely through the ordinary [`History`] API
    /// (add → paint → remove → add → paint → undo → undo), with no
    /// `LayerTree::add_mask` bypass involved. There is no fix for it
    /// short of allocating a surface id per mask *instance* rather than
    /// deriving it from the layer id, which is a separate decision (see
    /// [`crate::mask`]'s lifecycle notes), so this test exists to pin
    /// the behaviour honestly rather than to assert it is desirable.
    ///
    /// The `1.0` an unpainted mask reads is not asserted here on
    /// purpose: the point is precisely that the restored mask does
    /// *not* read the default.
    fn add_mask_undone_leaves_the_old_mask_reading_the_new_masks_coverage() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);

        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        // A *different* rectangle, so the two masks address this one
        // texel at two different document positions -- the shift is the
        // whole point, not incidental.
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, other_bounds()) {
            unreachable!("{err:?}");
        }
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), 1.0),
            "the fixture starts the replacement mask unpainted, as 0.81.0 guarantees"
        );
        paint_coverage(&mut store, mask, 3, 4, REPAINTED);

        // Undo the add, then the remove: mask A's struct comes back.
        for _ in 0..2 {
            if let Err(err) = history.undo(&mut tree) {
                unreachable!("{err:?}");
            }
        }

        let Some(restored) = tree.mask(id) else {
            unreachable!("two undos must put the original mask back");
        };
        assert_eq!(
            restored.bounds,
            bounds(),
            "the struct restored is the *original* mask, not the replacement"
        );
        assert!(
            exactly(coverage_at(&mut store, mask, 3, 4), REPAINTED),
            "and it reads the *replacement's* coverage -- not its own, and not the \
             unpainted default -- interpreted against its own, different origin"
        );
    }

    #[test]
    // The ordering's third named refusal branch, which the
    // `MaskAlreadyExists` test above does not reach: an out-of-range
    // rectangle. Here the layer is *maskless* and carrying residual
    // coverage from a prior removal -- coverage a later successful add
    // is entitled to free, and a refused one is not. Clearing before
    // validating would destroy it on a call that changed nothing else,
    // and would take the undo of that removal down with it.
    fn add_mask_refused_for_an_out_of_range_rectangle_leaves_residual_coverage_alone() {
        let (_dir, mut store) = real_tile_store();
        let mut tree = LayerTree::new();
        let mut history = History::new();
        let Ok(id) = history.add_pixel_layer(&mut tree, "Masked", bounds(), None) else {
            unreachable!("an empty tree accepts a root layer");
        };
        if let Err(err) = history.add_mask(&mut tree, &mut store, id, bounds()) {
            unreachable!("{err:?}");
        }
        let mask = mask_surface(&tree, id);
        paint_coverage(&mut store, mask, 3, 4, PAINTED);
        if let Err(err) = history.remove_mask(&mut tree, id) {
            unreachable!("{err:?}");
        }
        let before = texels(&mut store, mask);

        // The same origin bar `tree.rs`'s
        // `add_mask_rejects_an_origin_past_the_document_range_and_leaves_the_layer_maskless`
        // drives, reached here through `History` and with tiles at stake.
        let far = Rect {
            x: -aurora_core::MAX_DOCUMENT_ORIGIN - 1,
            y: 0,
            width: 10,
            height: 10,
        };
        match history.add_mask(&mut tree, &mut store, id, far) {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, -aurora_core::MAX_DOCUMENT_ORIGIN - 1);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }

        assert!(
            tree.mask(id).is_none(),
            "a refused add must leave the layer maskless, not half-masked"
        );
        assert!(
            store.contains_tile(mask, tile()),
            "a refused add must not free the residual coverage undo still needs"
        );
        assert_eq!(
            texels(&mut store, mask),
            before,
            "and must leave it byte-exact"
        );
        assert!(exactly(coverage_at(&mut store, mask, 3, 4), PAINTED));
    }
}
