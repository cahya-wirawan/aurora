//! Tests of the Quick Select engine ([`crate::quick_select_stamp`],
//! 0.238.0) against an independent reference: footprint, mean colour, the
//! tolerance edge, the window, repeats, caps and its read-only promise.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::HashSet;
use std::num::NonZeroUsize;

use aurora_tile::{CHANNELS, TILE, TileId, TileStore};
use half::f16;

use crate::test_support::surface;
use crate::{
    FillError, MAX_FILL_TILES, QUICK_MIN_REACH, QuickRegion, QuickStamp, footprint,
    quick_select_region, quick_select_stamp, reach_of,
};

const SIDE: usize = TILE as usize;

fn new_store() -> (tempfile::TempDir, TileStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = TileStore::new(dir.path().to_path_buf(), NonZeroUsize::new(64).unwrap()).unwrap();
    (dir, store)
}

fn put(store: &mut TileStore, x: u32, y: u32, rgba: [u8; 4]) {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    let tile = store.get_mut(surface(), id).unwrap();
    let index = ((y % TILE) as usize * SIDE + (x % TILE) as usize) * CHANNELS;
    for (offset, v) in rgba.iter().enumerate() {
        tile.texels_mut()[index + offset] = f16::from_f32(f32::from(*v) / 255.0);
    }
}

fn fill(store: &mut TileStore, w: u32, h: u32, rgba: [u8; 4]) {
    for y in 0..h {
        for x in 0..w {
            put(store, x, y, rgba);
        }
    }
}

fn stamp(centre: (f32, f32), radius: f32, tolerance: u8) -> QuickStamp {
    QuickStamp {
        centre,
        radius,
        tolerance,
    }
}

/// The set of selected texels, read back through `to_region`.
fn selected(grown: &QuickRegion) -> HashSet<(u32, u32)> {
    let mut out = HashSet::new();
    if let Some(region) = quick_select_region(grown).unwrap() {
        let w = region.bounds.width as usize;
        for (i, byte) in region.data.iter().enumerate() {
            assert!(*byte == 0 || *byte == 255);
            if *byte == 255 {
                out.insert((
                    region.bounds.x as u32 + (i % w) as u32,
                    region.bounds.y as u32 + (i / w) as u32,
                ));
            }
        }
        // The box is tight.
        assert!(out.iter().any(|p| p.0 == region.bounds.x as u32));
        assert!(out.iter().any(|p| p.1 == region.bounds.y as u32));
    }
    out
}

// ---------------------------------------------------------------------
// The independent reference (a fixpoint sweep, not a queue).

fn ref_footprint(c: (f64, f64), r: f64, w: u32, h: u32) -> HashSet<(u32, u32)> {
    let mut out = HashSet::new();
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (f64::from(x) + 0.5 - c.0, f64::from(y) + 0.5 - c.1);
            if dx * dx + dy * dy <= r * r {
                out.insert((x, y));
            }
        }
    }
    let (ux, uy) = (c.0.floor(), c.1.floor());
    if ux >= 0.0 && uy >= 0.0 && ux < f64::from(w) && uy < f64::from(h) {
        out.insert((ux as u32, uy as u32));
    }
    out
}

fn ref_stamp(
    image: &[[u8; 4]],
    (w, h): (u32, u32),
    c: (f64, f64),
    r: f64,
    tolerance: u8,
) -> HashSet<(u32, u32)> {
    let foot = ref_footprint(c, r, w, h);
    if foot.is_empty() {
        return foot;
    }
    let at = |x: u32, y: u32| image[(y * w + x) as usize];
    let mut sum = [0_f64; 4];
    for &(x, y) in &foot {
        for (s, v) in sum.iter_mut().zip(at(x, y)) {
            *s += f64::from(v);
        }
    }
    let mean = sum.map(|s| (s / foot.len() as f64).round() as u8);
    let reach = ((r * 3.0).ceil() as u32).max(24);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for &(x, y) in &foot {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (x0, y0) = (x0.saturating_sub(reach), y0.saturating_sub(reach));
    let (x1, y1) = ((x1 + reach).min(w - 1), (y1 + reach).min(h - 1));
    let mut set = foot;
    loop {
        let mut grew = false;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if set.contains(&(x, y)) {
                    continue;
                }
                let touches = (x > x0 && set.contains(&(x - 1, y)))
                    || (x < x1 && set.contains(&(x + 1, y)))
                    || (y > y0 && set.contains(&(x, y - 1)))
                    || (y < y1 && set.contains(&(x, y + 1)));
                let near = at(x, y)
                    .iter()
                    .zip(mean)
                    .all(|(a, b)| a.abs_diff(b) <= tolerance);
                if touches && near {
                    set.insert((x, y));
                    grew = true;
                }
            }
        }
        if !grew {
            return set;
        }
    }
}

