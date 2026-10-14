//! The Spot Healing Brush's engine (0.237.0): the stroke mask, the ring and
//! source search, the gradient-domain fill on synthetic images (flat,
//! linear, harmonic, striped), the refusals, determinism, the soft edge, the
//! transparency lock, selection coverage, undo, no phantom tiles and
//! unreadable tiles.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines
)]

use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::spot_heal::{dilate, margin_for, search_radius};
use crate::{
    Coverage, DabOutcome, HEAL_FEATHER, HealMask, HealRefusal, MAX_HEAL_SIDE, MAX_HEAL_TEXELS,
    MIN_RING_TEXELS, PixelHistory, StrokeSnapshot, spot_heal_covered,
};

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

fn put(store: &mut TileStore, x: u32, y: u32, v: [f32; 4]) {
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
    for (c, value) in v.iter().enumerate() {
        if let Some(slot) = tile.texels_mut().get_mut(index + c) {
            *slot = f16::from_f32(*value);
        }
    }
}

/// An opaque premultiplied texel from straight colour and alpha.
fn texel(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    [r * a, g * a, b * a, a]
}

fn get(store: &mut TileStore, x: u32, y: u32) -> [f16; CHANNELS] {
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

fn getf(store: &mut TileStore, x: u32, y: u32) -> [f32; 4] {
    get(store, x, y).map(f16::to_f32)
}

fn bits(t: [f16; CHANNELS]) -> [u16; 4] {
    t.map(f16::to_bits)
}

/// Paints `w x h` texels from the origin with `f(x, y)` (premultiplied).
fn paint(s: &mut TileStore, w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 4]) {
    for y in 0..h {
        for x in 0..w {
            put(s, x, y, f(x, y));
        }
    }
}

fn dump(s: &mut TileStore, w: u32, h: u32) -> Vec<[u16; 4]> {
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            out.push(bits(get(s, x, y)));
        }
    }
    out
}

fn disc(cx: f32, cy: f32, r: f32) -> HealMask {
    let mut mask = HealMask::new();
    mask.add_dab((cx, cy), r);
    mask
}

fn heal(
    s: &mut TileStore,
    mask: &HealMask,
    preserve: bool,
    snap: Option<&mut StrokeSnapshot>,
    cov: Option<&dyn Coverage>,
) -> Result<DabOutcome, HealRefusal> {
    spot_heal_covered(s, surface(), mask, 8.0, preserve, snap, cov)
}

struct Constant(f32);

impl Coverage for Constant {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, _: i64, _: i64) -> f32 {
        self.0
    }
}

struct RightOf(i64);

impl Coverage for RightOf {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, x: i64, _: i64) -> f32 {
        if x >= self.0 { 1.0 } else { 0.0 }
    }
}

fn close(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tol)
}

// ---------------------------------------------------------------- the mask

#[test]
fn a_dab_adds_the_texels_whose_centres_are_strictly_inside_the_radius() {
    let mask = disc(10.5, 10.5, 3.0);
    let mut want = 0;
    for y in 0..20_i64 {
        for x in 0..20_i64 {
            let d2 = (x as f32 + 0.5 - 10.5).powi(2) + (y as f32 + 0.5 - 10.5).powi(2);
            let inside = d2 < 9.0;
            assert_eq!(mask.contains(x, y), inside, "({x}, {y})");
            want += u64::from(inside);
        }
    }
    assert_eq!(mask.count(), want);
    // Exactly on the radius is out (strict): the centre texel's neighbour
    // at distance 3.0.
    assert!(!mask.contains(13, 10));
    assert!(mask.contains(12, 10));
}

#[test]
fn dabs_union_and_the_box_grows_without_losing_earlier_texels() {
    let mut mask = HealMask::new();
    mask.add_dab((10.5, 10.5), 2.0);
    let first: Vec<(i64, i64)> = (0..30_i64)
        .flat_map(|y| (0..30_i64).map(move |x| (x, y)))
        .filter(|&(x, y)| mask.contains(x, y))
        .collect();
    mask.add_dab((40.5, 5.5), 2.0);
    mask.add_dab((5.5, 40.5), 2.0);
    for &(x, y) in &first {
        assert!(mask.contains(x, y), "({x}, {y}) survived the growth");
    }
    assert!(mask.contains(40, 5) && mask.contains(5, 40));
    assert!(!mask.contains(40, 40));
    let Some((x0, y0, x1, y1)) = mask.tight_bounds() else {
        unreachable!("not empty");
    };
    assert_eq!((x0, y0), (4, 4));
    assert_eq!((x1, y1), (41, 41));
    // The same dab twice adds nothing.
    let count = mask.count();
    mask.add_dab((10.5, 10.5), 2.0);
    assert_eq!(mask.count(), count);
}

