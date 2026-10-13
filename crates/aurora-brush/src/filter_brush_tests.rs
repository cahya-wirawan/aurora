//! The Blur and Sharpen dab (0.223.0) at the brush level: hand-computed
//! values of the kernel, strength 0 and 1, uniform areas, the sharpen clamp,
//! whole strokes against an independent model, selection coverage, the
//! transparency lock, tile edges (present and absent neighbours), no phantom
//! tiles, undo, the cap and degenerate input.

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
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, DabOutcome, FilterKind, MAX_FILTER_PATCH_TEXELS, PixelHistory, SHARPEN_GAIN,
    StrokeSnapshot, dab_falloff, filter_dab_covered,
};

const BLUR: FilterKind = FilterKind::Blur;
const SHARPEN: FilterKind = FilterKind::Sharpen;

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

fn px(r: f32, g: f32, b: f32, a: f32) -> [f16; CHANNELS] {
    [
        f16::from_f32(r),
        f16::from_f32(g),
        f16::from_f32(b),
        f16::from_f32(a),
    ]
}

fn bits(t: [f16; CHANNELS]) -> [u16; 4] {
    [
        t[0].to_bits(),
        t[1].to_bits(),
        t[2].to_bits(),
        t[3].to_bits(),
    ]
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

/// A distinct, valid premultiplied texel per position, every value a short
/// dyadic fraction (so the kernel's sums are exact in `f32`): alpha in
/// `1/8..=1`, straight colour in `0..15/16`, premultiplied by the alpha.
fn pattern_colour(x: u32, y: u32) -> [f16; CHANNELS] {
    let a = ((x * 3 + y * 5) % 8 + 1) as f32 / 8.0;
    let s = |n: u32| (n % 16) as f32 / 16.0;
    px(
        s(x * 7 + y) * a,
        s(x + y * 5 + 3) * a,
        s(x * 3 + y * 11 + 9) * a,
        a,
    )
}

fn paint_pattern(s: &mut TileStore, x0: u32, y0: u32, w: u32, h: u32) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            put(s, x, y, pattern_colour(x, y));
        }
    }
}

fn fill_rect(s: &mut TileStore, x0: u32, y0: u32, x1: u32, y1: u32, v: [f16; CHANNELS]) {
    for y in y0..y1 {
        for x in x0..x1 {
            put(s, x, y, v);
        }
    }
}

fn dump(s: &mut TileStore, w: u32, h: u32) -> Vec<[u16; 4]> {
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            out.push(bits(texel(s, x, y)));
        }
    }
    out
}

fn grid_of(s: &mut TileStore, w: u32, h: u32) -> Vec<[f32; 4]> {
    let mut grid = Vec::new();
    for y in 0..h {
        for x in 0..w {
            grid.push(texel(s, x, y).map(f16::to_f32));
        }
    }
    grid
}

fn dab(
    s: &mut TileStore,
    kind: FilterKind,
    center: (f32, f32),
    radius: f32,
    strength: f32,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    filter_dab_covered(
        s,
        surface(),
        center,
        radius,
        kind,
        strength,
        false,
        snapshot,
        coverage,
    )
}

fn locked_dab(
    s: &mut TileStore,
    kind: FilterKind,
    center: (f32, f32),
    radius: f32,
    strength: f32,
) -> DabOutcome {
    filter_dab_covered(
        s,
        surface(),
        center,
        radius,
        kind,
        strength,
        true,
        None,
        None,
    )
}

/// A rectangle selection (layer space).
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

struct Constant(f32);

impl Coverage for Constant {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, _: i64, _: i64) -> f32 {
        self.0
    }
}

// -- The independent model ----------------------------------------------------