// ---------------------------------------------------------------------

#[test]
fn the_footprint_is_the_disk_of_texel_centres_plus_the_centre_texel() {
    // Centre on a texel centre, radius 1: the texel and its four
    // neighbours (their centres are exactly 1 away: inclusive).
    let five = footprint(&stamp((5.5, 5.5), 1.0, 0), (20, 20));
    assert_eq!(five.len(), 5);
    for p in [(5, 5), (4, 5), (6, 5), (5, 4), (5, 6)] {
        assert!(five.contains(&p), "{p:?}");
    }
    // Radius 0 is the texel under the centre alone, even off a texel
    // centre.
    assert_eq!(
        footprint(&stamp((7.9, 3.1), 0.0, 0), (20, 20)),
        vec![(7, 3)]
    );
    // A radius too small to hold any texel centre still selects one.
    assert_eq!(footprint(&stamp((7.0, 3.0), 0.3, 0), (20, 20)).len(), 1);
    // Clipped to the layer.
    let corner = footprint(&stamp((0.0, 0.0), 3.0, 0), (20, 20));
    assert!(corner.iter().all(|&(x, y)| x < 20 && y < 20));
    assert!(corner.contains(&(0, 0)));
    // Centre outside by more than the radius: nothing.
    assert!(footprint(&stamp((-10.0, 5.0), 3.0, 0), (20, 20)).is_empty());
    assert!(footprint(&stamp((f32::NAN, 5.0), 3.0, 0), (20, 20)).is_empty());
    assert!(footprint(&stamp((5.0, 5.0), f32::NAN, 0), (20, 20)).is_empty());
}

#[test]
fn the_footprint_matches_the_reference_for_random_discs() {
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    for _ in 0..60 {
        let c = (next(700) as f32 / 10.0 - 5.0, next(500) as f32 / 10.0 - 5.0);
        let r = next(160) as f32 / 10.0;
        let got: HashSet<(u32, u32)> = footprint(&stamp(c, r, 0), (64, 40)).into_iter().collect();
        let want = ref_footprint((f64::from(c.0), f64::from(c.1)), f64::from(r), 64, 40);
        assert_eq!(got, want, "{c:?} r {r}");
    }
}

#[test]
fn the_reach_is_three_radii_but_at_least_the_minimum() {
    assert_eq!(QUICK_MIN_REACH, 24);
    assert_eq!(reach_of(0.0), 24);
    assert_eq!(reach_of(8.0), 24);
    assert_eq!(reach_of(10.0), 30);
    assert_eq!(reach_of(100.0), 300);
    assert_eq!(reach_of(f32::NAN), 24);
}

#[test]
fn random_blocky_images_match_the_reference_over_stroke_sequences() {
    let (w, h) = (110_u32, 90_u32);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let mut compared = 0;
    for round in 0..6_u64 {
        let block = 8 + round as u32;
        let palette: Vec<[u8; 4]> = (0..4)
            .map(|k| {
                let v = (50 * k + next(25)) as u8;
                if k == 3 { [0, 0, 0, 0] } else { [v, v, v, 255] }
            })
            .collect();
        let mut image = vec![[0_u8; 4]; (w * h) as usize];
        let (_dir, mut store) = new_store();
        for y in 0..h {
            for x in 0..w {
                let cell = ((x / block) * 7 + (y / block) * 3 + round as u32) % 4;
                image[(y * w + x) as usize] = palette[cell as usize];
                put(&mut store, x, y, palette[cell as usize]);
            }
        }
        for tolerance in [0_u8, 10, 40, 255] {
            let mut grown = QuickRegion::new(MAX_FILL_TILES);
            let mut want: HashSet<(u32, u32)> = HashSet::new();
            // A stroke of three stamps.
            for _ in 0..3 {
                let c = (next(u64::from(w)) as f32, next(u64::from(h)) as f32);
                let r = 1.0 + next(60) as f32 / 10.0;
                quick_select_stamp(
                    &mut store,
                    surface(),
                    (w, h),
                    &stamp(c, r, tolerance),
                    &mut grown,
                )
                .unwrap();
                compared += 1;
                // A footprint already wholly selected grows nothing.
                let foot = ref_footprint((f64::from(c.0), f64::from(c.1)), f64::from(r), w, h);
                if foot.is_subset(&want) {
                    continue;
                }
                want.extend(ref_stamp(
                    &image,
                    (w, h),
                    (f64::from(c.0), f64::from(c.1)),
                    f64::from(r),
                    tolerance,
                ));
            }
            assert_eq!(
                selected(&grown),
                want,
                "round {round} tolerance {tolerance}"
            );
        }
    }
    assert_eq!(compared, 72);
}

