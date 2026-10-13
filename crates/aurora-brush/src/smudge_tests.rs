//! The Smudge tool's dab (0.222.0) at the brush level: an exact drag of a
//! known pattern, strength 0 and 1, the carried patch's mix, the footprint,
//! stroke-order exactness against an independent model, selection coverage,
//! the transparency lock, no phantom tiles, undo, the cap and degenerate
//! input.

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

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, DabOutcome, MAX_SMUDGE_PATCH_TEXELS, PixelHistory, SmudgeCarry, StrokeSnapshot,
    dab_falloff, smudge_dab_covered, stamp_dab,
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

/// A block of distinct, opaque texels.
fn pattern_colour(x: u32, y: u32) -> [f16; CHANNELS] {
    px(
        (x % 16) as f32 / 32.0 + 0.0625,
        (y % 16) as f32 / 32.0 + 0.0625,
        ((x + y) % 7) as f32 / 16.0 + 0.125,
        1.0,
    )
}

fn paint_pattern(s: &mut TileStore, x0: u32, y0: u32, w: u32, h: u32) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            put(s, x, y, pattern_colour(x, y));
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

fn smudge(
    s: &mut TileStore,
    carry: &mut SmudgeCarry,
    center: (f32, f32),
    radius: f32,
    strength: f32,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    smudge_dab_covered(
        s,
        surface(),
        center,
        radius,
        strength,
        false,
        carry,
        snapshot,
        coverage,
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

#[test]
fn the_first_dab_only_picks_up_and_changes_nothing() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 20, 20, 30, 30);
    let before = dump(&mut s, 64, 64);
    let mut carry = SmudgeCarry::default();
    assert!(carry.is_empty());
    let out = smudge(&mut s, &mut carry, (30.5, 30.5), 6.0, 1.0, None, None);
    assert!(out.painted().is_empty());
    assert!(out.is_complete());
    assert_eq!(dump(&mut s, 64, 64), before);
    assert!(!carry.is_empty());
    // It holds the layer under the finger, premultiplied, offset from the
    // anchor (30, 30).
    for (dx, dy) in [(0_i64, 0_i64), (3, -2), (-5, 4)] {
        let want = pattern_colour((30 + dx) as u32, (30 + dy) as u32).map(f16::to_f32);
        assert_eq!(carry.texel_at(dx, dy), Some(want), "({dx}, {dy})");
    }
    // Beyond the patch there is nothing.
    assert_eq!(carry.texel_at(100, 0), None);
    assert_eq!(SmudgeCarry::default().texel_at(0, 0), None);
}

#[test]
fn strength_one_drags_a_known_pattern_exactly_one_dab_ahead() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 10, 20, 60, 24);
    let original = dump(&mut s, 80, 64);
    let mut carry = SmudgeCarry::default();
    let radius = 6.0;
    let (c1, c2) = ((30.5_f32, 30.5_f32), (34.5_f32, 30.5_f32));
    let _ = smudge(&mut s, &mut carry, c1, radius, 1.0, None, None);
    let out = smudge(&mut s, &mut carry, c2, radius, 1.0, None, None);
    assert!(out.is_complete());
    assert!(!out.painted().is_empty());
    for y in 0..64u32 {
        for x in 0..80u32 {
            let d = (x as f32 + 0.5 - c2.0).hypot(y as f32 + 0.5 - c2.1);
            let got = bits(texel(&mut s, x, y));
            let old = original[(y * 80 + x) as usize];
            if d >= radius {
                assert_eq!(got, old, "outside the footprint at ({x}, {y})");
                continue;
            }
            let a = dab_falloff(d, radius);
            // The first dab changed nothing, so the carried paint is the
            // original layer, one dab (4 texels) behind the second dab.
            let carried = original[(y * 80 + x - 4) as usize].map(|b| f16::from_bits(b).to_f32());
            let want: Vec<u16> = (0..4)
                .map(|c| {
                    if a >= 1.0 {
                        f16::from_f32(carried[c]).to_bits()
                    } else {
                        let o = f16::from_bits(old[c]).to_f32();
                        f16::from_f32(o + (carried[c] - o) * a).to_bits()
                    }
                })
                .collect();
            assert_eq!(got.to_vec(), want, "texel ({x}, {y})");
        }
    }
    // The centre texel (falloff exactly 1) is the original texel 4 behind.
    assert_eq!(
        bits(texel(&mut s, 34, 30)),
        original[(30 * 80 + 30) as usize]
    );
}

