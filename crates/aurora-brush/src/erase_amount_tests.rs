//! Flow and Opacity of the Eraser (0.229.0) at the brush level.
//!
//! The model under test is written out on `crate::erase_dab_amount`. The
//! tests compare the engine, texel for texel and bit for bit, with an
//! **independent** model (a sparse map of texels, the erase formula
//! transcribed from that description, no tiles) over random dab sequences,
//! check the closed form `start + (erased - start) * opacity` against a
//! float64 accumulation of the whole stroke (which is what makes the
//! incremental rule the right one), and pin Opacity 100 % / Flow 100 % to
//! the pre-0.229.0 entry points by comparing against them on the same random
//! sequences (no stored hash: a float-derived pixel hash is platform
//! dependent).

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]

use std::collections::HashMap;

use aurora_core::Rect;
use aurora_tile::{TileId, TileStore};
use half::f16;

use crate::amount_tests::{Rng, SIDE, Tx, Wavy, bits, h, put, px, region_hash, soft_alpha};
use crate::amount_tests::{store, surface, texel};
use crate::{
    Coverage, PaintAmount, PixelHistory, StrokeSnapshot, erase_dab, erase_dab_amount,
    erase_dab_covered,
};

/// The Eraser model: sparse texels (absent is transparent black) and the
/// stroke-start copy the opacity ceiling reads.
#[derive(Clone, Default)]
struct EraseModel {
    now: HashMap<(u32, u32), Tx>,
    began: HashMap<(u32, u32), Tx>,
}

fn at(map: &HashMap<(u32, u32), Tx>, x: u32, y: u32) -> Tx {
    map.get(&(x, y)).copied().unwrap_or([f16::ZERO; 4])
}

impl EraseModel {
    fn begin_stroke(&mut self) {
        self.began = self.now.clone();
    }

    fn dab(
        &mut self,
        center: (f32, f32),
        radius: f32,
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
                let a = if c < 1.0 { a * c } else { a };
                let cur = at(&self.now, x, y);
                let dst = cur[3].to_f32();
                if dst <= 0.0 {
                    continue;
                }
                let fresh = if opacity < 1.0 {
                    let start = at(&self.began, x, y)[3].to_f32();
                    let floor = start * (1.0 - opacity);
                    if dst <= floor {
                        continue;
                    }
                    h(floor + (dst - floor) * (1.0 - a))
                } else {
                    h(dst * (1.0 - a))
                };
                if fresh.to_bits() == cur[3].to_bits() {
                    continue;
                }
                self.now.insert((x, y), [cur[0], cur[1], cur[2], fresh]);
            }
        }
    }
}

/// Translucent and opaque patches with colour, in the store and the model.
fn seed(store: &mut TileStore, model: &mut EraseModel, rng: &mut Rng) {
    for _ in 0..50 {
        let (x, y) = (
            50 + rng.below(u64::from(SIDE) - 130) as u32,
            50 + rng.below(u64::from(SIDE) - 130) as u32,
        );
        let a = if rng.below(3) == 0 {
            1.0
        } else {
            rng.range(0.1, 1.0)
        };
        let t = px(rng.unit() * a, rng.unit() * a, rng.unit() * a, a);
        for dy in 0..10 {
            for dx in 0..10 {
                put(store, x + dx, y + dy, t);
                model.now.insert((x + dx, y + dy), t);
            }
        }
    }
}

fn assert_matches(store: &mut TileStore, model: &EraseModel, why: &str) {
    for y in 0..SIDE {
        for x in 0..SIDE {
            let got = texel(store, x, y);
            let want = at(&model.now, x, y);
            assert_eq!(bits(got), bits(want), "{why}: texel ({x}, {y})");
        }
    }
}

/// A block of one texel value, in the store.
fn fill(store: &mut TileStore, x0: u32, y0: u32, x1: u32, y1: u32, t: Tx) {
    for y in y0..y1 {
        for x in x0..x1 {
            put(store, x, y, t);
        }
    }
}

fn erase_outcome(
    s: &mut TileStore,
    snap: Option<&mut StrokeSnapshot>,
    center: (f32, f32),
    radius: f32,
    amount: PaintAmount,
    cover: Option<&dyn Coverage>,
) -> crate::DabOutcome {
    let outcome = erase_dab_amount(s, surface(), center, radius, snap, cover, amount);
    assert!(outcome.is_complete());
    outcome
}

