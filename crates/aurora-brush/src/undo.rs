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
//!
//! **Capture happens where the write happens** (0.55.0, corrected in
//! 0.56.0). [`StrokeSnapshot::record_content`] takes a tile's bytes from
//! a caller that is already holding that tile, rather than paging it in
//! a second time the way [`StrokeSnapshot::record_touch`] does. That is
//! what keeps "captured" and "painted" the same set:
//! [`crate::stamp_dab`]/[`crate::erase_dab`] capture from the tile they
//! already hold, in the instant before that tile's *first actual texel
//! write*. So a tile whose page-in failed is never captured; a tile the
//! dab acquired but then changed nothing in — an off-surface dab, a dab
//! landing entirely on already-maximal alpha, an erase over
//! already-transparent pixels — is never captured either; and a stroke
//! that changed no pixel anywhere pushes no undo entry
//! ([`PixelHistory::push`]'s own empty-snapshot rule).
//!
//! 0.55.0 captured on the success arm of the `get_mut` itself, one step
//! too early: acquiring a tile is not the same as writing to it, and
//! each of the three cases above produced a real undo entry for pixels
//! nothing had changed. [`crate::DabOutcome::painted`] moved with it, so
//! the two are still the same set by construction — see
//! [`StrokeSnapshot::captured`] for the direct assertion.

use std::collections::HashMap;