#[test]
fn growth_stops_at_a_colour_edge() {
    let (_dir, mut store) = new_store();
    for y in 0..60 {
        for x in 0..200 {
            let c = if x < 100 {
                [200, 30, 30, 255]
            } else {
                [30, 30, 200, 255]
            };
            put(&mut store, x, y, c);
        }
    }
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    // The footprint is entirely red; the window reaches past the edge.
    assert!(
        quick_select_stamp(
            &mut store,
            surface(),
            (200, 60),
            &stamp((90.0, 30.0), 6.0, 20),
            &mut grown
        )
        .unwrap()
    );
    let set = selected(&grown);
    assert!(set.iter().all(|&(x, _)| x < 100), "no blue texel");
    assert!(set.contains(&(99, 30)), "it grew right up to the edge");
    assert!(set.contains(&(70, 30)), "and well past the footprint");
}

#[test]
fn growth_never_leaves_the_window() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 400, 400, [90, 90, 90, 255]);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    quick_select_stamp(
        &mut store,
        surface(),
        (400, 400),
        &stamp((200.0, 200.0), 10.0, 0),
        &mut grown,
    )
    .unwrap();
    let region = quick_select_region(&grown).unwrap().unwrap();
    // Footprint x 190..=209, reach 30: 160..=239.
    assert_eq!(
        (
            region.bounds.x,
            region.bounds.y,
            region.bounds.width,
            region.bounds.height
        ),
        (160, 160, 80, 80)
    );
    assert!(
        region.data.iter().all(|b| *b == 255),
        "a flat window is solid"
    );
}

#[test]
fn a_second_stamp_continues_where_the_first_window_ended() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 400, 100, [90, 90, 90, 255]);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    quick_select_stamp(
        &mut store,
        surface(),
        (400, 100),
        &stamp((50.0, 50.0), 5.0, 0),
        &mut grown,
    )
    .unwrap();
    let first = selected(&grown);
    assert!(!first.contains(&(200, 50)));
    // Dragging on extends the selection; the union keeps the first.
    for x in (60..=240).step_by(10) {
        quick_select_stamp(
            &mut store,
            surface(),
            (400, 100),
            &stamp((x as f32, 50.0), 5.0, 0),
            &mut grown,
        )
        .unwrap();
    }
    let all = selected(&grown);
    assert!(first.is_subset(&all));
    assert!(all.contains(&(200, 50)));
    assert!(!all.contains(&(300, 50)));
}

#[test]
fn a_footprint_is_selected_even_where_it_straddles_an_edge_and_matches_nothing() {
    let (_dir, mut store) = new_store();
    for y in 0..40 {
        for x in 0..40 {
            let c = if x < 20 {
                [250, 0, 0, 255]
            } else {
                [0, 0, 250, 255]
            };
            put(&mut store, x, y, c);
        }
    }
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    // The mean is purple: neither side is within a tolerance of 10 of it.
    let st = stamp((20.0, 20.0), 4.0, 10);
    quick_select_stamp(&mut store, surface(), (40, 40), &st, &mut grown).unwrap();
    let want: HashSet<(u32, u32)> = footprint(&st, (40, 40)).into_iter().collect();
    assert_eq!(selected(&grown), want);
    assert!(want.iter().any(|p| p.0 < 20) && want.iter().any(|p| p.0 >= 20));
}

#[test]
fn the_tolerance_is_inclusive_against_the_footprint_mean() {
    let (_dir, mut store) = new_store();
    // A uniform footprint of grey 100 with a neighbour at 100 + 30, then
    // one at 100 + 31 further on.
    for x in 0..40_u32 {
        put(&mut store, x, 0, [100, 100, 100, 255]);
    }
    put(&mut store, 11, 0, [130, 130, 130, 255]);
    put(&mut store, 13, 0, [131, 131, 131, 255]);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    quick_select_stamp(
        &mut store,
        surface(),
        (40, 1),
        &stamp((5.5, 0.5), 0.4, 30),
        &mut grown,
    )
    .unwrap();
    let set = selected(&grown);
    assert!(set.contains(&(11, 0)), "exactly the tolerance away is in");
    assert!(!set.contains(&(13, 0)), "one over is out");
    // 12 lies between them at 100 and is connected through 11.
    assert!(set.contains(&(12, 0)));
}

#[test]
fn a_repeat_inside_the_selection_changes_nothing_and_says_so() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 120, 120, [60, 60, 60, 255]);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    let st = stamp((60.0, 60.0), 8.0, 5);
    assert!(quick_select_stamp(&mut store, surface(), (120, 120), &st, &mut grown).unwrap());
    let before = selected(&grown);
    assert!(!quick_select_stamp(&mut store, surface(), (120, 120), &st, &mut grown).unwrap());
    // A smaller stamp inside is also a no-op.
    assert!(
        !quick_select_stamp(
            &mut store,
            surface(),
            (120, 120),
            &stamp((62.0, 58.0), 3.0, 5),
            &mut grown
        )
        .unwrap()
    );
    assert_eq!(selected(&grown), before);
}