#[test]
fn strength_one_keeps_depositing_the_first_paint_for_the_whole_stroke() {
    // At strength 1 nothing is picked up: the finger deposits what it
    // landed on at every dab, however far it goes.
    let (_dir, mut s) = store();
    for y in 20..28 {
        for x in 20..32 {
            put(&mut s, x, y, px(1.0, 0.0, 0.0, 1.0));
        }
    }
    let mut carry = SmudgeCarry::default();
    for x in [24.5_f32, 29.5, 34.5, 39.5, 44.5, 49.5] {
        let _ = smudge(&mut s, &mut carry, (x, 24.5), 3.0, 1.0, None, None);
    }
    // The last dab's centre texel is red again, 25 texels from where it
    // landed.
    assert_eq!(bits(texel(&mut s, 49, 24)), bits(px(1.0, 0.0, 0.0, 1.0)));
}

#[test]
fn strength_zero_changes_nothing() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 10, 10, 50, 30);
    let before = dump(&mut s, 80, 64);
    let mut carry = SmudgeCarry::default();
    let mut snapshot = StrokeSnapshot::new(surface());
    for x in [20.5_f32, 24.5, 28.5, 32.5, 36.5] {
        let out = smudge(
            &mut s,
            &mut carry,
            (x, 25.5),
            7.0,
            0.0,
            Some(&mut snapshot),
            None,
        );
        assert!(out.painted().is_empty());
        assert!(out.is_complete());
    }
    assert_eq!(dump(&mut s, 80, 64), before);
    assert_eq!(snapshot.captured().count(), 0, "nothing was captured");
    // The finger carries only what it is over (strength 0): the carried
    // centre texel is the original texel under the last dab.
    let want = pattern_colour(36, 25).map(f16::to_f32);
    assert_eq!(carry.texel_at(0, 0), Some(want));
}

#[test]
fn the_carried_paint_is_mixed_with_what_is_under_it_by_the_strength() {
    let (_dir, mut s) = store();
    for y in 10..30 {
        for x in 10..30 {
            put(&mut s, x, y, px(1.0, 0.0, 0.0, 1.0));
        }
        for x in 30..50 {
            put(&mut s, x, y, px(0.0, 0.0, 1.0, 1.0));
        }
    }
    let mut carry = SmudgeCarry::default();
    // Land on red, then move to a blue spot far enough that the whole
    // footprint is blue.
    let _ = smudge(&mut s, &mut carry, (20.5, 20.5), 3.0, 0.5, None, None);
    assert_eq!(carry.texel_at(0, 0), Some([1.0, 0.0, 0.0, 1.0]));
    let _ = smudge(&mut s, &mut carry, (40.5, 20.5), 3.0, 0.5, None, None);
    // carried = carried * s + under * (1 - s), exactly, in f32.
    assert_eq!(carry.texel_at(0, 0), Some([0.5, 0.0, 0.5, 1.0]));
    // The texel just written at the centre (falloff 1) equals the carried
    // paint *before* the mix: red moved 0.5 toward... it is `old +
    // (carried - old) * 0.5` with old blue and carried red.
    assert_eq!(bits(texel(&mut s, 40, 20)), bits(px(0.5, 0.0, 0.5, 1.0)));
    // A third dab on the same blue: carried 0.5/0/0.5 mixes toward blue.
    let _ = smudge(&mut s, &mut carry, (40.5, 25.5), 3.0, 0.5, None, None);
    assert_eq!(carry.texel_at(0, 0), Some([0.25, 0.0, 0.75, 1.0]));
}