#[test]
fn a_click_is_one_disc_and_texels_left_of_the_origin_are_skipped() {
    let mask = disc(1.0, 1.0, 4.0);
    assert!(mask.contains(0, 0));
    assert!(!mask.contains(-1, 0) && !mask.contains(0, -1));
    assert!(mask.count() > 0);
    let Some((x0, y0, _, _)) = mask.tight_bounds() else {
        unreachable!("not empty");
    };
    assert_eq!((x0, y0), (0, 0));
}

#[test]
fn bad_dabs_add_nothing() {
    let mut mask = HealMask::new();
    for (c, r) in [
        ((f32::NAN, 5.0), 3.0),
        ((5.0, f32::INFINITY), 3.0),
        ((5.0, 5.0), f32::NAN),
        ((5.0, 5.0), 0.0),
        ((5.0, 5.0), -2.0),
        ((-50.0, 5.0), 3.0),
        ((3.0e9, 5.0), 3.0),
    ] {
        mask.add_dab(c, r);
        assert!(mask.is_empty(), "{c:?} {r}");
        assert!(!mask.overflowed());
    }
}

#[test]
fn a_stroke_past_the_bound_stops_accumulating_and_is_flagged() {
    let mut mask = HealMask::new();
    mask.add_dab((10.5, 10.5), 3.0);
    let count = mask.count();
    // A dab 2,000 texels away would need a box over the side bound.
    mask.add_dab((10.5 + MAX_HEAL_SIDE as f32 + 10.0, 10.5), 3.0);
    assert!(mask.overflowed());
    assert_eq!(mask.count(), count);
    // Later dabs are ignored, even close ones.
    mask.add_dab((11.5, 11.5), 3.0);
    assert_eq!(mask.count(), count);
    // The area bound: a box of 1,100 x 1,000 is over 1 Mpx (sides are fine).
    let mut wide = HealMask::new();
    wide.add_dab((5.5, 5.5), 4.0);
    wide.add_dab((1100.5, 1000.5), 4.0);
    assert!(wide.overflowed());
    assert!(u64::from(1100_u32) * 1000 > MAX_HEAL_TEXELS);
    // A stroke inside the bound does not overflow.
    let mut fine = HealMask::new();
    fine.add_dab((5.5, 5.5), 4.0);
    fine.add_dab((900.5, 900.5), 4.0);
    assert!(!fine.overflowed());
}

// ----------------------------------------------------------------- helpers

#[test]
fn dilation_is_chebyshev_and_exact() {
    let (w, h) = (15, 12);
    let mut src = vec![false; w * h];
    src[5 * w + 7] = true;
    src[0] = true;
    for r in 0..4_usize {
        let out = dilate(&src, w, h, r);
        for y in 0..h {
            for x in 0..w {
                let near = |sx: usize, sy: usize| sx.abs_diff(x).max(sy.abs_diff(y)) <= r;
                let want = near(7, 5) || near(0, 0);
                assert_eq!(out[y * w + x], want, "r={r} ({x}, {y})");
            }
        }
    }
}

#[test]
fn the_margin_and_search_radius_follow_their_rules() {
    assert_eq!(margin_for(1.0), 8);
    assert_eq!(margin_for(8.0), 8);
    assert_eq!(margin_for(8.2), 9);
    assert_eq!(margin_for(30.0), 30);
    assert_eq!(margin_for(64.0), 64);
    assert_eq!(margin_for(200.0), 64);
    assert_eq!(margin_for(f32::NAN), 8);
    assert_eq!(search_radius(1), 16);
    assert_eq!(search_radius(10), 40);
    assert_eq!(search_radius(30), 120);
    assert_eq!(search_radius(500), 128);
}

// -------------------------------------------------------------- the fills

