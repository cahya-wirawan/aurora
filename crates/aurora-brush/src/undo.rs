//! A completed brush/eraser stroke's own undo/redo — PLAN.md's own
//! Undo/Redo bullet's pixel-edit half, closing the gap
//! `aurora_doc::History` structurally can't: a stroke is raw pixel
//! data, not a `LayerOp`, and there is no compact, replayable
//! description of "what a stroke did" the way `SetOpacity { value }`
//! has for a layer property. The only faithful record is the pixel
//! diff itself — invariant §7.3.3's own "reversible operations plus
//! dirtied tiles, not [whole-document] snapshots," applied here as
//! "the dirtied tiles' own before/after content," never anything wider.
//!
//! **A separate stack from `aurora_doc::History`'s own structural one,
//! not a unified chronological journal.** `aurora-brush` doesn't (and,
//! by PRD §7.2's own layering, can't) depend on `aurora-doc`, so this
//! has no way to interleave with `History`'s own undo/redo into one
//! true "most recent action, whatever kind" order. `aurora-app`, which
//! depends on both, picks a policy for `Ctrl+Z`/`Ctrl+Shift+Z` when
//! *both* stacks have something to offer — see its own doc comment for
//! what that policy is and why unifying them for real is separate,
//! still-open follow-on work.

use std::collections::HashMap;

use aurora_core::Rect;
use aurora_tile::{SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

/// The whole-tile dirty rectangle every [`StrokeSnapshot::apply`] write
/// marks — a snapshot restore always replaces a tile's entire content,
/// unlike a dab's own partial-coverage dirty rect.
const fn full_tile_rect() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: TILE,
        height: TILE,
    }
}

/// Which tiles one brush/eraser stroke touched on a single
/// [`aurora_tile::SurfaceId`], and their content from just before this
/// snapshot's own [`Self::apply`] would touch them. Built once per
/// stroke via [`Self::record_touch`] (called before each dab actually
/// writes), then handed to [`crate::PixelHistory::push`] once the
/// stroke ends.
#[derive(Debug, Clone)]
pub struct StrokeSnapshot {
    surface: SurfaceId,
    tiles: HashMap<TileId, Vec<f16>>,
}

