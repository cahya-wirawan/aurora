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

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

/// Index of tile-local pixel `(x, y)`'s first (red) channel within a
/// tile's own flat `texels()` slice.
const fn texel_index(x: u32, y: u32) -> usize {
    (y * TILE + x) as usize * CHANNELS
}

/// The inclusive tile range a dab centered at `center` with `radius`
/// overlaps — shared by [`stamp_dab`]/[`erase_dab`]/[`touched_tiles`] so
/// the bounding-box math exists in exactly one place. Callers with
/// `radius <= 0.0` should check that themselves first; this doesn't
/// special-case it (`min > max` would otherwise fall out, and every
/// caller here already guards it before reaching this function).
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

/// Every tile a dab centered at `center` with `radius` would touch, in
/// the same row-major order [`stamp_dab`]/[`erase_dab`] themselves
/// iterate — `radius <= 0.0` yields nothing, matching both. Public
/// specifically so a caller needing to know *which* tiles a dab is
/// about to change — capturing an undo snapshot beforehand
/// (`aurora_brush::undo::StrokeSnapshot::record_touch`) is the first
/// real one — doesn't have to duplicate this math.
#[must_use]
pub fn touched_tiles(center: (f32, f32), radius: f32) -> Vec<TileId> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let (t0, t1) = tile_range(center, radius);
    let mut tiles = Vec::with_capacity(((t1.x - t0.x + 1) * (t1.y - t0.y + 1)) as usize);
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
/// Returns the number of tiles touched.
///
/// `radius <= 0.0` touches nothing and returns `0` — a real input (e.g.
/// a zero-pressure sample at a stroke's edge), not just a defensive
/// check.
///
/// # Errors
///
/// Returns [`TileError`] if paging a touched tile in from the scratch
/// disk fails.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn stamp_dab(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
    colour: [f32; 3],
) -> Result<usize, TileError> {
    if radius <= 0.0 {
        return Ok(0);
    }
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    let mut touched = 0;

    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            let tile = store.get_mut(surface, id)?;
            touched += 1;

            let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
            let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
            let lx1 = max_x.saturating_sub(origin_x).min(TILE - 1);
            let ly1 = max_y.saturating_sub(origin_y).min(TILE - 1);
            let mut touched_this_tile = false;
            let texels = tile.texels_mut();

            for ly in ly0..=ly1 {
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
                    let Some(&dst_a) = texels.get(index + 3) else {
                        continue;
                    };
                    if a <= dst_a.to_f32() {
                        continue;
                    }
                    for (channel, &value) in colour.iter().enumerate() {
                        if let Some(sample) = texels.get_mut(index + channel) {
                            *sample = f16::from_f32(value * a);
                        }
                    }
                    if let Some(sample) = texels.get_mut(index + 3) {
                        *sample = f16::from_f32(a);
                    }
                    touched_this_tile = true;
                }
            }
            if touched_this_tile {
                tile.mark_dirty(Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                });
            }
        }
    }
    Ok(touched)
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
/// than marked dirty, since erasing it further is a no-op. Returns the
/// number of tiles touched.
///
/// `radius <= 0.0` touches nothing and returns `0`, matching
/// [`stamp_dab`].
///
/// # Errors
///
/// Returns [`TileError`] if paging a touched tile in from the scratch
/// disk fails.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn erase_dab(
    store: &mut TileStore,
    surface: SurfaceId,
    center: (f32, f32),
    radius: f32,
) -> Result<usize, TileError> {
    if radius <= 0.0 {
        return Ok(0);
    }
    let (cx, cy) = center;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().max(0.0) as u32;
    let max_y = (cy + radius).ceil().max(0.0) as u32;
    let (t0, t1) = tile_range(center, radius);
    let mut touched = 0;

    for ty in t0.y..=t1.y {
        for tx in t0.x..=t1.x {
            let id = TileId { x: tx, y: ty };
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            let tile = store.get_mut(surface, id)?;
            touched += 1;

            let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
            let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
            let lx1 = max_x.saturating_sub(origin_x).min(TILE - 1);
            let ly1 = max_y.saturating_sub(origin_y).min(TILE - 1);
            let mut touched_this_tile = false;
            let texels = tile.texels_mut();

            for ly in ly0..=ly1 {
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
                    let Some(&dst_a) = texels.get(index + 3) else {
                        continue;
                    };
                    let dst_a = dst_a.to_f32();
                    if dst_a <= 0.0 {
                        continue;
                    }
                    let new_a = dst_a * (1.0 - a);
                    if let Some(sample) = texels.get_mut(index + 3) {
                        *sample = f16::from_f32(new_a);
                    }
                    touched_this_tile = true;
                }
            }
            if touched_this_tile {
                tile.mark_dirty(Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                });
            }
        }
    }
    Ok(touched)
}

