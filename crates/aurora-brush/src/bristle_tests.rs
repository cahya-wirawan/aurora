//! Tests and the (ignored) cost measurement for the experimental bristle
//! brush (task 168). The measurement is
//! `cargo test --release -p aurora-brush bristle_measure -- --ignored --nocapture`.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::many_single_char_names
)]

use std::num::NonZeroUsize;
use std::time::Instant;

use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};

use crate::bristle::{
    BristleBrush, BristleConfig, DepositStats, MAX_BRISTLES, MAX_DABS_PER_BRISTLE_SAMPLE,
    MIN_BRISTLES, PenSample,
};
use crate::deposit::Mode;
use crate::{
    DEFAULT_SPACING, PaintAmount, PixelHistory, StrokeSnapshot, advance_segment, dab_step,
    stamp_dab_amount,
};

fn surface() -> SurfaceId {
    SurfaceId::from_raw(0)
}

fn store_with_budget(tiles: usize) -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(tiles) else {
        unreachable!("non-zero");
    };
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => (dir, store),
        Err(err) => unreachable!("store: {err:?}"),
    }
}

// ---------------------------------------------------------------------------
// Synthetic pen input (deterministic, test-only).

#[derive(Debug, Clone, Copy)]
enum Shape {
    Line,
    Circle,
    ZigZag,
}

/// Triangle wave in `-1..=1`, period 1.
fn tri(u: f32) -> f32 {
    4.0 * (u - u.floor() - 0.5).abs() - 1.0
}

/// `n` samples at `rate_hz` of `shape`, starting `phase` seconds in, on a
/// 2600 x 2600 px canvas, the pen moving at about `speed` px/s (the zig-zag's
/// speed is approximate: two triangle waves of different frequency).
/// Pressure, tilt and rotation vary smoothly.
fn synthetic(shape: Shape, rate_hz: f32, n: usize, phase: f32, speed: f32) -> Vec<PenSample> {
    (0..n)
        .map(|i| {
            let t = phase + i as f32 / rate_hz;
            let (x, y) = match shape {
                Shape::Line => (
                    1300.0 + 900.0 * tri(t * speed / 3600.0),
                    1300.0 + 40.0 * t.sin(),
                ),
                Shape::Circle => {
                    let a = t * speed / 800.0;
                    (1300.0 + 800.0 * a.cos(), 1300.0 + 800.0 * a.sin())
                }
                Shape::ZigZag => {
                    // |v| = 3200 fx + 2400 fy per axis; fx = 0.5 k, fy = 1.2 k
                    // gives about 3,300 px/s at k = 1.
                    let k = speed / 3300.0;
                    (
                        1300.0 + 800.0 * tri(t * 0.5 * k),
                        1300.0 + 600.0 * tri(t * 1.2 * k),
                    )
                }
            };
            let tau = std::f32::consts::TAU;
            PenSample {
                x,
                y,
                pressure: 0.2 + 0.75 * (0.5 + 0.5 * (tau * t / 1.7).sin()),
                tilt_x: 0.5 * (tau * t / 2.3).sin(),
                tilt_y: 0.4 * (tau * t / 3.1).cos(),
                rotation: tau * t / 5.0,
            }
        })
        .collect()
}

fn mean_step_px(samples: &[PenSample]) -> f32 {
    let total: f32 = samples
        .windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum();
    total / (samples.len().max(2) - 1) as f32
}

fn pen(x: f32, y: f32) -> PenSample {
    PenSample {
        x,
        y,
        pressure: 1.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        rotation: 0.0,
    }
}

fn texels_of(store: &mut TileStore, id: TileId) -> Vec<f32> {
    if !store.contains_tile(surface(), id) {
        return Vec::new();
    }
    match store.get(surface(), id) {
        Ok(tile) => tile.texels().iter().map(|v| v.to_f32()).collect(),
        Err(err) => unreachable!("tile: {err:?}"),
    }
}

fn paint_stroke(
    config: BristleConfig,
    samples: &[PenSample],
    store: &mut TileStore,
) -> (BristleBrush, StrokeSnapshot) {
    let mut brush = BristleBrush::new(config);
    let mut snap = StrokeSnapshot::new(surface());
    for s in samples {
        let stats = brush.step(*s, store, surface(), Some(&mut snap));
        assert_eq!(stats.failed, 0);
    }
    (brush, snap)
}

// ---------------------------------------------------------------------------
// Engine tests.

