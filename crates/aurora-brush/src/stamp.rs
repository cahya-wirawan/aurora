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

    let t0 = TileId {
        x: min_x / TILE,
        y: min_y / TILE,
    };
    let t1 = TileId {
        x: max_x / TILE,
        y: max_y / TILE,
    };
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

#[cfg(test)]
mod tests {
    use super::{stamp_dab, stamp_stroke};
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
}
