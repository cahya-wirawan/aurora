//! The batch deposit (task 170) against the per-dab path it replaces for the
//! bristle brush: every tile bit, every dirty rectangle, the set of tiles the
//! store holds, the stroke snapshot (captured tiles and their raw content),
//! the stroke coverage and the undo/redo result must be identical.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{SurfaceId, TileId, TileStore};
use half::f16;

use crate::deposit::{Mode, SoftDab, deposit_soft_dabs, deposit_soft_dabs_with};
use crate::test_support::surface;
use crate::{PaintAmount, PixelHistory, StrokeSnapshot, stamp_dab_amount};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1_u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn store(budget: usize) -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(budget) else {
        unreachable!("non-zero");
    };
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => (dir, store),
        Err(err) => unreachable!("store: {err:?}"),
    }
}

/// Fills a few tiles of both stores with the same random content: valid
/// premultiplied texels mostly, some arbitrary bit patterns (NaN, infinity,
/// negative, above one) so the start content covers what a float import
/// can hold. Dirty flags are cleared afterwards.
fn seed_content(rng: &mut Rng, stores: [&mut TileStore; 2]) {
    let tiles: Vec<TileId> = (0..5)
        .map(|_| TileId {
            x: rng.below(5) as u32,
            y: rng.below(5) as u32,
        })
        .collect();
    let mut contents: Vec<(TileId, Vec<f16>)> = Vec::new();
    for id in tiles {
        let mut texels = vec![f16::ZERO; aurora_tile::SAMPLES];
        for px in texels.chunks_exact_mut(4) {
            if rng.below(64) == 0 {
                for v in px.iter_mut() {
                    *v = f16::from_bits(rng.next() as u16);
                }
            } else if rng.below(3) != 0 {
                let a = rng.unit();
                px[0] = f16::from_f32(rng.unit() * a);
                px[1] = f16::from_f32(rng.unit() * a);
                px[2] = f16::from_f32(rng.unit() * a);
                px[3] = f16::from_f32(a);
            }
        }
        contents.push((id, texels));
    }
    for store in stores {
        for (id, texels) in &contents {
            match store.get_mut(surface(), *id) {
                Ok(tile) => tile.texels_mut().copy_from_slice(texels),
                Err(err) => unreachable!("seed: {err:?}"),
            }
            let _ = store.take_dirty(surface(), *id);
        }
    }
}

/// A random batch: clustered and scattered dabs, tile edges and corners,
/// negative and far-away centres, tiny dabs that reach no texel centre,
/// out-of-range colours, degenerate radii.
fn random_batch(rng: &mut Rng, len: usize) -> Vec<SoftDab> {
    let mut dabs = Vec::with_capacity(len);
    let cluster = (rng.range(-100.0, 1300.0), rng.range(-100.0, 1300.0));
    for _ in 0..len {
        let center = match rng.below(10) {
            0 => (rng.range(-700.0, 1400.0), rng.range(-700.0, 1400.0)),
            // Near a tile edge or corner.
            1 => (
                256.0 * rng.below(5) as f32 + rng.range(-3.0, 3.0),
                256.0 * rng.below(5) as f32 + rng.range(-3.0, 3.0),
            ),
            // Wholly outside the layer (negative), still reaching tile (0, 0)'s box.
            2 => (rng.range(-900.0, -200.0), rng.range(-900.0, 300.0)),
            // Past 2^22: the unlimited (whole-box) walk.
            3 if rng.below(4) == 0 => (rng.range(4_194_000.0, 4_195_000.0), rng.range(0.0, 600.0)),
            // Exactly on texel centres or corners.
            4 => (
                rng.below(800) as f32 + if rng.below(2) == 0 { 0.5 } else { 0.0 },
                rng.below(800) as f32 + if rng.below(2) == 0 { 0.5 } else { 0.0 },
            ),
            _ => (
                cluster.0 + rng.range(-60.0, 60.0),
                cluster.1 + rng.range(-60.0, 60.0),
            ),
        };
        let radius = match rng.below(12) {
            0 => rng.range(0.05, 0.6),
            1 => 0.0,
            2 => -1.0,
            3 => f32::NAN,
            4 => rng.range(60.0, 300.0),
            5 => rng.below(40) as f32 + 0.5,
            _ => rng.range(0.6, 40.0),
        };
        let colour = if rng.below(8) == 0 {
            [
                rng.range(-0.5, 1.5),
                rng.range(-0.5, 1.5),
                rng.range(-0.5, 1.5),
            ]
        } else {
            [rng.unit(), rng.unit(), rng.unit()]
        };
        let opacity = if rng.below(2) == 0 {
            1.0
        } else {
            rng.range(0.0, 1.0)
        };
        let flow = match rng.below(4) {
            0 => 1.0,
            1 => 0.01,
            _ => rng.unit(),
        };
        dabs.push(SoftDab {
            center,
            radius,
            colour,
            amount: PaintAmount::new(opacity, flow),
        });
    }
    // A tiny dab between texel centres: reaches no texel at all.
    dabs.push(SoftDab {
        center: (10.0, 10.0),
        radius: 0.4,
        colour: [1.0, 0.0, 0.0],
        amount: PaintAmount::FULL,
    });
    dabs
}

