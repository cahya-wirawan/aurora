//! Stamps a brush dab into a document's shared `aurora_tile::TileStore`,
//! at a given `SurfaceId` — the piece `dab`'s own doc comment
//! named as blocked until [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)
//! gave pixel storage a real, addressable shape. Ported from
//! `spike/vertical-slice::doc::Document::dab`, the one piece of this
//! crate's job that spike already measured and proved workable
//! (`spike/FINDINGS.md`), generalized to `aurora_tile`'s real,
//! multi-surface `TileStore` API instead of the spike's own
//! single-surface throwaway store.
//!
//! **Max-alpha accumulation within one stroke, not source-over**:
//! overlapping dabs along the same stroke don't darken along the
//! spine — the same choice the spike already made and measured, not a
//! new one. Colour is straight `[f32; 3]` RGB; the dab's own smooth
//! radial falloff supplies alpha, not a separately specified one.
//!
//! **No document-extent clamp**, unlike the spike's own fixed-size
//! `Document`: `aurora_tile::TileStore` has no notion of a document's
//! own width/height to clamp against — it is sparse and unbounded by
//! design (the same property invariant §7.3.1 relies on for the
//! 300,000 px ceiling). A caller that needs to keep a dab within a
//! layer's own `bounds` is responsible for clipping `center`/`radius`
//! itself before calling — real, separate follow-on work, since no
//! caller does that yet.
//!
//! **A broken tile costs only itself** (0.55.0). [`stamp_dab`] and
//! [`erase_dab`] return a [`DabOutcome`] rather than a
//! `Result<usize, TileError>`: a tile whose page-in fails is recorded,
//! with its own error, and the loop moves on to the next tile instead
//! of abandoning every tile after it. They also capture the undo
//! snapshot themselves, from each tile in the instant before they first
//! write to it, so the tiles an undo entry covers and the tiles a dab
//! actually changed are the same set by construction. See
//! [`DabOutcome`] for what the old shape could not express.
//!
//! **Acquired is not painted** (0.56.0). 0.55.0 captured — and counted
//! a tile as painted — on the success arm of the `get_mut` that
//! acquires it. But acquiring a tile is not writing to it: a dab
//! landing entirely outside the surface's own texel range, one landing
//! on pixels whose alpha is already at or above the dab's own falloff
//! (the max-alpha rule below), and an erase over already-transparent
//! pixels all acquire tiles and then change nothing. Each of those
//! still produced a full undo entry. Both the capture and the
//! [`DabOutcome::painted`] entry now happen at a tile's first real
//! texel write, so neither can name a tile nothing happened to.

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

use crate::StrokeSnapshot;

/// Index of tile-local pixel `(x, y)`'s first (red) channel within a
/// tile's own flat `texels()` slice.
const fn texel_index(x: u32, y: u32) -> usize {
    (y * TILE + x) as usize * CHANNELS
}

/// One whole texel, in the channel order a tile stores: R, G, B, A.
type Texel = [f16; CHANNELS];

/// The alpha channel's own offset within a texel — the last one.
const ALPHA: usize = CHANNELS - 1;

/// Compile-time proof that a [`Texel`] literal written as `[r, g, b, a]`
/// really is one whole texel. An `assert!` would be a `panic!` in
/// disguise, which this workspace denies; a length-mismatched array
/// binding is the same check with no runtime component at all.
const _: [(); 4] = [(); CHANNELS];

/// `true` if storing `new` over the texel currently at `current` would
/// not change a single bit.
///
/// **Bit equality, not float equality** — and this is the whole point of
/// the function. Tile storage is `f16` (invariant §7.3.1b) while the dab
/// arithmetic is `f32`, so the value a dab computes and the value it can
/// actually store are different numbers. Comparing the `f32` against the
/// stored `f16` (what `stamp_dab` did until 0.57.0) says "this texel
/// will change" for every `f32` that merely *rounds* to the `f16`
/// already there — reproducibly, by clicking the brush twice on the same
/// point with the same colour and radius. The tile was then marked
/// touched, captured, dirtied and reported painted for a write that
/// stored exactly the bytes already present: a recomposite for nothing,
/// and a "Ctrl+Z did nothing" undo entry — the precise symptom this
/// round of work exists to remove.
///
/// `to_bits`, not `==`, so a NaN channel (a caller handing in a NaN
/// `colour`) compares equal to itself and settles rather than
/// re-reporting the same texel as changed on every dab forever.
fn stores_the_same_bits(current: &[f16], new: Texel) -> bool {
    current.len() == new.len()
        && current
            .iter()
            .zip(new.iter())
            .all(|(old, fresh)| old.to_bits() == fresh.to_bits())
}

/// What one [`stamp_dab`]/[`erase_dab`] call actually managed to do —
/// which tiles it wrote through and which it could not acquire at all.
///
/// Replaces the `Result<usize, TileError>` these used to return, which
/// could not express the case that matters: a dab spanning a healthy
/// tile and a permanently unreadable one. That shape forced a choice
/// between reporting the error and reporting the tiles painted, so the
/// error won and the painted tiles were discarded — and, because the
/// error was returned with `?` from inside the loop, every tile *after*
/// the broken one in iteration order was never even attempted (PLAN.md
/// M1.9, opened by the 0.52.2 review, fixed in 0.55.0).
///
/// No error is dropped: [`Self::failed`] carries every failing tile with
/// its own [`TileError`], not just the first.
#[derive(Debug, Default)]
#[must_use]
pub struct DabOutcome {
    painted: Vec<TileId>,
    failed: Vec<(TileId, TileError)>,
}

impl DabOutcome {
    /// Tiles this dab actually changed at least one texel of, in the
    /// row-major order it visits them — and exactly the tiles a
    /// `snapshot` passed to the dab captured
    /// ([`crate::StrokeSnapshot::captured`]).
    ///
    /// Not every tile the dab's bounding box covers, and (since 0.56.0)
    /// not even every tile it successfully acquired: a tile the dab
    /// paged in and then wrote nothing to — its texels all outside the
    /// falloff radius, or all already at a higher alpha than this dab
    /// would give them — is absent from this list, because nothing about
    /// it changed and there is nothing to invalidate or to undo. Use
    /// [`touched_tiles`] for the geometry a dab is *aimed* at.
    #[must_use]
    pub fn painted(&self) -> &[TileId] {
        &self.painted
    }

    /// Tiles whose page-in failed. This dab left each of them completely
    /// untouched, and captured nothing for them.
    #[must_use]
    pub fn failed(&self) -> &[(TileId, TileError)] {
        &self.failed
    }

    /// The first failure's own error, for a caller that logs one line
    /// per dab rather than one per tile.
    #[must_use]
    pub fn first_error(&self) -> Option<&TileError> {
        self.failed.first().map(|(_, err)| err)
    }

    /// `true` if every tile this dab's bounding box covers was
    /// successfully acquired — i.e. nothing failed. Deliberately *not*
    /// "every covered tile was written to": a complete dab may still
    /// have painted nothing at all (see [`Self::painted`]). Includes the
    /// vacuous case of a dab with no real geometry (`radius <= 0.0`, or
    /// a non-finite `center`/`radius`), which covers no tile at all.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }

    /// The one place a failure is recorded, shared by [`stamp_dab`] and
    /// [`erase_dab`] so their two parallel loops cannot drift apart.
    fn fail(&mut self, tile: TileId, err: TileError) {
        self.failed.push((tile, err));
    }
}

/// `true` if this dab has real geometry to stamp — a finite position, a
/// finite radius, a radius above zero, and a bounding box spanning no
/// more than [`MAX_DAB_TILES`] tiles.
///
/// `radius <= 0.0` is an ordinary input (a zero-pressure sample at a
/// stroke's edge), not a defensive check. The finiteness half *is*
/// defensive, and it is not theoretical: `as u32` on a float saturates
/// in Rust, so a NaN `center` silently becomes `0` and lands the dab on
/// tile `(0, 0)` — painting somewhere the caller never asked for — while
/// an infinite `radius` or `center` produces a tile range spanning the
/// entire `u32` grid. Both are caller bugs, and both are cheaper to
/// refuse here than to reason about downstream.
///
/// **So is a finite-but-absurd radius** (0.57.0). Finiteness alone
/// bounds nothing: `radius = 1e6` spans over 15 million tiles and
/// `1e10` saturates the casts to roughly 2.8e14, and the dab loops walk
/// that range in full however large it is. 0.56.0 capped only the
/// `Vec::with_capacity` *hint*, which left the loop itself — and
/// [`touched_tiles`]' own allocation — unbounded. See [`MAX_DAB_TILES`]
/// for the bound and why it is where it is. `error!`, like
/// [`usable_snapshot`]'s own mismatch: a radius no brush could produce
/// is a caller bug, not a runtime condition.
fn is_paintable(center: (f32, f32), radius: f32) -> bool {
    if !(center.0.is_finite() && center.1.is_finite() && radius.is_finite() && radius > 0.0) {
        return false;
    }
    let (t0, t1) = tile_range(center, radius);
    let span = tile_span(t0, t1);
    if span > MAX_DAB_TILES {
        tracing::error!(
            ?center,
            radius,
            span,
            max = MAX_DAB_TILES,
            "refusing a dab whose bounding box spans more tiles than any real brush could \
             cover; this dab paints nothing"
        );
        return false;
    }
    true
}

