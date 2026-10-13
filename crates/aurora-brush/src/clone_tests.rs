//! The Clone Stamp's dab (0.221.0) at the brush level: an exact pattern copy,
//! the offset's sign, the footprint, sampling the stroke's start state,
//! selection coverage, the transparency lock, no phantom tiles, undo and the
//! patch cap.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, DabOutcome, MAX_CLONE_PATCH_TEXELS, PixelHistory, StrokeSnapshot, clone_dab_covered,
    dab_falloff, stamp_dab,
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

/// A block of distinct, opaque texels: `(x, y)` carries a colour no other
/// texel of the block does.
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

fn clone(
    s: &mut TileStore,
    center: (f32, f32),
    radius: f32,
    offset: (i64, i64),
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    clone_dab_covered(
        s,
        surface(),
        center,
        radius,
        offset,
        false,
        snapshot,
        coverage,
    )
}

fn changed_texels(s: &mut TileStore, w: u32, h: u32, before: &[[u16; 4]]) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    for y in 0..h {
        for x in 0..w {
            if bits(texel(s, x, y)) != before[(y * w + x) as usize] {
                out.insert((x, y));
            }
        }
    }
    out
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

#[test]
fn a_known_pattern_is_copied_exactly_with_the_offset_added_to_the_destination() {
    let (_dir, mut s) = store();
    // Source block at x 80..140, y 20..80; destination near (20, 20).
    paint_pattern(&mut s, 80, 20, 60, 60);
    let offset = (80, 20); // destination + offset = source
    let center = (20.5, 20.5);
    let radius = 6.0;
    let out = clone(&mut s, center, radius, offset, None, None);
    assert!(out.is_complete());
    for y in 0..48u32 {
        for x in 0..48u32 {
            let d = ((x as f32 + 0.5 - center.0).hypot(y as f32 + 0.5 - center.1)).min(100.0);
            let got = texel(&mut s, x, y);
            if d > radius {
                assert_eq!(bits(got), bits([f16::ZERO; 4]), "outside at ({x}, {y})");
                continue;
            }
            let a = dab_falloff(d, radius);
            let source = pattern_colour((x as i64 + offset.0) as u32, (y as i64 + offset.1) as u32);
            let want: Vec<u16> = (0..4)
                .map(|c| {
                    if a >= 1.0 {
                        source[c].to_bits()
                    } else {
                        f16::from_f32(source[c].to_f32() * a).to_bits()
                    }
                })
                .collect();
            assert_eq!(
                bits(got).to_vec(),
                want,
                "texel ({x}, {y}) copies source ({}, {})",
                x as i64 + offset.0,
                y as i64 + offset.1
            );
        }
    }
    // The centre texel has falloff exactly 1: an exact copy.
    assert_eq!(bits(texel(&mut s, 20, 20)), bits(pattern_colour(100, 40)));
    // The tile at the destination holds nothing outside the footprint.
}

#[test]
fn the_offset_sign_picks_the_side_the_source_is_read_from() {
    let (_dir, mut s) = store();
    put(&mut s, 30, 10, px(1.0, 0.0, 0.0, 1.0)); // right of (20, 10)
    put(&mut s, 10, 10, px(0.0, 0.0, 1.0, 1.0)); // left of it
    let _ = clone(&mut s, (20.5, 10.5), 2.0, (10, 0), None, None);
    assert_eq!(bits(texel(&mut s, 20, 10)), bits(px(1.0, 0.0, 0.0, 1.0)));
    // Another stroke elsewhere with the opposite sign reads the other side.
    put(&mut s, 50, 50, px(0.0, 0.0, 1.0, 1.0));
    put(&mut s, 70, 50, px(1.0, 0.0, 0.0, 1.0));
    let _ = clone(&mut s, (60.5, 50.5), 2.0, (-10, 0), None, None);
    assert_eq!(bits(texel(&mut s, 60, 50)), bits(px(0.0, 0.0, 1.0, 1.0)));
}

#[test]
fn the_footprint_is_the_brushs_soft_dab() {
    // Cloning a fully opaque white source touches exactly the texels the
    // Brush's own dab does, with alpha equal to the Brush's falloff.
    let (_dir, mut s) = store();
    for y in 0..64 {
        for x in 64..128 {
            put(&mut s, x, y, px(1.0, 1.0, 1.0, 1.0));
        }
    }
    let center = (20.3, 24.8);
    let _ = clone(&mut s, center, 9.0, (64, 0), None, None);
    let other = SurfaceId::from_raw(1);
    let _ = stamp_dab(&mut s, other, center, 9.0, [1.0, 1.0, 1.0], None);
    for y in 0..48u32 {
        for x in 0..48u32 {
            let id = TileId { x: 0, y: 0 };
            let Ok(brush) = s.get(other, id) else {
                unreachable!("tile");
            };
            let index = ((y * TILE + x) as usize) * CHANNELS;
            let brush_alpha = brush.texels()[index + 3];
            let cloned_alpha = texel(&mut s, x, y)[3];
            assert_eq!(cloned_alpha.to_bits(), brush_alpha.to_bits(), "({x}, {y})");
        }
    }
}

