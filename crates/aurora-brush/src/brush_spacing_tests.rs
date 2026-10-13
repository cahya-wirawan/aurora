//! The soft Brush's dense spacing (0.226.1): the beading regression tests
//! (computed from real stamped texels), the spacing constants, the
//! per-event dab cap and its carry, and a stroke's undo and tile set. The
//! app-side routing (which tool gets which spacing) is pinned in
//! `aurora-app`'s `brush_spacing_tests`.
//!
//! Runtime numbers behind the chosen constants are printed by
//! `measure_brush_spacing_cost` (a measurement, not an assertion).

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
    BRUSH_SPACING, DEFAULT_SPACING, MAX_BRUSH_DABS_PER_EVENT, MIN_BRUSH_DABS_PER_EVENT,
    PixelHistory, StrokeSnapshot, advance_segment, brush_dab_cap, brush_segment_step, dab_step,
    dabs_along_path, spacing_dip, stamp_dab,
};

/// How far below the strongest centre-line alpha a stroke's weakest may
/// fall: 2.5 % of full alpha.
const MAX_DIP: f32 = 0.025;

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

fn alpha(store: &mut TileStore, x: u32, y: u32) -> f32 {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    if !store.contains_tile(surface(), id) {
        return 0.0;
    }
    let Ok(tile) = store.get(surface(), id) else {
        unreachable!("tile");
    };
    let index = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
    tile.texels()[index + 3].to_f32()
}

fn stamp(store: &mut TileStore, dabs: &[(f32, f32)], radius: f32) {
    for &d in dabs {
        assert!(stamp_dab(store, surface(), d, radius, [0.0; 3], None).is_complete());
    }
}

/// The Brush's dabs along `path` at the production spacing.
fn brush_dabs(path: &[(f32, f32)], radius: f32) -> Vec<(f32, f32)> {
    dabs_along_path(path, radius, BRUSH_SPACING)
}

/// A polyline following a circular arc, one vertex every 6 degrees.
fn arc(centre: (f32, f32), arc_radius: f32, from_deg: f32, to_deg: f32) -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut deg = from_deg;
    while deg <= to_deg + 1e-3 {
        let rad = deg.to_radians();
        points.push((
            centre.0 + arc_radius * rad.cos(),
            centre.1 + arc_radius * rad.sin(),
        ));
        deg += 6.0;
    }
    points
}

/// The weakest and the strongest alpha over `texels` (the centre line).
fn min_and_max(store: &mut TileStore, texels: &[(u32, u32)]) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::MAX, 0.0_f32);
    for &(x, y) in texels {
        let a = alpha(store, x, y);
        lo = lo.min(a);
        hi = hi.max(a);
    }
    (lo, hi)
}

/// Asserts the weakest centre-line alpha is within [`MAX_DIP`] of the
/// strongest.
fn assert_no_beading(store: &mut TileStore, texels: &[(u32, u32)], what: &str) {
    let (lo, hi) = min_and_max(store, texels);
    assert!(
        hi > 0.9,
        "{what}: the stroke must reach nearly full alpha, got {hi}"
    );
    assert!(
        lo >= hi * (1.0 - MAX_DIP),
        "{what}: beading, weakest centre-line alpha {lo} vs strongest {hi} ({:.2} % dip)",
        (hi - lo) / hi * 100.0
    );
}

/// Texels along a horizontal stroke's centre line, strictly between its
/// first and last dab.
fn horizontal_line(first_x: f32, last_x: f32, y: u32) -> Vec<(u32, u32)> {
    ((first_x.ceil() as u32 + 1)..(last_x.floor() as u32))
        .map(|x| (x, y))
        .collect()
}

#[test]
fn a_horizontal_brush_stroke_has_no_beading_at_radius_24_and_100() {
    for radius in [24.0_f32, 100.0] {
        let (_dir, mut s) = store();
        let path = [(150.5_f32, 300.5_f32), (330.5, 300.5)];
        let dabs = brush_dabs(&path, radius);
        stamp(&mut s, &dabs, radius);
        let last = dabs.last().map_or(0.0, |d| d.0);
        let line = horizontal_line(150.5, last, 300);
        assert!(line.len() > 100, "radius {radius}: a long centre line");
        assert_no_beading(&mut s, &line, &format!("horizontal r={radius}"));
    }
}

