//! The Pencil's hard dab (0.220.0) at the brush level: footprint exactness,
//! no partial alpha, replacement of opaque pixels, composition with a
//! selection coverage and the transparency lock, stroke continuity, undo and
//! the Quick Mask grid variant.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, PixelHistory, StrokeSnapshot, dab_step, dabs_along_path, hard_dab_covers,
    hard_dab_snap, stamp_dab, stamp_dab_hard, stamp_dab_hard_covered,
    stamp_dab_hard_preserving_alpha, stamp_dab_hard_preserving_alpha_covered, stamp_toward,
    stamp_toward_hard,
};

const COLOUR: [f32; 3] = [0.25, 0.5, 0.75];

fn surface() -> SurfaceId {
    SurfaceId::from_raw(0)
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(64) else {
        unreachable!("non-zero");
    };
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => (dir, store),
        Err(err) => unreachable!("store: {err:?}"),
    }
}

fn texel(store: &mut TileStore, x: u32, y: u32) -> [f16; CHANNELS] {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    if !store.contains_tile(surface(), id) {
        return [f16::ZERO; CHANNELS];
    }
    let Ok(tile) = store.get(surface(), id) else {
        unreachable!("tile");
    };
    let index = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    let mut out = [f16::ZERO; CHANNELS];
    for (c, slot) in out.iter_mut().enumerate() {
        *slot = tile.texels().get(index + c).copied().unwrap_or(f16::NAN);
    }
    out
}

fn put(store: &mut TileStore, x: u32, y: u32, value: [f16; CHANNELS]) {
    let Ok(tile) = store.get_mut(
        surface(),
        TileId {
            x: x / TILE,
            y: y / TILE,
        },
    ) else {
        unreachable!("tile");
    };
    let index = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    for (c, v) in value.iter().enumerate() {
        if let Some(slot) = tile.texels_mut().get_mut(index + c) {
            *slot = *v;
        }
    }
}

fn bits(t: [f16; CHANNELS]) -> [u16; CHANNELS] {
    [
        t[0].to_bits(),
        t[1].to_bits(),
        t[2].to_bits(),
        t[3].to_bits(),
    ]
}

fn h(v: f32) -> f16 {
    f16::from_f32(v)
}

/// The colour as a full-alpha premultiplied texel.
fn solid() -> [f16; CHANNELS] {
    [h(COLOUR[0]), h(COLOUR[1]), h(COLOUR[2]), h(1.0)]
}

/// Every texel of the 3 x 3 tile block with any non-zero sample.
fn painted_texels(store: &mut TileStore) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    for y in 0..(3 * TILE) {
        for x in 0..(3 * TILE) {
            if texel(store, x, y).iter().any(|v| v.to_bits() != 0) {
                out.insert((x, y));
            }
        }
    }
    out
}

/// The footprint by its definition, written independently of the library:
/// integer offsets from the dab's texel with `dx^2 + dy^2 < r^2`.
fn expected_footprint(cx: u32, cy: u32, radius: f32) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    let reach = radius.ceil() as i64 + 1;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            if ((dx * dx + dy * dy) as f32) < radius * radius {
                let (x, y) = (i64::from(cx) + dx, i64::from(cy) + dy);
                if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) {
                    out.insert((x, y));
                }
            }
        }
    }
    out
}

fn dab_hard(store: &mut TileStore, center: (f32, f32), radius: f32) -> Vec<TileId> {
    let outcome = stamp_dab_hard(store, surface(), center, radius, COLOUR, None);
    assert!(outcome.is_complete());
    outcome.painted().to_vec()
}

#[test]
fn texel_counts_are_exact_for_radii_1_2_3_10() {
    for (radius, count) in [(1.0_f32, 1_usize), (2.0, 9), (3.0, 25), (10.0, 305)] {
        let (_dir, mut s) = store();
        dab_hard(&mut s, (100.5, 100.5), radius);
        let got = painted_texels(&mut s);
        assert_eq!(got.len(), count, "radius {radius}");
        assert_eq!(got, expected_footprint(100, 100, radius), "radius {radius}");
    }
}

#[test]
fn a_fractional_radius_uses_the_same_strict_rule() {
    let (_dir, mut s) = store();
    dab_hard(&mut s, (50.5, 50.5), 1.5);
    assert_eq!(painted_texels(&mut s), expected_footprint(50, 50, 1.5));
    assert_eq!(painted_texels(&mut s).len(), 9);
}

