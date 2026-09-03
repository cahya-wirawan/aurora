//! Real per-pixel mask coverage: where a [`crate::LayerMask`]'s own
//! grayscale data lives, and the one place its storage convention is
//! written down.
//!
//! [`crate::LayerMask`] itself stays a small, `Copy`-cheap struct of
//! bounds plus two toggles — the pixels do not live *in* it. They live
//! in the document's shared `aurora_tile::TileStore`, on their own
//! [`aurora_tile::SurfaceId`], exactly the way a pixel layer's own
//! content already does ([ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)'s
//! "one shared store, addressed by surface" decision). The surface a
//! given layer's mask is addressed under is
//! [`crate::LayerTree::mask_surface_id`].
//!
//! # The storage convention
//!
//! A mask tile is an ordinary RGBA `f16` tile read as an *opaque
//! grayscale image*:
//!
//! - Coverage `v` (`0.0..=1.0`) is stored as the texel `(v, v, v, 1.0)`.
//! - Coverage is read back from the **red** channel.
//! - **Alpha is the presence flag**, not coverage. `a == 0.0` means
//!   "this texel has never been painted", which reads back as coverage
//!   `1.0` — fully visible, today's unmasked default.
//!
//! That last rule is what makes real mask pixels a purely additive
//! change. `aurora_tile::TileStore::get` materializes an untouched tile
//! as `aurora_tile::Tile::blank()` — all zeros — so an unpainted mask
//! surface reads as coverage `1.0` everywhere, and a *half*-painted
//! mask tile still reads `1.0` across the half nobody painted. The
//! backward-compatible default is therefore per **texel**, not per tile
//! or per surface, and it needs no new `TileStore` API, no new
//! [`crate::LayerMask`] field, and no `.aur` format change.
//!
//! **Accepted trade-off, named rather than left implicit**: one grayscale
//! coverage value costs a full RGBA `f16` texel — 4x the bytes the data
//! itself needs — against a PRD that budgets 8 bytes/px and calls tile
//! compression mandatory. Reusing the existing store's own sparse
//! allocation and per-tile compression (rather than a dedicated
//! single-channel surface kind, which would need new `TileStore`/`.aur`
//! machinery) is the trade this round made, and it is why the
//! consequences list above can say "no new `TileStore` API." Revisit if
//! mask memory ever shows up as a real, measured cost.
//!
//! The writer ([`write_mask_coverage`]) and the reader
//! ([`read_mask_coverage`]) both live here, in one module, so the two
//! halves of that convention cannot drift apart.
//!
//! # The addressing convention — the other half, not just the value
//!
//! Everything above is the *value* half of the convention (what a texel
//! means once you have it). There is a second, equally load-bearing
//! half this module does not enforce and could not: a mask surface's
//! `aurora_tile::TileId` space is addressed **relative to the mask's
//! own [`crate::LayerMask::bounds`] origin**, not the document's origin
//! and not the layer's own `bounds` origin — the three coincide only
//! when a mask happens to sit at the same place as its layer, which is
//! not guaranteed. `write_mask_coverage` takes a bare `TileId` plus a
//! tile-local `(column, row)` and has no way to check which frame the
//! caller meant; both real callers today get this right by
//! construction, and for the same reason — each derives its tile
//! addressing from `mask.bounds` itself rather than from some other
//! frame. `aurora-app`'s `apply_mask` reads through the window it built
//! from `mask.bounds`'s own `(x, y)`; `aurora-io`'s `.aur`
//! writer/reader (0.71.0, `persisted_surfaces`) derives a mask
//! surface's whole persisted tile grid from `mask.bounds`'s own extent
//! at that same origin, which is what lets a mask sitting somewhere
//! other than its layer round-trip through a save at all. **A future
//! caller — the brush/tool UI in follow-on 1 below
//! — must convert a document-space paint stroke into `mask.bounds`-
//! relative tile coordinates before calling [`write_mask_coverage`], or
//! painted coverage will be shifted by exactly the offset between the
//! mask's origin and whatever frame the tool assumed.** This is exactly
//! the kind of drift this module's single-writer/single-reader design
//! prevents for the *value* convention — it cannot prevent it here,
//! because the origin lives on [`crate::LayerMask`], not in this
//! module, so the tool implementing follow-on 1 is responsible for
//! getting this right and should re-read this paragraph before wiring
//! coordinates through.
//!
//! **Nor is coverage checked against the mask's own grid.**
//! [`write_mask_coverage`] accepts any `TileId`, including one outside
//! the tile grid `mask.bounds` spans. Such a write lands in the store
//! and is then **silently dropped on every save**, with no error and no
//! log: `aurora-io`'s writer walks `0..tiles_x`/`0..tiles_y` of the
//! mask's own rectangle and never looks anywhere else. This is
//! unreachable today — nothing but tests calls the writer, and a mask's
//! `bounds` is immutable through the current API once
//! `LayerTree::add_mask` has accepted it — but it stops being
//! unreachable the moment a mask can be moved or resized, which is the
//! same "the origin/extent relationship is not enforced across time"
//! gap follow-on 3 below already tracks. It belongs to that item, not
//! to a new one.
//!
//! # Deliberately not built this round
//!
//! Three follow-ons are named rather than silently dropped, because
//! each is a task-sized piece of work in a different part of the stack.
//! (A fourth, `.aur` persistence of mask pixels, is **built** as of
//! 0.71.0: `aurora-io`'s own `persisted_surfaces` enumerates mask
//! surfaces alongside layer content surfaces, so painted coverage now
//! survives save/load — including on a group, and including a mask
//! whose `enabled` toggle is off. Nothing about the format had to
//! change, exactly as predicted above; only the code that walks
//! surfaces did.)
//!
//! 1. **A brush/tool UI for painting a mask.** Nothing in the app
//!    currently *writes* mask coverage — [`write_mask_coverage`] is the
//!    API a future mask-painting tool calls, and today only tests do.
//!    This is also why the persistence above, though real and tested,
//!    has never run end to end through the editor.
//! 2. **Mask-pixel undo/history.** `aurora_doc::History` records
//!    reversible operations plus dirtied tiles (§7.3.3); mask writes go
//!    through no history operation at all yet, so painting a mask would
//!    not be undoable.
//! 3. **Mask-surface lifecycle: still incomplete.** A mask surface id
//!    is *derived* from its layer's id, not allocated, and that has
//!    consequences the crate only partly handles. Three shapes, all
//!    harmless today (no mask coverage is ever painted yet) and all
//!    real the moment item 1 lands. The first is **fixed** as of
//!    0.81.0; the third has had only its document-discard half
//!    addressed (0.80.0); the second is untouched:
//!
//!    - **Remove a mask, add a new one to the same layer, and the old
//!      one's painted coverage comes back — fixed in 0.81.0.** The new
//!      mask resolves to the same
//!      [`crate::LayerTree::mask_surface_id`], and
//!      [`crate::LayerTree::remove_mask`] drops only the `LayerMask`
//!      struct — so the tiles under that surface used to survive into
//!      the replacement, which then opened wearing the deleted mask's
//!      pixels, shifted by the offset between the two masks' `bounds`
//!      origins (coverage is addressed relative to the mask's own
//!      origin — see "The addressing convention" above). `.aur`
//!      persistence (0.71.0) had widened this from session-scoped to
//!      durable: a mask surface's tiles round-trip through a save like
//!      any other, so the stale coverage could travel inside a shared
//!      file.
//!
//!      [`crate::History::add_mask`] now takes an
//!      `aurora_tile::TileStore` and calls
//!      [`forget_mask_coverage`] immediately after the tree accepts the
//!      new mask, so a fresh mask starts from unpainted coverage.
//!
//!      **The clear is at `add_mask` time, not `remove_mask` time, on
//!      purpose.** Removing a mask is undoable —
//!      `LayerOp::RestoreMask` puts the exact `LayerMask` back on the
//!      same derived surface — so freeing the tiles at removal would
//!      make Ctrl+Z restore a blank mask with the user's coverage
//!      already destroyed. That is the same rule the layer-removal
//!      bullet further down states, and `history.rs`'s own
//!      `undo_of_a_remove_mask_still_finds_its_painted_coverage`
//!      exists to keep it that way.
//!
//!      Two residuals, named rather than implied:
//!
//!      1. **[`crate::LayerTree::add_mask`] called directly still does
//!         not clear.** It holds no store handle, and keeping
//!         [`crate::LayerTree`] store-agnostic is deliberate; a caller
//!         that bypasses [`crate::History`] must call
//!         [`forget_mask_coverage`] itself. Same documented, accepted
//!         bypass shape as [`crate::forget_document_surfaces`]'s own
//!         "a removal that bypassed `History` entirely" gap.
//!      2. **Once a new mask is actually committed, the old one's
//!         coverage cannot be brought back by undo.** Undoing past the
//!         add restores the previous `LayerMask` struct exactly, but
//!         its pixels are gone: one derived surface can hold only one
//!         mask's tiles. Accepted and tested
//!         (`add_mask_makes_the_removed_masks_coverage_unrecoverable_by_undo`),
//!         not silently lost. Holding both would need a surface per
//!         mask *instance* — allocated rather than derived ids — which
//!         is a separate decision.
//!    - **Coverage written outside the mask's own grid is dropped on
//!      save.** [`write_mask_coverage`] does not check the `TileId` it
//!      is given against the grid `crate::LayerMask::bounds` spans, and
//!      `aurora-io`'s writer walks only that grid — so such a texel is
//!      lost silently, with no error and no log. Same category as the
//!      two above: the origin/extent relationship is not enforced
//!      across time. Harmless while `bounds` is immutable after
//!      `crate::LayerTree::add_mask`; real the moment a mask can be
//!      moved or resized, which is when this item is done anyway.
//!    - **Deleting a layer leaves its mask tiles in the store —
//!      deliberately, and now with a way out.** This is not specific to
//!      masks: a deleted layer's *own* pixel tiles are held exactly the
//!      same way, and masks merely double the number of surfaces
//!      involved. Two halves, and 0.80.0 changed only one of them.
//!
//!      *Within a live session, the tiles stay, on purpose.*
//!      [`crate::History`] undoes a remove by restoring the captured
//!      subtree under its original ids, which derive the very same
//!      surfaces — so freeing a removed layer's tiles at delete time
//!      would make Ctrl+Z restore a blank layer with the user's pixels
//!      already destroyed. That is strictly worse than holding them,
//!      and `history.rs`'s own
//!      `undo_of_a_remove_still_finds_the_removed_layers_painted_pixels`
//!      test exists to keep it that way.
//!
//!      *When the whole document is discarded, they can now be freed.*
//!      [`crate::forget_document_surfaces`] (0.80.0) takes a
//!      [`crate::LayerTree`] and a [`crate::History`] **by value** and
//!      sweeps every surface either can still name — live layers'
//!      content and mask surfaces, plus every subtree captured on the
//!      undo *or* redo stack, plus (since 0.80.1) every `Restore`
//!      entry in the crash-recovery journal — through
//!      `aurora_tile::TileStore::forget_surfaces`. Note it does not
//!      gate on a [`crate::LayerMask`] still being attached, which is
//!      what makes it reach the residue the first bullet above
//!      describes.
//!
//!      Three things it does **not** do, named rather than implied:
//!
//!      1. **Nothing in the app calls it.** `aurora_app::App`'s
//!         `open_file`/`open_aur_file` are the intended callers and are
//!         blocked on an ordering/aliasing problem —
//!         `aurora_io::read_aur` fills the store with the *new*
//!         document's tiles before the old one is dropped, and both
//!         documents' surface ids derive from `LayerId`s that restart
//!         at zero. See that function's own doc comment for the full
//!         account. So this is real, tested library code with no live
//!         behaviour change behind it yet.
//!      2. **A redo entry dropped mid-session still leaks — and this
//!         is the one leak path here that the shipped app really
//!         walks.** `History::push` (private) clears the redo stack on
//!         any new structural activity, and so does the *public*
//!         `History::clear_redo`, which `aurora_app::UndoOrder::record`
//!         calls on every committed edit so that a pixel edit and a
//!         structural edit invalidate each other's pending redo. The
//!         captured subtrees go with the cleared stack, and nothing can
//!         name their tiles afterwards. The 0.80.1 journal sweep
//!         recovers the subset that came from an *add* (whose `Restore`
//!         is journalled too); anything else on that stack is gone.
//!         Freeing at the clearing point needs a store handle neither
//!         `push` nor `clear_redo` has, which is a wider change than
//!         this round's scope.
//!      3. **A removal that bypassed `History` entirely leaks.**
//!         `LayerTree::remove` (as opposed to `remove_capturing`)
//!         discards the subtree rather than handing it back, so no
//!         `RemovedSubtree` reaches either stack or the journal and
//!         those surfaces are past even the sweep's reach. Mixing
//!         direct `LayerTree` calls with `History` is a discouraged but
//!         supported shape, so this is reachable by construction.