/// A dab on a plain `f32` grid, written from the module docs: the layer is
/// copied first, the average is a horizontal pass then a vertical pass of
/// `[1 2 1] / 4` summed in `f64`, and the formulas are the documented ones.
fn model_dab(
    grid: &mut [[f32; 4]],
    width: usize,
    center: (f32, f32),
    radius: f32,
    kind: FilterKind,
    strength: f32,
    lock: bool,
    selection: Option<&dyn Fn(i64, i64) -> f32>,
) {
    let height = grid.len() / width;
    let under: Vec<[f32; 4]> = grid.to_vec();
    let at = |x: i64, y: i64| -> [f64; 4] {
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            return [0.0; 4];
        }
        under[y as usize * width + x as usize].map(f64::from)
    };
    let horizontal = |x: i64, y: i64| -> [f64; 4] {
        let (l, c, r) = (at(x - 1, y), at(x, y), at(x + 1, y));
        [0, 1, 2, 3].map(|ch| (l[ch] + 2.0 * c[ch] + r[ch]) / 4.0)
    };
    let average = |x: i64, y: i64| -> [f32; 4] {
        let (u, c, d) = (horizontal(x, y - 1), horizontal(x, y), horizontal(x, y + 1));
        [0, 1, 2, 3].map(|ch| ((u[ch] + 2.0 * c[ch] + d[ch]) / 4.0) as f32)
    };
    let strength = strength.clamp(0.0, 1.0);
    let toward: f32 = match kind {
        FilterKind::Blur => 1.0,
        FilterKind::Sharpen => -2.0,
    };
    for y in 0..height as i64 {
        for x in 0..width as i64 {
            let d = (x as f32 + 0.5 - center.0).hypot(y as f32 + 0.5 - center.1);
            if d > radius {
                continue;
            }
            let a = dab_falloff(d, radius);
            if a <= 0.0 {
                continue;
            }
            let cover = selection.map_or(1.0, |f| f(x, y));
            if cover <= 0.0 {
                continue;
            }
            let k = strength * a;
            let old = under[y as usize * width + x as usize];
            let avg = average(x, y);
            let mut fresh = [0.0_f32; 4];
            if lock {
                if old[3] <= 0.0 || avg[3] <= 0.0 {
                    continue;
                }
                for ch in 0..3 {
                    let own = old[ch] / old[3];
                    let mean = avg[ch] / avg[3];
                    let mut moved = own + (mean - own) * toward * k;
                    if kind == FilterKind::Sharpen {
                        moved = moved.clamp(0.0, 1.0);
                    }
                    fresh[ch] = f16::from_f32(moved * old[3]).to_f32();
                }
                fresh[3] = old[3];
            } else {
                for ch in 0..4 {
                    fresh[ch] = old[ch] + (avg[ch] - old[ch]) * toward * k;
                }
                if kind == FilterKind::Sharpen {
                    fresh[3] = fresh[3].clamp(0.0, 1.0);
                    for ch in 0..3 {
                        fresh[ch] = fresh[ch].clamp(0.0, fresh[3]);
                    }
                }
                fresh = fresh.map(|v| f16::from_f32(v).to_f32());
            }
            if cover < 1.0 {
                for ch in 0..4 {
                    if fresh[ch].to_bits() != old[ch].to_bits() {
                        fresh[ch] = f16::from_f32(old[ch] + (fresh[ch] - old[ch]) * cover).to_f32();
                    }
                }
            }
            grid[y as usize * width + x as usize] = fresh;
        }
    }
}

fn assert_grid_matches(s: &mut TileStore, grid: &[[f32; 4]], w: u32, h: u32, what: &str) {
    for y in 0..h {
        for x in 0..w {
            let want = grid[(y * w + x) as usize].map(|v| f16::from_f32(v).to_bits());
            assert_eq!(bits(texel(s, x, y)), want, "{what}: texel ({x}, {y})");
        }
    }
}

/// Runs `dabs` as one stroke on a store holding pattern blocks and compares
/// every texel of `dims` with the model, for one kind, strength and lock.
fn assert_stroke_matches_model(
    blocks: &[(u32, u32, u32, u32)],
    dims: (u32, u32),
    dabs: &[(f32, f32)],
    radius: f32,
    kind: FilterKind,
    strength: f32,
    lock: bool,
) {
    let (_dir, mut s) = store();
    for block in blocks {
        paint_pattern(&mut s, block.0, block.1, block.2, block.3);
    }
    let (w, h) = dims;
    let mut grid = grid_of(&mut s, w, h);
    for &c in dabs {
        let _ = filter_dab_covered(
            &mut s,
            surface(),
            c,
            radius,
            kind,
            strength,
            lock,
            None,
            None,
        );
        model_dab(&mut grid, w as usize, c, radius, kind, strength, lock, None);
    }
    assert_grid_matches(
        &mut s,
        &grid,
        w,
        h,
        &format!("{kind:?} strength {strength} lock {lock}"),
    );
}

// -- Hand-computed values -------------------------------------------------------

#[test]
fn blur_of_one_bright_texel_gives_the_kernel_weights_times_the_falloff() {
    let (_dir, mut s) = store();
    put(&mut s, 30, 30, px(1.0, 1.0, 1.0, 1.0));
    let out = dab(&mut s, BLUR, (30.5, 30.5), 6.0, 1.0, None, None);
    assert!(out.is_complete());
    // The centre texel is at distance 0: k = 1, so it becomes its average,
    // 4/16 of the bright texel.
    assert_eq!(
        bits(texel(&mut s, 30, 30)),
        bits(px(0.25, 0.25, 0.25, 0.25))
    );
    // An edge neighbour (weight 2/16) and a corner (weight 1/16), each
    // moved from empty by the Brush's falloff at its distance.
    let edge = 0.125 * dab_falloff(1.0, 6.0);
    let corner = 0.0625 * dab_falloff(2.0_f32.sqrt(), 6.0);
    for (x, y) in [(29, 30), (31, 30), (30, 29), (30, 31)] {
        assert_eq!(bits(texel(&mut s, x, y)), bits(px(edge, edge, edge, edge)));
    }
    for (x, y) in [(29, 29), (31, 29), (29, 31), (31, 31)] {
        assert_eq!(
            bits(texel(&mut s, x, y)),
            bits(px(corner, corner, corner, corner))
        );
    }
    // Two texels away the kernel does not reach: nothing is written.
    for (x, y) in [(32, 30), (30, 32), (28, 28), (32, 32)] {
        assert_eq!(bits(texel(&mut s, x, y)), [0; 4], "({x}, {y})");
    }
}