impl StrokeSnapshot {
    /// A fresh, empty snapshot for a stroke about to begin on `surface`.
    #[must_use]
    pub fn new(surface: SurfaceId) -> Self {
        Self {
            surface,
            tiles: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn surface(&self) -> SurfaceId {
        self.surface
    }

    /// `true` if no tile has been captured yet — a stroke that started
    /// and ended a drag without ever actually stamping a dab (e.g. a
    /// zero-radius brush, or a click that landed with nothing to paint)
    /// has nothing worth pushing onto undo. [`crate::PixelHistory::push`]
    /// checks this so a no-op "stroke" doesn't clutter the undo stack
    /// with an entry that would restore nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Captures `tile`'s own current content on this snapshot's own
    /// surface, if it hasn't already been captured — call once, before
    /// the *first* write to `tile` during the stroke this snapshot
    /// belongs to, so a later dab touching the same tile again doesn't
    /// overwrite the stroke's own starting point with a mid-stroke
    /// state. A no-op (not an error) if `tile` is already captured.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if paging `tile` in from the scratch disk
    /// fails.
    pub fn record_touch(&mut self, store: &mut TileStore, tile: TileId) -> Result<(), TileError> {
        if self.tiles.contains_key(&tile) {
            return Ok(());
        }
        let content = store.get(self.surface, tile)?.texels().to_vec();
        self.tiles.insert(tile, content);
        Ok(())
    }

    /// Writes every captured tile's own content back into `store`,
    /// marking each dirty so a later GPU upload picks up the restored
    /// content, and returns a fresh snapshot capturing what was just
    /// overwritten — the same "applying an op returns its own inverse"
    /// shape `aurora_doc::History`'s internal `apply` already uses, so
    /// a caller's undo stack and redo stack can hand a snapshot
    /// straight to each other ([`crate::PixelHistory::undo`]/
    /// [`Self::apply`]'s own callers).
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if restoring a captured tile fails (the
    /// same scratch-disk-I/O class of failure every other
    /// `TileStore`-touching call in this crate can raise). A failure
    /// partway through leaves whichever tiles were already restored in
    /// their new state — the same no-larger-transaction guarantee
    /// `stamp_stroke`/`erase_stroke` already have for a mid-stroke
    /// failure, not a new risk this type introduces.
    pub fn apply(&self, store: &mut TileStore) -> Result<Self, TileError> {
        let mut inverse = Self::new(self.surface);
        for (&tile, content) in &self.tiles {
            let current = store.get_mut(self.surface, tile)?;
            inverse.tiles.insert(tile, current.texels().to_vec());
            current.texels_mut().copy_from_slice(content);
            current.mark_dirty(full_tile_rect());
        }
        Ok(inverse)
    }
}

/// Unlimited undo/redo over completed brush/eraser strokes — this
/// module's own doc comment explains why it's a separate stack from
/// `aurora_doc::History`'s structural one, not a unified journal.
#[derive(Debug, Default)]
pub struct PixelHistory {
    undo: Vec<StrokeSnapshot>,
    redo: Vec<StrokeSnapshot>,
}

impl PixelHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Records a just-completed stroke, ready to undo — clears the redo
    /// stack, matching every mainstream editor's own "new activity
    /// invalidates redo" behaviour (the same rule
    /// `aurora_doc::History`'s own recording methods already apply
    /// structurally). A no-op for an empty snapshot
    /// ([`StrokeSnapshot::is_empty`]) — nothing was actually touched, so
    /// there is nothing a later undo could meaningfully reverse.
    pub fn push(&mut self, stroke: StrokeSnapshot) {
        if stroke.is_empty() {
            return;
        }
        self.undo.push(stroke);
        self.redo.clear();
    }

    /// Undoes the most recently pushed stroke, if any. `Ok(false)` (not
    /// an error) when there was nothing to undo — check
    /// [`Self::can_undo`] first if the distinction matters.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if restoring the stroke's own captured
    /// tiles fails.
    pub fn undo(&mut self, store: &mut TileStore) -> Result<bool, TileError> {
        let Some(stroke) = self.undo.pop() else {
            return Ok(false);
        };
        let inverse = stroke.apply(store)?;
        self.redo.push(inverse);
        Ok(true)
    }

    /// Redoes the most recently undone stroke, if any. Same `Ok(false)`
    /// shape as [`Self::undo`] for nothing to redo.
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`].
    pub fn redo(&mut self, store: &mut TileStore) -> Result<bool, TileError> {
        let Some(stroke) = self.redo.pop() else {
            return Ok(false);
        };
        let inverse = stroke.apply(store)?;
        self.undo.push(inverse);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{PixelHistory, StrokeSnapshot};
    use aurora_tile::{SurfaceId, TileId};
    use half::f16;
    use std::num::NonZeroUsize;

    fn real_tile_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };
        (dir, store)
    }

    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    /// Fills every texel of `tile` with `value` — a cheap, distinctive
    /// way to tell "before" and "after" content apart in a test without
    /// needing a real dab.
    fn fill(store: &mut aurora_tile::TileStore, tile: TileId, value: f32) {
        let Ok(t) = store.get_mut(surface(), tile) else {
            unreachable!("a real store must accept this write");
        };
        for sample in t.texels_mut() {
            *sample = f16::from_f32(value);
        }
    }

    fn first_sample(store: &mut aurora_tile::TileStore, tile: TileId) -> f32 {
        let Ok(t) = store.get(surface(), tile) else {
            unreachable!("just written");
        };
        let Some(&sample) = t.texels().first() else {
            unreachable!("a real tile always has at least one sample");
        };
        sample.to_f32()
    }

    #[test]
    // Exact-literal round-trip through f16 storage, no arithmetic --
    // same reasoning `aurora-doc`'s own tests already document for
    // their float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn record_touch_only_captures_the_first_time() {
        let (_dir, mut store) = real_tile_store();
        let tile = TileId { x: 0, y: 0 };
        fill(&mut store, tile, 0.25);

        let mut stroke = StrokeSnapshot::new(surface());
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        // A mid-stroke change after the first capture -- must not be
        // what a later apply() restores to.
        fill(&mut store, tile, 0.75);
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }

        let Ok(inverse) = stroke.apply(&mut store) else {
            unreachable!("a real store must accept this write");
        };
        assert_eq!(
            first_sample(&mut store, tile),
            0.25,
            "must restore the stroke's own starting content, not the mid-stroke value"
        );
        // The inverse must hold what was just overwritten (0.75), so
        // redoing lands back there, not at some other value.
        let Ok(_) = inverse.apply(&mut store) else {
            unreachable!("a real store must accept this write");
        };
        assert_eq!(first_sample(&mut store, tile), 0.75);
    }

    #[test]
    fn a_snapshot_that_never_recorded_a_touch_is_empty() {
        let stroke = StrokeSnapshot::new(surface());
        assert!(stroke.is_empty());
    }

    #[test]
    fn pixel_history_push_of_an_empty_stroke_is_a_no_op() {
        let mut history = PixelHistory::new();
        history.push(StrokeSnapshot::new(surface()));
        assert!(!history.can_undo());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn pixel_history_undo_then_redo_round_trips_a_real_stroke() {
        let (_dir, mut store) = real_tile_store();
        let tile = TileId { x: 0, y: 0 };
        fill(&mut store, tile, 0.25);

        let mut stroke = StrokeSnapshot::new(surface());
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        fill(&mut store, tile, 0.75);

        let mut history = PixelHistory::new();
        history.push(stroke);
        assert!(history.can_undo());
        assert!(!history.can_redo());

        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert_eq!(first_sample(&mut store, tile), 0.25, "undo must restore");
        assert!(!history.can_undo());
        assert!(history.can_redo());

        match history.redo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert_eq!(first_sample(&mut store, tile), 0.75, "redo must reapply");
    }

    #[test]
    fn pixel_history_undo_with_nothing_to_undo_returns_false() {
        let (_dir, mut store) = real_tile_store();
        let mut history = PixelHistory::new();
        match history.undo(&mut store) {
            Ok(false) => {}
            other => unreachable!("expected Ok(false), got {other:?}"),
        }
    }

    #[test]
    fn pushing_a_new_stroke_clears_the_redo_stack() {
        let (_dir, mut store) = real_tile_store();
        let tile = TileId { x: 0, y: 0 };
        fill(&mut store, tile, 0.25);

        let mut first = StrokeSnapshot::new(surface());
        if let Err(err) = first.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        fill(&mut store, tile, 0.5);

        let mut history = PixelHistory::new();
        history.push(first);
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(history.can_redo());

        let mut second = StrokeSnapshot::new(surface());
        if let Err(err) = second.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        history.push(second);
        assert!(
            !history.can_redo(),
            "new activity must invalidate the old redo entry"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn stroke_touching_multiple_tiles_restores_all_of_them() {
        let (_dir, mut store) = real_tile_store();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        fill(&mut store, a, 0.25);
        fill(&mut store, b, 0.5);

        let mut stroke = StrokeSnapshot::new(surface());
        for tile in [a, b] {
            if let Err(err) = stroke.record_touch(&mut store, tile) {
                unreachable!("{err:?}");
            }
        }
        fill(&mut store, a, 0.9);
        fill(&mut store, b, 0.9);

        if let Err(err) = stroke.apply(&mut store) {
            unreachable!("{err:?}");
        }
        assert_eq!(first_sample(&mut store, a), 0.25);
        assert_eq!(first_sample(&mut store, b), 0.5);
    }
}