#[test]
fn a_dot_on_a_flat_colour_is_removed() {
    let (_dir, mut s) = store();
    let flat = texel(0.5, 0.25, 0.75, 1.0);
    paint(&mut s, 80, 80, |x, y| {
        let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 40.0).powi(2);
        if d2 < 25.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            flat
        }
    });
    let mask = disc(40.5, 40.5, 7.0);
    let out = heal(&mut s, &mask, false, None, None);
    assert!(out.is_ok_and(|o| !o.painted().is_empty()));
    for y in 0..80 {
        for x in 0..80 {
            assert!(
                close(getf(&mut s, x, y), flat, 2.0e-3),
                "({x}, {y}) {:?}",
                getf(&mut s, x, y)
            );
        }
    }
}

#[test]
fn a_blob_on_a_linear_gradient_is_restored_within_a_small_bound() {
    let (_dir, mut s) = store();
    let truth = |x: u32, y: u32| texel(x as f32 / 90.0, y as f32 / 90.0, 0.5, 1.0);
    paint(&mut s, 90, 90, |x, y| {
        let d2 = (x as f32 - 45.0).powi(2) + (y as f32 - 45.0).powi(2);
        if d2 < 60.0 {
            texel(0.0, 1.0, 0.0, 1.0)
        } else {
            truth(x, y)
        }
    });
    let mask = disc(45.5, 45.5, 9.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    let mut worst = 0.0_f32;
    for y in 0..90 {
        for x in 0..90 {
            let got = getf(&mut s, x, y);
            let want = truth(x, y);
            for c in 0..4 {
                worst = worst.max((got[c] - want[c]).abs());
            }
        }
    }
    assert!(worst <= 3.0e-3, "worst {worst}");
}

#[test]
fn a_harmonic_surface_is_reproduced_by_the_solve() {
    let (_dir, mut s) = store();
    let truth = |x: u32, y: u32| {
        let (dx, dy) = (x as f32 - 40.0, y as f32 - 40.0);
        let v = 0.5 + (dx * dx - dy * dy) / 8000.0;
        texel(v, 0.5 - (dx * dy) / 4000.0, 0.4, 1.0)
    };
    paint(&mut s, 80, 80, |x, y| {
        let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 40.0).powi(2);
        if d2 < 100.0 {
            texel(0.0, 0.0, 0.0, 1.0)
        } else {
            truth(x, y)
        }
    });
    let mask = disc(40.5, 40.5, 11.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    let mut worst = 0.0_f32;
    for y in 0..80 {
        for x in 0..80 {
            let (got, want) = (getf(&mut s, x, y), truth(x, y));
            for c in 0..4 {
                worst = worst.max((got[c] - want[c]).abs());
            }
        }
    }
    assert!(worst <= 3.0e-3, "worst {worst}");
}

fn stripes(x: u32, y: u32) -> [f32; 4] {
    let bright = if x % 6 < 3 { 0.8 } else { 0.2 };
    texel(bright, 0.3 + 0.1 * (y % 4) as f32 / 4.0, 0.5, 1.0)
}

#[test]
fn stripes_continue_through_a_blemish() {
    let (_dir, mut s) = store();
    paint(&mut s, 96, 96, |x, y| {
        let d2 = (x as f32 - 48.0).powi(2) + (y as f32 - 48.0).powi(2);
        if d2 < 36.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            stripes(x, y)
        }
    });
    let mask = disc(48.5, 48.5, 7.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    // Correlation of the healed red channel with the true stripes over the
    // blemish, and a tight error bound: the source search found the period.
    let (mut sum_g, mut sum_t, mut n) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut pairs = Vec::new();
    for y in 40..57 {
        for x in 40..57 {
            let got = f64::from(getf(&mut s, x, y)[0]);
            let want = f64::from(stripes(x, y)[0]);
            pairs.push((got, want));
            sum_g += got;
            sum_t += want;
            n += 1.0;
        }
    }
    let (mg, mt) = (sum_g / n, sum_t / n);
    let (mut cov, mut vg, mut vt) = (0.0, 0.0, 0.0);
    for (g, t) in pairs {
        cov += (g - mg) * (t - mt);
        vg += (g - mg) * (g - mg);
        vt += (t - mt) * (t - mt);
    }
    let corr = cov / (vg.sqrt() * vt.sqrt());
    assert!(corr > 0.98, "correlation {corr}");
    for y in 41..56 {
        for x in 41..56 {
            assert!(
                close(getf(&mut s, x, y), stripes(x, y), 0.03),
                "({x}, {y}) {:?} vs {:?}",
                getf(&mut s, x, y),
                stripes(x, y)
            );
        }
    }
}

#[test]
fn the_heal_is_deterministic() {
    let build = |s: &mut TileStore| {
        paint(s, 96, 96, |x, y| {
            let d2 = (x as f32 - 48.0).powi(2) + (y as f32 - 48.0).powi(2);
            if d2 < 36.0 {
                texel(1.0, 0.0, 0.0, 1.0)
            } else {
                stripes(x, y)
            }
        });
    };
    let mask = disc(48.5, 48.5, 7.0);
    let (_d1, mut a) = store();
    let (_d2, mut b) = store();
    build(&mut a);
    build(&mut b);
    assert!(heal(&mut a, &mask, false, None, None).is_ok());
    assert!(heal(&mut b, &mask, false, None, None).is_ok());
    assert_eq!(dump(&mut a, 96, 96), dump(&mut b, 96, 96));
}

#[test]
fn only_the_mask_and_its_two_texel_skirt_can_change_and_the_edge_is_soft() {
    let (_dir, mut s) = store();
    let flat = texel(0.2, 0.4, 0.6, 1.0);
    // A blemish wider than the mask, so texels just outside the mask are
    // blemished too.
    paint(&mut s, 80, 80, |x, y| {
        let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 40.0).powi(2);
        if d2 < 400.0 {
            texel(0.9, 0.9, 0.1, 1.0)
        } else {
            flat
        }
    });
    let before = dump(&mut s, 80, 80);
    let mask = disc(40.5, 40.5, 6.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    let after = dump(&mut s, 80, 80);
    let mut inside_ok = true;
    for y in 0..80_i64 {
        for x in 0..80_i64 {
            let near = (-HEAL_FEATHER..=HEAL_FEATHER)
                .any(|dy| (-HEAL_FEATHER..=HEAL_FEATHER).any(|dx| mask.contains(x + dx, y + dy)));
            let index = (y * 80 + x) as usize;
            if !near {
                assert_eq!(after[index], before[index], "({x}, {y}) is outside");
            }
            if mask.contains(x, y) {
                // Inside the mask the blemish is gone (the ring is the
                // blemish colour here, so the fill is it: unchanged)...
                inside_ok &= true;
            }
        }
    }
    assert!(inside_ok);
}

#[test]
fn the_soft_edge_blends_by_the_box_mean_of_the_mask() {
    // The ring is flat red; inside the domain (the mask's 33..=47 square)
    // the old content is green: the fill is red, and the skirt texels take
    // red in proportion to the 5 x 5 mean of the mask.
    let (_dir, mut s) = store();
    let mask = disc(40.5, 40.5, 6.0);
    let in_domain =
        |x: i64, y: i64| (-2..=2).any(|dy| (-2..=2).any(|dx| mask.contains(x + dx, y + dy)));
    paint(&mut s, 80, 80, |x, y| {
        if in_domain(i64::from(x), i64::from(y)) {
            texel(0.0, 1.0, 0.0, 1.0)
        } else {
            texel(1.0, 0.0, 0.0, 1.0)
        }
    });
    assert_eq!(mask.tight_bounds(), Some((35, 35, 45, 45)));
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    // Inside the mask: fully healed (red).
    assert!(close(getf(&mut s, 40, 40), [1.0, 0.0, 0.0, 1.0], 2.0e-3));
    assert!(close(getf(&mut s, 44, 40), [1.0, 0.0, 0.0, 1.0], 2.0e-3));
    assert!(close(getf(&mut s, 45, 40), [1.0, 0.0, 0.0, 1.0], 2.0e-3));
    // Along the +x axis the mask ends at x = 45, so 46 and 47 are skirt.
    assert!(mask.contains(45, 40) && !mask.contains(46, 40));
    let box_mean = |x: i64, y: i64| {
        let mut n = 0;
        for dy in -2..=2 {
            for dx in -2..=2 {
                n += i32::from(mask.contains(x + dx, y + dy));
            }
        }
        n as f32 / 25.0
    };
    let mut seen = Vec::new();
    for x in [46_u32, 47] {
        let edge = box_mean(i64::from(x), 40);
        assert!(edge > 0.0 && edge < 1.0);
        let got = getf(&mut s, x, 40);
        // old = green (0, 1, 0), healed = red (1, 0, 0).
        assert!((got[0] - edge).abs() < 3.0e-3, "x={x} {got:?} edge {edge}");
        assert!((got[1] - (1.0 - edge)).abs() < 3.0e-3, "x={x} {got:?}");
        seen.push(edge);
    }
    assert!(seen[0] > seen[1], "the edge falls off with distance");
}

// ---------------------------------------------------------------- refusals

#[test]
fn too_little_around_the_mask_is_refused_and_writes_nothing() {
    let (_dir, mut s) = store();
    // A 5 x 5 opaque patch with the mask over most of it: far fewer than
    // MIN_RING_TEXELS opaque texels outside the domain.
    paint(&mut s, 5, 5, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let before = dump(&mut s, 12, 12);
    let mask = disc(2.5, 2.5, 2.4);
    assert!(MIN_RING_TEXELS > 4);
    assert_eq!(
        heal(&mut s, &mask, false, None, None).err(),
        Some(HealRefusal::NotEnoughSurroundings)
    );
    assert_eq!(dump(&mut s, 12, 12), before);
    // An empty layer has no ring at all.
    let (_dir2, mut empty) = store();
    assert_eq!(
        heal(&mut empty, &disc(20.5, 20.5, 5.0), false, None, None).err(),
        Some(HealRefusal::NotEnoughSurroundings)
    );
    assert!(!empty.contains_tile(surface(), TileId { x: 0, y: 0 }));
}

#[test]
fn a_ring_of_exactly_the_minimum_heals_and_one_short_is_refused() {
    // A one-texel mask at (50, 50): the domain is x, y in 48..=52. The ring
    // is every opaque texel outside it; paint exactly `n` in a row from
    // (53, 50), the first of which is also the mask's boundary neighbour.
    for (n, ok) in [(MIN_RING_TEXELS - 1, false), (MIN_RING_TEXELS, true)] {
        let (_dir, mut s) = store();
        for k in 0..n {
            put(
                &mut s,
                53 + (k % 8) as u32,
                50 + (k / 8) as u32,
                texel(0.5, 0.5, 0.5, 1.0),
            );
        }
        let mut mask = HealMask::new();
        mask.add_dab((50.5, 50.5), 0.9);
        assert_eq!(mask.count(), 1);
        let result = heal(&mut s, &mask, false, None, None);
        assert_eq!(result.is_ok(), ok, "n = {n}: {:?}", result.err());
    }
}

#[test]
fn a_part_of_the_mask_no_ring_reaches_is_refused() {
    let (_dir, mut s) = store();
    // Opaque on the left half, nothing on the right.
    paint(&mut s, 60, 100, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let before = dump(&mut s, 120, 100);
    let mut mask = HealMask::new();
    mask.add_dab((30.5, 50.5), 5.0);
    // Reachable on its own.
    assert!(
        heal(&mut s, &mask, false, None, None).is_ok(),
        "the left dab alone heals"
    );
    // Restore and add a dab in the transparent half: its texels have no
    // opaque neighbour, so no ring reaches them.
    let (_dir2, mut s2) = store();
    paint(&mut s2, 60, 100, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    mask.add_dab((100.5, 50.5), 5.0);
    assert_eq!(
        heal(&mut s2, &mask, false, None, None).err(),
        Some(HealRefusal::NotEnoughSurroundings)
    );
    assert_eq!(dump(&mut s2, 120, 100), {
        let (_dir3, mut again) = store();
        paint(&mut again, 60, 100, |_, _| texel(0.5, 0.5, 0.5, 1.0));
        dump(&mut again, 120, 100)
    });
    let _ = before;
}

#[test]
fn an_overflowed_mask_is_refused_as_too_large() {
    let (_dir, mut s) = store();
    paint(&mut s, 64, 64, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let before = dump(&mut s, 64, 64);
    let mut mask = disc(20.5, 20.5, 4.0);
    mask.add_dab((20.5 + MAX_HEAL_SIDE as f32 + 5.0, 20.5), 4.0);
    assert_eq!(
        heal(&mut s, &mask, false, None, None).err(),
        Some(HealRefusal::TooLarge)
    );
    assert_eq!(dump(&mut s, 64, 64), before);
}

#[test]
fn an_empty_mask_does_nothing() {
    let (_dir, mut s) = store();
    paint(&mut s, 64, 64, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let before = dump(&mut s, 64, 64);
    let out = heal(&mut s, &HealMask::new(), false, None, None);
    assert!(out.is_ok_and(|o| o.painted().is_empty() && o.is_complete()));
    assert_eq!(dump(&mut s, 64, 64), before);
}

// ------------------------------------------------- locks, selection, undo

#[test]
fn the_transparency_lock_keeps_every_alpha_bit_for_bit() {
    // Alpha is a smooth ramp except inside the blemish, where it is low and
    // the colour is red.
    let build = |s: &mut TileStore| {
        paint(s, 80, 80, |x, y| {
            let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 40.0).powi(2);
            if d2 < 36.0 {
                texel(1.0, 0.0, 0.0, 0.3)
            } else {
                texel(0.2, 0.6, 0.3, 0.6 + x as f32 / 400.0)
            }
        });
    };
    let (_dir, mut s) = store();
    build(&mut s);
    let before = dump(&mut s, 80, 80);
    let mask = disc(40.5, 40.5, 7.0);
    assert!(heal(&mut s, &mask, true, None, None).is_ok());
    let after = dump(&mut s, 80, 80);
    let mut changed = 0;
    for i in 0..before.len() {
        assert_eq!(after[i][3], before[i][3], "alpha at {i}");
        changed += usize::from(after[i] != before[i]);
    }
    assert!(changed > 50, "the colour did change ({changed})");
    // The colour moved to the surroundings' straight green at the old alpha.
    let t = getf(&mut s, 40, 40);
    assert!((t[3] - 0.3).abs() < 1.0e-3);
    assert!(
        (t[1] / t[3] - 0.6).abs() < 0.02 && (t[0] / t[3] - 0.2).abs() < 0.02,
        "{t:?}"
    );
    // Without the lock the alpha is solved too.
    let (_dir2, mut free) = store();
    build(&mut free);
    assert!(heal(&mut free, &mask, false, None, None).is_ok());
    let t = getf(&mut free, 40, 40);
    assert!((t[3] - (0.6 + 40.0 / 400.0)).abs() < 0.02, "{t:?}");
    assert!(dump(&mut free, 80, 80) != after);
}

#[test]
fn a_texel_with_no_alpha_is_left_alone_under_the_lock() {
    let (_dir, mut s) = store();
    paint(&mut s, 80, 80, |x, y| {
        if (38..42).contains(&x) && (38..42).contains(&y) {
            [0.0; 4]
        } else {
            texel(0.2, 0.6, 0.3, 1.0)
        }
    });
    let mask = disc(40.5, 40.5, 7.0);
    assert!(heal(&mut s, &mask, true, None, None).is_ok());
    assert_eq!(bits(get(&mut s, 40, 40)), [0, 0, 0, 0]);
}

#[test]
fn selection_coverage_scales_the_change() {
    let build = |s: &mut TileStore| {
        paint(s, 80, 80, |x, y| {
            let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 40.0).powi(2);
            if d2 < 100.0 {
                texel(1.0, 0.0, 0.0, 1.0)
            } else {
                texel(0.0, 0.0, 1.0, 1.0)
            }
        });
    };
    let mask = disc(40.5, 40.5, 8.0);
    let (_d0, mut full) = store();
    build(&mut full);
    assert!(heal(&mut full, &mask, false, None, None).is_ok());
    let (_d1, mut half) = store();
    build(&mut half);
    assert!(heal(&mut half, &mask, false, None, Some(&Constant(0.5))).is_ok());
    // Blue is the surroundings; red the old. At full coverage the centre is
    // blue; at half coverage it is the midpoint.
    let f = getf(&mut full, 40, 40);
    let h = getf(&mut half, 40, 40);
    assert!(close(f, [0.0, 0.0, 1.0, 1.0], 2.0e-3), "{f:?}");
    assert!(close(h, [0.5, 0.0, 0.5, 1.0], 3.0e-3), "{h:?}");
    // Zero coverage on the left half: texels there are untouched.
    let (_d2, mut part) = store();
    build(&mut part);
    assert!(heal(&mut part, &mask, false, None, Some(&RightOf(40))).is_ok());
    assert!(close(getf(&mut part, 36, 40), [1.0, 0.0, 0.0, 1.0], 0.0));
    assert!(close(getf(&mut part, 44, 40), [0.0, 0.0, 1.0, 1.0], 2.0e-3));
    // No coverage at all changes nothing and records nothing.
    let (_d3, mut none) = store();
    build(&mut none);
    let before = dump(&mut none, 80, 80);
    let out = heal(&mut none, &mask, false, None, Some(&Constant(0.0)));
    assert!(out.is_ok_and(|o| o.painted().is_empty()));
    assert_eq!(dump(&mut none, 80, 80), before);
}

#[test]
fn one_snapshot_undoes_the_whole_heal_bit_for_bit_and_redoes_it() {
    let (_dir, mut s) = store();
    paint(&mut s, 300, 80, |x, y| {
        let d2 = (x as f32 - 256.0).powi(2) + (y as f32 - 40.0).powi(2);
        if d2 < 100.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            stripes(x, y)
        }
    });
    let before = dump(&mut s, 300, 80);
    let mut snap = StrokeSnapshot::new(surface());
    // The mask straddles the tile boundary at x = 256.
    let mask = disc(256.0, 40.5, 9.0);
    let out = heal(&mut s, &mask, false, Some(&mut snap), None);
    let Ok(out) = out else {
        unreachable!("heals");
    };
    assert_eq!(out.painted().len(), 2, "both tiles of the boundary");
    let healed = dump(&mut s, 300, 80);
    assert_ne!(healed, before);
    let captured: Vec<TileId> = snap.captured().collect();
    assert_eq!(captured.len(), 2);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 80), before);
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 80), healed);
}

