//! Flow and Opacity (0.228.0) at the brush level.
//!
//! The model under test is written out in `crate::amount`. The tests
//! compare the engine, texel for texel and bit for bit, with an
//! **independent** model of it (a sparse map of texels, per-texel formulas
//! transcribed from that description, no tiles), over random dab sequences;
//! they pin Opacity 100 % / Flow 100 % to a hash taken from the 0.227.0 code
//! itself; and they check the properties that make the settings what they
//! say (a stroke crossing itself never passes the opacity cap, Flow scales
//! the dab, a new stroke starts from the content it finds, no phantom tile,
//! one undo restores everything).

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::many_single_char_names,
    clippy::assertions_on_constants,
    clippy::too_many_lines
)]

use std::collections::HashMap;
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, MIN_AMOUNT, PaintAmount, PixelHistory, StrokeSnapshot, clone_dab_amount,
    clone_dab_covered, hard_dab_snap, stamp_dab, stamp_dab_amount, stamp_dab_covered,
    stamp_dab_hard, stamp_dab_hard_preserving_alpha_covered, stamp_dab_preserving_alpha,
};

type Tx = [f16; CHANNELS];

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

fn h(v: f32) -> f16 {
    f16::from_f32(v)
}

fn px(r: f32, g: f32, b: f32, a: f32) -> Tx {
    [h(r), h(g), h(b), h(a)]
}

fn bits(t: Tx) -> [u16; 4] {
    [
        t[0].to_bits(),
        t[1].to_bits(),
        t[2].to_bits(),
        t[3].to_bits(),
    ]
}