fn per_dab(
    store: &mut TileStore,
    dabs: &[SoftDab],
    mut snapshot: Option<&mut StrokeSnapshot>,
) -> BTreeSet<(u32, u32)> {
    let mut painted = BTreeSet::new();
    for d in dabs {
        let outcome = stamp_dab_amount(
            store,
            surface(),
            d.center,
            d.radius,
            d.colour,
            snapshot.as_deref_mut(),
            None,
            false,
            false,
            d.amount,
        );
        assert!(outcome.is_complete());
        painted.extend(outcome.painted().iter().map(|t| (t.y, t.x)));
    }
    painted
}

fn batch(
    store: &mut TileStore,
    dabs: &[SoftDab],
    snapshot: Option<&mut StrokeSnapshot>,
    mode: Mode,
) -> BTreeSet<(u32, u32)> {
    let outcome = deposit_soft_dabs_with(store, surface(), dabs, snapshot, mode);
    assert!(outcome.tiles.is_complete());
    assert_eq!(outcome.failed_dabs, 0);
    let painted: BTreeSet<(u32, u32)> =
        outcome.tiles.painted().iter().map(|t| (t.y, t.x)).collect();
    assert_eq!(
        painted.len(),
        outcome.tiles.painted().len(),
        "each tile once"
    );
    painted
}

fn held(store: &TileStore) -> BTreeSet<(u32, u32)> {
    store
        .held_tile_versions()
        .into_iter()
        .map(|((_, id), _)| (id.y, id.x))
        .collect()
}

fn bits(store: &mut TileStore, id: TileId) -> Vec<u16> {
    match store.get(surface(), id) {
        Ok(tile) => tile.texels().iter().map(|v| v.to_bits()).collect(),
        Err(err) => unreachable!("read {id:?}: {err:?}"),
    }
}

/// How dirty rectangles are compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Not at all (after an undo, which marks whole tiles).
    Skip,
    /// Exactly: no tile was evicted, so each rectangle is the union of the
    /// boxes of the dabs that changed the tile.
    Exact,
    /// Dirty or not only: an evicted dirty tile reports the whole tile, and
    /// which tiles the LRU evicts depends on the access order, which the
    /// two paths do not share.
    Presence,
}

/// Every held tile equal bit for bit, and the dirty rectangles compared as
/// `dirty` says (and consumed on both).
fn assert_stores_equal(a: &mut TileStore, b: &mut TileStore, dirty: Dirty, what: &str) {
    let held_a = held(a);
    assert_eq!(held_a, held(b), "{what}: the same tiles exist");
    for &(y, x) in &held_a {
        let id = TileId { x, y };
        if dirty != Dirty::Skip {
            let (da, db): (Option<Rect>, Option<Rect>) =
                (a.take_dirty(surface(), id), b.take_dirty(surface(), id));
            if dirty == Dirty::Exact {
                assert_eq!(da, db, "{what}: dirty rectangle of {id:?}");
            } else {
                assert_eq!(da.is_some(), db.is_some(), "{what}: dirty {id:?}");
            }
        }
        assert!(bits(a, id) == bits(b, id), "{what}: texels of {id:?}");
    }
}