#[test]
fn blur_strength_scales_the_move_toward_the_average() {
    let (_dir, mut s) = store();
    put(&mut s, 30, 30, px(1.0, 1.0, 1.0, 1.0));
    let _ = dab(&mut s, BLUR, (30.5, 30.5), 6.0, 0.5, None, None);
    // 1.0 + (0.25 - 1.0) * 0.5
    assert_eq!(
        bits(texel(&mut s, 30, 30)),
        bits(px(0.625, 0.625, 0.625, 0.625))
    );
    // A neighbour: 0 + (0.125 - 0) * (0.5 * falloff)
    let edge = 0.125 * (0.5 * dab_falloff(1.0, 6.0));
    assert_eq!(
        bits(texel(&mut s, 31, 30)),
        bits(px(edge, edge, edge, edge))
    );
}

#[test]
fn strength_zero_changes_nothing_for_both_kinds() {
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 20, 20, 30, 30);
        let before = dump(&mut s, 64, 64);
        let mut snapshot = StrokeSnapshot::new(surface());
        let out = dab(
            &mut s,
            kind,
            (30.5, 30.5),
            8.0,
            0.0,
            Some(&mut snapshot),
            None,
        );
        assert!(out.painted().is_empty(), "{kind:?}");
        assert!(out.is_complete());
        assert_eq!(snapshot.captured().count(), 0);
        assert_eq!(dump(&mut s, 64, 64), before, "{kind:?}");
    }
}

#[test]
fn strength_one_at_full_falloff_is_the_whole_move() {
    // Sharpen: the centre texel of a lone bright texel goes to
    // old + (old - avg) * G, clamped to the valid range.
    let (_dir, mut s) = store();
    put(&mut s, 30, 30, px(0.5, 0.25, 0.125, 0.5));
    let _ = dab(&mut s, SHARPEN, (30.5, 30.5), 6.0, 1.0, None, None);
    // old 0.5 -> 0.5 + (0.5 - 0.125) * 2 = 1.25, alpha 0.5 -> 1.0 (clamped
    // to 1); colour channels are clamped to the new alpha 1: r = 1.0
    // (0.5 + 0.75 = 1.25 -> 1), g = 0.25 + (0.25 - 0.0625) * 2 = 0.625,
    // b = 0.125 + (0.125 - 0.03125) * 2 = 0.3125.
    let want = px(1.0, 0.625, 0.3125, 1.0);
    assert_eq!(bits(texel(&mut s, 30, 30)), bits(want));
    assert_eq!(SHARPEN_GAIN, 2.0);
}

// -- Uniform areas, sharpen clamps ---------------------------------------------------

#[test]
fn a_uniform_area_is_unchanged_by_both_kinds() {
    for kind in [BLUR, SHARPEN] {
        for strength in [0.3_f32, 1.0] {
            let (_dir, mut s) = store();
            fill_rect(&mut s, 10, 10, 50, 50, px(0.5, 0.25, 0.125, 0.75));
            // 0.75 alpha with colour <= alpha: valid premultiplied.
            let before = dump(&mut s, 64, 64);
            let mut snapshot = StrokeSnapshot::new(surface());
            let out = dab(
                &mut s,
                kind,
                (30.5, 30.5),
                9.0,
                strength,
                Some(&mut snapshot),
                None,
            );
            assert!(out.painted().is_empty(), "{kind:?} {strength}");
            assert_eq!(snapshot.captured().count(), 0);
            assert_eq!(dump(&mut s, 64, 64), before, "{kind:?} {strength}");
        }
    }
}

#[test]
fn sharpening_a_step_edge_overshoots_then_clamps_to_the_valid_range() {
    let (_dir, mut s) = store();
    fill_rect(&mut s, 10, 10, 32, 50, px(0.125, 0.125, 0.125, 1.0));
    fill_rect(&mut s, 32, 10, 54, 50, px(0.875, 0.875, 0.875, 1.0));
    let _ = dab(&mut s, SHARPEN, (31.5, 30.5), 6.0, 1.0, None, None);
    // x = 31 (dark side of the edge, falloff 1): 0.125 + (0.125 - 0.3125) *
    // 2 = -0.25, clamped up to 0.
    assert_eq!(bits(texel(&mut s, 31, 30)), bits(px(0.0, 0.0, 0.0, 1.0)));
    // x = 32 (light side, falloff < 1): 0.875 + (0.875 - 0.6875) * 2 * k,
    // k = falloff(1) ~ 0.76, is above 1 and clamps to alpha 1.
    assert_eq!(bits(texel(&mut s, 32, 30)), bits(px(1.0, 1.0, 1.0, 1.0)));
    // Two texels from the edge the 3 x 3 window is flat: unchanged.
    assert_eq!(
        bits(texel(&mut s, 29, 30)),
        bits(px(0.125, 0.125, 0.125, 1.0))
    );
    assert_eq!(
        bits(texel(&mut s, 34, 30)),
        bits(px(0.875, 0.875, 0.875, 1.0))
    );
}

