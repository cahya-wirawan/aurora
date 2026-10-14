//! `fill_with_coverage` (Fill Path, 0.234.0): a solid colour through a
//! shape given as a coverage. It must be the Paint Bucket where the shape
//! is whole, a proportional blend where it is fractional, and must create
//! nothing it does not paint.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines
)]

use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, FillError, FillMode, FloodSpec, MAX_EXAMINED_TILES, MAX_FILL_TILES, ShapeFillSpec,
    StrokeSnapshot, fill_with_coverage, flood_fill,
};

const RED: [f32; 3] = [1.0, 0.25, 0.0];

fn surface() -> SurfaceId {
    SurfaceId::from_raw(0)
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = tempfile::tempdir().unwrap_or_else(|e| unreachable!("tempdir: {e}"));
    let budget = NonZeroUsize::new(64).unwrap_or_else(|| unreachable!("non-zero"));
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => (dir, store),
        Err(err) => unreachable!("store: {err:?}"),
    }
}

/// A rectangle with one constant coverage.
struct Block(Rect, f32);

impl Coverage for Block {
    fn bounds(&self) -> Option<Rect> {
        Some(self.0)
    }
    fn at(&self, x: i64, y: i64) -> f32 {
        if self.0.contains_point(x, y) {
            self.1
        } else {
            0.0
        }
    }
}

fn rect(x: i64, y: i64, w: u32, h: u32) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn put(store: &mut TileStore, x: u32, y: u32, texel: [f16; 4]) {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    let tile = store
        .get_mut(surface(), id)
        .unwrap_or_else(|_| unreachable!("tile"));
    let at = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    for (i, v) in texel.iter().enumerate() {
        tile.texels_mut()[at + i] = *v;
    }
}

fn get(store: &mut TileStore, x: u32, y: u32) -> [f16; 4] {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    if !store.contains_tile(surface(), id) {
        return [f16::ZERO; 4];
    }
    let tile = store
        .get(surface(), id)
        .unwrap_or_else(|_| unreachable!("tile"));
    let at = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    [0, 1, 2, 3].map(|i| tile.texels()[at + i])
}

fn h(v: f32) -> f16 {
    f16::from_f32(v)
}

/// A varied layer so a blend has something to show.
fn varied(store: &mut TileStore, w: u32, ht: u32) {
    for y in 0..ht {
        for x in 0..w {
            let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
            put(
                store,
                x,
                y,
                [h(v * 0.5), h(v * 0.25), h(v * 0.75), h(0.5 + v * 0.5)],
            );
        }
    }
}

fn spec(opacity: f32) -> ShapeFillSpec {
    ShapeFillSpec {
        colour: RED,
        opacity,
        max_tiles: MAX_FILL_TILES,
    }
}

fn all_bits(store: &mut TileStore, w: u32, ht: u32) -> Vec<u16> {
    let mut out = Vec::new();
    for y in 0..ht {
        for x in 0..w {
            out.extend(get(store, x, y).iter().map(|v| v.to_bits()));
        }
    }
    out
}

#[test]
fn whole_coverage_at_full_opacity_is_the_paint_bucket() {
    for mode in [FillMode::Normal, FillMode::PreserveAlpha] {
        let (_d1, mut a) = store();
        let (_d2, mut b) = store();
        varied(&mut a, 90, 70);
        varied(&mut b, 90, 70);
        // A transparent corner too, for the lock to skip.
        put(&mut a, 3, 3, [f16::ZERO; 4]);
        put(&mut b, 3, 3, [f16::ZERO; 4]);
        let flood = FloodSpec {
            seed: (0, 0),
            tolerance: 255,
            contiguous: true,
            max_tiles: MAX_FILL_TILES,
            max_examined: MAX_EXAMINED_TILES,
        };
        let bucket = flood_fill(&mut a, surface(), (90, 70), &flood, RED, mode, None);
        assert!(bucket.is_ok());
        let shape = Block(rect(0, 0, 90, 70), 1.0);
        let fill = fill_with_coverage(
            &mut b,
            surface(),
            (0, 0, 90, 70),
            &shape,
            &spec(1.0),
            mode,
            None,
            None,
        );
        assert!(fill.is_ok());
        assert_eq!(
            all_bits(&mut a, 90, 70),
            all_bits(&mut b, 90, 70),
            "{mode:?}"
        );
    }
}