fn assert_snapshots_equal(a: &mut StrokeSnapshot, b: &mut StrokeSnapshot, what: &str) {
    let ca: BTreeSet<(u32, u32)> = a.captured().map(|t| (t.y, t.x)).collect();
    let cb: BTreeSet<(u32, u32)> = b.captured().map(|t| (t.y, t.x)).collect();
    assert_eq!(ca, cb, "{what}: captured tiles");
    for &(y, x) in &ca {
        let id = TileId { x, y };
        let ra: Option<Vec<u16>> = a
            .raw_content(id)
            .map(|r| r.iter().map(|v| v.to_bits()).collect());
        let rb: Option<Vec<u16>> = b
            .raw_content(id)
            .map(|r| r.iter().map(|v| v.to_bits()).collect());
        assert!(ra == rb, "{what}: raw content of {id:?}");
    }
    let cover = |s: &mut StrokeSnapshot| -> HashMap<TileId, Vec<u32>> {
        s.cover_mut()
            .iter()
            .map(|(id, t)| (*id, t.iter().map(|v| v.to_bits()).collect()))
            .collect()
    };
    let (va, vb) = (cover(a), cover(b));
    assert_eq!(
        va.keys().collect::<BTreeSet<_>>(),
        vb.keys().collect::<BTreeSet<_>>(),
        "{what}: stroke coverage tiles"
    );
    for (id, values) in &va {
        assert!(
            Some(values) == vb.get(id),
            "{what}: stroke coverage of {id:?}"
        );
    }
}

/// One random scenario: `strokes` strokes of `batches` batches each, with a
/// stroke snapshot, then undo and redo of every stroke; `budget` small
/// forces paging to the scratch disk between dabs.
fn differential(seed: u64, budget: usize, strokes: usize, batches: usize, mode: Mode) {
    let mut rng = Rng(seed);
    let (_da, mut a) = store(budget);
    let (_db, mut b) = store(budget);
    seed_content(&mut rng, [&mut a, &mut b]);
    let (mut ha, mut hb) = (PixelHistory::new(), PixelHistory::new());
    for stroke in 0..strokes {
        let mut sa = StrokeSnapshot::new(surface());
        let mut sb = StrokeSnapshot::new(surface());
        for k in 0..batches {
            let len = 1 + rng.below(60) as usize;
            let dabs = random_batch(&mut rng, len);
            let pa = per_dab(&mut a, &dabs, Some(&mut sa));
            let pb = batch(&mut b, &dabs, Some(&mut sb), mode);
            let what = format!("{mode:?} seed {seed} stroke {stroke} batch {k}");
            assert_eq!(pa, pb, "{what}: painted tiles");
            let dirty = if budget >= 4096 {
                Dirty::Exact
            } else {
                Dirty::Presence
            };
            assert_stores_equal(&mut a, &mut b, dirty, &what);
            assert_snapshots_equal(&mut sa, &mut sb, &what);
        }
        assert_eq!(ha.push(sa), hb.push(sb), "seed {seed}: pushed alike");
    }
    for step in 0..strokes {
        let ua = ha.undo(&mut a);
        let ub = hb.undo(&mut b);
        assert!(
            matches!((&ua, &ub), (Ok(x), Ok(y)) if x == y),
            "undo {step}"
        );
        assert_stores_equal(
            &mut a,
            &mut b,
            Dirty::Skip,
            &format!("seed {seed} undo {step}"),
        );
    }
    for step in 0..strokes {
        let ra = ha.redo(&mut a);
        let rb = hb.redo(&mut b);
        assert!(
            matches!((&ra, &rb), (Ok(x), Ok(y)) if x == y),
            "redo {step}"
        );
        assert_stores_equal(
            &mut a,
            &mut b,
            Dirty::Skip,
            &format!("seed {seed} redo {step}"),
        );
    }
}