#[test]
fn a_diagonal_brush_stroke_has_no_beading_at_radius_24_and_100() {
    for radius in [24.0_f32, 100.0] {
        let (_dir, mut s) = store();
        // Texel centres (k + 0.5, k + 0.5) lie exactly on y = x.
        let path = [(150.5_f32, 150.5_f32), (300.5, 300.5)];
        let dabs = brush_dabs(&path, radius);
        stamp(&mut s, &dabs, radius);
        let last = dabs.last().map_or(0.0, |d| d.0);
        let line: Vec<(u32, u32)> = (151..(last.floor() as u32)).map(|k| (k, k)).collect();
        assert!(line.len() > 100);
        assert_no_beading(&mut s, &line, &format!("diagonal r={radius}"));
    }
}

/// A texel-by-texel comparison against the continuous-limit stroke of the
/// same path (dabs 0.1 px apart at radius 24, 0.5 px at radius 100): the
/// weakest texel of the dense Brush within [`MAX_DIP`] of it, over every
/// texel the reference paints above half alpha. Pointwise, so it needs no
/// centre line (a curve's texel centres are not on its path).
fn deviation_from_the_continuous_stroke(path: &[(f32, f32)], radius: f32, spacing: f32) -> f32 {
    let (_d1, mut test) = store();
    stamp(&mut test, &dabs_along_path(path, radius, spacing), radius);

    let (_d2, mut reference) = store();
    let fine_step = if radius <= 30.0 { 0.1 } else { 0.5 };
    let mut ref_dabs = vec![path[0]];
    let mut carry = 0.0;
    for pair in path.windows(2) {
        let (dabs, c) = advance_segment(pair[0], pair[1], carry, fine_step);
        ref_dabs.extend(dabs);
        carry = c;
    }
    stamp(&mut reference, &ref_dabs, radius);

    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f32::MAX, 0.0_f32, f32::MAX, 0.0_f32);
    for p in path {
        min_x = min_x.min(p.0);
        max_x = max_x.max(p.0);
        min_y = min_y.min(p.1);
        max_y = max_y.max(p.1);
    }
    let mut worst = 0.0_f32;
    // Skip the texels within a radius of the stroke's last point: the last
    // dab of the two strokes differs by up to a step, which is an end
    // effect, not beading. (The first dab is the same in both.)
    let pad = radius * 0.5;
    let end = path[path.len() - 1];
    let (x0, x1) = ((min_x - pad).max(0.0) as u32, (max_x + pad) as u32);
    let (y0, y1) = ((min_y - pad).max(0.0) as u32, (max_y + pad) as u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let want = alpha(&mut reference, x, y);
            let near_end = (x as f32 + 0.5 - end.0).hypot(y as f32 + 0.5 - end.1) < radius;
            if want > 0.5 && !near_end {
                worst = worst.max(want - alpha(&mut test, x, y));
            }
        }
    }
    worst
}

#[test]
fn a_curved_brush_stroke_has_no_beading_at_radius_24_and_100() {
    for radius in [24.0_f32, 100.0] {
        let path = arc((250.0, 400.0), 140.0, 200.0, 320.0);
        let worst = deviation_from_the_continuous_stroke(&path, radius, BRUSH_SPACING);
        println!("curve r={radius}: worst texel {worst:.4} below the continuous stroke");
        assert!(
            worst <= MAX_DIP,
            "curve r={radius}: a texel is {worst} below the continuous stroke"
        );
    }
}

#[test]
fn the_old_quarter_radius_spacing_does_fail_these_beading_checks() {
    // The tests above have teeth: at the old spacing the same measurements
    // show the 18 % beading the design owner reported.
    for radius in [24.0_f32, 100.0] {
        let (_dir, mut s) = store();
        let path = [(150.5_f32, 300.5_f32), (330.5, 300.5)];
        let dabs = dabs_along_path(&path, radius, DEFAULT_SPACING);
        stamp(&mut s, &dabs, radius);
        let last = dabs.last().map_or(0.0, |d| d.0);
        let (lo, hi) = min_and_max(&mut s, &horizontal_line(150.5, last, 300));
        let dip = (hi - lo) / hi;
        assert!(dip > 0.15 && dip < 0.20, "radius {radius}: dip {dip}");

        let worst = deviation_from_the_continuous_stroke(
            &arc((250.0, 400.0), 140.0, 200.0, 320.0),
            radius,
            DEFAULT_SPACING,
        );
        assert!(worst > 0.15, "curve r={radius}: {worst}");
    }
}