#[test]
fn radius_one_is_a_single_texel_wherever_the_pointer_is_inside_it() {
    for (cx, cy) in [(7.0, 9.0), (7.01, 9.99), (7.5, 9.5), (7.99, 9.01)] {
        let (_dir, mut s) = store();
        dab_hard(&mut s, (cx, cy), 1.0);
        let got = painted_texels(&mut s);
        assert_eq!(got, BTreeSet::from([(7, 9)]), "pointer ({cx}, {cy})");
    }
}

#[test]
fn the_footprint_is_symmetric_about_its_texel() {
    for radius in [1.0_f32, 2.0, 3.0, 10.0, 4.5] {
        let (_dir, mut s) = store();
        dab_hard(&mut s, (120.5, 130.5), radius);
        let got = painted_texels(&mut s);
        for &(x, y) in &got {
            let (dx, dy) = (i64::from(x) - 120, i64::from(y) - 130);
            for (mx, my) in [(-dx, dy), (dx, -dy), (dy, dx), (-dy, -dx)] {
                let (ox, oy) = (120 + mx, 130 + my);
                let (Ok(ox), Ok(oy)) = (u32::try_from(ox), u32::try_from(oy)) else {
                    unreachable!("near the middle of the grid");
                };
                assert!(
                    got.contains(&(ox, oy)),
                    "radius {radius}: ({dx}, {dy}) has no mirror"
                );
            }
        }
    }
}

#[test]
fn a_dab_across_tile_edges_has_the_same_footprint() {
    // Texel (255, 255) is the corner of four tiles.
    let (_dir, mut s) = store();
    let painted = dab_hard(&mut s, (255.9, 255.2), 3.0);
    assert_eq!(painted.len(), 4, "all four tiles");
    assert_eq!(painted_texels(&mut s), expected_footprint(255, 255, 3.0));
}

#[test]
fn every_covered_texel_is_exactly_the_full_value_and_none_is_partial() {
    let (_dir, mut s) = store();
    dab_hard(&mut s, (64.5, 64.5), 10.0);
    let footprint = expected_footprint(64, 64, 10.0);
    for y in 40..90 {
        for x in 40..90 {
            let t = texel(&mut s, x, y);
            if footprint.contains(&(x, y)) {
                assert_eq!(bits(t), bits(solid()), "({x}, {y}) is a full-value write");
            } else {
                assert_eq!(bits(t), [0; 4], "({x}, {y}) is untouched");
            }
            let alpha = t[3].to_f32();
            assert!(
                alpha == 0.0 || alpha == 1.0,
                "no partial alpha at ({x}, {y})"
            );
        }
    }
}

#[test]
fn the_brush_has_an_antialiased_edge_the_pencil_does_not() {
    let (_dir, mut soft) = store();
    let outcome = stamp_dab(&mut soft, surface(), (64.5, 64.5), 10.0, COLOUR, None);
    assert!(outcome.is_complete());
    let partial = painted_texels(&mut soft)
        .into_iter()
        .filter(|&(x, y)| {
            let a = texel(&mut soft, x, y)[3].to_f32();
            a > 0.0 && a < 1.0
        })
        .count();
    assert!(partial > 0, "the soft brush does produce partial alpha");
}

#[test]
fn a_pencil_dab_replaces_an_opaque_pixel_of_another_colour() {
    let (_dir, mut s) = store();
    let red = [h(1.0), h(0.0), h(0.0), h(1.0)];
    for y in 18..23 {
        for x in 18..23 {
            put(&mut s, x, y, red);
        }
    }
    dab_hard(&mut s, (20.5, 20.5), 2.0);
    assert_eq!(bits(texel(&mut s, 20, 20)), bits(solid()));
    assert_eq!(bits(texel(&mut s, 19, 19)), bits(solid()));
    assert_eq!(bits(texel(&mut s, 18, 18)), bits(red), "outside the 3 x 3");
}

#[test]
fn painting_the_same_dab_twice_changes_nothing_the_second_time() {
    let (_dir, mut s) = store();
    assert!(!dab_hard(&mut s, (30.5, 30.5), 4.0).is_empty());
    assert!(dab_hard(&mut s, (30.5, 30.5), 4.0).is_empty());
}

#[test]
fn a_degenerate_dab_paints_nothing() {
    let (_dir, mut s) = store();
    for (c, r) in [
        ((f32::NAN, 5.0), 3.0),
        ((5.0, f32::INFINITY), 3.0),
        ((5.0, 5.0), 0.0),
        ((5.0, 5.0), -2.0),
        ((5.0, 5.0), f32::NAN),
    ] {
        assert!(dab_hard(&mut s, c, r).is_empty());
    }
    assert!(painted_texels(&mut s).is_empty());
}

