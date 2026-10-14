//! Stroke compositing for the soft Brush (0.230.0): see `stroke_cover`.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::too_many_lines
)]

use std::num::NonZeroUsize;

use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};

use crate::{
    Coverage, DEFAULT_SPACING, PaintAmount, PixelHistory, StrokeSnapshot, advance_segment,
    dab_step, dabs_along_path, stamp_dab, stamp_dab_amount,
};

type Tx = [f32; CHANNELS];

fn texel(store: &mut TileStore, x: u32, y: u32) -> Tx {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    if !store.contains_tile(surface(), id) {
        return [0.0; CHANNELS];
    }
    let Ok(tile) = store.get(surface(), id) else {
        unreachable!("tile");
    };
    let index = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    let mut out = [0.0; CHANNELS];
    for (c, slot) in out.iter_mut().enumerate() {
        *slot = tile
            .texels()
            .get(index + c)
            .map_or(f32::NAN, |v| v.to_f32());
    }
    out
}

/// One stroke: dabs along `path` at `spacing`, one snapshot, `colour`.
fn stroke(
    store: &mut TileStore,
    path: &[(f32, f32)],
    radius: f32,
    colour: [f32; 3],
    amount: PaintAmount,
    spacing: f32,
    lock: bool,
) -> StrokeSnapshot {
    let mut snap = StrokeSnapshot::new(surface());
    for d in dabs_along_path(path, radius, spacing) {
        let o = stamp_dab_amount(
            store,
            surface(),
            d,
            radius,
            colour,
            Some(&mut snap),
            None,
            lock,
            false,
            amount,
        );
        assert!(o.is_complete());
    }
    snap
}

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

// ---------------------------------------------------------------------------
// The reported bug: a white stroke over a red one.

/// The screenshot's scene: a red horizontal stroke, then a white vertical
/// one crossing it, radius 100. The max-alpha rule kept whichever colour had
/// the higher alpha per texel (a hard bow-tie seam where the two alphas
/// cross); stroke compositing blends the white's soft edge over the red.
fn red_then_white() -> (tempfile::TempDir, TileStore) {
    let (dir, mut s) = store();
    let r = 100.0;
    // Dab centres land on texel centres: x = 200.5 + 25 k, y = 150.5 + 25 k.
    stroke(
        &mut s,
        &[(200.5, 300.5), (500.5, 300.5)],
        r,
        RED,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        false,
    );
    stroke(
        &mut s,
        &[(350.5, 150.5), (350.5, 450.5)],
        r,
        WHITE,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        false,
    );
    (dir, s)
}

#[test]
fn a_white_stroke_blends_its_soft_edge_over_a_red_one() {
    let (_dir, mut s) = red_then_white();
    // The white centre line over the red is pure white: at a dab centre the
    // falloff is 1, so the texel is exactly white; between dabs it is within
    // half a percent.
    assert_eq!(texel(&mut s, 350, 300), [1.0, 1.0, 1.0, 1.0]);
    for y in 250..350 {
        for c in 0..4 {
            assert!(texel(&mut s, 350, y)[c] >= 0.99, "centre line ({y}, {c})");
        }
    }
    // Where the white's alpha is about one half (about 0.67 r from its centre
    // line) over solid red, the texel is a blend of the two: red stays full,
    // green and blue are about half. The max-alpha rule left pure red there.
    let t = texel(&mut s, 350 + 67, 300);
    assert!(t[0] > 0.95, "{t:?}");
    for c in [1, 2] {
        assert!(t[c] > 0.25 && t[c] < 0.75, "channel {c} is a blend: {t:?}");
    }
    assert!(t[3] > 0.99);
    // And away from both centre lines, on the diagonal, still a blend.
    for off in [30_i32, 45, 55] {
        let t = texel(&mut s, (350 + off) as u32, (300 + off) as u32);
        assert!(t[1] > 0.1 && t[1] < 0.99, "diagonal {off}: {t:?}");
        assert!((t[1] - t[2]).abs() < 1e-3, "grey in g/b: {t:?}");
    }
}