#[test]
fn count_and_radius_are_clamped_and_layout_is_seeded() {
    let mut small = BristleBrush::new(BristleConfig::new(1, 12.0));
    let mut big = BristleBrush::new(BristleConfig::new(1_000_000, f32::NAN));
    assert!(small.advance(pen(10.0, 10.0)));
    assert!(big.advance(pen(10.0, 10.0)));
    assert_eq!(small.positions().count(), MIN_BRISTLES);
    assert_eq!(big.positions().count(), MAX_BRISTLES);
    assert!(big.config().radius.is_finite());

    let mut a = BristleBrush::new(BristleConfig::new(64, 30.0));
    let mut b = BristleBrush::new(BristleConfig::new(64, 30.0));
    let mut c = BristleBrush::new(BristleConfig {
        seed: 7,
        ..BristleConfig::new(64, 30.0)
    });
    for brush in [&mut a, &mut b, &mut c] {
        assert!(brush.advance(pen(100.0, 100.0)));
    }
    let pa: Vec<_> = a.positions().collect();
    assert_eq!(pa, b.positions().collect::<Vec<_>>());
    assert_ne!(
        pa,
        c.positions().collect::<Vec<_>>(),
        "a different seed lays out differently"
    );
}

#[test]
fn a_stroke_is_deterministic_to_the_bit() {
    let samples = synthetic(Shape::Circle, 240.0, 120, 0.0, 800.0);
    let run = || {
        let (_dir, mut store) = store_with_budget(256);
        let (brush, _snap) = paint_stroke(BristleConfig::new(48, 20.0), &samples, &mut store);
        let tiles: Vec<Vec<f32>> = (0..11)
            .flat_map(|y| (0..11).map(move |x| TileId { x, y }))
            .map(|id| texels_of(&mut store, id))
            .collect();
        (brush.positions().collect::<Vec<_>>(), tiles)
    };
    let (pos1, tiles1) = run();
    let (pos2, tiles2) = run();
    assert_eq!(pos1, pos2);
    assert_eq!(tiles1, tiles2);
    assert!(
        tiles1.iter().any(|t| !t.is_empty()),
        "the stroke painted something"
    );
}

#[test]
fn bristles_lag_a_jump_then_converge_when_the_pen_stops() {
    let mut brush = BristleBrush::new(BristleConfig::new(64, 20.0));
    assert!(brush.advance(pen(100.0, 100.0)));
    assert!(
        brush.max_lag(pen(100.0, 100.0)) < 1.0e-3,
        "the first sample snaps"
    );
    let target = pen(160.0, 100.0);
    assert!(brush.advance(target));
    let after_one = brush.max_lag(target);
    assert!(
        after_one > 5.0,
        "bristles trail after a 60 px jump: {after_one}"
    );
    let mut last = after_one;
    for _ in 0..60 {
        assert!(brush.advance(target));
        let now = brush.max_lag(target);
        assert!(now <= last + 1.0e-4, "lag only shrinks while the pen rests");
        last = now;
    }
    assert!(last < 1.0e-3, "converged: {last}");
    // Stiffer bristles are closer after one step than softer ones.
    let mut rigid = BristleBrush::new(BristleConfig {
        lag: 0.0,
        ..BristleConfig::new(64, 20.0)
    });
    assert!(rigid.advance(pen(100.0, 100.0)));
    assert!(rigid.advance(target));
    assert!(rigid.max_lag(target) < 1.0e-3, "lag 0 follows exactly");
}

#[test]
fn bristles_splay_on_a_curve() {
    // On a circular path with lag, the bristle cloud is displaced inward of
    // the path relative to a rigid brush: its centroid trails the pen.
    let samples = synthetic(Shape::Circle, 240.0, 200, 0.0, 800.0);
    let centroid_gap = |lag: f32| {
        let mut brush = BristleBrush::new(BristleConfig {
            lag,
            ..BristleConfig::new(64, 20.0)
        });
        let mut gap = 0.0;
        for s in &samples {
            assert!(brush.advance(*s));
            let n = brush.positions().count() as f32;
            let (sx, sy) = brush
                .positions()
                .fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
            gap = (sx / n - s.x).hypot(sy / n - s.y);
        }
        gap
    };
    assert!(centroid_gap(0.8) > centroid_gap(0.0) + 0.5);
}