#[test]
fn fractional_coverage_and_opacity_blend_proportionally() {
    let (_d, mut s) = store();
    put(&mut s, 5, 5, [h(0.0), h(0.0), h(0.5), h(0.5)]);
    let shape = Block(rect(0, 0, 10, 10), 0.5);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 10, 10),
        &shape,
        &spec(0.5),
        FillMode::Normal,
        None,
        None,
    );
    assert!(out.is_ok());
    // amount 0.25: colour * 0.25 + old * 0.75
    let px = get(&mut s, 5, 5).map(f16::to_f32);
    assert!((px[0] - 0.25).abs() < 2e-3, "{px:?}");
    assert!((px[1] - 0.0625).abs() < 2e-3);
    assert!((px[2] - 0.375).abs() < 2e-3);
    assert!((px[3] - (0.25 + 0.5 * 0.75)).abs() < 2e-3);
}

#[test]
fn the_selection_multiplies_the_amount() {
    let (_d1, mut a) = store();
    let (_d2, mut b) = store();
    varied(&mut a, 20, 20);
    varied(&mut b, 20, 20);
    let whole = Block(rect(0, 0, 20, 20), 1.0);
    let half = Block(rect(0, 0, 20, 20), 0.5);
    let x = fill_with_coverage(
        &mut a,
        surface(),
        (0, 0, 20, 20),
        &whole,
        &spec(1.0),
        FillMode::Normal,
        None,
        Some(&half),
    );
    let y = fill_with_coverage(
        &mut b,
        surface(),
        (0, 0, 20, 20),
        &half,
        &spec(1.0),
        FillMode::Normal,
        None,
        None,
    );
    assert!(x.is_ok() && y.is_ok());
    for (p, q) in all_bits(&mut a, 20, 20)
        .iter()
        .zip(all_bits(&mut b, 20, 20))
    {
        assert!((f16::from_bits(*p).to_f32() - f16::from_bits(q).to_f32()).abs() < 2e-3);
    }
}

#[test]
fn the_transparency_lock_keeps_alpha_bit_for_bit_and_skips_transparent() {
    let (_d, mut s) = store();
    varied(&mut s, 16, 16);
    put(&mut s, 2, 2, [f16::ZERO; 4]);
    let before: Vec<u16> = (0..16)
        .flat_map(|x| get(&mut s, x, 4))
        .map(f16::to_bits)
        .collect();
    let shape = Block(rect(0, 0, 16, 16), 0.7);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 16, 16),
        &shape,
        &spec(0.8),
        FillMode::PreserveAlpha,
        None,
        None,
    );
    assert!(out.is_ok());
    let after: Vec<u16> = (0..16)
        .flat_map(|x| get(&mut s, x, 4))
        .map(f16::to_bits)
        .collect();
    for i in (3..before.len()).step_by(4) {
        assert_eq!(before[i], after[i], "alpha at {i}");
    }
    assert_ne!(before, after);
    assert_eq!(get(&mut s, 2, 2), [f16::ZERO; 4]);
}

#[test]
fn an_uncovered_or_zero_opacity_fill_creates_no_tile_and_captures_nothing() {
    let (_d, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    let none = Block(rect(0, 0, 10, 10), 0.0);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 10, 10),
        &none,
        &spec(1.0),
        FillMode::Normal,
        Some(&mut snap),
        None,
    );
    assert!(out.is_ok_and(|o| o.painted().is_empty()));
    let some = Block(rect(0, 0, 10, 10), 1.0);
    for opacity in [0.0, f32::NAN, -1.0] {
        let out = fill_with_coverage(
            &mut s,
            surface(),
            (0, 0, 10, 10),
            &some,
            &spec(opacity),
            FillMode::Normal,
            Some(&mut snap),
            None,
        );
        assert!(out.is_ok_and(|o| o.painted().is_empty()));
    }
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
    assert!(snap.is_empty());
    // A shape in another tile does not touch this one.
    let far = Block(rect(300, 0, 10, 10), 1.0);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 400, 100),
        &far,
        &spec(1.0),
        FillMode::Normal,
        Some(&mut snap),
        None,
    );
    assert!(out.is_ok_and(|o| o.painted().len() == 1));
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
}