/// An independent model of the whole stroke on a plain `f32` grid, written
/// from the module docs and nothing else.
fn model_stroke(
    grid: &mut [[f32; 4]],
    width: usize,
    dabs: &[(f32, f32)],
    radius: f32,
    strength: f32,
) {
    let half = radius.ceil() as i64 + 1;
    let side = (2 * half + 1) as usize;
    let mut carried: Vec<[f32; 4]> = Vec::new();
    for &(cx, cy) in dabs {
        let (ax, ay) = (cx.floor() as i64, cy.floor() as i64);
        let mut under = vec![[0.0_f32; 4]; side * side];
        for j in 0..side {
            for i in 0..side {
                let (x, y) = (ax - half + i as i64, ay - half + j as i64);
                if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < grid.len() / width {
                    under[j * side + i] = grid[y as usize * width + x as usize];
                }
            }
        }
        if carried.is_empty() {
            carried = under;
            continue;
        }
        for j in 0..side {
            for i in 0..side {
                let (x, y) = (ax - half + i as i64, ay - half + j as i64);
                if x < 0 || y < 0 || (x as usize) >= width || (y as usize) >= grid.len() / width {
                    continue;
                }
                let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
                let a = dab_falloff(d, radius);
                if a <= 0.0 {
                    continue;
                }
                let k = strength * a;
                let old = grid[y as usize * width + x as usize];
                let c = carried[j * side + i];
                let mut new = [0.0_f32; 4];
                for ch in 0..4 {
                    let v = if k >= 1.0 {
                        c[ch]
                    } else {
                        old[ch] + (c[ch] - old[ch]) * k
                    };
                    new[ch] = f16::from_f32(v).to_f32();
                }
                grid[y as usize * width + x as usize] = new;
            }
        }
        for (c, u) in carried.iter_mut().zip(&under) {
            for ch in 0..4 {
                c[ch] = c[ch] * strength + u[ch] * (1.0 - strength);
            }
        }
    }
}

#[test]
fn a_whole_stroke_matches_an_independent_model_exactly() {
    for strength in [0.25_f32, 0.5, 1.0] {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 70, 40);
        let (w, h) = (96_u32, 64_u32);
        let mut grid: Vec<[f32; 4]> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                grid.push(texel(&mut s, x, y).map(f16::to_f32));
            }
        }
        let dabs = [
            (20.3_f32, 24.7_f32),
            (23.9, 26.1),
            (27.4, 27.8),
            (31.2, 29.9),
            (35.6, 31.0),
            (39.5, 28.2),
            (42.2, 24.4),
        ];
        let mut carry = SmudgeCarry::default();
        for &(cx, cy) in &dabs {
            let _ = smudge(&mut s, &mut carry, (cx, cy), 9.0, strength, None, None);
        }
        model_stroke(&mut grid, w as usize, &dabs, 9.0, strength);
        for y in 0..h {
            for x in 0..w {
                let want = grid[(y * w + x) as usize].map(|v| f16::from_f32(v).to_bits());
                assert_eq!(
                    bits(texel(&mut s, x, y)),
                    want,
                    "strength {strength} texel ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn a_stroke_is_deterministic_and_depends_on_dab_order() {
    let run = |order: &[(f32, f32)]| {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 60, 40);
        let mut carry = SmudgeCarry::default();
        for &c in order {
            let _ = smudge(&mut s, &mut carry, c, 8.0, 0.5, None, None);
        }
        dump(&mut s, 80, 64)
    };
    let forward = [(20.5, 25.5), (25.5, 25.5), (30.5, 27.5), (35.5, 27.5)];
    let mut backward = forward;
    backward.reverse();
    assert_eq!(run(&forward), run(&forward), "the same stroke twice");
    assert_ne!(run(&forward), run(&backward), "the order matters");
}

#[test]
fn the_footprint_is_the_brushs_soft_dab_and_symmetric() {
    // The finger lands on opaque white, then dabs over empty space: the
    // deposited alpha is exactly the Brush's falloff at strength 1.
    let (_dir, mut s) = store();
    for y in 0..40 {
        for x in 0..40 {
            put(&mut s, x, y, px(1.0, 1.0, 1.0, 1.0));
        }
    }
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut s, &mut carry, (20.5, 20.5), 9.0, 1.0, None, None);
    let center = (100.5, 24.5);
    let _ = smudge(&mut s, &mut carry, center, 9.0, 1.0, None, None);
    let other = SurfaceId::from_raw(1);
    let _ = stamp_dab(&mut s, other, center, 9.0, [1.0, 1.0, 1.0], None);
    for y in 0..48u32 {
        for x in 80..128u32 {
            let id = TileId { x: x / TILE, y: 0 };
            let Ok(brush) = s.get(other, id) else {
                continue;
            };
            let index = (((y % TILE) * TILE + (x % TILE)) as usize) * CHANNELS;
            let brush_alpha = brush.texels()[index + 3];
            assert_eq!(
                texel(&mut s, x, y)[3].to_bits(),
                brush_alpha.to_bits(),
                "({x}, {y})"
            );
        }
    }
    // Mirror symmetry about the centre texel (100, 24).
    for dy in 0..=9_i64 {
        for dx in 0..=9_i64 {
            let at = |sx: i64, sy: i64, s: &mut TileStore| {
                bits(texel(s, (100 + sx * dx) as u32, (24 + sy * dy) as u32))
            };
            let base = at(1, 1, &mut s);
            assert_eq!(at(-1, 1, &mut s), base, "x mirror {dx} {dy}");
            assert_eq!(at(1, -1, &mut s), base, "y mirror {dx} {dy}");
            assert_eq!(at(-1, -1, &mut s), base, "both {dx} {dy}");
        }
    }
}