fn erase(
    s: &mut TileStore,
    snap: Option<&mut StrokeSnapshot>,
    center: (f32, f32),
    radius: f32,
    amount: PaintAmount,
    cover: Option<&dyn Coverage>,
) {
    let _ = erase_outcome(s, snap, center, radius, amount, cover);
}

fn alpha(s: &mut TileStore, x: u32, y: u32) -> f32 {
    texel(s, x, y)[3].to_f32()
}

// ---------------------------------------------------------------------------
// Differential tests.

#[test]
fn random_erase_strokes_match_the_independent_model_bit_for_bit() {
    for (seed_value, covered) in [(11_u64, false), (12, true), (13, false), (14, true)] {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed_value);
        let (_dir, mut s) = store();
        let mut model = EraseModel::default();
        seed(&mut s, &mut model, &mut rng);
        for stroke in 0..5 {
            model.begin_stroke();
            let mut snap = StrokeSnapshot::new(surface());
            let amount = match stroke {
                0 => PaintAmount::new(0.5, 1.0),
                1 => PaintAmount::new(0.3, 0.6),
                2 => PaintAmount::new(1.0, 0.4),
                3 => PaintAmount::FULL,
                _ => PaintAmount::new(0.8, 0.9),
            };
            let mut cx = rng.range(80.0, 430.0);
            let mut cy = rng.range(80.0, 430.0);
            for _ in 0..30 {
                cx = (cx + rng.range(-9.0, 9.0)).clamp(60.0, 450.0);
                cy = (cy + rng.range(-9.0, 9.0)).clamp(60.0, 450.0);
                let radius = rng.range(2.0, 28.0);
                let cov: Option<&dyn Coverage> = if covered { Some(&Wavy) } else { None };
                erase(&mut s, Some(&mut snap), (cx, cy), radius, amount, cov);
                model.dab((cx, cy), radius, amount.opacity(), amount.flow(), cov);
            }
            assert_matches(
                &mut s,
                &model,
                &format!("seed {seed_value} stroke {stroke}"),
            );
        }
    }
}

#[test]
fn full_opacity_and_flow_are_the_pre_0_229_0_entry_points() {
    // One random sequence through the old entry points and through
    // `erase_dab_amount` at every spelling of 100 %: identical texels.
    let run = |mode: u8| {
        let mut rng = Rng(0xABCD_EF01_2345_6789);
        let (_dir, mut s) = store();
        let mut model = EraseModel::default();
        seed(&mut s, &mut model, &mut rng);
        let mut snap = StrokeSnapshot::new(surface());
        for i in 0..200 {
            let center = (rng.range(40.0, 470.0), rng.range(40.0, 470.0));
            let radius = rng.range(1.0, 30.0);
            let cover: Option<&dyn Coverage> = if i % 2 == 0 { Some(&Wavy) } else { None };
            let outcome = match mode {
                0 => erase_dab_covered(&mut s, surface(), center, radius, Some(&mut snap), cover),
                1 => erase_dab_amount(
                    &mut s,
                    surface(),
                    center,
                    radius,
                    Some(&mut snap),
                    cover,
                    PaintAmount::FULL,
                ),
                2 => erase_dab_amount(
                    &mut s,
                    surface(),
                    center,
                    radius,
                    Some(&mut snap),
                    cover,
                    PaintAmount::new(1.0, 1.0),
                ),
                _ => erase_dab_amount(
                    &mut s,
                    surface(),
                    center,
                    radius,
                    Some(&mut snap),
                    cover,
                    PaintAmount::new(7.0, f32::NAN),
                ),
            };
            assert!(outcome.is_complete());
        }
        region_hash(&mut s)
    };
    let legacy = run(0);
    for mode in 1..4 {
        assert_eq!(run(mode), legacy, "spelling {mode}");
    }
    // And the bare `erase_dab` (no selection) is the same entry again.
    let mut rng = Rng(5);
    let (_d1, mut a) = store();
    let (_d2, mut b) = store();
    let mut scratch = EraseModel::default();
    let mut rng2 = Rng(5);
    seed(&mut a, &mut scratch, &mut rng);
    seed(&mut b, &mut scratch, &mut rng2);
    for _ in 0..60 {
        let center = (rng.range(40.0, 470.0), rng.range(40.0, 470.0));
        let radius = rng.range(1.0, 30.0);
        assert!(erase_dab(&mut a, surface(), center, radius, None).is_complete());
        assert!(
            erase_dab_amount(
                &mut b,
                surface(),
                center,
                radius,
                None,
                None,
                PaintAmount::FULL
            )
            .is_complete()
        );
    }
    assert_eq!(region_hash(&mut a), region_hash(&mut b));
}