fn texel(store: &mut TileStore, x: u32, y: u32) -> Tx {
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

fn put(store: &mut TileStore, x: u32, y: u32, value: Tx) {
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

/// A deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A selection with fractional values, defined by a formula.
struct Wavy;

impl Coverage for Wavy {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, x: i64, y: i64) -> f32 {
        match (x / 5 + y / 7).rem_euclid(4) {
            0 => 0.0,
            1 => 0.5,
            2 => 0.25,
            _ => 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// The independent model.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Kind {
    lock: bool,
    hard: bool,
}

/// Sparse texels; absent is transparent black. `began` is the stroke-start
/// copy the opacity cap reads.
#[derive(Clone, Default)]
struct Model {
    now: HashMap<(u32, u32), Tx>,
    began: HashMap<(u32, u32), Tx>,
}

fn at(map: &HashMap<(u32, u32), Tx>, x: u32, y: u32) -> Tx {
    map.get(&(x, y)).copied().unwrap_or([f16::ZERO; CHANNELS])
}

/// The Brush dab's alpha at a texel, from its definition.
fn soft_alpha(x: u32, y: u32, center: (f32, f32), radius: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let d = (px - center.0).hypot(py - center.1);
    if d > radius {
        return 0.0;
    }
    (1.0 - (d / radius)).clamp(0.0, 1.0).powf(1.5)
}

/// The Pencil dab: a texel centre strictly inside the circle.
fn hard_alpha(x: u32, y: u32, center: (f32, f32), radius: f32) -> f32 {
    let (dx, dy) = (x as f32 + 0.5 - center.0, y as f32 + 0.5 - center.1);
    if dx * dx + dy * dy < radius * radius {
        1.0
    } else {
        0.0
    }
}

impl Model {
    fn begin_stroke(&mut self) {
        self.began = self.now.clone();
    }

    fn dab(
        &mut self,
        center: (f32, f32),
        radius: f32,
        colour: [f32; 3],
        kind: Kind,
        opacity: f32,
        flow: f32,
        cover: Option<&dyn Coverage>,
    ) {
        let center = if kind.hard {
            hard_dab_snap(center)
        } else {
            center
        };
        let flow = if kind.hard { 1.0 } else { flow };
        let (x0, y0) = (
            (center.0 - radius).floor().max(0.0) as u32,
            (center.1 - radius).floor().max(0.0) as u32,
        );
        let (x1, y1) = (
            (center.0 + radius).ceil().max(0.0) as u32,
            (center.1 + radius).ceil().max(0.0) as u32,
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let a = if kind.hard {
                    hard_alpha(x, y, center, radius)
                } else {
                    soft_alpha(x, y, center, radius)
                };
                if a <= 0.0 {
                    continue;
                }
                let a = a * flow;
                let c = cover.map_or(1.0, |cov| cov.at(i64::from(x), i64::from(y)));
                if c <= 0.0 {
                    continue;
                }
                let cur = at(&self.now, x, y);
                let s = at(&self.began, x, y);
                let capped = opacity < 1.0;
                let f = |t: Tx, i: usize| t[i].to_f32();
                let fresh: Tx = if kind.lock {
                    let alpha = f(cur, 3);
                    if alpha <= 0.0 {
                        continue;
                    }
                    let one = |i: usize| {
                        if capped {
                            let limit = f(s, i) + (colour[i] * alpha - f(s, i)) * opacity;
                            h(f(cur, i) * (1.0 - a) + limit * a)
                        } else {
                            h(f(cur, i) * (1.0 - a) + colour[i] * alpha * a)
                        }
                    };
                    [one(0), one(1), one(2), cur[3]]
                } else if capped {
                    let painted = [colour[0] * a, colour[1] * a, colour[2] * a, a];
                    let lim = |i: usize| h(f(s, i) + (painted[i] - f(s, i)) * opacity);
                    let t = [lim(0), lim(1), lim(2), lim(3)];
                    if !kind.hard && f(t, 3) <= f(cur, 3) {
                        continue;
                    }
                    t
                } else {
                    if !kind.hard && a <= f(cur, 3) {
                        continue;
                    }
                    [h(colour[0] * a), h(colour[1] * a), h(colour[2] * a), h(a)]
                };
                let out = if c < 1.0 {
                    let mut o = fresh;
                    for i in 0..4 {
                        if cur[i].to_bits() != fresh[i].to_bits() {
                            o[i] = h(f(cur, i) + (f(fresh, i) - f(cur, i)) * c);
                        }
                    }
                    o
                } else {
                    fresh
                };
                self.now.insert((x, y), out);
            }
        }
    }

    /// The Clone Stamp dab: source texels from the stroke-start map.
    fn clone_dab(
        &mut self,
        center: (f32, f32),
        radius: f32,
        offset: (i64, i64),
        lock: bool,
        opacity: f32,
        flow: f32,
        cover: Option<&dyn Coverage>,
    ) {
        let (x0, y0) = (
            (center.0 - radius).floor().max(0.0) as u32,
            (center.1 - radius).floor().max(0.0) as u32,
        );
        let (x1, y1) = (
            (center.0 + radius).ceil().max(0.0) as u32,
            (center.1 + radius).ceil().max(0.0) as u32,
        );
        let patch = self.began.clone();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let a = soft_alpha(x, y, center, radius);
                if a <= 0.0 {
                    continue;
                }
                let a = a * flow;
                let c = cover.map_or(1.0, |cov| cov.at(i64::from(x), i64::from(y)));
                if c <= 0.0 {
                    continue;
                }
                let (sx, sy) = (i64::from(x) + offset.0, i64::from(y) + offset.1);
                let src = if sx < 0 || sy < 0 {
                    [f16::ZERO; CHANNELS]
                } else {
                    at(&patch, sx as u32, sy as u32)
                };
                let cur = at(&self.now, x, y);
                let s = at(&self.began, x, y);
                let f = |t: Tx, i: usize| t[i].to_f32();
                let lim = |i: usize, target: f32| {
                    if opacity < 1.0 {
                        f(s, i) + (target - f(s, i)) * opacity
                    } else {
                        target
                    }
                };
                let fresh: Tx = if lock {
                    let (da, sa) = (f(cur, 3), f(src, 3));
                    if da <= 0.0 || sa <= 0.0 {
                        continue;
                    }
                    let one = |i: usize| {
                        let target = lim(i, f(src, i) / sa * da);
                        h(f(cur, i) + (target - f(cur, i)) * a)
                    };
                    [one(0), one(1), one(2), cur[3]]
                } else if a >= 1.0 && opacity >= 1.0 {
                    src
                } else {
                    let one = |i: usize| {
                        let t = lim(i, f(src, i));
                        if a >= 1.0 {
                            h(t)
                        } else {
                            h(f(cur, i) + (t - f(cur, i)) * a)
                        }
                    };
                    [one(0), one(1), one(2), one(3)]
                };
                let out = if c < 1.0 {
                    let mut o = fresh;
                    for i in 0..4 {
                        if cur[i].to_bits() != fresh[i].to_bits() {
                            o[i] = h(f(cur, i) + (f(fresh, i) - f(cur, i)) * c);
                        }
                    }
                    o
                } else {
                    fresh
                };
                self.now.insert((x, y), out);
            }
        }
    }
}

const SIDE: u32 = 2 * TILE;

/// Every texel of the first 2 x 2 tiles, engine against model.
fn assert_matches(store: &mut TileStore, model: &Model, why: &str) {
    for y in 0..SIDE {
        for x in 0..SIDE {
            let got = texel(store, x, y);
            let want = at(&model.now, x, y);
            assert_eq!(bits(got), bits(want), "{why}: texel ({x}, {y})");
        }
    }
}