/// The largest tile count one dab may span — the bound [`is_paintable`]
/// refuses a dab for exceeding, and therefore also the most capacity
/// [`reserved_tiles`] can ever be asked for.
///
/// Two separate things go wrong without a bound here, and 0.56.0 closed
/// only the second. The tile range is derived from caller-supplied
/// floats, so a large *finite* `radius` produces a bounding box of any
/// size at all: `1e6` spans over 15 million tiles, `1e10` saturates the
/// casts to roughly 2.8e14. (a) The dab loops iterate that range in
/// full — an unbounded freeze on the UI thread, since painting happens
/// there. (b) `Vec::with_capacity` for a count that large aborts the
/// process on allocation failure, which no `Result` and no
/// `catch_unwind` can catch. 0.56.0 capped the capacity *hint*, which
/// fixed (b) for the dab loops' own `painted` vector and neither half
/// anywhere else: the iteration is what costs the time, and
/// [`touched_tiles`] still sized a `Vec` from the same unbounded range.
/// Refusing the dab outright fixes both at once.
///
/// **Why 4096.** It is a 64 x 64 grid of tiles: a 16,384 x 16,384 px
/// bounding box, i.e. radii up to roughly 8,100 px. Photoshop's own
/// largest brush is 5,000 px across (radius 2,500) and `aurora-app`'s
/// is a hardcoded 24 px, so this leaves better than 3x headroom over
/// the largest brush any shipping editor offers — nothing a real
/// brush-size control could produce comes near it — while keeping the
/// worst *accepted* case a bounded amount of genuine painting work
/// instead of an open-ended one. This is a bound on caller bugs, not on
/// brushes.
const MAX_DAB_TILES: u64 = 4096;

/// How many tiles the inclusive tile range `t0..=t1` covers —
/// overflow-safe (the two sides' product overflows `u32` for a large
/// `radius`, and this workspace sets no `overflow-checks`, so in release
/// it would wrap to a nonsense count rather than trap) and deliberately
/// *uncapped*: [`is_paintable`]'s bound check needs the true count,
/// which is exactly what a capped one could not give it.
fn tile_span(t0: TileId, t1: TileId) -> u64 {
    let wide = u64::from(t1.x.saturating_sub(t0.x)) + 1;
    let high = u64::from(t1.y.saturating_sub(t0.y)) + 1;
    wide.saturating_mul(high)
}

/// How many tiles to reserve for the inclusive tile range `t0..=t1`.
///
/// Every dab reaching this has already passed [`is_paintable`], so its
/// span is at most [`MAX_DAB_TILES`] by construction and the `min` below
/// changes nothing for it. The `min` is what keeps this function's own
/// contract true when it is called directly with a range nothing
/// validated (its own test does exactly that).
fn reserved_tiles(t0: TileId, t1: TileId) -> usize {
    let planned = tile_span(t0, t1).min(MAX_DAB_TILES);
    // Capped at 4096 a line above, so this conversion always succeeds;
    // `unwrap_or` rather than an assertion because a wrong capacity hint
    // must never be what takes a professional's unsaved work down.
    usize::try_from(planned).unwrap_or(0)
}

/// The inclusive tile range a dab centered at `center` with `radius`
/// overlaps — shared by [`stamp_dab`]/[`erase_dab`]/[`touched_tiles`] so
/// the bounding-box math exists in exactly one place. Callers must have
/// checked `is_paintable` first; this doesn't special-case a dab
/// without real geometry (`min > max` would otherwise fall out, and
/// every caller here already guards it before reaching this function).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn tile_range(center: (f32, f32), radius: f32) -> (TileId, TileId) {
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    (
        TileId {
            x: min_x / TILE,
            y: min_y / TILE,
        },
        TileId {
            x: max_x / TILE,
            y: max_y / TILE,
        },
    )
}

/// Every tile a dab centered at `center` with `radius` *would* touch if
/// every one of them could be paged in, in the same row-major order
/// [`stamp_dab`]/[`erase_dab`] themselves iterate — a dab with no real
/// geometry (`is_paintable`: `radius <= 0.0`, or a non-finite
/// `center`/`radius`) yields nothing, matching both. Pure geometry, and
/// therefore an upper bound rather than a result: what a dab actually
/// painted is [`DabOutcome::painted`], which is this list minus whatever
/// [`DabOutcome::failed`] holds *and* minus every covered tile the dab
/// turned out to change nothing in.
///
/// **No longer how the brush path captures undo state** (0.55.0). It
/// used to be: `aurora-app` called this before each dab and captured
/// every listed tile via
/// [`crate::StrokeSnapshot::record_touch`], which recorded tiles the
/// dab then failed to paint — a real undo entry for pixels nothing had
/// changed. Capture now happens inside the dab itself
/// ([`crate::StrokeSnapshot::record_content`]). This stays public for
/// callers that genuinely want the geometry up front: a caller sizing
/// a buffer, or a test asserting which tiles a dab is aimed at.
#[must_use]
pub fn touched_tiles(center: (f32, f32), radius: f32) -> Vec<TileId> {
    if !is_paintable(center, radius) {
        return Vec::new();
    }
    let (t0, t1) = tile_range(center, radius);
    let mut tiles = Vec::with_capacity(reserved_tiles(t0, t1));
    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            tiles.push(TileId { x: tx, y: ty });
        }
    }
    tiles
}

/// Stamps one dab centered at `center` (surface-pixel coordinates, i.e.
/// document space — the same space [`crate::dabs_along_path`] produces)
/// with `radius` and straight-RGB `colour` into `store`'s `surface`.
///
/// Touches every tile the dab's own bounding box overlaps (up to four,
/// for a dab near a tile corner), marking each touched tile's dirty
/// rectangle so a later GPU upload only re-reads what actually changed.
///
/// **A tile that cannot be paged in costs its own 256 × 256 px and
/// nothing else** (0.55.0). This used to return the first
/// [`TileError`] out of the loop with `?`, so every tile *later* in
/// iteration order was never attempted either — a dab straddling a
/// healthy tile and a permanently broken one landed half-applied, and
/// a dragged brush left a dead zone up to 512 × 512 px wide. Now each
/// failing tile is recorded in the returned [`DabOutcome`] and the loop
/// continues; see that type for the full account.
///
/// If `snapshot` is `Some`, each tile this dab actually *changes* has
/// its pre-dab content captured into it
/// ([`crate::StrokeSnapshot::record_content`]) *in the instant before
/// that tile's first texel write* — so the tiles captured, the tiles
/// listed in [`DabOutcome::painted`], and the tiles whose pixels really
/// differ afterwards are one and the same set by construction, and a
/// dab that changed nothing captures nothing and pushes no undo entry.
/// A `snapshot` belonging to a different [`SurfaceId`] is ignored
/// rather than mis-captured, and that mismatch is logged as the caller
/// bug it would be.
///
/// A dab with no real geometry (`is_paintable`) touches nothing and
/// returns an empty, complete [`DabOutcome`]. `radius <= 0.0` is a real
/// input (e.g. a zero-pressure sample at a stroke's edge); a non-finite
/// `center`/`radius` is a caller bug, refused rather than silently
/// rounded onto tile `(0, 0)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn stamp_dab(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    colour: [f32; 3],
    snapshot: Option<&mut StrokeSnapshot>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if !is_paintable(center, radius) {
        return outcome;
    }
    let mut snapshot = usable_snapshot(snapshot, surface);
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    outcome.painted = Vec::with_capacity(reserved_tiles(t0, t1));

    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            // Continue, don't return: a tile that cannot be paged in
            // costs its own 256x256 px and nothing else. Returning here
            // skipped every *later* tile too, leaving a dab straddling a
            // broken tile half-applied and a dragged brush a dead zone up
            // to 512x512 px wide.
            let tile = match store.get_mut(surface, id) {
                Ok(tile) => tile,
                Err(err) => {
                    outcome.fail(id, err);
                    continue;
                }
            };
            let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
            let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
            let lx1 = max_x.saturating_sub(origin_x).min(TILE - 1);
            let ly1 = max_y.saturating_sub(origin_y).min(TILE - 1);
            let mut touched_this_tile = false;
            let mut capture_failed = false;
            let texels = tile.texels_mut();

            'texels: for ly in ly0..=ly1 {
                for lx in lx0..=lx1 {
                    #[allow(clippy::cast_precision_loss)]
                    let px = (origin_x + lx) as f32 + 0.5;
                    #[allow(clippy::cast_precision_loss)]
                    let py = (origin_y + ly) as f32 + 0.5;
                    let d = (px - cx).hypot(py - cy);
                    if d > radius {
                        continue;
                    }
                    // Smooth falloff; hardness is not a variable here,
                    // matching the spike this is ported from.
                    let a = (1.0 - (d / radius)).clamp(0.0, 1.0).powf(1.5);
                    if a <= 0.0 {
                        continue;
                    }
                    let index = texel_index(lx, ly);
                    let Some(current) = texels.get(index..index + CHANNELS) else {
                        continue;
                    };
                    let Some(&dst_a) = current.last() else {
                        continue;
                    };
                    // Max-alpha accumulation, this module's own rule: a
                    // dab never lowers a texel's alpha.
                    if a <= dst_a.to_f32() {
                        continue;
                    }
                    // What this texel would actually *store*, computed
                    // before anything is decided on it -- comparing the
                    // `f32` `a` against the `f16` already there answers a
                    // different question than "will this write change
                    // anything" (`stores_the_same_bits`).
                    let [red, green, blue] = colour;
                    let fresh: Texel = [
                        f16::from_f32(red * a),
                        f16::from_f32(green * a),
                        f16::from_f32(blue * a),
                        f16::from_f32(a),
                    ];
                    if stores_the_same_bits(current, fresh) {
                        continue;
                    }
                    // Every `continue` above is a texel this dab
                    // declined to write, and a tile can be made
                    // entirely of them. This line is the first moment
                    // this tile is certainly about to change, so it is
                    // where the pre-dab content is captured: after that
                    // certainty, before the write two lines down, and
                    // never for a tile that turns out to change nothing.
                    if !touched_this_tile {
                        // A refused capture must not become a painted
                        // tile with no undo entry: abandon this tile
                        // entirely instead, before its first write, and
                        // report it as failed. Unreachable from any
                        // caller in this workspace (both hand a whole
                        // `Tile::texels()` slice), which is exactly why
                        // it must not be able to quietly cost a user
                        // their undo if a future one gets it wrong.
                        if let Some(stroke) = snapshot.as_mut()
                            && !stroke.record_content(id, texels)
                        {
                            capture_failed = true;
                            break 'texels;
                        }
                        touched_this_tile = true;
                    }
                    for (channel, &value) in fresh.iter().enumerate() {
                        if let Some(sample) = texels.get_mut(index + channel) {
                            *sample = value;
                        }
                    }
                }
            }
            if capture_failed {
                outcome.fail(id, malformed_tile(surface, id, texels.len()));
                continue;
            }
            if touched_this_tile {
                outcome.painted.push(id);
                tile.mark_dirty(Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                });
            }
        }
    }
    outcome
}