#[test]
fn a_stroke_samples_the_layer_as_it_was_when_the_stroke_began() {
    let (_dir, mut s) = store();
    let red = px(1.0, 0.0, 0.0, 1.0);
    let blue = px(0.0, 0.0, 1.0, 1.0);
    // Destination of the first dab (around x 30) is blue; its source
    // (x + 8) is red. The second dab (around x 22) reads x + 8 = x 30:
    // the first dab's output.
    for y in 8..14 {
        for x in 26..34 {
            put(&mut s, x, y, blue);
        }
        for x in 34..42 {
            put(&mut s, x, y, red);
        }
    }
    let mut snapshot = StrokeSnapshot::new(surface());
    let first = clone(&mut s, (30.5, 10.5), 3.0, (8, 0), Some(&mut snapshot), None);
    assert!(!first.painted().is_empty());
    assert_eq!(
        bits(texel(&mut s, 30, 10)),
        bits(red),
        "the first dab copied red over the blue"
    );
    let second = clone(&mut s, (22.5, 10.5), 3.0, (8, 0), Some(&mut snapshot), None);
    let _ = second;
    // Stroke-start sampling: the second dab reads (30, 10) as it was before
    // this stroke wrote it, blue. Live sampling would read red.
    assert_eq!(
        bits(texel(&mut s, 22, 10)),
        bits(blue),
        "the second dab did not re-clone the stroke's own output"
    );
}

#[test]
fn without_a_snapshot_the_source_is_the_live_layer() {
    // The same arrangement with no stroke snapshot reads the live (already
    // overwritten) texel: this is what the snapshot read prevents.
    let (_dir, mut s) = store();
    let red = px(1.0, 0.0, 0.0, 1.0);
    let blue = px(0.0, 0.0, 1.0, 1.0);
    for y in 8..14 {
        for x in 26..34 {
            put(&mut s, x, y, blue);
        }
        for x in 34..42 {
            put(&mut s, x, y, red);
        }
    }
    let _ = clone(&mut s, (30.5, 10.5), 3.0, (8, 0), None, None);
    let _ = clone(&mut s, (22.5, 10.5), 3.0, (8, 0), None, None);
    assert_eq!(bits(texel(&mut s, 22, 10)), bits(red));
}

#[test]
fn a_dab_whose_source_overlaps_its_own_footprint_does_not_clone_itself() {
    // Offset (2, 0) with radius 6: the footprint overlaps its own source.
    // Each destination texel must get the texel two to its right as it was
    // before the dab, not a texel the dab already wrote.
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 4, 4, 24, 24);
    let before = dump(&mut s, 32, 32);
    let center = (16.5, 16.5);
    let _ = clone(&mut s, center, 6.0, (2, 0), None, None);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let d = (x as f32 + 0.5 - center.0).hypot(y as f32 + 0.5 - center.1);
            if d > 6.0 {
                continue;
            }
            let a = dab_falloff(d, 6.0);
            let old = before[(y * 32 + x) as usize].map(|b| f16::from_bits(b).to_f32());
            let src = before[(y * 32 + x + 2) as usize].map(|b| f16::from_bits(b).to_f32());
            let want: Vec<u16> = (0..4)
                .map(|c| {
                    if a >= 1.0 {
                        f16::from_f32(src[c]).to_bits()
                    } else {
                        f16::from_f32(old[c] + (src[c] - old[c]) * a).to_bits()
                    }
                })
                .collect();
            assert_eq!(bits(texel(&mut s, x, y)).to_vec(), want, "({x}, {y})");
        }
    }
}

#[test]
fn a_source_with_no_pixels_clones_transparency_and_creates_no_tile() {
    let (_dir, mut s) = store();
    // An absent destination tile and an empty source: nothing is created.
    let out = clone(&mut s, (300.5, 300.5), 5.0, (-100, -100), None, None);
    assert!(out.painted().is_empty());
    assert!(!s.contains_tile(surface(), TileId { x: 1, y: 1 }));
    // A source left of the surface origin is empty too.
    put(&mut s, 3, 3, px(0.5, 0.5, 0.5, 1.0));
    let out = clone(&mut s, (3.5, 3.5), 2.0, (-50, -50), None, None);
    // The centre texel is faded to nothing: transparency is cloned.
    assert!(!out.painted().is_empty());
    assert_eq!(bits(texel(&mut s, 3, 3)), bits([f16::ZERO; 4]));
}