fn seed_content(store: &mut TileStore, model: &mut Model, rng: &mut Rng) {
    // Some translucent paint to start from, over the dab area.
    for _ in 0..40 {
        let (x, y) = (
            60 + rng.below(u64::from(SIDE) - 120) as u32,
            60 + rng.below(u64::from(SIDE) - 120) as u32,
        );
        let a = rng.range(0.1, 0.9);
        let t = px(rng.unit() * a, rng.unit() * a, rng.unit() * a, a);
        for dy in 0..9 {
            for dx in 0..9 {
                put(store, x + dx, y + dy, t);
                model.now.insert((x + dx, y + dy), t);
            }
        }
    }
}

fn random_kind(rng: &mut Rng) -> Kind {
    Kind {
        lock: rng.below(3) == 0,
        hard: rng.below(3) == 0,
    }
}

fn engine_dab(
    store: &mut TileStore,
    snap: &mut StrokeSnapshot,
    center: (f32, f32),
    radius: f32,
    colour: [f32; 3],
    kind: Kind,
    amount: PaintAmount,
    cover: Option<&dyn Coverage>,
) {
    let outcome = stamp_dab_amount(
        store,
        surface(),
        center,
        radius,
        colour,
        Some(snap),
        cover,
        kind.lock,
        kind.hard,
        amount,
    );
    assert!(outcome.is_complete());
}

// ---------------------------------------------------------------------------
// Differential tests.

#[test]
fn random_strokes_match_the_independent_model_bit_for_bit() {
    for (seed, covered) in [(1_u64, false), (2, true), (3, false), (4, true)] {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
        let (_dir, mut s) = store();
        let mut model = Model::default();
        seed_content(&mut s, &mut model, &mut rng);
        for stroke in 0..4 {
            model.begin_stroke();
            let mut snap = StrokeSnapshot::new(surface());
            let amount = match stroke {
                0 => PaintAmount::new(0.5, 1.0),
                1 => PaintAmount::new(0.3, 0.6),
                2 => PaintAmount::new(1.0, 0.4),
                _ => PaintAmount::new(0.8, 0.9),
            };
            let colour = [rng.unit(), rng.unit(), rng.unit()];
            // One kind per stroke, as a tool has.
            let kind = random_kind(&mut rng);
            let mut cx = rng.range(80.0, 430.0);
            let mut cy = rng.range(80.0, 430.0);
            for _ in 0..25 {
                cx = (cx + rng.range(-9.0, 9.0)).clamp(60.0, 450.0);
                cy = (cy + rng.range(-9.0, 9.0)).clamp(60.0, 450.0);
                let radius = rng.range(2.0, 28.0);
                let cov: Option<&dyn Coverage> = if covered { Some(&Wavy) } else { None };
                engine_dab(
                    &mut s,
                    &mut snap,
                    (cx, cy),
                    radius,
                    colour,
                    kind,
                    amount,
                    cov,
                );
                model.dab(
                    (cx, cy),
                    radius,
                    colour,
                    kind,
                    amount.opacity(),
                    amount.flow(),
                    cov,
                );
            }
            assert_matches(
                &mut s,
                &model,
                &format!("seed {seed} stroke {stroke} {kind:?}"),
            );
        }
    }
}

#[test]
fn random_clone_strokes_match_the_independent_model_bit_for_bit() {
    for (seed, covered, lock) in [(11_u64, false, false), (12, true, false), (13, false, true)] {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03 ^ seed);
        let (_dir, mut s) = store();
        let mut model = Model::default();
        // A source area of distinct opaque and translucent texels.
        for y in 40..200_u32 {
            for x in 40..200_u32 {
                let a = if (x / 8 + y / 8) % 3 == 0 { 0.5 } else { 1.0 };
                let t = px(
                    (x % 16) as f32 / 40.0 * a,
                    (y % 16) as f32 / 40.0 * a,
                    ((x + y) % 7) as f32 / 12.0 * a,
                    a,
                );
                put(&mut s, x, y, t);
                model.now.insert((x, y), t);
            }
        }
        for stroke in 0..4 {
            model.begin_stroke();
            let mut snap = StrokeSnapshot::new(surface());
            let amount = match stroke {
                0 => PaintAmount::new(0.5, 1.0),
                1 => PaintAmount::new(0.35, 0.5),
                2 => PaintAmount::new(1.0, 0.5),
                _ => PaintAmount::FULL,
            };
            let offset = (
                rng.range(-5.0, 120.0) as i64 + 40,
                rng.range(-5.0, 120.0) as i64,
            );
            let mut cx = rng.range(60.0, 140.0);
            let mut cy = rng.range(60.0, 140.0);
            for _ in 0..20 {
                cx = (cx + rng.range(-6.0, 6.0)).clamp(40.0, 220.0);
                cy = (cy + rng.range(-6.0, 6.0)).clamp(40.0, 220.0);
                let radius = rng.range(2.0, 20.0);
                let cov: Option<&dyn Coverage> = if covered { Some(&Wavy) } else { None };
                let outcome = clone_dab_amount(
                    &mut s,
                    surface(),
                    (cx, cy),
                    radius,
                    offset,
                    lock,
                    Some(&mut snap),
                    cov,
                    amount,
                );
                assert!(outcome.is_complete());
                model.clone_dab(
                    (cx, cy),
                    radius,
                    offset,
                    lock,
                    amount.opacity(),
                    amount.flow(),
                    cov,
                );
            }
            assert_matches(
                &mut s,
                &model,
                &format!("clone seed {seed} stroke {stroke}"),
            );
        }
    }
}