#[test]
fn paint_load_depletes_along_the_stroke() {
    let mut brush = BristleBrush::new(BristleConfig {
        load_length_radii: 10.0,
        ..BristleConfig::new(32, 10.0)
    });
    let mut flows = Vec::new();
    for i in 0..400 {
        assert!(brush.advance(pen(50.0 + 3.0 * i as f32, 100.0)));
        let total: f32 = brush.pending().iter().map(|d| d.flow).sum();
        flows.push((total, brush.pending().len()));
    }
    assert!(brush.loads().all(|l| (0.0..=1.0).contains(&l)));
    assert!(
        brush.loads().all(|l| l < 0.5),
        "a 1,200 px stroke dries a 100 px load"
    );
    let early: f32 = flows[1..21].iter().map(|f| f.0).sum();
    let late: f32 = flows[380..400].iter().map(|f| f.0).sum();
    assert!(late < early * 0.2, "flow falls: early {early}, late {late}");
    assert_eq!(flows[399].1, 0, "a dry brush deposits nothing");
}

#[test]
fn pressure_zero_deposits_nothing() {
    let (_dir, mut store) = store_with_budget(64);
    let mut brush = BristleBrush::new(BristleConfig::new(64, 20.0));
    let mut snap = StrokeSnapshot::new(surface());
    for i in 0..50 {
        let s = PenSample {
            pressure: 0.0,
            ..pen(40.0 + 5.0 * i as f32, 60.0)
        };
        let stats = brush.step(s, &mut store, surface(), Some(&mut snap));
        assert_eq!(stats.dabs, 0);
    }
    assert!(brush.pending().is_empty());
    assert!(snap.is_empty(), "no tile was captured");
    assert!(
        !store.contains_tile(surface(), TileId { x: 0, y: 0 }),
        "no phantom tile"
    );
    // The same path at pressure 1 does paint.
    for i in 0..50 {
        brush.step(
            pen(40.0 + 5.0 * i as f32, 60.0),
            &mut store,
            surface(),
            Some(&mut snap),
        );
    }
    assert!(!snap.is_empty());
}

#[test]
fn pressure_scales_the_spread_and_tilt_shifts_and_stretches() {
    let spread = |s: PenSample| {
        let mut b = BristleBrush::new(BristleConfig::new(256, 40.0));
        assert!(b.advance(s));
        let n = b.positions().count() as f32;
        let (cx, cy) = b
            .positions()
            .fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (cx, cy) = (cx / n, cy / n);
        let ext_x = b.positions().map(|p| (p.0 - cx).abs()).fold(0.0, f32::max);
        let ext_y = b.positions().map(|p| (p.1 - cy).abs()).fold(0.0, f32::max);
        (cx, cy, ext_x, ext_y)
    };
    let (_, _, hx, hy) = spread(pen(500.0, 500.0));
    let (_, _, lx, ly) = spread(PenSample {
        pressure: 0.2,
        ..pen(500.0, 500.0)
    });
    assert!(
        lx < hx * 0.7 && ly < hy * 0.7,
        "light pressure spreads less"
    );
    assert!(lx > 0.0, "but never collapses to a point");

    let (cx, cy, ex, ey) = spread(PenSample {
        tilt_x: 0.8,
        ..pen(500.0, 500.0)
    });
    assert!(
        cx > 500.0 + 10.0 && (cy - 500.0).abs() < 1.0,
        "centre shifts along +x"
    );
    assert!(
        ex > ey * 1.5,
        "stretched along the tilt direction: {ex} vs {ey}"
    );
    let (_, cy2, ex2, ey2) = spread(PenSample {
        tilt_y: -0.8,
        ..pen(500.0, 500.0)
    });
    assert!(cy2 < 500.0 - 10.0 && ey2 > ex2 * 1.5);
}

#[test]
fn rotation_rotates_the_layout() {
    let at = |rotation: f32| {
        let mut b = BristleBrush::new(BristleConfig::new(64, 40.0));
        assert!(b.advance(PenSample {
            rotation,
            ..pen(500.0, 500.0)
        }));
        b.positions().collect::<Vec<_>>()
    };
    let a = at(0.0);
    let half_turn = at(std::f32::consts::PI);
    for (p, q) in a.iter().zip(&half_turn) {
        // A half turn about the centre maps each offset to its negative.
        assert!(((p.0 - 500.0) + (q.0 - 500.0)).abs() < 1.0e-2);
        assert!(((p.1 - 500.0) + (q.1 - 500.0)).abs() < 1.0e-2);
    }
    assert_ne!(a, at(1.0));
    // A rotation of 2 pi (or a huge one) is the same as none.
    assert_eq!(
        at(std::f32::consts::TAU).len(),
        a.len(),
        "wrapping keeps the layout intact"
    );
}