#[test]
fn there_is_no_hard_seam_where_the_two_strokes_alphas_cross() {
    let (_dir, mut s) = red_then_white();
    // Over the whole overlap square no two adjacent texels (horizontal or
    // vertical) differ by more than a few percent in any channel: a smooth
    // gradient, not the red/white jump the max-alpha rule drew along the
    // diagonals (a difference of up to 1.0 between neighbours).
    let mut worst = 0.0_f32;
    for y in 230..380_u32 {
        for x in 280..420_u32 {
            let here = texel(&mut s, x, y);
            for next in [texel(&mut s, x + 1, y), texel(&mut s, x, y + 1)] {
                for c in 0..4 {
                    worst = worst.max((here[c] - next[c]).abs());
                }
            }
        }
    }
    println!("largest neighbour difference in the overlap: {worst:.4}");
    assert!(worst < 0.05, "a seam: {worst}");
}

// ---------------------------------------------------------------------------
// Ceilings.

/// A stroke that loops back and crosses itself several times.
fn looping_path() -> Vec<(f32, f32)> {
    vec![
        (100.5, 100.5),
        (300.5, 100.5),
        (300.5, 300.5),
        (100.5, 300.5),
        (100.5, 100.5),
        (300.5, 300.5),
        (300.5, 100.5),
        (100.5, 300.5),
    ]
}

fn max_over(store: &mut TileStore, f: impl Fn(Tx) -> f32) -> f32 {
    let mut worst = 0.0_f32;
    for y in 60..340 {
        for x in 60..340 {
            worst = worst.max(f(texel(store, x, y)));
        }
    }
    worst
}

#[test]
fn a_stroke_crossing_itself_at_full_opacity_never_overshoots_its_colour() {
    let (_dir, mut s) = store();
    let colour = [0.8, 0.4, 0.2];
    stroke(
        &mut s,
        &looping_path(),
        24.0,
        colour,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        false,
    );
    for (c, &limit) in colour.iter().enumerate() {
        let worst = max_over(&mut s, |t| t[c]);
        assert!(worst <= limit + 1e-3, "channel {c}: {worst} > {limit}");
        assert!(
            worst > limit - 0.01,
            "and it does reach the colour: {worst}"
        );
    }
    assert!(max_over(&mut s, |t| t[3]) <= 1.0);
}

#[test]
fn a_stroke_crossing_itself_at_half_opacity_never_exceeds_half_coverage() {
    let (_dir, mut s) = store();
    let colour = [0.8, 0.4, 0.2];
    stroke(
        &mut s,
        &looping_path(),
        24.0,
        colour,
        PaintAmount::new(0.5, 1.0),
        DEFAULT_SPACING,
        false,
    );
    let alpha = max_over(&mut s, |t| t[3]);
    assert!(alpha <= 0.5 + 1e-3, "alpha {alpha}");
    assert!(alpha > 0.49, "it reaches the ceiling: {alpha}");
    for (c, &v) in colour.iter().enumerate() {
        assert!(max_over(&mut s, |t| t[c]) <= v * 0.5 + 1e-3, "channel {c}");
    }
    // Over existing content the stroke moves at most half way to its colour.
    let (_d2, mut over) = store();
    for y in 60..340 {
        for x in 60..340 {
            let o = crate::stamp_dab_hard(
                &mut over,
                surface(),
                (x as f32, y as f32),
                1.0,
                [0.0; 3],
                None,
            );
            assert!(o.is_complete());
        }
    }
    stroke(
        &mut over,
        &looping_path(),
        24.0,
        RED,
        PaintAmount::new(0.5, 1.0),
        DEFAULT_SPACING,
        false,
    );
    // Black opaque start, red at 50 %: red never above 0.5.
    assert!(max_over(&mut over, |t| t[0]) <= 0.5 + 1e-3);
    assert!(max_over(&mut over, |t| t[3]) <= 1.0);
}