#[test]
fn blurring_the_same_step_edge_never_leaves_the_range_of_its_neighbours() {
    let (_dir, mut s) = store();
    fill_rect(&mut s, 10, 10, 32, 50, px(0.125, 0.125, 0.125, 1.0));
    fill_rect(&mut s, 32, 10, 54, 50, px(0.875, 0.875, 0.875, 1.0));
    let _ = dab(&mut s, BLUR, (31.5, 30.5), 6.0, 1.0, None, None);
    // x = 31: 0.125 + (0.3125 - 0.125) * 1.
    assert_eq!(
        bits(texel(&mut s, 31, 30)),
        bits(px(0.3125, 0.3125, 0.3125, 1.0))
    );
    for x in 25..39 {
        let v = texel(&mut s, x, 30)[0].to_f32();
        assert!((0.125..=0.875).contains(&v), "x {x}: {v}");
    }
}

#[test]
fn a_sharpened_translucent_pattern_is_always_a_valid_premultiplied_texel() {
    for strength in [0.5_f32, 1.0] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 60, 40);
        for c in [(20.5_f32, 20.5_f32), (30.3, 24.1), (40.9, 30.7)] {
            let _ = dab(&mut s, SHARPEN, c, 9.0, strength, None, None);
        }
        for y in 0..56 {
            for x in 0..80 {
                let t = texel(&mut s, x, y).map(f16::to_f32);
                assert!(t.iter().all(|v| v.is_finite()), "({x}, {y})");
                assert!((0.0..=1.0).contains(&t[3]), "alpha {t:?} at ({x}, {y})");
                for c in &t[..3] {
                    assert!(
                        (0.0..=t[3]).contains(c),
                        "colour above alpha {t:?} ({x}, {y})"
                    );
                }
            }
        }
    }
}

// -- Against the independent model ---------------------------------------------------

#[test]
fn whole_strokes_match_an_independent_model_exactly() {
    let dabs = [
        (20.3_f32, 24.7_f32),
        (23.9, 26.1),
        (27.4, 27.8),
        (31.2, 29.9),
        (35.6, 31.0),
        (39.5, 28.2),
        (42.2, 24.4),
    ];
    for kind in [BLUR, SHARPEN] {
        for strength in [0.25_f32, 0.5, 1.0] {
            assert_stroke_matches_model(
                &[(8, 8, 70, 40)],
                (96, 64),
                &dabs,
                9.0,
                kind,
                strength,
                false,
            );
        }
    }
}

#[test]
fn a_dab_reads_the_layer_as_it_was_before_the_dab() {
    // A single dab at strength 1 on a pattern: if any texel were averaged
    // from already-written neighbours the model (which copies first) would
    // disagree. The pattern is busy so a read-after-write error shows.
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        let mut grid = grid_of(&mut s, 64, 64);
        let _ = dab(&mut s, kind, (27.5, 27.5), 12.0, 1.0, None, None);
        model_dab(&mut grid, 64, (27.5, 27.5), 12.0, kind, 1.0, false, None);
        assert_grid_matches(&mut s, &grid, 64, 64, &format!("{kind:?}"));
    }
}

#[test]
fn later_dabs_see_earlier_dabs_and_the_order_matters() {
    let run = |kind: FilterKind, order: &[(f32, f32)]| {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 60, 40);
        for &c in order {
            let _ = dab(&mut s, kind, c, 6.0, 0.8, None, None);
        }
        dump(&mut s, 80, 56)
    };
    let forward = [(20.5_f32, 24.5_f32), (24.5, 24.5), (28.5, 24.5)];
    let backward = [(28.5_f32, 24.5_f32), (24.5, 24.5), (20.5, 24.5)];
    for kind in [BLUR, SHARPEN] {
        assert_eq!(run(kind, &forward), run(kind, &forward), "deterministic");
        assert_ne!(run(kind, &forward), run(kind, &backward), "{kind:?}");
    }
}

#[test]
fn blur_and_sharpen_are_different_operations() {
    let run = |kind: FilterKind| {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        let _ = dab(&mut s, kind, (27.5, 27.5), 9.0, 0.7, None, None);
        dump(&mut s, 64, 64)
    };
    assert_ne!(run(BLUR), run(SHARPEN));
}