use aurora_core::Rect;
use aurora_tile::{SAMPLES, SurfaceId, TILE, TileError, TileId, TileStore};
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
/// stroke — via [`Self::record_content`] for the brush/eraser dab path
/// (each dab captures the tiles it actually acquires, as it acquires
/// them), or [`Self::record_touch`] for a caller with no tile already
/// in hand — then handed to [`crate::PixelHistory::push`] once the
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
    /// **Not what the brush/eraser dab path uses any more** (0.55.0).
    /// A caller that has to page `tile` in *itself*, before the write
    /// that will page it in again, can capture a tile the write then
    /// fails to touch — which is exactly the phantom undo entry
    /// [`Self::record_content`] exists to make impossible.
    /// [`crate::stamp_dab`]/[`crate::erase_dab`] capture through that
    /// method instead, from the tile they already hold. This one
    /// remains for a caller that genuinely has no tile in hand.
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

    /// Captures `texels` as `tile`'s own pre-stroke content, if that
    /// tile hasn't already been captured — the same once-per-stroke rule
    /// [`Self::record_touch`] follows, for a caller that is already
    /// holding the tile it is about to write
    /// ([`crate::stamp_dab`]/[`crate::erase_dab`], which capture in the
    /// instant before a tile's first actual texel write, so a captured
    /// tile and a painted tile are the same set by construction).
    /// Infallible: nothing is paged in, because the content is already
    /// in hand.
    ///
    /// **First capture wins.** A later dab in the same stroke touching
    /// the same tile must not overwrite the stroke's own starting point
    /// with a mid-stroke state — that is what makes undo restore a
    /// multi-dab stroke to where it began rather than to where its first
    /// dab left off.
    ///
    /// `texels` must be one whole tile's worth of samples
    /// (`aurora_tile::SAMPLES`, which is by definition the length of
    /// every `aurora_tile::Tile::texels()` slice). Anything else is a
    /// caller bug: it is refused and logged (`tracing::error!`) rather
    /// than stored, because a stored short or long slice would reach
    /// [`Self::apply`]'s own `copy_from_slice` and *panic* there — and
    /// this workspace denies panics precisely because one loses a
    /// professional's unsaved work. Both real callers pass
    /// `Tile::texels()` directly, so this branch is unreachable today;
    /// it exists so that a future one cannot reopen the panic.
    ///
    /// # Returns
    ///
    /// `true` if `tile`'s pre-stroke content is captured after this call
    /// — including the case where an earlier call in the same stroke
    /// already captured it — and `false` only on the refusal above.
    ///
    /// **The `bool` is the point** (0.57.0). Refusing used to return
    /// `()`, and the callers had already decided to write by the time
    /// they asked: they wrote the tile, counted it painted, and reported
    /// the dab a success with no undo entry covering it — a silent loss
    /// of undo coverage for a real edit, which is strictly worse than
    /// the panic the refusal replaced (a panic at least loses nothing
    /// already committed). `#[must_use]` so a caller cannot go back to
    /// ignoring it; [`crate::stamp_dab`]/[`crate::erase_dab`] now
    /// abandon the tile before its first write and report it in
    /// [`crate::DabOutcome::failed`] instead.
    #[must_use]
    pub fn record_content(&mut self, tile: TileId, texels: &[f16]) -> bool {
        if texels.len() != SAMPLES {
            tracing::error!(
                got = texels.len(),
                expected = SAMPLES,
                surface = ?self.surface,
                ?tile,
                "refusing to capture a stroke snapshot from something that is not one whole \
                 tile's worth of texels; this tile will not be painted either"
            );
            return false;
        }
        self.tiles.entry(tile).or_insert_with(|| texels.to_vec());
        true
    }

    /// Every tile this snapshot has captured, in no particular order —
    /// exactly the tiles [`Self::apply`] would restore, and (for a
    /// snapshot the brush/eraser dab path built) exactly the union of
    /// every [`crate::DabOutcome::painted`] along the stroke.
    ///
    /// Exists so a test can assert *what* was captured directly rather
    /// than inferring it from whether a later `apply` happened to
    /// succeed.
    ///
    /// No `#[must_use]`: `impl Iterator` already carries one, and
    /// `clippy::double_must_use` (denied here) rejects the second.
    pub fn captured(&self) -> impl Iterator<Item = TileId> + '_ {
        self.tiles.keys().copied()
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
    /// **All or nothing** (0.52.2, second review round). Every tile is
    /// read into the inverse snapshot *first*, and only once all of them
    /// have been read is a single byte written. A read that fails
    /// therefore leaves the store completely untouched, which is what
    /// makes a later retry correct rather than merely possible.
    ///
    /// It was not always so, and the failure was subtle enough to be
    /// worth recording. This used to read and write one tile at a time,
    /// capturing each tile's inverse just before overwriting it. A
    /// failure partway through left some tiles restored and some not —
    /// tolerable while a store read failed at most once, but 0.52.2 made
    /// a broken tile fail *every* time, and
    /// [`crate::PixelHistory::undo`] correctly began retrying. The retry
    /// then recaptured its inverse from the half-restored state, so the
    /// inverse recorded the *restored* content for tiles the first
    /// attempt had already rewritten — and the matching redo silently
    /// put those tiles back to where they already were, losing the
    /// stroke on a nondeterministic subset of tiles (`HashMap` iteration
    /// order) while returning `Ok`. An independent review reproduced
    /// exactly that: 2–6 of 8 tiles losing their stroke, every run, with
    /// no error anywhere.
    ///
    /// The write phase can still fail in one narrow case — a tile
    /// evicted between the two phases whose page-in then fails — and it
    /// is handled rather than ignored: whatever was already written is
    /// rolled back from the inverse just captured, so the store returns
    /// to its pre-`apply` state and the retry stays correct. A rollback
    /// write that itself fails is logged (`tracing::error!`) and the
    /// original error is still returned; nothing better than "report it
    /// loudly" exists at that point, and it takes a scratch disk failing
    /// mid-rollback to reach.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if reading or restoring a captured tile
    /// fails (the same scratch-disk-I/O class of failure every other
    /// `TileStore`-touching call in this crate can raise).
    pub fn apply(&self, store: &mut TileStore) -> Result<Self, TileError> {
        // Phase one: read every tile, write nothing. `?` here is safe to
        // return on precisely because nothing has been modified yet.
        let mut inverse = Self::new(self.surface);
        for &tile in self.tiles.keys() {
            let content = store.get(self.surface, tile)?.texels().to_vec();
            inverse.tiles.insert(tile, content);
        }

        // Phase two: write. Every tile was resident a moment ago, so the
        // only way this fails is an eviction between the phases whose
        // page-in then fails -- narrow, but not impossible, so it rolls
        // back rather than leaving a half-applied restore behind.
        let mut written: Vec<TileId> = Vec::with_capacity(self.tiles.len());
        for (&tile, content) in &self.tiles {
            if let Err(err) = Self::restore_tile(store, self.surface, tile, content) {
                for done in &written {
                    let Some(previous) = inverse.tiles.get(done) else {
                        continue;
                    };
                    if let Err(rollback) = Self::restore_tile(store, self.surface, *done, previous)
                    {
                        tracing::error!(
                            ?rollback,
                            surface = ?self.surface,
                            tile = ?done,
                            "failed to roll back a partially applied stroke restore; this tile is \
                             left restored while others are not"
                        );
                    }
                }
                return Err(err);
            }
            written.push(tile);
        }
        Ok(inverse)
    }

    /// Overwrites one tile with `content` and marks it fully dirty — the
    /// single write both [`Self::apply`]'s write phase and its rollback
    /// are built from, so the two cannot drift apart.
    ///
    /// The length check is not dead weight. `copy_from_slice` panics on
    /// a length mismatch, in either direction, and this crate has one
    /// `pub`, infallible door through which content enters a snapshot
    /// ([`Self::record_content`]). That door already refuses anything
    /// that isn't `aurora_tile::SAMPLES` long, so a mismatch here is
    /// structurally unreachable — this is the second lock on the same
    /// door, because the cost of being wrong is a panic that loses
    /// unsaved work, and the cost of the check is one integer compare
    /// per tile on the undo path (not the brush hot path). It refuses
    /// the tile rather than half-writing it: a partially overwritten
    /// tile is worse than an unrestored one.
    fn restore_tile(
        store: &mut TileStore,
        surface: SurfaceId,
        tile: TileId,
        content: &[f16],
    ) -> Result<(), TileError> {
        let current = store.get_mut(surface, tile)?;
        let texels = current.texels_mut();
        if texels.len() != content.len() {
            tracing::error!(
                got = content.len(),
                expected = texels.len(),
                ?surface,
                ?tile,
                "refusing to restore a tile from a snapshot that is not one whole tile long; \
                 this tile is left as it is"
            );
            return Ok(());
        }
        texels.copy_from_slice(content);
        current.mark_dirty(full_tile_rect());
        Ok(())
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
    /// there is nothing a later undo could meaningfully reverse. Returns
    /// whether the stroke was actually recorded, so a caller keeping its
    /// own parallel record of *when* each push happened (`aurora-app`'s
    /// own unified undo ordering across this type and
    /// `aurora_doc::History`) knows whether to record this one too.
    #[must_use]
    pub fn push(&mut self, stroke: StrokeSnapshot) -> bool {
        if stroke.is_empty() {
            return false;
        }
        self.undo.push(stroke);
        self.redo.clear();
        true
    }

    /// Discards every redo entry without touching the undo stack — the
    /// same clearing [`Self::push`] already does internally when a new
    /// stroke completes, exposed here for a caller that needs to
    /// invalidate this type's own redo stack in response to activity
    /// recorded somewhere else entirely (a structural edit through
    /// `aurora_doc::History`, from the same unified-ordering need
    /// `aurora_doc::History::clear_redo` itself already documents).
    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    /// The stroke [`Self::undo`] would restore next, without restoring
    /// it — `None` exactly when [`Self::can_undo`] is `false`.
    ///
    /// Purely a read: nothing is popped, nothing is applied, and the
    /// store is not consulted at all. It exists so a caller can find out
    /// *which tiles on which surface* the next undo is about to rewrite
    /// **before** that undo happens, which is the only moment the
    /// information is still available — `undo` itself reports a bare
    /// `bool`, and by the time it has returned the snapshot has been
    /// replaced by its own inverse. `aurora-app` uses this to invalidate
    /// exactly the composite tiles an undo will change instead of
    /// throwing its whole composite cache away.
    ///
    /// The tiles reported are precisely the set [`StrokeSnapshot::apply`]
    /// will write ([`StrokeSnapshot::captured`]), on
    /// [`StrokeSnapshot::surface`] — same snapshot, same call, so the two
    /// cannot drift apart.
    #[must_use]
    pub fn peek_undo(&self) -> Option<&StrokeSnapshot> {
        self.undo.last()
    }

    /// The stroke [`Self::redo`] would reapply next, without reapplying
    /// it — [`Self::peek_undo`]'s counterpart on the other stack, with
    /// the same guarantees, and `None` exactly when [`Self::can_redo`] is
    /// `false`.
    #[must_use]
    pub fn peek_redo(&self) -> Option<&StrokeSnapshot> {
        self.redo.last()
    }

    /// Undoes the most recently pushed stroke, if any. `Ok(false)` (not
    /// an error) when there was nothing to undo — check
    /// [`Self::can_undo`] first if the distinction matters.
    ///
    /// **A failed undo keeps the entry.** The stroke is popped only once
    /// [`StrokeSnapshot::apply`] has actually succeeded. Until 0.52.2 it
    /// was popped first, so a failing `apply` — which since 0.52.2 is
    /// what a permanently unreadable tile produces on *every* attempt,
    /// rather than only the first — dropped that snapshot on the floor:
    /// not pushed to redo, not put back on undo, simply gone, taking the
    /// only record of what those pixels used to be with it. Peeking first
    /// is the same "hold a real replacement before letting go of the only
    /// copy" rule `aurora_tile::TileStore::ensure_resident` follows for a
    /// failed page-in.
    ///
    /// Retrying really is safe, and that took a second fix to be true:
    /// [`StrokeSnapshot::apply`] is now all-or-nothing, so a failed undo
    /// leaves the store exactly as it was. The first shape of this fix
    /// made a failed undo *retryable* while `apply` was still writing
    /// tile by tile, and a retry then captured its inverse from the
    /// half-restored state — which silently lost the stroke on part of
    /// the following redo. See `apply`'s own doc comment for the full
    /// account.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if restoring the stroke's own captured
    /// tiles fails. The undo stack is unchanged in that case.
    pub fn undo(&mut self, store: &mut TileStore) -> Result<bool, TileError> {
        let Some(stroke) = self.undo.last() else {
            return Ok(false);
        };
        // Borrowed, not removed: `apply` is fallible, and `?` below
        // returns before the `pop` that commits to this undo actually
        // having happened.
        let inverse = stroke.apply(store)?;
        let _ = self.undo.pop();
        self.redo.push(inverse);
        Ok(true)
    }

    /// Redoes the most recently undone stroke, if any. Same `Ok(false)`
    /// shape as [`Self::undo`] for nothing to redo, and the same
    /// peek-apply-then-commit ordering: a redo whose `apply` fails leaves
    /// the redo stack exactly as it was rather than discarding the entry.
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`].
    pub fn redo(&mut self, store: &mut TileStore) -> Result<bool, TileError> {
        let Some(stroke) = self.redo.last() else {
            return Ok(false);
        };
        let inverse = stroke.apply(store)?;
        let _ = self.redo.pop();
        self.undo.push(inverse);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{PixelHistory, StrokeSnapshot};
    use crate::test_support::{break_the_only_scratch_file, surface};
    use aurora_tile::TileId;
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

    /// The scratch file `dir`'s single store paged `tile` out to.
    ///
    /// Found by scanning rather than by rebuilding the name: since
    /// 0.53.0 `aurora_tile::TileStore` prefixes every scratch file with
    /// a per-store instance token (so two stores sharing a directory
    /// cannot collide), and that token is deliberately private to that
    /// crate. The `(surface, x, y)` tail is still the stable,
    /// addressable part, and exactly one file in a single-store
    /// directory ends with it.
    fn evicted_tile_file(dir: &std::path::Path, tile: TileId) -> std::path::PathBuf {
        let suffix = format!("_{}_{}_{}.tile", surface().to_raw(), tile.x, tile.y);
        let Ok(entries) = std::fs::read_dir(dir) else {
            unreachable!("the scratch directory must be readable");
        };
        let matches: Vec<std::path::PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|name| name.ends_with(&suffix))
            })
            .collect();
        match matches.as_slice() {
            [only] => only.clone(),
            other => unreachable!("exactly one evicted file names this tile, found {other:?}"),
        }
    }

    /// A refused capture has to be *visible* to the caller (0.57.0).
    ///
    /// Returning `()` on refusal meant the caller — which had already
    /// decided to write by the time it asked — wrote the tile anyway,
    /// counted it painted, and reported the dab a success with no undo
    /// entry covering it: a silent loss of undo coverage for a real
    /// edit, and strictly worse than the panic the refusal replaced. The
    /// `bool` is what lets `stamp_dab`/`erase_dab` abandon the tile
    /// before its first write instead.
    #[test]
    fn record_content_reports_whether_it_actually_captured() {
        let tile = TileId { x: 3, y: 4 };
        let mut stroke = StrokeSnapshot::new(surface());

        assert!(
            !stroke.record_content(tile, &[f16::from_f32(1.0); 7]),
            "a slice that is not one whole tile must be refused, and say so"
        );
        assert!(
            stroke.is_empty(),
            "and must not have captured anything either"
        );

        let whole = vec![f16::from_f32(0.25); aurora_tile::SAMPLES];
        assert!(
            stroke.record_content(tile, &whole),
            "one whole tile's worth of samples must be captured, and say so"
        );
        assert_eq!(stroke.captured().collect::<Vec<_>>(), [tile]);
        assert!(
            stroke.record_content(tile, &whole),
            "an already-captured tile is still captured after this call, so this reports \
             success too -- `false` means `this tile is not undoable`, not `nothing was \
             stored just now`"
        );
        assert_eq!(
            stroke.captured().collect::<Vec<_>>(),
            [tile],
            "first capture still wins"
        );
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
        assert!(!history.push(StrokeSnapshot::new(surface())));
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
        assert!(history.push(stroke));
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
        assert!(history.push(first));
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(history.can_redo());

        let mut second = StrokeSnapshot::new(surface());
        if let Err(err) = second.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        assert!(history.push(second));
        assert!(
            !history.can_redo(),
            "new activity must invalidate the old redo entry"
        );
    }

    /// [`PixelHistory::peek_undo`] has one job: report the surface and
    /// tile set the *following* `undo` will actually rewrite, so a
    /// caller can invalidate exactly those and nothing else. Asserted
    /// against the real `undo` rather than against the snapshot the test
    /// pushed, so the two can't drift apart.
    #[test]
    fn peek_undo_reports_the_same_surface_and_tiles_the_following_undo_restores() {
        let (_dir, mut store) = real_tile_store();
        let touched = [TileId { x: 0, y: 0 }, TileId { x: 3, y: 2 }];
        for tile in touched {
            fill(&mut store, tile, 0.25);
        }

        let mut stroke = StrokeSnapshot::new(surface());
        for tile in touched {
            if let Err(err) = stroke.record_touch(&mut store, tile) {
                unreachable!("{err:?}");
            }
        }
        for tile in touched {
            fill(&mut store, tile, 0.75);
        }

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));

        let (peeked_surface, mut peeked): (aurora_tile::SurfaceId, Vec<TileId>) =
            match history.peek_undo() {
                Some(next) => (next.surface(), next.captured().collect()),
                None => unreachable!("a stroke was just pushed"),
            };
        peeked.sort_by_key(|tile| (tile.x, tile.y));
        assert_eq!(peeked_surface, surface());
        assert_eq!(peeked, touched.to_vec());

        // Nothing was consumed by peeking, and the tiles it named are
        // exactly the ones the real undo rewrites.
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        for tile in touched {
            assert!(
                (first_sample(&mut store, tile) - 0.25).abs() < 1e-3,
                "{tile:?} must be restored to its pre-stroke content"
            );
        }
    }

    /// The same contract on the redo stack, plus the empty-stack case
    /// for both — `None` exactly when there is nothing to peek at.
    #[test]
    fn peek_redo_reports_the_next_redo_and_both_peeks_are_none_on_an_empty_stack() {
        let (_dir, mut store) = real_tile_store();
        let tile = TileId { x: 1, y: 1 };
        fill(&mut store, tile, 0.25);

        let mut history = PixelHistory::new();
        assert!(
            history.peek_undo().is_none(),
            "nothing pushed yet, so nothing to undo"
        );
        assert!(
            history.peek_redo().is_none(),
            "nothing undone yet, so nothing to redo"
        );

        let mut stroke = StrokeSnapshot::new(surface());
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        fill(&mut store, tile, 0.75);
        assert!(history.push(stroke));
        assert!(
            history.peek_redo().is_none(),
            "a fresh push leaves the redo stack empty"
        );

        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(
            history.peek_undo().is_none(),
            "the only stroke moved to the redo stack"
        );
        let peeked: Vec<TileId> = match history.peek_redo() {
            Some(next) => {
                assert_eq!(next.surface(), surface());
                next.captured().collect()
            }
            None => unreachable!("the undone stroke is now redoable"),
        };
        assert_eq!(peeked, vec![tile]);

        match history.redo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(
            (first_sample(&mut store, tile) - 0.75).abs() < 1e-3,
            "the redo must put the stroke's own content back"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn clear_redo_discards_a_pending_redo_without_touching_undo() {
        let (_dir, mut store) = real_tile_store();
        let tile = TileId { x: 0, y: 0 };

        // Two strokes on the same tile, so undoing the second one still
        // leaves a real, distinguishable first stroke behind on the
        // undo stack.
        fill(&mut store, tile, 0.25);
        let mut first = StrokeSnapshot::new(surface());
        if let Err(err) = first.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        fill(&mut store, tile, 0.5);

        let mut history = PixelHistory::new();
        assert!(history.push(first));

        let mut second = StrokeSnapshot::new(surface());
        if let Err(err) = second.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        fill(&mut store, tile, 0.75);
        assert!(history.push(second));

        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(history.can_redo());
        assert!(history.can_undo(), "the first stroke is still undoable");

        history.clear_redo();

        assert!(!history.can_redo(), "the pending redo must be discarded");
        assert!(
            history.can_undo(),
            "clear_redo must not touch the undo stack"
        );
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert_eq!(
            first_sample(&mut store, tile),
            0.25,
            "the untouched undo entry must still restore correctly"
        );
    }

    /// Red-team's own reproduction, as a regression test: an eight-tile
    /// stroke, one tile transiently unreadable, an undo that fails, the
    /// tile repaired, the undo retried — and then a **redo that must
    /// restore all eight tiles**, not the nondeterministic two-to-six of
    /// them the pre-0.52.2 `apply` managed.
    ///
    /// The mechanism it guards: `apply` used to capture each tile's
    /// inverse immediately before overwriting that tile, so a failure
    /// partway through left the store half-restored, and the retry then
    /// captured its inverse from *that* state — recording the restored
    /// content as if it were the pre-undo content for every tile the
    /// first attempt had already rewritten. The redo built from that
    /// inverse put those tiles back where they already were and returned
    /// `Ok`, silently losing the stroke on them. Two-phase `apply` closes
    /// it: nothing is written until every tile has been read.
    ///
    /// The store's budget is exactly the stroke's tile count, and eight
    /// unrelated "filler" tiles are touched in between to push the whole
    /// stroke out to the scratch disk. That is what makes the broken tile
    /// genuinely read from disk (a budget larger than the document never
    /// evicts anything, so no read could fail) **without** any tile being
    /// evicted twice: a smaller budget would churn tiles in and out
    /// during `apply` itself and expose the separate, still-open
    /// stale-write race (`PLAN.md`, "write jobs carry no sequence
    /// number"), which made an earlier draft of this test intermittent
    /// for a reason that had nothing to do with what it is testing.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_retried_undo_restores_every_tile_and_its_redo_puts_every_stroke_back() {
        const TILES: u32 = 8;
        const BEFORE: f32 = 0.25;
        const AFTER: f32 = 0.75;

        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = NonZeroUsize::new(TILES as usize) else {
            unreachable!("8 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };
        let tiles: Vec<TileId> = (0..TILES).map(|x| TileId { x, y: 0 }).collect();

        let mut stroke = StrokeSnapshot::new(surface());
        for &tile in &tiles {
            fill(&mut store, tile, BEFORE);
            if let Err(err) = stroke.record_touch(&mut store, tile) {
                unreachable!("{err:?}");
            }
        }
        for &tile in &tiles {
            fill(&mut store, tile, AFTER);
        }

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));

        // Touch `TILES` unrelated tiles: with a budget of exactly
        // `TILES`, this evicts the whole stroke to the scratch disk and
        // nothing else. `flush` then confirms those writes, so `pending`
        // is empty and a later read genuinely reaches the file below.
        for x in 100..100 + TILES {
            if let Err(err) = store.get_mut(surface(), TileId { x, y: 0 }) {
                unreachable!("a first touch always succeeds: {err:?}");
            }
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        // Break the *first* tile's scratch file, keeping the real bytes
        // so it can be repaired again.
        let Some(&broken) = tiles.first() else {
            unreachable!("eight tiles were just built");
        };
        let victim = evicted_tile_file(dir.path(), broken);
        let victim = victim.as_path();
        let Ok(original) = std::fs::read(victim) else {
            unreachable!("the evicted tile file must be readable");
        };
        let Some(truncated) = original.get(..original.len() / 2) else {
            unreachable!("half of a slice's own length is always in range");
        };
        if let Err(err) = std::fs::write(victim, truncated) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        match history.undo(&mut store) {
            Err(aurora_tile::TileError::CorruptFile(_)) => {}
            other => unreachable!("expected the restore to fail, got {other:?}"),
        }
        assert!(history.can_undo(), "the entry must survive a failed undo");
        // Atomicity, stated directly: not one readable tile was touched
        // by the attempt that failed.
        for &tile in tiles.iter().skip(1) {
            assert_eq!(
                first_sample(&mut store, tile),
                AFTER,
                "a failed undo must not restore *any* tile, not even partially"
            );
        }

        if let Err(err) = std::fs::write(victim, &original) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("the retry must succeed once the tile is readable: {other:?}"),
        }
        for &tile in &tiles {
            assert_eq!(
                first_sample(&mut store, tile),
                BEFORE,
                "the retried undo must restore every tile"
            );
        }

        match history.redo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        for &tile in &tiles {
            assert_eq!(
                first_sample(&mut store, tile),
                AFTER,
                "redo must put the stroke back on every tile -- the inverse must have been \
                 captured from the pre-undo state, not from a half-restored one"
            );
        }
    }

    /// A `TileStore` whose scratch budget is one tile, so touching a
    /// second tile evicts the first to disk — the setup
    /// [`break_the_only_scratch_file`] needs to manufacture a tile that
    /// genuinely cannot be read back.
    fn one_tile_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };
        (dir, store)
    }

    /// An undo whose restore fails must not consume the undo entry
    /// (0.52.2). `PixelHistory::undo` used to pop first and apply second,
    /// so a failing `apply` dropped the popped snapshot entirely — never
    /// pushed to redo, never put back — and with it the only record of
    /// what those pixels were before the stroke. Harmless while a store
    /// read failed at most once; not harmless since 0.52.2 made an
    /// unreadable tile fail on every read, which turns "one failed undo"
    /// into "that stroke is unundoable forever, silently".
    #[test]
    fn an_undo_whose_restore_fails_keeps_the_entry_instead_of_destroying_it() {
        let (dir, mut store) = one_tile_store();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        fill(&mut store, a, 0.25);

        let mut stroke = StrokeSnapshot::new(surface());
        if let Err(err) = stroke.record_touch(&mut store, a) {
            unreachable!("{err:?}");
        }
        fill(&mut store, a, 0.75);

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));

        // Evict `a` to the scratch disk, make the write real, then
        // corrupt the file it landed in: `a` is now permanently
        // unreadable, so the undo below cannot complete.
        fill(&mut store, b, 0.0);
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        break_the_only_scratch_file(&dir);

        match history.undo(&mut store) {
            Err(aurora_tile::TileError::CorruptFile(_)) => {}
            other => unreachable!("expected the restore to fail, got {other:?}"),
        }

        assert!(
            history.can_undo(),
            "a failed undo must leave the stroke on the undo stack -- dropping it destroys the \
             only record of what those pixels were"
        );
        assert!(
            !history.can_redo(),
            "and must not push a redo entry for an undo that never happened"
        );
    }

    /// The mirror of the test above for [`PixelHistory::redo`], which had
    /// the identical pop-then-apply shape and the identical consequence.
    #[test]
    fn a_redo_whose_restore_fails_keeps_the_entry_instead_of_destroying_it() {
        let (dir, mut store) = one_tile_store();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        fill(&mut store, a, 0.25);

        let mut stroke = StrokeSnapshot::new(surface());
        if let Err(err) = stroke.record_touch(&mut store, a) {
            unreachable!("{err:?}");
        }
        fill(&mut store, a, 0.75);

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(history.can_redo());

        fill(&mut store, b, 0.0);
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        break_the_only_scratch_file(&dir);

        match history.redo(&mut store) {
            Err(aurora_tile::TileError::CorruptFile(_)) => {}
            other => unreachable!("expected the restore to fail, got {other:?}"),
        }

        assert!(
            history.can_redo(),
            "a failed redo must leave the stroke on the redo stack"
        );
        assert!(
            !history.can_undo(),
            "and must not push an undo entry for a redo that never happened"
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