#[test]
fn flow_ten_percent_builds_up_over_repeated_dabs_toward_the_ceiling() {
    for opacity in [1.0_f32, 0.6] {
        let (_dir, mut s) = store();
        let mut snap = StrokeSnapshot::new(surface());
        let amount = PaintAmount::new(opacity, 0.1);
        let mut seen = Vec::new();
        for n in 1..=60 {
            let o = stamp_dab_amount(
                &mut s,
                surface(),
                (100.5, 100.5),
                10.0,
                RED,
                Some(&mut snap),
                None,
                false,
                false,
                amount,
            );
            assert!(o.is_complete());
            if [1, 2, 5, 10, 60].contains(&n) {
                seen.push(texel(&mut s, 100, 100)[3]);
            }
        }
        // 1 - 0.9^n of the ceiling: 0.1, 0.19, 0.41, 0.65, 0.998.
        for (got, n) in seen.iter().zip([1, 2, 5, 10, 60]) {
            let want = opacity * (1.0 - 0.9_f32.powi(n));
            assert!(
                (got - want).abs() < 2e-3,
                "o {opacity} n {n}: {got} vs {want}"
            );
        }
        assert!(seen.windows(2).all(|w| w[1] > w[0]));
    }
}

// ---------------------------------------------------------------------------
// The transparency lock, the selection, the stroke's life.

#[test]
fn the_transparency_lock_keeps_alpha_bit_for_bit_through_a_crossing_stroke() {
    let (_dir, mut s) = store();
    // A translucent patch of known colour and alpha.
    for y in 60..340_u32 {
        for x in 60..340_u32 {
            let o = crate::stamp_dab_hard(
                &mut s,
                surface(),
                (x as f32, y as f32),
                1.0,
                [0.5, 0.5, 0.5],
                None,
            );
            assert!(o.is_complete());
        }
    }
    // Halve every alpha: a straight translucent grey (premultiplied 0.25).
    for ty in 0..2 {
        for tx in 0..2 {
            let Ok(t) = s.get_mut(surface(), TileId { x: tx, y: ty }) else {
                unreachable!("tile");
            };
            for c in t.texels_mut().chunks_exact_mut(4) {
                if c[3].to_f32() > 0.0 {
                    c[0] = half::f16::from_f32(0.25);
                    c[1] = half::f16::from_f32(0.25);
                    c[2] = half::f16::from_f32(0.25);
                    c[3] = half::f16::from_f32(0.5);
                }
            }
        }
    }
    let before = texel(&mut s, 200, 200);
    stroke(
        &mut s,
        &looping_path(),
        24.0,
        RED,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        true,
    );
    let after = texel(&mut s, 100, 100);
    assert_eq!(after[3], 0.5, "alpha untouched");
    assert_eq!(texel(&mut s, 200, 200)[3], before[3]);
    // The centre of the stroke has taken the colour: premultiplied red = alpha.
    assert!(after[0] > 0.45 && after[0] <= 0.5 + 1e-3, "{after:?}");
    assert!(after[1] < 0.02 && after[2] < 0.02, "{after:?}");
    // Never beyond colour * alpha anywhere.
    assert!(max_over(&mut s, |t| t[0]) <= 0.5 + 1e-3);
}

struct Half;
impl Coverage for Half {
    fn bounds(&self) -> Option<aurora_core::Rect> {
        None
    }
    fn at(&self, x: i64, _y: i64) -> f32 {
        if x < 100 { 0.0 } else { 0.5 }
    }
}

#[test]
fn a_selection_scales_the_amount_and_excludes_what_it_covers_not() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    for x in [100.5_f32, 100.5] {
        let o = stamp_dab_amount(
            &mut s,
            surface(),
            (x, 100.5),
            20.0,
            RED,
            Some(&mut snap),
            Some(&Half),
            false,
            false,
            PaintAmount::FULL,
        );
        assert!(o.is_complete());
    }
    // Coverage 0.5 twice: A = 1 - 0.5 * 0.5 = 0.75.
    assert!((texel(&mut s, 100, 100)[3] - 0.75).abs() < 2e-3);
    assert_eq!(texel(&mut s, 90, 100), [0.0; 4], "outside the selection");
}