/// The bit that separates mask surfaces from layer-pixel surfaces in
/// the shared `aurora_tile::TileStore`'s single `SurfaceId` space.
///
/// The partition is a plain top-bit split, and it is exhaustive:
///
/// - **Layer pixel surfaces** occupy the bottom half. They are
///   `SurfaceId::from_raw(layer_id.to_raw())`
///   ([`crate::LayerTree::surface_id`]).
/// - **Mask surfaces** occupy the top half:
///   `layer_id.to_raw() | MASK_SURFACE_BIT`
///   ([`crate::LayerTree::mask_surface_id`]).
/// - **`aurora-app`'s reserved composite surface** is
///   `SurfaceId::from_raw(u64::MAX)`. It is in the top half too, which
///   is why [`crate::LayerTree::mask_surface_id`] refuses the single
///   layer id (`MASK_SURFACE_BIT - 1`) that would map onto it.
///
/// # What the partition actually rests on
///
/// **Every `LayerId` in a live tree has this bit clear.** That is the
/// whole load-bearing statement, and it is not self-evident — it is
/// enforced, in two places, because two different kinds of tree exist:
///
/// 1. **Trees this process builds.** `aurora_core::IdGenerator` starts
///    at `0` and hands ids out one at a time, monotonically, so a real
///    id would have to survive `2^63` allocations before the bit could
///    be set in one.
/// 2. **Trees deserialized from an untrusted `.aur` file** (a corrupted
///    one, or a crafted one). `LayerId` and `IdGenerator` are both
///    `Deserialize`, so point 1 says nothing at all here: the file
///    supplies the ids *and* the counter. `aurora_doc`'s own
///    `validate_layer_id_range` — run by both whole-tree gates,
///    `#[serde(try_from = "LayerTreeRepr")]` and
///    `LayerTree::validate` (the journal-replay path) — refuses any id
///    with this bit set, and any id counter positioned to hand one out
///    ([`crate::DocError::ReservedLayerIdBit`],
///    [`crate::DocError::ReservedLayerIdCounter`]).
///
/// **Half of that was missing until 0.70.1, and this comment used to
/// claim otherwise.** It said a mask surface collides with nothing "for
/// every id any document this process could build" — true of point 1,
/// false of point 2, and the gap was real and reachable: a manifest
/// holding layer `5` (with a mask) alongside layer
/// `5 | MASK_SURFACE_BIT` (a plain pixel layer) gave both the same
/// `SurfaceId`, so painting the second layer's pixels rewrote the
/// first layer's mask coverage and vice versa, through one tile-store
/// slot with two owners and no error anywhere. Nothing but
/// `validate_id_allocator`'s "counter is ahead of every id present"
/// check stood in the way, and a crafted counter of `u64::MAX`
/// satisfies that. [`crate::LayerTree::surface_id`] now carries the
/// mirror of [`crate::LayerTree::mask_surface_id`]'s own guard as well,
/// so the invariant holds at the type's boundary and not only at the
/// validation call site.
///
/// Given that, a mask surface collides with neither a layer's own pixel
/// surface (different half) nor the composite surface (explicitly
/// excluded), for every id any `LayerTree` this crate will hand out.
pub const MASK_SURFACE_BIT: u64 = 1 << 63;