#[test]
fn the_shape_and_selection_bounds_clip_the_area_and_the_cap_applies_before_any_write() {
    let (_d, mut s) = store();
    let big = Block(rect(0, 0, 100_000, 100_000), 1.0);
    let spec_small = ShapeFillSpec {
        colour: RED,
        opacity: 1.0,
        max_tiles: 4,
    };
    let refused = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 100_000, 100_000),
        &big,
        &spec_small,
        FillMode::Normal,
        None,
        None,
    );
    assert!(matches!(refused, Err(FillError::TooLarge { max: 4 })));
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
    // The same area through a small selection fits.
    let sel = Block(rect(10, 10, 20, 20), 1.0);
    let ok = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 100_000, 100_000),
        &big,
        &spec_small,
        FillMode::Normal,
        None,
        Some(&sel),
    );
    assert!(ok.is_ok_and(|o| o.painted().len() == 1));
    assert_eq!(get(&mut s, 15, 15)[3], f16::ONE);
    assert_eq!(get(&mut s, 5, 5)[3], f16::ZERO);
}

#[test]
fn one_snapshot_restores_the_layer() {
    let (_d, mut s) = store();
    varied(&mut s, 70, 70);
    let before = all_bits(&mut s, 70, 70);
    let mut snap = StrokeSnapshot::new(surface());
    let shape = Block(rect(10, 10, 60, 60), 0.6);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 70, 70),
        &shape,
        &spec(1.0),
        FillMode::Normal,
        Some(&mut snap),
        None,
    );
    assert!(out.is_ok_and(|o| !o.painted().is_empty()));
    assert_ne!(all_bits(&mut s, 70, 70), before);
    assert!(snap.apply(&mut s).is_ok());
    assert_eq!(all_bits(&mut s, 70, 70), before);
}

#[test]
fn a_small_shape_in_a_huge_area_only_counts_the_tiles_it_reaches() {
    let (_d, mut s) = store();
    let small = Block(rect(10, 10, 20, 20), 1.0);
    let tight = ShapeFillSpec {
        colour: RED,
        opacity: 1.0,
        max_tiles: 1,
    };
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 100_000, 100_000),
        &small,
        &tight,
        FillMode::Normal,
        None,
        None,
    );
    assert!(out.is_ok_and(|o| o.painted().len() == 1));
    assert_eq!(get(&mut s, 15, 15)[3], f16::ONE);
}

#[test]
fn the_transparency_lock_recolours_by_the_amount_toward_colour_times_alpha() {
    let (_d, mut s) = store();
    put(&mut s, 3, 3, [h(0.0), h(0.0), h(0.5), h(0.5)]);
    let shape = Block(rect(0, 0, 8, 8), 1.0);
    let out = fill_with_coverage(
        &mut s,
        surface(),
        (0, 0, 8, 8),
        &shape,
        &spec(0.25),
        FillMode::PreserveAlpha,
        None,
        None,
    );
    assert!(out.is_ok());
    let px = get(&mut s, 3, 3).map(f16::to_f32);
    // red: 0 * 0.75 + 1.0 * 0.5 * 0.25; blue: 0.5 * 0.75 + 0 * 0.5 * 0.25
    assert!((px[0] - 0.125).abs() < 1e-3, "{px:?}");
    assert!((px[1] - 0.25 * 0.25 * 0.5).abs() < 1e-3, "{px:?}");
    assert!((px[2] - 0.375).abs() < 1e-3, "{px:?}");
    assert_eq!(px[3], 0.5);
}