#[test]
fn a_source_tile_is_never_created_by_reading_it() {
    let (_dir, mut s) = store();
    put(&mut s, 5, 5, px(1.0, 1.0, 1.0, 1.0));
    let _ = clone(&mut s, (5.5, 5.5), 3.0, (1000, 1000), None, None);
    assert!(!s.contains_tile(surface(), TileId { x: 3, y: 3 }));
    assert!(!s.contains_tile(surface(), TileId { x: 4, y: 4 }));
}

#[test]
fn repeated_dabs_converge_on_the_source_and_never_overshoot() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 60, 10, 20, 20);
    let offset = (50, -5);
    let mut snapshot = StrokeSnapshot::new(surface());
    let _ = clone(&mut s, (20.5, 20.5), 5.0, offset, Some(&mut snapshot), None);
    let centre_after_one = bits(texel(&mut s, 20, 20));
    assert_eq!(
        centre_after_one,
        bits(pattern_colour(70, 15)),
        "falloff 1 is an exact copy"
    );
    let distance = |s: &mut TileStore| -> f32 {
        let mut total = 0.0;
        for y in 14..28u32 {
            for x in 14..28u32 {
                let src =
                    pattern_colour((x as i64 + offset.0) as u32, (y as i64 + offset.1) as u32);
                let got = texel(s, x, y);
                let d = (x as f32 + 0.5 - 20.5).hypot(y as f32 + 0.5 - 20.5);
                if d <= 5.0 {
                    for c in 0..4 {
                        total += (got[c].to_f32() - src[c].to_f32()).abs();
                    }
                }
            }
        }
        total
    };
    let mut last = distance(&mut s);
    for _ in 0..6 {
        let _ = clone(&mut s, (20.5, 20.5), 5.0, offset, Some(&mut snapshot), None);
        let now = distance(&mut s);
        assert!(
            now <= last + 1e-3,
            "{now} > {last}: moved away from the source"
        );
        last = now;
    }
    assert_eq!(bits(texel(&mut s, 20, 20)), centre_after_one);
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
fn a_selection_restricts_the_clone_and_blends_by_coverage() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 100, 0, 40, 40);
    // Only x 20..24 is selected.
    let cover = RectCov(Rect {
        x: 20,
        y: 0,
        width: 4,
        height: 64,
    });
    let before = dump(&mut s, 48, 48);
    let _ = clone(&mut s, (20.5, 20.5), 8.0, (100, 0), None, Some(&cover));
    let changed = changed_texels(&mut s, 48, 48, &before);
    assert!(!changed.is_empty());
    for &(x, _) in &changed {
        assert!(
            (20..24).contains(&x),
            "texel x {x} is outside the selection"
        );
    }
    // Half coverage: the centre texel lands halfway to the source.
    let (_dir, mut h) = store();
    paint_pattern(&mut h, 100, 0, 40, 40);
    put(&mut h, 20, 20, px(0.0, 0.0, 0.0, 1.0));
    let _ = clone(
        &mut h,
        (20.5, 20.5),
        4.0,
        (100, 0),
        None,
        Some(&Constant(0.5)),
    );
    let src = pattern_colour(120, 20);
    let got = texel(&mut h, 20, 20);
    for c in 0..3 {
        let want = f16::from_f32(src[c].to_f32() * 0.5);
        assert_eq!(got[c].to_bits(), want.to_bits(), "channel {c}");
    }
}

#[test]
fn an_excluded_tile_is_neither_read_nor_created() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 100, 0, 40, 40);
    let cover = RectCov(Rect {
        x: 500,
        y: 500,
        width: 4,
        height: 4,
    });
    let out = clone(&mut s, (20.5, 20.5), 6.0, (100, 0), None, Some(&cover));
    assert!(out.painted().is_empty());
    assert!(
        !s.contains_tile(surface(), TileId { x: 0, y: 0 }) || {
            // The source tile exists (it holds the pattern); the destination
            // is the same tile here, so check the destination is untouched.
            bits(texel(&mut s, 20, 20)) == bits([f16::ZERO; 4])
        }
    );
}