#[test]
fn output_is_valid_premultiplied_half_float() {
    let (_dir, mut store) = store_with_budget(256);
    let samples = synthetic(Shape::ZigZag, 240.0, 300, 0.0, 3000.0);
    let _ = paint_stroke(BristleConfig::new(96, 24.0), &samples, &mut store);
    let mut checked = 0_usize;
    for y in 0..11 {
        for x in 0..11 {
            let texels = texels_of(&mut store, TileId { x, y });
            for px in texels.chunks_exact(CHANNELS) {
                let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
                assert!(px.iter().all(|v| v.is_finite()));
                assert!((0.0..=1.0).contains(&a), "alpha {a}");
                for c in [r, g, b] {
                    assert!(
                        c >= 0.0 && c <= a + 2.0e-3,
                        "premultiplied colour {c} alpha {a}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn hostile_pen_samples_are_refused_or_clamped_without_panic() {
    let (_dir, mut store) = store_with_budget(64);
    let mut brush = BristleBrush::new(BristleConfig::new(16, 12.0));
    let mut snap = StrokeSnapshot::new(surface());
    let nan = f32::NAN;
    let inf = f32::INFINITY;
    let good = pen(100.0, 100.0);
    brush.step(good, &mut store, surface(), Some(&mut snap));
    let before: Vec<_> = brush.positions().collect();
    for bad in [
        PenSample { x: nan, ..good },
        PenSample { y: inf, ..good },
        PenSample {
            pressure: nan,
            ..good
        },
        PenSample {
            tilt_x: -inf,
            ..good
        },
        PenSample {
            tilt_y: nan,
            ..good
        },
        PenSample {
            rotation: inf,
            ..good
        },
    ] {
        assert!(!brush.advance(bad), "{bad:?}");
        assert_eq!(
            brush.step(bad, &mut store, surface(), Some(&mut snap)).dabs,
            0
        );
        assert_eq!(
            before,
            brush.positions().collect::<Vec<_>>(),
            "a refused sample changes nothing"
        );
    }
    // Finite but absurd values are clamped, not refused, and stay bounded.
    let mut far = BristleBrush::new(BristleConfig::new(16, 12.0));
    let huge = PenSample {
        x: -1.0e30,
        y: -1.0e30,
        pressure: 50.0,
        tilt_x: 1.0e20,
        tilt_y: -1.0e20,
        rotation: 1.0e30,
    };
    assert!(far.advance(huge));
    assert!(far.positions().all(|p| p.0.is_finite() && p.1.is_finite()));
    assert!(
        far.positions()
            .all(|p| p.0.abs() <= 2.0e6 && p.1.abs() <= 2.0e6)
    );
    let jump = PenSample {
        x: 1.0e30,
        y: 1.0e30,
        ..huge
    };
    assert!(far.advance(jump));
    assert!(
        far.pending().len() <= 16 * MAX_DABS_PER_BRISTLE_SAMPLE,
        "a jump is bounded"
    );
    assert!(far.positions().all(|p| p.0.is_finite() && p.1.is_finite()));
    // Stamping the far-away (negative, clamped) dabs must not panic either.
    let _ = far.deposit(&mut store, surface(), None);
    // And a zero-length stroke (the same sample again and again) is fine.
    let mut still = BristleBrush::new(BristleConfig::new(16, 12.0));
    for _ in 0..10 {
        still.step(good, &mut store, surface(), Some(&mut snap));
    }
}

#[test]
fn the_batch_deposit_paints_the_same_stroke_as_the_per_dab_deposit() {
    // Task 170: a whole bristle stroke (lag, splay, tilt, depletion, several
    // tile boundaries) through both deposits gives the same tiles bit for
    // bit, the same captured undo state, and the same result after undo.
    let samples = synthetic(Shape::ZigZag, 240.0, 240, 0.2, 3000.0);
    for (count, radius) in [(64_usize, 40.0_f32), (512, 150.0), (1024, 12.0)] {
        let config = BristleConfig {
            load_length_radii: 30.0,
            ..BristleConfig::new(count, radius)
        };
        let (_da, mut a) = store_with_budget(4096);
        let (_db, mut b) = store_with_budget(4096);
        let mut ba = BristleBrush::new(config);
        let mut bb = BristleBrush::new(config);
        let mut sa = StrokeSnapshot::new(surface());
        let mut sb = StrokeSnapshot::new(surface());
        for s in &samples {
            assert!(ba.advance(*s) && bb.advance(*s));
            let x = ba.deposit_per_dab(&mut a, surface(), Some(&mut sa));
            let y = bb.deposit(&mut b, surface(), Some(&mut sb));
            assert_eq!(x, y);
        }
        let mut held: Vec<TileId> = a
            .held_tile_versions()
            .into_iter()
            .map(|((_, id), _)| id)
            .collect();
        let mut held_b: Vec<TileId> = b
            .held_tile_versions()
            .into_iter()
            .map(|((_, id), _)| id)
            .collect();
        held.sort_by_key(|t| (t.y, t.x));
        held_b.sort_by_key(|t| (t.y, t.x));
        assert_eq!(held, held_b, "N={count}: the same tiles");
        assert!(
            held.len() > 2,
            "N={count}: the stroke crosses tile boundaries ({})",
            held.len()
        );
        let same = |a: &mut TileStore, b: &mut TileStore| {
            held.iter().all(|&id| {
                let x: Vec<u16> = match a.get(surface(), id) {
                    Ok(t) => t.texels().iter().map(|v| v.to_bits()).collect(),
                    Err(_) => Vec::new(),
                };
                let y: Vec<u16> = match b.get(surface(), id) {
                    Ok(t) => t.texels().iter().map(|v| v.to_bits()).collect(),
                    Err(_) => Vec::new(),
                };
                x == y
            })
        };
        assert!(same(&mut a, &mut b), "N={count}: the same pixels");
        let mut ca: Vec<TileId> = sa.captured().collect();
        let mut cb: Vec<TileId> = sb.captured().collect();
        ca.sort_by_key(|t| (t.y, t.x));
        cb.sort_by_key(|t| (t.y, t.x));
        assert_eq!(ca, cb, "N={count}: the same captured tiles");
        let (mut ha, mut hb) = (PixelHistory::new(), PixelHistory::new());
        assert!(ha.push(sa) && hb.push(sb));
        assert!(matches!(ha.undo(&mut a), Ok(true)));
        assert!(matches!(hb.undo(&mut b), Ok(true)));
        assert!(
            same(&mut a, &mut b),
            "N={count}: the same pixels after undo"
        );
    }
}

// ---------------------------------------------------------------------------
// The measurement.

struct Timing {
    total_us: Vec<f64>,
    advance_us: f64,
    deposit_us: f64,
    dabs: usize,
    empty_samples: usize,
    push_ms: f64,
    snapshot_tiles: usize,
    snapshot_mib: f64,
    pushes: usize,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted.get(i).copied().unwrap_or(f64::NAN)
}

fn report(label: &str, t: &Timing) {
    let mut v = t.total_us.clone();
    v.sort_by(f64::total_cmp);
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    println!(
        "{label:<34} n={:<5} mean {:>8.1} us  median {:>8.1}  p99 {:>9.1}  max {:>9.1} | adv {:>6.1} dep {:>8.1} us/sample | dabs/sample {:>7.1} empty {:>4.1}% | undo: {:>5.1} tiles {:>6.1} MiB push {:>6.2} ms",
        v.len(),
        mean,
        percentile(&v, 0.5),
        percentile(&v, 0.99),
        v.last().copied().unwrap_or(f64::NAN),
        t.advance_us / n,
        t.deposit_us / n,
        t.dabs as f64 / n,
        100.0 * t.empty_samples as f64 / n,
        t.snapshot_tiles as f64 / t.pushes.max(1) as f64,
        t.snapshot_mib / t.pushes.max(1) as f64,
        t.push_ms / t.pushes.max(1) as f64,
    );
}

/// Samples per stroke; a stroke is one snapshot and one fresh brush load.
const STROKE_SAMPLES: usize = 300;
/// Strokes per configuration: 8 x 300 = 2,400 timed samples.
const STROKES: usize = 8;

fn strokes_at(shape: Shape, rate: f32, speed: f32) -> Vec<Vec<PenSample>> {
    (0..STROKES)
        .map(|k| synthetic(shape, rate, STROKE_SAMPLES, k as f32 * 0.37, speed))
        .collect()
}

/// The standard speeds: 800 px/s for the line and circle, 3,000 for the
/// scribble.
fn strokes_for(shape: Shape, rate: f32) -> Vec<Vec<PenSample>> {
    let speed = if matches!(shape, Shape::ZigZag) {
        3000.0
    } else {
        800.0
    };
    strokes_at(shape, rate, speed)
}

/// Which deposit a measurement runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// The task 168 spike's: `stamp_dab_amount` once per dab.
    PerDab,
    /// Task 170's batch deposit, one tile after another.
    Serial,
    /// Task 170's batch deposit as the brush runs it (`Mode::Auto`: tiles
    /// on the deposit pool when the batch is big enough).
    Batch,
}

fn measure_bristle(config: BristleConfig, strokes: &[Vec<PenSample>], path: Path) -> Timing {
    let (_dir, mut store) = store_with_budget(512);
    let mut t = Timing {
        total_us: Vec::new(),
        advance_us: 0.0,
        deposit_us: 0.0,
        dabs: 0,
        empty_samples: 0,
        push_ms: 0.0,
        snapshot_tiles: 0,
        snapshot_mib: 0.0,
        pushes: 0,
    };
    let mut history = PixelHistory::new();
    for samples in strokes {
        let mut brush = BristleBrush::new(config);
        let mut snap = StrokeSnapshot::new(surface());
        for s in samples {
            let t0 = Instant::now();
            let ok = brush.advance(*s);
            let t1 = Instant::now();
            let stats = if !ok {
                DepositStats::default()
            } else if path == Path::Batch {
                brush.deposit(&mut store, surface(), Some(&mut snap))
            } else if path == Path::Serial {
                brush.deposit_with(&mut store, surface(), Some(&mut snap), Mode::Serial)
            } else {
                brush.deposit_per_dab(&mut store, surface(), Some(&mut snap))
            };
            let t2 = Instant::now();
            let adv = (t1 - t0).as_secs_f64() * 1.0e6;
            let dep = (t2 - t1).as_secs_f64() * 1.0e6;
            t.advance_us += adv;
            t.deposit_us += dep;
            t.total_us.push(adv + dep);
            t.dabs += stats.dabs;
            t.empty_samples += usize::from(stats.dabs == 0);
        }
        let tiles = snap.captured().count();
        let t0 = Instant::now();
        let kept = history.push(snap);
        t.push_ms += t0.elapsed().as_secs_f64() * 1.0e3;
        t.snapshot_tiles += tiles;
        t.snapshot_mib += history.stats().memory_bytes as f64 / 1_048_576.0_f64;
        t.pushes += 1;
        assert!(kept);
        history = PixelHistory::new();
    }
    t
}

/// The existing soft Brush driven the way `aurora-app` drives it: dabs along
/// the pen path at `DEFAULT_SPACING`, one snapshot per stroke.
fn measure_soft_brush(radius: f32, strokes: &[Vec<PenSample>]) -> Timing {
    let (_dir, mut store) = store_with_budget(512);
    let mut t = Timing {
        total_us: Vec::new(),
        advance_us: 0.0,
        deposit_us: 0.0,
        dabs: 0,
        empty_samples: 0,
        push_ms: 0.0,
        snapshot_tiles: 0,
        snapshot_mib: 0.0,
        pushes: 0,
    };
    let amount = PaintAmount::new(1.0, 0.6);
    for samples in strokes {
        let mut snap = StrokeSnapshot::new(surface());
        let step = dab_step(radius, DEFAULT_SPACING);
        let mut prev: Option<(f32, f32)> = None;
        let mut carry = 0.0;
        for s in samples {
            let t0 = Instant::now();
            let point = (s.x, s.y);
            let dabs = match prev {
                None => vec![point],
                Some(p) => {
                    let (d, c) = advance_segment(p, point, carry, step);
                    carry = c;
                    d
                }
            };
            prev = Some(point);
            let n = dabs.len();
            for d in dabs {
                let o = stamp_dab_amount(
                    &mut store,
                    surface(),
                    d,
                    radius,
                    [0.8, 0.1, 0.1],
                    Some(&mut snap),
                    None,
                    false,
                    false,
                    amount,
                );
                assert!(o.is_complete());
            }
            t.total_us.push(t0.elapsed().as_secs_f64() * 1.0e6);
            t.dabs += n;
            t.empty_samples += usize::from(n == 0);
        }
        let tiles = snap.captured().count();
        let mut history = PixelHistory::new();
        let t0 = Instant::now();
        let kept = history.push(snap);
        t.push_ms += t0.elapsed().as_secs_f64() * 1.0e3;
        t.snapshot_tiles += tiles;
        t.snapshot_mib += history.stats().memory_bytes as f64 / 1_048_576.0_f64;
        t.pushes += 1;
        assert!(kept);
    }
    // The soft Brush's whole cost is the deposit.
    t.deposit_us = t.total_us.iter().sum();
    t
}

fn measure_config(count: usize, radius: f32) -> BristleConfig {
    BristleConfig {
        // Keep the paint wet so late samples are not free: a stroke is 300
        // samples, a few hundred px.
        load_length_radii: 400.0,
        ..BristleConfig::new(count, radius)
    }
}

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").unwrap_or_else(|_| "unknown".into())
}

#[test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
fn bristle_measure() {
    println!(
        "profile: {}",
        if cfg!(debug_assertions) {
            "dev (opt-level 1)"
        } else {
            "release"
        }
    );
    println!("loadavg before: {}", load_average().trim());
    let circle240 = strokes_for(Shape::Circle, 240.0);
    println!(
        "mean pen step: circle@240 {:.1} px/sample, circle@120 {:.1}, line@240 {:.1}, zigzag@240 {:.1}, zigzag@120 {:.1}",
        mean_step_px(&circle240[0]),
        mean_step_px(&strokes_for(Shape::Circle, 120.0)[0]),
        mean_step_px(&strokes_for(Shape::Line, 240.0)[0]),
        mean_step_px(&strokes_for(Shape::ZigZag, 240.0)[0]),
        mean_step_px(&strokes_for(Shape::ZigZag, 120.0)[0]),
    );
    let quick = std::env::var("AURORA_BRISTLE_QUICK").is_ok();
    println!("--- circle @ 240 Hz: bristle matrix ---");
    let radii: &[f32] = if quick { &[48.0] } else { &[12.0, 48.0, 150.0] };
    let counts: &[usize] = if quick {
        &[128, 512]
    } else {
        &[32, 128, 512, 1024]
    };
    for &radius in radii {
        report(
            &format!("soft Brush r={radius}"),
            &measure_soft_brush(radius, &circle240),
        );
        for &count in counts {
            let probe = BristleBrush::new(measure_config(count, radius));
            report(
                &format!(
                    "bristle N={count} r={radius} (rb {:.2})",
                    probe.bristle_radius()
                ),
                &measure_bristle(measure_config(count, radius), &circle240, Path::PerDab),
            );
        }
    }
    if quick {
        // The fast cases too: a 3,200 px/s circle and the zig-zag at 240 Hz.
        for (name, strokes) in [
            ("circle 3200 px/s", strokes_at(Shape::Circle, 240.0, 3200.0)),
            ("zigzag@240", strokes_for(Shape::ZigZag, 240.0)),
        ] {
            report(
                &format!("{name} soft r=48"),
                &measure_soft_brush(48.0, &strokes),
            );
            for &count in counts {
                report(
                    &format!("{name} bristle N={count} r=48"),
                    &measure_bristle(measure_config(count, 48.0), &strokes, Path::PerDab),
                );
            }
        }
        println!("loadavg after: {}", load_average().trim());
        return;
    }
    println!("--- pen speed sweep, circle @ 240 Hz, r=48 ---");
    for speed in [200.0_f32, 800.0, 1600.0, 3200.0, 6400.0] {
        let strokes = strokes_at(Shape::Circle, 240.0, speed);
        println!(
            "speed {speed} px/s = {:.1} px/sample",
            mean_step_px(&strokes[0])
        );
        report("  soft r=48", &measure_soft_brush(48.0, &strokes));
        for count in [128_usize, 512, 1024] {
            report(
                &format!("  bristle N={count} r=48"),
                &measure_bristle(measure_config(count, 48.0), &strokes, Path::PerDab),
            );
        }
    }
    println!("--- other shapes and rates, headline cases ---");
    for (name, shape, rate) in [
        ("circle@120", Shape::Circle, 120.0),
        ("line@240", Shape::Line, 240.0),
        ("line@120", Shape::Line, 120.0),
        ("zigzag@240", Shape::ZigZag, 240.0),
        ("zigzag@120", Shape::ZigZag, 120.0),
    ] {
        let strokes = strokes_for(shape, rate);
        for radius in [48.0_f32] {
            report(
                &format!("{name} soft r={radius}"),
                &measure_soft_brush(radius, &strokes),
            );
            for count in [128_usize, 512] {
                report(
                    &format!("{name} bristle N={count} r={radius}"),
                    &measure_bristle(measure_config(count, radius), &strokes, Path::PerDab),
                );
            }
        }
    }
    println!("loadavg after: {}", load_average().trim());
}

/// Task 170: the per-dab deposit against the batch deposit, configuration by
/// configuration (each pair run back to back on fresh stores, the per-dab
/// one first). `cargo test --release -p aurora-brush bristle_deposit_measure
/// -- --ignored --nocapture`; `AURORA_BRISTLE_QUICK` runs only r = 150.
#[test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
fn bristle_deposit_measure() {
    println!(
        "profile: {}",
        if cfg!(debug_assertions) {
            "dev (opt-level 1)"
        } else {
            "release"
        }
    );
    println!("loadavg before: {}", load_average().trim());
    let quick = std::env::var("AURORA_BRISTLE_QUICK").is_ok();
    let circle240 = strokes_for(Shape::Circle, 240.0);
    println!(
        "--- circle @ 240 Hz, 800 px/s ({:.1} px/sample) ---",
        mean_step_px(&circle240[0])
    );
    let radii: &[f32] = if quick {
        &[150.0]
    } else {
        &[12.0, 48.0, 150.0]
    };
    for &radius in radii {
        for count in [32_usize, 128, 512, 1024] {
            let config = measure_config(count, radius);
            for path in [Path::PerDab, Path::Serial, Path::Batch] {
                report(
                    &format!("{path:?} N={count} r={radius}"),
                    &measure_bristle(config, &circle240, path),
                );
            }
        }
    }
    if !quick {
        for radius in [48.0_f32, 150.0] {
            println!("--- pen speed sweep, circle @ 240 Hz, r={radius} ---");
            for speed in [200.0_f32, 800.0, 1600.0, 3200.0, 6400.0] {
                let strokes = strokes_at(Shape::Circle, 240.0, speed);
                println!(
                    "speed {speed} px/s = {:.1} px/sample",
                    mean_step_px(&strokes[0])
                );
                for count in [512_usize, 1024] {
                    let config = measure_config(count, radius);
                    for path in [Path::PerDab, Path::Serial, Path::Batch] {
                        report(
                            &format!("  {path:?} N={count} r={radius}"),
                            &measure_bristle(config, &strokes, path),
                        );
                    }
                }
            }
        }
    }
    println!("loadavg after: {}", load_average().trim());
}

/// Writes one stroke as a PPM (premultiplied over white) to the path in
/// `AURORA_BRISTLE_DUMP`, for a human to look at. Not run by default.
#[test]
#[ignore = "writes a picture to $AURORA_BRISTLE_DUMP"]
fn bristle_dump() {
    let Ok(path) = std::env::var("AURORA_BRISTLE_DUMP") else {
        println!("AURORA_BRISTLE_DUMP not set; nothing to do");
        return;
    };
    let count: usize = std::env::var("AURORA_BRISTLE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let (_dir, mut store) = store_with_budget(512);
    // A quarter circle arc then a zig-zag, so lag, splay and twist show.
    let mut samples = synthetic(Shape::Circle, 240.0, 700, 0.0, 800.0);
    samples.truncate(700);
    let config = BristleConfig {
        load_length_radii: 400.0,
        ..BristleConfig::new(count, 60.0)
    };
    let (_brush, _snap) = paint_stroke(config, &samples, &mut store);
    let (x0, y0, w, h) = (400_u32, 400_u32, 1800_u32, 1800_u32);
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let id = TileId {
                x: x / TILE,
                y: y / TILE,
            };
            let rgba = if store.contains_tile(surface(), id) {
                match store.get(surface(), id) {
                    Ok(tile) => {
                        let i = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
                        let g = |c: usize| tile.texels().get(i + c).map_or(0.0, |v| v.to_f32());
                        [g(0), g(1), g(2), g(3)]
                    }
                    Err(_) => [0.0; 4],
                }
            } else {
                [0.0; 4]
            };
            for c in 0..3 {
                let v = rgba[c] + (1.0 - rgba[3]);
                out.push((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
    }
    if let Err(err) = std::fs::write(&path, out) {
        unreachable!("write {path}: {err}");
    }
    println!("wrote {path}");
}