/// FNV-1a over every texel's bits of the first 2 x 2 tiles.
fn region_hash(store: &mut TileStore) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325_u64;
    for y in 0..SIDE {
        for x in 0..SIDE {
            for v in texel(store, x, y) {
                hash ^= u64::from(v.to_bits());
                hash = hash.wrapping_mul(0x0100_0000_01B3);
            }
        }
    }
    hash
}

/// The legacy entry points on a fixed random sequence; this exact sequence
/// was hashed with the 0.227.0 code (before this round) to get the constants
/// below, so Opacity 100 % / Flow 100 % is pinned to what shipped.
#[cfg(target_os = "linux")]
fn legacy_hash() -> (u64, u64) {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let (_dir, mut s) = store();
    let mut model = Model::default();
    seed_content(&mut s, &mut model, &mut rng);
    let mut snap = StrokeSnapshot::new(surface());
    for i in 0..400_u32 {
        let center = (rng.range(40.0, 470.0), rng.range(40.0, 470.0));
        let radius = rng.range(1.0, 30.0);
        let colour = [rng.unit(), rng.unit(), rng.unit()];
        let cover: Option<&dyn Coverage> = if i % 2 == 0 { Some(&Wavy) } else { None };
        let outcome = match rng.below(5) {
            0 => stamp_dab(&mut s, surface(), center, radius, colour, Some(&mut snap)),
            1 => stamp_dab_covered(
                &mut s,
                surface(),
                center,
                radius,
                colour,
                Some(&mut snap),
                cover,
            ),
            2 => stamp_dab_preserving_alpha(
                &mut s,
                surface(),
                center,
                radius,
                colour,
                Some(&mut snap),
            ),
            3 => stamp_dab_hard(&mut s, surface(), center, radius, colour, Some(&mut snap)),
            _ => stamp_dab_hard_preserving_alpha_covered(
                &mut s,
                surface(),
                center,
                radius,
                colour,
                Some(&mut snap),
                cover,
            ),
        };
        assert!(outcome.is_complete());
    }
    let brush = region_hash(&mut s);
    // Clone dabs over the result.
    let mut snap = StrokeSnapshot::new(surface());
    for i in 0..150_u32 {
        let center = (rng.range(40.0, 470.0), rng.range(40.0, 470.0));
        let radius = rng.range(1.0, 25.0);
        let offset = (rng.range(-90.0, 90.0) as i64, rng.range(-90.0, 90.0) as i64);
        let cover: Option<&dyn Coverage> = if i % 3 == 0 { Some(&Wavy) } else { None };
        let outcome = clone_dab_covered(
            &mut s,
            surface(),
            center,
            radius,
            offset,
            i % 4 == 0,
            Some(&mut snap),
            cover,
        );
        assert!(outcome.is_complete());
    }
    (brush, region_hash(&mut s))
}

/// The hashes the 0.227.0 code produced for [`legacy_hash`] on Linux. The
/// falloff uses `powf`, whose last bit differs between platform math
/// libraries (macOS failed this pin), so the pin is Linux-only; the
/// platform-independent guard is the differential test below.
#[cfg(target_os = "linux")]
const LEGACY_BRUSH_HASH: u64 = 0x236b_8639_623e_9433;
#[cfg(target_os = "linux")]
const LEGACY_CLONE_HASH: u64 = 0xb72b_4bd9_a93c_3dd6;

#[cfg(target_os = "linux")]
#[test]
fn the_legacy_entry_points_are_bit_identical_to_0_227_0() {
    let (brush, clone) = legacy_hash();
    assert_eq!(brush, LEGACY_BRUSH_HASH, "Brush/Pencil dabs changed");
    assert_eq!(clone, LEGACY_CLONE_HASH, "Clone Stamp dabs changed");
}