/// A coverage with a single value everywhere.
struct Constant(f32);

impl Coverage for Constant {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, _: i64, _: i64) -> f32 {
        self.0
    }
}

/// A rectangle selection (document == layer space here).
struct RectCov(Rect);

impl Coverage for RectCov {
    fn bounds(&self) -> Option<Rect> {
        Some(self.0)
    }
    fn at(&self, x: i64, y: i64) -> f32 {
        if self.0.contains_point(x, y) {
            1.0
        } else {
            0.0
        }
    }
}

fn dump(store: &mut TileStore) -> Vec<Vec<[u16; 4]>> {
    let mut out = Vec::new();
    for ty in 0..2 {
        for tx in 0..2 {
            let mut rows = Vec::new();
            if store.contains_tile(surface(), TileId { x: tx, y: ty }) {
                for y in 0..TILE {
                    for x in 0..TILE {
                        rows.push(bits(texel(store, tx * TILE + x, ty * TILE + y)));
                    }
                }
            }
            out.push(rows);
        }
    }
    out
}

#[test]
fn no_coverage_and_full_coverage_are_bit_identical() {
    let (_d1, mut a) = store();
    let (_d2, mut b) = store();
    let (_d3, mut c) = store();
    let full = Constant(1.0);
    let rect = RectCov(Rect {
        x: -50,
        y: -50,
        width: 1000,
        height: 1000,
    });
    for (center, radius) in [
        ((255.7, 255.1), 6.0),
        ((40.2, 41.8), 12.0),
        ((300.0, 20.0), 1.0),
    ] {
        assert!(stamp_dab_hard(&mut a, surface(), center, radius, COLOUR, None).is_complete());
        assert!(
            stamp_dab_hard_covered(&mut b, surface(), center, radius, COLOUR, None, Some(&full))
                .is_complete()
        );
        assert!(
            stamp_dab_hard_covered(&mut c, surface(), center, radius, COLOUR, None, Some(&rect))
                .is_complete()
        );
    }
    assert_eq!(dump(&mut a), dump(&mut b));
    assert_eq!(dump(&mut a), dump(&mut c));
}

#[test]
fn a_rectangle_selection_clips_the_hard_footprint_exactly() {
    let (_dir, mut s) = store();
    let sel = RectCov(Rect {
        x: 98,
        y: 0,
        width: 3,
        height: 1000,
    });
    let outcome = stamp_dab_hard_covered(
        &mut s,
        surface(),
        (100.5, 100.5),
        5.0,
        COLOUR,
        None,
        Some(&sel),
    );
    assert!(outcome.is_complete());
    let got = painted_texels(&mut s);
    let want: BTreeSet<(u32, u32)> = expected_footprint(100, 100, 5.0)
        .into_iter()
        .filter(|&(x, _)| (98..101).contains(&x))
        .collect();
    assert_eq!(got, want);
    assert!(!got.is_empty());
}

#[test]
fn a_selection_that_excludes_the_dab_creates_no_tile_and_captures_nothing() {
    let (_dir, mut s) = store();
    let sel = RectCov(Rect {
        x: 700,
        y: 700,
        width: 10,
        height: 10,
    });
    let mut snap = StrokeSnapshot::new(surface());
    let outcome = stamp_dab_hard_covered(
        &mut s,
        surface(),
        (100.5, 100.5),
        8.0,
        COLOUR,
        Some(&mut snap),
        Some(&sel),
    );
    assert!(outcome.painted().is_empty());
    assert!(snap.is_empty());
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
}

#[test]
fn fractional_coverage_blends_the_hard_value_by_that_coverage() {
    let (_dir, mut s) = store();
    let half_cov = Constant(0.5);
    let outcome = stamp_dab_hard_covered(
        &mut s,
        surface(),
        (10.5, 10.5),
        1.0,
        COLOUR,
        None,
        Some(&half_cov),
    );
    assert!(outcome.is_complete());
    let t = texel(&mut s, 10, 10);
    // old 0 -> new (colour, 1): the half-way blend.
    let want = [
        h(0.5 * h(COLOUR[0]).to_f32()),
        h(0.5 * h(COLOUR[1]).to_f32()),
        h(0.5 * h(COLOUR[2]).to_f32()),
        h(0.5),
    ];
    assert_eq!(bits(t), bits(want));
    // Over an opaque texel the blend is a mix of the two colours.
    let red = [h(1.0), h(0.0), h(0.0), h(1.0)];
    put(&mut s, 30, 30, red);
    let _ = stamp_dab_hard_covered(
        &mut s,
        surface(),
        (30.5, 30.5),
        1.0,
        COLOUR,
        None,
        Some(&half_cov),
    );
    let t = texel(&mut s, 30, 30);
    assert_eq!(t[3].to_f32(), 1.0);
    assert_eq!(
        t[0].to_f32(),
        h(1.0 + (h(COLOUR[0]).to_f32() - 1.0) * 0.5).to_f32()
    );
    assert_eq!(t[1].to_f32(), h(h(COLOUR[1]).to_f32() * 0.5).to_f32());
}