#[test]
fn a_selection_restricts_the_smudge_and_blends_by_coverage() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 8, 8, 60, 30);
    let mut carry = SmudgeCarry::default();
    let cover = RectCov(Rect {
        x: 28,
        y: 0,
        width: 4,
        height: 64,
    });
    let _ = smudge(&mut s, &mut carry, (20.5, 20.5), 6.0, 1.0, None, None);
    let before = dump(&mut s, 64, 40);
    let _ = smudge(
        &mut s,
        &mut carry,
        (30.5, 20.5),
        6.0,
        1.0,
        None,
        Some(&cover),
    );
    let after = dump(&mut s, 64, 40);
    let mut changed = 0;
    for y in 0..40u32 {
        for x in 0..64u32 {
            let i = (y * 64 + x) as usize;
            if after[i] != before[i] {
                changed += 1;
                assert!(
                    (28..32).contains(&x),
                    "texel x {x} is outside the selection"
                );
            }
        }
    }
    assert!(changed > 0);

    // Half coverage: the centre texel lands halfway to the carried paint.
    let (_dir, mut h) = store();
    paint_pattern(&mut h, 8, 8, 60, 30);
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut h, &mut carry, (20.5, 20.5), 4.0, 1.0, None, None);
    let old = texel(&mut h, 30, 20);
    let _ = smudge(
        &mut h,
        &mut carry,
        (30.5, 20.5),
        4.0,
        1.0,
        None,
        Some(&Constant(0.5)),
    );
    let carried = pattern_colour(20, 20);
    let got = texel(&mut h, 30, 20);
    for c in 0..4 {
        let (o, n) = (old[c].to_f32(), carried[c].to_f32());
        assert_eq!(
            got[c].to_bits(),
            f16::from_f32(o + (n - o) * 0.5).to_bits(),
            "channel {c}"
        );
    }
}

#[test]
fn a_selection_that_excludes_a_tile_neither_reads_into_it_nor_creates_it() {
    let (_dir, mut s) = store();
    for y in 0..20 {
        for x in 0..20 {
            put(&mut s, x, y, px(1.0, 1.0, 1.0, 1.0));
        }
    }
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut s, &mut carry, (10.5, 10.5), 5.0, 1.0, None, None);
    let cover = RectCov(Rect {
        x: 500,
        y: 500,
        width: 4,
        height: 4,
    });
    let out = smudge(
        &mut s,
        &mut carry,
        (300.5, 300.5),
        5.0,
        1.0,
        None,
        Some(&cover),
    );
    assert!(out.painted().is_empty());
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 1 }));
}