#[test]
fn the_stored_alpha_is_start_plus_erased_minus_start_times_opacity() {
    // The definition, evaluated for the whole stroke in float64 (the
    // product of every dab's `1 - a`), against the incremental engine.
    // The allowed difference is `f16` rounding accumulated over the dabs.
    for (opacity, flow) in [(0.4_f32, 1.0_f32), (0.7, 0.3), (0.25, 0.6), (0.9, 1.0)] {
        let mut rng = Rng(0x1234_5678 ^ u64::from(opacity.to_bits()));
        let (_dir, mut s) = store();
        fill(&mut s, 60, 60, 200, 200, px(0.25, 0.25, 0.25, 0.5));
        fill(&mut s, 120, 60, 200, 200, px(0.5, 0.5, 0.5, 1.0));
        let mut snap = StrokeSnapshot::new(surface());
        let amount = PaintAmount::new(opacity, flow);
        let mut product: HashMap<(u32, u32), f64> = HashMap::new();
        let mut cx = 120.0_f32;
        let mut cy = 120.0_f32;
        for _ in 0..25 {
            cx = (cx + rng.range(-8.0, 8.0)).clamp(90.0, 150.0);
            cy = (cy + rng.range(-8.0, 8.0)).clamp(90.0, 150.0);
            erase(&mut s, Some(&mut snap), (cx, cy), 22.0, amount, None);
            for y in 80..180 {
                for x in 80..180 {
                    let a = soft_alpha(x, y, (cx, cy), 22.0);
                    if a > 0.0 {
                        *product.entry((x, y)).or_insert(1.0) *= 1.0 - f64::from(a * flow);
                    }
                }
            }
        }
        let mut compared = 0;
        for (&(x, y), &p) in &product {
            let start = if x >= 120 { 1.0 } else { 0.5 };
            let want = start * (1.0 - f64::from(opacity) * (1.0 - p));
            let got = f64::from(alpha(&mut s, x, y));
            assert!(
                (got - want).abs() < 0.012,
                "opacity {opacity} flow {flow} at ({x}, {y}): {got} vs {want}"
            );
            compared += 1;
        }
        assert!(compared > 1000, "a real area was compared: {compared}");
    }
}

// ---------------------------------------------------------------------------
// What the settings mean.

#[test]
fn flow_scales_each_erase_dab() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let (cx, cy) = (100.5, 100.5);
    let a = soft_alpha(100, 100, (cx, cy), 20.0);
    assert!(a > 0.9);
    erase(
        &mut s,
        None,
        (cx, cy),
        20.0,
        PaintAmount::new(1.0, 0.5),
        None,
    );
    assert_eq!(
        texel(&mut s, 100, 100)[3].to_bits(),
        h(1.0 - a * 0.5).to_bits()
    );
    // The same dab at Flow 100 % erases further.
    let (_dir2, mut t) = store();
    fill(&mut t, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    erase(&mut t, None, (cx, cy), 20.0, PaintAmount::FULL, None);
    assert_eq!(texel(&mut t, 100, 100)[3].to_bits(), h(1.0 - a).to_bits());
    assert!(alpha(&mut s, 100, 100) > alpha(&mut t, 100, 100));
}

#[test]
fn flow_accumulates_over_a_stroke_and_only_opacity_caps_it() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let mut snap = StrokeSnapshot::new(surface());
    let mut last = 1.0;
    for _ in 0..12 {
        erase(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            20.0,
            PaintAmount::new(1.0, 0.2),
            None,
        );
        let now = alpha(&mut s, 100, 100);
        assert!(now < last, "each dab erases a little more: {now} < {last}");
        last = now;
    }
    assert!(last < 0.1, "the erase built up toward nothing: {last}");
}