#[test]
fn the_transparency_lock_recolours_without_changing_alpha() {
    let (_dir, mut s) = store();
    let half_alpha = [h(0.5), h(0.0), h(0.0), h(0.5)];
    let opaque = [h(0.0), h(1.0), h(0.0), h(1.0)];
    put(&mut s, 20, 20, half_alpha);
    put(&mut s, 21, 20, opaque);
    // (19, 20) stays fully transparent.
    let outcome =
        stamp_dab_hard_preserving_alpha(&mut s, surface(), (20.5, 20.5), 2.0, COLOUR, None);
    assert!(outcome.is_complete());
    let t = texel(&mut s, 20, 20);
    assert_eq!(t[3].to_bits(), h(0.5).to_bits(), "alpha kept bit for bit");
    assert_eq!(
        t[0].to_bits(),
        h(COLOUR[0] * 0.5).to_bits(),
        "full colour at its alpha"
    );
    assert_eq!(t[1].to_bits(), h(COLOUR[1] * 0.5).to_bits());
    let t = texel(&mut s, 21, 20);
    assert_eq!(
        bits(t),
        bits(solid()),
        "an opaque texel takes the colour fully"
    );
    assert_eq!(
        bits(texel(&mut s, 19, 20)),
        [0; 4],
        "transparent stays transparent"
    );
    assert_eq!(bits(texel(&mut s, 20, 21)), [0; 4]);
}

#[test]
fn the_transparency_lock_composes_with_a_selection() {
    let (_dir, mut s) = store();
    for x in 10..14 {
        put(&mut s, x, 10, [h(0.0), h(0.0), h(0.0), h(1.0)]);
    }
    let sel = RectCov(Rect {
        x: 12,
        y: 0,
        width: 10,
        height: 50,
    });
    let outcome = stamp_dab_hard_preserving_alpha_covered(
        &mut s,
        surface(),
        (11.5, 10.5),
        3.0,
        COLOUR,
        None,
        Some(&sel),
    );
    assert!(outcome.is_complete());
    assert_eq!(
        bits(texel(&mut s, 11, 10)),
        [0, 0, 0, h(1.0).to_bits()],
        "outside the selection"
    );
    assert_eq!(bits(texel(&mut s, 12, 10)), bits(solid()));
    assert_eq!(bits(texel(&mut s, 13, 10)), bits(solid()));
}

#[test]
fn a_stroke_has_no_gaps_for_small_and_large_radii() {
    let path = [
        (10.3_f32, 10.7_f32),
        (90.1, 60.9),
        (40.0, 100.2),
        (41.0, 12.0),
    ];
    for radius in [1.0_f32, 2.0, 5.0] {
        let dabs = dabs_along_path(&path, radius, crate::DEFAULT_SPACING);
        let (_dir, mut s) = store();
        let mut snapped = Vec::new();
        for &d in &dabs {
            dab_hard(&mut s, d, radius);
            snapped.push(hard_dab_snap(d));
        }
        // Consecutive dab texels never skip one: 8-connected while the
        // spacing is at most a texel, and overlapping discs beyond that.
        let step = dab_step(radius, crate::DEFAULT_SPACING);
        for pair in snapped.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if step <= 1.0 {
                assert!(
                    (a.0 - b.0).abs() <= 1.0 && (a.1 - b.1).abs() <= 1.0,
                    "radius {radius}: {a:?} -> {b:?} skips a texel"
                );
            } else {
                assert!(
                    (a.0 - b.0).hypot(a.1 - b.1) < radius,
                    "radius {radius}: {a:?} -> {b:?} leaves a gap"
                );
            }
        }
        // Every dab's whole footprint is painted: the union is exact.
        let got = painted_texels(&mut s);
        let mut want = BTreeSet::new();
        for &(sx, sy) in &snapped {
            want.extend(expected_footprint(
                sx.floor() as u32,
                sy.floor() as u32,
                radius,
            ));
        }
        assert_eq!(got, want, "radius {radius}");
        assert!(
            got.contains(&(10, 10)),
            "the first dab lands where the pointer went down"
        );
    }
}