#[test]
fn a_stroke_is_one_undo_step_and_its_coverage_is_not_history() {
    let (_dir, mut s) = store();
    let snap = stroke(
        &mut s,
        &[(200.5, 300.5), (330.5, 300.5)],
        24.0,
        RED,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        false,
    );
    assert_eq!(snap.captured().count(), 2, "crosses the tile column at 256");
    assert_eq!(snap.stroke_coverage_tiles(), 2, "A is kept while it lives");
    // A clone (history keeps the snapshot) is packed on push and drops A.
    let mut history = PixelHistory::new();
    let probe = snap.clone();
    assert!(history.push(snap.clone()));
    assert_eq!(
        history
            .peek_undo()
            .map(StrokeSnapshot::stroke_coverage_tiles),
        Some(0),
        "pushing the stroke frees its coverage"
    );
    assert_eq!(probe.stroke_coverage_tiles(), 2);
    assert!(texel(&mut s, 260, 300)[3] > 0.9);
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(
        texel(&mut s, 260, 300),
        [0.0; 4],
        "undo restores the blank layer"
    );
    assert!(!history.can_undo(), "one step, not one per dab");
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert!(texel(&mut s, 260, 300)[3] > 0.9);
}

#[test]
fn no_snapshot_means_no_coverage_is_kept_and_each_dab_is_its_own_stroke() {
    let (_dir, mut s) = store();
    for _ in 0..2 {
        assert!(stamp_dab(&mut s, surface(), (100.5, 100.5), 10.0, RED, None).is_complete());
    }
    // Two separate strokes build up: 1 - (1 - 1)(1 - 1) = 1 at the centre,
    // and 1 - (1 - a)^2 elsewhere.
    let a = (1.0 - 5.0_f32 / 10.0).powf(1.5);
    let t = texel(&mut s, 105, 100);
    assert!((t[3] - (1.0 - (1.0 - a) * (1.0 - a))).abs() < 2e-3, "{t:?}");
}

// ---------------------------------------------------------------------------
// Spacing: why the soft Brush is back on the quarter-radius spacing.

/// The stroke's coverage at lateral offset `y` from a long horizontal
/// centre line, with the dabs shifted by `phase` of a step.
fn lateral_alpha(spacing: f32, radius: f32, y: f32, phase: f32) -> f32 {
    let (_dir, mut s) = store();
    let step = dab_step(radius, spacing);
    let x0 = 150.5 + phase * step;
    let path = [(x0, 300.5), (x0 + 400.0, 300.5)];
    stroke(
        &mut s,
        &path,
        radius,
        RED,
        PaintAmount::FULL,
        spacing,
        false,
    );
    texel(&mut s, 350, (300.5 + y).floor() as u32)[3]
}

#[test]
fn at_the_default_spacing_a_stroke_has_no_visible_beading() {
    for radius in [24.0_f32, 100.0] {
        // Centre line and a few lateral offsets: the alpha depends on where
        // the dabs fall by under 1 % of full coverage.
        let mut worst = 0.0_f32;
        for frac in [0.0_f32, 0.25, 0.5, 0.75, 0.9] {
            let y = frac * radius;
            let values: Vec<f32> = [0.0, 0.25, 0.5, 0.75]
                .iter()
                .map(|&ph| lateral_alpha(DEFAULT_SPACING, radius, y, ph))
                .collect();
            let lo = values.iter().copied().fold(f32::MAX, f32::min);
            let hi = values.iter().copied().fold(0.0, f32::max);
            worst = worst.max(hi - lo);
        }
        println!("r={radius}: largest phase ripple {worst:.4}");
        assert!(worst < 0.012, "radius {radius}: ripple {worst}");
        assert!(lateral_alpha(DEFAULT_SPACING, radius, 0.0, 0.5) > 0.99);
    }
}

