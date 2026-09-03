//! The layer tree itself: identity, nesting, and ordering. PLAN.md M1.4's
//! first deliverable.

use std::collections::{HashMap, HashSet};

use aurora_core::{IdGenerator, Rect};

use crate::error::DocError;
use crate::layer::{BlendMode, Layer, LayerEntry, LayerId, LayerKind, LayerLock, LayerMask};

/// The deepest group nesting a [`LayerTree`] may reach, however it was
/// built — see [`LayerTree`]'s own `Deserialize` notes for the whole
/// rationale.
///
/// Enforced at both ends, so the set of trees this crate's API will
/// build and the set its validators accept are the same set: this
/// module's own `validate_shape` refuses bytes (and a spliced subtree)
/// already nested past it, and
/// [`LayerTree::add_pixel_layer`]/[`LayerTree::add_group`]/
/// [`LayerTree::reparent`] refuse a live edit that would nest past it.
/// The producer half was missing until 0.50.0, which meant ordinary
/// editing could grow a document that no `.aur` save could ever
/// round-trip — `aurora-io` verifies a write by reopening, and the read
/// side runs that same validator.
///
/// The number is deliberately far above any real document and far below
/// anything that strains a traversal: Photoshop's own UI has never let a
/// user nest groups more than ten deep, so 256 is roughly twenty-five
/// times past the deepest document anyone actually has.
///
/// **What it does and does not bound.** It caps *depth*, and nothing
/// else. It is tempting to read it as a memory bound on the one
/// traversal in this project that allocates per level — `aurora-app`'s
/// own recursive `resolve_tile`, which holds about half a megabyte of
/// tile buffer per contributor — and an early version of this comment
/// claimed exactly that. The claim was wrong *at the time*:
/// `resolve_tile` then collected one such buffer per *sibling* before
/// compositing them, so its peak was `O(siblings × depth)`, and a single
/// group holding a few thousand ordinary sibling layers already cost a
/// gigabyte on a ten-pixel-square document. That was fixed in 0.51.0 —
/// `resolve_tile` now folds each child into one running accumulator via
/// `aurora_render::composite_layer_into` and drops it before resolving
/// the next, so its peak really is `O(depth)`. Only *now* does this
/// constant bound that allocation, and only loosely.
///
/// The peak is `depth + 1` tile buffers, not `2 × depth`. Each `Group`
/// frame allocates exactly one accumulator; a child's own buffer is
/// bound only *after* the recursive call for it returns, and dropped at
/// the end of that loop iteration before the next child recurses. So
/// while the recursion is descending, every ancestor frame holds one
/// buffer and no second one — the frame that has just been returned
/// into is the only one ever holding two at once (its accumulator plus
/// the buffer moved out of the callee, which is the callee's own
/// accumulator moved, not copied). At 256 levels that is 257 buffers of
/// [`aurora_tile::SAMPLES`] `f16`s (512 KiB each), roughly **128.5 MiB**
/// worst case, for a document nested twenty-five times deeper than any
/// that exists. (An earlier version of this paragraph said ~256 MiB on
/// the reasoning that every level holds an accumulator *and* a transient
/// child buffer simultaneously; review 2026-08-24 re-derived it from the
/// control flow and found that wrong — only one level at a time is ever
/// in that state.) It is a backstop, not a budget — the number was
/// chosen for traversal sanity, not for memory, and nothing here should
/// be read as a claim of constant memory. Likewise, because that
/// traversal recurses
/// once per child rather than once per level, a tree that defeats
/// `validate_shape`'s duplicate check would fan out exponentially
/// rather than merely deeply; `aurora-app` bounds that with its own
/// per-tile node budget rather than relying on this constant.
pub const MAX_LAYER_TREE_DEPTH: usize = 256;

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

impl RemovedSubtree {
    /// Every `aurora_tile::SurfaceId` the captured entries address —
    /// the detached counterpart of [`LayerTree::all_surfaces`], for a
    /// subtree that is no longer in any tree and so cannot be asked.
    ///
    /// **This deliberately mirrors [`LayerTree::surface_id`] and
    /// [`LayerTree::mask_surface_id`] by hand**, because their real
    /// bodies need a live tree to look an entry's kind up in and there
    /// isn't one here. The two guards reproduced below are theirs,
    /// exactly:
    ///
    /// - a content surface only for [`LayerKind::Pixel`], and only for
    ///   an id below [`crate::MASK_SURFACE_BIT`] (an id with that bit
    ///   set would address some other layer's *mask* storage);
    /// - a mask surface for any kind, groups included, but only for an
    ///   id below `MASK_SURFACE_BIT - 1` (the single id whose masked
    ///   form is `u64::MAX`, `aurora-app`'s reserved composite
    ///   surface).
    ///
    /// If these ever diverge from the two functions they copy, this
    /// returns a surface id some *other* owner holds — freeing the
    /// wrong layer's pixels, or sweeping the composite surface — so
    /// change all three together or none.
    pub(crate) fn surfaces(&self) -> Vec<aurora_tile::SurfaceId> {
        // Two per entry, not one: a pixel layer contributes both a
        // content and a mask surface below.
        let mut out = Vec::with_capacity(self.entries.len().saturating_mul(2));
        for (id, entry) in &self.entries {
            if id.to_raw() < crate::MASK_SURFACE_BIT
                && matches!(entry.kind, LayerKind::Pixel { .. })
            {
                out.push(aurora_tile::SurfaceId::from_raw(id.to_raw()));
            }
            if id.to_raw() < crate::MASK_SURFACE_BIT - 1 {
                out.push(aurora_tile::SurfaceId::from_raw(
                    id.to_raw() | crate::MASK_SURFACE_BIT,
                ));
            }
        }
        out
    }
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
/// [`LayerMask`] (see [`Self::add_mask`] and neighbours). The mask's
/// own grayscale *pixels* do not live in the tree either: they live in
/// the document's shared `aurora_tile::TileStore`, under
/// [`Self::mask_surface_id`] — see [`crate::mask`] for the storage
/// convention and for what is deliberately still missing around it.
///
/// `Serialize`/`Deserialize`: a `.aur` file's own manifest entry (ADR
/// 0009) is the whole tree, `postcard`-encoded — every field here
/// (including `ids`, via `IdGenerator`'s own hand-written impl) needs to
/// round-trip so a reloaded document keeps allocating fresh, non-
/// colliding `LayerId`s rather than restarting the counter from `0`.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(try_from = "LayerTreeRepr")]
pub struct LayerTree {
    ids: IdGenerator<Layer>,
    layers: HashMap<LayerId, LayerEntry>,
    /// Root-level layers. See this type's own doc comment for the
    /// ordering convention.
    roots: Vec<LayerId>,
}

/// [`LayerTree`]'s own on-the-wire shape — field-for-field identical to
/// it, so `postcard`'s positional encoding is unchanged and every `.aur`
/// file this project has ever written keeps loading (ADR 0009's own
/// backward-compatibility policy).
///
/// It exists so that `LayerTree`'s derived `Deserialize` can go through
/// [`LayerTree::try_from`] and **validate the tree's shape before any
/// caller ever traverses it** (`#[serde(try_from = "LayerTreeRepr")]`).
/// Without that step, a `LayerTree`'s `Deserialize` was purely
/// structural: nothing stopped a hand-crafted manifest from declaring a
/// [`LayerKind::Group`] whose `children` list names the group itself.
/// Every downward walk in this project (this crate's own
/// [`LayerTree::paint_order`], `aurora_io::aur`'s own pixel-layer scan,
/// `aurora-app`'s own `resolve_tile`) then recurses forever on it — and
/// a stack overflow is not a catchable `Err` but a process abort, on a
/// path `aurora-app` runs *before it has a window* (crash-recovery
/// autosave) and again whenever a user opens a `.aur` file they were
/// sent. A 226-byte crafted file was enough to abort the process, so
/// this is validated here, once, rather than defended against
/// separately in every traversal.
///
/// The validated scope has since grown past "no cycles, not too deep":
/// [`validate_shape`] also holds each entry's own recorded `parent` to
/// the shape the downward walk actually found, and refuses an entry the
/// walk never reaches at all. That closes a second, quieter abort of
/// exactly the same class — [`LayerTree::remove_capturing`] and
/// [`LayerTree::reparent`] read an entry's recorded `parent` and then
/// expect to find the id in that parent's sibling list, and a crafted
/// manifest could make that expectation false.
///
/// The `ids` field is checked too, by [`validate_id_allocator`], and
/// that one is not about shape at all. `IdGenerator` is `Deserialize`,
/// so a manifest can hand back a counter that has *already been used* —
/// while every shape rule above passes, because the tree really is a
/// tree. One ordinary `add_group` on such a document then generates an
/// id a live layer already holds, and (before this) silently replaced
/// it, orphaning its contents and splicing the replacement into its own
/// `children`: a cycle constructed *after* deserialization, where a
/// validator that runs once, here, can never see it. The counter is
/// therefore held to "strictly ahead of every id present", and
/// [`LayerTree::insert`] refuses a colliding id besides.
///
/// "Every id *present*" is load-bearing, and was for one round a gap of
/// its own: a manifest could *name* an id — from `roots`, or from a
/// group's `children` — while carrying no entry for it, and such an id is
/// present in neither `layers.keys()` nor anything the shape walk counts.
/// Both validators passed, and the next ordinary `add_pixel_layer` was
/// handed exactly that id, making the new layer a fresh root *and* an
/// already-named child at once. [`validate_shape`] therefore refuses a
/// dangling reference outright ([`DocError::DanglingLayerReference`])
/// rather than skipping it, which is what makes "every id present" and
/// "every id the tree names" the same set — and so makes the counter
/// check above complete rather than merely nearly complete.
///
/// One thing checked here is not about structure at all:
/// [`validate_opacities`] holds each entry's `opacity`/`fill_opacity` to
/// the `0.0..=1.0` range [`LayerTree::set_opacity`] enforces on live
/// edits, since a crafted manifest can otherwise hand the compositor
/// `NaN`.
#[derive(serde::Deserialize)]
struct LayerTreeRepr {
    ids: IdGenerator<Layer>,
    layers: HashMap<LayerId, LayerEntry>,
    roots: Vec<LayerId>,
}

impl TryFrom<LayerTreeRepr> for LayerTree {
    type Error = DocError;