#[test]
fn explicit_full_amounts_equal_the_legacy_entry_points() {
    // Same random sequence through `stamp_dab_amount(.., FULL)` and through
    // `PaintAmount::new(1.0, 1.0)`: identical to the legacy functions.
    let run = |amount: Option<PaintAmount>| {
        let mut rng = Rng(0xABCD_EF01_2345_6789);
        let (_dir, mut s) = store();
        let mut model = Model::default();
        seed_content(&mut s, &mut model, &mut rng);
        let mut snap = StrokeSnapshot::new(surface());
        for _ in 0..200 {
            let center = (rng.range(40.0, 470.0), rng.range(40.0, 470.0));
            let radius = rng.range(1.0, 30.0);
            let colour = [rng.unit(), rng.unit(), rng.unit()];
            let kind = random_kind(&mut rng);
            let cover: Option<&dyn Coverage> = Some(&Wavy);
            let outcome = if let Some(amount) = amount {
                stamp_dab_amount(
                    &mut s,
                    surface(),
                    center,
                    radius,
                    colour,
                    Some(&mut snap),
                    cover,
                    kind.lock,
                    kind.hard,
                    amount,
                )
            } else {
                match (kind.lock, kind.hard) {
                    (false, false) => stamp_dab_covered(
                        &mut s,
                        surface(),
                        center,
                        radius,
                        colour,
                        Some(&mut snap),
                        cover,
                    ),
                    (true, false) => crate::stamp_dab_preserving_alpha_covered(
                        &mut s,
                        surface(),
                        center,
                        radius,
                        colour,
                        Some(&mut snap),
                        cover,
                    ),
                    (false, true) => crate::stamp_dab_hard_covered(
                        &mut s,
                        surface(),
                        center,
                        radius,
                        colour,
                        Some(&mut snap),
                        cover,
                    ),
                    (true, true) => stamp_dab_hard_preserving_alpha_covered(
                        &mut s,
                        surface(),
                        center,
                        radius,
                        colour,
                        Some(&mut snap),
                        cover,
                    ),
                }
            };
            assert!(outcome.is_complete());
        }
        region_hash(&mut s)
    };
    let legacy = run(None);
    assert_eq!(run(Some(PaintAmount::FULL)), legacy);
    assert_eq!(run(Some(PaintAmount::new(1.0, 1.0))), legacy);
    assert_eq!(run(Some(PaintAmount::new(7.0, f32::NAN))), legacy);
}

// ---------------------------------------------------------------------------
// What the settings mean.

const COLOUR: [f32; 3] = [0.8, 0.4, 0.2];

fn dab_at(
    s: &mut TileStore,
    snap: Option<&mut StrokeSnapshot>,
    center: (f32, f32),
    radius: f32,
    amount: PaintAmount,
) {
    let outcome = stamp_dab_amount(
        s,
        surface(),
        center,
        radius,
        COLOUR,
        snap,
        None,
        false,
        false,
        amount,
    );
    assert!(outcome.is_complete());
}

#[test]
fn the_amount_is_clamped_and_a_non_finite_value_reads_as_full() {
    let a = PaintAmount::new(0.0, -3.0);
    assert_eq!((a.opacity(), a.flow()), (MIN_AMOUNT, MIN_AMOUNT));
    let b = PaintAmount::new(2.0, f32::INFINITY);
    assert_eq!((b.opacity(), b.flow()), (1.0, 1.0));
    let c = PaintAmount::new(f32::NAN, 0.25);
    assert_eq!((c.opacity(), c.flow()), (1.0, 0.25));
    assert_eq!(PaintAmount::default(), PaintAmount::FULL);
    assert!(MIN_AMOUNT > 0.0 && MIN_AMOUNT <= 0.01);
}

#[test]
fn flow_scales_the_dab_alpha() {
    let (_dir, mut s) = store();
    dab_at(
        &mut s,
        None,
        (100.5, 100.5),
        20.0,
        PaintAmount::new(1.0, 0.5),
    );
    // Exactly the dab at flow 100 % with half the alpha: alpha = a * 0.5.
    let t = texel(&mut s, 100, 100);
    let d = 0.0_f32.hypot(0.0);
    let a = (1.0 - d / 20.0_f32).clamp(0.0, 1.0).powf(1.5) * 0.5;
    assert_eq!(
        bits(t),
        bits(px(COLOUR[0] * a, COLOUR[1] * a, COLOUR[2] * a, a))
    );
    // A texel at a quarter of the radius is scaled the same way.
    let (_dir2, mut full) = store();
    dab_at(&mut full, None, (100.5, 100.5), 20.0, PaintAmount::FULL);
    for x in 90..111 {
        let half_flow = texel(&mut s, x, 100)[3].to_f32();
        let full_flow = texel(&mut full, x, 100)[3].to_f32();
        assert!((half_flow - full_flow * 0.5).abs() <= 1.0e-3, "x {x}");
    }
}