#[test]
fn batch_deposit_equals_per_dab_stamping_bit_for_bit_with_a_stroke_snapshot() {
    for mode in [Mode::Serial, Mode::Parallel, Mode::Auto] {
        for seed in 0..16_u64 {
            differential(0xD3_0517 + seed, 4096, 2, 4, mode);
        }
    }
}

#[test]
fn batch_deposit_equals_per_dab_stamping_when_tiles_page_to_disk() {
    // Budget 3: tiles evict and page back in between dabs and batches.
    for mode in [Mode::Serial, Mode::Parallel] {
        for seed in 0..6_u64 {
            differential(0xFA_6E00 + seed, 3, 2, 3, mode);
        }
    }
}

#[test]
fn batch_deposit_equals_per_dab_stamping_with_no_snapshot() {
    // No snapshot: every dab is its own stroke (start = the live texel,
    // coverage from 0), so the order of dabs within a texel matters most.
    for (seed, mode) in (0..24_u64).map(|s| (s, [Mode::Serial, Mode::Parallel][(s % 2) as usize])) {
        let mut rng = Rng(0x0005_7A9E + seed);
        let (_da, mut a) = store(4096);
        let (_db, mut b) = store(4096);
        seed_content(&mut rng, [&mut a, &mut b]);
        for k in 0..4 {
            let len = 1 + rng.below(60) as usize;
            let dabs = random_batch(&mut rng, len);
            let pa = per_dab(&mut a, &dabs, None);
            let pb = batch(&mut b, &dabs, None, mode);
            let what = format!("{mode:?} seed {seed} batch {k}");
            assert_eq!(pa, pb, "{what}: painted tiles");
            assert_stores_equal(&mut a, &mut b, Dirty::Exact, &what);
        }
    }
}

#[test]
fn heavily_overlapping_dabs_keep_their_order() {
    for mode in [Mode::Serial, Mode::Parallel] {
        overlapping(mode);
    }
}

fn overlapping(mode: Mode) {
    // Many dabs on the same few texels with different colours: any reorder
    // within a texel changes the stored colour.
    let (_da, mut a) = store(64);
    let (_db, mut b) = store(64);
    let mut rng = Rng(7);
    let dabs: Vec<SoftDab> = (0..400)
        .map(|i| SoftDab {
            center: (255.5 + rng.range(-2.0, 2.0), 255.5 + rng.range(-2.0, 2.0)),
            radius: 3.0 + (i % 5) as f32,
            colour: [rng.unit(), rng.unit(), rng.unit()],
            amount: PaintAmount::new(rng.range(0.3, 1.0), rng.range(0.05, 1.0)),
        })
        .collect();
    let mut sa = StrokeSnapshot::new(surface());
    let mut sb = StrokeSnapshot::new(surface());
    for chunk in dabs.chunks(37) {
        let pa = per_dab(&mut a, chunk, Some(&mut sa));
        let pb = batch(&mut b, chunk, Some(&mut sb), mode);
        assert_eq!(pa, pb);
        assert_stores_equal(&mut a, &mut b, Dirty::Exact, "overlap");
        assert_snapshots_equal(&mut sa, &mut sb, "overlap");
    }
    // And the reversed order gives a different picture (the test can see order).
    let (_dc, mut c) = store(64);
    let mut sc = StrokeSnapshot::new(surface());
    let reversed: Vec<SoftDab> = dabs.iter().rev().copied().collect();
    let _ = batch(&mut c, &reversed, Some(&mut sc), mode);
    assert!(
        bits(&mut a, TileId { x: 0, y: 0 }) != bits(&mut c, TileId { x: 0, y: 0 }),
        "dab order must matter for this fixture"
    );
}