#[test]
fn the_brush_spacing_constants_and_the_documented_dip() {
    assert_eq!(DEFAULT_SPACING, 0.25, "every other tool keeps a quarter");
    assert_eq!(BRUSH_SPACING, 0.03);
    // The dip formula against the documented table.
    assert!((spacing_dip(DEFAULT_SPACING) - 0.1815).abs() < 1e-3);
    let dip = spacing_dip(BRUSH_SPACING);
    assert!((0.021..=0.0235).contains(&dip), "{dip}");
    assert!(dip <= MAX_DIP);
    assert_eq!(spacing_dip(0.0), 0.0);
    assert_eq!(spacing_dip(f32::NAN), 0.0);
    assert_eq!(spacing_dip(2.0), 1.0);
    // Steps from the table (the 0.5 px floor included).
    let step = |r: f32| dab_step(r, BRUSH_SPACING);
    assert!((step(24.0) - 0.72).abs() < 1e-5);
    assert!((step(100.0) - 3.0).abs() < 1e-5);
    assert!((step(256.0) - 7.68).abs() < 1e-4);
    assert_eq!(step(10.0), 0.5);
    assert_eq!(step(4.0), 0.5);
    assert_eq!(dab_step(0.0, BRUSH_SPACING), 0.5, "the floor holds at 0");
    // The dip the table claims at each radius, from the real step.
    for (radius, want) in [
        (24.0_f32, 0.022),
        (100.0, 0.022),
        (256.0, 0.022),
        (10.0, 0.037),
    ] {
        let got = spacing_dip(step(radius) / radius);
        assert!((got - want).abs() < 0.002, "radius {radius}: {got}");
    }
}

#[test]
fn the_per_event_dab_cap_follows_the_dab_area() {
    assert_eq!(brush_dab_cap(24.0), 138);
    assert_eq!(brush_dab_cap(100.0), 7);
    assert_eq!(brush_dab_cap(256.0), MIN_BRUSH_DABS_PER_EVENT);
    assert_eq!(brush_dab_cap(1.0), MAX_BRUSH_DABS_PER_EVENT);
    assert_eq!(brush_dab_cap(0.0), MAX_BRUSH_DABS_PER_EVENT);
    assert_eq!(brush_dab_cap(-5.0), MAX_BRUSH_DABS_PER_EVENT);
    assert_eq!(brush_dab_cap(f32::NAN), MAX_BRUSH_DABS_PER_EVENT);
    assert_eq!(brush_dab_cap(f32::INFINITY), MAX_BRUSH_DABS_PER_EVENT);
    // Never above the maximum, never below the minimum, non-increasing.
    let mut previous = usize::MAX;
    for r in 1..400 {
        let cap = brush_dab_cap(r as f32);
        assert!((MIN_BRUSH_DABS_PER_EVENT..=MAX_BRUSH_DABS_PER_EVENT).contains(&cap));
        assert!(cap <= previous, "radius {r}");
        previous = cap;
    }
}

#[test]
fn a_short_event_keeps_the_dense_step_and_a_long_one_is_capped() {
    for radius in [24.0_f32, 100.0] {
        let dense = dab_step(radius, BRUSH_SPACING);
        let cap = brush_dab_cap(radius);
        // Short: the cap would allow more dabs than the segment holds.
        assert_eq!(brush_segment_step(radius, 0.0, dense * 2.0), dense);
        // Exactly at the cap with no carry: still the dense step.
        let at_cap = dense * cap as f32;
        assert_eq!(brush_segment_step(radius, 0.0, at_cap), dense);
        // Long segments, with and without a carry: at most `cap` dabs, and
        // the cap is used (not one fewer than allowed, give or take a
        // rounding step).
        for (carry, len) in [(0.0_f32, 5_000.0_f32), (0.4, 800.0), (dense * 0.9, 333.3)] {
            let step = brush_segment_step(radius, carry, len);
            assert!(step > dense, "radius {radius}: widened");
            let (dabs, new_carry) = advance_segment((0.0, 0.0), (len, 0.0), carry, step);
            assert!(dabs.len() <= cap, "radius {radius}: {} > {cap}", dabs.len());
            assert!(
                dabs.len() + 1 >= cap,
                "radius {radius}: {} << {cap}",
                dabs.len()
            );
            assert!(
                (0.0..=step).contains(&new_carry),
                "carry stays below a step"
            );
        }
    }
    // Non-finite inputs fall back to the dense step rather than NaN.
    assert_eq!(
        brush_segment_step(24.0, f32::NAN, f32::NAN),
        dab_step(24.0, BRUSH_SPACING)
    );
    assert_eq!(
        brush_segment_step(24.0, 0.0, f32::INFINITY),
        dab_step(24.0, BRUSH_SPACING)
    );
}