/// Writes one texel of mask coverage at tile-local `(x, y)`.
///
/// `coverage` is clamped to `0.0..=1.0` and stored as `(v, v, v, 1.0)`,
/// per this module's own storage convention — the opaque alpha is the
/// "painted" flag, not coverage, so writing coverage `0.0` still stores
/// a fully *opaque* texel. The touched texel is marked dirty on the
/// tile, so a caller that later uploads or pages it out sees the write.
///
/// `surface` should come from [`crate::LayerTree::mask_surface_id`];
/// this function does not check that, because `aurora_tile` deals only
/// in opaque surface ids and has no way to tell one apart from a
/// layer's own.
///
/// **`tile`/`(column, row)` must already be in the mask's own frame**
/// — relative to [`crate::LayerMask::bounds`]'s own origin, not the
/// document's and not the layer's own `bounds`. See this module's own
/// "The addressing convention" section; this function cannot check it.
///
/// # Errors
///
/// Returns [`aurora_tile::TileError`] if the store cannot page the tile
/// in. Also returns [`aurora_tile::TileError::MalformedTile`] when the
/// requested texel is not inside the tile — either because `(x, y)` is
/// outside `0..aurora_tile::TILE` on an axis, or (unreachable for a
/// tile this store hands out) because the tile holds fewer samples than
/// one whole tile's worth. Both are the same statement from here: the
/// tile does not hold the texel that was asked for, and nothing is
/// written.
pub fn write_mask_coverage(
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    tile: aurora_tile::TileId,
    column: usize,
    row: usize,
    coverage: f32,
) -> Result<(), aurora_tile::TileError> {
    let side = aurora_tile::TILE as usize;
    let malformed = |samples: usize| aurora_tile::TileError::MalformedTile {
        surface,
        id: tile,
        samples,
        expected: aurora_tile::SAMPLES,
    };
    let value = half::f16::from_f32(coverage.clamp(0.0, 1.0));
    let one = half::f16::from_f32(1.0);

    let entry = store.get_mut(surface, tile)?;
    let samples = entry.texels().len();
    if column >= side || row >= side {
        return Err(malformed(samples));
    }
    // Both are below `TILE`, so both conversions always succeed --
    // done ahead of the write so a failure cannot leave a texel
    // written but unreported as dirty.
    let (Ok(dirty_x), Ok(dirty_y)) = (i64::try_from(column), i64::try_from(row)) else {
        return Err(malformed(samples));
    };
    let base = (row * side + column) * aurora_tile::CHANNELS;
    let Some(texel) = entry
        .texels_mut()
        .get_mut(base..base + aurora_tile::CHANNELS)
    else {
        return Err(malformed(samples));
    };
    let [r, g, b, a] = texel else {
        return Err(malformed(samples));
    };
    *r = value;
    *g = value;
    *b = value;
    *a = one;

    // Tile-local, exactly the one texel written.
    entry.mark_dirty(aurora_core::Rect {
        x: dirty_x,
        y: dirty_y,
        width: 1,
        height: 1,
    });
    Ok(())
}