#[test]
fn an_empty_batch_and_dabs_that_reach_nothing_change_nothing() {
    let (_d, mut s) = store(16);
    let mut snap = StrokeSnapshot::new(surface());
    let out = deposit_soft_dabs(&mut s, surface(), &[], Some(&mut snap));
    assert!(out.tiles.painted().is_empty() && out.tiles.is_complete());
    assert!(held(&s).is_empty() && snap.is_empty());
    // Degenerate radii touch no tile at all.
    let degenerate = [0.0, -2.0, f32::NAN, f32::INFINITY].map(|radius| SoftDab {
        center: (100.0, 100.0),
        radius,
        colour: [1.0, 1.0, 1.0],
        amount: PaintAmount::FULL,
    });
    let out = deposit_soft_dabs(&mut s, surface(), &degenerate, Some(&mut snap));
    assert!(out.tiles.painted().is_empty());
    assert!(held(&s).is_empty() && snap.is_empty());
    assert_eq!(snap.stroke_coverage_tiles(), 0);
}

#[test]
fn a_snapshot_for_another_surface_is_ignored_like_the_per_dab_path() {
    let (_da, mut a) = store(16);
    let (_db, mut b) = store(16);
    let other = SurfaceId::from_raw(9);
    let mut sa = StrokeSnapshot::new(other);
    let mut sb = StrokeSnapshot::new(other);
    let dabs = [SoftDab {
        center: (40.0, 40.0),
        radius: 10.0,
        colour: [0.2, 0.4, 0.6],
        amount: PaintAmount::new(0.7, 0.5),
    }; 3];
    let pa = per_dab(&mut a, &dabs, Some(&mut sa));
    let pb = batch(&mut b, &dabs, Some(&mut sb), Mode::Parallel);
    assert_eq!(pa, pb);
    assert!(sa.is_empty() && sb.is_empty());
    assert_stores_equal(&mut a, &mut b, Dirty::Exact, "foreign snapshot");
}

#[test]
fn the_four_channel_conversions_match_one_at_a_time() {
    use half::slice::HalfFloatSliceExt;
    // f16 -> f32: every bit pattern, four at a time.
    for base in (0..=u16::MAX).step_by(4) {
        let h: Vec<f16> = (0..4)
            .map(|k| f16::from_bits(base.wrapping_add(k)))
            .collect();
        let mut v = [0.0_f32; 4];
        h.convert_to_f32_slice(&mut v);
        for (x, y) in h.iter().zip(v) {
            assert_eq!(x.to_f32().to_bits(), y.to_bits(), "{:#06x}", x.to_bits());
        }
    }
    // f32 -> f16: every f16 value, the f32 midpoints between neighbours
    // (ties to even) and one ulp either side, specials, and random floats.
    let mut rng = Rng(0xF16C);
    let mut probe: Vec<f32> = vec![
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
        f32::from_bits(0x7FC0_1234),
        f32::from_bits(0x7F80_0001),
        f32::MAX,
        f32::MIN_POSITIVE,
        1.0e-10,
        65_520.0,
        65_519.996,
    ];
    for bits in 0..=u16::MAX {
        let x = f16::from_bits(bits).to_f32();
        let next = f16::from_bits(bits.wrapping_add(1)).to_f32();
        if x.is_finite() && next.is_finite() {
            let mid = f64::midpoint(f64::from(x), f64::from(next));
            let mid = mid as f32;
            probe.extend([
                x,
                mid,
                f32::from_bits(mid.to_bits().wrapping_add(1)),
                f32::from_bits(mid.to_bits().wrapping_sub(1)),
            ]);
        }
    }
    for _ in 0..200_000 {
        probe.push(f32::from_bits(rng.next() as u32));
    }
    for chunk in probe.chunks_exact(4) {
        let mut h = [f16::ZERO; 4];
        h.convert_from_f32_slice(chunk);
        for (x, y) in chunk.iter().zip(h) {
            assert_eq!(
                f16::from_f32(*x).to_bits(),
                y.to_bits(),
                "{:#010x}",
                x.to_bits()
            );
        }
    }
}