#[test]
fn blur_softens_and_sharpen_roughens_a_busy_pattern() {
    // The sum of squared differences between horizontal neighbours falls
    // under Blur and rises under Sharpen.
    let roughness = |s: &mut TileStore| {
        let mut sum = 0.0_f64;
        for y in 20..36 {
            for x in 20..36 {
                let (a, b) = (texel(s, x, y), texel(s, x + 1, y));
                for c in 0..3 {
                    let d = f64::from(a[c].to_f32() - b[c].to_f32());
                    sum += d * d;
                }
            }
        }
        sum
    };
    let mut results = Vec::new();
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        let before = roughness(&mut s);
        let _ = dab(&mut s, kind, (28.5, 28.5), 14.0, 1.0, None, None);
        results.push((before, roughness(&mut s)));
    }
    assert!(results[0].1 < results[0].0, "blur {results:?}");
    assert!(results[1].1 > results[1].0, "sharpen {results:?}");
}

#[test]
fn the_footprint_is_the_brushs_soft_dab_and_symmetric() {
    // A single bright texel at the centre of an empty layer: the output is
    // symmetric about it under both mirrors, and nothing outside the
    // footprint is written.
    let (_dir, mut s) = store();
    put(&mut s, 100, 24, px(1.0, 1.0, 1.0, 1.0));
    let _ = dab(&mut s, BLUR, (100.5, 24.5), 9.0, 1.0, None, None);
    for dy in 0..=3_i64 {
        for dx in 0..=3_i64 {
            let at = |sx: i64, sy: i64, s: &mut TileStore| {
                bits(texel(s, (100 + sx * dx) as u32, (24 + sy * dy) as u32))
            };
            let base = at(1, 1, &mut s);
            assert_eq!(at(-1, 1, &mut s), base, "x mirror {dx} {dy}");
            assert_eq!(at(1, -1, &mut s), base, "y mirror {dx} {dy}");
            assert_eq!(at(-1, -1, &mut s), base, "both {dx} {dy}");
        }
    }
    // A checkerboard block blurred: only texels strictly inside the
    // radius change.
    let (_dir, mut t) = store();
    for y in 10..50 {
        for x in 10..50 {
            let v = if (x + y) % 2 == 0 { 1.0 } else { 0.0 };
            put(&mut t, x, y, px(v, v, v, 1.0));
        }
    }
    let before = dump(&mut t, 64, 64);
    let _ = dab(&mut t, BLUR, (30.5, 30.5), 6.0, 1.0, None, None);
    let after = dump(&mut t, 64, 64);
    let mut changed = 0;
    for y in 0..64_u32 {
        for x in 0..64_u32 {
            let i = (y * 64 + x) as usize;
            let d = (x as f32 + 0.5 - 30.5).hypot(y as f32 + 0.5 - 30.5);
            if d >= 6.0 {
                assert_eq!(after[i], before[i], "outside the radius at ({x}, {y})");
            } else if after[i] != before[i] {
                changed += 1;
            }
        }
    }
    assert!(changed > 60, "{changed}");
}

// -- Selections ---------------------------------------------------------------------

#[test]
fn a_selection_restricts_the_dab_and_blends_by_coverage() {
    for kind in [BLUR, SHARPEN] {
        // Hard rectangle: nothing outside changes, inside equals the
        // unselected dab.
        let (_d1, mut free) = store();
        paint_pattern(&mut free, 8, 8, 50, 40);
        let (_d2, mut cut) = store();
        paint_pattern(&mut cut, 8, 8, 50, 40);
        let before = dump(&mut cut, 70, 56);
        let _ = dab(&mut free, kind, (30.5, 28.5), 12.0, 1.0, None, None);
        let rect = Rect {
            x: 28,
            y: 0,
            width: 4,
            height: 100,
        };
        let _ = dab(
            &mut cut,
            kind,
            (30.5, 28.5),
            12.0,
            1.0,
            None,
            Some(&RectCov(rect)),
        );
        for y in 0..56_u32 {
            for x in 0..70_u32 {
                let i = (y * 70 + x) as usize;
                let want = if (28..32).contains(&x) {
                    bits(texel(&mut free, x, y))
                } else {
                    before[i]
                };
                assert_eq!(bits(texel(&mut cut, x, y)), want, "{kind:?} ({x}, {y})");
            }
        }
        // Half coverage everywhere: each changed channel lies halfway.
        let (_d3, mut half) = store();
        paint_pattern(&mut half, 8, 8, 50, 40);
        let mut grid = grid_of(&mut half, 70, 56);
        let _ = dab(
            &mut half,
            kind,
            (30.5, 28.5),
            12.0,
            1.0,
            None,
            Some(&Constant(0.5)),
        );
        let half_cover = |_: i64, _: i64| 0.5_f32;
        model_dab(
            &mut grid,
            70,
            (30.5, 28.5),
            12.0,
            kind,
            1.0,
            false,
            Some(&half_cover),
        );
        assert_grid_matches(&mut half, &grid, 70, 56, &format!("{kind:?} half coverage"));
        assert_ne!(dump(&mut half, 70, 56), dump(&mut free, 70, 56));
    }
}