#[test]
fn a_stamp_that_finds_nothing_new_returns_false_but_one_that_adds_returns_true() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 100, 20, [60, 60, 60, 255]);
    // Bar the way: a different column on each side of the footprint.
    for y in 0..20 {
        put(&mut store, 30, y, [250, 0, 0, 255]);
        put(&mut store, 50, y, [250, 0, 0, 255]);
    }
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    assert!(
        quick_select_stamp(
            &mut store,
            surface(),
            (100, 20),
            &stamp((40.0, 10.0), 3.0, 5),
            &mut grown
        )
        .unwrap()
    );
    // Footprint wholly selected already: false. Footprint reaching one new
    // column: true.
    assert!(
        !quick_select_stamp(
            &mut store,
            surface(),
            (100, 20),
            &stamp((40.0, 10.0), 2.0, 5),
            &mut grown
        )
        .unwrap()
    );
    assert!(
        quick_select_stamp(
            &mut store,
            surface(),
            (100, 20),
            &stamp((30.0, 10.0), 2.0, 5),
            &mut grown
        )
        .unwrap()
    );
}

#[test]
fn it_selects_transparent_areas_without_creating_tiles() {
    let (_dir, mut store) = new_store();
    put(&mut store, 300, 5, [10, 10, 10, 255]);
    let tiles = |store: &TileStore| {
        (0..2_u32)
            .flat_map(|y| (0..2_u32).map(move |x| TileId { x, y }))
            .filter(|id| store.contains_tile(surface(), *id))
            .count()
    };
    let before = tiles(&store);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    quick_select_stamp(
        &mut store,
        surface(),
        (400, 400),
        &stamp((100.0, 100.0), 5.0, 5),
        &mut grown,
    )
    .unwrap();
    assert!(!grown.is_empty());
    assert_eq!(tiles(&store), before, "reading made no tile");
}

#[test]
fn a_stamp_off_the_layer_or_with_a_bad_centre_does_nothing() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 30, 30, [60, 60, 60, 255]);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    for centre in [
        (-50.0, 5.0),
        (5.0, 500.0),
        (f32::NAN, 1.0),
        (f32::INFINITY, 1.0),
    ] {
        assert!(
            !quick_select_stamp(
                &mut store,
                surface(),
                (30, 30),
                &stamp(centre, 4.0, 5),
                &mut grown
            )
            .unwrap()
        );
    }
    assert!(grown.is_empty());
    assert!(quick_select_region(&grown).unwrap().is_none());
}

#[test]
fn the_tile_cap_refuses_a_stamp_that_would_touch_too_many_tiles() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 300, 100, [60, 60, 60, 255]);
    // The stamp's window spans the tile edge at x = 256: two tiles.
    let mut grown = QuickRegion::new(1);
    let r = quick_select_stamp(
        &mut store,
        surface(),
        (300, 100),
        &stamp((256.0, 50.0), 4.0, 5),
        &mut grown,
    );
    assert!(matches!(r, Err(FillError::TooLarge { max: 1 })), "{r:?}");
    // With room for both it succeeds.
    let mut roomy = QuickRegion::new(2);
    assert!(
        quick_select_stamp(
            &mut store,
            surface(),
            (300, 100),
            &stamp((256.0, 50.0), 4.0, 5),
            &mut roomy
        )
        .unwrap()
    );
}

#[test]
fn it_writes_nothing() {
    let (_dir, mut store) = new_store();
    fill(&mut store, 60, 60, [60, 70, 80, 255]);
    let snapshot = |store: &mut TileStore| {
        store
            .get(surface(), TileId { x: 0, y: 0 })
            .unwrap()
            .texels()
            .to_vec()
    };
    let before = snapshot(&mut store);
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    quick_select_stamp(
        &mut store,
        surface(),
        (60, 60),
        &stamp((30.0, 30.0), 6.0, 90),
        &mut grown,
    )
    .unwrap();
    assert_eq!(before, snapshot(&mut store));
}

#[test]
fn an_unreadable_tile_is_an_error() {
    let (_dir, mut store) =
        crate::test_support::store_with_a_broken_tile(TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 });
    let mut grown = QuickRegion::new(MAX_FILL_TILES);
    let r = quick_select_stamp(
        &mut store,
        surface(),
        (512, 256),
        &stamp((10.0, 10.0), 4.0, 5),
        &mut grown,
    );
    assert!(matches!(r, Err(FillError::Tile(_))), "{r:?}");
    assert!(grown.is_empty());
}