    fn try_from(repr: LayerTreeRepr) -> Result<Self, Self::Error> {
        let LayerTreeRepr { ids, layers, roots } = repr;
        // `None`: a manifest's `roots` are the root-level sibling list by
        // definition, so every layer named there must record no parent
        // of its own. (`LayerTree::restore` is the other caller, and
        // passes the captured subtree's own recorded parent instead.)
        // `1`: these roots really are the top level, so the depth budget
        // starts from scratch.
        validate_shape(&layers, &roots, None, 1)?;
        // Shape first, then the allocator: a file can be wrong in both
        // ways at once, and a malformed *shape* is the one that aborts a
        // traversal outright, so it is the more useful thing to report.
        // Property ranges last, for the same ordering reason -- an
        // out-of-range opacity renders wrongly, which is worth refusing
        // but strictly less urgent than a traversal that never returns.
        validate_id_allocator(&ids, &layers)?;
        // Same family as the allocator check, and for the same reason
        // it sits next to it rather than inside `validate_shape`: an id
        // with `MASK_SURFACE_BIT` set makes a perfectly well-formed
        // tree that aliases another layer's mask storage.
        validate_layer_id_range(&ids, &layers)?;
        validate_opacities(&layers)?;
        Ok(Self { ids, layers, roots })
    }
}

/// Walks `roots` downward through `layers`, rejecting anything that
/// isn't a real tree — see [`LayerTreeRepr`] for why this runs at
/// deserialization time.
///
/// `root_parent` is the parent the *starting* sibling list hangs under,
/// so one validator serves both callers: [`LayerTree::try_from`] passes
/// `None` (a manifest's `roots` are root-level by definition), and
/// [`LayerTree::restore`] passes the captured subtree's own recorded
/// parent with `roots = &[subtree_root]`.
///
/// `start_depth` is the depth the entries in `roots` themselves sit at,
/// so the depth budget is cumulative rather than per-call: a manifest's
/// own roots are depth `1`, while a subtree [`LayerTree::restore`]
/// splices under a live group starts one level below however deep that
/// group already sits. Without it, two crafted `Restore` ops could each
/// stay inside [`MAX_LAYER_TREE_DEPTH`] and still build a tree twice
/// that deep between them.
///
/// Iterative on an explicit stack, never recursive: the whole point is
/// to survive input designed to blow the call stack, so the validator
/// itself must not be the thing that overflows. Four rules:
///
/// - **Each layer is reached at most once.** A second visit means either
///   a cycle (a group inside itself) or the same layer listed under two
///   parents; neither is a tree, and both make a downward walk either
///   loop forever or duplicate work exponentially.
/// - **Nesting stays within [`MAX_LAYER_TREE_DEPTH`].**
/// - **Every layer's own recorded `parent` agrees with where the walk
///   found it** — `root_parent` for a starting-list entry, and the
///   group that named it for anything reached through a `children`
///   list. Otherwise [`DocError::InconsistentLayerParent`]. Because a
///   child is only ever descended into *through* some group's
///   `children`, "recorded equals actual" also proves, for free, that a
///   non-`None` recorded parent exists and really is a group — which is
///   exactly the assumption [`LayerTree::remove_capturing`] and
///   [`LayerTree::reparent`] make when they read an entry's `parent`
///   and then look the id up in that parent's sibling list.
/// - **No orphans**: every entry in `layers` is reached. Otherwise
///   [`DocError::OrphanedLayer`], naming the lowest-numbered
///   unreachable id so the error does not depend on `HashMap`'s
///   arbitrary iteration order.
///
/// - **No dangling references**: every id named — in `roots`, or in
///   some group's `children` — has an entry of its own in `layers`.
///   Otherwise [`DocError::DanglingLayerReference`]. Note this is a
///   *different* case from the orphan rule above, and its exact mirror:
///   an orphan is an entry nothing names, a dangling reference is a name
///   with no entry.
///
/// That last rule was, in the first round of this validation, the
/// opposite: a dangling id was *skipped* rather than rejected, reasoned
/// as "every traversal here already tolerates one (`kind` returns
/// `None`), so rejecting it would newly refuse files this reader used to
/// open". The reasoning was wrong in the part that mattered. No tree
/// this project *writes* can contain a dangling reference —
/// [`LayerTree::insert`], [`LayerTree::remove_capturing`],
/// [`LayerTree::reparent`] and [`LayerTree::restore`] each write a
/// sibling list and the `layers` map together — so nothing legitimate is
/// refused. Meanwhile skipping it left the named-but-absent id invisible
/// to [`validate_id_allocator`], which compares the counter only against
/// ids actually *present*: a manifest naming id `1` from a group's
/// `children` while carrying no entry for it passed both validators, and
/// the very next ordinary `add_pixel_layer` was then handed exactly id
/// `1` — a layer that is simultaneously a fresh root and an
/// already-named child of that group. Refusing the reference outright is
/// what keeps "every id present" and "every id named" the same set, so
/// that [`validate_id_allocator`] checking the first really does cover
/// the second.
///
/// Rejecting rather than skipping does not loosen the bound on the
/// explicit stack, which is still `layers.len()`: only ids that really
/// exist are ever pushed, and each is pushed at most once.
fn validate_shape(
    layers: &HashMap<LayerId, LayerEntry>,
    roots: &[LayerId],
    root_parent: Option<LayerId>,
    start_depth: usize,
) -> Result<(), DocError> {
    let mut seen: HashSet<LayerId> = HashSet::with_capacity(layers.len());
    let mut stack: Vec<(LayerId, usize)> = Vec::new();

    for &id in roots {
        let Some(entry) = layers.get(&id) else {
            return Err(DocError::DanglingLayerReference(id));
        };
        // The duplicate/cycle check stays *before* the parent check: a
        // group listing itself is both, and it is the cycle that every
        // downward traversal actually cannot survive, so that is the
        // error worth reporting.
        if !seen.insert(id) {
            return Err(DocError::MalformedLayerTree(id));
        }
        if entry.parent != root_parent {
            return Err(DocError::InconsistentLayerParent(id));
        }
        stack.push((id, start_depth));
    }

    while let Some((id, depth)) = stack.pop() {
        if depth > MAX_LAYER_TREE_DEPTH {
            return Err(DocError::LayerTreeTooDeep {
                depth,
                max: MAX_LAYER_TREE_DEPTH,
            });
        }
        // Nothing that is not present in `layers` is ever pushed (both
        // push sites above return `DanglingLayerReference` first), so
        // this cannot miss -- kept as a `continue` rather than an
        // assertion because a validator whose whole job is surviving
        // hostile input must not be the thing that aborts.
        let Some(entry) = layers.get(&id) else {
            continue;
        };
        let LayerKind::Group { children } = &entry.kind else {
            continue;
        };
        for &child in children {
            let Some(child_entry) = layers.get(&child) else {
                return Err(DocError::DanglingLayerReference(child));
            };
            // Same ordering as the root loop above, for the same reason.
            if !seen.insert(child) {
                return Err(DocError::MalformedLayerTree(child));
            }
            if child_entry.parent != Some(id) {
                return Err(DocError::InconsistentLayerParent(child));
            }
            stack.push((child, depth.saturating_add(1)));
        }
    }

    if seen.len() != layers.len() {
        // Deterministic despite `HashMap`'s arbitrary iteration order.
        // `aurora_core::Id<T>` deliberately has no `Ord` (it is an
        // opaque handle, not a sortable value), so the minimum is taken
        // over the raw values rather than over the ids themselves.
        if let Some(orphan) = layers
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .min_by_key(|id| id.to_raw())
        {
            return Err(DocError::OrphanedLayer(orphan));
        }
    }
    Ok(())
}

/// Holds a tree's own id counter to the ids the tree actually contains:
/// the next id the generator would hand out must be strictly greater
/// than every raw id present in `layers`.
///
/// [`validate_shape`]'s companion, and deliberately separate from it,
/// because the defect is of a different kind. A tree whose counter has
/// fallen behind is a perfectly well-formed *tree* — every shape rule
/// passes — so nothing in the shape walk can see it. What it breaks is
/// the invariant every insert path relies on: that a freshly generated
/// id is unused. One `add_group` on such a tree hands out an id a live
/// layer already holds, `HashMap::insert` replaces that layer, and the
/// new group is spliced into the sibling list that the *replaced* entry
/// was named in — for a group listing itself as its own child, that is a
/// cycle built after deserialization, which the one-shot shape validator
/// never gets a second look at. See
/// [`DocError::StaleLayerIdGenerator`].
///
/// Deterministic despite `HashMap`'s arbitrary iteration order: the
/// *highest* colliding id is reported, which is unique.
///
/// Comparing against `layers.keys()` — the ids actually *present* —
/// covers every id the tree names only because [`validate_shape`] refuses
/// a name with no entry behind it. See [`LayerTreeRepr`] for the round
/// where that was not yet true and this check could be walked around.
fn validate_id_allocator(
    ids: &IdGenerator<Layer>,
    layers: &HashMap<LayerId, LayerEntry>,
) -> Result<(), DocError> {
    let next = ids.peek_next();
    if let Some(highest) = layers
        .keys()
        .copied()
        .filter(|id| id.to_raw() >= next)
        .max_by_key(|id| id.to_raw())
    {
        return Err(DocError::StaleLayerIdGenerator {
            next,
            existing: highest,
        });
    }
    Ok(())
}

/// Rejects any [`LayerId`] — present in `layers`, or about to be handed
/// out by `ids` — that has [`crate::MASK_SURFACE_BIT`] set.
///
/// [`validate_id_allocator`]'s neighbour, and a *different* defect
/// again. That one holds the counter ahead of the ids present; this one
/// holds both to the half of the `aurora_tile::SurfaceId` space that
/// belongs to layer pixels. [`LayerTree::surface_id`] is
/// `id.to_raw()` unchanged and [`LayerTree::mask_surface_id`] is
/// `id.to_raw() | MASK_SURFACE_BIT`, so the two ranges are disjoint
/// only while no live id sets that bit — and a crafted manifest can set
/// it, since both [`LayerId`] and `IdGenerator` are `Deserialize` and
/// [`validate_id_allocator`] compares ids only against a `peek_next()`
/// the same file supplies. A document holding layer `5` (masked) and
/// layer `5 | MASK_SURFACE_BIT` (a plain pixel layer) then addresses
/// the second layer's pixels and the first layer's mask coverage
/// through one and the same tile-store slot.
///
/// Both halves are checked because either alone leaves a door open: the
/// map check refuses the ids the file already carries, the counter
/// check refuses a counter positioned to create one on the next
/// ordinary insert. See [`DocError::ReservedLayerIdBit`] and
/// [`DocError::ReservedLayerIdCounter`].
///
/// Deterministic despite `HashMap`'s arbitrary iteration order: the
/// *lowest* offending id is reported, which is unique.
///
/// Only `layers.keys()` is walked, for the same reason
/// [`validate_id_allocator`] can get away with it — [`validate_shape`]
/// refuses a name with no entry behind it, so "every id present" and
/// "every id named" are the same set.
fn validate_layer_id_range(
    ids: &IdGenerator<Layer>,
    layers: &HashMap<LayerId, LayerEntry>,
) -> Result<(), DocError> {
    if let Some(id) = layers
        .keys()
        .copied()
        .filter(|id| id.to_raw() >= crate::MASK_SURFACE_BIT)
        .min_by_key(|id| id.to_raw())
    {
        return Err(DocError::ReservedLayerIdBit {
            id,
            limit: crate::MASK_SURFACE_BIT,
        });
    }
    let next = ids.peek_next();
    if next >= crate::MASK_SURFACE_BIT {
        return Err(DocError::ReservedLayerIdCounter {
            next,
            limit: crate::MASK_SURFACE_BIT,
        });
    }
    Ok(())
}

/// Rejects a group `children` reference that points from one of two
/// about-to-be-merged maps into the other.
///
/// [`LayerTree::restore`] splices a captured subtree into a live tree,
/// and each half was only ever validated on its own: [`validate_shape`]
/// walks `incoming` alone, and the live tree's own shape was settled back
/// when it was deserialized. Neither walk can see a reference that points
/// *across* the two, because to the map it was handed the id simply names
/// nothing — and [`validate_shape`]'s own dangling-reference rule is no
/// help here either, since this is precisely the case where the reference
/// is not dangling: the instant the maps are merged it resolves, and the
/// layer it names becomes reachable from two parents at once. Caught
/// before the merge, because afterwards no walk of either half alone can
/// see it.
///
/// Called in **both** directions by [`LayerTree::restore`], because the
/// defect is symmetric and one direction alone leaves the other open: an
/// incoming group's `children` naming a live layer, and a live group's
/// `children` naming an incoming one, produce the same two-parent shape.
/// (The second direction only became reachable at all through a live tree
/// that already carried a dangling reference — which [`validate_shape`]
/// now refuses outright — so this is the belt to that fix's braces, kept
/// because a cycle reaching `aurora-app`'s recursive `resolve_tile` is a
/// process abort, not a catchable error.)
///
/// One implementation used twice rather than two hand-written checks
/// that must be kept in step: the earlier rounds of this work established
/// that discipline for [`validate_shape`] (shared by the manifest and
/// splice paths) and it applies just as much here.
fn validate_cross_references(
    from: &HashMap<LayerId, LayerEntry>,
    to: &HashMap<LayerId, LayerEntry>,
) -> Result<(), DocError> {
    for entry in from.values() {
        let LayerKind::Group { children } = &entry.kind else {
            continue;
        };
        for child in children {
            if !from.contains_key(child) && to.contains_key(child) {
                return Err(DocError::MalformedRemovedSubtree(*child));
            }
        }
    }
    Ok(())
}

/// Holds every deserialized layer's `opacity` and `fill_opacity` to the
/// same `0.0..=1.0` bar [`LayerTree::set_opacity`] enforces on a live
/// edit.
///
/// Both are plain `f32` fields in the manifest, so a crafted file can
/// carry `NaN`, a negative value, or `1e38` straight past validators that
/// only look at ids, and on into the compositor — which multiplies texels
/// by them. Unlike the shape defects this module's other validators
/// refuse, this is not a crash: it is a rendering-correctness one, of the
/// same "trust a number from an untrusted file" class the rest of this
/// round has been closing (contrast `Rect`, whose *extent* `aurora-io`
/// bounds on the way in, and whose *origin* it now bounds too — the
/// parenthetical here used to say "`Rect`, which `aurora-io` already
/// bounds", which overclaimed: only the extent was checked until
/// `IoError::LayerOriginOutOfRange` landed).
///
/// Rejected rather than clamped, so that the file's own value is what
/// gets reported and a document never silently renders as something other
/// than what it says. The range test is [`LayerTree::set_opacity`]'s,
/// character for character — which also rejects `NaN`, since a
/// `RangeInclusive::contains` is false for it.
///
/// Deterministic despite `HashMap`'s arbitrary iteration order: when
/// several layers are out of range, the one with the lowest raw
/// [`LayerId`] is the one reported — the same discipline
/// [`validate_shape`] states for its own orphan report, and for the same
/// reason (a validator that names a different offender on each run makes
/// a bug report unreproducible). Within one entry, `opacity` is reported
/// ahead of `fill_opacity`.
fn validate_opacities(layers: &HashMap<LayerId, LayerEntry>) -> Result<(), DocError> {
    // `aurora_core::Id<T>` deliberately has no `Ord`, so the minimum is
    // taken over the raw values rather than over the ids themselves --
    // see `validate_shape`'s own note.
    if let Some((_, value)) = layers
        .iter()
        .filter_map(|(id, entry)| offending_opacity(entry).map(|value| (id.to_raw(), value)))
        .min_by_key(|(raw, _)| *raw)
    {
        return Err(DocError::OpacityOutOfRange(value));
    }
    Ok(())
}

/// The first of `entry`'s two opacities outside `0.0..=1.0`, if either
/// is. Split out of [`validate_opacities`] so the "is this entry an
/// offender" test and the "what value gets reported" answer cannot drift
/// apart across the two passes the deterministic selection needs.
fn offending_opacity(entry: &LayerEntry) -> Option<f32> {
    [entry.opacity, entry.fill_opacity]
        .into_iter()
        .find(|value| !(0.0..=1.0).contains(value))
}

/// Whole-tree counterpart to [`validate_origin`]: holds every stored
/// `Rect` origin in `layers` to that same shared predicate, so a tree
/// assembled by some route other than the per-call guards can still be
/// refused as a whole.
///
/// The mask check sits *outside* the `kind` match on purpose. A group
/// carries a mask too, so testing only the `Pixel` arm would leave a
/// masked group's own origin unchecked — the same reasoning
/// `aurora-io`'s `validate_persisted_rects` follows when it walks every
/// layer id regardless of kind.
///
/// [`LayerTree::restore`] and [`LayerTree::restore_mask`] still do not
/// call this (nor [`validate_origin`]) for the reason
/// [`validate_origin`]'s own doc comment gives: re-checking there would
/// let an ordinary undo fail on a value the tree itself produced. The
/// bar is applied instead where an *untrusted* whole tree arrives —
/// [`LayerTree::validate`], which `crate::History::replay` runs as its
/// closing step — and, on the `.aur` file-read path, by `aurora-io`'s
/// own `tile_grid`/`validate_persisted_rects`. See [`validate_origin`] for
/// exactly which routes into the tree that does and does not cover:
/// [`LayerTree`]'s bare `Deserialize` is deliberately not one of them.
///
/// Deterministic despite `HashMap`'s arbitrary iteration order: when
/// several layers are out of range, the one with the lowest raw
/// [`LayerId`] is the one reported — the same discipline
/// [`validate_shape`] states for its own orphan report. Within one
/// entry, the layer's own `bounds` is reported ahead of its mask's.
fn validate_origins(layers: &HashMap<LayerId, LayerEntry>) -> Result<(), DocError> {
    // `aurora_core::Id<T>` deliberately has no `Ord`, so the minimum is
    // taken over the raw values rather than over the ids themselves --
    // see `validate_shape`'s own note.
    match layers
        .iter()
        .filter_map(|(id, entry)| offending_origin(entry).map(|bounds| (id.to_raw(), bounds)))
        .min_by_key(|(raw, _)| *raw)
    {
        // Routed back through the shared predicate rather than building
        // the error here, so there stays exactly one place that maps an
        // out-of-range `Rect` to a `DocError`.
        Some((_, bounds)) => validate_origin(bounds),
        None => Ok(()),
    }
}

/// The first of `entry`'s stored rectangles whose origin is out of
/// document range, if either is. Split out of [`validate_origins`] for
/// the same reason [`offending_opacity`] is split out of
/// [`validate_opacities`]: the "is this entry an offender" test and the
/// "which rectangle gets reported" answer must not drift apart across
/// the two passes the deterministic selection needs.
fn offending_origin(entry: &LayerEntry) -> Option<Rect> {
    let own = match &entry.kind {
        LayerKind::Pixel { bounds } if !bounds.origin_in_document_range() => Some(*bounds),
        _ => None,
    };
    own.or_else(|| {
        entry
            .mask
            .as_ref()
            .map(|mask| mask.bounds)
            .filter(|bounds| !bounds.origin_in_document_range())
    })
}

/// Refuses a `bounds` whose origin sits further from the document
/// origin than `aurora_core::MAX_DOCUMENT_ORIGIN` — the shared
/// predicate is `Rect::origin_in_document_range`; this is only the
/// mapping from its `false` to this crate's own error.
///
/// Called from every public path that stores a caller-supplied `Rect`
/// (`insert_unchecked` for a pixel layer, `set_bounds`, `add_mask`),
/// always *before* the value is written, so a refusal changes nothing.
/// [`crate::History::record_bounds_change`] calls it too — it stores no
/// `Rect` in the tree, but journals one as an undo entry, and an
/// out-of-range value there used to wedge undo permanently.
///
/// Deliberately not called from `restore`/`restore_mask`: those are
/// crate-private undo paths putting back a value that reached the tree
/// through one of the checked routes above, and re-checking there would
/// let an undo fail on a value the tree itself produced.
///
/// **What "already validated on the way in" does and does not mean.**
/// It covers the *live-edit API* (`set_bounds`, `add_mask`,
/// `insert_unchecked`) and the *`.aur` file-read path* (`aurora-io`'s
/// own `tile_grid` and `validate_persisted_rects`, which is where the
/// read-time origin bar lives — see
/// [`DocError::LayerOriginOutOfRange`] for why it is there and not in
/// this crate's deserializer). It does **not** cover [`LayerTree`]'s
/// own bare `Deserialize`: that is `#[serde(try_from =
/// "LayerTreeRepr")]`, and [`LayerTreeRepr`]'s `try_from` deliberately
/// runs [`validate_shape`], [`validate_id_allocator`] and
/// [`validate_opacities`] but *not* [`validate_origins`]. So a direct
/// `postcard::from_bytes::<LayerTree>()` that bypasses `aurora-io` can
/// construct a live tree holding an out-of-range origin, which
/// `restore`/`restore_mask` (and so `undo`/`redo`) will then put back
/// unchanged. That is a real residual, narrower than `restore` alone
/// suggests: the prior round's audit (PLAN.md, "third door") found the
/// only such call sites today are inside `aurora-app`'s own
/// `#[cfg(test)]` module, so it is not reachable in production, and
/// [`LayerTree::validate`] — which [`validate_origins`] backs — is the
/// whole-tree bar any untrusted tree is held to.
///
/// Extent is deliberately *not* checked here — see
/// [`DocError::LayerOriginOutOfRange`] and
/// `aurora_core::MAX_DOCUMENT_ORIGIN` for why. It is bounded where it
/// is owned instead (`aurora_core::Size::new`, and `aurora-io`'s own
/// `tile_grid` for a manifest read off disk).
/// Refuses a *mask* rectangle whose extent is past the document ceiling
/// ([`aurora_core::MAX_DOCUMENT_EXTENT`]).
///
/// Deliberately narrower than [`validate_origin`], which every
/// rectangle-taking entry point runs: this is only called from
/// [`LayerTree::add_mask`], and [`DocError::LayerBoundsTooLarge`]'s own
/// doc comment records why a layer's own bounds are *not* held to the
/// same bar here. It is not called from `restore_mask` either, for the
/// reason that one already documents: it puts back a rectangle that
/// reached the tree through a checked route, and re-checking would let
/// an undo fail on a value the tree itself produced.
fn validate_mask_extent(bounds: Rect) -> Result<(), DocError> {
    if bounds.width <= aurora_core::MAX_DOCUMENT_EXTENT
        && bounds.height <= aurora_core::MAX_DOCUMENT_EXTENT
    {
        return Ok(());
    }
    Err(DocError::LayerBoundsTooLarge {
        width: bounds.width,
        height: bounds.height,
        max: aurora_core::MAX_DOCUMENT_EXTENT,
    })
}

pub(crate) fn validate_origin(bounds: Rect) -> Result<(), DocError> {
    if bounds.origin_in_document_range() {
        return Ok(());
    }
    Err(DocError::LayerOriginOutOfRange {
        x: bounds.x,
        y: bounds.y,
        max: aurora_core::MAX_DOCUMENT_ORIGIN,
    })
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
    /// pixel layer. Returns [`DocError::LayerTreeTooDeep`] if the new
    /// layer would land deeper than [`MAX_LAYER_TREE_DEPTH`] — a
    /// document nested past that bound cannot be saved at all (see that
    /// constant), so the nest is refused rather than allowed and then
    /// discovered at save time. Returns
    /// [`DocError::LayerOriginOutOfRange`] if `bounds`' own origin sits
    /// further than [`aurora_core::MAX_DOCUMENT_ORIGIN`] from the
    /// document origin on either axis — a negative origin is still
    /// legal (a layer may sit off the canvas edge); see
    /// [`Self::set_bounds`], which enforces the same bound for a later
    /// move. Also returns [`DocError::LayerIdCollision`] if the id
    /// generated for the new layer is somehow already in use — see that
    /// variant for why it is returned rather than asserted away.
    ///
    /// Nothing is added when any of these happens. No id is consumed
    /// either — with one exception that cannot be otherwise:
    /// [`DocError::LayerIdCollision`] is discovered *by* generating the
    /// id, so the counter has already moved by the time the collision is
    /// seen, and is deliberately not rewound. A generator that never
    /// reissues an id is the property `validate_id_allocator` leans
    /// on; rewinding it to keep this sentence unqualified would trade a
    /// real invariant for a tidier doc comment.
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
    /// Same as [`Self::add_pixel_layer`], except
    /// [`DocError::LayerOriginOutOfRange`], which this cannot return: a
    /// group has no `bounds` of its own to be out of range (see
    /// [`LayerKind::Group`], and [`Self::set_bounds`], which refuses a
    /// group for the same reason).
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

    /// Whether `parent` names something a new child may hang under:
    /// `None` (a new root) always may; `Some(id)` must exist and must be
    /// a group.
    ///
    /// One implementation, called from both [`Self::insert`] and
    /// [`Self::insert_unchecked`], rather than the two hand-written
    /// copies those two used to carry. Both callers read the same
    /// unmutated state, so the copies could not disagree today — but
    /// keeping them in step was a standing obligation with nothing
    /// enforcing it, which is the shape of drift this module argues
    /// against elsewhere.
    ///
    /// # Errors
    ///
    /// [`DocError::UnknownLayer`] if `parent` is `Some` and names
    /// nothing; [`DocError::NotAGroup`] if it names a pixel layer.
    fn validate_parent(&self, parent: Option<LayerId>) -> Result<(), DocError> {
        let Some(parent_id) = parent else {
            return Ok(());
        };
        match self.layers.get(&parent_id) {
            None => Err(DocError::UnknownLayer(parent_id)),
            Some(entry) if !entry.kind.is_group() => Err(DocError::NotAGroup(parent_id)),
            Some(_) => Ok(()),
        }
    }

    /// [`Self::add_pixel_layer`]/[`Self::add_group`]'s shared body: the
    /// depth guard, then [`Self::insert_unchecked`] for the insert
    /// itself.
    ///
    /// The guard lives here rather than inside `insert_unchecked` so
    /// that the two halves are separable -- `insert_unchecked` is what
    /// the `test-support` escape hatch reuses to build the deliberately
    /// over-deep tree `aurora-app`'s own `resolve_tile` bound is
    /// defence against.
    fn insert(
        &mut self,
        name: String,
        parent: Option<LayerId>,
        kind: LayerKind,
    ) -> Result<LayerId, DocError> {
        // Validate `parent` before touching `self.layers` at all, so a
        // failed call adds nothing -- same "all or nothing" discipline
        // `aurora_graph::RenderGraph::add_node` uses for its own inputs.
        //
        // Ordered *before* the depth check on purpose: a caller naming a
        // parent that does not exist, or that is not a group, should
        // hear about that rather than about a depth computed from an
        // entry this method could not even find.
        self.validate_parent(parent)?;

        // A root sits at depth 1, so a child of `parent` sits one below
        // whatever depth `parent` itself does -- the same seeding
        // `restore` uses for a spliced subtree, and the same `1` that
        // `validate` passes `validate_shape` for a root. Checked before
        // `next_id`, so a refused insert does not even consume an id:
        // an id burnt here would still be invisible in `layers` but
        // would move the generator, and "a failed call changes nothing"
        // is easier to keep true than to keep qualified.
        let depth = parent.map_or(1, |id| self.depth_of(id).saturating_add(1));
        if depth > MAX_LAYER_TREE_DEPTH {
            return Err(DocError::LayerTreeTooDeep {
                depth,
                max: MAX_LAYER_TREE_DEPTH,
            });
        }

        self.insert_unchecked(name, parent, kind)
    }

    /// [`Self::insert`] without the depth guard -- every other guard
    /// (parent exists, parent is a group, the generated id is unused)
    /// still applies.
    ///
    /// Split out only so [`Self::insert`] and the `test-support`
    /// `insert_pixel_ignoring_the_depth_limit` can share one body;
    /// nothing else calls it, and nothing else should. Deliberately
    /// *not* an intra-doc link: the item it names only exists when the
    /// `test-support` feature is on, so a link here is an unresolved
    /// one without `--all-features` — measured, and it does fail
    /// `RUSTDOCFLAGS=-D warnings cargo doc -p aurora-doc --no-deps
    /// --document-private-items`. A plain `cargo doc --no-deps` does
    /// not notice, only because this method is private and its docs go
    /// unrendered; that is not a guarantee worth depending on.
    fn insert_unchecked(
        &mut self,
        name: String,
        parent: Option<LayerId>,
        kind: LayerKind,
    ) -> Result<LayerId, DocError> {
        // The same "all or nothing" parent validation `insert` already
        // ran. Re-run rather than assumed, because this is also the
        // entry point the `test-support` hatch uses directly -- and it
        // is the *same* call, not a second hand-written copy that would
        // have to be kept in step with the first.
        self.validate_parent(parent)?;

        // The single call site that covers every insert: `insert` (and
        // so `add_pixel_layer`, and so `History::add_pixel_layer`) and
        // the `test-support` `insert_pixel_ignoring_the_depth_limit`
        // both funnel through here. A group carries no bounds of its
        // own, so there is nothing to check on that arm.
        //
        // Before `next_id`, for the same reason `insert`'s own depth
        // check is: a refused insert should not even consume an id. An
        // id burnt here would be invisible in `layers` but would still
        // move the generator, and "a failed call changes nothing" is
        // easier to keep true than to keep qualified.
        if let LayerKind::Pixel { bounds } = &kind {
            validate_origin(*bounds)?;
        }

        let id = self.ids.next_id();
        // The displaced value is checked, not discarded: a plain
        // `HashMap::insert` here would *silently destroy* whatever layer
        // was already under `id`, orphan everything it contained, and
        // then splice the replacement into a sibling list under the same
        // id -- which, for a group, is a group listing itself as its own
        // child. That is a cycle constructed after deserialization, so
        // the one-shot `validate_shape` never sees it and every downward
        // walk recurses forever. `validate_id_allocator` refuses the
        // crafted file that makes this reachable; this refuses the
        // insert itself, whatever the generator's provenance. Nothing
        // has been mutated but the counter (monotonic, so harmless), and
        // the displaced entry goes straight back.
        if let Some(previous) = self.layers.insert(id, LayerEntry::new(name, parent, kind)) {
            self.layers.insert(id, previous);
            return Err(DocError::LayerIdCollision(id));
        }

        // Left as an assertion rather than a `?`, unlike the ones in
        // `remove_capturing`/`reparent`: `parent` here is a caller's own
        // argument, checked twelve lines above with no intervening
        // mutation of `self.layers` that could invalidate it, and no
        // deserialized `parent` field is involved at all. See
        // `reparent`'s own comment for the same distinction spelled out.
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

    /// [`Self::add_pixel_layer`] with the [`MAX_LAYER_TREE_DEPTH`]
    /// guard skipped — a **test-only** escape hatch, behind this crate's
    /// `test-support` feature.
    ///
    /// Precisely what "behind a feature" buys, since the isolation is
    /// the whole justification for this existing at all: this method is
    /// absent from `cargo build --workspace` and from
    /// `cargo doc --workspace --no-deps` with no extra flags. It is
    /// *present* whenever `--all-features` is passed (which this
    /// project's own clippy and rustdoc gate commands both do) and
    /// whenever dev-targets are built (`cargo test`, `cargo nextest`,
    /// `--all-targets`), because `aurora-app`'s `[dev-dependencies]`
    /// turns the feature on. The guarantee is "not in a shipped build",
    /// not "not in any build".
    ///
    /// It exists for exactly one caller: `aurora-app`'s own
    /// `composite_document_drops_the_branch_that_nests_one_level_past_the_maximum_tree_depth`,
    /// whose whole subject is a tree nested one level *past* the bound.
    /// That fixture used to be built with ordinary `add_group` calls,
    /// because nothing stopped it; now that the producers refuse, the
    /// only honest way to keep testing `resolve_tile`'s own independent
    /// depth guard against a genuinely over-deep tree is to build one
    /// deliberately. Removing the fixture instead would delete the test
    /// that proves the guard works.
    ///
    /// Every other guard still applies: `parent` must exist, must be a
    /// group, the generated id must be unused, and `bounds`' own origin
    /// must be within [`aurora_core::MAX_DOCUMENT_ORIGIN`] — that last
    /// check lives in `insert_unchecked`, the shared body, precisely so
    /// this hatch cannot become a second way around it. Only the depth
    /// check is skipped.
    ///
    /// It builds a [`LayerKind::Pixel`] and nothing else, on purpose.
    /// An earlier draft took an arbitrary [`LayerKind`], which let it
    /// construct far more than an over-deep tree — a
    /// `LayerKind::Group { children }` naming ids that already live
    /// elsewhere rebuilds exactly the two-parent and cycle shapes three
    /// rounds of hardening exist to prevent, and made the sentence
    /// above ("only the depth check is skipped") false. Its one caller
    /// only ever passed `LayerKind::Pixel`, so the narrower signature
    /// costs nothing and makes that sentence true.
    ///
    /// A tree built with this **cannot be saved**: `.aur`'s write path
    /// verifies by reopening, and the read side's own `validate_shape`
    /// refuses anything past [`MAX_LAYER_TREE_DEPTH`]. That is the whole
    /// reason the guard exists, and the reason this is not a `pub`
    /// production API.
    ///
    /// # Errors
    ///
    /// [`DocError::UnknownLayer`], [`DocError::NotAGroup`],
    /// [`DocError::LayerIdCollision`] and
    /// [`DocError::LayerOriginOutOfRange`], exactly as
    /// [`Self::add_pixel_layer`] returns them — but never
    /// [`DocError::LayerTreeTooDeep`].
    #[cfg(feature = "test-support")]
    pub fn insert_pixel_ignoring_the_depth_limit(
        &mut self,
        name: impl Into<String>,
        bounds: Rect,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        self.insert_unchecked(name.into(), parent, LayerKind::Pixel { bounds })
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
    ///
    /// Returns [`DocError::UnknownLayer`]/[`DocError::NotAGroup`] naming
    /// `id`'s *recorded parent*, or
    /// [`DocError::InconsistentLayerParent`], if `id`'s recorded `parent`
    /// disagrees with where `id` actually sits — a tree that reached this
    /// type through [`validate_shape`] (every deserialized one, and every
    /// [`Self::restore`]d subtree) cannot be in that state, and neither
    /// can one built through this type's own API, so these exist to make
    /// the abort impossible by construction rather than merely
    /// unreachable by argument. Nothing is removed when they happen.
    pub(crate) fn remove_capturing(&mut self, id: LayerId) -> Result<RemovedSubtree, DocError> {
        let parent = self
            .layers
            .get(&id)
            .ok_or(DocError::UnknownLayer(id))?
            .parent;

        let siblings = self.sibling_list_mut(parent)?;
        let Some(index) = siblings.iter().position(|&sibling| sibling == id) else {
            return Err(DocError::InconsistentLayerParent(id));
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
    ///
    /// Iterative on an explicit stack rather than recursive, and it
    /// *skips* an id that names nothing instead of treating that as
    /// unreachable: a `LayerTree` deserialized from an untrusted
    /// `.aur` file's own history journal (`RemovedSubtree` carries whole
    /// `LayerEntry` values, and `crate::History::undo` replays them back
    /// into the live tree) can name the same child twice, and a walk
    /// that panicked or recursed on that would turn a malformed file
    /// into a crash. The visit order is unchanged: root first, then each
    /// child's own subtree in stored order.
    fn capture_subtree(&mut self, id: LayerId, out: &mut Vec<(LayerId, LayerEntry)>) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let Some(entry) = self.layers.remove(&id) else {
                continue;
            };
            if let LayerKind::Group { children } = &entry.kind {
                stack.extend(children.iter().rev().copied());
            }
            out.push((id, entry));
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
    ///
    /// Returns [`DocError::MalformedRemovedSubtree`] if `removed` is not
    /// internally coherent before its shape is even walked — the same id
    /// carried twice, an incoming id that the live tree already holds,
    /// the declared `root` missing from `entries`, or an incoming
    /// group's `children` naming a layer that is already live *outside*
    /// the incoming set, or a *live* group's `children` already naming an
    /// incoming id (either way, that layer would be reachable from two
    /// parents the moment the two are merged). Beyond that, the
    /// incoming subtree is held to exactly the bar a deserialized
    /// manifest is: [`validate_shape`], rooted at `root` and hanging
    /// under `parent`, so [`DocError::MalformedLayerTree`],
    /// [`DocError::LayerTreeTooDeep`],
    /// [`DocError::InconsistentLayerParent`],
    /// [`DocError::OrphanedLayer`] and
    /// [`DocError::DanglingLayerReference`] are all reachable too, as is
    /// [`DocError::OpacityOutOfRange`] for an incoming entry whose
    /// opacity or fill opacity is outside `0.0..=1.0`. None of that is
    /// reachable from a `RemovedSubtree` this type itself produced; it is
    /// reachable from the one inside an untrusted `.aur` file's history
    /// journal, which [`crate::History::replay`] feeds straight back
    /// through here.
    ///
    /// On success the tree's own id generator is advanced past every id
    /// restored, so a subtree carrying ids this tree never allocated
    /// (the [`crate::History::replay`] case, which starts from
    /// [`Self::new`] with the counter at `0`) cannot make the next
    /// `add_pixel_layer` alias one of them.
    ///
    /// **A layer origin is deliberately *not* re-checked here** — this
    /// puts back a rectangle that reached the tree through a checked
    /// route, and re-checking would let an ordinary undo fail on a value
    /// the tree itself produced. "A checked route" means the live-edit
    /// API or the `.aur` file-read path, *not* this type's own bare
    /// `Deserialize`, which does not bound origin; [`validate_origin`]'s
    /// doc comment states exactly what that leaves open (in short: a
    /// direct `postcard::from_bytes::<LayerTree>()` bypassing
    /// `aurora-io`, whose only call sites today are `aurora-app` tests).
    /// The whole-tree bar for an untrusted tree is [`Self::validate`].
    ///
    /// Nothing is changed when any of these happens — every check runs
    /// before the first mutation.
    pub(crate) fn restore(&mut self, removed: RemovedSubtree) -> Result<LayerId, DocError> {
        let RemovedSubtree {
            root,
            parent,
            index,
            entries,
        } = removed;

        // Validate before mutating anything -- same "all or nothing"
        // discipline `insert` uses for its own `parent` argument. This
        // one first, because it is the only check about the *live* tree
        // (and it also catches a `parent` naming an id that only exists
        // inside the incoming subtree, which is not in `self.layers`
        // yet).
        if let Some(parent_id) = parent {
            match self.layers.get(&parent_id) {
                None => return Err(DocError::UnknownLayer(parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(parent_id));
                }
                Some(_) => {}
            }
        }

        // Move the flat capture into the map shape `validate_shape`
        // works on -- cheap, since `removed` is consumed by value
        // anyway -- rejecting the three ways it can be incoherent before
        // its shape is even walked.
        let mut incoming: HashMap<LayerId, LayerEntry> = HashMap::with_capacity(entries.len());
        for (id, entry) in entries {
            if self.layers.contains_key(&id) {
                return Err(DocError::MalformedRemovedSubtree(id));
            }
            if incoming.insert(id, entry).is_some() {
                return Err(DocError::MalformedRemovedSubtree(id));
            }
        }
        if !incoming.contains_key(&root) {
            return Err(DocError::MalformedRemovedSubtree(root));
        }

        // Neither half's own shape walk can see a `children` reference
        // that points across the two maps, and the merge is what makes
        // such a reference resolve into a layer with two parents. Both
        // directions, one implementation -- see
        // `validate_cross_references` for why the second is not
        // redundant.
        validate_cross_references(&incoming, &self.layers)?;
        validate_cross_references(&self.layers, &incoming)?;

        // The subtree hangs under a live `parent`, so its depth budget
        // continues that parent's rather than restarting -- otherwise a
        // crafted journal could stack two individually-legal `Restore`
        // ops into a tree twice past `MAX_LAYER_TREE_DEPTH`.
        let start_depth = parent.map_or(1, |id| self.depth_of(id).saturating_add(1));
        validate_shape(&incoming, &[root], parent, start_depth)?;
        validate_opacities(&incoming)?;

        // Every id in `incoming` is about to become live, and the ids in
        // a captured subtree are whatever they originally were -- which,
        // when the tree being restored into was built from scratch (as
        // `crate::History::replay` builds it), can sit *ahead* of this
        // tree's own counter. Moving the counter past them keeps the
        // "a freshly generated id is unused" invariant true, so the very
        // next `add_pixel_layer` cannot alias a layer just restored.
        if let Some(highest) = incoming.keys().map(|id| id.to_raw()).max() {
            self.ids.advance_past(highest);
        }

        self.layers.extend(incoming);

        let siblings = self.sibling_list_mut(parent)?;
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
    ///
    /// Returns [`DocError::LayerTreeTooDeep`] if the move would push any
    /// part of the moved subtree past [`MAX_LAYER_TREE_DEPTH`] — a
    /// document nested past that bound cannot be saved at all (see that
    /// constant), so the move is refused rather than allowed and then
    /// discovered at save time. The reported `depth` is where the moved
    /// subtree's *deepest descendant* would land, not where `id` itself
    /// would: moving a three-level group under a parent at depth 255
    /// is refused even though `id` alone would fit.
    ///
    /// That downward walk is skipped entirely when `new_parent` is no
    /// deeper than `id`'s current parent — every same-level reorder and
    /// every move *toward* the root, which is most of what a
    /// drag-and-drop reorder actually does. Such a move lands every node
    /// of the moved subtree at a depth no greater than the one it
    /// already had, so it cannot be the thing that breaks the bound.
    /// This is a cost change, not a contract change, for any tree this
    /// type's own API can build — measured independently three times
    /// (build, revision, and re-verification passes), each on a legal
    /// 40,000-layer group: 5.8-8.8 ms for the full walk before this
    /// change, 250 ns-3 µs after; the multi-order-of-magnitude
    /// improvement held every time, the exact figure did not, so it's
    /// reported as a range rather than a single falsely-precise number.
    /// Its one visible edge is
    /// on a tree that is *already* malformed or already over-deep —
    /// constructible only through the `test-support` hatch or a
    /// hand-built struct literal — where a non-deepening move is now
    /// performed rather than refused. That is the better answer anyway:
    /// moving such a subtree shallower is what un-nesting looks like,
    /// and refusing it traps the state instead of letting the caller
    /// out of it.
    ///
    /// Returns [`DocError::MalformedLayerTree`] if that downward walk
    /// reaches the same layer twice — a group inside itself, or one
    /// layer listed as a child of two groups within the moved subtree.
    /// Not reachable from any deserialized or API-built tree (both are
    /// held to the deserialize-time validator's "each layer reached at
    /// most once" rule); returned rather than asserted for the same reason the
    /// `InconsistentLayerParent` case below is.
    ///
    /// Also returns [`DocError::UnknownLayer`]/[`DocError::NotAGroup`]
    /// naming `id`'s *current* parent, or
    /// [`DocError::InconsistentLayerParent`] naming `id` itself, if the
    /// `parent` `id` records names something that is gone, is not a
    /// group, or is a group that does not actually list `id`. That
    /// cannot happen on a
    /// tree built through this type's own API, nor on one deserialized
    /// from bytes (whose shape — including every entry's own recorded
    /// `parent` — is validated up front; see
    /// [`DocError::InconsistentLayerParent`]). It is returned rather
    /// than asserted anyway, so that a future caller holding a tree
    /// assembled some third way gets an error instead of a process
    /// abort.
    ///
    /// Nothing is changed when any of these happens.
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

        // Both halves of the depth question, now that the destination is
        // known good: how deep `new_parent` sits, plus how tall the
        // moved subtree is. The `- 1` is load-bearing -- `id` itself
        // occupies `new_depth`, not one below it, so a single leaf
        // (height 1) moving under a parent at depth 255 lands at 256 and
        // is legal. Placed before the first mutation below, so a refused
        // move leaves both sibling lists and `id`'s own `parent` field
        // exactly as they were.
        let new_depth = new_parent.map_or(1, |p| self.depth_of(p).saturating_add(1));
        // ...but only when the move could deepen anything. Every node of
        // the moved subtree sits at `depth_of(id) + its level - 1` now
        // and would sit at `new_depth + its level - 1` after, so
        // `new_depth <= depth_of(id)` means every one of them lands no
        // deeper than it already is and the bound cannot newly break.
        // `depth_of` is an upward walk of one chain; `subtree_height` is
        // a downward walk of the whole moved subtree, so skipping it
        // turns the common drag-and-drop cases (reorder among siblings,
        // drag out to a shallower group) from O(moved subtree) into
        // O(depth). See this method's own doc comment for the one edge
        // this changes.
        if new_depth > self.depth_of(id) {
            let deepest = new_depth
                .saturating_add(self.subtree_height(id)?)
                .saturating_sub(1);
            if deepest > MAX_LAYER_TREE_DEPTH {
                return Err(DocError::LayerTreeTooDeep {
                    depth: deepest,
                    max: MAX_LAYER_TREE_DEPTH,
                });
            }
        }

        // Everything about the *destination* is validated. The source is
        // not: `old_parent` is whatever `id`'s own entry records, and on
        // a tree that never went through `validate_shape` that can name
        // a layer that is gone, or a pixel layer. Returning the error
        // rather than asserting it away is what keeps a crafted `.aur`
        // file from aborting the process here (`panic = "abort"`); note
        // it returns *before* the first mutation, so a refused reparent
        // still changes nothing.
        let old_siblings = self.sibling_list_mut(old_parent)?;
        // Mirrors `remove_capturing`: find the position first, and treat
        // "not there" as the invariant violation it is. A `retain` here
        // was a silent no-op in exactly that case, and the move then
        // went ahead anyway -- leaving `id` still listed in its old
        // parent's `children` *and* listed under the new one, i.e. the
        // very "same layer under two parents" shape `validate_shape`
        // exists to forbid, manufactured through this method's own
        // public API.
        let Some(old_index) = old_siblings.iter().position(|&sibling| sibling == id) else {
            return Err(DocError::InconsistentLayerParent(id));
        };
        old_siblings.remove(old_index);

        // ...then attach at the new one.
        //
        // These last two `unreachable!`s stay, deliberately, where the
        // `old_parent` one above became a real `?`. The asymmetry is the
        // point: `old_parent` is *recorded data* that untrusted bytes
        // can lie about, while `new_parent` and `id` were each read and
        // checked a few lines above with no intervening mutation that
        // could invalidate them (the removal just above drops one id
        // from `old_siblings` via `Vec::remove`; it removes no `layers`
        // entry and changes no `LayerKind`). They are also
        // now past the first mutation, so turning them into `?` would
        // trade an impossible abort for a reachable half-applied move --
        // `id` detached from its old parent and attached to nothing.
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
        // A plain loop, bounded by the tree's own layer count. A tree
        // built through this type's own API always terminates at a root,
        // but one restored from an untrusted `.aur` file's history
        // journal can have a parent chain that loops -- and unbounded
        // recursion on that is a process abort, not an error. Running out
        // of budget means the chain cycles, so answer "yes, a
        // descendant": that is the direction that makes `reparent`
        // *refuse* the move rather than perform one on a broken tree.
        let mut current = descendant;
        for _ in 0..self.layers.len() {
            let Some(entry) = self.layers.get(&current) else {
                return false;
            };
            match entry.parent {
                Some(parent) if parent == ancestor => return true,
                Some(parent) => current = parent,
                None => return false,
            }
        }
        true
    }

    /// How deep `id` sits: `1` for a root, one more per enclosing group.
    /// [`Self::restore`]'s starting depth, so a spliced subtree's own
    /// depth budget continues the live tree's rather than restarting.
    ///
    /// Walks upward, bounded by the tree's own layer count for the same
    /// reason [`Self::is_descendant`] is: a parent chain restored from an
    /// untrusted journal can loop, and unbounded walking on that is a
    /// hang rather than an error. Running out of budget means the chain
    /// cycles, so it reports the largest depth it counted — the
    /// direction that makes the caller *refuse* rather than accept.
    fn depth_of(&self, id: LayerId) -> usize {
        let mut depth: usize = 1;
        let mut current = id;
        for _ in 0..self.layers.len() {
            match self.layers.get(&current).and_then(|entry| entry.parent) {
                Some(parent) => {
                    depth = depth.saturating_add(1);
                    current = parent;
                }
                None => return depth,
            }
        }
        depth
    }

    /// How many levels `id`'s own subtree occupies, counting `id` itself
    /// as `1` — [`Self::reparent`]'s depth guard for the moved half.
    /// A leaf is `1`; a group holding one leaf is `2`.
    ///
    /// [`Self::depth_of`] answers the other half (how deep the
    /// *destination* sits); the two together say where the moved
    /// subtree's deepest node would land.
    ///
    /// Iterative, with an explicit stack, for the same reason
    /// [`validate_shape`] and [`Self::capture_subtree`] are: a downward
    /// walk here can be handed a tree restored from an untrusted
    /// journal, and recursion on a deep or cyclic one is a process
    /// abort under `panic = "abort"`, not a catchable error. A `visited`
    /// set expands each id at most once, so the total work is bounded by
    /// `layers.len()` even on a tree where the same child is listed
    /// under two parents (which would otherwise fan out exponentially).
    ///
    /// Two conventions borrowed from this file's neighbours. A child id
    /// that names nothing is skipped rather than asserted away, exactly
    /// as [`Self::capture_subtree`] and [`validate_shape`]'s own walk
    /// loop do. And reaching the same id twice is
    /// [`DocError::MalformedLayerTree`] — the same variant
    /// [`validate_shape`] returns for the same shape, so
    /// [`Self::reparent`] refuses with the *reason* rather than
    /// reporting a malformed tree as merely "very deep", which is what
    /// an earlier draft's saturating `MAX_LAYER_TREE_DEPTH + 1` did.
    /// Such a tree is malformed and no move on it is safe to perform.
    ///
    /// Precisely what that catches, since the ideal is stronger than the
    /// code: `visited` is per call and covers only the subtree this walk
    /// actually descends, so it sees a cycle within that subtree, and a
    /// child listed twice within it — but *not* a layer shared between
    /// two sibling subtrees neither of which is being walked. That
    /// broader rule is [`validate_shape`]'s, enforced once at
    /// deserialization time over the whole tree; this is the local
    /// check that keeps *this* walk finite.
    ///
    /// The `Ok` answer saturates at `MAX_LAYER_TREE_DEPTH + 1` rather
    /// than counting a subtree already taller than the bound exactly.
    /// Only the refusal is load-bearing there, and the walk stops as
    /// soon as the outcome is settled.
    ///
    /// # Errors
    ///
    /// [`DocError::MalformedLayerTree`], naming the layer reached twice.
    fn subtree_height(&self, id: LayerId) -> Result<usize, DocError> {
        let refuse = MAX_LAYER_TREE_DEPTH.saturating_add(1);
        let mut visited: HashSet<LayerId> = HashSet::new();
        let mut stack: Vec<(LayerId, usize)> = vec![(id, 1)];
        let mut height: usize = 1;

        while let Some((current, level)) = stack.pop() {
            if !visited.insert(current) {
                return Err(DocError::MalformedLayerTree(current));
            }
            height = height.max(level);
            // Already past the bound: no deeper level can change what
            // the caller does with the answer, so stop walking.
            if height > MAX_LAYER_TREE_DEPTH {
                return Ok(refuse);
            }
            let Some(entry) = self.layers.get(&current) else {
                continue;
            };
            let LayerKind::Group { children } = &entry.kind else {
                continue;
            };
            for &child in children {
                stack.push((child, level.saturating_add(1)));
            }
        }

        Ok(height)
    }

    /// Holds this whole tree to exactly the bar
    /// `#[serde(try_from = "LayerTreeRepr")]` holds a deserialized one
    /// to — literally the same [`validate_shape`], [`validate_opacities`],
    /// [`validate_id_allocator`] and [`validate_layer_id_range`] calls,
    /// rooted at [`Self::roots`]
    /// with no parent above them, plus [`validate_origins`] for the
    /// stored `Rect` origins the per-call guards cover on the live edit
    /// paths.
    ///
    /// It exists because there is a second way an untrusted `.aur` file
    /// reaches a live `LayerTree` that never touches
    /// [`LayerTree::try_from`]: [`crate::History::load_journal`] followed
    /// by [`crate::History::replay`], which starts from
    /// [`LayerTree::new`] and applies the file's own recorded
    /// [`crate::History`] ops. Validating the manifest alone would have
    /// left that path open, so a journal is held to the same bar as a
    /// manifest.
    ///
    /// # Errors
    ///
    /// Whatever [`validate_shape`] returns — see its own doc comment for
    /// the four rules — plus [`DocError::StaleLayerIdGenerator`] from
    /// [`validate_id_allocator`], the same pairing
    /// `#[serde(try_from = "LayerTreeRepr")]` runs; plus
    /// [`DocError::ReservedLayerIdBit`] and
    /// [`DocError::ReservedLayerIdCounter`] from
    /// [`validate_layer_id_range`]; plus
    /// [`DocError::OpacityOutOfRange`] from [`validate_opacities`] and
    /// [`DocError::LayerOriginOutOfRange`] from [`validate_origins`].
    ///
    /// The last two close a gap between this function and the two other
    /// whole-tree gates. `try_from` already ran `validate_opacities`
    /// directly, and [`Self::restore`] already runs it on an incoming
    /// subtree; `set_bounds`/`add_mask`/`insert_unchecked` already run
    /// [`validate_origin`] on every caller-supplied `Rect`. Only
    /// `validate` itself stopped at shape and ids — which mattered
    /// because [`crate::History::replay`] has no other gate: a crafted
    /// journal's `Restore`/`RestoreMask` op could splice in an origin
    /// past `aurora_core::MAX_DOCUMENT_ORIGIN` and reach a live tree.
    /// Property ranges are now checked here too, so `replay` clears the
    /// same bar `try_from` and `restore` each already apply on their own
    /// paths. (The opacity call is contract completeness rather than a
    /// second closed exploit: both journal doors for an opacity —
    /// `SetOpacity`/`SetFillOpacity`, which go through the
    /// already-range-checked setters, and `Restore`, which goes through
    /// [`Self::restore`]'s own `validate_opacities` — were guarded
    /// before this.)
    pub(crate) fn validate(&self) -> Result<(), DocError> {
        // Shape, allocator, id range, opacity, origin -- `try_from`'s
        // own ordering, one step further.
        validate_shape(&self.layers, &self.roots, None, 1)?;
        validate_id_allocator(&self.ids, &self.layers)?;
        validate_layer_id_range(&self.ids, &self.layers)?;
        validate_opacities(&self.layers)?;
        validate_origins(&self.layers)
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
    ///
    /// # And for an id with [`crate::MASK_SURFACE_BIT`] set
    ///
    /// Also `None`, and this branch is the mirror image of the one
    /// [`Self::mask_surface_id`] already carries. A layer's pixel
    /// surface is the *bottom* half of the id space by construction;
    /// an id with the top bit set would put it in the half reserved for
    /// mask surfaces, where it aliases the mask storage of the layer
    /// whose id is this one with that bit cleared — one tile-store slot
    /// with two owners.
    ///
    /// `validate_layer_id_range` (this module's own, crate-private)
    /// refuses such an id at the deserialization boundary, so this
    /// guard should be unreachable.
    /// It is here anyway, deliberately, so the invariant is enforced at
    /// the type's own boundary rather than only at the one call site
    /// that validates: `LayerEntry`/`LayerId` are both `Deserialize`
    /// and a `LayerTree` can also be assembled by `restore`, and this
    /// crate would rather return "no surface" than hand a caller an id
    /// that addresses somebody else's pixels. Before 0.70.1 the
    /// validation half was missing entirely, and a crafted `.aur`
    /// manifest could reach exactly that collision — see
    /// [`DocError::ReservedLayerIdBit`].
    #[must_use]
    pub fn surface_id(&self, id: LayerId) -> Option<aurora_tile::SurfaceId> {
        if id.to_raw() >= crate::MASK_SURFACE_BIT {
            return None;
        }
        match self.kind(id)? {
            LayerKind::Pixel { .. } => Some(aurora_tile::SurfaceId::from_raw(id.to_raw())),
            LayerKind::Group { .. } => None,
        }
    }

    /// The [`aurora_tile::SurfaceId`] `id`'s own **mask** coverage is
    /// addressed under in the same shared `aurora_tile::TileStore` —
    /// `id`'s own raw value with [`crate::MASK_SURFACE_BIT`] set. Like
    /// [`Self::surface_id`], it is derived, not allocated: there is no
    /// stored field, no second id scheme, and nothing in the `.aur`
    /// format has to change to carry it.
    ///
    /// **Unlike [`Self::surface_id`], this returns `Some` for a
    /// [`LayerKind::Group`] too.** Photoshop masks groups, and
    /// `aurora-app`'s own compositor already masks a group's whole
    /// isolated buffer as one unit — a group has no pixels of its own,
    /// but it certainly can have a mask.
    ///
    /// # Why the bit partition cannot collide
    ///
    /// See [`crate::MASK_SURFACE_BIT`] for the full partition. In
    /// short: layer pixel surfaces are the bottom half, mask surfaces
    /// the top half, and `aurora-app`'s reserved composite surface is
    /// `u64::MAX` — which is why the guard below excludes
    /// `MASK_SURFACE_BIT - 1` as well as everything above it, since
    /// `(MASK_SURFACE_BIT - 1) | MASK_SURFACE_BIT == u64::MAX` is the
    /// one layer id that would otherwise land on the composite
    /// surface.
    ///
    /// # When this is `None`
    ///
    /// For an unknown `id`, and — structurally unreachably — for a real
    /// id at or above `MASK_SURFACE_BIT - 1`.
    /// `aurora_core::IdGenerator` starts at `0` and hands ids out one
    /// at a time, monotonically, so reaching `2^63 - 1` would take
    /// `2^63` layer creations in a single session; and an id
    /// deserialized from an untrusted file cannot get there either,
    /// because this module's own crate-private
    /// `validate_layer_id_range` refuses anything at or above
    /// `MASK_SURFACE_BIT` outright. The branch exists because this
    /// crate refuses to `panic` (see CLAUDE.md's "Lints worth
    /// knowing": a panic costs a professional their unsaved work), so
    /// an unreachable case still has to return something honest rather
    /// than assert.
    #[must_use]
    pub fn mask_surface_id(&self, id: LayerId) -> Option<aurora_tile::SurfaceId> {
        if !self.contains(id) || id.to_raw() >= crate::MASK_SURFACE_BIT - 1 {
            return None;
        }
        Some(aurora_tile::SurfaceId::from_raw(
            id.to_raw() | crate::MASK_SURFACE_BIT,
        ))
    }

    /// Every `aurora_tile::SurfaceId` this tree's layers currently
    /// address — each pixel layer's own content surface
    /// ([`Self::surface_id`]) and every layer's mask surface
    /// ([`Self::mask_surface_id`]), for every entry the tree holds.
    ///
    /// Crate-private on purpose: this module owns the two derivation
    /// rules, so the enumeration of their results belongs beside them
    /// rather than in a caller that would have to re-derive the guards.
    /// [`crate::forget_document_surfaces`] is the one consumer.
    ///
    /// Walks `layers` itself rather than recursing from
    /// [`Self::roots`], because the point is exhaustiveness: an entry
    /// unreachable from any root still owns tiles in the store, and
    /// `LayerTreeRepr`'s own `validate_shape` only guarantees
    /// reachability for a tree that came in through deserialization.
    ///
    /// A mask surface is emitted whether or not the layer currently
    /// carries a [`crate::LayerMask`] — deliberately. `remove_mask`
    /// drops only the struct, leaving whatever coverage was painted
    /// under that derived surface behind (see [`crate::mask`]'s own
    /// lifecycle notes), so gating on `mask.is_some()` would leave
    /// exactly the residue this enumeration exists to find.
    pub(crate) fn all_surfaces(&self) -> Vec<aurora_tile::SurfaceId> {
        // Two per layer, not one: a pixel layer contributes both a
        // content and a mask surface below.
        let mut out = Vec::with_capacity(self.layers.len().saturating_mul(2));
        for id in self.layers.keys() {
            out.extend(self.surface_id(*id));
            out.extend(self.mask_surface_id(*id));
        }
        out
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
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist,
    /// [`DocError::NotAPixelLayer`] if it names a group, or
    /// [`DocError::LayerOriginOutOfRange`] if `bounds`' own origin sits
    /// further than [`aurora_core::MAX_DOCUMENT_ORIGIN`] from the
    /// document origin on either axis — a *negative* origin is still
    /// perfectly legal, since moving a layer off the canvas edge is
    /// what this method is for. The layer's existing `bounds` are left
    /// exactly as they were when any of these happens.
    ///
    /// The two id checks run first on purpose: naming a layer that does
    /// not exist, or one that is a group, should be reported as that
    /// rather than as a complaint about a rectangle destined for an
    /// entry this method could not use anyway.
    pub fn set_bounds(&mut self, id: LayerId, bounds: Rect) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        let LayerKind::Pixel { bounds: current } = &mut entry.kind else {
            return Err(DocError::NotAPixelLayer(id));
        };
        validate_origin(bounds)?;
        *current = bounds;
        Ok(())
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
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist,
    /// [`DocError::MaskAlreadyExists`] if it already has a mask,
    /// [`DocError::LayerOriginOutOfRange`] if `bounds`' own origin sits
    /// further than [`aurora_core::MAX_DOCUMENT_ORIGIN`] from the
    /// document origin — the same bound (and the same "negative is
    /// still legal") [`Self::set_bounds`] documents, applied to a
    /// mask's own rectangle — or [`DocError::LayerBoundsTooLarge`] if
    /// its *extent* is past [`aurora_core::MAX_DOCUMENT_EXTENT`].
    /// Nothing is changed when any of these happens; in particular a
    /// refused rectangle leaves the layer maskless rather than
    /// half-masked.
    ///
    /// **The extent check is here, and not only at the `.aur` file
    /// boundary** (0.71.3). Since 0.71.0 a mask's rectangle drives a
    /// real tile grid in `aurora-io`, and an oversized one is not a
    /// large loop there but an unfinishable one. `aurora-io` refuses it
    /// — but by then the tree already holds it, and that writer refuses
    /// the *whole document*, so one oversized mask silently disables
    /// every save and every crash-recovery autosave for the rest of the
    /// session. A rectangle no writer will accept has no business
    /// entering the tree, so it is refused where it is created.
    ///
    /// # This does not clear residual coverage
    ///
    /// A mask surface id is derived from the layer's own id
    /// ([`Self::mask_surface_id`]), so a mask added here lands on the
    /// same surface any *previous* mask on this layer painted into —
    /// and [`Self::remove_mask`] drops only the [`LayerMask`] struct.
    /// This function holds no `aurora_tile::TileStore` handle and so
    /// cannot do anything about that; keeping [`LayerTree`] free of the
    /// store is deliberate.
    ///
    /// **[`crate::History::add_mask`] is the caller that closes the
    /// gap** — it takes a store and calls
    /// [`crate::mask::forget_mask_coverage`] straight after this
    /// returns. A caller that bypasses [`crate::History`] and calls this
    /// directly must make that call itself, or the new mask inherits the
    /// old one's painted coverage, shifted by the offset between the two
    /// `bounds` origins. That is the same documented, accepted
    /// bypass shape [`crate::forget_document_surfaces`]'s own "a removal
    /// that bypassed `History` entirely" gap already records for
    /// [`Self::remove`].
    pub fn add_mask(&mut self, id: LayerId, bounds: Rect) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        if entry.mask.is_some() {
            return Err(DocError::MaskAlreadyExists(id));
        }
        validate_origin(bounds)?;
        validate_mask_extent(bounds)?;
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
    /// **The mask's origin is deliberately *not* re-checked here**, for
    /// the same reason [`Self::restore`] gives: it puts back a rectangle
    /// that reached the tree through a checked route (the live-edit API,
    /// via [`Self::add_mask`], or the `.aur` file-read path, via
    /// `aurora-io`'s `validate_persisted_rects`), and re-checking would let
    /// an ordinary undo fail on a value the tree itself produced. This
    /// type's own bare `Deserialize` is *not* one of those checked
    /// routes — see [`validate_origin`] for what that leaves open, and
    /// [`Self::validate`] for the whole-tree bar that covers it.
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
    /// **Scope, stated honestly — what this method itself does *not*
    /// do**: this flat list carries no group boundaries, so a group's
    /// own `opacity`/`blend_mode`/mask cannot be aggregated into its
    /// children's effective compositing from `paint_order`'s own return
    /// value alone — a real compositor cannot reconstruct "which run of
    /// entries came from the same group" after the fact. That real
    /// aggregation now exists for all three — `opacity`/`blend_mode` and,
    /// as of a later round, mask — but as its own separate recursive walk
    /// over [`Self::roots`]/[`Self::children`] (`aurora-app`'s
    /// `resolve_tile`, isolating each group's own visible direct
    /// children before applying the group's own opacity/blend mode one
    /// level up — the only semantic `aurora_doc::BlendMode` can express,
    /// since it has no "Pass Through" variant to model Photoshop's own
    /// isolated-vs-pass-through distinction with — and, on both a plain
    /// layer and a group's own isolated composite alike, masking by the
    /// layer's own [`LayerMask`] when it has one and
    /// [`LayerMask::enabled`] is true — real per-pixel grayscale
    /// coverage since 0.70.0, read from the mask's own surface
    /// ([`Self::mask_surface_id`], [`crate::mask`]) and multiplied with
    /// the [`LayerMask::bounds`] rectangle, so feathering and soft
    /// edges are expressible; it was a hard rectangular inside/outside
    /// test only before that), not a feature of this method. Callers that need
    /// real per-group compositing should walk the tree shape directly
    /// rather than call `paint_order`, which remains what it always was:
    /// a flat, group-blind paint list for callers (a Layers-panel row
    /// order, for instance) that only need "every visible pixel layer,
    /// correctly stacked" and no group-level aggregation at all. The same
    /// subtree-bounds/effective-visibility aggregation this crate's own
    /// private `layer_dirty_rect` helper (`history.rs`) still lacks for
    /// groups remains a separate, still-open gap.
    #[must_use]
    pub fn paint_order(&self) -> Vec<LayerId> {
        let mut out = Vec::new();
        // An explicit stack, not recursion. Popping visits siblings
        // last-to-first and dives into a group's own children before
        // continuing with the siblings beneath it -- the exact order the
        // recursive walk this replaced produced, and the order the
        // `paint_order_*` tests below pin down.
        //
        // `budget` bounds the walk at one visit per layer the tree
        // actually holds, which is all a real tree ever needs. It only
        // ever runs out on a malformed tree (a group nested inside
        // itself), which `LayerTree`'s own `Deserialize` already rejects
        // -- but `paint_order` returns a plain `Vec` with no way to
        // report an error, and this is a compositing path, so a
        // belt-and-braces bound here is what keeps "the tree is somehow
        // cyclic" a wrong picture rather than a stack overflow that
        // aborts the process.
        let mut stack: Vec<LayerId> = self.roots.clone();
        let mut budget = self.layers.len();
        while let Some(id) = stack.pop() {
            // Also covers an id naming nothing at all (`visible` is
            // `None` then), exactly as the recursive walk did.
            if self.visible(id) != Some(true) {
                continue;
            }
            if budget == 0 {
                break;
            }
            budget -= 1;
            match self.kind(id) {
                Some(LayerKind::Pixel { .. }) => out.push(id),
                Some(LayerKind::Group { children }) => stack.extend(children.iter().copied()),
                None => {}
            }
        }
        out
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
    use aurora_core::{Id, IdGenerator, Rect};
    use std::collections::HashMap;

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
    // A layer's mask coverage and its own pixels must never share a
    // surface -- writing one would clobber the other.
    fn mask_surface_id_differs_from_the_same_layers_own_pixel_surface() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(tree.mask_surface_id(id), tree.surface_id(id));
        assert_eq!(
            tree.mask_surface_id(id),
            Some(aurora_tile::SurfaceId::from_raw(
                id.to_raw() | crate::MASK_SURFACE_BIT
            ))
        );
    }

    #[test]
    fn mask_surface_id_is_distinct_for_two_different_layers() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(a, b);
        assert_ne!(tree.mask_surface_id(a), tree.mask_surface_id(b));
    }

    #[test]
    // The deliberate contrast with `surface_id`, which is `None` for a
    // group: a group has no pixels of its own, but Photoshop masks
    // groups and so does this compositor.
    fn mask_surface_id_is_some_for_a_group_where_surface_id_is_none() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.surface_id(group), None);
        assert!(tree.mask_surface_id(group).is_some());
    }

    #[test]
    fn mask_surface_id_is_none_for_an_unknown_id() {
        let tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        assert_eq!(tree.mask_surface_id(bogus), None);
    }

    #[test]
    // `aurora-app`'s reserved composite surface is
    // `SurfaceId::from_raw(u64::MAX)`; no mask surface may ever land on
    // it. `IdGenerator` allocates from 0 upward, so a handful of real
    // ids stands in for "every id a document could hold" -- and the one
    // id that *would* collide (`MASK_SURFACE_BIT - 1`) is refused
    // outright.
    fn mask_surface_id_never_collides_with_the_reserved_composite_surface() {
        let mut tree = LayerTree::new();
        let composite = aurora_tile::SurfaceId::from_raw(u64::MAX);
        for index in 0..64 {
            let id = match tree.add_pixel_layer(format!("layer {index}"), bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            assert_ne!(tree.mask_surface_id(id), Some(composite));
            assert_ne!(tree.surface_id(id), Some(composite));
        }
        // The single id whose masked form *would* be `u64::MAX`, and
        // the two above it -- each **actually present in the tree**, so
        // the arithmetic guard is what refuses them rather than
        // `contains`. (This test asserted the same thing against a tree
        // that did *not* contain those ids until 0.70.1, which made it
        // vacuous: `mask_surface_id` short-circuits on `!contains`
        // before it ever reaches the guard, so it would have stayed
        // green with the guard deleted outright.)
        //
        // Built by struct literal because no other path can produce
        // them: `IdGenerator` would need `2^63` allocations, and
        // `validate_layer_id_range` refuses exactly these ids on the
        // deserialization path. That is the point -- the guard is the
        // last line behind a validator, so the test has to reach past
        // the validator to exercise it.
        for raw in [
            crate::MASK_SURFACE_BIT - 1,
            crate::MASK_SURFACE_BIT,
            u64::MAX,
        ] {
            let colliding: super::LayerId = Id::from_raw(raw);
            let mut layers = HashMap::new();
            layers.insert(colliding, pixel_entry("boundary", None));
            let forced = LayerTree {
                ids: ids_for(&layers),
                layers,
                roots: vec![colliding],
            };
            assert!(forced.contains(colliding), "the guard, not `contains`");
            assert_eq!(
                forced.mask_surface_id(colliding),
                None,
                "layer id {raw} must not get a mask surface"
            );
        }

        // ... while an ordinary id one step below the boundary block
        // still gets one, so the guard is a boundary and not a blanket
        // refusal.
        let below: super::LayerId = Id::from_raw(crate::MASK_SURFACE_BIT - 2);
        let mut layers = HashMap::new();
        layers.insert(below, pixel_entry("below", None));
        let forced = LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![below],
        };
        assert_eq!(
            forced.mask_surface_id(below),
            Some(aurora_tile::SurfaceId::from_raw(u64::MAX - 1)),
        );
    }

    #[test]
    // The partition itself: every mask surface is in the top half of
    // the id space, every pixel surface in the bottom half, so the two
    // sets cannot overlap for *any* pair of layers, not just the same
    // layer.
    fn mask_surfaces_and_pixel_surfaces_occupy_opposite_halves_of_the_id_space() {
        let mut tree = LayerTree::new();
        for index in 0..8 {
            let id = match tree.add_pixel_layer(format!("layer {index}"), bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(mask) = tree.mask_surface_id(id) else {
                unreachable!("a freshly generated id is far below MASK_SURFACE_BIT");
            };
            let Some(pixels) = tree.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            assert!(mask.to_raw() >= crate::MASK_SURFACE_BIT);
            assert!(pixels.to_raw() < crate::MASK_SURFACE_BIT);
        }
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

    // --- the depth bound, enforced on the producer side too ----------

    /// Asserts `tree` is one both this crate's own validator and a real
    /// `postcard` round trip accept — the two bars a document has to
    /// clear to be saveable at all. Used after every step of the tests
    /// below, so that "the API built it" and "`validate_shape` accepts
    /// it" are checked to be the same set at each boundary rather than
    /// only at the end.
    fn assert_round_trips(tree: &LayerTree) {
        if let Err(err) = tree.validate() {
            unreachable!("a tree the public API built must validate: {err:?}");
        }
        let bytes = match postcard::to_allocvec(tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("encoding must succeed: {err:?}"),
        };
        if let Err(err) = postcard::from_bytes::<LayerTree>(&bytes) {
            unreachable!("a tree the public API built must round-trip: {err}");
        }
    }

    /// A root-level chain of `height` nested groups. Returns every id,
    /// outermost first, so a caller can name any level of it.
    fn group_chain(tree: &mut LayerTree, height: usize, label: &str) -> Vec<super::LayerId> {
        let mut ids = Vec::with_capacity(height);
        let mut parent = None;
        for level in 0..height {
            let id = match tree.add_group(format!("{label}-{level}"), parent) {
                Ok(id) => id,
                Err(err) => unreachable!("building a legal chain must succeed: {err:?}"),
            };
            ids.push(id);
            parent = Some(id);
        }
        ids
    }

    #[test]
    fn adding_a_layer_past_the_depth_limit_is_refused_and_changes_nothing() {
        let mut tree = LayerTree::new();
        let chain = group_chain(&mut tree, super::MAX_LAYER_TREE_DEPTH, "g");
        let Some(&deepest) = chain.last() else {
            unreachable!("the chain is not empty");
        };

        // The legal side first: `deepest` sits at exactly
        // `MAX_LAYER_TREE_DEPTH`, so the whole chain was accepted.
        assert_eq!(tree.len(), super::MAX_LAYER_TREE_DEPTH);
        assert_round_trips(&tree);

        let before = tree.len();
        let before_children = tree.children(deepest).map(<[_]>::to_vec);
        for outcome in [
            tree.add_pixel_layer("one too deep", bounds(), Some(deepest)),
            tree.add_group("one too deep", Some(deepest)),
        ] {
            match outcome {
                Err(DocError::LayerTreeTooDeep { depth, max }) => {
                    assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
                    assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
                }
                other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
            }
        }
        assert_eq!(tree.len(), before, "a refused add must add nothing");
        assert_eq!(
            tree.children(deepest).map(<[_]>::to_vec),
            before_children,
            "the intended parent's children must be untouched"
        );
        assert_round_trips(&tree);

        // And the boundary is the right side of the fence: one level up
        // still takes a child, landing it at exactly the maximum.
        let Some(&one_above) = chain.get(super::MAX_LAYER_TREE_DEPTH - 2) else {
            unreachable!("the chain has at least two levels");
        };
        if let Err(err) = tree.add_pixel_layer("exactly at the limit", bounds(), Some(one_above)) {
            unreachable!("a child landing at exactly the limit must be accepted: {err:?}");
        }
        assert_round_trips(&tree);
    }

    #[test]
    fn reparenting_a_subtree_past_the_depth_limit_is_refused_and_changes_nothing() {
        // Two root chains: `a` 128 tall, `b` 128 tall. Moving `b`'s root
        // under `a`'s deepest group lands `b`'s deepest node at
        // 128 + 128 = 256 -- exactly the limit.
        let mut tree = LayerTree::new();
        let a = group_chain(&mut tree, 128, "a");
        let b = group_chain(&mut tree, 128, "b");
        let (Some(&a_deepest), Some(&b_root)) = (a.last(), b.first()) else {
            unreachable!("both chains are non-empty");
        };
        if let Err(err) = tree.reparent(b_root, Some(a_deepest), 0) {
            unreachable!("a move landing exactly at the limit must succeed: {err:?}");
        }
        assert_round_trips(&tree);

        // One taller, and the same move is refused -- and note what
        // trips it: `b_root` itself would land at 129, comfortably
        // legal. It is the moved subtree's own *height* that pushes its
        // deepest descendant past the bound, which is the whole reason
        // `subtree_height` exists.
        let mut tree = LayerTree::new();
        let a = group_chain(&mut tree, 128, "a");
        let b = group_chain(&mut tree, 129, "b");
        let (Some(&a_deepest), Some(&b_root)) = (a.last(), b.first()) else {
            unreachable!("both chains are non-empty");
        };
        let old_roots = tree.roots().to_vec();
        let old_children = tree.children(a_deepest).map(<[_]>::to_vec);
        let old_parent = tree.parent(b_root);
        match tree.reparent(b_root, Some(a_deepest), 0) {
            Err(DocError::LayerTreeTooDeep { depth, max }) => {
                assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
                assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }
        assert_eq!(
            tree.roots(),
            old_roots.as_slice(),
            "a refused reparent must leave the old sibling list alone"
        );
        assert!(
            old_roots.contains(&b_root),
            "the moved layer was a root and must still be listed as one"
        );
        assert_eq!(
            tree.children(a_deepest).map(<[_]>::to_vec),
            old_children,
            "a refused reparent must not attach anything to the destination"
        );
        assert_eq!(
            tree.parent(b_root),
            old_parent,
            "a refused reparent must leave the moved entry's own parent alone"
        );
        assert_round_trips(&tree);

        // A leaf, moved to the deepest legal spot and then one past it:
        // the single-node case of the same bound (`subtree_height` of a
        // leaf is 1, so it lands exactly where its new parent's depth
        // plus one says).
        let mut tree = LayerTree::new();
        let chain = group_chain(&mut tree, super::MAX_LAYER_TREE_DEPTH, "c");
        let leaf = match tree.add_pixel_layer("leaf", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let (Some(&deepest), Some(&one_above)) =
            (chain.last(), chain.get(super::MAX_LAYER_TREE_DEPTH - 2))
        else {
            unreachable!("the chain has at least two levels");
        };
        match tree.reparent(leaf, Some(deepest), 0) {
            Err(DocError::LayerTreeTooDeep { depth, max }) => {
                assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
                assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }
        if let Err(err) = tree.reparent(leaf, Some(one_above), 0) {
            unreachable!("a leaf landing at exactly the limit must move: {err:?}");
        }
        assert_round_trips(&tree);
    }

    #[test]
    // The whole point of 0.50.0, stated as one property: every tree the
    // public API is willing to build is one `validate_shape` is willing
    // to accept -- which is what makes it saveable, since `.aur`'s write
    // path verifies by reopening through exactly that validator.
    // Deterministic and table-driven; no randomness, so a failure is
    // always reproducible.
    //
    // Deliberately one test rather than five: the property is about the
    // *sequence* -- each step round-trips against the state the previous
    // ones left, which is exactly what a set of independent tests would
    // stop checking. Same precedent as `history.rs`'s own long
    // scenario test.
    #[allow(clippy::too_many_lines)]
    fn every_tree_the_public_api_will_build_is_one_validate_shape_accepts() {
        // 1. Pure-`insert` chains, at the interesting depths.
        for &height in &[1_usize, 2, 3, 255, super::MAX_LAYER_TREE_DEPTH] {
            let mut tree = LayerTree::new();
            let mut parent = None;
            for level in 0..height {
                parent = match tree.add_group(format!("g{level}"), parent) {
                    Ok(id) => Some(id),
                    Err(err) => unreachable!("depth {} must be legal: {err:?}", level + 1),
                };
                assert_round_trips(&tree);
            }
            assert_eq!(tree.len(), height);
        }

        // 2. The refusal boundary, and that a refused call is inert --
        // including the id generator, which is why the encoded bytes are
        // compared before and after.
        let mut tree = LayerTree::new();
        let chain = group_chain(&mut tree, super::MAX_LAYER_TREE_DEPTH, "g");
        let Some(&deepest) = chain.last() else {
            unreachable!("the chain is not empty");
        };
        let before = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let roots_before = tree.roots().to_vec();
        match tree.add_group("past it", Some(deepest)) {
            Err(DocError::LayerTreeTooDeep { depth, max }) => {
                assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
                assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }
        assert_eq!(tree.len(), super::MAX_LAYER_TREE_DEPTH);
        assert_eq!(tree.roots(), roots_before.as_slice());
        assert_eq!(tree.children(deepest), Some(&[][..]));
        let after = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            before, after,
            "a refused add must not even move the id generator"
        );
        assert_round_trips(&tree);

        // 3. `reparent` landing a whole subtree exactly at the limit.
        let mut tree = LayerTree::new();
        let a = group_chain(&mut tree, 128, "a");
        let b = group_chain(&mut tree, 128, "b");
        let (Some(&a_deepest), Some(&b_root)) = (a.last(), b.first()) else {
            unreachable!("both chains are non-empty");
        };
        if let Err(err) = tree.reparent(b_root, Some(a_deepest), 0) {
            unreachable!("256 is legal, so this move must succeed: {err:?}");
        }
        assert_round_trips(&tree);

        // 4. One past it, refused, with nothing moved.
        let mut tree = LayerTree::new();
        let a = group_chain(&mut tree, 128, "a");
        let b = group_chain(&mut tree, 129, "b");
        let (Some(&a_deepest), Some(&b_root)) = (a.last(), b.first()) else {
            unreachable!("both chains are non-empty");
        };
        match tree.reparent(b_root, Some(a_deepest), 0) {
            Err(DocError::LayerTreeTooDeep { depth, .. }) => {
                assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }
        assert!(tree.roots().contains(&b_root));
        assert_eq!(tree.children(a_deepest), Some(&[][..]));
        assert_eq!(tree.parent(b_root), None);
        assert_round_trips(&tree);

        // 5. A mixed sequence: remove an interior group (which frees the
        // depth its subtree occupied), re-add into the freed room, and
        // move a leaf sideways between two parents at equal depth.
        let mut tree = LayerTree::new();
        let chain = group_chain(&mut tree, super::MAX_LAYER_TREE_DEPTH, "m");
        let Some(&interior) = chain.get(200) else {
            unreachable!("the chain is 256 long");
        };
        let Some(&above_interior) = chain.get(199) else {
            unreachable!("the chain is 256 long");
        };
        if let Err(err) = tree.remove(interior) {
            unreachable!("removing an interior group must succeed: {err:?}");
        }
        assert_eq!(tree.len(), 200);
        assert_round_trips(&tree);

        // `above_interior` sits at depth 200, so a fresh chain of 56
        // groups under it lands the last one at exactly 256 again.
        let mut parent = Some(above_interior);
        for level in 0..56 {
            parent = match tree.add_group(format!("re{level}"), parent) {
                Ok(id) => Some(id),
                Err(err) => unreachable!("re-adding into the freed depth: {err:?}"),
            };
            assert_round_trips(&tree);
        }

        // Two siblings at equal depth, and a leaf moved between them:
        // the depth guard must not object to a move that changes no
        // depth at all.
        let left = match tree.add_group("left", Some(above_interior)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let right = match tree.add_group("right", Some(above_interior)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.add_pixel_layer("leaf", bounds(), Some(left)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.reparent(leaf, Some(right), 0) {
            unreachable!("a sideways move at equal depth must succeed: {err:?}");
        }
        assert_eq!(tree.parent(leaf), Some(right));
        assert_round_trips(&tree);
    }

    /// `reparent`'s depth guard written out with the short-circuit
    /// removed -- the reference implementation the optimised one is
    /// compared against below.
    fn depth_guard_without_the_short_circuit(
        tree: &LayerTree,
        id: super::LayerId,
        new_parent: Option<super::LayerId>,
    ) -> Result<(), DocError> {
        let new_depth = new_parent.map_or(1, |p| tree.depth_of(p).saturating_add(1));
        let deepest = new_depth
            .saturating_add(tree.subtree_height(id)?)
            .saturating_sub(1);
        if deepest > super::MAX_LAYER_TREE_DEPTH {
            return Err(DocError::LayerTreeTooDeep {
                depth: deepest,
                max: super::MAX_LAYER_TREE_DEPTH,
            });
        }
        Ok(())
    }

    #[test]
    // `reparent` skips `subtree_height`'s whole downward walk when the
    // destination is no deeper than where `id` already sits. That is a
    // real performance change (5.8-8.8 ms to move a legal 40,000-layer
    // group before, 250 ns-3 us after, across three independent
    // measurements) and must not be a behaviour change, so every
    // case here runs the move for real and cross-checks it against the
    // un-short-circuited guard above. Covers all three directions a
    // real drag-and-drop produces: deeper, shallower, and sideways.
    fn the_reparent_depth_short_circuit_agrees_with_the_full_walk() {
        // (left height, right height, which one moves under which, the
        // outcome the depth rule should reach).
        let cases: [(usize, usize, bool, bool); 6] = [
            // b (128 tall) under a's deepest (depth 128) -> 256, legal.
            (128, 128, true, true),
            // b one taller -> 257, refused.
            (128, 129, true, false),
            // a's *deepest* group moved under b's root: a big move
            // towards the root, which the short-circuit takes.
            (128, 129, false, true),
            // The same, from a chain that is itself at the limit.
            (super::MAX_LAYER_TREE_DEPTH, 1, false, true),
            // A single-group chain moved under a chain at the limit:
            // lands at 257, refused.
            (super::MAX_LAYER_TREE_DEPTH, 1, true, false),
            // Both short, nothing near the bound.
            (2, 2, true, true),
        ];
        for (a_height, b_height, move_b_root_under_a, expect_ok) in cases {
            let mut tree = LayerTree::new();
            let a = group_chain(&mut tree, a_height, "a");
            let b = group_chain(&mut tree, b_height, "b");
            let (Some(&a_deepest), Some(&b_root)) = (a.last(), b.first()) else {
                unreachable!("both chains are non-empty");
            };
            let (moved, destination) = if move_b_root_under_a {
                (b_root, a_deepest)
            } else {
                (a_deepest, b_root)
            };
            let reference = depth_guard_without_the_short_circuit(&tree, moved, Some(destination));
            let actual = tree.reparent(moved, Some(destination), 0);
            match (&reference, &actual) {
                (Ok(()), Ok(())) => assert!(
                    expect_ok,
                    "case ({a_height}, {b_height}, {move_b_root_under_a}) was expected to be refused"
                ),
                (
                    Err(DocError::LayerTreeTooDeep { depth, max }),
                    Err(DocError::LayerTreeTooDeep {
                        depth: got_depth,
                        max: got_max,
                    }),
                ) => {
                    assert!(
                        !expect_ok,
                        "case ({a_height}, {b_height}, {move_b_root_under_a}) was expected to be allowed"
                    );
                    assert_eq!(depth, got_depth, "the reported depth must match too");
                    assert_eq!(max, got_max);
                }
                (reference, actual) => unreachable!(
                    "short-circuit disagreed with the full walk on case \
                     ({a_height}, {b_height}, {move_b_root_under_a}): \
                     full walk said {reference:?}, reparent said {actual:?}"
                ),
            }
            if actual.is_ok() {
                assert_round_trips(&tree);
            }
        }
    }

    // --- `subtree_height`'s malformed-tree escape paths ---------------
    //
    // Neither shape below is reachable from any file `aurora-io` will
    // read or any live edit: `validate_shape` refuses both at
    // deserialization time, and no method on `LayerTree` can build
    // either. They are constructed here the way this file's other
    // hardening tests construct their trees -- by hand, straight into
    // the private struct -- because "unreachable today" is an argument,
    // and the point of these is that the walk survives being wrong
    // anyway.

    #[test]
    fn reparenting_a_group_that_lists_the_same_child_twice_is_refused() {
        // `into` is an empty root group; `shared` is listed twice by
        // `holder`, so a downward walk of `holder` reaches it twice.
        let holder = super::LayerId::from_raw(0);
        let shared = super::LayerId::from_raw(1);
        let into = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(holder, group_entry("holder", None, vec![shared, shared]));
        layers.insert(shared, pixel_entry("shared", Some(holder)));
        layers.insert(into, group_entry("into", None, Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![holder, into],
        };

        // `into` sits at depth 1, so this move lands `holder` at 2 --
        // deeper than the 1 it has now, which is what makes the guard
        // actually walk rather than short-circuit.
        match tree.reparent(holder, Some(into), 0) {
            Err(DocError::MalformedLayerTree(id)) => assert_eq!(id, shared),
            other => unreachable!("expected MalformedLayerTree, got {other:?}"),
        }
        // Refused before the first mutation, like every other refusal here.
        assert_eq!(tree.roots(), &[holder, into][..]);
        assert_eq!(tree.children(into), Some(&[][..]));
        assert_eq!(tree.parent(holder), None);
    }

    #[test]
    fn reparenting_within_a_children_cycle_returns_rather_than_hanging() {
        // `a`'s children name `b`, `b`'s name `a`: a cycle in the
        // *downward* direction, which `is_descendant`'s upward walk
        // cannot see. Only `subtree_height`'s own `visited` set stops
        // this, and the guarantee is that it stops -- an unbounded walk
        // here is a hang, and a recursive one a process abort under
        // `panic = "abort"`.
        let a = super::LayerId::from_raw(0);
        let b = super::LayerId::from_raw(1);
        let into = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", None, vec![b]));
        layers.insert(b, group_entry("b", Some(a), vec![a]));
        layers.insert(into, group_entry("into", None, Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![a, into],
        };

        let started = std::time::Instant::now();
        let result = tree.reparent(a, Some(into), 0);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a children cycle must not make reparent run away"
        );
        match result {
            Err(DocError::MalformedLayerTree(id)) => assert_eq!(id, a),
            other => unreachable!("expected MalformedLayerTree, got {other:?}"),
        }
        assert_eq!(tree.parent(a), None);
    }

    #[test]
    fn a_non_deepening_move_of_an_already_malformed_subtree_succeeds_without_worsening_it() {
        // The short-circuit's one disclosed behaviour change, pinned by
        // a real test rather than left as a claim in a doc comment.
        // `holder` starts nested one level under `container` (depth 2)
        // and is malformed the same way as
        // `reparenting_a_group_that_lists_the_same_child_twice_is_refused`
        // above; `target` is a separate root. Moving `holder` from
        // `container` to `target` lands it at the same depth (2) it
        // already had -- `new_depth &lt;= depth_of(id)`, so the walk that
        // would notice the duplicate child is skipped.
        let container = super::LayerId::from_raw(0);
        let holder = super::LayerId::from_raw(1);
        let shared = super::LayerId::from_raw(2);
        let target = super::LayerId::from_raw(3);
        let mut layers = HashMap::new();
        layers.insert(container, group_entry("container", None, vec![holder]));
        layers.insert(
            holder,
            group_entry("holder", Some(container), vec![shared, shared]),
        );
        layers.insert(shared, pixel_entry("shared", Some(holder)));
        layers.insert(target, group_entry("target", None, Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![container, target],
        };
        let holder_depth_before = tree.depth_of(holder);
        assert_eq!(
            holder_depth_before, 2,
            "holder starts one level under container"
        );
        let holder_children_before = tree.children(holder).map(<[_]>::to_vec);

        match tree.reparent(holder, Some(target), 0) {
            Ok(()) => {}
            other => unreachable!("expected Ok(()), got {other:?}"),
        }

        // The point of this test: the malformation is exactly what it
        // was before, not worse, and `holder` landed no deeper than it
        // already sat. A move that *would* deepen this same subtree
        // still walks and still refuses --
        // `reparenting_a_group_that_lists_the_same_child_twice_is_refused`
        // already proves that half.
        assert_eq!(tree.depth_of(holder), holder_depth_before);
        assert_eq!(
            tree.children(holder).map(<[_]>::to_vec),
            holder_children_before
        );
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
    fn set_bounds_rejects_an_origin_past_the_document_range_and_leaves_the_old_bounds() {
        // Pre-fix this call returned `Ok` and the pathological origin
        // went straight into the tree -- measured, not assumed. The
        // "leaves the old bounds" half is the part that matters most: a
        // refused move must not half-apply.
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let before = tree.bounds(id);
        let far = Rect {
            x: i64::MAX,
            y: 0,
            width: 10,
            height: 10,
        };
        match tree.set_bounds(id, far) {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, i64::MAX);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
        assert_eq!(
            tree.bounds(id),
            before,
            "a refused move must leave the layer exactly where it was"
        );
    }

    #[test]
    fn set_bounds_accepts_an_origin_exactly_at_the_document_range() {
        // The other side of the same check. The limits are legal scope,
        // and the negative one especially so -- a layer a whole document
        // extent off the top edge is somewhere a user can still drag it
        // back from, which is what `Rect`'s signed origin is for.
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let at_limit = Rect {
            x: aurora_core::MAX_DOCUMENT_ORIGIN,
            y: -aurora_core::MAX_DOCUMENT_ORIGIN,
            width: 10,
            height: 10,
        };
        if let Err(err) = tree.set_bounds(id, at_limit) {
            unreachable!("an origin exactly at the limit must be accepted: {err:?}");
        }
        assert_eq!(tree.bounds(id), Some(at_limit));
    }

    #[test]
    fn set_bounds_reports_an_unknown_layer_before_an_out_of_range_origin() {
        // Pins the precedence decision: both faults are present, and the
        // id fault is the one reported. Naming a layer that does not
        // exist should be reported as that rather than as a complaint
        // about a rectangle destined for an entry the method could not
        // have used anyway.
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(999);
        let far = Rect {
            x: i64::MAX,
            y: i64::MIN,
            width: 10,
            height: 10,
        };
        match tree.set_bounds(bogus, far) {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn add_pixel_layer_rejects_an_origin_past_the_document_range_and_adds_nothing() {
        // Pre-fix this returned `Ok` and the layer landed. The
        // "adds nothing" half is what `insert_unchecked`'s check placement
        // (before `next_id`) buys.
        let mut tree = LayerTree::new();
        let far = Rect {
            x: 0,
            y: i64::MIN,
            width: 10,
            height: 10,
        };
        match tree.add_pixel_layer("far", far, None) {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, 0);
                assert_eq!(y, i64::MIN);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
        assert_eq!(tree.len(), 0, "a refused insert must add nothing");
        assert!(tree.roots().is_empty());
    }

    #[test]
    fn add_mask_rejects_an_origin_past_the_document_range_and_leaves_the_layer_maskless() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let far = Rect {
            x: -aurora_core::MAX_DOCUMENT_ORIGIN - 1,
            y: 0,
            width: 10,
            height: 10,
        };
        match tree.add_mask(id, far) {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, -aurora_core::MAX_DOCUMENT_ORIGIN - 1);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
        assert!(
            tree.mask(id).is_none(),
            "a refused mask must leave the layer maskless, not half-masked"
        );
    }

    #[test]
    fn add_mask_rejects_an_extent_past_the_document_ceiling_and_leaves_the_layer_maskless() {
        // The origin bar's companion (0.71.3). Since 0.71.0 a mask's
        // rectangle drives a real tile grid in `aurora-io`'s `.aur`
        // writer, so an oversized one there is an unfinishable loop --
        // and `aurora-io` refusing it means refusing the *whole
        // document*, so a single oversized mask would silently disable
        // every save and every crash-recovery autosave for the rest of
        // the session. Refused where it is created instead.
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let huge = Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT + 1,
            height: 1,
        };
        match tree.add_mask(id, huge) {
            Err(DocError::LayerBoundsTooLarge { width, height, max }) => {
                assert_eq!(width, aurora_core::MAX_DOCUMENT_EXTENT + 1);
                assert_eq!(height, 1);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_EXTENT);
            }
            other => unreachable!("expected LayerBoundsTooLarge, got {other:?}"),
        }
        assert!(
            tree.mask(id).is_none(),
            "a refused mask must leave the layer maskless, not half-masked"
        );

        // And the documented ceiling itself is still legal scope (PRD
        // §7.3.1) -- the bar must not be what rejects the largest mask a
        // real document can carry.
        let at_ceiling = Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: aurora_core::MAX_DOCUMENT_EXTENT,
        };
        if let Err(err) = tree.add_mask(id, at_ceiling) {
            unreachable!("a mask exactly at the document ceiling must be accepted: {err:?}");
        }
    }

    #[test]
    fn add_group_is_unaffected_by_the_origin_check() {
        // A group carries no bounds of its own, so `insert_unchecked`'s
        // `LayerKind::Pixel` guard must skip it entirely -- including a
        // group nested under another group, which goes through the exact
        // same insert path.
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_group("inner", Some(outer)) {
            unreachable!("a group has no bounds to be out of range: {err:?}");
        }
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.bounds(outer), None);
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

    // --- structural validation of a tree deserialized from bytes ------
    //
    // A `.aur` file's manifest is a `postcard`-encoded `LayerTree` from
    // an untrusted source, and until this round nothing checked that the
    // bytes described a tree at all. These tests hand-craft the ones a
    // writer would never produce.

    /// Field-for-field identical to `super::LayerTreeRepr`, so its own
    /// `postcard` bytes decode as a `LayerTree` -- the only way to build
    /// a structurally impossible tree, since every path through this
    /// type's own API refuses to make one.
    #[derive(serde::Serialize)]
    struct TreeReprForTest {
        ids: aurora_core::IdGenerator<Layer>,
        layers: HashMap<super::LayerId, super::LayerEntry>,
        roots: Vec<super::LayerId>,
    }

    /// An id generator positioned exactly where a real one would be for
    /// `layers`: one past the highest id present. Every crafted repr
    /// below uses it, so each test isolates the one defect it is about
    /// rather than also tripping `validate_id_allocator` (see
    /// `a_manifest_whose_id_counter_has_fallen_behind_is_rejected` for
    /// that check's own tests).
    fn ids_for(layers: &HashMap<super::LayerId, super::LayerEntry>) -> IdGenerator<Layer> {
        let mut ids = IdGenerator::new();
        if let Some(highest) = layers.keys().map(|id| id.to_raw()).max() {
            ids.advance_past(highest);
        }
        ids
    }

    fn group_entry(
        name: &str,
        parent: Option<super::LayerId>,
        children: Vec<super::LayerId>,
    ) -> super::LayerEntry {
        super::LayerEntry::new(name.to_owned(), parent, LayerKind::Group { children })
    }

    fn decode_tree(repr: &TreeReprForTest) -> Result<LayerTree, String> {
        let bytes = match postcard::to_allocvec(repr) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        postcard::from_bytes::<LayerTree>(&bytes).map_err(|err| err.to_string())
    }

    #[test]
    fn deserializing_a_group_that_contains_itself_is_rejected_not_a_stack_overflow() {
        // The whole reason this validation exists. Before it, a 226-byte
        // crafted `.aur` file carrying exactly this tree aborted the
        // process with `fatal runtime error: stack overflow` (exit 134,
        // core dumped) on `aurora-io`'s own read path -- measured, not
        // assumed. A stack overflow is not a catchable panic, so no
        // amount of error handling downstream could have contained it;
        // the tree has to be refused before anything walks it.
        let root = super::LayerId::from_raw(0);
        let mut layers = HashMap::new();
        layers.insert(root, group_entry("cycle", None, vec![root]));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![root],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a group listing itself as its own child must be refused"
        );
        // `postcard`'s own error type discards a `Display` message from a
        // `try_from` conversion (it is a no-alloc format, so every custom
        // Serde error collapses to one variant) -- so the reason is
        // pinned down here, against the validator itself, rather than
        // against the deserialized message.
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::MalformedLayerTree(id)) => assert_eq!(id, root),
            other => unreachable!("expected MalformedLayerTree, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_tree_listing_one_layer_under_two_parents_is_rejected() {
        // Not a cycle, but not a tree either: the shared layer gets
        // walked (and composited, and saved) twice.
        //
        // *Which* rule refuses it depends on which of the two parents
        // the walk reaches first, and both orders are worth pinning
        // because both are real files. `roots` is a `Vec` walked on a
        // LIFO stack, so listing `[a, b]` reaches `b` first: `shared`
        // records `a`, so the parent cross-check fires before the
        // duplicate one ever gets a chance. See
        // `deserializing_a_layer_reached_from_two_parents_honest_one_first_is_rejected`
        // for the other order, where the duplicate rule is what fires.
        let a = super::LayerId::from_raw(0);
        let b = super::LayerId::from_raw(1);
        let shared = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", None, vec![shared]));
        layers.insert(b, group_entry("b", None, vec![shared]));
        layers.insert(
            shared,
            super::LayerEntry::new(
                "shared".to_owned(),
                Some(a),
                LayerKind::Pixel { bounds: bounds() },
            ),
        );
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![a, b],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "the same layer under two parents must be refused"
        );
        // Pinned against the validator rather than postcard's own
        // flattened message, like every neighbouring test.
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, shared),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_layer_reached_from_two_parents_honest_one_first_is_rejected() {
        // The other walk order of the test above: `roots` listed `[b,
        // a]` reaches `a` first, whose claim on `shared` matches what
        // `shared` records -- so the parent check passes and it is the
        // *duplicate* rule that catches `b`'s second claim. This is the
        // order that guards what the duplicate rule is actually for: a
        // layer walked, composited, and saved twice.
        let a = super::LayerId::from_raw(0);
        let b = super::LayerId::from_raw(1);
        let shared = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", None, vec![shared]));
        layers.insert(b, group_entry("b", None, vec![shared]));
        layers.insert(shared, pixel_entry("shared", Some(a)));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![b, a],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "the same layer under two parents must be refused either way round"
        );
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::MalformedLayerTree(id)) => assert_eq!(id, shared),
            other => unreachable!("expected MalformedLayerTree, got {other:?}"),
        }
    }

    /// A chain of `depth` groups, each the sole child of the one above.
    fn nested_chain(depth: usize) -> TreeReprForTest {
        let mut layers = HashMap::new();
        for level in 0..depth {
            let id = super::LayerId::from_raw(level as u64);
            let children = if level + 1 < depth {
                vec![super::LayerId::from_raw(level as u64 + 1)]
            } else {
                Vec::new()
            };
            let parent = level
                .checked_sub(1)
                .map(|above| super::LayerId::from_raw(above as u64));
            layers.insert(id, group_entry("g", parent, children));
        }
        TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![super::LayerId::from_raw(0)],
        }
    }

    #[test]
    fn deserializing_a_tree_nested_past_the_depth_limit_is_rejected() {
        // Deliberately one level past the limit rather than deep enough
        // to really overflow a call stack: this must be a fast,
        // deterministic test, not one that risks taking the test runner
        // down with it.
        let repr = nested_chain(super::MAX_LAYER_TREE_DEPTH + 1);
        assert!(
            decode_tree(&repr).is_err(),
            "nesting past the depth limit must be refused"
        );
        // See the cycle test above for why the reason is checked against
        // the validator rather than the deserializer's own message.
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::LayerTreeTooDeep { depth, max }) => {
                assert_eq!(depth, super::MAX_LAYER_TREE_DEPTH + 1);
                assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_tree_nested_exactly_at_the_depth_limit_still_works() {
        // The other side of the same check: the limit is documented,
        // legal scope, so it must not be what gets rejected.
        let repr = nested_chain(super::MAX_LAYER_TREE_DEPTH);
        if let Err(message) = decode_tree(&repr) {
            unreachable!("nesting exactly at the limit must still load: {message}");
        }
    }

    #[test]
    fn deserializing_a_real_tree_round_trips_unchanged() {
        // The validation must not have changed the wire format: a tree
        // this type built itself still encodes and decodes to the same
        // shape (ADR 0009's backward-compatibility policy).
        let mut tree = LayerTree::new();
        let group = match tree.add_group("Group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_pixel_layer("Inner", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let restored = match postcard::from_bytes::<LayerTree>(&bytes) {
            Ok(restored) => restored,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(restored.roots(), &[group]);
        assert_eq!(restored.children(group), Some([inner].as_slice()));
        assert_eq!(restored.paint_order(), vec![inner]);
    }

    #[test]
    fn paint_order_terminates_on_a_cyclic_tree_rather_than_recursing_forever() {
        // Belt and braces: `Deserialize` already refuses a cyclic tree,
        // so this shape can no longer arrive from a file. It is
        // constructed by hand here (only possible from inside this
        // module) because `paint_order` runs on the compositing path and
        // returns a plain `Vec` with nowhere to report an error -- the
        // guarantee worth pinning down is that it *returns*.
        let root = super::LayerId::from_raw(0);
        let leaf = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(root, group_entry("cycle", None, vec![leaf, root]));
        layers.insert(
            leaf,
            super::LayerEntry::new(
                "leaf".to_owned(),
                Some(root),
                LayerKind::Pixel { bounds: bounds() },
            ),
        );
        let tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![root],
        };
        let started = std::time::Instant::now();
        let order = tree.paint_order();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a cyclic tree must not make paint_order run away"
        );
        assert!(
            order.len() <= tree.len(),
            "paint_order must not emit more entries than the tree holds: {order:?}"
        );
    }

    #[test]
    fn reparent_terminates_on_a_cyclic_parent_chain() {
        // `is_descendant` walks *upward* through `parent` links, which a
        // tree restored from an untrusted history journal can make loop.
        // As above, the guarantee is that it returns -- refusing the
        // move, which is the safe direction on a broken tree.
        let a = super::LayerId::from_raw(0);
        let b = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", Some(b), Vec::new()));
        layers.insert(b, group_entry("b", Some(a), Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: Vec::new(),
        };
        let started = std::time::Instant::now();
        let result = tree.reparent(a, Some(b), 0);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a cyclic parent chain must not make reparent run away"
        );
        assert!(
            result.is_err(),
            "reparenting within a cyclic chain must be refused, not performed"
        );
    }

    // --- parent back-links and orphans -------------------------------
    //
    // `validate_shape` used to check only that the *downward* walk from
    // `roots` was a tree. Nothing cross-checked each `LayerEntry`'s own
    // recorded `parent` against that walk, and nothing noticed an entry
    // the walk never reached at all -- yet `remove_capturing` and
    // `reparent` both read that recorded `parent` and then expect to
    // find the id in the sibling list it names. These craft the trees
    // where that expectation is false.

    fn pixel_entry(name: &str, parent: Option<super::LayerId>) -> super::LayerEntry {
        super::LayerEntry::new(
            name.to_owned(),
            parent,
            LayerKind::Pixel { bounds: bounds() },
        )
    }

    #[test]
    fn deserializing_a_root_that_records_a_parent_is_rejected() {
        // It sits in `roots`, so its real parent is "none" -- but it
        // claims to live inside `other`, whose `children` never mentions
        // it. `remove_capturing` would look it up in `other`'s children
        // and find nothing.
        let root = super::LayerId::from_raw(0);
        let other = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(root, pixel_entry("root", Some(other)));
        layers.insert(other, group_entry("other", None, Vec::new()));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![root, other],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a root recording a parent must be refused"
        );
        // See the cycle test above for why the reason is pinned against
        // the validator rather than postcard's own flattened message.
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, root),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_child_that_records_the_wrong_group_as_its_parent_is_rejected() {
        // `leaf` really sits in `a`'s children, but records `b`. Both
        // groups exist and both are groups, so nothing but the
        // cross-check catches it.
        let a = super::LayerId::from_raw(0);
        let b = super::LayerId::from_raw(1);
        let leaf = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", None, vec![leaf]));
        layers.insert(b, group_entry("b", None, Vec::new()));
        layers.insert(leaf, pixel_entry("leaf", Some(b)));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![a, b],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a child recording the wrong group must be refused"
        );
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, leaf),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_child_that_records_no_parent_at_all_is_rejected() {
        // The mirror of the root case: it really sits inside `a`, but
        // records `None`, so `remove_capturing` would look for it among
        // the tree's own roots.
        let a = super::LayerId::from_raw(0);
        let leaf = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(a, group_entry("a", None, vec![leaf]));
        layers.insert(leaf, pixel_entry("leaf", None));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![a],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a child recording no parent must be refused"
        );
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, leaf),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
    }

    #[test]
    fn deserializing_a_tree_holding_a_layer_unreachable_from_its_roots_is_rejected() {
        // In the map, named by nothing: invisible to every traversal in
        // this project, yet still counted by `len` and still written
        // back out on save.
        let root = super::LayerId::from_raw(0);
        let orphan = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(root, group_entry("root", None, Vec::new()));
        layers.insert(orphan, pixel_entry("orphan", None));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![root],
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a layer unreachable from the roots must be refused"
        );
        match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
            Err(DocError::OrphanedLayer(id)) => assert_eq!(id, orphan),
            other => unreachable!("expected OrphanedLayer, got {other:?}"),
        }
    }

    #[test]
    fn the_orphan_reported_is_the_lowest_numbered_one_not_whichever_hashmap_yields_first() {
        // Two orphans, so which one gets named is a real choice. It must
        // not depend on `HashMap`'s own per-process iteration order, or
        // the same file produces a different error message on every run.
        // The root is deliberately the *highest* id, so "first in the
        // map" and "lowest numbered" cannot coincide by accident.
        let low = super::LayerId::from_raw(1);
        let high = super::LayerId::from_raw(2);
        let root = super::LayerId::from_raw(3);
        let mut layers = HashMap::new();
        layers.insert(root, group_entry("root", None, Vec::new()));
        layers.insert(high, pixel_entry("high", None));
        layers.insert(low, pixel_entry("low", None));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![root],
        };
        // Repeated, because a single run of a `HashMap` walk can agree
        // with the required answer by luck.
        for _ in 0..32 {
            match super::validate_shape(&repr.layers, &repr.roots, None, 1) {
                Err(DocError::OrphanedLayer(id)) => assert_eq!(id, low),
                other => unreachable!("expected OrphanedLayer, got {other:?}"),
            }
        }
    }

    // --- the three sites that used to be `unreachable!()` -------------
    //
    // Each of these builds an inconsistent tree by hand (struct literal,
    // only possible from inside this module) rather than through
    // `decode_tree`, precisely because the validator above now refuses
    // that shape at the door. What is under test here is the runtime
    // site itself: that it *returns* rather than aborting the process,
    // which under this workspace's `panic = "abort"` release profile is
    // what an `unreachable!()` would have done.

    #[test]
    fn removing_a_layer_whose_recorded_parent_is_a_pixel_layer_errors_rather_than_aborting() {
        let victim = super::LayerId::from_raw(0);
        let not_a_group = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", Some(not_a_group)));
        layers.insert(not_a_group, pixel_entry("not a group", None));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![not_a_group],
        };
        match tree.remove(victim) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, not_a_group),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
        assert!(
            tree.contains(victim),
            "a refused remove must leave the tree alone"
        );
    }

    #[test]
    fn removing_a_layer_its_recorded_parent_does_not_list_errors_rather_than_aborting() {
        let victim = super::LayerId::from_raw(0);
        let group = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", Some(group)));
        // A real group -- it just does not list `victim` as a child.
        layers.insert(group, group_entry("group", None, Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![group],
        };
        match tree.remove(victim) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, victim),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
        assert!(
            tree.contains(victim),
            "a refused remove must leave the tree alone"
        );
    }

    #[test]
    fn reparenting_a_layer_whose_recorded_parent_is_a_pixel_layer_errors_rather_than_aborting() {
        let victim = super::LayerId::from_raw(0);
        let not_a_group = super::LayerId::from_raw(1);
        let destination = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", Some(not_a_group)));
        layers.insert(not_a_group, pixel_entry("not a group", None));
        layers.insert(destination, group_entry("destination", None, Vec::new()));
        let mut tree = LayerTree {
            ids: aurora_core::IdGenerator::new(),
            layers,
            roots: vec![not_a_group, destination],
        };
        match tree.reparent(victim, Some(destination), 0) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, not_a_group),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
        // The error is returned before the first mutation, so the move
        // must not be half-applied.
        assert_eq!(tree.parent(victim), Some(not_a_group));
        assert_eq!(tree.children(destination), Some([].as_slice()));
        assert_eq!(tree.roots(), &[not_a_group, destination]);
    }

    #[test]
    fn every_legitimate_remove_and_reparent_on_a_decoded_tree_still_succeeds() {
        // The positive control for the three tests above: the new checks
        // must refuse only genuinely broken trees, never real usage. A
        // three-level tree is decoded from bytes (so it goes through the
        // validator), then every id in it is moved and removed through
        // the ordinary public API.
        let outer = super::LayerId::from_raw(0);
        let inner = super::LayerId::from_raw(1);
        let deep = super::LayerId::from_raw(2);
        let sibling = super::LayerId::from_raw(3);
        let mut layers = HashMap::new();
        layers.insert(outer, group_entry("outer", None, vec![inner]));
        layers.insert(inner, group_entry("inner", Some(outer), vec![deep]));
        layers.insert(deep, pixel_entry("deep", Some(inner)));
        layers.insert(sibling, pixel_entry("sibling", None));
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![outer, sibling],
        };
        let mut tree = match decode_tree(&repr) {
            Ok(tree) => tree,
            Err(message) => unreachable!("a well-formed tree must still decode: {message}"),
        };

        // Every legal destination, including back to root.
        for (id, destination) in [
            (deep, None),
            (deep, Some(inner)),
            (sibling, Some(outer)),
            (sibling, Some(inner)),
            (sibling, None),
            (inner, None),
            (inner, Some(outer)),
        ] {
            if let Err(err) = tree.reparent(id, destination, 0) {
                unreachable!("reparenting {id:?} under {destination:?} must succeed: {err:?}");
            }
        }

        // Then remove every id, innermost first, so each one is removed
        // from a different kind of sibling list.
        for id in [deep, sibling, inner, outer] {
            if let Err(err) = tree.remove(id) {
                unreachable!("removing {id:?} must succeed: {err:?}");
            }
        }
        assert!(tree.is_empty());
        assert!(tree.roots().is_empty());
    }

    // --- the id allocator, not the shape ------------------------------
    //
    // A tree can be a perfectly well-formed tree and still be unsafe to
    // *add to*: `IdGenerator` is `Deserialize`, so a crafted manifest
    // can carry a counter that has already been used. Every check above
    // passes on such a file. These cover both halves of the defence --
    // refusing the file, and refusing the insert.

    #[test]
    fn a_manifest_whose_id_counter_has_fallen_behind_is_rejected() {
        // Shape-wise flawless: one group at the root holding one pixel
        // layer, every recorded parent correct, nothing orphaned. The
        // only thing wrong is the counter: it says the next id to hand
        // out is 1, while layer 1 is right there in the map.
        let target = super::LayerId::from_raw(1);
        let held = super::LayerId::from_raw(0);
        let mut layers = HashMap::new();
        layers.insert(target, group_entry("target", None, vec![held]));
        layers.insert(held, pixel_entry("held", Some(target)));
        let roots = vec![target];

        // The shape validator has no complaint -- that is the point.
        if let Err(err) = super::validate_shape(&layers, &roots, None, 1) {
            unreachable!("the shape is valid; only the counter is stale: {err:?}");
        }

        let mut stale = IdGenerator::new();
        let _ = stale.next_id(); // counter now 1, and layer 1 exists.
        match super::validate_id_allocator(&stale, &layers) {
            Err(DocError::StaleLayerIdGenerator { next, existing }) => {
                assert_eq!(next, 1);
                assert_eq!(existing, target);
            }
            other => unreachable!("expected StaleLayerIdGenerator, got {other:?}"),
        }

        // And it is refused at the door, through real bytes. (The reason
        // is pinned against the validator above rather than this
        // message, for the same reason every neighbouring test does it
        // that way: postcard flattens a `TryFrom` error to one opaque
        // string.)
        let repr = TreeReprForTest {
            ids: stale,
            layers,
            roots,
        };
        assert!(
            decode_tree(&repr).is_err(),
            "a manifest whose id counter has already been used must be refused"
        );
    }

    #[test]
    fn a_counter_exactly_one_past_the_highest_id_is_accepted() {
        // The boundary, in the direction that must keep working: this is
        // precisely where a real generator sits after allocating both
        // ids, so an off-by-one here would refuse every genuine file.
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_pixel_layer("leaf", bounds(), Some(group)) {
            unreachable!("{err:?}");
        }
        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = postcard::from_bytes::<LayerTree>(&bytes) {
            unreachable!("a tree written by this very build must decode: {err}");
        }
    }

    #[test]
    fn a_manifest_holding_a_layer_id_with_the_mask_surface_bit_set_is_rejected() {
        // The cross-layer surface collision, as the crafted file that
        // reaches it. Shape-wise flawless again -- two root pixel
        // layers, no cycle, no orphan, no dangling reference, and a
        // counter comfortably ahead of both ids, so every other
        // validator passes. What is wrong is only which *numbers* the
        // ids are: `victim` is an ordinary layer that carries a mask,
        // and `attacker` is `victim`'s id with `MASK_SURFACE_BIT` set.
        //
        // `LayerTree::surface_id(attacker)` is `attacker.to_raw()` and
        // `LayerTree::mask_surface_id(victim)` is
        // `victim.to_raw() | MASK_SURFACE_BIT` -- the same number. One
        // `aurora_tile` slot, two owners: painting the attacker layer's
        // pixels rewrites what the victim layer's mask reads back as
        // coverage, and vice versa, with no error raised anywhere.
        let victim = super::LayerId::from_raw(5);
        let attacker = super::LayerId::from_raw(5 | crate::MASK_SURFACE_BIT);
        assert_eq!(
            attacker.to_raw(),
            victim.to_raw() | crate::MASK_SURFACE_BIT,
            "the two ids really do alias one surface -- that is the bug"
        );

        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", None));
        layers.insert(attacker, pixel_entry("attacker", None));
        let roots = vec![victim, attacker];

        // Neither of the pre-0.70.1 whole-tree checks has a complaint,
        // which is exactly why this shipped.
        if let Err(err) = super::validate_shape(&layers, &roots, None, 1) {
            unreachable!("the shape is valid; only the id numbering is: {err:?}");
        }
        let ids = ids_for(&layers);
        if let Err(err) = super::validate_id_allocator(&ids, &layers) {
            unreachable!("the counter is ahead of both ids: {err:?}");
        }

        match super::validate_layer_id_range(&ids, &layers) {
            Err(DocError::ReservedLayerIdBit { id, limit }) => {
                assert_eq!(id, attacker);
                assert_eq!(limit, crate::MASK_SURFACE_BIT);
            }
            other => unreachable!("expected ReservedLayerIdBit, got {other:?}"),
        }

        // And refused at the door, through real bytes -- the reason
        // pinned against the validator above rather than the message,
        // for the same reason every neighbouring test does it that way.
        let repr = TreeReprForTest { ids, layers, roots };
        assert!(
            decode_tree(&repr).is_err(),
            "a manifest whose layer id aliases another layer's mask surface must be refused"
        );
    }

    #[test]
    fn a_manifest_whose_id_counter_is_already_in_the_mask_surface_half_is_rejected() {
        // The other half of the same door. This file carries no
        // offending id at all -- just one ordinary layer 0 -- but its
        // counter is parked so that the *next* ordinary
        // `add_pixel_layer` hands out an id with `MASK_SURFACE_BIT`
        // set, which is the same collision one user action later.
        let held = super::LayerId::from_raw(0);
        let mut layers = HashMap::new();
        layers.insert(held, pixel_entry("held", None));
        let roots = vec![held];

        let mut ids: IdGenerator<Layer> = IdGenerator::new();
        ids.advance_past(crate::MASK_SURFACE_BIT - 1);
        assert_eq!(ids.peek_next(), crate::MASK_SURFACE_BIT);
        if let Err(err) = super::validate_id_allocator(&ids, &layers) {
            unreachable!("the counter is far ahead of layer 0: {err:?}");
        }

        match super::validate_layer_id_range(&ids, &layers) {
            Err(DocError::ReservedLayerIdCounter { next, limit }) => {
                assert_eq!(next, crate::MASK_SURFACE_BIT);
                assert_eq!(limit, crate::MASK_SURFACE_BIT);
            }
            other => unreachable!("expected ReservedLayerIdCounter, got {other:?}"),
        }

        let repr = TreeReprForTest { ids, layers, roots };
        assert!(
            decode_tree(&repr).is_err(),
            "a manifest whose id counter is about to hand out a mask-surface id must be refused"
        );
    }

    #[test]
    fn surface_id_refuses_an_id_in_the_mask_surface_half_even_past_validation() {
        // The belt-and-braces half of the fix: the guard lives on the
        // accessor too, not only in the validator, so a tree assembled
        // some third way still cannot hand out an aliasing surface.
        // Struct literal, because `validate_layer_id_range` is exactly
        // what stops every ordinary path from producing this.
        let victim = super::LayerId::from_raw(5);
        let attacker = super::LayerId::from_raw(5 | crate::MASK_SURFACE_BIT);
        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", None));
        layers.insert(attacker, pixel_entry("attacker", None));
        let forced = LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![victim, attacker],
        };

        assert!(forced.contains(attacker));
        assert_eq!(
            forced.surface_id(attacker),
            None,
            "a pixel surface must never be addressed in the mask half"
        );
        // The victim's own mask surface is unaffected, and so is its
        // own pixel surface -- the guard is one-sided.
        assert_eq!(
            forced.mask_surface_id(victim),
            Some(aurora_tile::SurfaceId::from_raw(
                5 | crate::MASK_SURFACE_BIT
            ))
        );
        assert_eq!(
            forced.surface_id(victim),
            Some(aurora_tile::SurfaceId::from_raw(5))
        );
    }

    #[test]
    fn a_stale_counter_cannot_silently_destroy_the_layer_already_under_that_id() {
        // The whole reason the counter is checked at all, reproduced as
        // the sequence it would actually take. This builds the tree the
        // rejected manifest above would have produced -- by struct
        // literal, which is the only way to get one now -- and then does
        // the single ordinary user action that used to weaponise it.
        //
        // Before: `add_group` generated id 1 (the stale counter), and
        // `HashMap::insert` silently replaced the group already under it
        // -- destroying it, orphaning the pixel layer it held, and then
        // pushing id 1 into the *replacement's* own `children`, so the
        // new group listed itself as its own child. That is a cycle
        // built after deserialization, which the one-shot shape
        // validator never re-runs on, and every downward walk
        // (`paint_order` here, `aurora-app`'s `resolve_tile` in the real
        // app) then recurses forever.
        let target = super::LayerId::from_raw(1);
        let held = super::LayerId::from_raw(0);
        let mut layers = HashMap::new();
        layers.insert(target, group_entry("target", None, vec![held]));
        layers.insert(held, pixel_entry("held", Some(target)));
        let mut stale = IdGenerator::new();
        let _ = stale.next_id();
        let mut tree = LayerTree {
            ids: stale,
            layers,
            roots: vec![target],
        };

        match tree.add_group("new", None) {
            Err(DocError::LayerIdCollision(id)) => assert_eq!(id, target),
            other => unreachable!("expected LayerIdCollision, got {other:?}"),
        }

        // Nothing lost, nothing corrupted: the refused insert leaves the
        // tree exactly as it was.
        assert!(tree.contains(target), "the existing group must survive");
        assert!(tree.contains(held), "its child must not be orphaned");
        assert_eq!(tree.children(target), Some([held].as_slice()));
        assert_eq!(tree.roots(), &[target]);
        assert_eq!(
            tree.len(),
            2,
            "a refused insert must not add a layer either"
        );
        assert_eq!(
            tree.paint_order(),
            vec![held],
            "the pixel layer must still reach the composite"
        );
    }

    // --- names with no entry behind them ------------------------------
    //
    // The first round of this validation deliberately *skipped* an id
    // named by `roots` or by a group's `children` with no entry of its
    // own, reasoning that every traversal here already tolerates one. The
    // tests below are why that is now a rejection instead: the skip left
    // the named-but-absent id invisible to `validate_id_allocator`, which
    // only ever looked at ids actually present.

    #[test]
    fn a_manifest_naming_a_layer_it_does_not_carry_is_rejected() {
        // The exploit, at the door. Shape-wise this passes every other
        // rule: one group at the root, its recorded parent correct,
        // nothing orphaned (`seen.len() == layers.len() == 1`). And the
        // counter is exactly where a real generator would sit for the
        // ids *present* -- one past the highest, so
        // `validate_id_allocator` has nothing to say either. The only
        // defect is that the group names a child, id 1, that the file
        // does not carry.
        let group = super::LayerId::from_raw(0);
        let ghost = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(group, group_entry("group", None, vec![ghost]));
        let roots = vec![group];

        let ids = ids_for(&layers);
        assert_eq!(
            ids.peek_next(),
            1,
            "the counter is one past the highest id present -- the whole problem"
        );
        if let Err(err) = super::validate_id_allocator(&ids, &layers) {
            unreachable!("the allocator check cannot see a named-but-absent id: {err:?}");
        }

        match super::validate_shape(&layers, &roots, None, 1) {
            Err(DocError::DanglingLayerReference(id)) => assert_eq!(id, ghost),
            other => unreachable!("expected DanglingLayerReference, got {other:?}"),
        }

        // And refused at the door, through real bytes. (The reason is
        // pinned against the validator above rather than this message,
        // for the same reason every neighbouring test does it that way:
        // postcard flattens a `TryFrom` error to one opaque string.)
        let repr = TreeReprForTest { ids, layers, roots };
        assert!(
            decode_tree(&repr).is_err(),
            "a manifest naming a layer it does not carry must be refused"
        );
    }

    #[test]
    fn a_manifest_whose_roots_name_a_layer_it_does_not_carry_is_rejected() {
        // The same defect in the other sibling list. `roots` is walked by
        // its own loop, so it needs its own coverage.
        let present = super::LayerId::from_raw(0);
        let ghost = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(present, pixel_entry("present", None));
        let roots = vec![present, ghost];

        match super::validate_shape(&layers, &roots, None, 1) {
            Err(DocError::DanglingLayerReference(id)) => assert_eq!(id, ghost),
            other => unreachable!("expected DanglingLayerReference, got {other:?}"),
        }
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots,
        };
        assert!(decode_tree(&repr).is_err());
    }

    #[test]
    fn the_dangling_reference_a_manifest_names_cannot_be_handed_out_by_the_id_counter() {
        // What refusing the file above actually prevents, spelled out as
        // the sequence a user would perform: open a `.aur` someone sent,
        // click "New Layer" once, save, reopen. This builds the tree the
        // rejected manifest *would* have produced -- by struct literal,
        // which is the only way to get one now -- and shows that the very
        // next ordinary add lands on the named-but-absent id and leaves
        // the document unopenable by Aurora itself.
        //
        // Note what does *not* save this: the counter is not stale (it is
        // correctly one past every id present) and `insert`'s collision
        // guard does not fire (nothing is under id 1), so both of the
        // previous round's defences pass the add through. The defect had
        // to be refused at the manifest, which is what
        // `a_manifest_naming_a_layer_it_does_not_carry_is_rejected`
        // covers.
        let group = super::LayerId::from_raw(0);
        let ghost = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(group, group_entry("group", None, vec![ghost]));
        let mut tree = LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![group],
        };

        let fresh = match tree.add_group("new", None) {
            Ok(id) => id,
            Err(err) => unreachable!("nothing here refuses the add: {err:?}"),
        };
        assert_eq!(
            fresh, ghost,
            "the counter hands out exactly the id the group already names"
        );
        // Simultaneously a root recording no parent, and a child of
        // `group` -- the two-parent shape `validate_shape` forbids.
        assert_eq!(tree.roots(), &[fresh, group]);
        assert_eq!(tree.children(group), Some([ghost].as_slice()));
        assert_eq!(tree.parent(ghost), None);

        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            postcard::from_bytes::<LayerTree>(&bytes).is_err(),
            "Aurora would no longer be able to open its own save"
        );
    }

    #[test]
    fn every_tree_this_type_can_build_still_decodes_after_dangling_ids_became_an_error() {
        // The direction that must keep working. A dangling reference is
        // now refused on the reasoning that no tree this project *writes*
        // can contain one, so exercise the paths that write sibling
        // lists -- add, remove (which detaches a whole subtree), reparent,
        // and restore (undo of that remove) -- and require the result to
        // round-trip.
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
        let sibling = match tree.add_pixel_layer("sibling", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.reparent(leaf, Some(outer), 0) {
            unreachable!("{err:?}");
        }
        let removed = match tree.remove_capturing(inner) {
            Ok(removed) => removed,
            Err(err) => unreachable!("{err:?}"),
        };
        let re_encode = |tree: &LayerTree| match postcard::to_allocvec(tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        // After the remove: `outer`'s children no longer name `inner`.
        if let Err(err) = postcard::from_bytes::<LayerTree>(&re_encode(&tree)) {
            unreachable!("a tree with a subtree removed must still decode: {err}");
        }
        if let Err(err) = tree.restore(removed) {
            unreachable!("{err:?}");
        }
        if let Err(err) = postcard::from_bytes::<LayerTree>(&re_encode(&tree)) {
            unreachable!("a tree with a subtree restored must still decode: {err}");
        }
        assert_eq!(tree.roots(), &[sibling, outer]);
    }

    /// The root leads `RemovedSubtree::entries`, and something outside
    /// this file depends on it.
    ///
    /// `capture_subtree` documents its visit order as "root first, then
    /// each child's own subtree in stored order", and
    /// `History::describe`'s `Restore` arm now searches only the first
    /// `MAX_ROOT_SEARCH_ENTRIES` entries for the root's recorded name —
    /// so a capture that stopped leading with the root would silently
    /// turn every deep subtree's History-panel description into the
    /// `"layer"` placeholder. A review round proved the gap by mutation:
    /// reversing `entries` after the capture left all 199 tests in this
    /// crate green. This is the test that would have failed.
    ///
    /// Deliberately a *multi-entry* capture (a group, three children,
    /// one of them a nested group with its own child), because a
    /// one-element list satisfies "root first" no matter what the walk
    /// does. The whole subtree must be captured, too — a bound that
    /// holds only because entries went missing would be no bound at all.
    #[test]
    fn remove_capturing_puts_the_root_first_in_its_captured_entries() {
        let mut tree = LayerTree::new();
        let root = match tree.add_group("root", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let nested = match tree.add_group("nested", Some(root)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut expected = vec![root, nested];
        for (name, parent) in [
            ("deep", nested),
            ("first", root),
            ("second", root),
            ("third", root),
        ] {
            match tree.add_pixel_layer(name, bounds(), Some(parent)) {
                Ok(id) => expected.push(id),
                Err(err) => unreachable!("{err:?}"),
            }
        }

        let removed = match tree.remove_capturing(root) {
            Ok(removed) => removed,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some((first, _)) = removed.entries.first() else {
            unreachable!(
                "a captured subtree is never empty: {}",
                removed.entries.len()
            );
        };
        assert_eq!(
            *first, removed.root,
            "the root must lead the captured entries -- `History::describe` searches only \
             the first MAX_ROOT_SEARCH_ENTRIES of them for it"
        );

        // `LayerId` is `Eq` but not `Ord`, so compare the raw values --
        // this is about *which* layers were captured, not their order
        // (the order assertion above is the one that matters).
        let mut captured: Vec<_> = removed.entries.iter().map(|(id, _)| id.to_raw()).collect();
        captured.sort_unstable();
        let mut expected: Vec<_> = expected.iter().map(|id| id.to_raw()).collect();
        expected.sort_unstable();
        assert_eq!(
            captured, expected,
            "the whole subtree must be captured, not merely a prefix of it"
        );
    }

    // --- restore: cross-references, in both directions -----------------

    #[test]
    fn restoring_a_subtree_a_live_group_already_names_is_refused() {
        // The mirror of
        // `restoring_a_subtree_whose_child_names_a_live_layer_is_refused`
        // below: there, the *incoming* group named a live id; here, a
        // *live* group already names an *incoming* one. Same two-parent
        // shape after the merge, opposite direction, so it needs its own
        // check -- one shared implementation called both ways rather than
        // two hand-written ones that have to be kept in step.
        //
        // Reaching it needs a live tree that already carries a dangling
        // reference, which is exactly what `validate_shape` now refuses
        // at the door; built here by struct literal, the only way left.
        let host = super::LayerId::from_raw(0);
        let ghost = super::LayerId::from_raw(1);
        let mut layers = HashMap::new();
        layers.insert(host, group_entry("host", None, vec![ghost]));
        let mut tree = LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![host],
        };

        let removed = super::RemovedSubtree {
            root: ghost,
            parent: None,
            index: 0,
            entries: vec![(ghost, pixel_entry("ghost", None))],
        };
        match tree.restore(removed) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, ghost),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
        // Refused before the first mutation.
        assert!(!tree.contains(ghost), "a refused restore must add nothing");
        assert_eq!(tree.roots(), &[host]);
        assert_eq!(tree.len(), 1);
    }

    // --- opacity ranges, not shape ------------------------------------

    #[test]
    fn a_manifest_carrying_a_nonsense_opacity_is_rejected() {
        // `opacity`/`fill_opacity` are plain `f32`s on the wire, and
        // `set_opacity`'s `0.0..=1.0` guard only covers live edits -- so
        // until this check a crafted file handed the compositor whatever
        // it liked. Not a crash like the shape defects above; a rendering
        // -correctness one, the same "trust a number from an untrusted
        // file" class.
        for bad in [f32::NAN, -1.0, 1.0e38, f32::INFINITY] {
            let id = super::LayerId::from_raw(0);
            let mut entry = pixel_entry("a", None);
            entry.opacity = bad;
            let mut layers = HashMap::new();
            layers.insert(id, entry);
            match super::validate_opacities(&layers) {
                Err(DocError::OpacityOutOfRange(_)) => {}
                other => unreachable!("expected OpacityOutOfRange for {bad}, got {other:?}"),
            }
            let repr = TreeReprForTest {
                ids: ids_for(&layers),
                layers,
                roots: vec![id],
            };
            assert!(
                decode_tree(&repr).is_err(),
                "a manifest carrying opacity {bad} must be refused"
            );
        }
    }

    #[test]
    fn a_manifest_carrying_a_nonsense_fill_opacity_is_rejected() {
        // The second field, which has its own slider and its own defect.
        let id = super::LayerId::from_raw(0);
        let mut entry = pixel_entry("a", None);
        entry.fill_opacity = f32::NAN;
        let mut layers = HashMap::new();
        layers.insert(id, entry);
        match super::validate_opacities(&layers) {
            Err(DocError::OpacityOutOfRange(value)) => assert!(value.is_nan()),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
        let repr = TreeReprForTest {
            ids: ids_for(&layers),
            layers,
            roots: vec![id],
        };
        assert!(decode_tree(&repr).is_err());
    }

    #[test]
    fn the_opacity_range_boundaries_are_both_accepted() {
        // The direction that must keep working: 0.0 and 1.0 are ordinary
        // values a real document carries (a fully hidden layer, and every
        // freshly created one), so an exclusive bound here would refuse
        // genuine files.
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_opacity(a, 0.0) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_fill_opacity(b, 1.0) {
            unreachable!("{err:?}");
        }
        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = postcard::from_bytes::<LayerTree>(&bytes) {
            unreachable!("0.0 and 1.0 are ordinary opacities: {err}");
        }
    }

    #[test]
    fn restoring_a_subtree_carrying_a_nonsense_opacity_is_refused() {
        // The splice path is held to the same bar as the manifest path,
        // through the same validator -- a crafted history journal's own
        // `Restore` op is the second way an untrusted `f32` reaches a
        // live tree.
        let mut tree = LayerTree::new();
        let ghost = super::LayerId::from_raw(7);
        let mut entry = pixel_entry("ghost", None);
        entry.opacity = f32::NAN;
        let removed = super::RemovedSubtree {
            root: ghost,
            parent: None,
            index: 0,
            entries: vec![(ghost, entry)],
        };
        match tree.restore(removed) {
            Err(DocError::OpacityOutOfRange(value)) => assert!(value.is_nan()),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
        assert!(!tree.contains(ghost), "a refused restore must add nothing");
        assert_eq!(tree.len(), 0);
    }

    // --- property ranges reaching `validate` itself --------------------
    //
    // `validate` used to run only `validate_shape` and
    // `validate_id_allocator`, while `try_from` additionally ran
    // `validate_opacities` and every live edit path ran
    // `validate_origin`. `History::replay` has no gate but `validate`,
    // so a crafted journal could splice an out-of-range origin into a
    // live tree (see history.rs's own two replay tests for that end of
    // it). These check the validator directly.

    /// A rectangle whose origin sits one step past the document range on
    /// `x`, with an extent small enough that only the origin is at fault.
    fn out_of_range_origin() -> Rect {
        Rect {
            x: aurora_core::MAX_DOCUMENT_ORIGIN + 1,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    fn mask_at(bounds: Rect) -> LayerMask {
        LayerMask {
            bounds,
            enabled: true,
            inverted: false,
        }
    }

    /// A one-layer tree holding `entry`, with its id allocator already
    /// positioned past that id so `validate_id_allocator` cannot fire
    /// before the check under test.
    fn one_layer_tree(entry: super::LayerEntry) -> LayerTree {
        let id = super::LayerId::from_raw(0);
        let mut layers = HashMap::new();
        layers.insert(id, entry);
        LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![id],
        }
    }

    #[test]
    fn validate_rejects_a_pixel_layer_whose_origin_is_out_of_document_range() {
        let mut entry = pixel_entry("a", None);
        entry.kind = LayerKind::Pixel {
            bounds: out_of_range_origin(),
        };
        match one_layer_tree(entry).validate() {
            Err(DocError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_pixel_layers_mask_whose_origin_is_out_of_document_range() {
        // The layer's own bounds are fine; only the mask is out of range.
        let mut entry = pixel_entry("a", None);
        entry.mask = Some(mask_at(out_of_range_origin()));
        match one_layer_tree(entry).validate() {
            Err(DocError::LayerOriginOutOfRange { x, .. }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_group_layers_mask_whose_origin_is_out_of_document_range() {
        // A group has no `Pixel { bounds }` of its own, so this is only
        // caught if the mask check sits outside the `kind` match.
        let mut entry = group_entry("g", None, Vec::new());
        entry.mask = Some(mask_at(Rect {
            x: 0,
            y: -aurora_core::MAX_DOCUMENT_ORIGIN - 1,
            width: 10,
            height: 10,
        }));
        match one_layer_tree(entry).validate() {
            Err(DocError::LayerOriginOutOfRange { x, y, .. }) => {
                assert_eq!(x, 0);
                assert_eq!(y, -aurora_core::MAX_DOCUMENT_ORIGIN - 1);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    // The exact literal set two lines below, returned unchanged in the
    // error with no arithmetic in between -- the same reasoning
    // `set_opacity_updates_and_rejects_out_of_range` above documents.
    #[allow(clippy::float_cmp)]
    fn validate_rejects_an_opacity_outside_the_allowed_range() {
        // Contract completeness rather than a newly closed exploit --
        // both journal doors for an opacity were already guarded (the
        // setters range-check, and `restore` runs `validate_opacities`
        // on the incoming subtree). What was missing is that `validate`
        // itself did not hold to the bar `try_from` and `restore` do.
        let mut entry = pixel_entry("a", None);
        entry.fill_opacity = 1.5;
        match one_layer_tree(entry).validate() {
            Err(DocError::OpacityOutOfRange(value)) => assert_eq!(value, 1.5),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_origins_exactly_on_the_document_range_boundary() {
        // The direction that must keep working: `MAX_DOCUMENT_ORIGIN` is
        // inclusive on both axes and both signs, so a legitimate
        // far-corner document must still validate. Bounds and mask are
        // pinned to opposite corners so one fixture covers all four
        // comparisons.
        let max = aurora_core::MAX_DOCUMENT_ORIGIN;
        let mut entry = pixel_entry("a", None);
        entry.kind = LayerKind::Pixel {
            bounds: Rect {
                x: max,
                y: max,
                width: 10,
                height: 10,
            },
        };
        entry.mask = Some(mask_at(Rect {
            x: -max,
            y: -max,
            width: 10,
            height: 10,
        }));
        if let Err(err) = one_layer_tree(entry).validate() {
            unreachable!("the boundary origins are legitimate: {err:?}");
        }
    }

    #[test]
    fn the_out_of_range_origin_reported_is_the_lowest_numbered_layers() {
        // Same discipline as
        // `the_orphan_reported_is_the_lowest_numbered_one_not_whichever_hashmap_yields_first`:
        // with two offenders, which one is named must not depend on
        // `HashMap`'s per-process iteration order, or the same file
        // yields a different error message on every run. The offenders
        // carry *different* x values so the reported one is identifiable.
        let low = super::LayerId::from_raw(1);
        let high = super::LayerId::from_raw(2);
        let out_of_range_at = |x: i64| Rect {
            x,
            y: 0,
            width: 10,
            height: 10,
        };
        // A *fresh* map each iteration, not one map walked 32 times:
        // `HashMap`'s `RandomState` is seeded per instance, so re-walking
        // a single map yields the same order every time and would prove
        // nothing. Rebuilding reseeds it, so 32 rounds make agreeing by
        // luck vanishingly unlikely.
        for _ in 0..32 {
            let mut low_entry = pixel_entry("low", None);
            low_entry.kind = LayerKind::Pixel {
                bounds: out_of_range_at(aurora_core::MAX_DOCUMENT_ORIGIN + 1),
            };
            let mut high_entry = pixel_entry("high", None);
            high_entry.kind = LayerKind::Pixel {
                bounds: out_of_range_at(aurora_core::MAX_DOCUMENT_ORIGIN + 2),
            };
            let mut layers = HashMap::new();
            layers.insert(high, high_entry);
            layers.insert(low, low_entry);
            match super::validate_origins(&layers) {
                Err(DocError::LayerOriginOutOfRange { x, .. }) => {
                    assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
                }
                other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
            }
        }
    }

    #[test]
    // The exact literals set below, reported unchanged with no
    // arithmetic in between.
    #[allow(clippy::float_cmp)]
    fn the_out_of_range_opacity_reported_is_the_lowest_numbered_layers() {
        // The same nondeterminism `validate_origins` had, which this
        // check has had since it landed: it reported whichever violating
        // entry the `HashMap` happened to yield first.
        let low = super::LayerId::from_raw(1);
        let high = super::LayerId::from_raw(2);
        // Rebuilt each round for the reseeding reason the origin test
        // above spells out.
        for _ in 0..32 {
            let mut low_entry = pixel_entry("low", None);
            low_entry.opacity = 1.5;
            let mut high_entry = pixel_entry("high", None);
            high_entry.opacity = 2.5;
            let mut layers = HashMap::new();
            layers.insert(high, high_entry);
            layers.insert(low, low_entry);
            match super::validate_opacities(&layers) {
                Err(DocError::OpacityOutOfRange(value)) => assert_eq!(value, 1.5),
                other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
            }
        }
    }

    #[test]
    // As above: the literal is reported unchanged.
    #[allow(clippy::float_cmp)]
    fn one_layers_own_opacity_is_reported_ahead_of_its_fill_opacity() {
        // Pins the documented within-entry order, so the "which value"
        // half of the determinism guarantee is covered too -- `opacity`
        // and `fill_opacity` are both out of range on the same entry.
        let id = super::LayerId::from_raw(0);
        let mut entry = pixel_entry("a", None);
        entry.opacity = 1.5;
        entry.fill_opacity = 2.5;
        let mut layers = HashMap::new();
        layers.insert(id, entry);
        match super::validate_opacities(&layers) {
            Err(DocError::OpacityOutOfRange(value)) => assert_eq!(value, 1.5),
            other => unreachable!("expected OpacityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn one_layers_own_bounds_are_reported_ahead_of_its_masks() {
        // The origin counterpart to the test above: both rectangles on
        // one entry are out of range, and the layer's own `bounds` is
        // the documented answer.
        let id = super::LayerId::from_raw(0);
        let mut entry = pixel_entry("a", None);
        entry.kind = LayerKind::Pixel {
            bounds: Rect {
                x: aurora_core::MAX_DOCUMENT_ORIGIN + 1,
                y: 0,
                width: 10,
                height: 10,
            },
        };
        entry.mask = Some(mask_at(Rect {
            x: aurora_core::MAX_DOCUMENT_ORIGIN + 2,
            y: 0,
            width: 10,
            height: 10,
        }));
        let mut layers = HashMap::new();
        layers.insert(id, entry);
        match super::validate_origins(&layers) {
            Err(DocError::LayerOriginOutOfRange { x, .. }) => {
                assert_eq!(x, aurora_core::MAX_DOCUMENT_ORIGIN + 1);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    // --- reparent's own stale sibling list ----------------------------

    #[test]
    fn reparenting_a_layer_its_recorded_parent_does_not_list_errors_rather_than_moving_it() {
        // The counterpart to
        // `removing_a_layer_its_recorded_parent_does_not_list_errors_rather_than_aborting`.
        // `reparent` used to `retain` the id out of the old sibling list,
        // which is a silent no-op when it is not there -- and then went
        // ahead and attached it to the new parent anyway. On a tree where
        // the old parent *did* still list it under some other name, that
        // manufactures the "same layer under two parents" shape
        // `validate_shape` exists to forbid, through the public API.
        let victim = super::LayerId::from_raw(0);
        let group = super::LayerId::from_raw(1);
        let destination = super::LayerId::from_raw(2);
        let mut layers = HashMap::new();
        layers.insert(victim, pixel_entry("victim", Some(group)));
        // A real group -- it just does not list `victim` as a child.
        layers.insert(group, group_entry("group", None, Vec::new()));
        layers.insert(destination, group_entry("destination", None, Vec::new()));
        let mut tree = LayerTree {
            ids: ids_for(&layers),
            layers,
            roots: vec![group, destination],
        };
        match tree.reparent(victim, Some(destination), 0) {
            Err(DocError::InconsistentLayerParent(id)) => assert_eq!(id, victim),
            other => unreachable!("expected InconsistentLayerParent, got {other:?}"),
        }
        // Refused before the first mutation, so nothing moved anywhere.
        assert_eq!(tree.parent(victim), Some(group));
        assert_eq!(tree.children(group), Some([].as_slice()));
        assert_eq!(tree.children(destination), Some([].as_slice()));
        assert_eq!(tree.roots(), &[group, destination]);
    }

    // --- restore: the live tree, not just the incoming subtree --------

    #[test]
    fn restoring_a_subtree_whose_child_names_a_live_layer_is_refused() {
        // The incoming subtree is coherent read on its own -- the child
        // id simply names nothing inside it, which every traversal here
        // treats as a harmless dangling reference. It stops being
        // harmless the instant the two maps are merged: the live layer
        // it names is then reachable from its real parent *and* from the
        // spliced-in group.
        let mut tree = LayerTree::new();
        let live = match tree.add_pixel_layer("live", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let thief = super::LayerId::from_raw(9);
        let removed = super::RemovedSubtree {
            root: thief,
            parent: None,
            index: 0,
            entries: vec![(thief, group_entry("thief", None, vec![live]))],
        };
        match tree.restore(removed) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, live),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
        assert_eq!(tree.roots(), &[live]);
        assert!(!tree.contains(thief), "a refused restore must add nothing");
    }

    #[test]
    fn restoring_a_subtree_carrying_an_id_the_live_tree_already_holds_is_refused() {
        // The third `MalformedRemovedSubtree` branch (the other two --
        // an id carried twice, and a missing declared root -- are
        // covered in `history.rs`). Without it, the splice would replace
        // a live layer wholesale.
        let mut tree = LayerTree::new();
        let live = match tree.add_pixel_layer("live", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let removed = super::RemovedSubtree {
            root: live,
            parent: None,
            index: 0,
            entries: vec![(live, pixel_entry("impostor", None))],
        };
        match tree.restore(removed) {
            Err(DocError::MalformedRemovedSubtree(id)) => assert_eq!(id, live),
            other => unreachable!("expected MalformedRemovedSubtree, got {other:?}"),
        }
        assert_eq!(tree.name(live), Some("live"), "the live layer must survive");
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn restoring_a_subtree_advances_the_id_generator_past_every_id_it_brings_back() {
        // A subtree can carry ids this tree never allocated -- that is
        // exactly the shape `History::replay` produces, rebuilding from
        // `LayerTree::new()` (counter at 0) while every restored layer
        // keeps its original id. Without this, the next `add_pixel_layer`
        // hands out an id a restored layer already holds.
        let mut tree = LayerTree::new();
        let high = super::LayerId::from_raw(500);
        let removed = super::RemovedSubtree {
            root: high,
            parent: None,
            index: 0,
            entries: vec![(high, pixel_entry("high", None))],
        };
        if let Err(err) = tree.restore(removed) {
            unreachable!("an honest subtree must restore: {err:?}");
        }
        let next = match tree.add_pixel_layer("next", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(next, high, "a fresh id must not alias a restored one");
        assert!(next.to_raw() > high.to_raw());
        if let Err(err) = tree.validate() {
            unreachable!("the resulting tree must pass its own validator: {err:?}");
        }
    }

    /// A `RemovedSubtree` holding a chain of `depth` fresh groups, ids
    /// running from `first_raw`, with `parent` above the first —
    /// `restoring_a_subtree_counts_depth_from_where_it_is_spliced_in`'s
    /// fixture.
    fn spliced_chain(
        first_raw: u64,
        depth: u64,
        parent: Option<super::LayerId>,
    ) -> super::RemovedSubtree {
        let mut entries = Vec::new();
        for level in 0..depth {
            let id = super::LayerId::from_raw(first_raw + level);
            let children = if level + 1 < depth {
                vec![super::LayerId::from_raw(first_raw + level + 1)]
            } else {
                Vec::new()
            };
            let above = if level == 0 {
                parent
            } else {
                Some(super::LayerId::from_raw(first_raw + level - 1))
            };
            entries.push((id, group_entry("spliced", above, children)));
        }
        super::RemovedSubtree {
            root: super::LayerId::from_raw(first_raw),
            parent,
            index: 0,
            entries,
        }
    }

    #[test]
    fn restoring_a_subtree_counts_depth_from_where_it_is_spliced_in() {
        // `restore` used to start the depth budget at 1 for the incoming
        // subtree regardless of how deep the live `parent` already sat,
        // so two individually-legal splices could stack past
        // `MAX_LAYER_TREE_DEPTH` between them.
        let mut tree = LayerTree::new();
        let mut deepest: Option<super::LayerId> = None;
        for level in 0..250 {
            deepest = match tree.add_group(format!("g{level}"), deepest) {
                Ok(id) => Some(id),
                Err(err) => unreachable!("{err:?}"),
            };
        }
        // 250 groups, so `deepest` sits at depth 250.

        // 250 + 7 = 257, one past the limit.
        match tree.restore(spliced_chain(1_000, 7, deepest)) {
            Err(DocError::LayerTreeTooDeep { depth, max }) => {
                assert_eq!(depth, 257);
                assert_eq!(max, super::MAX_LAYER_TREE_DEPTH);
            }
            other => unreachable!("expected LayerTreeTooDeep, got {other:?}"),
        }

        // 250 + 6 = 256, exactly at it: still allowed.
        if let Err(err) = tree.restore(spliced_chain(2_000, 6, deepest)) {
            unreachable!("a splice landing exactly at the limit must succeed: {err:?}");
        }
    }

    // --- the wire format did not move --------------------------------

    /// A single-layer tree, encoded by the build immediately *before*
    /// this round's validation changes. One map entry, so the bytes are
    /// fully deterministic -- see
    /// `encoding_a_two_layer_tree_still_matches_one_of_the_pre_change_orderings`
    /// for why more than one entry is not.
    const GOLDEN_ONE_LAYER: &[u8] = &[
        1, 1, 0, 4, 79, 110, 108, 121, 0, 0, 0, 0, 10, 10, 0, 0, 128, 63, 0, 0, 128, 63, 0, 1, 0,
        0, 0, 0, 1, 0,
    ];

    /// The same fixed two-layer tree, in the two orderings `HashMap` can
    /// serialize its `layers` map in.
    const GOLDEN_TWO_LAYER_GROUP_FIRST: &[u8] = &[
        2, 2, 0, 5, 71, 114, 111, 117, 112, 0, 1, 1, 1, 0, 0, 128, 63, 0, 0, 128, 63, 0, 1, 0, 0,
        0, 0, 1, 5, 73, 110, 110, 101, 114, 1, 0, 0, 0, 0, 10, 10, 0, 0, 128, 63, 0, 0, 128, 63, 0,
        1, 0, 0, 0, 0, 1, 0,
    ];
    const GOLDEN_TWO_LAYER_INNER_FIRST: &[u8] = &[
        2, 2, 1, 5, 73, 110, 110, 101, 114, 1, 0, 0, 0, 0, 10, 10, 0, 0, 128, 63, 0, 0, 128, 63, 0,
        1, 0, 0, 0, 0, 0, 5, 71, 114, 111, 117, 112, 0, 1, 1, 1, 0, 0, 128, 63, 0, 0, 128, 63, 0,
        1, 0, 0, 0, 0, 1, 0,
    ];

    #[test]
    fn encoding_a_single_layer_tree_matches_the_pre_change_golden_bytes() {
        // Byte-for-byte against a literal captured from the build before
        // this round -- a stronger claim than "it still round-trips",
        // which would stay green even if the encoding had shifted
        // wholesale (ADR 0009's backward-compatibility policy is about
        // files written by *older* builds).
        let mut tree = LayerTree::new();
        if let Err(err) = tree.add_pixel_layer("Only", bounds(), None) {
            unreachable!("{err:?}");
        }
        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(bytes, GOLDEN_ONE_LAYER);
    }

    #[test]
    fn encoding_a_two_layer_tree_still_matches_one_of_the_pre_change_orderings() {
        // `layers` is a `HashMap`, and `postcard` writes a map in
        // iteration order -- which `RandomState` reshuffles per process.
        // So a two-entry tree has two legal encodings, both captured
        // pre-change; asserting a single literal here would be a flaky
        // test, not a stricter one.
        let mut tree = LayerTree::new();
        let group = match tree.add_group("Group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.add_pixel_layer("Inner", bounds(), Some(group)) {
            unreachable!("{err:?}");
        }
        let bytes = match postcard::to_allocvec(&tree) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            bytes == GOLDEN_TWO_LAYER_GROUP_FIRST || bytes == GOLDEN_TWO_LAYER_INNER_FIRST,
            "the wire format moved: {bytes:?}"
        );
    }

    #[test]
    fn bytes_written_by_the_pre_change_build_still_decode() {
        // The other direction, and the one that actually matters to a
        // user: files this project already wrote must still open.
        for golden in [GOLDEN_TWO_LAYER_GROUP_FIRST, GOLDEN_TWO_LAYER_INNER_FIRST] {
            let tree = match postcard::from_bytes::<LayerTree>(golden) {
                Ok(tree) => tree,
                Err(err) => unreachable!("a pre-change file must still load: {err:?}"),
            };
            assert_eq!(tree.len(), 2);
            let roots = tree.roots().to_vec();
            let [group] = roots.as_slice() else {
                unreachable!("expected exactly one root, got {roots:?}");
            };
            assert_eq!(tree.name(*group), Some("Group"));
            let children = match tree.children(*group) {
                Some(children) => children.to_vec(),
                None => unreachable!("the root must still be a group"),
            };
            let [inner] = children.as_slice() else {
                unreachable!("expected exactly one child, got {children:?}");
            };
            assert_eq!(tree.name(*inner), Some("Inner"));
            assert_eq!(tree.bounds(*inner), Some(bounds()));
            assert_eq!(tree.paint_order(), vec![*inner]);
        }

        let one = match postcard::from_bytes::<LayerTree>(GOLDEN_ONE_LAYER) {
            Ok(tree) => tree,
            Err(err) => unreachable!("a pre-change file must still load: {err:?}"),
        };
        assert_eq!(one.len(), 1);
        assert_eq!(one.roots().len(), 1);
    }

    // ---- `RemovedSubtree::surfaces` guard mirroring ----------------

    /// The raw layer ids where `RemovedSubtree::surfaces`'s two
    /// hand-copied guards can go wrong: both sides of
    /// `MASK_SURFACE_BIT` and of `MASK_SURFACE_BIT - 1` (the single id
    /// whose masked form is `aurora-app`'s reserved `u64::MAX`
    /// composite surface), an ordinary id with the mask bit set, and
    /// the very top of the range.
    fn surface_guard_boundary_ids() -> Vec<u64> {
        vec![
            0,
            1,
            5,
            crate::MASK_SURFACE_BIT - 2,
            crate::MASK_SURFACE_BIT - 1,
            crate::MASK_SURFACE_BIT,
            crate::MASK_SURFACE_BIT + 1,
            crate::MASK_SURFACE_BIT | 5,
            u64::MAX - 1,
            u64::MAX,
        ]
    }

    /// A one-entry `RemovedSubtree` rooted at `raw`, of either kind.
    fn boundary_subtree(raw: u64, pixel: bool) -> super::RemovedSubtree {
        let id = super::LayerId::from_raw(raw);
        let entry = if pixel {
            pixel_entry("boundary", None)
        } else {
            group_entry("boundary", None, Vec::new())
        };
        super::RemovedSubtree {
            root: id,
            parent: None,
            index: 0,
            entries: vec![(id, entry)],
        }
    }

    /// What `RemovedSubtree::surfaces` must return for `raw`, derived
    /// independently of both it and the two `LayerTree` methods it
    /// mirrors: phrased in terms of *which half of the id space* the id
    /// sits in and whether its masked form collides with the reserved
    /// composite surface, rather than by the numeric `< BIT` / `< BIT -
    /// 1` thresholds all three of those use. Equivalent, differently
    /// written — so a threshold edited in all three at once still fails
    /// here.
    fn expected_boundary_surfaces(raw: u64, pixel: bool) -> Vec<aurora_tile::SurfaceId> {
        let in_mask_half = (raw & crate::MASK_SURFACE_BIT) != 0;
        let masked_form = raw | crate::MASK_SURFACE_BIT;
        let masked_form_is_the_composite_sentinel = masked_form == u64::MAX;
        let mut out = Vec::new();
        if pixel && !in_mask_half {
            out.push(aurora_tile::SurfaceId::from_raw(raw));
        }
        if !in_mask_half && !masked_form_is_the_composite_sentinel {
            out.push(aurora_tile::SurfaceId::from_raw(masked_form));
        }
        out
    }

    #[test]
    /// The differential test the mirroring has been missing.
    ///
    /// `RemovedSubtree::surfaces` reproduces `LayerTree::surface_id`
    /// and `LayerTree::mask_surface_id`'s guards **by hand**, because a
    /// detached subtree has no tree to ask — and its own doc comment
    /// says what divergence would cost (freeing another layer's pixels,
    /// or sweeping the reserved composite surface). This splices the
    /// very same subtree into a real `LayerTree` and holds the detached
    /// answer against the live one, at every boundary id and for both
    /// layer kinds. `restore` deliberately does not run
    /// `validate_layer_id_range`, which is what makes even the
    /// out-of-range ids comparable here.
    fn removed_subtree_surfaces_mirrors_the_live_trees_own_two_derivations() {
        for raw in surface_guard_boundary_ids() {
            for pixel in [true, false] {
                let id = super::LayerId::from_raw(raw);
                let detached = boundary_subtree(raw, pixel).surfaces();

                let mut tree = LayerTree::new();
                if let Err(err) = tree.restore(boundary_subtree(raw, pixel)) {
                    unreachable!("a one-entry root subtree must restore: {err:?}");
                }
                let mut live: Vec<aurora_tile::SurfaceId> = Vec::new();
                live.extend(tree.surface_id(id));
                live.extend(tree.mask_surface_id(id));

                assert_eq!(
                    detached, live,
                    "detached and live answers diverged at raw {raw:#x} (pixel: {pixel})"
                );
            }
        }
    }

    #[test]
    /// The same boundary sweep against an independently written
    /// reference, so the two mirrored implementations cannot drift
    /// together and stay green.
    fn removed_subtree_surfaces_matches_an_independent_reference_at_every_boundary() {
        for raw in surface_guard_boundary_ids() {
            for pixel in [true, false] {
                assert_eq!(
                    boundary_subtree(raw, pixel).surfaces(),
                    expected_boundary_surfaces(raw, pixel),
                    "raw {raw:#x} (pixel: {pixel})"
                );
            }
        }
    }

    #[test]
    /// `aurora-app` reserves `SurfaceId::from_raw(u64::MAX)` for the
    /// composite. A `RemovedSubtree` carrying a crafted or foreign id
    /// must never make a sweep name it — that would delete the live
    /// composite out from under the app.
    fn removed_subtree_surfaces_never_emits_the_reserved_composite_surface() {
        let composite = aurora_tile::SurfaceId::from_raw(u64::MAX);
        for raw in surface_guard_boundary_ids() {
            for pixel in [true, false] {
                assert!(
                    !boundary_subtree(raw, pixel).surfaces().contains(&composite),
                    "raw {raw:#x} (pixel: {pixel}) emitted the reserved composite surface"
                );
            }
        }
    }
}