/// The failure a dab reports for a tile whose own texel slice is not one
/// whole tile's worth of samples, and which
/// [`crate::StrokeSnapshot::record_content`] therefore refused to
/// capture (0.57.0).
///
/// The dab abandons such a tile *before* its first write rather than
/// painting it uncapturable, so [`DabOutcome::failed`]'s own "left
/// completely untouched" contract still holds and "captured and painted
/// are the same set" stays true by construction.
///
/// A dedicated [`TileError::MalformedTile`] rather than the existing
/// [`TileError::CorruptFile`]: no file is involved — the tile is
/// resident, was paged in cleanly, and is simply the wrong length in
/// memory — and reporting a scratch-disk corruption that did not happen
/// would send whoever reads the log looking in the wrong place.
fn malformed_tile(surface: SurfaceId, id: TileId, samples: usize) -> TileError {
    TileError::MalformedTile {
        surface,
        id,
        samples,
        expected: SAMPLES,
    }
}

/// The `snapshot` a dab may actually capture into: `None` unless it
/// belongs to the surface being painted.
///
/// A snapshot for some other surface would capture the wrong pixels
/// entirely, so it is dropped rather than mis-captured. `App` cannot
/// reach this — its snapshot is built from the same active layer that
/// supplies `surface`, and replacing the document clears the drag — but
/// nothing in the types says so, and a silent drop would leave a stroke
/// quietly unundoable with no signal anywhere. `error!`, not `warn!`:
/// this is a programming error in the caller, not a runtime condition
/// like a scratch disk going away.
fn usable_snapshot(
    snapshot: Option<&mut StrokeSnapshot>,
    surface: SurfaceId,
) -> Option<&mut StrokeSnapshot> {
    let stroke = snapshot?;
    if stroke.surface() == surface {
        return Some(stroke);
    }
    tracing::error!(
        snapshot_surface = ?stroke.surface(),
        dab_surface = ?surface,
        "ignoring a stroke snapshot built for a different surface than the dab is painting; \
         this dab will not be undoable"
    );
    None
}