#[test]
fn a_stroke_fed_in_many_small_events_equals_the_same_stroke_in_one_segment() {
    for radius in [10.0_f32, 24.0, 100.0] {
        // Short enough that even the whole path stays under the cap.
        let dense = dab_step(radius, BRUSH_SPACING);
        let length = (dense * brush_dab_cap(radius) as f32 * 0.9).min(150.0);
        let end = (length * 0.96, length * 0.28);
        // One event: the cap is not reached, so no widening.
        let whole_step = brush_segment_step(radius, 0.0, end.0.hypot(end.1));
        assert_eq!(whole_step, dab_step(radius, BRUSH_SPACING));
        let (whole, whole_carry) = advance_segment((0.0, 0.0), end, 0.0, whole_step);

        for pieces in [2_usize, 7, 31, 150] {
            let mut dabs = Vec::new();
            let mut carry = 0.0_f32;
            let mut prev = (0.0_f32, 0.0_f32);
            for i in 1..=pieces.min(150) {
                let t = i as f32 / pieces as f32;
                let next = (end.0 * t, end.1 * t);
                let len = (next.0 - prev.0).hypot(next.1 - prev.1);
                let step = brush_segment_step(radius, carry, len);
                assert_eq!(step, whole_step, "no event reaches the cap");
                let (d, c) = advance_segment(prev, next, carry, step);
                dabs.extend(d);
                carry = c;
                prev = next;
            }
            // Splitting the vector re-derives each dab from a new origin,
            // so positions agree to float noise, not bit for bit.
            assert_eq!(dabs.len(), whole.len(), "radius {radius}, {pieces} pieces");
            for (a, b) in dabs.iter().zip(&whole) {
                assert!(
                    (a.0 - b.0).abs() < 2e-3 && (a.1 - b.1).abs() < 2e-3,
                    "radius {radius}, {pieces} pieces: {a:?} vs {b:?}"
                );
            }
            assert!((carry - whole_carry).abs() < 2e-3);
        }
    }
}

#[test]
fn a_capped_event_hands_the_right_carry_to_the_next_one() {
    // A long flick (capped), then a run of short events: whatever carry the
    // flick left, the next dab is due at most one capped step after the
    // flick's last, none lands behind the pointer, and from there the dabs
    // sit one dense step apart again.
    let radius = 100.0_f32;
    let dense = dab_step(radius, BRUSH_SPACING);
    let mut carry = 0.0_f32;
    let mut prev = (0.0_f32, 0.0_f32);
    let flick = (400.0_f32, 0.0_f32);
    let flick_step = brush_segment_step(radius, carry, flick.0);
    let (dabs, c) = advance_segment(prev, flick, carry, flick_step);
    assert!(dabs.len() <= brush_dab_cap(radius));
    assert!(dabs.len() + 1 >= brush_dab_cap(radius));
    carry = c;
    prev = flick;
    let mut after = Vec::new();
    for i in 1..=60 {
        let next = (flick.0 + i as f32, 0.0);
        let step = brush_segment_step(radius, carry, 1.0);
        let (d, c) = advance_segment(prev, next, carry, step);
        assert!(d.len() <= brush_dab_cap(radius));
        after.extend(d);
        carry = c;
        prev = next;
    }
    assert!((19..=21).contains(&after.len()), "{}", after.len());
    for pair in after.windows(2) {
        assert!((pair[1].0 - pair[0].0 - dense).abs() < 2e-3, "{pair:?}");
    }
    let last_flick = dabs.last().map_or(0.0, |d| d.0);
    let first_after = after.first().map_or(0.0, |d| d.0);
    assert!(first_after >= flick.0, "no dab lands behind the pointer");
    assert!(
        first_after - last_flick <= flick_step + 1e-3,
        "the gap is one capped step at most"
    );
}