#[test]
fn flow_is_a_ceiling_on_the_soft_brushs_alpha_not_a_build_up() {
    // The max-alpha rule does not accumulate (documented): a hundred dabs at
    // flow 50 % leave the centre at one dab's alpha.
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..100 {
        dab_at(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            20.0,
            PaintAmount::new(1.0, 0.5),
        );
    }
    assert_eq!(texel(&mut s, 100, 100)[3].to_bits(), h(0.5).to_bits());
}

#[test]
fn opacity_limits_a_stroke_to_that_fraction_of_the_way_from_its_start() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    dab_at(
        &mut s,
        Some(&mut snap),
        (100.5, 100.5),
        20.0,
        PaintAmount::new(0.5, 1.0),
    );
    // Blank start: painted (colour, 1) * 0.5.
    assert_eq!(
        bits(texel(&mut s, 100, 100)),
        bits(px(COLOUR[0] * 0.5, COLOUR[1] * 0.5, COLOUR[2] * 0.5, 0.5))
    );
}

#[test]
fn a_stroke_crossing_itself_never_passes_the_cap() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    let amount = PaintAmount::new(0.4, 1.0);
    // Back and forth over the same ground, many times.
    for pass in 0..6 {
        for i in 0..60 {
            let x = if pass % 2 == 0 {
                80.5 + i as f32
            } else {
                139.5 - i as f32
            };
            dab_at(&mut s, Some(&mut snap), (x, 100.5), 12.0, amount);
        }
    }
    let mut max_alpha = 0.0_f32;
    for y in 80..120 {
        for x in 60..160 {
            max_alpha = max_alpha.max(texel(&mut s, x, y)[3].to_f32());
        }
    }
    assert_eq!(h(0.4).to_bits(), h(max_alpha).to_bits(), "the cap, no more");
    // The same strokes at 100 % reach 1.
    let (_dir2, mut full) = store();
    let mut snap2 = StrokeSnapshot::new(surface());
    for i in 0..60 {
        dab_at(
            &mut full,
            Some(&mut snap2),
            (80.5 + i as f32, 100.5),
            12.0,
            PaintAmount::FULL,
        );
    }
    assert_eq!(texel(&mut full, 100, 100)[3].to_bits(), h(1.0).to_bits());
}

#[test]
fn a_new_stroke_starts_from_the_content_it_finds() {
    let (_dir, mut s) = store();
    let amount = PaintAmount::new(0.5, 1.0);
    let mut first = StrokeSnapshot::new(surface());
    dab_at(&mut s, Some(&mut first), (100.5, 100.5), 20.0, amount);
    dab_at(&mut s, Some(&mut first), (100.5, 100.5), 20.0, amount);
    assert_eq!(texel(&mut s, 100, 100)[3].to_bits(), h(0.5).to_bits());
    // A second stroke: 0.5 + (1 - 0.5) * 0.5.
    let mut second = StrokeSnapshot::new(surface());
    dab_at(&mut s, Some(&mut second), (100.5, 100.5), 20.0, amount);
    assert_eq!(texel(&mut s, 100, 100)[3].to_bits(), h(0.75).to_bits());
}

#[test]
fn with_no_snapshot_every_dab_is_its_own_stroke() {
    let (_dir, mut s) = store();
    let amount = PaintAmount::new(0.5, 1.0);
    dab_at(&mut s, None, (100.5, 100.5), 20.0, amount);
    dab_at(&mut s, None, (100.5, 100.5), 20.0, amount);
    assert_eq!(texel(&mut s, 100, 100)[3].to_bits(), h(0.75).to_bits());
}

#[test]
fn a_dab_never_lowers_a_texels_alpha_under_the_cap() {
    let (_dir, mut s) = store();
    // Existing paint, more opaque than a faint dab would make it.
    let existing = px(0.5, 0.25, 0.125, 0.5);
    put(&mut s, 100, 100, existing);
    let mut snap = StrokeSnapshot::new(surface());
    dab_at(
        &mut s,
        Some(&mut snap),
        (100.5, 100.5),
        20.0,
        PaintAmount::new(0.5, 0.3),
    );
    assert_eq!(bits(texel(&mut s, 100, 100)), bits(existing));
}