#[test]
fn no_tile_is_created_unless_a_texel_in_it_changes() {
    let (_dir, mut s) = store();
    paint(&mut s, 100, 100, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    // The search window reaches tiles (1, 0), (0, 1) and (1, 1), which the
    // layer does not hold.
    let mask = disc(50.5, 50.5, 6.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    for id in [
        TileId { x: 1, y: 0 },
        TileId { x: 0, y: 1 },
        TileId { x: 1, y: 1 },
    ] {
        assert!(!s.contains_tile(surface(), id), "{id:?}");
    }
    // A flat image heals to itself: nothing changes, nothing is reported.
    let (_dir2, mut flat) = store();
    paint(&mut flat, 100, 100, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let before = dump(&mut flat, 100, 100);
    let mut snap = StrokeSnapshot::new(surface());
    let out = heal(&mut flat, &mask, false, Some(&mut snap), None);
    assert!(out.is_ok_and(|o| o.painted().is_empty()));
    assert!(snap.is_empty());
    assert_eq!(dump(&mut flat, 100, 100), before);
}

#[test]
fn an_unreadable_tile_fails_the_heal_before_any_write() {
    let broken = TileId { x: 0, y: 0 };
    let healthy = TileId { x: 1, y: 0 };
    let (_dir, mut s) = crate::test_support::store_with_a_broken_tile(broken, healthy);
    let mask = disc(100.5, 100.5, 6.0);
    let out = heal(&mut s, &mask, false, None, None);
    let Ok(out) = out else {
        unreachable!("not a refusal");
    };
    assert!(!out.is_complete());
    assert!(out.painted().is_empty());
}

/// Prints the cost of a heal of a square mask (dev profile numbers for the
/// round's report). `cargo test -p aurora-brush heal_cost -- --ignored
/// --nocapture`; asserts nothing about time.
#[test]
#[ignore = "a measurement, not a test"]
fn heal_cost_report() {
    for side in [50_u32, 200, 500, 1000] {
        let (_dir, mut s) = store();
        let size = side * 3;
        paint(&mut s, size, size, |x, y| {
            texel(
                0.3 + 0.2 * ((x % 7) as f32 / 7.0),
                0.4 + 0.2 * ((y % 5) as f32 / 5.0),
                0.5,
                1.0,
            )
        });
        let mut mask = HealMask::new();
        let c = side as f32 * 1.5;
        mask.add_dab((c, c), side as f32 / 2.0);
        let start = std::time::Instant::now();
        let out = spot_heal_covered(&mut s, surface(), &mask, 24.0, false, None, None);
        eprintln!(
            "mask {side} x {side} (disc, {} texels): {:?} ok={}",
            mask.count(),
            start.elapsed(),
            out.is_ok()
        );
    }
}