#[test]
fn a_carry_past_a_shrunken_step_puts_a_dab_at_the_segment_start() {
    // The case a variable step creates: 40 px carried, then a dense 3 px
    // step. The dab is due at once, at the segment's start, not 37 px
    // behind it.
    let (dabs, carry) = advance_segment((100.0, 0.0), (101.0, 0.0), 40.0, 3.0);
    assert_eq!(dabs, vec![(100.0, 0.0)]);
    assert!((carry - 1.0).abs() < 1e-5);
}

#[test]
fn a_dense_stroke_is_one_undo_step_over_the_same_tiles_as_before() {
    let radius = 24.0_f32;
    // Crosses the tile column at x = 256, well inside tile row 1.
    let path = [(200.5_f32, 300.5_f32), (330.5, 300.5)];
    let captured_tiles = |spacing: f32| {
        let (dir, mut s) = store();
        let mut snap = StrokeSnapshot::new(surface());
        for d in dabs_along_path(&path, radius, spacing) {
            assert!(
                stamp_dab(&mut s, surface(), d, radius, [0.0; 3], Some(&mut snap)).is_complete()
            );
        }
        let mut tiles: Vec<TileId> = snap.captured().collect();
        tiles.sort_by_key(|t| (t.y, t.x));
        (s, snap, tiles, dir)
    };
    let (_s_old, _snap_old, tiles_old, _d_old) = captured_tiles(DEFAULT_SPACING);
    let (mut s, snap, tiles_new, _d_new) = captured_tiles(BRUSH_SPACING);
    assert_eq!(tiles_new, tiles_old, "the same tiles are touched");
    assert_eq!(tiles_new.len(), 2);

    // The snapshot is a tile-level record: dense dabs add no tiles and no
    // bytes beyond the touched set.
    let mut history = PixelHistory::new();
    assert!(history.push(snap), "the whole stroke is one step");
    assert!(!history.can_redo());
    assert!(alpha(&mut s, 260, 300) > 0.9);
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(
        alpha(&mut s, 260, 300),
        0.0,
        "undo restores the blank layer"
    );
    assert!(!history.can_undo(), "one step, not one per dab");
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert!(alpha(&mut s, 260, 300) > 0.9);
}

/// Prints the dab counts and times behind the constants. Not an
/// assertion (timings vary by machine); run with `--nocapture`.
#[test]
fn measure_brush_spacing_cost() {
    for radius in [24.0_f32, 100.0] {
        for f in [DEFAULT_SPACING, 0.0267, 0.03, 0.04, 0.05, 0.06] {
            let (_dir, mut s) = store();
            let path = [(150.5_f32, 300.5_f32), (350.5, 300.5)];
            let dabs = dabs_along_path(&path, radius, f);
            let t = std::time::Instant::now();
            stamp(&mut s, &dabs, radius);
            let el = t.elapsed();
            let (_d2, mut s2) = store();
            let seg = dabs_along_path(&[(150.5, 300.5), (200.5, 300.5)], radius, f);
            let t2 = std::time::Instant::now();
            stamp(&mut s2, &seg, radius);
            let el2 = t2.elapsed();
            let last = dabs.last().map_or(0.0, |d| d.0);
            let (lo, hi) = min_and_max(&mut s, &horizontal_line(150.5, last, 300));
            println!(
                "r={radius} f={f} step={:.3} 200px: {} dabs {:?} ({:.0} us/dab) | 50px: {} dabs {:?} | dip {:.2}%",
                dab_step(radius, f),
                dabs.len(),
                el,
                el.as_micros() as f64 / dabs.len().max(1) as f64,
                seg.len(),
                el2,
                (hi - lo) / hi * 100.0
            );
        }
        let cap = brush_dab_cap(radius);
        let step = brush_segment_step(radius, 0.0, 50.0);
        let (_d3, mut s3) = store();
        let (capped, _) = advance_segment((150.5, 300.5), (200.5, 300.5), 0.0, step);
        let t3 = std::time::Instant::now();
        stamp(&mut s3, &capped, radius);
        println!(
            "r={radius} cap={cap}: a 50 px event places {} dabs (step {step:.3}) in {:?}",
            capped.len(),
            t3.elapsed()
        );
    }
}