#[test]
fn a_selection_that_excludes_a_tile_neither_reads_into_it_nor_creates_it() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 230, 8, 26, 40);
    // The selection holds only x < 250, so the dab never writes tile 1.
    let rect = Rect {
        x: 0,
        y: 0,
        width: 250,
        height: 100,
    };
    let _ = dab(
        &mut s,
        BLUR,
        (252.5, 24.5),
        9.0,
        1.0,
        None,
        Some(&RectCov(rect)),
    );
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 0 }));
    // A selection covering nothing changes nothing.
    let before = dump(&mut s, 300, 60);
    let nothing = Rect {
        x: 1000,
        y: 1000,
        width: 5,
        height: 5,
    };
    let out = dab(
        &mut s,
        BLUR,
        (240.5, 24.5),
        9.0,
        1.0,
        None,
        Some(&RectCov(nothing)),
    );
    assert!(out.painted().is_empty());
    assert_eq!(dump(&mut s, 300, 60), before);
}

// -- The transparency lock ------------------------------------------------------------

#[test]
fn the_transparency_lock_keeps_alpha_bit_for_bit_and_matches_the_model() {
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 50, 40);
        // Holes of full transparency inside the block.
        fill_rect(&mut s, 24, 20, 30, 26, [f16::ZERO; CHANNELS]);
        let mut alpha_before = Vec::new();
        for y in 0..56 {
            for x in 0..70 {
                alpha_before.push(texel(&mut s, x, y)[3].to_bits());
            }
        }
        let mut grid = grid_of(&mut s, 70, 56);
        for c in [(27.5_f32, 24.5_f32), (31.5, 27.5), (36.0, 30.0)] {
            let _ = locked_dab(&mut s, kind, c, 10.0, 0.9);
            model_dab(&mut grid, 70, c, 10.0, kind, 0.9, true, None);
        }
        let mut i = 0;
        for y in 0..56 {
            for x in 0..70 {
                assert_eq!(
                    texel(&mut s, x, y)[3].to_bits(),
                    alpha_before[i],
                    "{kind:?} alpha at ({x}, {y})"
                );
                i += 1;
            }
        }
        assert_grid_matches(&mut s, &grid, 70, 56, &format!("{kind:?} locked"));
    }
}

#[test]
fn a_locked_blur_mixes_colour_by_alpha_weight_so_transparency_does_not_darken_it() {
    let (_dir, mut s) = store();
    // One half-transparent red texel (straight (1, 0, 0), alpha 0.5) alone.
    put(&mut s, 30, 30, px(0.5, 0.0, 0.0, 0.5));
    let before = dump(&mut s, 64, 64);
    let out = locked_dab(&mut s, BLUR, (30.5, 30.5), 6.0, 1.0);
    // The alpha-weighted straight average of the texel and its transparent
    // neighbours is its own colour: nothing changes, and no tile or
    // neighbour is created.
    assert!(out.painted().is_empty());
    assert_eq!(dump(&mut s, 64, 64), before);
    // Next to a blue opaque texel the red takes some blue, weighted by
    // alpha, and keeps its alpha.
    put(&mut s, 31, 30, px(0.0, 0.0, 1.0, 1.0));
    let _ = locked_dab(&mut s, BLUR, (30.5, 30.5), 6.0, 1.0);
    let t = texel(&mut s, 30, 30).map(f16::to_f32);
    assert_eq!(t[3], 0.5, "alpha kept");
    // avg: r = 0.5*4/16 = 0.125, b = 1.0*2/16 = 0.125, a = (2 + 2)/16 =
    // 0.25 -> straight (0.5, 0, 0.5); own (1, 0, 0); k = 1 -> straight
    // (0.5, 0, 0.5), premultiplied by 0.5.
    assert_eq!(t, [0.25, 0.0, 0.25, 0.5]);
    // The fully transparent neighbours keep their (absent) alpha.
    assert_eq!(bits(texel(&mut s, 29, 30)), [0; 4]);
}

#[test]
fn the_transparency_lock_creates_no_tile() {
    let (_dir, mut s) = store();
    put(&mut s, 250, 30, px(1.0, 1.0, 1.0, 1.0));
    let _ = locked_dab(&mut s, BLUR, (253.5, 30.5), 9.0, 1.0);
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 0 }));
}

// -- Tile edges, the layer origin, phantom tiles -----------------------------------