#[test]
fn a_stroke_is_one_undo_step_that_restores_every_pixel() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    for d in dabs_along_path(&[(20.0, 20.0), (300.0, 40.0)], 4.0, crate::DEFAULT_SPACING) {
        let outcome = stamp_dab_hard(&mut s, surface(), d, 4.0, COLOUR, Some(&mut snap));
        assert!(outcome.is_complete());
    }
    assert!(!snap.is_empty());
    assert!(!painted_texels(&mut s).is_empty());
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert!(
        painted_texels(&mut s).is_empty(),
        "undo restores the blank layer"
    );
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert!(!painted_texels(&mut s).is_empty());
}

#[test]
fn snapping_and_covering_helpers() {
    assert_eq!(hard_dab_snap((3.0, 4.99)), (3.5, 4.5));
    assert_eq!(hard_dab_snap((-0.2, 0.0)), (-0.5, 0.5));
    assert!(hard_dab_snap((f32::NAN, 1.0)).0.is_nan());
    assert!(hard_dab_covers(5.5, 5.5, (5.5, 5.5), 1.0));
    assert!(
        !hard_dab_covers(6.5, 5.5, (5.5, 5.5), 1.0),
        "strict: d == r is out"
    );
    assert!(hard_dab_covers(6.5, 6.5, (5.5, 5.5), 2.0));
    assert!(!hard_dab_covers(7.5, 5.5, (5.5, 5.5), 2.0));
}

// -- The Quick Mask grid variant ----------------------------------------

fn grid_set(data: &[u8], w: u32) -> BTreeSet<(u32, u32)> {
    data.iter()
        .enumerate()
        .filter(|&(_, &v)| v != 255)
        .map(|(i, _)| {
            (
                u32::try_from(i).unwrap_or(0) % w,
                u32::try_from(i).unwrap_or(0) / w,
            )
        })
        .collect()
}

#[test]
fn the_grid_dab_has_the_same_footprint_and_sets_the_target_outright() {
    for (radius, count) in [(1.0_f32, 1_usize), (2.0, 9), (3.0, 25), (10.0, 305)] {
        let mut data = vec![255_u8; 64 * 64];
        let rect = stamp_toward_hard(&mut data, 64, 64, (30.2, 31.9), radius, 40);
        let set = grid_set(&data, 64);
        assert_eq!(set.len(), count, "radius {radius}");
        assert_eq!(set, expected_footprint(30, 31, radius));
        assert!(
            data.iter().all(|&v| v == 255 || v == 40),
            "no in-between byte"
        );
        let Some(rect) = rect else {
            unreachable!("something changed");
        };
        let x1 = rect.x + i64::from(rect.width) - 1;
        let y1 = rect.y + i64::from(rect.height) - 1;
        assert_eq!(set.iter().map(|p| i64::from(p.0)).min(), Some(rect.x));
        assert_eq!(set.iter().map(|p| i64::from(p.0)).max(), Some(x1));
        assert_eq!(set.iter().map(|p| i64::from(p.1)).min(), Some(rect.y));
        assert_eq!(set.iter().map(|p| i64::from(p.1)).max(), Some(y1));
    }
}

#[test]
fn the_grid_dab_clips_to_the_grid_and_reports_no_change_when_already_set() {
    let mut data = vec![255_u8; 16 * 16];
    assert!(stamp_toward_hard(&mut data, 16, 16, (0.1, 0.1), 3.0, 0).is_some());
    assert_eq!(grid_set(&data, 16), expected_footprint(0, 0, 3.0));
    assert!(stamp_toward_hard(&mut data, 16, 16, (0.1, 0.1), 3.0, 0).is_none());
    assert!(stamp_toward_hard(&mut data, 16, 16, (500.0, 500.0), 3.0, 0).is_none());
    assert!(stamp_toward_hard(&mut data, 16, 16, (f32::NAN, 1.0), 3.0, 0).is_none());
    // The soft dab is a different thing: partial bytes near its edge.
    let mut soft = vec![255_u8; 64 * 64];
    let _ = stamp_toward(&mut soft, 64, 64, (30.5, 30.5), 10.0, 0);
    assert!(soft.iter().any(|&v| v != 0 && v != 255));
}