#[test]
fn the_transparency_lock_keeps_alpha_and_moves_only_colour() {
    let (_dir, mut s) = store();
    // Source: opaque green. Destination: half-transparent red and one empty
    // texel.
    for x in 100..110 {
        put(&mut s, x, 10, px(0.0, 1.0, 0.0, 1.0));
    }
    put(&mut s, 20, 10, px(0.5, 0.0, 0.0, 0.5));
    let before_alpha = texel(&mut s, 20, 10)[3].to_bits();
    let out = clone_dab_covered(
        &mut s,
        surface(),
        (20.5, 10.5),
        3.0,
        (80, 0),
        true,
        None,
        None,
    );
    assert!(!out.painted().is_empty());
    let got = texel(&mut s, 20, 10);
    assert_eq!(got[3].to_bits(), before_alpha, "alpha is kept bit for bit");
    // Full falloff at the centre: colour is the source's straight colour
    // times the destination's alpha.
    assert_eq!(bits(got)[..3], bits(px(0.0, 0.5, 0.0, 0.5))[..3]);
    // A fully transparent destination texel is left alone.
    assert_eq!(bits(texel(&mut s, 21, 10)), bits([f16::ZERO; 4]));
    // And no tile is created for one that is wholly transparent.
    let (_dir2, mut t) = store();
    put(&mut t, 100, 10, px(0.0, 1.0, 0.0, 1.0));
    let out = clone_dab_covered(
        &mut t,
        surface(),
        (400.5, 10.5),
        3.0,
        (-300, 0),
        true,
        None,
        None,
    );
    assert!(out.painted().is_empty());
    assert!(!t.contains_tile(surface(), TileId { x: 1, y: 0 }));
}

#[test]
fn undo_restores_exactly_the_captured_tiles() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 100, 100, 40, 40);
    // A destination straddling a tile border.
    paint_pattern(&mut s, 250, 10, 12, 12);
    let before = dump(&mut s, 300, 40);
    let mut snapshot = StrokeSnapshot::new(surface());
    let mut painted = BTreeSet::new();
    for centre in [(254.5, 15.5), (260.5, 15.5)] {
        let out = clone(&mut s, centre, 6.0, (-150, 90), Some(&mut snapshot), None);
        painted.extend(out.painted().iter().copied());
    }
    // The destination (254..260 x 10..21 shifted) reads the pattern at
    // 100..140? offset (-150, 90) maps (254, 15) to (104, 105): inside it.
    assert!(!painted.is_empty());
    let captured: BTreeSet<TileId> = snapshot.captured().collect();
    assert_eq!(captured, painted, "captured and painted are one set");
    assert_ne!(dump(&mut s, 300, 40), before);
    let mut history = PixelHistory::new();
    assert!(history.push(snapshot));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 40), before);
}

#[test]
fn a_dab_over_the_patch_cap_is_refused_whole() {
    let (_dir, mut s) = store();
    put(&mut s, 100, 100, px(1.0, 1.0, 1.0, 1.0));
    let radius: f32 = 600.0;
    assert!((2.0 * radius + 1.0).powi(2) > MAX_CLONE_PATCH_TEXELS as f32);
    let out = clone(&mut s, (700.5, 700.5), radius, (0, 0), None, None);
    assert!(out.painted().is_empty());
    assert!(out.failed().is_empty());
    // A dab under it works.
    let ok = clone(&mut s, (300.5, 300.5), 20.0, (-200, -200), None, None);
    assert!(!ok.painted().is_empty());
}

#[test]
fn a_dab_without_geometry_does_nothing() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 0, 0, 10, 10);
    let before = dump(&mut s, 16, 16);
    for (c, r) in [
        ((f32::NAN, 5.0), 3.0),
        ((5.0, 5.0), 0.0),
        ((5.0, 5.0), f32::NAN),
        ((5.0, 5.0), -1.0),
    ] {
        assert!(clone(&mut s, c, r, (3, 3), None, None).painted().is_empty());
    }
    assert_eq!(dump(&mut s, 16, 16), before);
}

#[test]
fn an_unreadable_source_tile_refuses_the_dab_instead_of_erasing() {
    let broken = TileId { x: 0, y: 0 };
    let healthy = TileId { x: 1, y: 0 };
    let (_dir, mut s) = crate::test_support::store_with_a_broken_tile(broken, healthy);
    let surf = crate::test_support::surface();
    // Destination in the healthy tile, source in the broken one.
    let out = clone_dab_covered(
        &mut s,
        surf,
        (260.5, 10.5),
        3.0,
        (-250, 0),
        false,
        None,
        None,
    );
    assert!(out.painted().is_empty());
    assert_eq!(out.failed().len(), 1);
}