#[test]
fn a_stroke_straddling_a_tile_edge_matches_the_model_with_both_tiles_present() {
    assert_eq!(TILE, 256, "the test is written for 256-texel tiles");
    let dabs = [
        (244.3_f32, 24.7_f32),
        (249.9, 26.1),
        (254.6, 27.8),
        (258.2, 29.9),
        (262.6, 31.0),
        (267.5, 28.2),
        (272.2, 24.4),
    ];
    for kind in [BLUR, SHARPEN] {
        for strength in [0.5_f32, 1.0] {
            assert_stroke_matches_model(
                &[(230, 8, 60, 40)],
                (300, 56),
                &dabs,
                9.0,
                kind,
                strength,
                false,
            );
        }
        // Across the horizontal edge (y = 256) with a vertical stroke.
        let tall = [
            (30.4_f32, 244.3_f32),
            (31.9, 249.9),
            (33.6, 254.6),
            (34.2, 258.2),
            (35.6, 262.6),
            (36.5, 267.5),
        ];
        assert_stroke_matches_model(
            &[(10, 230, 40, 60)],
            (60, 300),
            &tall,
            9.0,
            kind,
            0.5,
            false,
        );
    }
}

#[test]
fn a_blur_dab_bleeds_into_an_absent_neighbour_tile_and_a_sharpen_does_not() {
    assert_eq!(TILE, 256);
    // The block ends exactly at the tile edge (x 230..255), tile (1, 0) is
    // absent: its first columns border content.
    let dabs = [(250.5_f32, 24.5_f32), (254.5, 27.5), (257.5, 26.5)];
    let tile1 = TileId { x: 1, y: 0 };
    for (kind, tile_created) in [(BLUR, true), (SHARPEN, false)] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 230, 8, 26, 40);
        assert!(!s.contains_tile(surface(), tile1));
        let mut grid = grid_of(&mut s, 300, 56);
        for &c in &dabs {
            let _ = dab(&mut s, kind, c, 9.0, 1.0, None, None);
            model_dab(&mut grid, 300, c, 9.0, kind, 1.0, false, None);
        }
        assert_eq!(s.contains_tile(surface(), tile1), tile_created, "{kind:?}");
        assert_grid_matches(&mut s, &grid, 300, 56, &format!("{kind:?}"));
    }
    // Content in the second tile only, the first absent: the same.
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 256, 8, 30, 40);
        let mut grid = grid_of(&mut s, 300, 56);
        for &c in &[(258.5_f32, 24.5_f32), (254.5, 27.5), (251.5, 26.5)] {
            let _ = dab(&mut s, kind, c, 9.0, 1.0, None, None);
            model_dab(&mut grid, 300, c, 9.0, kind, 1.0, false, None);
        }
        assert_grid_matches(&mut s, &grid, 300, 56, &format!("{kind:?} reversed"));
    }
}

#[test]
fn a_stroke_near_the_layer_origin_matches_the_model_including_negative_patch_texels() {
    let dabs = [
        (3.2_f32, 2.7_f32),
        (5.9, 4.1),
        (7.4, 6.8),
        (10.6, 8.0),
        (14.5, 9.2),
        (2.5, 12.4),
        (0.5, 0.5),
    ];
    for kind in [BLUR, SHARPEN] {
        for strength in [0.25_f32, 0.5, 1.0] {
            assert_stroke_matches_model(
                &[(0, 0, 40, 30)],
                (48, 40),
                &dabs,
                9.0,
                kind,
                strength,
                false,
            );
        }
        assert_stroke_matches_model(&[(0, 0, 40, 30)], (48, 40), &dabs, 9.0, kind, 0.7, true);
    }
}

#[test]
fn blurring_or_sharpening_empty_space_creates_no_tile_and_captures_nothing() {
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        let mut snapshot = StrokeSnapshot::new(surface());
        let out = dab(
            &mut s,
            kind,
            (100.5, 100.5),
            20.0,
            1.0,
            Some(&mut snapshot),
            None,
        );
        assert!(out.painted().is_empty());
        assert!(out.is_complete());
        assert_eq!(snapshot.captured().count(), 0);
        assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
    }
    // A dab near content but whose footprint touches an absent tile far
    // from any content creates that tile only where the result is not blank.
    let (_dir, mut s) = store();
    put(&mut s, 10, 10, px(1.0, 1.0, 1.0, 1.0));
    let _ = dab(&mut s, BLUR, (250.5, 10.5), 12.0, 1.0, None, None);
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 0 }));
}

#[test]
fn undo_restores_exactly_the_captured_tiles() {
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 230, 10, 60, 30);
        let before = dump(&mut s, 300, 60);
        let mut snapshot = StrokeSnapshot::new(surface());
        let mut painted = BTreeSet::new();
        for x in [240.5_f32, 246.5, 252.5, 258.5, 264.5, 270.5] {
            let out = dab(&mut s, kind, (x, 24.5), 8.0, 0.7, Some(&mut snapshot), None);
            painted.extend(out.painted().iter().copied());
        }
        assert!(painted.len() >= 2, "{painted:?}");
        let captured: BTreeSet<TileId> = snapshot.captured().collect();
        assert_eq!(captured, painted, "captured and painted are one set");
        assert_ne!(dump(&mut s, 300, 60), before);
        let mut history = PixelHistory::new();
        assert!(history.push(snapshot));
        assert!(matches!(history.undo(&mut s), Ok(true)));
        assert_eq!(dump(&mut s, 300, 60), before, "{kind:?}");
    }
}