/// Frees every tile stored under `id`'s mask surface, so a mask on that
/// layer starts from unpainted coverage.
///
/// Returns how many tiles were actually forgotten — `0` for a layer the
/// tree does not contain, and `0` for the structurally unreachable
/// reserved-id branch [`crate::LayerTree::mask_surface_id`] documents.
/// Both are honest no-ops rather than errors: there is no coverage to
/// free either way.
///
/// **This destroys pixels, and that is its whole purpose** — the same
/// warning `aurora_tile::TileStore::forget_surface` carries, which is
/// the call this delegates to. There is no way back from it.
///
/// # Why this takes `&LayerTree`, unlike `forget_document_surfaces`
///
/// [`crate::forget_document_surfaces`] deliberately takes its
/// [`crate::LayerTree`] and [`crate::History`] **by value**, so that
/// sweeping a document the user still has open is a compile error. That
/// reasoning does not transfer here, and inverting it is the point:
/// this is a *live-document* operation by design. Its one intended
/// caller — [`crate::History::add_mask`] — runs it on a tree that is
/// still in active use, immediately after a genuinely new
/// [`crate::LayerMask`] has been committed onto that very tree, to clear
/// whatever a previously removed mask left behind under the same
/// derived surface. A by-value signature would make that call
/// impossible to write.
///
/// The narrower blast radius is what makes that safe: this touches
/// exactly one layer's mask surface, never its pixel content and never
/// another layer's anything.
pub fn forget_mask_coverage(
    layers: &crate::LayerTree,
    store: &mut aurora_tile::TileStore,
    id: crate::LayerId,
) -> usize {
    let Some(surface) = layers.mask_surface_id(id) else {
        return 0;
    };
    store.forget_surface(surface)
}