#[test]
fn the_transparency_lock_keeps_alpha_and_approaches_the_limited_colour() {
    let (_dir, mut s) = store();
    let start = px(0.25, 0.25, 0.25, 0.5);
    for y in 90..111 {
        for x in 90..111 {
            put(&mut s, x, y, start);
        }
    }
    let mut snap = StrokeSnapshot::new(surface());
    let amount = PaintAmount::new(0.5, 1.0);
    for _ in 0..30 {
        let outcome = stamp_dab_amount(
            &mut s,
            surface(),
            (100.5, 100.5),
            15.0,
            COLOUR,
            Some(&mut snap),
            None,
            true,
            false,
            amount,
        );
        assert!(outcome.is_complete());
    }
    let t = texel(&mut s, 100, 100);
    assert_eq!(t[3].to_bits(), h(0.5).to_bits(), "alpha kept bit for bit");
    // The centre dab weight is 1: stored = start + (colour * alpha - start) * 0.5.
    for (i, c) in COLOUR.iter().enumerate() {
        let want = 0.25 + (c * 0.5 - 0.25) * 0.5;
        assert_eq!(t[i].to_bits(), h(want).to_bits(), "channel {i}");
    }
}

#[test]
fn the_pencil_ignores_flow_and_honours_opacity() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..3 {
        let o = stamp_dab_amount(
            &mut s,
            surface(),
            (50.5, 50.5),
            3.0,
            COLOUR,
            Some(&mut snap),
            None,
            false,
            true,
            PaintAmount::new(0.5, 0.1),
        );
        assert!(o.is_complete());
    }
    // Opaque pencil colour at half opacity over nothing: half, flow unused.
    assert_eq!(
        bits(texel(&mut s, 50, 50)),
        bits(px(COLOUR[0] * 0.5, COLOUR[1] * 0.5, COLOUR[2] * 0.5, 0.5))
    );
    // And a Pencil stroke over existing opaque paint blends toward its colour
    // by the opacity, once, however often it crosses.
    let (_d2, mut s2) = store();
    put(&mut s2, 50, 50, px(0.0, 0.0, 0.0, 1.0));
    let mut snap2 = StrokeSnapshot::new(surface());
    for _ in 0..5 {
        let _ = stamp_dab_amount(
            &mut s2,
            surface(),
            (50.5, 50.5),
            3.0,
            COLOUR,
            Some(&mut snap2),
            None,
            false,
            true,
            PaintAmount::new(0.25, 1.0),
        );
    }
    assert_eq!(
        bits(texel(&mut s2, 50, 50)),
        bits(px(
            COLOUR[0] * 0.25,
            COLOUR[1] * 0.25,
            COLOUR[2] * 0.25,
            1.0
        ))
    );
}

#[test]
fn a_capped_stroke_is_one_undo_step_that_restores_the_start() {
    let (_dir, mut s) = store();
    let existing = px(0.125, 0.25, 0.25, 0.5);
    put(&mut s, 255, 100, existing);
    put(&mut s, 256, 100, existing);
    let mut snap = StrokeSnapshot::new(surface());
    // A dab pair straddling the tile edge, crossing itself.
    for x in [250.5, 262.5, 256.5, 250.5] {
        dab_at(
            &mut s,
            Some(&mut snap),
            (x, 100.5),
            18.0,
            PaintAmount::new(0.6, 0.8),
        );
    }
    let captured: Vec<TileId> = snap.captured().collect();
    assert_eq!(captured.len(), 2);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(bits(texel(&mut s, 255, 100)), bits(existing));
    assert_eq!(bits(texel(&mut s, 256, 100)), bits(existing));
    assert_eq!(texel(&mut s, 250, 100)[3].to_bits(), 0);
    assert!(!history.can_undo());
}

#[test]
fn a_dab_wholly_outside_the_selection_creates_no_tile_under_the_cap() {
    struct Nowhere;
    impl Coverage for Nowhere {
        fn bounds(&self) -> Option<Rect> {
            Some(Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            })
        }
        fn at(&self, x: i64, y: i64) -> f32 {
            if (0..10).contains(&x) && (0..10).contains(&y) {
                1.0
            } else {
                0.0
            }
        }
    }
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    let outcome = stamp_dab_amount(
        &mut s,
        surface(),
        (400.5, 400.5),
        20.0,
        COLOUR,
        Some(&mut snap),
        Some(&Nowhere),
        false,
        false,
        PaintAmount::new(0.5, 0.5),
    );
    assert!(outcome.painted().is_empty());
    assert!(snap.is_empty());
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 1 }));
}

#[test]
fn a_partly_covered_texel_stays_within_the_cap() {
    let (_dir, mut s) = store();
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..20 {
        let o = stamp_dab_amount(
            &mut s,
            surface(),
            (100.5, 100.5),
            15.0,
            COLOUR,
            Some(&mut snap),
            Some(&Wavy),
            false,
            false,
            PaintAmount::new(0.5, 1.0),
        );
        assert!(o.is_complete());
    }
    for y in 80..120 {
        for x in 80..120 {
            assert!(texel(&mut s, x, y)[3].to_f32() <= 0.5, "({x}, {y})");
        }
    }
}

// --- Clone Stamp ---