#[test]
fn a_curved_stroke_matches_the_continuous_stroke() {
    // Against the same path stamped with dabs 0.5 px apart (the continuous
    // limit's accumulation is much harder, so compare against a stroke at
    // the same spacing but the dabs shifted by half a step: ripple only).
    let radius = 24.0_f32;
    let mut points = Vec::new();
    let mut deg = 200.0_f32;
    while deg <= 320.0 {
        let r = deg.to_radians();
        points.push((250.0 + 140.0 * r.cos(), 400.0 + 140.0 * r.sin()));
        deg += 6.0;
    }
    let (_d1, mut a) = store();
    let (_d2, mut b) = store();
    stroke(
        &mut a,
        &points,
        radius,
        RED,
        PaintAmount::FULL,
        DEFAULT_SPACING,
        false,
    );
    let shifted: Vec<(f32, f32)> = points.iter().map(|p| (p.0, p.1)).collect();
    let first = dab_step(radius, DEFAULT_SPACING) * 0.5;
    let mut dabs = vec![shifted[0]];
    let mut carry = first;
    for pair in shifted.windows(2) {
        let (d, c) = advance_segment(pair[0], pair[1], carry, dab_step(radius, DEFAULT_SPACING));
        dabs.extend(d);
        carry = c;
    }
    let mut snap = StrokeSnapshot::new(surface());
    for d in dabs {
        let o = stamp_dab(&mut b, surface(), d, radius, RED, Some(&mut snap));
        assert!(o.is_complete());
    }
    let mut worst = 0.0_f32;
    for y in 240..420_u32 {
        for x in 90..420_u32 {
            let (p, q) = (texel(&mut a, x, y)[3], texel(&mut b, x, y)[3]);
            // Skip the stroke's two ends, where the extra dab differs.
            let near_end = [points[0], points[points.len() - 1]]
                .iter()
                .any(|e| (x as f32 - e.0).hypot(y as f32 - e.1) < radius * 1.5);
            if !near_end {
                worst = worst.max((p - q).abs());
            }
        }
    }
    println!("curve: worst phase difference {worst:.4}");
    assert!(worst < 0.03, "{worst}");
}

#[test]
fn denser_dabs_would_harden_the_brush() {
    // The reason 0.226.1's 0.03 spacing is gone: accumulated, dense dabs
    // make the soft edge nearly hard. At 0.75 r from the centre line the
    // quarter-radius stroke is about a third covered, the 0.03 one 96 %.
    let radius = 100.0_f32;
    let default = lateral_alpha(DEFAULT_SPACING, radius, 75.0, 0.3);
    let dense = lateral_alpha(0.03, radius, 75.0, 0.3);
    println!("coverage at 0.75 r: spacing 0.25 -> {default:.3}, 0.03 -> {dense:.3}");
    assert!(default > 0.25 && default < 0.4, "{default}");
    assert!(dense > 0.9, "{dense}");
    // And at the quarter-radius spacing the stroke is still soft: the 10 %
    // and 90 % points are 0.4 r or more apart.
    let at = |frac: f32| lateral_alpha(DEFAULT_SPACING, radius, frac * radius, 0.3);
    assert!(
        at(0.35) > 0.9 && at(0.85) < 0.2,
        "{} {}",
        at(0.35),
        at(0.85)
    );
}

#[test]
fn a_carry_past_a_shrunken_step_puts_a_dab_at_the_segment_start() {
    // 0.226.1's fix, still wanted when a tool's radius shrinks mid-stroke:
    // 40 px carried, then a 3 px step. The dab is due at once, at the
    // segment's start, not 37 px behind it.
    let (dabs, carry) = advance_segment((100.0, 0.0), (101.0, 0.0), 40.0, 3.0);
    assert_eq!(dabs, vec![(100.0, 0.0)]);
    assert!((carry - 1.0).abs() < 1e-5);
}

fn surface() -> SurfaceId {
    SurfaceId::from_raw(0)
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(256) else {
        unreachable!("non-zero");
    };
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => (dir, store),
        Err(err) => unreachable!("store: {err:?}"),
    }
}

/// Prints the per-dab cost behind the docs. A measurement, not an
/// assertion (timings vary by machine); run with `--nocapture`.
#[test]
fn measure_soft_brush_dab_cost() {
    for radius in [24.0_f32, 100.0] {
        for with_snapshot in [false, true] {
            let (_dir, mut s) = store();
            let path = [(150.5_f32, 300.5_f32), (350.5, 300.5)];
            let dabs = dabs_along_path(&path, radius, DEFAULT_SPACING);
            let mut snap = StrokeSnapshot::new(surface());
            let t = std::time::Instant::now();
            for &d in &dabs {
                let snapshot = if with_snapshot { Some(&mut snap) } else { None };
                assert!(
                    stamp_dab(&mut s, surface(), d, radius, [1.0, 0.0, 0.0], snapshot)
                        .is_complete()
                );
            }
            let el = t.elapsed();
            println!(
                "COST r={radius} snapshot={with_snapshot}: {} dabs {:?} ({:.0} us/dab)",
                dabs.len(),
                el,
                el.as_micros() as f64 / dabs.len().max(1) as f64
            );
        }
    }
}