#[test]
fn the_transparency_lock_keeps_alpha_bit_for_bit_and_moves_only_colour() {
    let (_dir, mut s) = store();
    // Opaque green carried over half-transparent red and an empty texel.
    for y in 8..14 {
        for x in 8..20 {
            put(&mut s, x, y, px(0.0, 1.0, 0.0, 1.0));
        }
        for x in 20..32 {
            put(&mut s, x, y, px(0.5, 0.0, 0.0, 0.5));
        }
    }
    let alphas: Vec<u16> = (20..32)
        .map(|x| texel(&mut s, x, 10)[3].to_bits())
        .collect();
    let mut carry = SmudgeCarry::default();
    let stroke = |s: &mut TileStore, carry: &mut SmudgeCarry, c: (f32, f32)| {
        smudge_dab_covered(s, surface(), c, 3.0, 1.0, true, carry, None, None)
    };
    let _ = stroke(&mut s, &mut carry, (14.5, 10.5));
    let out = stroke(&mut s, &mut carry, (26.5, 10.5));
    assert!(!out.painted().is_empty());
    let after: Vec<u16> = (20..32)
        .map(|x| texel(&mut s, x, 10)[3].to_bits())
        .collect();
    assert_eq!(after, alphas, "alpha is kept bit for bit");
    // Full falloff at the centre: green times the destination's alpha.
    let got = texel(&mut s, 26, 10);
    assert_eq!(bits(got)[..3], bits(px(0.0, 0.5, 0.0, 0.5))[..3]);
    // A fully transparent destination is left alone and creates no tile.
    let (_d2, mut t) = store();
    put(&mut t, 14, 10, px(0.0, 1.0, 0.0, 1.0));
    let mut carry = SmudgeCarry::default();
    let _ = smudge_dab_covered(
        &mut t,
        surface(),
        (14.5, 10.5),
        3.0,
        1.0,
        true,
        &mut carry,
        None,
        None,
    );
    let out = smudge_dab_covered(
        &mut t,
        surface(),
        (400.5, 10.5),
        3.0,
        1.0,
        true,
        &mut carry,
        None,
        None,
    );
    assert!(out.painted().is_empty());
    assert!(!t.contains_tile(surface(), TileId { x: 1, y: 0 }));
}

#[test]
fn smudging_empty_space_with_an_empty_finger_creates_no_tile() {
    let (_dir, mut s) = store();
    let mut carry = SmudgeCarry::default();
    for x in [300.5_f32, 304.5, 308.5, 600.5] {
        let out = smudge(&mut s, &mut carry, (x, 300.5), 6.0, 1.0, None, None);
        assert!(out.painted().is_empty());
    }
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 1 }));
    assert!(!s.contains_tile(surface(), TileId { x: 2, y: 1 }));
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
    // Reading a source never creates its tiles either.
    let (_d, mut t) = store();
    put(&mut t, 5, 5, px(1.0, 1.0, 1.0, 1.0));
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut t, &mut carry, (5.5, 5.5), 3.0, 1.0, None, None);
    assert!(!t.contains_tile(surface(), TileId { x: 1, y: 0 }));
    assert!(!t.contains_tile(surface(), TileId { x: 0, y: 1 }));
}

#[test]
fn paint_is_pushed_into_empty_space_and_creates_the_tile_it_needs() {
    let (_dir, mut s) = store();
    for y in 0..16 {
        for x in 240..256 {
            put(&mut s, x, y, px(1.0, 0.0, 0.0, 1.0));
        }
    }
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 0 }));
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut s, &mut carry, (250.5, 8.5), 5.0, 1.0, None, None);
    let out = smudge(&mut s, &mut carry, (258.5, 8.5), 5.0, 1.0, None, None);
    assert!(out.painted().contains(&TileId { x: 1, y: 0 }));
    assert!(s.contains_tile(surface(), TileId { x: 1, y: 0 }));
    assert_eq!(bits(texel(&mut s, 258, 8)), bits(px(1.0, 0.0, 0.0, 1.0)));
}

#[test]
fn undo_restores_exactly_the_captured_tiles() {
    let (_dir, mut s) = store();
    // A stroke across a tile border.
    paint_pattern(&mut s, 230, 10, 60, 30);
    let before = dump(&mut s, 300, 60);
    let mut snapshot = StrokeSnapshot::new(surface());
    let mut carry = SmudgeCarry::default();
    let mut painted = BTreeSet::new();
    for x in [240.5_f32, 246.5, 252.5, 258.5, 264.5, 270.5] {
        let out = smudge(
            &mut s,
            &mut carry,
            (x, 24.5),
            8.0,
            0.7,
            Some(&mut snapshot),
            None,
        );
        painted.extend(out.painted().iter().copied());
    }
    assert!(painted.len() >= 2, "{painted:?}");
    let captured: BTreeSet<TileId> = snapshot.captured().collect();
    assert_eq!(captured, painted, "captured and painted are one set");
    assert_ne!(dump(&mut s, 300, 60), before);
    let mut history = PixelHistory::new();
    assert!(history.push(snapshot));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 60), before);
}