#[test]
fn opacity_caps_how_much_a_stroke_can_erase_in_total() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let mut snap = StrokeSnapshot::new(surface());
    // Thirty full-strength dabs crossing the same texels.
    for i in 0..30 {
        let jitter = (i % 3) as f32 - 1.0;
        erase(
            &mut s,
            Some(&mut snap),
            (100.5 + jitter, 100.5),
            20.0,
            PaintAmount::new(0.4, 1.0),
            None,
        );
    }
    let floor = h(0.6).to_f32();
    let centre = alpha(&mut s, 100, 100);
    assert!(
        (centre - floor).abs() < 1e-3,
        "the centre reached the ceiling 1 - 0.4 and stopped: {centre}"
    );
    for y in 80..130 {
        for x in 80..130 {
            assert!(alpha(&mut s, x, y) >= floor - 1e-3, "({x}, {y})");
        }
    }
    // Without the cap the same dabs erase the centre outright.
    let (_dir2, mut t) = store();
    fill(&mut t, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let mut snap2 = StrokeSnapshot::new(surface());
    for i in 0..30 {
        let jitter = (i % 3) as f32 - 1.0;
        erase(
            &mut t,
            Some(&mut snap2),
            (100.5 + jitter, 100.5),
            20.0,
            PaintAmount::FULL,
            None,
        );
    }
    assert!(alpha(&mut t, 100, 100) < 0.01);
}

#[test]
fn the_ceiling_does_not_depend_on_flow() {
    // A lower flow only gets there in more dabs (and stops an `f16` step or
    // two short, where one more dab no longer changes the stored bits).
    for flow in [0.1_f32, 0.5, 1.0] {
        let (_dir, mut s) = store();
        fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
        let mut snap = StrokeSnapshot::new(surface());
        for _ in 0..600 {
            erase(
                &mut s,
                Some(&mut snap),
                (100.5, 100.5),
                20.0,
                PaintAmount::new(0.5, flow),
                None,
            );
        }
        let reached = alpha(&mut s, 100, 100);
        assert!(
            (0.499..0.505).contains(&reached),
            "flow {flow} reached {reached}, not 1 - opacity"
        );
    }
}

#[test]
fn each_texel_is_capped_against_its_own_start_alpha() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 100, 130, px(0.5, 0.25, 0.125, 1.0));
    fill(&mut s, 100, 80, 130, 130, px(0.25, 0.125, 0.0625, 0.5));
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..40 {
        erase(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            24.0,
            PaintAmount::new(0.5, 1.0),
            None,
        );
    }
    assert!((alpha(&mut s, 95, 100) - 0.5).abs() < 2e-3, "opaque side");
    assert!((alpha(&mut s, 105, 100) - 0.25).abs() < 2e-3, "half side");
}

#[test]
fn a_new_stroke_starts_from_the_content_it_finds() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    for _ in 0..2 {
        let mut snap = StrokeSnapshot::new(surface());
        for _ in 0..20 {
            erase(
                &mut s,
                Some(&mut snap),
                (100.5, 100.5),
                20.0,
                PaintAmount::new(0.4, 1.0),
                None,
            );
        }
    }
    // 1.0 -> 0.6 (first stroke) -> 0.36 (second stroke from 0.6).
    assert!((alpha(&mut s, 100, 100) - 0.36).abs() < 2e-3);
}

#[test]
fn with_no_snapshot_every_dab_is_its_own_stroke() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    // The centre texel's dab amount is ~0.947: the dab erases ~half of what
    // is there at opacity 0.5, then half of the rest, and so on.
    let mut expect = 1.0_f32;
    for _ in 0..3 {
        erase(
            &mut s,
            None,
            (100.5, 100.5),
            20.0,
            PaintAmount::new(0.5, 1.0),
            None,
        );
        let a = soft_alpha(100, 100, (100.5, 100.5), 20.0);
        let floor = expect * 0.5;
        expect = h(floor + (expect - floor) * (1.0 - a)).to_f32();
        assert_eq!(alpha(&mut s, 100, 100), expect);
    }
    assert!(expect < 0.15, "it kept erasing, dab by dab: {expect}");
}

#[test]
fn a_texel_at_its_floor_changes_nothing_and_captures_nothing() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let mut snap = StrokeSnapshot::new(surface());
    // Long enough for every texel of this small dab to reach a fixed point
    // (at its floor, or too faint a dab to move the stored bits at all).
    for _ in 0..3000 {
        erase(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            6.0,
            PaintAmount::new(0.5, 1.0),
            None,
        );
    }
    let before = region_hash(&mut s);
    let outcome = erase_outcome(
        &mut s,
        Some(&mut snap),
        (100.5, 100.5),
        6.0,
        PaintAmount::new(0.5, 1.0),
        None,
    );
    assert!(outcome.painted().is_empty(), "nothing left to erase");
    assert_eq!(region_hash(&mut s), before);
    assert!(alpha(&mut s, 100, 100) >= 0.499);
    // A fresh stroke over the same pixels has a new floor, so it erases.
    let mut fresh = StrokeSnapshot::new(surface());
    let outcome = erase_outcome(
        &mut s,
        Some(&mut fresh),
        (100.5, 100.5),
        6.0,
        PaintAmount::new(0.5, 1.0),
        None,
    );
    assert!(!outcome.painted().is_empty());
}

