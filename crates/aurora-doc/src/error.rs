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
    /// [`crate::LayerTree::set_bounds`] was called on a layer that names
    /// a group — a group has no `bounds` of its own to move (see
    /// [`crate::LayerKind::Group`]).
    #[error("layer {0:?} is a group and has no bounds of its own to move")]
    NotAPixelLayer(LayerId),
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
    /// [`crate::History::save_journal`] failed. `postcard`'s own error
    /// type carries no useful structure for a caller to match on, so
    /// this holds its rendered message (via `to_string`) rather than
    /// the error value itself, avoiding a `postcard` dependency leaking
    /// into every downstream crate's own error-handling `match`.
    #[error("failed to serialize the crash-recovery journal: {0}")]
    JournalSerialization(String),
    /// [`crate::History::load_journal`] failed — the bytes weren't a
    /// valid, `postcard`-encoded journal.
    #[error("failed to deserialize the crash-recovery journal: {0}")]
    JournalDeserialization(String),
    /// A [`crate::LayerTree`] nests groups deeper than
    /// [`crate::MAX_LAYER_TREE_DEPTH`] — see that constant's own doc
    /// comment for why a bound exists at all.
    ///
    /// Returned from both ends of that bound, so that "a tree this
    /// crate's API will build" and "a tree its validators accept" stay
    /// the same set: by the live-edit producers
    /// [`crate::LayerTree::add_pixel_layer`],
    /// [`crate::LayerTree::add_group`] and
    /// [`crate::LayerTree::reparent`], which refuse an edit that would
    /// nest past the bound, *and* by the deserialize-time validator
    /// (and the history journal's own subtree splice), which refuses
    /// bytes already nested past it. The same pairing
    /// [`Self::OpacityOutOfRange`] already has.
    ///
    /// `depth` is the depth the refused layer — or, for
    /// [`crate::LayerTree::reparent`], the deepest node of the moved
    /// subtree — would have landed at.
    #[error("layer tree nests {depth} levels deep, past the {max}-level limit")]
    LayerTreeTooDeep { depth: usize, max: usize },
    /// A layer tree reaches the same layer twice while walking down
    /// from its roots — a group that (directly or through a chain of
    /// groups) contains itself, or the same layer listed as a child of
    /// two different parents. Neither is a tree, and every traversal in
    /// this crate assumes one. Returned by the deserialize-time
    /// validator for a manifest already in this shape, *and* by
    /// [`crate::LayerTree::reparent`]'s own internal subtree-height walk
    /// when the subtree being moved is already in this shape — reachable
    /// there only through a hand-built tree or the `test-support`
    /// escape hatch, never through this crate's own public API or any
    /// file it will read, since nothing else can construct one. The
    /// same "both a read-time and an edit-time source" pairing
    /// [`Self::LayerTreeTooDeep`] already has, and for the same reason.
    #[error(
        "layer {0:?} is reachable more than once from the layer tree's roots: a cycle, or the same layer listed under two parents"
    )]
    MalformedLayerTree(LayerId),
    /// A deserialized [`crate::LayerTree`] holds a layer whose own
    /// recorded `parent` disagrees with where the downward walk from the
    /// roots actually found it — it names a root while recording a
    /// parent, records `None` while sitting in some group's `children`,
    /// or records a *different* group from the one that listed it.
    ///
    /// Reachable only from crafted bytes: a `.aur` file's manifest, or
    /// the `RemovedSubtree` inside a crafted history journal's own
    /// `Restore` op. No path through [`crate::LayerTree`]'s public API
    /// can produce one — `insert`, `remove_capturing`, and `reparent`
    /// each write the sibling list and the entry's own `parent` field
    /// together. It matters because `remove_capturing` and `reparent`
    /// both *trust* that agreement: they read an entry's recorded
    /// `parent`, then look the id up in that parent's `children`. This
    /// variant is what they return instead of the process abort a
    /// violated assumption used to cause under this workspace's
    /// `panic = "abort"` release profile.
    #[error(
        "layer {0:?} records a parent that disagrees with where it actually sits in the layer tree"
    )]
    InconsistentLayerParent(LayerId),
    /// A deserialized [`crate::LayerTree`] holds a layer that the
    /// downward walk from its roots never reaches — it is in the
    /// `layers` map but named neither by `roots` nor by any group's
    /// `children`.
    ///
    /// Reachable only from crafted bytes (see
    /// [`Self::InconsistentLayerParent`] for the two sources). An
    /// unreachable entry is invisible to every traversal in this project
    /// yet still counted by `LayerTree::len`, still saved back out, and
    /// still able to make a later `remove`/`reparent` look up a sibling
    /// list that does not list it. When more than one entry is
    /// unreachable the *lowest-numbered* id is reported, so the error is
    /// the same on every run despite `HashMap`'s own arbitrary iteration
    /// order.
    #[error("layer {0:?} is in the layer tree's map but unreachable from its roots")]
    OrphanedLayer(LayerId),
    /// A deserialized [`crate::LayerTree`] names a layer that does not
    /// exist: an id in `roots`, or in some group's `children` list, with
    /// no entry of its own in the `layers` map.
    ///
    /// Reachable only from crafted bytes (see
    /// [`Self::InconsistentLayerParent`] for the two sources) — every
    /// path through [`crate::LayerTree`]'s own API writes a sibling list
    /// and the `layers` map together, so no tree this project *writes*
    /// can contain one.
    ///
    /// It was deliberately tolerated at first, on the reasoning that
    /// every traversal here already survives one (`LayerTree::kind`
    /// returns `None`) so refusing it would newly reject files this
    /// reader used to open. That was wrong in the part that mattered.
    /// A named-but-absent id is invisible to
    /// [`Self::StaleLayerIdGenerator`]'s check, which compares the
    /// counter against the ids actually *present*: a manifest naming id
    /// `1` from a group's `children` while carrying no entry for it
    /// passes both validators, and then the very next ordinary
    /// `add_pixel_layer` is handed exactly id `1` — making the new layer
    /// simultaneously a fresh root and an already-named child of that
    /// group. That is the "same layer reachable from two places" shape
    /// [`Self::MalformedLayerTree`] exists to forbid, manufactured
    /// *after* deserialization, where the one-shot validator can never
    /// see it. The immediate consequence is that saving and reopening
    /// the document fails; a later `reparent` on it can complete a real
    /// cycle, and a cycle reaching `aurora-app`'s own recursive
    /// `resolve_tile` is a stack overflow, which under this workspace's
    /// `panic = "abort"` profile is a process abort rather than a
    /// catchable error.
    #[error("the layer tree names layer {0:?}, which has no entry of its own")]
    DanglingLayerReference(LayerId),
    /// A `RemovedSubtree` being restored is internally inconsistent
    /// before its shape is even walked: it lists the same id twice, it
    /// names an id that the live tree already holds, or its own declared
    /// root is missing from the entries it carries.
    ///
    /// Reachable only from a crafted history journal — a
    /// `RemovedSubtree` produced by
    /// `crate::LayerTree::remove_capturing` always carries exactly the
    /// entries it just detached, each exactly once, root first.
    #[error("the removed subtree rooted at layer {0:?} is internally inconsistent")]
    MalformedRemovedSubtree(LayerId),
    /// A deserialized [`crate::LayerTree`]'s own `IdGenerator` counter
    /// sits at or behind a [`LayerId`] the same tree already uses, so
    /// the *next* layer added to it would be handed an id a live layer
    /// already holds.
    ///
    /// This is an allocator defect rather than a shape defect: such a
    /// tree is a perfectly well-formed tree, and every check in
    /// `validate_shape` passes on it. What it defeats is the assumption
    /// every insert path makes — that a freshly generated id is
    /// unused. Left unchecked, one ordinary `add_group` on a document
    /// loaded from a crafted `.aur` file overwrites the colliding entry
    /// (destroying it and orphaning whatever it held) and can splice the
    /// new group into its own `children`, building a cycle *after*
    /// deserialization, where the one-shot shape validator can never see
    /// it. Every downward walk then recurses forever, which under this
    /// workspace's `panic = "abort"` profile is a process abort, not a
    /// catchable error.
    ///
    /// Not reachable from a tree built through [`crate::LayerTree`]'s
    /// own API: its generator only ever moves forward, and
    /// `LayerTree::restore` (crate-private) advances it past every id
    /// it splices back in.
    #[error(
        "the layer tree's id counter is at {next}, but layer {existing:?} already uses that id or a higher one"
    )]
    StaleLayerIdGenerator { next: u64, existing: LayerId },
    /// A newly generated [`LayerId`] was already present in the tree's
    /// own map, so inserting under it would have silently destroyed the
    /// entry already there.
    ///
    /// The last line of defence behind
    /// [`Self::StaleLayerIdGenerator`]: that variant refuses the
    /// crafted *file*, this one refuses the *insert* whatever the
    /// generator's provenance. Unreachable through this crate's own API
    /// (`HashMap::insert`'s displaced value is checked rather than
    /// discarded, and it is always `None` for a monotonic generator on a
    /// validated tree), and returned rather than asserted so that a tree
    /// assembled some third way gets an error instead of losing a
    /// layer. Nothing is inserted when it happens.
    #[error("layer id {0:?} is already in use: refusing to overwrite the layer already under it")]
    LayerIdCollision(LayerId),
    /// A [`aurora_core::Rect`] handed to this crate's own editing API
    /// sits further from the document origin than
    /// [`aurora_core::MAX_DOCUMENT_ORIGIN`] on `x`, `y`, or both.
    ///
    /// Returned by [`crate::LayerTree::add_pixel_layer`] (and so by
    /// [`crate::History::add_pixel_layer`], which delegates to it before
    /// journalling anything), [`crate::LayerTree::set_bounds`] and
    /// [`crate::LayerTree::add_mask`] — every public path that stores a
    /// caller-supplied `Rect` **in the tree**. Nothing is changed when
    /// it fires: no layer is added, no bounds are replaced, no mask is
    /// attached, and `add_pixel_layer` does not even consume a
    /// [`LayerId`], since the check runs before the generator is
    /// touched.
    ///
    /// Since 0.57.14 it is **also** returned by
    /// [`crate::History::replay`], via the whole-tree walk
    /// `LayerTree::validate` runs as `replay`'s closing step. That path
    /// takes no `Rect` from its caller directly — it splices one out of
    /// a journal's `Restore`/`RestoreMask` entry, where the per-call
    /// guards above never see it — so it is listed separately rather
    /// than folded into the sentence above.
    ///
    /// One public method takes a caller-supplied `Rect` without storing
    /// it in the tree, and it is covered too, since 0.57.14:
    /// [`crate::History::record_bounds_change`] journals the caller's
    /// `old` rectangle as an undo entry for a change the caller already
    /// applied itself. It used to accept any `old` at all, on the
    /// argument that an out-of-range one reaches the journal and not the
    /// tree, and that the first thing to put it *in* the tree would be
    /// an ordinary undo, refused by [`crate::LayerTree::set_bounds`] and
    /// this same variant. That argument was right about where the value
    /// lands and wrong about the consequence: the refusal happens with
    /// the entry already on the undo stack, so *every* subsequent
    /// `undo()` fails identically and `can_undo()` stays `true` forever
    /// — undo wedged permanently, with no way back. The check now runs
    /// before anything is pushed, so a refusal leaves the journal and
    /// both stacks untouched, exactly as `set_bounds`' own refusal
    /// leaves the tree untouched.
    ///
    /// **Negative origins are still legal**, and deliberately so — a
    /// layer dragged partly or wholly off the canvas is an ordinary
    /// edit, which is what `Rect`'s signed `x`/`y` are for. What is
    /// refused is an origin further out than one whole document extent
    /// (300,000 px, PRD §7.3.1 / ADR 0002) in either direction; see
    /// [`aurora_core::MAX_DOCUMENT_ORIGIN`] for why the bound is that
    /// number and why it does not also fold in the rectangle's own
    /// width/height.
    ///
    /// **The `.aur` read-time twin lives in `aurora-io`**, as
    /// `IoError::LayerOriginOutOfRange`, and not in this crate's own
    /// deserializer.
    ///
    /// An earlier draft of this comment justified that by saying an
    /// out-of-range origin is "inert to every traversal in this crate",
    /// unlike [`Self::LayerTreeTooDeep`], which is a property of the
    /// tree's own shape and so has to be checked here. Half of that is
    /// sound — depth really does defeat this crate's own walks, which
    /// is why it is checked at both ends — but the other half does not
    /// distinguish anything: [`crate::LayerTree`]'s deserializer *also*
    /// runs `validate_opacities`, which is likewise a pure value-range
    /// check, likewise inert to every traversal here, and likewise
    /// endangers only a downstream consumer (the compositor, handed a
    /// `NaN`). By the stated rule that one belongs in `aurora-io` too,
    /// and it is not there.
    ///
    /// **The real distinction is what error the caller gets.** This
    /// crate's `Deserialize` is `#[serde(try_from = "LayerTreeRepr")]`,
    /// so a `DocError` raised inside it is converted through `Display`
    /// into a `serde` error and reaches `aurora-io` as
    /// `IoError::ManifestDeserialization(String)` — a stringified
    /// message with no structured fields, which is exactly why
    /// `aurora-doc`'s own tests assert those cases against
    /// `validate_shape` directly "rather than the deserializer's own
    /// message". A check in `aurora-io` returns a typed
    /// `IoError::LayerOriginOutOfRange { x, y, max }` instead, which a
    /// caller can match on and report the file's actual numbers from.
    /// Origin is checked there for that reason, and because the same
    /// guard then also covers the *write* path (`tile_grid` and
    /// `validate_mask_origins` are shared by `read`, `write` and
    /// `write_best_effort`), which a deserializer cannot reach at all.
    /// `validate_opacities` stays in the deserializer because it is
    /// load-bearing for a second, non-`.aur` entry point —
    /// `LayerTree::restore`'s spliced subtree — where there is no
    /// `aurora-io` boundary to put it at.
    #[error(
        "layer origin ({x}, {y}) is further than {max}px from the document origin on at least one axis"
    )]
    LayerOriginOutOfRange { x: i64, y: i64, max: i64 },
    /// A rectangle's own *extent* is past the document ceiling
    /// ([`aurora_core::MAX_DOCUMENT_EXTENT`], 300,000 px — PRD §7.3.1 /
    /// ADR 0002). [`Self::LayerOriginOutOfRange`]'s companion: that one
    /// is about where a rectangle sits, this one about how big it is.
    ///
    /// **Only [`crate::LayerTree::add_mask`] raises it today, and that
    /// asymmetry is deliberate rather than an oversight** (0.71.3). A
    /// mask's rectangle drives a real tile grid in `aurora-io`'s `.aur`
    /// writer, and an oversized one there is not a big loop but an
    /// unfinishable one — so it is refused at the point the mask is
    /// created, not only at the file boundary, because a tree that
    /// already holds one makes *every* save and *every* autosave for
    /// the rest of the session fail. `add_pixel_layer`/`set_bounds` do
    /// **not** carry this check: a layer's extent has meaning beyond
    /// the tile grid (it is the document's own content extent, and
    /// `aurora_core::Size::new` is where that ceiling is owned), and
    /// tightening it here would be a live-editing policy change rather
    /// than a hardening fix. `aurora-io`'s own hoisted
    /// `validate_persisted_rects` refuses an oversized layer before it
    /// writes a byte, which is what keeps that gap from producing a
    /// partial file.
    #[error("rectangle extent {width}x{height} is past the {max}px document ceiling")]
    LayerBoundsTooLarge { width: u32, height: u32, max: u32 },
    /// A [`LayerId`] read back from an untrusted manifest (or journal)
    /// has [`crate::MASK_SURFACE_BIT`] set, which would put its own
    /// *pixel* surface into the half of the `aurora_tile::SurfaceId`
    /// space reserved for **mask** surfaces.
    ///
    /// # The collision this refuses
    ///
    /// [`crate::LayerTree::surface_id`] is `id.to_raw()` and
    /// [`crate::LayerTree::mask_surface_id`] is
    /// `id.to_raw() | MASK_SURFACE_BIT`. Those two ranges are disjoint
    /// only while every live id keeps that bit clear. A crafted `.aur`
    /// manifest holding both layer `5` (with a mask) and layer
    /// `5 | MASK_SURFACE_BIT` (an ordinary pixel layer) makes the
    /// second layer's own pixel surface and the first layer's mask
    /// surface the *same* `SurfaceId` — one tile-store slot with two
    /// owners, so painting one silently rewrites the other's coverage,
    /// with no error anywhere.
    ///
    /// Nothing built through this crate's own API can reach it:
    /// `aurora_core::IdGenerator` starts at `0` and hands ids out one
    /// at a time, so a real session would need `2^63` layer creations.
    /// But `IdGenerator` and `LayerId` are both `Deserialize`, and
    /// [`Self::StaleLayerIdGenerator`]'s counter check compares ids
    /// only against `peek_next()` — which a crafted manifest is free to
    /// set to `u64::MAX`. That is why this is a validation rule and not
    /// merely a comment.
    #[error(
        "layer id {id:?} has the reserved mask-surface bit set: layer ids must stay below {limit}"
    )]
    ReservedLayerIdBit { id: LayerId, limit: u64 },
    /// The layer tree's own id *counter* is at or past
    /// [`crate::MASK_SURFACE_BIT`], so the next ordinary
    /// `add_pixel_layer`/`add_group` would hand out an id
    /// [`Self::ReservedLayerIdBit`] refuses.
    ///
    /// [`Self::ReservedLayerIdBit`] is to this what
    /// [`Self::LayerIdCollision`] is to
    /// [`Self::StaleLayerIdGenerator`]: that one refuses ids the file
    /// already carries, this one refuses a counter positioned to
    /// *create* one on the very next insert. Both halves are needed
    /// for the same reason the stale-counter pair is — a manifest can
    /// be shape-perfect and still be unsafe to add to.
    #[error(
        "the layer tree's id counter is at {next}, at or past the reserved mask-surface bit {limit}"
    )]
    ReservedLayerIdCounter { next: u64, limit: u64 },
}
