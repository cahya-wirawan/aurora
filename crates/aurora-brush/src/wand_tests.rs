//! Tests of the Magic Wand's region finder ([`crate::wand_region`], 0.214.0)
//! against a brute-force reference, plus its tolerance edge, its caps and
//! its read-only promise.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::naive_bytecount
)]

use std::num::NonZeroUsize;

use aurora_tile::{CHANNELS, TILE, TileId, TileStore};
use half::f16;

use crate::test_support::surface;
use crate::{FillError, FloodSpec, MAX_EXAMINED_TILES, MAX_FILL_TILES, wand_region};

const SIDE: usize = TILE as usize;

fn new_store() -> (tempfile::TempDir, TileStore) {
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

/// Writes the 8-bit-equivalent premultiplied `rgba` at `(x, y)`.
fn put(store: &mut TileStore, x: u32, y: u32, rgba: [u8; 4]) {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    let Ok(tile) = store.get_mut(surface(), id) else {
        unreachable!("tile");
    };
    let index = ((y % TILE) as usize * SIDE + (x % TILE) as usize) * CHANNELS;
    for (offset, v) in rgba.iter().enumerate() {
        if let Some(sample) = tile.texels_mut().get_mut(index + offset) {
            *sample = f16::from_f32(f32::from(*v) / 255.0);
        }
    }
}

fn spec(seed: (u32, u32), tolerance: u8, contiguous: bool) -> FloodSpec {
    FloodSpec {
        seed,
        tolerance,
        contiguous,
        max_tiles: MAX_FILL_TILES,
        max_examined: MAX_EXAMINED_TILES,
    }
}

fn near(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// The reference: matched flags over the whole `w x h` image.
fn reference(
    image: &[[u8; 4]],
    w: usize,
    h: usize,
    seed: (usize, usize),
    tolerance: u8,
    contiguous: bool,
) -> Vec<bool> {
    let at = |x: usize, y: usize| image.get(y * w + x).copied().unwrap_or([0; 4]);
    let want = at(seed.0, seed.1);
    let mut matched = vec![false; w * h];
    if !contiguous {
        for y in 0..h {
            for x in 0..w {
                matched[y * w + x] = near(want, at(x, y), tolerance);
            }
        }
        return matched;
    }
    let mut stack = vec![seed];
    while let Some((x, y)) = stack.pop() {
        if matched[y * w + x] || !near(want, at(x, y), tolerance) {
            continue;
        }
        matched[y * w + x] = true;
        if x > 0 {
            stack.push((x - 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if x + 1 < w {
            stack.push((x + 1, y));
        }
        if y + 1 < h {
            stack.push((x, y + 1));
        }
    }
    matched
}

#[test]
fn random_images_match_the_brute_force_reference() {
    let (w, h) = (150_usize, 90_usize);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let mut compared = 0;
    for round in 0..8 {
        // A blocky image of a few similar greys with some alpha-zero cells.
        let mut image = vec![[0_u8; 4]; w * h];
        let blocks = 6 + round;
        let palette: Vec<[u8; 4]> = (0..5)
            .map(|k| {
                let v = u8::try_from(40 * k + next(30)).unwrap_or(255);
                if k == 4 { [0, 0, 0, 0] } else { [v, v, v, 255] }
            })
            .collect();
        for y in 0..h {
            for x in 0..w {
                let cell = ((x / blocks) * 31 + (y / blocks) * 17 + round) % 5;
                image[y * w + x] = palette[cell];
            }
        }
        let (_dir, mut store) = new_store();
        for y in 0..h {
            for x in 0..w {
                put(&mut store, x as u32, y as u32, image[y * w + x]);
            }
        }
        for tolerance in [0_u8, 1, 32, 45, 255] {
            for contiguous in [true, false] {
                let seed = (next(w as u64) as usize, next(h as u64) as usize);
                let expect = reference(&image, w, h, seed, tolerance, contiguous);
                let got = match wand_region(
                    &mut store,
                    surface(),
                    (w as u32, h as u32),
                    &spec((seed.0 as u32, seed.1 as u32), tolerance, contiguous),
                ) {
                    Ok(Some(region)) => region,
                    other => unreachable!("{other:?}"),
                };
                // The box is the tight box of the reference.
                let xs: Vec<usize> = (0..w * h).filter(|i| expect[*i]).map(|i| i % w).collect();
                let ys: Vec<usize> = (0..w * h).filter(|i| expect[*i]).map(|i| i / w).collect();
                let (x0, x1) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
                let (y0, y1) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
                assert_eq!(got.bounds.x, x0 as i64, "t{tolerance} c{contiguous}");
                assert_eq!(got.bounds.y, y0 as i64);
                assert_eq!(got.bounds.width as usize, x1 - x0 + 1);
                assert_eq!(got.bounds.height as usize, y1 - y0 + 1);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let byte = got.data[(y - y0) * got.bounds.width as usize + (x - x0)];
                        assert_eq!(
                            byte == 255,
                            expect[y * w + x],
                            "({x}, {y}) tolerance {tolerance} contiguous {contiguous}"
                        );
                        assert!(byte == 0 || byte == 255);
                    }
                }
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 80);
}

#[test]
fn tolerance_is_inclusive_at_its_edge() {
    let (_dir, mut store) = new_store();
    for x in 0..8_u32 {
        put(&mut store, x, 0, [100, 100, 100, 255]);
    }
    put(&mut store, 3, 0, [132, 132, 132, 255]);
    let mut region = |tolerance| match wand_region(
        &mut store,
        surface(),
        (8, 1),
        &spec((0, 0), tolerance, true),
    ) {
        Ok(Some(r)) => r,
        other => unreachable!("{other:?}"),
    };
    // 132 - 100 = 32: inside at 32, outside at 31.
    assert_eq!(region(32).bounds.width, 8);
    assert_eq!(region(31).bounds.width, 3, "stops before the odd pixel");
    assert_eq!(region(0).bounds.width, 3);
}

#[test]
fn contiguous_stops_at_a_barrier_and_the_other_mode_jumps_it() {
    let (_dir, mut store) = new_store();
    for x in 0..9_u32 {
        put(&mut store, x, 0, [10, 10, 10, 255]);
    }
    put(&mut store, 4, 0, [200, 200, 200, 255]);
    let run = |store: &mut TileStore, contiguous| match wand_region(
        store,
        surface(),
        (9, 1),
        &spec((0, 0), 5, contiguous),
    ) {
        Ok(Some(r)) => r,
        other => unreachable!("{other:?}"),
    };
    assert_eq!(run(&mut store, true).bounds.width, 4);
    let all = run(&mut store, false);
    assert_eq!(all.bounds.width, 9);
    assert_eq!(all.data.iter().filter(|b| **b == 255).count(), 8);
}

#[test]
fn a_seed_outside_the_layer_selects_nothing() {
    let (_dir, mut store) = new_store();
    assert!(matches!(
        wand_region(&mut store, surface(), (10, 10), &spec((10, 3), 32, true)),
        Ok(None)
    ));
    assert!(matches!(
        wand_region(&mut store, surface(), (10, 10), &spec((3, 10), 32, true)),
        Ok(None)
    ));
}

#[test]
fn a_blank_layer_is_one_region_and_reading_it_creates_no_tile() {
    let (_dir, mut store) = new_store();
    let region = match wand_region(&mut store, surface(), (100, 70), &spec((5, 5), 0, true)) {
        Ok(Some(r)) => r,
        other => unreachable!("{other:?}"),
    };
    assert_eq!((region.bounds.x, region.bounds.y), (0, 0));
    assert_eq!((region.bounds.width, region.bounds.height), (100, 70));
    assert!(region.data.iter().all(|b| *b == 255));
    for id in [
        TileId { x: 0, y: 0 },
        TileId { x: 1, y: 0 },
        TileId { x: 1, y: 1 },
    ] {
        assert!(!store.contains_tile(surface(), id), "{id:?}");
    }
}

#[test]
fn the_caps_refuse_and_the_sparse_box_is_bounded_too() {
    let (_dir, mut store) = new_store();
    // A 3-tile-wide blank row: a contiguous region of 3 tiles.
    let mut capped = spec((0, 0), 0, true);
    capped.max_tiles = 2;
    assert!(matches!(
        wand_region(&mut store, surface(), (3 * TILE, 1), &capped),
        Err(FillError::TooLarge { max: 2 })
    ));
    // Non-contiguous: examined tiles are bounded.
    put(&mut store, 0, 0, [1, 1, 1, 255]);
    put(&mut store, TILE, 0, [1, 1, 1, 255]);
    let mut examined = spec((0, 0), 0, false);
    examined.max_examined = 1;
    assert!(matches!(
        wand_region(&mut store, surface(), (2 * TILE, TILE), &examined),
        Err(FillError::TooLarge { max: 1 })
    ));
    // Two matched pixels in opposite corners of a 2 x 2 tile layer: two
    // tiles matched (within the tile cap of 2) but a box of four tiles'
    // area, which the box cap refuses.
    let (_dir2, mut sparse) = new_store();
    put(&mut sparse, 0, 0, [9, 9, 9, 255]);
    put(&mut sparse, 2 * TILE - 1, 2 * TILE - 1, [9, 9, 9, 255]);
    let mut boxed = spec((0, 0), 0, false);
    boxed.max_tiles = 2;
    assert!(matches!(
        wand_region(&mut sparse, surface(), (2 * TILE, 2 * TILE), &boxed),
        Err(FillError::TooLarge { max: 2 })
    ));
    boxed.max_tiles = 4;
    assert!(matches!(
        wand_region(&mut sparse, surface(), (2 * TILE, 2 * TILE), &boxed),
        Ok(Some(_))
    ));
}