/// Stamps a whole stroke: [`crate::dabs_along_path`]'s own dab centers,
/// each via [`stamp_dab`], in order. The actual "wire `dabs_along_path`
/// into real dab-stamping" step `dab`'s own doc comment named.
///
/// # Errors
///
/// Returns [`TileError`] if any dab's own [`stamp_dab`] call fails —
/// stops at the first failure rather than continuing to paint into a
/// surface a scratch-disk error has already been raised against.
pub fn stamp_stroke(
    store: &mut TileStore,
    surface: SurfaceId,
    points: &[(f32, f32)],
    radius: f32,
    spacing: f32,
    colour: [f32; 3],
) -> Result<usize, TileError> {
    let mut touched = 0;
    for dab in crate::dabs_along_path(points, radius, spacing) {
        touched += stamp_dab(store, surface, dab, radius, colour)?;
    }
    Ok(touched)
}

/// Erases a whole stroke: [`crate::dabs_along_path`]'s own dab centers,
/// each via [`erase_dab`], in order — [`stamp_stroke`]'s subtractive
/// counterpart.
///
/// # Errors
///
/// Returns [`TileError`] if any dab's own [`erase_dab`] call fails —
/// stops at the first failure rather than continuing to erase a
/// surface a scratch-disk error has already been raised against.
pub fn erase_stroke(
    store: &mut TileStore,
    surface: SurfaceId,
    points: &[(f32, f32)],
    radius: f32,
    spacing: f32,
) -> Result<usize, TileError> {
    let mut touched = 0;
    for dab in crate::dabs_along_path(points, radius, spacing) {
        touched += erase_dab(store, surface, dab, radius)?;
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::{erase_dab, erase_stroke, stamp_dab, stamp_stroke, touched_tiles};
    use aurora_tile::{SurfaceId, TileId, TileStore};
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

    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    #[test]
    fn zero_radius_touches_nothing() {
        let (_dir, mut store) = store();
        let touched = match stamp_dab(&mut store, surface(), (10.0, 10.0), 0.0, [1.0, 0.0, 0.0]) {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(touched, 0);
        assert_eq!(store.resident_len(), 0, "must not even touch a tile");
    }

    #[test]
    fn a_dab_away_from_any_tile_boundary_touches_one_tile() {
        let (_dir, mut store) = store();
        let touched = match stamp_dab(&mut store, surface(), (128.0, 128.0), 20.0, [1.0, 0.0, 0.0])
        {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(touched, 1);
    }

    #[test]
    fn a_dab_centered_on_a_tile_corner_touches_four_tiles() {
        let (_dir, mut store) = store();
        // TILE is 256; a dab centered exactly on the (256, 256) corner
        // with enough radius spills into all four neighbouring tiles.
        let touched = match stamp_dab(&mut store, surface(), (256.0, 256.0), 20.0, [1.0, 0.0, 0.0])
        {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(touched, 4);
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
        if let Err(err) = stamp_dab(&mut store, surface(), (10.0, 10.0), 20.0, [0.0, 1.0, 0.0]) {
            unreachable!("{err:?}");
        }
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
        if let Err(err) = stamp_dab(&mut store, surface(), center, 20.0, [1.0, 0.0, 0.0]) {
            unreachable!("{err:?}");
        }
        if let Err(err) = stamp_dab(&mut store, surface(), center, 2.0, [0.0, 0.0, 1.0]) {
            unreachable!("{err:?}");
        }
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
    fn stamp_stroke_touches_every_dab_along_the_path() {
        let (_dir, mut store) = store();
        let touched = match stamp_stroke(
            &mut store,
            surface(),
            &[(0.0, 0.0), (10.0, 0.0)],
            10.0,
            0.25,
            [1.0, 1.0, 1.0],
        ) {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        // radius 10, spacing 0.25 -> step 2.5; a 10-unit path lands 5
        // dabs (0, 2.5, 5, 7.5, 10 -- see `aurora_brush::dab`'s own
        // `a_straight_segment_places_dabs_at_the_expected_step`), each
        // comfortably inside tile (0, 0), so one touched tile per dab.
        assert_eq!(touched, 5);
    }

    #[test]
    fn stamping_two_different_surfaces_does_not_cross_contaminate() {
        let (_dir, mut store) = store();
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        if let Err(err) = stamp_dab(&mut store, surface_a, (10.0, 10.0), 8.0, [1.0, 0.0, 0.0]) {
            unreachable!("{err:?}");
        }
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
        let touched = match erase_dab(&mut store, surface(), (10.0, 10.0), 0.0) {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(touched, 0);
        assert_eq!(store.resident_len(), 0, "must not even touch a tile");
    }

    #[test]
    fn erasing_an_untouched_pixel_is_a_no_op() {
        let (_dir, mut store) = store();
        if let Err(err) = erase_dab(&mut store, surface(), (10.0, 10.0), 20.0) {
            unreachable!("{err:?}");
        }
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
        if let Err(err) = stamp_dab(&mut store, surface(), (10.5, 10.5), 20.0, [1.0, 0.0, 0.0]) {
            unreachable!("{err:?}");
        }
        if let Err(err) = erase_dab(&mut store, surface(), (10.5, 10.5), 20.0) {
            unreachable!("{err:?}");
        }
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
        if let Err(err) = stamp_dab(&mut store, surface(), (10.0, 10.0), 20.0, [0.0, 1.0, 0.0]) {
            unreachable!("{err:?}");
        }
        if let Err(err) = erase_dab(&mut store, surface(), (10.0, 10.0), 20.0) {
            unreachable!("{err:?}");
        }
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
        if let Err(err) = stamp_dab(&mut store, surface(), (10.0, 10.0), 20.0, [1.0, 0.0, 0.0]) {
            unreachable!("{err:?}");
        }
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
        if let Err(err) = erase_dab(&mut store, surface(), (13.0, 10.0), 4.0) {
            unreachable!("{err:?}");
        }
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
    fn erase_stroke_touches_every_dab_along_the_path() {
        let (_dir, mut store) = store();
        if let Err(err) = stamp_stroke(
            &mut store,
            surface(),
            &[(0.0, 0.0), (10.0, 0.0)],
            10.0,
            0.25,
            [1.0, 1.0, 1.0],
        ) {
            unreachable!("{err:?}");
        }
        let touched = match erase_stroke(
            &mut store,
            surface(),
            &[(0.0, 0.0), (10.0, 0.0)],
            10.0,
            0.25,
        ) {
            Ok(touched) => touched,
            Err(err) => unreachable!("{err:?}"),
        };
        // Same path/radius/spacing as `stamp_stroke_touches_every_dab_along_the_path`:
        // 5 dabs, each landing in the same one already-touched tile.
        assert_eq!(touched, 5);
    }

    #[test]
    fn erasing_two_different_surfaces_does_not_cross_contaminate() {
        let (_dir, mut store) = store();
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        if let Err(err) = stamp_dab(&mut store, surface_b, (10.0, 10.0), 8.0, [1.0, 0.0, 0.0]) {
            unreachable!("{err:?}");
        }
        if let Err(err) = erase_dab(&mut store, surface_a, (10.0, 10.0), 8.0) {
            unreachable!("{err:?}");
        }
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

    #[test]
    fn stamp_dab_latency_stays_within_a_generous_ci_safe_budget() {
        // spike/FINDINGS.md's own "Recommended follow-ups" #5: p99 stroke
        // latency measured at 9.1 ms against a real 10 ms product budget
        // on real hardware -- under 1 ms of margin -- with an explicit
        // call to "add a latency regression test in CI ... do not assume
        // this holds as the brush engine grows." This is that test.
        //
        // What it measures, and what it deliberately doesn't: only
        // `stamp_dab`'s own CPU cost against an already-resident tile --
        // the same "CPU compositing/stamping, not disk I/O" hot path the
        // spike named as the real bottleneck -- isolated from GPU frame
        // submission (no GPU exists at this layer) and from first-touch
        // page-in cost (warmed up below; `aurora-tile`'s own benches
        // already cover paging). The 10 ms *product* budget the spike
        // measured covers the whole input-to-frame-submitted path across
        // `aurora-gpu`/`aurora-app` too, not just this one call, so the
        // threshold here is deliberately far more generous than 10 ms --
        // wide enough to absorb a slow, shared CI runner (the real
        // environment this test actually runs in, across
        // `.github/workflows/ci.yml`'s own three-OS matrix) while still
        // catching a genuine algorithmic regression, not a strict
        // recreation of the spike's own end-to-end figure.
        const SAMPLES: usize = 200;
        // 20 ms: roughly 2x the real 10 ms *product* budget, and nowhere
        // near the low-microsecond costs this call actually measures on
        // real hardware (a 24 px dab touches ~2,300 texels in one tile)
        // -- generous enough to absorb CI noise while still catching a
        // real regression.
        const BUDGET_MICROS: u128 = 20_000;

        let (_dir, mut store) = store();
        let center = (128.0, 128.0);
        let radius = 24.0; // aurora-app's own BRUSH_RADIUS/ERASER_RADIUS.
        let colour = [1.0, 0.0, 0.0];

        // Warm-up: settle the one touched tile as resident before any
        // timed iteration, so page-in cost (a separate, already-covered
        // concern) doesn't leak into this measurement.
        for _ in 0..8 {
            if let Err(err) = stamp_dab(&mut store, surface(), center, radius, colour) {
                unreachable!("{err:?}");
            }
        }

        let mut micros = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let start = std::time::Instant::now();
            if let Err(err) = stamp_dab(&mut store, surface(), center, radius, colour) {
                unreachable!("{err:?}");
            }
            micros.push(start.elapsed().as_micros());
        }
        micros.sort_unstable();
        let Some(&p99) = micros.get(SAMPLES * 99 / 100) else {
            unreachable!("SAMPLES * 99 / 100 is always in bounds for a SAMPLES-length vec");
        };

        assert!(
            p99 <= BUDGET_MICROS,
            "stamp_dab p99 latency regressed: {p99} \u{b5}s against a {BUDGET_MICROS} \u{b5}s \
             budget (spike/FINDINGS.md's own real 10 ms product budget had under 1 ms of margin)"
        );
    }
}