// -- Bounds and degenerate input -----------------------------------------------------------

#[test]
fn the_patch_cap_refuses_a_big_dab_whole_and_accepts_one_just_under() {
    assert_eq!(MAX_FILTER_PATCH_TEXELS, 1 << 20);
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 600, 600, 30, 30);
    let before = dump(&mut s, 700, 700);
    // half = ceil(r) + 2: r = 510 -> 1025^2 texels, over the cap.
    let out = dab(&mut s, BLUR, (610.5, 610.5), 510.0, 1.0, None, None);
    assert!(out.painted().is_empty());
    assert!(out.is_complete());
    assert_eq!(dump(&mut s, 700, 700), before);
    // r = 509 -> 1023^2 texels, just under.
    let ok = dab(&mut s, BLUR, (610.5, 610.5), 509.0, 1.0, None, None);
    assert!(ok.is_complete());
    assert!(!ok.painted().is_empty());
    assert_ne!(dump(&mut s, 700, 700), before);
}

#[test]
fn degenerate_input_changes_nothing_and_never_makes_nan() {
    for kind in [BLUR, SHARPEN] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 0, 0, 20, 20);
        let before = dump(&mut s, 32, 32);
        for (c, r, k) in [
            ((f32::NAN, 5.0), 3.0, 1.0),
            ((5.0, f32::INFINITY), 3.0, 1.0),
            ((5.0, 5.0), 0.0, 1.0),
            ((5.0, 5.0), f32::NAN, 1.0),
            ((5.0, 5.0), -1.0, 1.0),
            ((5.0, 5.0), 3.0, f32::NAN),
            ((1e30, 5.0), 3.0, 1.0),
            ((5.0, -1e30), 3.0, 1.0),
            ((5.0, 5.0), 3.0, -2.0),
        ] {
            let out = dab(&mut s, kind, c, r, k, None, None);
            assert!(out.painted().is_empty(), "{kind:?} {c:?} {r} {k}");
            assert!(out.is_complete());
        }
        assert_eq!(dump(&mut s, 32, 32), before, "{kind:?}");

        // An out-of-range strength is clamped to 1; an infinite one too.
        let run = |k: f32| {
            let (_d, mut t) = store();
            paint_pattern(&mut t, 0, 0, 40, 20);
            let _ = dab(&mut t, kind, (10.5, 10.5), 4.0, k, None, None);
            dump(&mut t, 40, 20)
        };
        assert_eq!(run(7.0), run(1.0));
        assert_eq!(run(f32::INFINITY), run(1.0));

        // A stored NaN or infinity is read as nothing, never spread.
        let (_d, mut t) = store();
        paint_pattern(&mut t, 0, 0, 40, 20);
        put(
            &mut t,
            10,
            10,
            [f16::NAN, f16::INFINITY, f16::NEG_INFINITY, f16::NAN],
        );
        put(&mut t, 12, 10, [f16::INFINITY; CHANNELS]);
        let _ = dab(&mut t, kind, (10.5, 10.5), 6.0, 1.0, None, None);
        for y in 5..16_u32 {
            for x in 5..16_u32 {
                let d = (x as f32 + 0.5 - 10.5).hypot(y as f32 + 0.5 - 10.5);
                if d < 6.0 {
                    for v in texel(&mut t, x, y) {
                        assert!(v.is_finite(), "{kind:?} ({x}, {y})");
                    }
                }
            }
        }
    }
}

#[test]
fn an_unreadable_tile_refuses_the_dab_before_anything_is_written() {
    let broken = TileId { x: 0, y: 0 };
    let healthy = TileId { x: 1, y: 0 };
    let (_dir, mut s) = crate::test_support::store_with_a_broken_tile(broken, healthy);
    let surf = crate::test_support::surface();
    for kind in [BLUR, SHARPEN] {
        // The patch around (260, 10) reaches the broken tile 0.
        let out = filter_dab_covered(
            &mut s,
            surf,
            (260.5, 10.5),
            6.0,
            kind,
            1.0,
            false,
            None,
            None,
        );
        assert!(out.painted().is_empty());
        assert_eq!(out.failed().len(), 1, "{kind:?}");
    }
}

#[test]
fn a_snapshot_for_another_surface_is_ignored_not_captured() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 8, 8, 40, 40);
    let mut other = StrokeSnapshot::new(SurfaceId::from_raw(9));
    let out = dab(&mut s, BLUR, (27.5, 27.5), 8.0, 1.0, Some(&mut other), None);
    assert!(!out.painted().is_empty());
    assert_eq!(other.captured().count(), 0);
}