fn clone_scene() -> (tempfile::TempDir, TileStore) {
    let (dir, mut s) = store();
    // Source block at x 20..60, destination (offset +100) holds grey.
    for y in 20..60 {
        for x in 20..60 {
            put(&mut s, x, y, px(0.5, 0.25, 0.125, 1.0));
        }
    }
    for y in 20..60 {
        for x in 120..160 {
            put(&mut s, x, y, px(0.125, 0.125, 0.125, 1.0));
        }
    }
    (dir, s)
}

fn clone_at(s: &mut TileStore, snap: &mut StrokeSnapshot, amount: PaintAmount, lock: bool) {
    let o = clone_dab_amount(
        s,
        surface(),
        (140.5, 40.5),
        10.0,
        (-100, 0),
        lock,
        Some(snap),
        None,
        amount,
    );
    assert!(o.is_complete());
}

#[test]
fn clone_opacity_is_a_stroke_ceiling_and_flow_builds_up() {
    // Opacity only: the centre sits at start + (source - start) * 0.5 and
    // stays there however often the stroke dabs.
    let (_dir, mut s) = clone_scene();
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..4 {
        clone_at(&mut s, &mut snap, PaintAmount::new(0.5, 1.0), false);
    }
    assert_eq!(
        bits(texel(&mut s, 140, 40)),
        bits(px(
            0.125 + (0.5 - 0.125) * 0.5,
            0.125 + (0.25 - 0.125) * 0.5,
            0.125,
            1.0
        ))
    );
    // Flow only: each dab moves half of the remaining way.
    let (_d2, mut s2) = clone_scene();
    let mut snap2 = StrokeSnapshot::new(surface());
    clone_at(&mut s2, &mut snap2, PaintAmount::new(1.0, 0.5), false);
    let one = texel(&mut s2, 140, 40)[0].to_f32();
    assert_eq!(h(one).to_bits(), h(0.125 + (0.5 - 0.125) * 0.5).to_bits());
    clone_at(&mut s2, &mut snap2, PaintAmount::new(1.0, 0.5), false);
    let two = texel(&mut s2, 140, 40)[0].to_f32();
    assert_eq!(h(two).to_bits(), h(one + (0.5 - one) * 0.5).to_bits());
    // Both: moves toward the limited target and no further.
    let (_d3, mut s3) = clone_scene();
    let mut snap3 = StrokeSnapshot::new(surface());
    for _ in 0..40 {
        clone_at(&mut s3, &mut snap3, PaintAmount::new(0.5, 0.5), false);
    }
    let limit = 0.125 + (0.5 - 0.125) * 0.5;
    let got = texel(&mut s3, 140, 40)[0].to_f32();
    assert!(
        (got - limit).abs() <= 1.0e-3 && got <= limit + 1.0e-3,
        "{got}"
    );
}

#[test]
fn clone_with_a_full_amount_still_copies_exactly() {
    let (_dir, mut s) = clone_scene();
    let mut snap = StrokeSnapshot::new(surface());
    clone_at(&mut s, &mut snap, PaintAmount::FULL, false);
    assert_eq!(
        bits(texel(&mut s, 140, 40)),
        bits(px(0.5, 0.25, 0.125, 1.0))
    );
}

#[test]
fn clone_with_the_lock_keeps_alpha_under_the_cap() {
    let (_dir, mut s) = clone_scene();
    // The destination is translucent: alpha must stay.
    for y in 20..60 {
        for x in 120..160 {
            put(&mut s, x, y, px(0.0625, 0.0625, 0.0625, 0.5));
        }
    }
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..3 {
        clone_at(&mut s, &mut snap, PaintAmount::new(0.5, 1.0), true);
    }
    let t = texel(&mut s, 140, 40);
    assert_eq!(t[3].to_bits(), h(0.5).to_bits());
    // Source straight colour (0.5, 0.25, 0.125) at the destination alpha
    // 0.5 is (0.25, 0.125, 0.0625); half of the way from 0.0625.
    assert_eq!(t[0].to_bits(), h(0.0625 + (0.25 - 0.0625) * 0.5).to_bits());
}

#[test]
fn clone_amounts_do_not_change_what_is_captured() {
    let (_dir, mut s) = clone_scene();
    let mut snap = StrokeSnapshot::new(surface());
    clone_at(&mut s, &mut snap, PaintAmount::new(0.3, 0.3), false);
    let captured: Vec<TileId> = snap.captured().collect();
    assert_eq!(captured, vec![TileId { x: 0, y: 0 }]);
    let again = clone_dab_covered(
        &mut s,
        surface(),
        (400.5, 400.5),
        10.0,
        (-100, 0),
        false,
        None,
        None,
    );
    assert!(again.painted().is_empty(), "nothing to clone, no tile");
}