#[test]
fn colour_channels_are_never_touched_and_transparent_texels_stay_so() {
    let (_dir, mut s) = store();
    let colour = px(0.375, 0.25, 0.125, 0.75);
    fill(&mut s, 80, 80, 130, 130, colour);
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..10 {
        erase(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            30.0,
            PaintAmount::new(0.6, 0.7),
            None,
        );
    }
    for y in 80..130 {
        for x in 80..130 {
            let t = texel(&mut s, x, y);
            assert_eq!(bits([t[0], t[1], t[2], f16::ZERO])[..3], bits(colour)[..3]);
        }
    }
    // Outside the painted block nothing appeared.
    assert_eq!(bits(texel(&mut s, 60, 60)), [0; 4]);
}

// ---------------------------------------------------------------------------
// Composition.

#[test]
fn a_partly_covered_texel_stays_within_the_ceiling() {
    let (_dir, mut s) = store();
    fill(&mut s, 60, 60, 160, 160, px(0.5, 0.25, 0.125, 1.0));
    let mut snap = StrokeSnapshot::new(surface());
    for _ in 0..40 {
        erase(
            &mut s,
            Some(&mut snap),
            (100.5, 100.5),
            25.0,
            PaintAmount::new(0.5, 1.0),
            Some(&Wavy),
        );
    }
    let mut erased_some = false;
    let mut kept_some = false;
    for y in 70..130 {
        for x in 70..130 {
            let a = alpha(&mut s, x, y);
            assert!(a >= 0.5 - 1e-3, "({x}, {y}) went past the ceiling: {a}");
            erased_some |= a < 0.9;
            kept_some |= a > 0.999;
        }
    }
    assert!(erased_some && kept_some, "the selection kept some pixels");
}

#[test]
fn a_dab_wholly_outside_the_selection_erases_nothing_and_creates_no_tile() {
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
    let outcome = erase_outcome(
        &mut s,
        Some(&mut snap),
        (400.5, 400.5),
        20.0,
        PaintAmount::new(0.5, 0.5),
        Some(&Nowhere),
    );
    assert!(outcome.painted().is_empty());
    assert!(snap.is_empty());
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 1 }));
}

#[test]
fn a_capped_erase_stroke_is_one_undo_step_that_restores_the_start() {
    let (_dir, mut s) = store();
    let before = px(0.125, 0.25, 0.25, 0.5);
    fill(&mut s, 230, 80, 280, 120, before);
    let mut snap = StrokeSnapshot::new(surface());
    // Dabs straddling the tile edge at x = 256, crossing themselves.
    for x in [250.5, 262.5, 256.5, 250.5] {
        erase(
            &mut s,
            Some(&mut snap),
            (x, 100.5),
            18.0,
            PaintAmount::new(0.6, 0.8),
            None,
        );
    }
    assert_eq!(snap.captured().count(), 2, "both tiles of the edge");
    assert!(alpha(&mut s, 256, 100) < 0.5);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    for x in [230, 255, 256, 279] {
        assert_eq!(bits(texel(&mut s, x, 100)), bits(before), "x = {x}");
    }
    assert!(!history.can_undo());
}

#[test]
fn a_texel_already_below_its_floor_is_never_raised() {
    let (_dir, mut s) = store();
    fill(&mut s, 80, 80, 130, 130, px(0.5, 0.25, 0.125, 1.0));
    let mut snap = StrokeSnapshot::new(surface());
    // The first dab captures the tile, so the stroke started at alpha 1.
    erase(
        &mut s,
        Some(&mut snap),
        (100.5, 100.5),
        20.0,
        PaintAmount::new(0.5, 1.0),
        None,
    );
    // Something else takes a texel below the stroke's floor of 0.5.
    put(&mut s, 100, 100, px(0.5, 0.25, 0.125, 0.25));
    erase(
        &mut s,
        Some(&mut snap),
        (100.5, 100.5),
        20.0,
        PaintAmount::new(0.5, 1.0),
        None,
    );
    assert_eq!(texel(&mut s, 100, 100)[3].to_bits(), h(0.25).to_bits());
}