#[test]
fn a_dab_over_the_patch_cap_is_refused_whole_and_keeps_the_carry() {
    let (_dir, mut s) = store();
    put(&mut s, 100, 100, px(1.0, 1.0, 1.0, 1.0));
    let radius: f32 = 600.0;
    assert!((2.0 * radius + 3.0).powi(2) > MAX_SMUDGE_PATCH_TEXELS as f32);
    let mut carry = SmudgeCarry::default();
    let out = smudge(&mut s, &mut carry, (700.5, 700.5), radius, 1.0, None, None);
    assert!(out.painted().is_empty());
    assert!(out.is_complete());
    assert!(carry.is_empty(), "a refused dab picks nothing up");
    // A dab under it works.
    let ok = smudge(&mut s, &mut carry, (100.5, 100.5), 20.0, 1.0, None, None);
    assert!(ok.is_complete());
    assert!(!carry.is_empty());
}

#[test]
fn a_changed_radius_makes_the_finger_land_again() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 8, 8, 50, 30);
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut s, &mut carry, (20.5, 20.5), 5.0, 1.0, None, None);
    let before = dump(&mut s, 64, 40);
    let out = smudge(&mut s, &mut carry, (30.5, 20.5), 9.0, 1.0, None, None);
    assert!(out.painted().is_empty(), "the bigger dab only picks up");
    assert_eq!(dump(&mut s, 64, 40), before);
    assert_eq!(
        carry.texel_at(0, 0),
        Some(pattern_colour(30, 20).map(f16::to_f32))
    );
}

#[test]
fn degenerate_input_changes_nothing_and_never_makes_nan() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 0, 0, 20, 20);
    let before = dump(&mut s, 32, 32);
    let mut carry = SmudgeCarry::default();
    for (c, r, k) in [
        ((f32::NAN, 5.0), 3.0, 1.0),
        ((5.0, f32::INFINITY), 3.0, 1.0),
        ((5.0, 5.0), 0.0, 1.0),
        ((5.0, 5.0), f32::NAN, 1.0),
        ((5.0, 5.0), -1.0, 1.0),
        ((5.0, 5.0), 3.0, f32::NAN),
        ((1e30, 5.0), 3.0, 1.0),
        ((5.0, -1e30), 3.0, 1.0),
    ] {
        let out = smudge(&mut s, &mut carry, c, r, k, None, None);
        assert!(out.painted().is_empty(), "{c:?} {r} {k}");
        assert!(out.is_complete());
    }
    assert!(carry.is_empty(), "no refused dab picked anything up");
    assert_eq!(dump(&mut s, 32, 32), before);

    // An out-of-range strength is clamped, an infinite one too.
    for k in [-3.0_f32, 7.0, f32::INFINITY, f32::NEG_INFINITY] {
        let (_d, mut t) = store();
        paint_pattern(&mut t, 0, 0, 40, 20);
        let mut carry = SmudgeCarry::default();
        let _ = smudge(&mut t, &mut carry, (10.5, 10.5), 4.0, k, None, None);
        let _ = smudge(&mut t, &mut carry, (16.5, 10.5), 4.0, k, None, None);
        for y in 0..20 {
            for x in 0..40 {
                for v in texel(&mut t, x, y) {
                    assert!(v.is_finite(), "strength {k} at ({x}, {y})");
                }
            }
        }
    }

    // A stored NaN or infinity is picked up as nothing, never spread.
    let (_d, mut t) = store();
    paint_pattern(&mut t, 0, 0, 40, 20);
    put(
        &mut t,
        10,
        10,
        [f16::NAN, f16::INFINITY, f16::NEG_INFINITY, f16::NAN],
    );
    let mut carry = SmudgeCarry::default();
    let _ = smudge(&mut t, &mut carry, (10.5, 10.5), 4.0, 0.5, None, None);
    let _ = smudge(&mut t, &mut carry, (16.5, 10.5), 4.0, 0.5, None, None);
    let _ = smudge(&mut t, &mut carry, (22.5, 10.5), 4.0, 0.5, None, None);
    for y in 0..20 {
        for x in 12..40 {
            for v in texel(&mut t, x, y) {
                assert!(v.is_finite(), "({x}, {y})");
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
    let mut carry = SmudgeCarry::default();
    // The patch around (250, 10) reaches the broken tile 0.
    let out = smudge_dab_covered(
        &mut s,
        surf,
        (260.5, 10.5),
        6.0,
        1.0,
        false,
        &mut carry,
        None,
        None,
    );
    assert!(out.painted().is_empty());
    assert_eq!(out.failed().len(), 1);
    assert!(carry.is_empty(), "the carry is untouched by a refused dab");
}