/// Erases within one dab centered at `center` (surface-pixel
/// coordinates, same space [`stamp_dab`] uses), by the same smooth
/// radial falloff `stamp_dab` computes for `a`, into `store`'s
/// `surface` — PLAN.md M1.9's "basic brush and eraser" bullet's other
/// half. **Subtractive, not blended**: reduces existing alpha
/// multiplicatively toward zero (`new_alpha = dst_alpha * (1.0 - a)`)
/// rather than painting a `colour` in; a dab centered on an already-
/// opaque texel erases it outright (`a` reaches `1.0` at dead center),
/// while one at the falloff's edge only thins it. Leaves RGB
/// untouched — this store's alpha is straight (unassociated), so a
/// channel behind a fully-erased pixel carries no meaning until
/// something is painted there again.
///
/// Touches the same tiles [`stamp_dab`] would for the same
/// `center`/`radius`, marking each touched tile's dirty rectangle so a
/// later GPU upload only re-reads what actually changed. A texel
/// already fully transparent (`dst_alpha <= 0.0`) is skipped rather
/// than marked dirty, since erasing it further is a no-op.
///
/// Reports what it managed to do through the same [`DabOutcome`]
/// [`stamp_dab`] returns, under the same 0.55.0 rule: a tile that
/// cannot be paged in is recorded in [`DabOutcome::failed`] and the
/// loop continues to the next tile rather than abandoning every tile
/// after it. `snapshot` is captured on exactly the same terms too — in
/// the instant before each tile's first real alpha write, so an erase
/// that found nothing to erase (every covered texel already fully
/// transparent) captures nothing and reports nothing painted — and
/// ignored, with an `error!`, if it belongs to another [`SurfaceId`].
///
/// A dab with no real geometry (`is_paintable`) touches nothing and
/// returns an empty, complete [`DabOutcome`], matching [`stamp_dab`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn erase_dab(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    snapshot: Option<&mut StrokeSnapshot>,
) -> DabOutcome {
    let mut outcome = DabOutcome::default();
    if !is_paintable(center, radius) {
        return outcome;
    }
    // Same surface guard `stamp_dab` applies, for the same reason.
    let mut snapshot = usable_snapshot(snapshot, surface);
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    outcome.painted = Vec::with_capacity(reserved_tiles(t0, t1));

    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            // `continue`, not `return` -- `stamp_dab`'s own rule, and
            // the same reasoning, mirrored.
            let tile = match store.get_mut(surface, id) {
                Ok(tile) => tile,
                Err(err) => {
                    outcome.fail(id, err);
                    continue;
                }
            };
            let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
            let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
            let lx1 = max_x.saturating_sub(origin_x).min(TILE - 1);
            let ly1 = max_y.saturating_sub(origin_y).min(TILE - 1);
            let mut touched_this_tile = false;
            let mut capture_failed = false;
            let texels = tile.texels_mut();

            'texels: for ly in ly0..=ly1 {
                for lx in lx0..=lx1 {
                    #[allow(clippy::cast_precision_loss)]
                    let px = (origin_x + lx) as f32 + 0.5;
                    #[allow(clippy::cast_precision_loss)]
                    let py = (origin_y + ly) as f32 + 0.5;
                    let d = (px - cx).hypot(py - cy);
                    if d > radius {
                        continue;
                    }
                    let a = (1.0 - (d / radius)).clamp(0.0, 1.0).powf(1.5);
                    if a <= 0.0 {
                        continue;
                    }
                    let index = texel_index(lx, ly);
                    let Some(&stored_a) = texels.get(index + ALPHA) else {
                        continue;
                    };
                    let dst_a = stored_a.to_f32();
                    if dst_a <= 0.0 {
                        continue;
                    }
                    // The value that would really be stored, against the
                    // value really there -- `stamp_dab`'s own
                    // `stores_the_same_bits` rule, in the one-channel
                    // form an erase needs. Out near the falloff's own
                    // edge `1.0 - a` is within half an `f16` ulp of
                    // `1.0`, so the multiply lands back on the very bits
                    // already stored: a texel the old `dst_a <= 0.0`
                    // guard called changed, marked the tile dirty and
                    // captured an undo entry for, having changed nothing.
                    let fresh_a = f16::from_f32(dst_a * (1.0 - a));
                    if fresh_a.to_bits() == stored_a.to_bits() {
                        continue;
                    }
                    // Captured here, not before the loop --
                    // `stamp_dab`'s own "capture at the first real
                    // write, never for a tile nothing changed in" rule,
                    // mirrored. An erase over already-transparent
                    // pixels reaches neither this line nor the write
                    // below.
                    if !touched_this_tile {
                        // `stamp_dab`'s own refused-capture rule,
                        // mirrored: no write, no `painted` entry, a
                        // reported failure instead.
                        if let Some(stroke) = snapshot.as_mut()
                            && !stroke.record_content(id, texels)
                        {
                            capture_failed = true;
                            break 'texels;
                        }
                        touched_this_tile = true;
                    }
                    if let Some(sample) = texels.get_mut(index + ALPHA) {
                        *sample = fresh_a;
                    }
                }
            }
            if capture_failed {
                outcome.fail(id, malformed_tile(surface, id, texels.len()));
                continue;
            }
            if touched_this_tile {
                outcome.painted.push(id);
                tile.mark_dirty(Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                });
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::{erase_dab, stamp_dab, touched_tiles};
    use crate::test_support::{store_with_a_broken_tile, surface};
    use crate::{PixelHistory, StrokeSnapshot};
    use aurora_tile::{SurfaceId, TileError, TileId, TileStore};
    use half::f16;
    use std::num::NonZeroUsize;

    fn store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let Some(budget) = NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
        };
        (dir, store)
    }

    #[test]
    fn zero_radius_touches_nothing() {
        let (_dir, mut store) = store();
        let outcome = stamp_dab(
            &mut store,
            surface(),
            (10.0, 10.0),
            0.0,
            [1.0, 0.0, 0.0],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a zero-radius dab covers no tile at all"
        );
        assert_eq!(outcome.painted().len(), 0);
        assert_eq!(store.resident_len(), 0, "must not even touch a tile");
    }

    #[test]
    fn a_dab_away_from_any_tile_boundary_touches_one_tile() {
        let (_dir, mut store) = store();
        let outcome = stamp_dab(
            &mut store,
            surface(),
            (128.0, 128.0),
            20.0,
            [1.0, 0.0, 0.0],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert_eq!(outcome.painted().len(), 1);
    }

    #[test]
    fn a_dab_centered_on_a_tile_corner_touches_four_tiles() {
        let (_dir, mut store) = store();
        // TILE is 256; a dab centered exactly on the (256, 256) corner
        // with enough radius spills into all four neighbouring tiles.
        let outcome = stamp_dab(
            &mut store,
            surface(),
            (256.0, 256.0),
            20.0,
            [1.0, 0.0, 0.0],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert_eq!(outcome.painted().len(), 4);
    }

    #[test]
    fn touched_tiles_of_a_zero_radius_dab_is_empty() {
        assert_eq!(touched_tiles((10.0, 10.0), 0.0), []);
    }

    #[test]
    fn touched_tiles_matches_stamp_dabs_own_tile_count_away_from_a_boundary() {
        assert_eq!(touched_tiles((128.0, 128.0), 20.0), [TileId { x: 0, y: 0 }]);
    }

    #[test]
    fn touched_tiles_matches_stamp_dabs_own_tile_count_at_a_corner() {
        let tiles = touched_tiles((256.0, 256.0), 20.0);
        assert_eq!(
            tiles.len(),
            4,
            "must agree with stamp_dab's own corner test"
        );
        for expected in [
            TileId { x: 0, y: 0 },
            TileId { x: 1, y: 0 },
            TileId { x: 0, y: 1 },
            TileId { x: 1, y: 1 },
        ] {
            assert!(
                tiles.contains(&expected),
                "missing {expected:?} in {tiles:?}"
            );
        }
    }

    #[test]
    fn the_center_texel_is_opaque_and_the_right_colour() {
        let (_dir, mut store) = store();
        // A generous radius (20) relative to the roughly-half-texel
        // distance from the dab's own center (10, 10) to that texel's
        // sample point (10.5, 10.5) keeps the falloff curve's own alpha
        // safely above 0.9 here -- the exact curve isn't this test's
        // point (ported and trusted from the spike), only that a hit
        // near dead-center lands opaque and the right colour.
        assert!(
            stamp_dab(
                &mut store,
                surface(),
                (10.0, 10.0),
                20.0,
                [0.0, 1.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let Some(&r) = tile.texels().get(index) else {
            unreachable!("index is in bounds for a full tile");
        };
        let Some(&g) = tile.texels().get(index + 1) else {
            unreachable!("index is in bounds for a full tile");
        };
        let Some(&a) = tile.texels().get(index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            g.to_f32() > 0.5,
            "green channel should be clearly present: {g:?}"
        );
        assert!(r.to_f32() < 0.1, "red channel should be near zero: {r:?}");
        assert!(a.to_f32() > 0.5, "alpha should be clearly opaque: {a:?}");
    }

    #[test]
    // `b` is asserted exactly `0.0` because the second dab's own write
    // is skipped entirely (not because of accumulated float rounding),
    // so exact equality is the correct check here.
    #[allow(clippy::float_cmp)]
    fn overlapping_dabs_use_max_alpha_not_source_over() {
        let (_dir, mut store) = store();
        let center = (10.0, 10.0);
        // First dab: a large radius, so the center texel's falloff alpha
        // is high. Second dab: a much smaller radius at the *same*
        // center, so its own falloff alpha at that same texel is clearly
        // lower -- comfortably outside any f16-rounding ambiguity a
        // near-equal comparison would risk. Source-over would still
        // blend the second colour in on top; max-alpha accumulation
        // (this module's own, spike-proven choice) must leave the
        // first dab's higher-alpha colour untouched.
        assert!(
            stamp_dab(&mut store, surface(), center, 20.0, [1.0, 0.0, 0.0], None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert!(
            stamp_dab(&mut store, surface(), center, 2.0, [0.0, 0.0, 1.0], None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let Some(&r) = tile.texels().get(index) else {
            unreachable!("index is in bounds for a full tile");
        };
        let Some(&b) = tile.texels().get(index + 2) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            r.to_f32() > 0.5,
            "the first (red) dab's higher alpha must survive: {r:?}"
        );
        assert_eq!(
            b.to_f32(),
            0.0,
            "the second (blue) dab's lower alpha must not have overwritten it"
        );
    }

    #[test]
    fn stamping_two_different_surfaces_does_not_cross_contaminate() {
        let (_dir, mut store) = store();
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        assert!(
            stamp_dab(
                &mut store,
                surface_a,
                (10.0, 10.0),
                8.0,
                [1.0, 0.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile_b = match store.get(surface_b, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            tile_b.texels().iter().all(|s| s.to_f32() == 0.0),
            "surface_b must be untouched by a dab stamped on surface_a"
        );
    }

    #[test]
    fn zero_radius_erase_touches_nothing() {
        let (_dir, mut store) = store();
        let outcome = erase_dab(&mut store, surface(), (10.0, 10.0), 0.0, None);
        assert!(
            outcome.is_complete(),
            "a zero-radius erase covers no tile at all"
        );
        assert_eq!(outcome.painted().len(), 0);
        assert_eq!(store.resident_len(), 0, "must not even touch a tile");
    }

    #[test]
    fn erasing_an_untouched_pixel_is_a_no_op() {
        let (_dir, mut store) = store();
        assert!(
            erase_dab(&mut store, surface(), (10.0, 10.0), 20.0, None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            tile.texels().iter().all(|s| s.to_f32() == 0.0),
            "erasing already-transparent pixels must leave them exactly as they were"
        );
    }

    #[test]
    fn erase_dab_centered_on_an_opaque_texel_erases_it_outright() {
        let (_dir, mut store) = store();
        // (10.5, 10.5) lands exactly on texel (10, 10)'s own sample
        // point (`stamp_dab`'s `px`/`py` add 0.5), so `d == 0` and the
        // falloff `a` is exactly `1.0` -- genuinely dead center, not
        // just close to it.
        assert!(
            stamp_dab(
                &mut store,
                surface(),
                (10.5, 10.5),
                20.0,
                [1.0, 0.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert!(
            erase_dab(&mut store, surface(), (10.5, 10.5), 20.0, None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let Some(&a) = tile.texels().get(index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            a.to_f32() < 0.01,
            "a dab centered dead-on an opaque texel (falloff a == 1.0) must erase it to ~0: {a:?}"
        );
    }

    #[test]
    fn erase_dab_leaves_rgb_untouched() {
        let (_dir, mut store) = store();
        assert!(
            stamp_dab(
                &mut store,
                surface(),
                (10.0, 10.0),
                20.0,
                [0.0, 1.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert!(
            erase_dab(&mut store, surface(), (10.0, 10.0), 20.0, None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let Some(&g) = tile.texels().get(index + 1) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            g.to_f32() > 0.5,
            "erasing must reduce alpha only, leaving the RGB channels as they were: {g:?}"
        );
    }

    #[test]
    fn erase_dab_at_the_falloff_edge_only_thins_a_texel_not_erase_it_outright() {
        let (_dir, mut store) = store();
        assert!(
            stamp_dab(
                &mut store,
                surface(),
                (10.0, 10.0),
                20.0,
                [1.0, 0.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let before = {
            let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(&a) = tile.texels().get(index + 3) else {
                unreachable!("index is in bounds for a full tile");
            };
            a.to_f32()
        };
        // A small eraser radius centered a few pixels away from (10, 10)
        // reaches it only near the falloff's own edge (small `a`), so it
        // should thin rather than fully clear it.
        assert!(
            erase_dab(&mut store, surface(), (13.0, 10.0), 4.0, None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(&after) = tile.texels().get(index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        let after = after.to_f32();
        assert!(
            after < before,
            "must have thinned the alpha at all: {after} vs {before}"
        );
        assert!(
            after > 0.1,
            "an edge-of-falloff erase must not fully clear the texel: {after}"
        );
    }

    #[test]
    fn erasing_two_different_surfaces_does_not_cross_contaminate() {
        let (_dir, mut store) = store();
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        assert!(
            stamp_dab(
                &mut store,
                surface_b,
                (10.0, 10.0),
                8.0,
                [1.0, 0.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        assert!(
            erase_dab(&mut store, surface_a, (10.0, 10.0), 8.0, None).is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let tile_b = match store.get(surface_b, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (10 * aurora_tile::TILE + 10) as usize * aurora_tile::CHANNELS;
        let Some(&a) = tile_b.texels().get(index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            a.to_f32() > 0.5,
            "erasing surface_a must not have touched surface_b's own opaque pixel: {a:?}"
        );
    }

    /// Tile-local texel `(lx, ly)`'s alpha on `tile` — how these tests
    /// tell "actually painted" apart from merely "listed as painted".
    fn alpha_at(store: &mut TileStore, tile: TileId, lx: u32, ly: u32) -> f32 {
        let t = match store.get(surface(), tile) {
            Ok(t) => t,
            Err(err) => unreachable!("{tile:?} must be readable here: {err:?}"),
        };
        let Some(&a) = t.texels().get(super::texel_index(lx, ly) + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        a.to_f32()
    }

    /// Sets every texel's alpha on `tile` to `value`, leaving RGB alone —
    /// how these tests manufacture an "already opaque" tile without
    /// depending on the falloff curve's own arithmetic.
    fn fill_alpha(store: &mut TileStore, tile: TileId, value: f32) {
        let Ok(t) = store.get_mut(surface(), tile) else {
            unreachable!("a real store must accept this write");
        };
        for sample in t.texels_mut().iter_mut().skip(3).step_by(4) {
            *sample = f16::from_f32(value);
        }
    }

    /// The tiles `stroke` captured, in `DabOutcome::painted`'s own
    /// row-major order, so the two can be compared directly instead of
    /// inferred from whether a later undo happened to succeed.
    fn captured_in_order(stroke: &StrokeSnapshot) -> Vec<TileId> {
        let mut tiles: Vec<TileId> = stroke.captured().collect();
        tiles.sort_unstable_by_key(|tile| (tile.y, tile.x));
        tiles
    }

    /// A dab centered here spans exactly tiles (0, 0) and (1, 0), in
    /// that iteration order — the two-tile geometry every broken-tile
    /// test below is built on. The half-texel offsets put the dab's own
    /// centre exactly on a texel sample point (`px`/`py` add 0.5), so
    /// the falloff `a` there is exactly `1.0`: paint lands fully opaque
    /// and an eraser clears it outright, which is what lets these tests
    /// assert on real pixel values rather than on curve arithmetic.
    const SPANNING_DAB: (f32, f32) = (256.5, 128.5);
    /// A dab centered here stays entirely inside tile (0, 0).
    const SINGLE_TILE_DAB: (f32, f32) = (128.0, 128.0);
    const DAB_RADIUS: f32 = 24.0;

    /// A dab straddling a permanently unreadable tile and a healthy one
    /// must still paint the healthy one. Until 0.55.0 `stamp_dab`
    /// returned the first `TileError` out of its loop with `?`, so every
    /// tile *later* in iteration order — healthy or not — was never even
    /// attempted: a half-applied dab, and a dead zone up to 512 x 512 px
    /// under a dragged brush.
    ///
    /// The broken tile is (0, 0), i.e. *first* in iteration order, which
    /// is the case the old code got wrong.
    #[test]
    fn a_dab_spanning_a_broken_tile_still_paints_the_healthy_tile_after_it() {
        let broken = TileId { x: 0, y: 0 };
        let healthy = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        let outcome = stamp_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );

        assert_eq!(
            outcome.painted(),
            [healthy],
            "the healthy tile after the broken one must still have been painted"
        );
        match outcome.failed() {
            [(tile, _)] => assert_eq!(*tile, broken, "only the broken tile may fail"),
            other => unreachable!("exactly one tile must fail, got {other:?}"),
        }
        // Not just listed as painted -- really painted. (0, 128) in tile
        // (1, 0) is half a texel from the dab's own center.
        assert!(
            alpha_at(&mut store, healthy, 0, 128) > 0.5,
            "the healthy tile must carry real paint, not merely be listed"
        );
    }

    /// The mirror of the test above, with the *healthy* tile first in
    /// iteration order — proof the fix isn't accidentally order-dependent
    /// the other way (e.g. by painting only up to the first failure).
    #[test]
    fn a_dab_spanning_a_broken_tile_still_paints_the_healthy_tile_before_it() {
        let healthy = TileId { x: 0, y: 0 };
        let broken = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        let outcome = stamp_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );

        assert_eq!(outcome.painted(), [healthy]);
        match outcome.failed() {
            [(tile, _)] => assert_eq!(*tile, broken, "only the broken tile may fail"),
            other => unreachable!("exactly one tile must fail, got {other:?}"),
        }
        assert!(
            alpha_at(&mut store, healthy, 255, 128) > 0.5,
            "the healthy tile before the broken one must carry real paint"
        );
    }

    /// `DabOutcome` has to distinguish three states the old
    /// `Result<usize, TileError>` collapsed into two.
    #[test]
    fn a_dab_outcome_tells_total_failure_partial_success_and_a_clean_dab_apart() {
        // (a) Nothing broken: every covered tile painted.
        {
            let (_dir, mut store) = store();
            let outcome = stamp_dab(
                &mut store,
                surface(),
                (256.0, 256.0),
                DAB_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            );
            assert_eq!(outcome.painted().len(), 4);
            assert_eq!(outcome.failed().len(), 0);
            assert!(outcome.is_complete());
            assert!(outcome.first_error().is_none());
        }

        // (b) Partial: one tile lost, one painted.
        {
            let broken = TileId { x: 0, y: 0 };
            let healthy = TileId { x: 1, y: 0 };
            let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);
            let outcome = stamp_dab(
                &mut store,
                surface(),
                SPANNING_DAB,
                DAB_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            );
            assert_eq!(outcome.painted().len(), 1);
            assert_eq!(outcome.failed().len(), 1);
            assert!(!outcome.is_complete());
        }

        // (c) Total: the only covered tile is the broken one.
        {
            let broken = TileId { x: 0, y: 0 };
            let healthy = TileId { x: 1, y: 0 };
            let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);
            let outcome = stamp_dab(
                &mut store,
                surface(),
                SINGLE_TILE_DAB,
                DAB_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            );
            assert!(outcome.painted().is_empty());
            assert_eq!(outcome.failed().len(), 1);
            assert!(!outcome.is_complete());
            // `matches!`, not equality: `TileError` is deliberately not
            // `Clone`/`PartialEq` (it carries an `io::Error`).
            let Some(err) = outcome.first_error() else {
                unreachable!("a failed dab must carry its own error");
            };
            assert!(
                matches!(err, TileError::CorruptFile(_)),
                "a truncated scratch file must surface as CorruptFile: {err:?}"
            );
        }
    }

    /// A dab that painted nothing must capture nothing, and therefore
    /// leave the undo stack completely alone. Before 0.55.0 `App`
    /// captured every tile `touched_tiles` listed *before* stamping, so
    /// a dab whose paint then failed still produced a real undo entry
    /// covering pixels nothing had changed — and, worse, an entry whose
    /// own restore could never succeed.
    #[test]
    fn a_dab_entirely_on_a_broken_tile_captures_nothing_and_pushes_no_undo_entry() {
        let broken = TileId { x: 0, y: 0 };
        let healthy = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = stamp_dab(
            &mut store,
            surface(),
            SINGLE_TILE_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );

        assert!(outcome.painted().is_empty());
        assert!(
            stroke.is_empty(),
            "a dab that painted nothing must have captured nothing"
        );
        assert_eq!(
            captured_in_order(&stroke),
            outcome.painted(),
            "captured and painted must be the same set -- asserted directly, not inferred \
             from a later undo's own result"
        );
        let mut history = PixelHistory::new();
        assert!(
            !history.push(stroke),
            "an empty snapshot must not become an undo entry"
        );
        assert!(!history.can_undo());
    }

    /// The partial case's undo entry must cover exactly the tiles the dab
    /// really painted. The load-bearing assertion is that `undo` returns
    /// `Ok(true)`: `StrokeSnapshot::apply`'s phase-one read touches every
    /// captured tile before writing anything, so had the broken tile been
    /// captured, this undo would have failed with `Err` instead.
    #[test]
    fn a_partially_failed_dab_records_an_undo_entry_covering_only_the_tiles_it_painted() {
        let broken = TileId { x: 0, y: 0 };
        let healthy = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = stamp_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );
        assert_eq!(outcome.painted(), [healthy]);
        assert!(
            !stroke.is_empty(),
            "the painted tile must have been captured"
        );
        assert_eq!(
            captured_in_order(&stroke),
            outcome.painted(),
            "the snapshot must hold exactly the painted tile -- not the broken one, and not \
             nothing"
        );
        assert!(
            alpha_at(&mut store, healthy, 0, 128) > 0.5,
            "setup: the healthy tile must actually carry paint before the undo"
        );

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));
        match history.undo(&mut store) {
            Ok(undone) => assert!(undone, "there was an entry to undo"),
            Err(err) => unreachable!(
                "the snapshot must not have captured the broken tile, so this undo must \
                 succeed: {err:?}"
            ),
        }
        assert!(
            alpha_at(&mut store, healthy, 0, 128) < 0.01,
            "undo must restore the painted tile to its pre-dab transparency"
        );
    }

    /// `erase_dab`'s mirror of
    /// `a_dab_spanning_a_broken_tile_still_paints_the_healthy_tile_after_it`.
    #[test]
    fn an_erase_dab_spanning_a_broken_tile_still_erases_the_healthy_tile_after_it() {
        let broken = TileId { x: 0, y: 0 };
        let healthy = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        // Paint the healthy tile first -- erasing an already-transparent
        // texel is a documented no-op, so there would be nothing to see.
        let painted = stamp_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );
        assert_eq!(painted.painted(), [healthy], "setup");
        assert!(alpha_at(&mut store, healthy, 0, 128) > 0.5, "setup");

        let outcome = erase_dab(&mut store, surface(), SPANNING_DAB, DAB_RADIUS, None);

        assert_eq!(
            outcome.painted(),
            [healthy],
            "the healthy tile after the broken one must still have been erased"
        );
        match outcome.failed() {
            [(tile, _)] => assert_eq!(*tile, broken),
            other => unreachable!("exactly one tile must fail, got {other:?}"),
        }
        assert!(
            alpha_at(&mut store, healthy, 0, 128) < 0.01,
            "the healthy tile must really have been erased, not merely listed"
        );
    }

    /// `erase_dab`'s mirror of
    /// `a_partially_failed_dab_records_an_undo_entry_covering_only_the_tiles_it_painted`.
    #[test]
    fn a_partially_failed_erase_dab_records_an_undo_entry_covering_only_what_it_erased() {
        let broken = TileId { x: 0, y: 0 };
        let healthy = TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_a_broken_tile(broken, healthy);

        let painted = stamp_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );
        assert_eq!(painted.painted(), [healthy], "setup");
        let before = alpha_at(&mut store, healthy, 0, 128);
        assert!(before > 0.5, "setup");

        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = erase_dab(
            &mut store,
            surface(),
            SPANNING_DAB,
            DAB_RADIUS,
            Some(&mut stroke),
        );
        assert_eq!(outcome.painted(), [healthy]);
        assert_eq!(outcome.failed().len(), 1);
        assert!(!stroke.is_empty());
        assert_eq!(
            captured_in_order(&stroke),
            outcome.painted(),
            "the snapshot must hold exactly the erased tile, not the broken one"
        );
        assert!(
            alpha_at(&mut store, healthy, 0, 128) < 0.01,
            "setup: erased"
        );

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));
        match history.undo(&mut store) {
            Ok(undone) => assert!(undone),
            Err(err) => {
                unreachable!("the snapshot must not have captured the broken tile: {err:?}")
            }
        }
        assert!(
            alpha_at(&mut store, healthy, 0, 128) > 0.5,
            "undo must put the erased paint back: {before}"
        );
    }

    /// The happy path of the new in-dab capture: every tile a healthy
    /// corner dab paints is captured, and undo restores all four.
    #[test]
    fn a_dab_on_a_healthy_store_captures_every_tile_it_paints() {
        let (_dir, mut store) = store();
        let corner = (256.0, 256.0);
        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = stamp_dab(
            &mut store,
            surface(),
            corner,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );
        assert_eq!(outcome.painted().len(), 4);
        assert!(outcome.is_complete());
        assert_eq!(
            captured_in_order(&stroke),
            outcome.painted(),
            "every painted tile, and only those, must have been captured"
        );

        // One texel per tile, each half a texel from the dab's centre.
        let probes = [
            (TileId { x: 0, y: 0 }, 255, 255),
            (TileId { x: 1, y: 0 }, 0, 255),
            (TileId { x: 0, y: 1 }, 255, 0),
            (TileId { x: 1, y: 1 }, 0, 0),
        ];
        for (tile, lx, ly) in probes {
            assert!(
                alpha_at(&mut store, tile, lx, ly) > 0.5,
                "setup: {tile:?} must carry paint"
            );
        }

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));
        match history.undo(&mut store) {
            Ok(undone) => assert!(undone),
            Err(err) => unreachable!("a healthy store must undo cleanly: {err:?}"),
        }
        for (tile, lx, ly) in probes {
            assert!(
                alpha_at(&mut store, tile, lx, ly) < 0.01,
                "undo must restore {tile:?} to full transparency"
            );
        }
    }

    /// **First capture wins, across dabs.** Two disjoint dabs in one
    /// stroke, both landing in the same tile: undo must restore that
    /// tile to how it looked before the *first* of them, not before the
    /// second.
    ///
    /// This is the whole reason `StrokeSnapshot::record_content` uses
    /// `entry().or_insert_with()` rather than `insert()`, and until
    /// 0.56.0 nothing tested it — an independent review mutated that one
    /// call to an unconditional `insert` and the entire suite still
    /// passed. With that mutation, the second dab's capture records the
    /// first dab's paint as if it were the pre-stroke state, so the undo
    /// below leaves the first dab's pixels on the canvas forever. The
    /// first assertion after the undo is the one that catches it.
    #[test]
    fn two_dabs_in_one_stroke_on_one_tile_undo_to_the_state_before_the_first() {
        let (_dir, mut store) = store();
        let tile = TileId { x: 0, y: 0 };
        // Two centres 240 px apart with a 24 px radius: comfortably
        // disjoint, comfortably inside the same tile.
        let first = (30.5, 30.5);
        let second = (200.5, 200.5);

        let mut stroke = StrokeSnapshot::new(surface());
        let one = stamp_dab(
            &mut store,
            surface(),
            first,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );
        assert_eq!(one.painted(), [tile], "setup: the first dab paints it");
        let two = stamp_dab(
            &mut store,
            surface(),
            second,
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );
        assert_eq!(
            two.painted(),
            [tile],
            "setup: the second dab paints the same tile again"
        );
        assert_eq!(
            captured_in_order(&stroke),
            [tile],
            "one tile, captured once -- not twice, and not re-captured"
        );
        assert!(alpha_at(&mut store, tile, 30, 30) > 0.5, "setup");
        assert!(alpha_at(&mut store, tile, 200, 200) > 0.5, "setup");

        let mut history = PixelHistory::new();
        assert!(history.push(stroke));
        match history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }

        assert!(
            alpha_at(&mut store, tile, 30, 30) < 0.01,
            "the FIRST dab's own pixels must be gone too -- a later dab re-capturing the tile \
             would have recorded them as the stroke's starting point and stranded them here"
        );
        assert!(
            alpha_at(&mut store, tile, 200, 200) < 0.01,
            "and the second dab's, obviously"
        );
    }

    /// A dab whose bounding box clamps onto a real tile but whose every
    /// texel is outside the falloff radius: the tile is acquired, and
    /// then nothing at all happens to it. Until 0.56.0 that still
    /// produced a captured tile and a full undo entry, because capture
    /// and `painted` both fired on the `get_mut` success arm rather than
    /// on a write.
    #[test]
    fn a_dab_that_acquires_a_tile_but_writes_no_texel_paints_and_captures_nothing() {
        let (_dir, mut store) = store();
        let mut stroke = StrokeSnapshot::new(surface());
        // `tile_range` clamps a negative bounding box up to tile (0, 0),
        // so that tile is genuinely paged in -- but its nearest texel
        // sample point is ~142 px from this centre, far outside a 24 px
        // radius.
        let outcome = stamp_dab(
            &mut store,
            surface(),
            (-100.0, -100.0),
            DAB_RADIUS,
            [1.0, 0.0, 0.0],
            Some(&mut stroke),
        );

        assert!(outcome.is_complete(), "the tile was acquired without error");
        assert_eq!(store.resident_len(), 1, "setup: it really was acquired");
        assert!(
            outcome.painted().is_empty(),
            "acquiring a tile is not painting it"
        );
        assert_eq!(
            captured_in_order(&stroke),
            [],
            "and not capturing it either"
        );
        let mut history = PixelHistory::new();
        assert!(
            !history.push(stroke),
            "a dab that changed no pixel must leave no undo entry behind"
        );
    }

    /// The max-alpha rule's own no-op: a dab landing entirely on pixels
    /// already at alpha `1.0` skips every texel (`a <= dst_a`), so it
    /// must report nothing painted and capture nothing.
    #[test]
    fn a_dab_landing_entirely_on_opaque_pixels_paints_and_captures_nothing() {
        let (_dir, mut store) = store();
        let tile = TileId { x: 0, y: 0 };
        fill_alpha(&mut store, tile, 1.0);

        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = stamp_dab(
            &mut store,
            surface(),
            SINGLE_TILE_DAB,
            DAB_RADIUS,
            [0.0, 1.0, 0.0],
            Some(&mut stroke),
        );

        assert!(outcome.is_complete());
        assert!(
            outcome.painted().is_empty(),
            "the falloff never exceeds 1.0, so not one texel can change"
        );
        assert_eq!(captured_in_order(&stroke), []);
        let mut history = PixelHistory::new();
        assert!(!history.push(stroke));
    }

    /// `erase_dab`'s own version of the same: erasing already-transparent
    /// pixels is a documented no-op, so it must also be an undo no-op.
    #[test]
    fn an_erase_over_already_transparent_pixels_erases_and_captures_nothing() {
        let (_dir, mut store) = store();
        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = erase_dab(
            &mut store,
            surface(),
            SINGLE_TILE_DAB,
            DAB_RADIUS,
            Some(&mut stroke),
        );

        assert!(outcome.is_complete());
        assert!(outcome.painted().is_empty());
        assert_eq!(captured_in_order(&stroke), []);
        let mut history = PixelHistory::new();
        assert!(!history.push(stroke));
    }

    /// A non-finite `center` or `radius` is a caller bug, and `as u32`
    /// saturates rather than trapping — so before 0.56.0 a NaN centre
    /// quietly landed the dab on tile (0, 0) and an infinite radius
    /// walked the entire `u32` tile grid. Both are refused outright now,
    /// and `touched_tiles` agrees with them.
    #[test]
    fn a_dab_with_a_non_finite_center_or_radius_touches_nothing() {
        let (_dir, mut store) = store();
        for center in [
            (f32::NAN, 10.0),
            (10.0, f32::NAN),
            (f32::INFINITY, 10.0),
            (10.0, f32::NEG_INFINITY),
        ] {
            let outcome = stamp_dab(
                &mut store,
                surface(),
                center,
                DAB_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            );
            assert!(outcome.is_complete(), "{center:?}");
            assert!(outcome.painted().is_empty(), "{center:?}");
            assert_eq!(touched_tiles(center, DAB_RADIUS), [], "{center:?}");
        }
        for radius in [f32::NAN, f32::INFINITY] {
            assert!(
                stamp_dab(
                    &mut store,
                    surface(),
                    (10.0, 10.0),
                    radius,
                    [1.0, 0.0, 0.0],
                    None
                )
                .painted()
                .is_empty(),
                "{radius}"
            );
            assert!(
                erase_dab(&mut store, surface(), (10.0, 10.0), radius, None)
                    .painted()
                    .is_empty(),
                "{radius}"
            );
            assert_eq!(touched_tiles((10.0, 10.0), radius), [], "{radius}");
        }
        assert_eq!(
            store.resident_len(),
            0,
            "a dab with no real geometry must not even acquire a tile"
        );
    }

    /// The capacity hint a dab reserves for its own `painted` list is
    /// derived from caller-supplied floats. Computed as `u32` — as it was
    /// until 0.56.0 — the product of the two sides panics on overflow in
    /// a debug build and, with no `overflow-checks` in release, wraps to
    /// a nonsense capacity instead. Neither is acceptable in a workspace
    /// that denies `panic` because a panic loses unsaved work.
    #[test]
    fn reserving_capacity_for_an_absurd_tile_range_neither_overflows_nor_asks_for_the_moon() {
        assert_eq!(
            super::reserved_tiles(TileId { x: 0, y: 0 }, TileId { x: 0, y: 0 }),
            1,
            "an inclusive range of one tile reserves one"
        );
        assert_eq!(
            super::reserved_tiles(TileId { x: 1, y: 2 }, TileId { x: 3, y: 5 }),
            12,
            "3 wide by 4 high"
        );
        assert_eq!(
            super::reserved_tiles(
                TileId { x: 0, y: 0 },
                TileId {
                    x: u32::MAX,
                    y: u32::MAX
                }
            ),
            4096,
            "the whole u32 grid must saturate to the cap, not overflow and not try to allocate \
             18 exabytes"
        );
    }

    /// **The phantom this whole round exists to kill, in its last
    /// hiding place** (0.57.0). Click the brush, then click again on
    /// the exact same point with the same colour and radius: not one
    /// pixel can change, so the second click must produce no undo entry.
    ///
    /// Until 0.57.0 it produced a full one. The gate compared the `f32`
    /// falloff `a` against the *`f16`-quantized* alpha already stored
    /// and then wrote `f16::from_f32(a)`. For every texel whose `a`
    /// rounds *down* to the stored value — 812 of the 1,804 texels a
    /// 24 px dab writes, 45% of them — `a > dst_a` is true, the guard
    /// passes, and the write stores the bits already there. The tile was
    /// captured, marked dirty, recomposited and reported painted for a
    /// bit-for-bit no-op, and `Ctrl+Z` then did visibly nothing.
    #[test]
    fn a_second_identical_dab_on_the_same_spot_paints_and_captures_nothing() {
        let (_dir, mut store) = store();
        let tile = TileId { x: 0, y: 0 };
        let colour = [1.0, 0.0, 0.0];

        let first = stamp_dab(
            &mut store,
            surface(),
            SINGLE_TILE_DAB,
            DAB_RADIUS,
            colour,
            None,
        );
        assert_eq!(
            first.painted(),
            [tile],
            "setup: the first click really paints"
        );
        let before: Vec<f16> = match store.get(surface(), tile) {
            Ok(t) => t.texels().to_vec(),
            Err(err) => unreachable!("{err:?}"),
        };

        let mut stroke = StrokeSnapshot::new(surface());
        let second = stamp_dab(
            &mut store,
            surface(),
            SINGLE_TILE_DAB,
            DAB_RADIUS,
            colour,
            Some(&mut stroke),
        );

        assert!(second.is_complete());
        assert!(
            second.painted().is_empty(),
            "an identical second click cannot change a single stored bit"
        );
        assert_eq!(captured_in_order(&stroke), []);
        let mut history = PixelHistory::new();
        assert!(
            !history.push(stroke),
            "and must therefore leave no undo entry for Ctrl+Z to do nothing with"
        );
        assert!(!history.can_undo());

        let after: Vec<f16> = match store.get(surface(), tile) {
            Ok(t) => t.texels().to_vec(),
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            before
                .iter()
                .zip(after.iter())
                .all(|(b, a)| b.to_bits() == a.to_bits()),
            "and must not have written one differing bit either"
        );
    }

    /// `erase_dab`'s own half of the same bug. The eraser had no
    /// change-detection at all beyond `dst_a <= 0.0`, so out where the
    /// falloff is small enough that `dst_a * (1.0 - a)` lands back on the
    /// very `f16` bits already stored, it captured and dirtied a tile it
    /// changed nothing in.
    ///
    /// The geometry: `f16` keeps 11 significand bits, so a relative
    /// change below about `2.4e-4` rounds away entirely — which is every
    /// texel past roughly 99.6% of the falloff radius. The dab below is
    /// centred so that tile (0, 0) takes its full force while tile
    /// (1, 0) is reached *only* by that outermost band (its nearest
    /// covered texel column sits 23.95 px from the centre of a 24 px
    /// dab, giving `a` around `9.5e-5`). Tile (1, 0) must therefore be
    /// absent from `painted` and from the snapshot; before 0.57.0 it was
    /// in both.
    #[test]
    fn an_erase_that_only_grazes_a_tile_paints_and_captures_nothing_there() {
        let (_dir, mut store) = store();
        let struck = TileId { x: 0, y: 0 };
        let grazed = TileId { x: 1, y: 0 };
        fill_alpha(&mut store, struck, 0.5);
        fill_alpha(&mut store, grazed, 0.5);
        let before = alpha_at(&mut store, grazed, 0, 128);

        let mut stroke = StrokeSnapshot::new(surface());
        let outcome = erase_dab(
            &mut store,
            surface(),
            (232.55, 128.5),
            DAB_RADIUS,
            Some(&mut stroke),
        );

        assert!(outcome.is_complete(), "both tiles were acquired cleanly");
        assert_eq!(
            outcome.painted(),
            [struck],
            "the grazed tile changes no stored bit, so only the struck one may be reported"
        );
        assert_eq!(
            captured_in_order(&stroke),
            [struck],
            "captured and painted must still be the same set"
        );
        assert!(
            alpha_at(&mut store, struck, 232, 128) < 0.01,
            "setup: the struck tile really was erased"
        );
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                alpha_at(&mut store, grazed, 0, 128),
                before,
                "and the grazed tile really is unchanged -- exact equality, because the write \
                 is skipped outright rather than rounding back by luck"
            );
        }
    }

    /// A finite radius is not a bounded one. `1e6` spans over 15 million
    /// tiles and `1e10` saturates the tile-range casts to roughly
    /// 2.8e14; 0.56.0 capped only the `Vec::with_capacity` *hint*, so
    /// the double loop still walked the whole range — an unbounded
    /// freeze on the UI thread — and `touched_tiles` still sized its own
    /// `Vec` from it, which aborts the process on allocation failure.
    /// Both are refused outright now ([`super::MAX_DAB_TILES`]).
    ///
    /// The elapsed-time assertion is the point of the test: a wrong fix
    /// here still returns an empty outcome, just slowly.
    #[test]
    fn an_absurd_but_finite_radius_is_refused_instead_of_iterated() {
        let (_dir, mut store) = store();
        let start = std::time::Instant::now();
        for radius in [1e6_f32, 1e10_f32, f32::MAX] {
            let outcome = stamp_dab(
                &mut store,
                surface(),
                (128.0, 128.0),
                radius,
                [1.0, 0.0, 0.0],
                None,
            );
            assert!(outcome.is_complete(), "{radius}");
            assert!(outcome.painted().is_empty(), "{radius}");
            assert!(
                erase_dab(&mut store, surface(), (128.0, 128.0), radius, None)
                    .painted()
                    .is_empty(),
                "{radius}"
            );
            assert_eq!(touched_tiles((128.0, 128.0), radius), [], "{radius}");
        }
        assert_eq!(
            store.resident_len(),
            0,
            "a refused dab must not page in even one tile"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "a refused dab must be refused, not iterated: {:?}",
            start.elapsed()
        );
    }

    /// The bound is on caller bugs, not on brushes: a radius far larger
    /// than any shipping editor offers must still paint normally. 1,000
    /// px is 40x `aurora-app`'s own 24 px brush and still spans only 25
    /// tiles, comfortably inside [`super::MAX_DAB_TILES`]'s own 4,096.
    #[test]
    fn a_large_but_real_radius_is_still_painted() {
        let (_dir, mut store) = store();
        let outcome = stamp_dab(
            &mut store,
            surface(),
            (128.0, 128.0),
            1000.0,
            [1.0, 0.0, 0.0],
            None,
        );
        assert!(outcome.is_complete());
        assert_eq!(
            touched_tiles((128.0, 128.0), 1000.0).len(),
            25,
            "the geometry must be accepted, not refused: a 5 x 5 block of tiles"
        );
        assert_eq!(
            outcome.painted().len(),
            22,
            "and really painted -- 22, not 25, because three tiles at the far corners of the \
             bounding box hold no texel inside the radius at all, which is exactly the \
             `acquired is not painted` rule doing its job at scale"
        );
    }

    /// `stamp_dab`'s own p99, measured twice: without an undo snapshot,
    /// and — since 0.56.0 — with a fresh one per dab, which is what
    /// production actually does.
    ///
    /// spike/FINDINGS.md's own "Recommended follow-ups" #5: p99 stroke
    /// latency measured at 9.1 ms against a real 10 ms product budget on
    /// real hardware — under 1 ms of margin — with an explicit call to
    /// "add a latency regression test in CI ... do not assume this holds
    /// as the brush engine grows." This is that test.
    ///
    /// What it measures, and what it deliberately doesn't: only
    /// `stamp_dab`'s own CPU cost against an already-resident tile — the
    /// same "CPU compositing/stamping, not disk I/O" hot path the spike
    /// named as the real bottleneck — isolated from GPU frame submission
    /// (no GPU exists at this layer) and from first-touch page-in cost
    /// (warmed up below; `aurora-tile`'s own benches already cover
    /// paging). The 10 ms *product* budget the spike measured covers the
    /// whole input-to-frame-submitted path across `aurora-gpu`/
    /// `aurora-app` too, not just this one call, so the threshold here is
    /// deliberately far more generous than 10 ms — wide enough to absorb
    /// a slow, shared CI runner (the real environment this test actually
    /// runs in, across `.github/workflows/ci.yml`'s own three-OS matrix)
    /// while still catching a genuine algorithmic regression, not a
    /// strict recreation of the spike's own end-to-end figure.
    ///
    /// **Two things it got wrong until 0.56.0.** It passed `None` for
    /// `snapshot`, while `App::paint_dab` always passes `Some(stroke)` —
    /// so the whole-tile `to_vec` a first touch now costs was invisible
    /// to the one test guarding a budget with under 1 ms of margin. And
    /// it re-stamped the *same* dab 200 times over, which the max-alpha
    /// rule turns into 200 dabs that write nothing: it was timing the
    /// skip path, not the paint path. The tile is cleared before each
    /// timed iteration now (outside the timed region), so every measured
    /// dab does a full write and, in the second loop, a full capture.
    #[test]
    fn stamp_dab_latency_stays_within_a_generous_ci_safe_budget() {
        const SAMPLES: usize = 200;
        // 20 ms: roughly 2x the real 10 ms *product* budget, and nowhere
        // near the low-microsecond costs this call actually measures on
        // real hardware (a 24 px dab touches ~2,300 texels in one tile)
        // -- generous enough to absorb CI noise while still catching a
        // real regression.
        const BUDGET_MICROS: u128 = 20_000;

        let (_dir, mut store) = store();
        let tile = TileId { x: 0, y: 0 };
        let center = (128.0, 128.0);
        let radius = 24.0; // aurora-app's own BRUSH_RADIUS/ERASER_RADIUS.
        let colour = [1.0, 0.0, 0.0];

        // Warm-up: settle the one touched tile as resident before any
        // timed iteration, so page-in cost (a separate, already-covered
        // concern) doesn't leak into this measurement.
        for _ in 0..8 {
            assert!(
                stamp_dab(&mut store, surface(), center, radius, colour, None).is_complete(),
                "a healthy store must paint every tile this dab covers"
            );
        }

        // Two runs: no snapshot (the baseline this test has always
        // measured) and a fresh snapshot per dab (what production does,
        // and the only way the first-touch capture cost lands inside the
        // timed region every time).
        for capturing in [false, true] {
            let mut micros = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                // Outside the timed region: reset the tile so the dab
                // below really writes rather than being skipped wholesale
                // by the max-alpha rule.
                fill_alpha(&mut store, tile, 0.0);
                let mut stroke = StrokeSnapshot::new(surface());
                let start = std::time::Instant::now();
                let outcome = if capturing {
                    stamp_dab(
                        &mut store,
                        surface(),
                        center,
                        radius,
                        colour,
                        Some(&mut stroke),
                    )
                } else {
                    stamp_dab(&mut store, surface(), center, radius, colour, None)
                };
                micros.push(start.elapsed().as_micros());
                // Asserted outside the timed region, but note that
                // `DabOutcome`'s own `painted` allocation *is* inside it
                // -- deliberately, since that allocation is now part of
                // what a real dab costs.
                assert_eq!(
                    outcome.painted(),
                    [tile],
                    "every timed dab must really paint, or this measures the wrong path"
                );
                assert_eq!(
                    captured_in_order(&stroke),
                    if capturing { vec![tile] } else { Vec::new() },
                    "and must really capture when a snapshot was handed to it"
                );
            }
            micros.sort_unstable();
            let Some(&p99) = micros.get(SAMPLES * 99 / 100) else {
                unreachable!("SAMPLES * 99 / 100 is always in bounds for a SAMPLES-length vec");
            };

            assert!(
                p99 <= BUDGET_MICROS,
                "stamp_dab p99 latency regressed (capturing: {capturing}): {p99} \u{b5}s against \
                 a {BUDGET_MICROS} \u{b5}s budget (spike/FINDINGS.md's own real 10 ms product \
                 budget had under 1 ms of margin)"
            );
        }
    }
}