/// Reads one texel's mask coverage, per this module's own storage
/// convention.
///
/// `texel` is one already-fetched RGBA sample — four `f16`s, in
/// channel order. Pure: it touches no store, so the caller decides how
/// the texel was obtained (`aurora-app` reads a whole tile-sized window
/// once and then calls this per texel).
///
/// Returns:
///
/// - `1.0` when alpha is zero — never painted, so fully visible, which
///   is exactly the pre-mask-pixels behaviour.
/// - the red channel, clamped to `0.0..=1.0`, otherwise — the painted
///   coverage.
/// - `1.0` for a `texel` that isn't four samples long, or whose alpha
///   or red channel is `NaN`. Every degenerate case **fails open**, on
///   purpose: a mask that cannot be read must not silently erase a
///   layer's content, which is the one failure this crate can't let a
///   user discover later.
///
/// The zero test is written as `abs() <= 0.0` rather than `== 0.0` so
/// that `-0.0` and any (impossible, but not structurally excluded)
/// negative alpha count as unpainted too, and so that it does not trip
/// this workspace's `clippy::float_cmp`.
#[must_use]
pub fn read_mask_coverage(texel: &[half::f16]) -> f32 {
    let [r, _, _, a] = texel else {
        return 1.0;
    };
    let alpha = a.to_f32();
    if alpha.is_nan() || alpha.abs() <= 0.0 {
        return 1.0;
    }
    let coverage = r.to_f32();
    if coverage.is_nan() {
        return 1.0;
    }
    coverage.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{MASK_SURFACE_BIT, read_mask_coverage, write_mask_coverage};

    fn real_tile_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(4) else {
            unreachable!("4 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("a freshly created tempdir must be usable: {err:?}"),
        };
        (dir, store)
    }

    /// Exact float equality expressed as bit equality.
    ///
    /// These round trips genuinely are exact -- a value written as
    /// `f16` and read back as `f32` is the same number, not merely a
    /// close one -- so an epsilon comparison would be the weaker
    /// assertion. Written this way rather than with `assert_eq!` on
    /// bare `f32`s because this workspace denies `clippy::float_cmp`.
    fn exactly(actual: f32, expected: f32) -> bool {
        actual.to_bits() == expected.to_bits()
    }

    fn texel_at(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
        tile: aurora_tile::TileId,
        x: usize,
        y: usize,
    ) -> Vec<half::f16> {
        let Ok(entry) = store.get(surface, tile) else {
            unreachable!("a real store must serve this tile");
        };
        let base = (y * aurora_tile::TILE as usize + x) * aurora_tile::CHANNELS;
        let Some(texel) = entry.texels().get(base..base + aurora_tile::CHANNELS) else {
            unreachable!("(x, y) constructed in range for a whole tile");
        };
        texel.to_vec()
    }

    #[test]
    // An untouched mask surface reads as fully visible -- the whole
    // point of the alpha-as-presence-flag convention.
    fn read_mask_coverage_on_a_never_painted_texel_is_fully_visible() {
        let blank = [half::f16::from_f32(0.0); aurora_tile::CHANNELS];
        assert!(exactly(read_mask_coverage(&blank), 1.0));
    }

    #[test]
    // A short or malformed slice fails open, never closed: a bad read
    // must not silently erase a layer.
    fn read_mask_coverage_on_a_malformed_texel_is_fully_visible() {
        let short = [half::f16::from_f32(0.0); 3];
        assert!(exactly(read_mask_coverage(&short), 1.0));
    }

    #[test]
    // Coverage 0.0 is stored *opaquely* -- it is a painted "hide this",
    // not the never-painted default, and the two must not be confused.
    fn write_mask_coverage_zero_reads_back_as_zero_not_as_the_unpainted_default() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x07 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        if let Err(err) = write_mask_coverage(&mut store, surface, tile, 3, 4, 0.0) {
            unreachable!("{err:?}");
        }
        let texel = texel_at(&mut store, surface, tile, 3, 4);
        assert!(exactly(read_mask_coverage(&texel), 0.0));
    }

    #[test]
    // The round trip every caller depends on: what was written is what
    // comes back, through a real store.
    fn write_mask_coverage_round_trips_through_a_real_store() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x01 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 2, y: 1 };
        for (x, y, coverage) in [(0, 0, 1.0_f32), (10, 20, 0.5), (255, 255, 0.25)] {
            if let Err(err) = write_mask_coverage(&mut store, surface, tile, x, y, coverage) {
                unreachable!("{err:?}");
            }
            let texel = texel_at(&mut store, surface, tile, x, y);
            // `f16` cannot hold every `f32`, so compare against what the
            // written value actually quantizes to.
            assert!(
                exactly(
                    read_mask_coverage(&texel),
                    half::f16::from_f32(coverage).to_f32()
                ),
                "coverage written at ({x}, {y}) must read back"
            );
        }
    }

    #[test]
    // Writing one texel must not disturb its neighbours -- an unwritten
    // neighbour still reads as the never-painted default.
    fn write_mask_coverage_leaves_its_neighbours_at_the_unpainted_default() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x03 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        if let Err(err) = write_mask_coverage(&mut store, surface, tile, 5, 5, 0.0) {
            unreachable!("{err:?}");
        }
        let neighbour = texel_at(&mut store, surface, tile, 6, 5);
        assert!(exactly(read_mask_coverage(&neighbour), 1.0));
    }

    #[test]
    // Out-of-range coverage is clamped rather than stored as-is or
    // refused.
    fn write_mask_coverage_clamps_out_of_range_coverage() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x04 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        for (x, coverage, expected) in [(0, 5.0_f32, 1.0_f32), (1, -5.0, 0.0)] {
            if let Err(err) = write_mask_coverage(&mut store, surface, tile, x, 0, coverage) {
                unreachable!("{err:?}");
            }
            let texel = texel_at(&mut store, surface, tile, x, 0);
            assert!(exactly(read_mask_coverage(&texel), expected));
        }
    }

    #[test]
    // A texel outside the tile is refused, not clamped into some other
    // pixel and not silently dropped.
    fn write_mask_coverage_refuses_a_texel_outside_the_tile() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x05 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        let side = aurora_tile::TILE as usize;
        assert!(write_mask_coverage(&mut store, surface, tile, side, 0, 1.0).is_err());
        assert!(write_mask_coverage(&mut store, surface, tile, 0, side, 1.0).is_err());
    }

    /// A layer tree holding `count` root pixel layers, with the ids in
    /// creation order.
    fn tree_of(count: usize) -> (crate::LayerTree, Vec<crate::LayerId>) {
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let mut tree = crate::LayerTree::new();
        let mut ids = Vec::new();
        for index in 0..count {
            let Ok(id) = tree.add_pixel_layer(format!("layer {index}"), bounds, None) else {
                unreachable!("an empty parent accepts a root layer");
            };
            ids.push(id);
        }
        (tree, ids)
    }

    #[test]
    // An id the tree does not hold has no mask surface to free, so this
    // is a plain no-op rather than an error or a panic -- the same
    // "return something honest instead of asserting" shape
    // `mask_surface_id` itself already takes.
    fn forget_mask_coverage_of_an_unknown_layer_is_zero() {
        let (_dir, mut store) = real_tile_store();
        let (tree, ids) = tree_of(1);
        let Some(known) = ids.first() else {
            unreachable!("one layer was created");
        };
        // Painted so a sweep that ignored its `id` argument entirely
        // would have something to wrongly free, and be caught below.
        let Some(surface) = tree.mask_surface_id(*known) else {
            unreachable!("a layer in the tree has a mask surface");
        };
        if let Err(err) = write_mask_coverage(&mut store, surface, tile_zero(), 1, 1, 0.5) {
            unreachable!("{err:?}");
        }

        let unknown = aurora_core::Id::from_raw(9_999);
        assert_eq!(
            super::forget_mask_coverage(&tree, &mut store, unknown),
            0,
            "an unknown layer has no coverage to free"
        );
        assert!(
            store.contains_tile(surface, tile_zero()),
            "and freeing nothing must not have touched a real layer's mask"
        );
    }

    #[test]
    // The blast radius: one layer's mask surface, and nothing else --
    // not its own pixel content, and not another layer's anything.
    fn forget_mask_coverage_frees_only_the_requested_layers_mask() {
        let (_dir, mut store) = real_tile_store();
        let (tree, ids) = tree_of(2);
        let [a, b] = ids.as_slice() else {
            unreachable!("two layers were created");
        };
        let (a, b) = (*a, *b);
        let surfaces = |id| {
            let (Some(content), Some(mask)) = (tree.surface_id(id), tree.mask_surface_id(id))
            else {
                unreachable!("a pixel layer in the tree has both surfaces");
            };
            (content, mask)
        };
        let (a_content, a_mask) = surfaces(a);
        let (b_content, b_mask) = surfaces(b);
        for surface in [a_content, a_mask, b_content, b_mask] {
            if let Err(err) = write_mask_coverage(&mut store, surface, tile_zero(), 2, 3, 0.25) {
                unreachable!("{err:?}");
            }
        }

        assert_eq!(super::forget_mask_coverage(&tree, &mut store, a), 1);
        assert!(!store.contains_tile(a_mask, tile_zero()), "a's mask freed");
        for (surface, what) in [
            (a_content, "a's own pixel content"),
            (b_mask, "b's mask"),
            (b_content, "b's pixel content"),
        ] {
            assert!(
                store.contains_tile(surface, tile_zero()),
                "{what} must be untouched"
            );
        }
    }

    fn tile_zero() -> aurora_tile::TileId {
        aurora_tile::TileId { x: 0, y: 0 }
    }

    #[test]
    // The write has to be visible to whatever uploads or pages the tile
    // out; a coverage write nobody is told about is a lost edit.
    fn write_mask_coverage_marks_the_tile_dirty() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0x06 | MASK_SURFACE_BIT);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        assert!(!store.is_dirty(surface, tile), "nothing written yet");
        if let Err(err) = write_mask_coverage(&mut store, surface, tile, 1, 2, 0.75) {
            unreachable!("{err:?}");
        }
        assert!(store.is_dirty(surface, tile));
    }
}
