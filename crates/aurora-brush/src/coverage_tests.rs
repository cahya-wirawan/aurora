//! Painting through a selection (0.211.0): the tools at the brush level.
//!
//! Every tool is checked three ways: with no coverage (must be the old
//! code's output bit for bit), with coverage that is `1` everywhere but
//! reached through the per-texel path (also bit for bit), and with real
//! coverage (rectangles, inverted, constant fractions and a function that
//! is not a rectangle at all).

use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::{
    Coverage, FillMode, FloodSpec, GradientKind, GradientSpec, MAX_EXAMINED_TILES, MAX_FILL_TILES,
    PixelHistory, StrokeSnapshot, erase_dab, erase_dab_covered, flood_fill, flood_fill_covered,
    gradient_fill, gradient_fill_covered, stamp_dab, stamp_dab_covered, stamp_dab_preserving_alpha,
    stamp_dab_preserving_alpha_covered,
};

fn sorted(tiles: impl Iterator<Item = TileId>) -> Vec<TileId> {
    let mut v: Vec<TileId> = tiles.collect();
    v.sort_by_key(|t| (t.y, t.x));
    v
}

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

fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Every tile of the 3 x 3 block, `None` when the store does not hold it.
fn dump(store: &mut TileStore) -> Vec<Option<Vec<u16>>> {
    let mut out = Vec::new();
    for ty in 0..3 {
        for tx in 0..3 {
            let id = TileId { x: tx, y: ty };
            if store.contains_tile(surface(), id) {
                let Ok(tile) = store.get(surface(), id) else {
                    unreachable!("tile");
                };
                out.push(Some(tile.texels().iter().map(|v| v.to_bits()).collect()));
            } else {
                out.push(None);
            }
        }
    }
    out
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

fn bits(t: [f16; CHANNELS]) -> [u16; CHANNELS] {
    [
        t[0].to_bits(),
        t[1].to_bits(),
        t[2].to_bits(),
        t[3].to_bits(),
    ]
}

fn h(v: f32) -> f16 {
    f16::from_f32(v)
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1_u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// A rectangle, optionally inverted (the shape `aurora-doc`'s selection has).
struct RectCov {
    rect: Rect,
    inverted: bool,
}

impl Coverage for RectCov {
    fn bounds(&self) -> Option<Rect> {
        (!self.inverted).then_some(self.rect)
    }
    fn at(&self, x: i64, y: i64) -> f32 {
        if self.rect.contains_point(x, y) != self.inverted {
            1.0
        } else {
            0.0
        }
    }
}

/// The same coverage everywhere, no bounds: forces the per-texel path.
struct Constant(f32);

impl Coverage for Constant {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    fn at(&self, _: i64, _: i64) -> f32 {
        self.0
    }
}

/// Not a rectangle at all: a smooth ramp in `x`, `0` left of 100 and `1`
/// from 356 on. Proves the tools need nothing rectangular.
struct Ramp;

impl Coverage for Ramp {
    fn bounds(&self) -> Option<Rect> {
        None
    }
    #[allow(clippy::cast_precision_loss)]
    fn at(&self, x: i64, _: i64) -> f32 {
        ((x - 100) as f32 / 256.0).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Brush,
    Preserve,
    Eraser,
}

/// One dab of `kind` into `store` through `coverage`, plain when `None` and
/// `plain` is set (the pre-0.211.0 entry points).
#[allow(clippy::too_many_arguments)]
fn dab(
    store: &mut TileStore,
    kind: Kind,
    center: (f32, f32),
    radius: f32,
    colour: [f32; 3],
    snapshot: &mut StrokeSnapshot,
    coverage: Option<&dyn Coverage>,
    plain: bool,
) -> Vec<TileId> {
    let outcome = match (kind, plain) {
        (Kind::Brush, true) => stamp_dab(store, surface(), center, radius, colour, Some(snapshot)),
        (Kind::Preserve, true) => {
            stamp_dab_preserving_alpha(store, surface(), center, radius, colour, Some(snapshot))
        }
        (Kind::Eraser, true) => erase_dab(store, surface(), center, radius, Some(snapshot)),
        (Kind::Brush, false) => stamp_dab_covered(
            store,
            surface(),
            center,
            radius,
            colour,
            Some(snapshot),
            coverage,
        ),
        (Kind::Preserve, false) => stamp_dab_preserving_alpha_covered(
            store,
            surface(),
            center,
            radius,
            colour,
            Some(snapshot),
            coverage,
        ),
        (Kind::Eraser, false) => {
            erase_dab_covered(store, surface(), center, radius, Some(snapshot), coverage)
        }
    };
    assert!(outcome.is_complete());
    outcome.painted().to_vec()
}

/// A deterministic random sequence of dabs. Returns the store dump, every
/// outcome's painted list and the captured tiles.
fn run_sequence(
    seed: u64,
    coverage: Option<&dyn Coverage>,
    plain: bool,
) -> (Vec<Option<Vec<u16>>>, Vec<Vec<TileId>>, Vec<TileId>) {
    let (_dir, mut s) = store();
    let mut rng = Rng(seed);
    let mut painted = Vec::new();
    let mut snapshot = StrokeSnapshot::new(surface());
    // Content first, always plain, so every run starts from the same pixels.
    let mut setup = StrokeSnapshot::new(surface());
    for _ in 0..24 {
        let c = (rng.range(0.0, 600.0), rng.range(0.0, 600.0));
        let radius = rng.range(2.0, 70.0);
        let colour = [rng.unit(), rng.unit(), rng.unit()];
        dab(
            &mut s,
            Kind::Brush,
            c,
            radius,
            colour,
            &mut setup,
            None,
            true,
        );
    }
    for _ in 0..60 {
        let c = (rng.range(-20.0, 620.0), rng.range(-20.0, 620.0));
        let radius = rng.range(0.5, 90.0);
        let colour = [rng.unit(), rng.unit(), rng.unit()];
        let kind = match rng.next() % 3 {
            0 => Kind::Brush,
            1 => Kind::Preserve,
            _ => Kind::Eraser,
        };
        painted.push(dab(
            &mut s,
            kind,
            c,
            radius,
            colour,
            &mut snapshot,
            coverage,
            plain,
        ));
    }
    let captured = sorted(snapshot.captured());
    (dump(&mut s), painted, captured)
}

#[test]
fn no_coverage_and_full_coverage_are_bit_identical_to_the_plain_dabs() {
    for seed in [1_u64, 7, 99, 12_345, 0xDEAD_BEEF] {
        let plain = run_sequence(seed, None, true);
        let none = run_sequence(seed, None, false);
        assert!(plain == none, "seed {seed}: None must be the old path");
        let one = Constant(1.0);
        let full = run_sequence(seed, Some(&one), false);
        assert_eq!(
            plain, full,
            "seed {seed}: coverage 1 must write the same bits"
        );
        let everything = RectCov {
            rect: r(-100, -100, 2000, 2000),
            inverted: false,
        };
        let rect = run_sequence(seed, Some(&everything), false);
        assert!(plain == rect, "seed {seed}: a selection covering all of it");
        let nothing_inverted = RectCov {
            rect: r(5000, 5000, 10, 10),
            inverted: true,
        };
        let inv = run_sequence(seed, Some(&nothing_inverted), false);
        assert!(plain == inv, "seed {seed}: an inverted selection far away");
        let (dump, painted, _) = &plain;
        assert!(
            dump.iter().flatten().count() > 0 && painted.iter().any(|p| !p.is_empty()),
            "the sequence must paint something or the differential is vacuous"
        );
    }
}

/// Paints one dab of `kind` into a copy of the same starting content with
/// and without `coverage`, returning both dumps and the plain one's start.
fn start_content(s: &mut TileStore) {
    // A smooth, partly transparent grey block over x 0..400, y 0..400 so
    // brush (max-alpha) and eraser both have something to do.
    for y in (0..400).step_by(1) {
        for x in (0..400).step_by(1) {
            let a = 0.15 + 0.5 * ((x + y) % 7) as f32 / 7.0;
            put(s, x, y, [h(0.2 * a), h(0.3 * a), h(0.4 * a), h(a)]);
        }
    }
}

#[test]
fn a_rectangle_clips_each_dab_kind_exactly_with_inclusive_left_top_and_exclusive_right_bottom() {
    // The rectangle straddles the tile boundary at 256 on both axes.
    let rect = r(200, 210, 100, 90);
    let cov = RectCov {
        rect,
        inverted: false,
    };
    for kind in [Kind::Brush, Kind::Preserve, Kind::Eraser] {
        let (_d1, mut plain) = store();
        let (_d2, mut masked) = store();
        let (_d3, mut original) = store();
        for s in [&mut plain, &mut masked, &mut original] {
            start_content(s);
        }
        let mut snap = StrokeSnapshot::new(surface());
        let mut snap2 = StrokeSnapshot::new(surface());
        let mut changed_inside = 0;
        for center in [(250.0, 255.0), (290.0, 295.0), (205.0, 215.0)] {
            dab(
                &mut plain,
                kind,
                center,
                60.0,
                [1.0, 0.0, 0.0],
                &mut snap,
                None,
                true,
            );
            dab(
                &mut masked,
                kind,
                center,
                60.0,
                [1.0, 0.0, 0.0],
                &mut snap2,
                Some(&cov),
                false,
            );
        }
        for y in 0..400_u32 {
            for x in 0..400_u32 {
                let inside = rect.contains_point(i64::from(x), i64::from(y));
                let want = if inside {
                    texel(&mut plain, x, y)
                } else {
                    texel(&mut original, x, y)
                };
                assert_eq!(
                    bits(texel(&mut masked, x, y)),
                    bits(want),
                    "texel ({x}, {y}) inside={inside}"
                );
                if inside && bits(texel(&mut plain, x, y)) != bits(texel(&mut original, x, y)) {
                    changed_inside += 1;
                }
            }
        }
        assert!(changed_inside > 500, "the dabs must change pixels inside");
        // The four edge lines, explicitly: the last column/row inside
        // changes (somewhere), the first one outside never does.
        let outside_right = (0..400_u32)
            .any(|y| bits(texel(&mut masked, 300, y)) != bits(texel(&mut original, 300, y)));
        let outside_bottom = (0..400_u32)
            .any(|x| bits(texel(&mut masked, x, 300)) != bits(texel(&mut original, x, 300)));
        let outside_left = (0..400_u32)
            .any(|y| bits(texel(&mut masked, 199, y)) != bits(texel(&mut original, 199, y)));
        let outside_top = (0..400_u32)
            .any(|x| bits(texel(&mut masked, x, 209)) != bits(texel(&mut original, x, 209)));
        assert!(!outside_right && !outside_bottom && !outside_left && !outside_top);
        let inside_right = (210..300_u32)
            .any(|y| bits(texel(&mut masked, 299, y)) != bits(texel(&mut original, 299, y)));
        let inside_left = (210..300_u32)
            .any(|y| bits(texel(&mut masked, 200, y)) != bits(texel(&mut original, 200, y)));
        assert!(
            inside_right && inside_left,
            "the first/last column is inside"
        );
    }
}

#[test]
fn an_inverted_rectangle_paints_everywhere_except_inside_it() {
    let rect = r(150, 150, 100, 100);
    let cov = RectCov {
        rect,
        inverted: true,
    };
    for kind in [Kind::Brush, Kind::Eraser] {
        let (_d1, mut plain) = store();
        let (_d2, mut masked) = store();
        let (_d3, mut original) = store();
        for s in [&mut plain, &mut masked, &mut original] {
            start_content(s);
        }
        let mut a = StrokeSnapshot::new(surface());
        let mut b = StrokeSnapshot::new(surface());
        dab(
            &mut plain,
            kind,
            (200.0, 200.0),
            90.0,
            [0.0, 1.0, 0.0],
            &mut a,
            None,
            true,
        );
        dab(
            &mut masked,
            kind,
            (200.0, 200.0),
            90.0,
            [0.0, 1.0, 0.0],
            &mut b,
            Some(&cov),
            false,
        );
        let mut changed_outside = 0;
        for y in 0..400_u32 {
            for x in 0..400_u32 {
                let inside = rect.contains_point(i64::from(x), i64::from(y));
                let want = if inside {
                    texel(&mut original, x, y)
                } else {
                    texel(&mut plain, x, y)
                };
                assert_eq!(bits(texel(&mut masked, x, y)), bits(want), "({x}, {y})");
                if !inside && bits(want) != bits(texel(&mut original, x, y)) {
                    changed_outside += 1;
                }
            }
        }
        assert!(changed_outside > 300);
    }
}

/// `old + (new - old) * c` the way the tools promise it, from the plain
/// result: where the plain dab changed the texel, the covered one sits
/// `c` of the way there; elsewhere it is untouched.
fn lerp_expected(old: [f16; 4], plain: [f16; 4], c: f32) -> [u16; 4] {
    if bits(old) == bits(plain) {
        return bits(old);
    }
    let mut out = [0_u16; 4];
    for i in 0..4 {
        let (o, n) = (old[i].to_f32(), plain[i].to_f32());
        out[i] = if old[i].to_bits() == plain[i].to_bits() {
            old[i].to_bits()
        } else {
            h(o + (n - o) * c).to_bits()
        };
    }
    out
}

#[test]
fn fractional_coverage_is_an_exact_lerp_of_the_old_and_the_new_texel() {
    for c in [0.5_f32, 0.25, 0.9] {
        let cov = Constant(c);
        for kind in [Kind::Brush, Kind::Preserve] {
            let (_d1, mut plain) = store();
            let (_d2, mut masked) = store();
            let (_d3, mut original) = store();
            for s in [&mut plain, &mut masked, &mut original] {
                start_content(s);
            }
            let mut a = StrokeSnapshot::new(surface());
            let mut b = StrokeSnapshot::new(surface());
            dab(
                &mut plain,
                kind,
                (120.0, 130.0),
                50.0,
                [1.0, 0.5, 0.0],
                &mut a,
                None,
                true,
            );
            dab(
                &mut masked,
                kind,
                (120.0, 130.0),
                50.0,
                [1.0, 0.5, 0.0],
                &mut b,
                Some(&cov),
                false,
            );
            let mut blended = 0;
            for y in 60..200_u32 {
                for x in 60..200_u32 {
                    let (o, p) = (texel(&mut original, x, y), texel(&mut plain, x, y));
                    assert_eq!(
                        bits(texel(&mut masked, x, y)),
                        lerp_expected(o, p, c),
                        "c {c} ({x}, {y})"
                    );
                    let far = (0..4).any(|i| (o[i].to_f32() - p[i].to_f32()).abs() > 0.05);
                    if bits(o) != bits(p) {
                        blended += 1;
                    }
                    if far {
                        assert_ne!(
                            bits(texel(&mut masked, x, y)),
                            bits(p),
                            "a fractional blend is not the plain value"
                        );
                    }
                }
            }
            assert!(blended > 1000, "c {c}: the dab must change pixels");
        }
    }
}

#[test]
fn the_blend_weights_the_new_value_not_the_old_one() {
    // A reversed lerp would give the old value coverage c: with c = 0.25 a
    // texel moves a quarter of the way to the new colour, not three.
    let (_d, mut s) = store();
    put(&mut s, 10, 10, [h(0.0), h(0.0), h(0.0), h(0.1)]);
    let cov = Constant(0.25);
    let mut snap = StrokeSnapshot::new(surface());
    dab(
        &mut s,
        Kind::Brush,
        (10.5, 10.5),
        3.0,
        [1.0, 1.0, 1.0],
        &mut snap,
        Some(&cov),
        false,
    );
    let t = texel(&mut s, 10, 10);
    // At dead centre the falloff is 1: new = [1, 1, 1, 1].
    assert_eq!(
        bits(t),
        [
            h(0.25).to_bits(),
            h(0.25).to_bits(),
            h(0.25).to_bits(),
            h(0.325).to_bits()
        ]
    );
}

#[test]
fn the_eraser_scales_its_erase_amount_by_coverage() {
    let (_d, mut s) = store();
    for y in 0..40 {
        for x in 0..40 {
            put(&mut s, x, y, [h(0.5), h(0.5), h(0.5), h(1.0)]);
        }
    }
    let (_d2, mut full) = store();
    for y in 0..40 {
        for x in 0..40 {
            put(&mut full, x, y, [h(0.5), h(0.5), h(0.5), h(1.0)]);
        }
    }
    let mut a = StrokeSnapshot::new(surface());
    let mut b = StrokeSnapshot::new(surface());
    dab(
        &mut full,
        Kind::Eraser,
        (20.0, 20.0),
        15.0,
        [0.0; 3],
        &mut a,
        None,
        true,
    );
    let half = Constant(0.5);
    dab(
        &mut s,
        Kind::Eraser,
        (20.0, 20.0),
        15.0,
        [0.0; 3],
        &mut b,
        Some(&half),
        false,
    );
    let mut checked = 0;
    for y in 5..35_u32 {
        for x in 5..35_u32 {
            let erased_full = 1.0 - texel(&mut full, x, y)[3].to_f32();
            let erased_half = 1.0 - texel(&mut s, x, y)[3].to_f32();
            // Half the erasure, to within the f16 rounding of the stored alpha.
            assert!(
                (erased_half - erased_full * 0.5).abs() <= 0.001,
                "({x}, {y}): {erased_half} vs half of {erased_full}"
            );
            if erased_full > 0.2 {
                checked += 1;
            }
            // Colour is never touched by an erase.
            assert_eq!(texel(&mut s, x, y)[0].to_bits(), h(0.5).to_bits());
        }
    }
    assert!(checked > 100);
}

#[test]
fn a_ramp_that_is_not_a_rectangle_blends_per_texel() {
    let (_d1, mut plain) = store();
    let (_d2, mut masked) = store();
    let mut a = StrokeSnapshot::new(surface());
    let mut b = StrokeSnapshot::new(surface());
    dab(
        &mut plain,
        Kind::Brush,
        (230.0, 100.0),
        120.0,
        [1.0, 0.0, 0.0],
        &mut a,
        None,
        true,
    );
    dab(
        &mut masked,
        Kind::Brush,
        (230.0, 100.0),
        120.0,
        [1.0, 0.0, 0.0],
        &mut b,
        Some(&Ramp),
        false,
    );
    let mut seen_partial = 0;
    for y in 0..220_u32 {
        for x in 100..350_u32 {
            let c = Ramp.at(i64::from(x), i64::from(y));
            let p = texel(&mut plain, x, y);
            let want = lerp_expected([f16::ZERO; 4], p, c.min(1.0));
            let want = if c <= 0.0 { bits([f16::ZERO; 4]) } else { want };
            assert_eq!(bits(texel(&mut masked, x, y)), want, "({x}, {y}) c {c}");
            if c > 0.0 && c < 1.0 && p[3].to_f32() > 0.0 {
                seen_partial += 1;
            }
        }
    }
    assert!(seen_partial > 1000);
    // Left of the ramp nothing at all, even where the plain dab painted.
    assert!(texel(&mut plain, 120, 100)[3].to_f32() > 0.0);
    assert_eq!(bits(texel(&mut masked, 100, 100)), bits([f16::ZERO; 4]));
}

#[test]
fn a_dab_outside_the_selection_creates_no_tile_captures_nothing_and_paints_nothing() {
    let cov = RectCov {
        rect: r(600, 600, 50, 50),
        inverted: false,
    };
    for kind in [Kind::Brush, Kind::Preserve, Kind::Eraser] {
        let (_d, mut s) = store();
        let mut snap = StrokeSnapshot::new(surface());
        let painted = dab(
            &mut s,
            kind,
            (250.0, 250.0),
            30.0,
            [1.0; 3],
            &mut snap,
            Some(&cov),
            false,
        );
        assert!(painted.is_empty());
        assert!(snap.is_empty());
        assert!(dump(&mut s).iter().all(Option::is_none), "no phantom tile");
    }
}

#[test]
fn a_dab_across_a_tile_corner_creates_only_the_tiles_the_selection_reaches() {
    // The dab spans tiles (0,0) (1,0) (0,1) (1,1); the selection is the
    // top-left tile only (and the bottom-right quadrant's corner for the
    // inverted case).
    let (_d, mut s) = store();
    let cov = RectCov {
        rect: r(0, 0, 256, 256),
        inverted: false,
    };
    let mut snap = StrokeSnapshot::new(surface());
    let painted = dab(
        &mut s,
        Kind::Brush,
        (256.0, 256.0),
        40.0,
        [1.0; 3],
        &mut snap,
        Some(&cov),
        false,
    );
    assert_eq!(painted, vec![TileId { x: 0, y: 0 }]);
    assert_eq!(
        snap.captured().collect::<Vec<_>>(),
        vec![TileId { x: 0, y: 0 }]
    );
    let d = dump(&mut s);
    assert!(d.first().is_some_and(Option::is_some));
    assert_eq!(d.iter().flatten().count(), 1, "only tile (0,0) exists");

    // The same dab, inverted: tile (0,0) is excluded from the corner, the
    // other three are painted; the tile outside the dab's box is never made.
    let (_d, mut s) = store();
    let cov = RectCov {
        rect: r(0, 0, 256, 256),
        inverted: true,
    };
    let mut snap = StrokeSnapshot::new(surface());
    let painted = dab(
        &mut s,
        Kind::Brush,
        (256.0, 256.0),
        40.0,
        [1.0; 3],
        &mut snap,
        Some(&cov),
        false,
    );
    assert_eq!(painted.len(), 3);
    assert!(!painted.contains(&TileId { x: 0, y: 0 }));
    assert!(!s.contains_tile(surface(), TileId { x: 0, y: 0 }));
}

#[test]
fn a_tile_the_selection_excludes_is_not_even_touched() {
    // Tile (1,0) holds content; the selection excludes it, so a dab over it
    // neither changes it nor bumps its content version.
    let (_d, mut s) = store();
    put(&mut s, 300, 10, [h(0.5), h(0.5), h(0.5), h(0.5)]);
    let version = s.content_version(surface(), TileId { x: 1, y: 0 });
    let cov = RectCov {
        rect: r(0, 0, 200, 200),
        inverted: false,
    };
    let mut snap = StrokeSnapshot::new(surface());
    dab(
        &mut s,
        Kind::Eraser,
        (300.0, 10.0),
        30.0,
        [1.0; 3],
        &mut snap,
        Some(&cov),
        false,
    );
    assert_eq!(s.content_version(surface(), TileId { x: 1, y: 0 }), version);
    assert_eq!(bits(texel(&mut s, 300, 10)), [h(0.5).to_bits(); 4]);
}

#[test]
fn a_selected_stroke_undoes_and_redoes_to_the_exact_pixels_as_one_step() {
    let (_d, mut s) = store();
    start_content(&mut s);
    let before = dump(&mut s);
    let cov = RectCov {
        rect: r(100, 100, 200, 200),
        inverted: false,
    };
    let mut snap = StrokeSnapshot::new(surface());
    for c in [
        (150.0, 150.0),
        (200.0, 220.0),
        (290.0, 295.0),
        (120.0, 280.0),
    ] {
        dab(
            &mut s,
            Kind::Brush,
            c,
            40.0,
            [0.0, 0.0, 1.0],
            &mut snap,
            Some(&cov),
            false,
        );
        dab(
            &mut s,
            Kind::Eraser,
            c,
            20.0,
            [0.0; 3],
            &mut snap,
            Some(&cov),
            false,
        );
    }
    // Only tiles the selection reaches were captured: (0,0), (1,0), (0,1), (1,1).
    assert!(snap.captured().count() <= 4);
    let after = dump(&mut s);
    assert_ne!(before, after);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s), before, "undo restores every bit");
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s), after, "redo restores every bit");
    assert!(!history.can_redo());
}

#[test]
fn transparency_lock_with_a_selection_never_changes_alpha_and_stays_inside() {
    let cov = RectCov {
        rect: r(50, 50, 100, 100),
        inverted: false,
    };
    let (_d, mut s) = store();
    start_content(&mut s);
    let (_d2, mut original) = store();
    start_content(&mut original);
    let mut snap = StrokeSnapshot::new(surface());
    dab(
        &mut s,
        Kind::Preserve,
        (100.0, 100.0),
        100.0,
        [1.0, 0.0, 0.0],
        &mut snap,
        Some(&cov),
        false,
    );
    let mut recoloured = 0;
    for y in 0..300_u32 {
        for x in 0..300_u32 {
            let (n, o) = (texel(&mut s, x, y), texel(&mut original, x, y));
            assert_eq!(n[3].to_bits(), o[3].to_bits(), "alpha ({x}, {y})");
            if !cov.rect.contains_point(i64::from(x), i64::from(y)) {
                assert_eq!(bits(n), bits(o));
            } else if bits(n) != bits(o) {
                recoloured += 1;
            }
        }
    }
    assert!(recoloured > 500);
}

// ---- fills ----

const RED: [f32; 3] = [1.0, 0.0, 0.0];

fn flood_spec(seed: (u32, u32), contiguous: bool) -> FloodSpec {
    FloodSpec {
        seed,
        tolerance: 0,
        contiguous,
        max_tiles: MAX_FILL_TILES,
        max_examined: MAX_EXAMINED_TILES,
    }
}

fn white_block(s: &mut TileStore, w: u32, ht: u32) {
    for y in 0..ht {
        for x in 0..w {
            put(s, x, y, [h(1.0), h(1.0), h(1.0), h(1.0)]);
        }
    }
}

#[allow(clippy::type_complexity)]
fn bucket(
    s: &mut TileStore,
    area: (u32, u32),
    spec: &FloodSpec,
    mode: FillMode,
    coverage: Option<&dyn Coverage>,
    plain: bool,
) -> (Vec<TileId>, Vec<TileId>) {
    let mut snap = StrokeSnapshot::new(surface());
    let outcome = if plain {
        flood_fill(s, surface(), area, spec, RED, mode, Some(&mut snap))
    } else {
        flood_fill_covered(
            s,
            surface(),
            area,
            spec,
            RED,
            mode,
            Some(&mut snap),
            coverage,
        )
    };
    let Ok(outcome) = outcome else {
        unreachable!("fill");
    };
    assert!(outcome.failed().is_empty());
    (outcome.painted().to_vec(), sorted(snap.captured()))
}

#[test]
fn bucket_with_no_coverage_or_full_coverage_is_the_plain_fill_bit_for_bit() {
    for contiguous in [true, false] {
        for mode in [FillMode::Normal, FillMode::PreserveAlpha] {
            let mut results = Vec::new();
            for variant in 0..4 {
                let (_d, mut s) = store();
                white_block(&mut s, 700, 300);
                // A wall so the region is not simply everything.
                for y in 0..300 {
                    put(&mut s, 400, y, [h(0.0), h(0.0), h(0.0), h(1.0)]);
                }
                let one = Constant(1.0);
                let all = RectCov {
                    rect: r(-5, -5, 5000, 5000),
                    inverted: false,
                };
                let cov: Option<&dyn Coverage> = match variant {
                    2 => Some(&one),
                    3 => Some(&all),
                    _ => None,
                };
                let out = bucket(
                    &mut s,
                    (700, 300),
                    &flood_spec((10, 10), contiguous),
                    mode,
                    cov,
                    variant == 0,
                );
                results.push((out, dump(&mut s)));
            }
            assert!(
                results.windows(2).all(|w| w[0] == w[1]),
                "{contiguous} {mode:?}"
            );
            assert!(!results[0].0.0.is_empty());
        }
    }
}

#[test]
fn bucket_fills_only_the_selected_part_of_its_region() {
    for contiguous in [true, false] {
        let (_d, mut s) = store();
        white_block(&mut s, 600, 300);
        let cov = RectCov {
            rect: r(250, 100, 100, 150),
            inverted: false,
        };
        let (painted, captured) = bucket(
            &mut s,
            (600, 300),
            &flood_spec((260, 110), contiguous),
            FillMode::Normal,
            Some(&cov),
            false,
        );
        assert_eq!(painted, captured);
        for y in 0..300_u32 {
            for x in 0..600_u32 {
                let inside = cov.rect.contains_point(i64::from(x), i64::from(y));
                let want = if inside {
                    [h(1.0), h(0.0), h(0.0), h(1.0)]
                } else {
                    [h(1.0); 4]
                };
                assert_eq!(bits(texel(&mut s, x, y)), bits(want), "({x}, {y})");
            }
        }
    }
}

#[test]
fn a_bucket_click_outside_the_selection_does_nothing_and_reads_nothing() {
    let (_d, mut s) = store();
    white_block(&mut s, 600, 300);
    let before = dump(&mut s);
    let versions: Vec<u64> = (0..3)
        .map(|x| s.content_version(surface(), TileId { x, y: 0 }))
        .collect();
    let cov = RectCov {
        rect: r(250, 100, 100, 150),
        inverted: false,
    };
    let (painted, captured) = bucket(
        &mut s,
        (600, 300),
        &flood_spec((10, 10), true),
        FillMode::Normal,
        Some(&cov),
        false,
    );
    assert!(painted.is_empty() && captured.is_empty());
    assert_eq!(dump(&mut s), before);
    let after: Vec<u64> = (0..3)
        .map(|x| s.content_version(surface(), TileId { x, y: 0 }))
        .collect();
    assert_eq!(versions, after);
    // The seed's own pixel is exactly the boundary: the right/bottom edge
    // is outside, so a click there does nothing either.
    for seed in [(350, 110), (260, 250), (249, 110), (260, 99)] {
        let (p, _) = bucket(
            &mut s,
            (600, 300),
            &flood_spec(seed, true),
            FillMode::Normal,
            Some(&cov),
            false,
        );
        assert!(p.is_empty(), "{seed:?}");
    }
    let (p, _) = bucket(
        &mut s,
        (600, 300),
        &flood_spec((349, 249), true),
        FillMode::Normal,
        Some(&cov),
        false,
    );
    assert!(!p.is_empty(), "the last selected pixel is inside");
}

#[test]
fn bucket_through_an_inverted_selection_and_a_fractional_one() {
    let (_d, mut s) = store();
    white_block(&mut s, 600, 300);
    let cov = RectCov {
        rect: r(100, 0, 400, 300),
        inverted: true,
    };
    let _ = bucket(
        &mut s,
        (600, 300),
        &flood_spec((10, 10), true),
        FillMode::Normal,
        Some(&cov),
        false,
    );
    assert_eq!(texel(&mut s, 99, 5)[1].to_bits(), h(0.0).to_bits());
    assert_eq!(texel(&mut s, 100, 5)[1].to_bits(), h(1.0).to_bits());
    assert_eq!(texel(&mut s, 500, 5)[1].to_bits(), h(0.0).to_bits());
    assert_eq!(texel(&mut s, 499, 5)[1].to_bits(), h(1.0).to_bits());

    let (_d, mut s) = store();
    white_block(&mut s, 100, 100);
    let half = Constant(0.5);
    let _ = bucket(
        &mut s,
        (100, 100),
        &flood_spec((5, 5), true),
        FillMode::Normal,
        Some(&half),
        false,
    );
    // old white, fill red opaque: the halfway texel is [1, .5, .5, 1].
    assert_eq!(
        bits(texel(&mut s, 50, 50)),
        [
            h(1.0).to_bits(),
            h(0.5).to_bits(),
            h(0.5).to_bits(),
            h(1.0).to_bits()
        ]
    );
}

#[test]
fn bucket_selection_creates_no_phantom_tiles_in_an_empty_layer() {
    // An empty layer: the whole 600 x 300 area is one transparent region.
    let (_d, mut s) = store();
    let cov = RectCov {
        rect: r(10, 10, 100, 100),
        inverted: false,
    };
    let (painted, captured) = bucket(
        &mut s,
        (600, 300),
        &flood_spec((20, 20), true),
        FillMode::Normal,
        Some(&cov),
        false,
    );
    assert_eq!(painted, vec![TileId { x: 0, y: 0 }]);
    assert_eq!(captured, painted);
    assert_eq!(dump(&mut s).iter().flatten().count(), 1);
}

fn gradient_spec(opacity: f32) -> GradientSpec {
    GradientSpec {
        kind: GradientKind::Linear,
        start: (20.0, 30.0),
        end: (500.0, 220.0),
        from: [1.0, 0.0, 0.0],
        to: [0.0, 0.0, 1.0],
        opacity,
        max_tiles: MAX_FILL_TILES,
    }
}

fn gradient(
    s: &mut TileStore,
    area: (u32, u32, u32, u32),
    spec: &GradientSpec,
    mode: FillMode,
    coverage: Option<&dyn Coverage>,
    plain: bool,
) -> (Vec<TileId>, Vec<TileId>) {
    let mut snap = StrokeSnapshot::new(surface());
    let outcome = if plain {
        gradient_fill(s, surface(), area, spec, mode, Some(&mut snap))
    } else {
        gradient_fill_covered(s, surface(), area, spec, mode, Some(&mut snap), coverage)
    };
    let Ok(outcome) = outcome else {
        unreachable!("gradient");
    };
    (outcome.painted().to_vec(), sorted(snap.captured()))
}

#[test]
fn gradient_with_no_coverage_or_full_coverage_is_the_plain_gradient_bit_for_bit() {
    for mode in [FillMode::Normal, FillMode::PreserveAlpha] {
        for opacity in [1.0, 0.6] {
            let mut results = Vec::new();
            for variant in 0..4 {
                let (_d, mut s) = store();
                start_content(&mut s);
                let one = Constant(1.0);
                let all = RectCov {
                    rect: r(-5, -5, 5000, 5000),
                    inverted: false,
                };
                let cov: Option<&dyn Coverage> = match variant {
                    2 => Some(&one),
                    3 => Some(&all),
                    _ => None,
                };
                let out = gradient(
                    &mut s,
                    (0, 0, 520, 300),
                    &gradient_spec(opacity),
                    mode,
                    cov,
                    variant == 0,
                );
                results.push((out, dump(&mut s)));
            }
            assert!(
                results.windows(2).all(|w| w[0] == w[1]),
                "{mode:?} {opacity}"
            );
            assert!(!results[0].0.0.is_empty());
        }
    }
}

#[test]
fn gradient_is_evaluated_as_before_and_painted_only_through_the_selection() {
    for inverted in [false, true] {
        let (_d1, mut plain) = store();
        let (_d2, mut masked) = store();
        let (_d3, mut original) = store();
        for s in [&mut plain, &mut masked, &mut original] {
            start_content(s);
        }
        let cov = RectCov {
            rect: r(200, 50, 150, 200),
            inverted,
        };
        let spec = gradient_spec(1.0);
        let _ = gradient(
            &mut plain,
            (0, 0, 520, 300),
            &spec,
            FillMode::Normal,
            None,
            true,
        );
        let _ = gradient(
            &mut masked,
            (0, 0, 520, 300),
            &spec,
            FillMode::Normal,
            Some(&cov),
            false,
        );
        for y in 0..300_u32 {
            for x in 0..520_u32 {
                let inside = cov.rect.contains_point(i64::from(x), i64::from(y)) != inverted;
                let want = if inside {
                    texel(&mut plain, x, y)
                } else {
                    texel(&mut original, x, y)
                };
                assert_eq!(
                    bits(texel(&mut masked, x, y)),
                    bits(want),
                    "({x}, {y}) inv {inverted}"
                );
            }
        }
    }
}

#[test]
fn gradient_blends_fractionally_and_counts_only_the_selected_tiles_against_the_cap() {
    let (_d1, mut plain) = store();
    let (_d2, mut masked) = store();
    let (_d3, mut original) = store();
    for s in [&mut plain, &mut masked, &mut original] {
        start_content(s);
    }
    let half = Constant(0.5);
    let spec = gradient_spec(1.0);
    let _ = gradient(
        &mut plain,
        (0, 0, 520, 300),
        &spec,
        FillMode::Normal,
        None,
        true,
    );
    let _ = gradient(
        &mut masked,
        (0, 0, 520, 300),
        &spec,
        FillMode::Normal,
        Some(&half),
        false,
    );
    for (x, y) in [(5, 5), (100, 100), (300, 200), (400, 250), (399, 399)] {
        let (o, p) = (texel(&mut original, x, y), texel(&mut plain, x, y));
        assert_eq!(
            bits(texel(&mut masked, x, y)),
            lerp_expected(o, p, 0.5),
            "({x}, {y})"
        );
    }

    // A 9000 x 9000 area is 1,296 tiles: refused outright, but allowed
    // when the selection reaches only one tile of it.
    let (_d, mut big) = store();
    let spec = GradientSpec {
        end: (100.0, 0.0),
        ..gradient_spec(1.0)
    };
    let refused = gradient_fill(
        &mut big,
        surface(),
        (0, 0, 9000, 9000),
        &spec,
        FillMode::Normal,
        None,
    );
    assert!(refused.is_err());
    let small = RectCov {
        rect: r(10, 10, 50, 50),
        inverted: false,
    };
    let (painted, captured) = gradient(
        &mut big,
        (0, 0, 9000, 9000),
        &spec,
        FillMode::Normal,
        Some(&small),
        false,
    );
    assert_eq!(painted, vec![TileId { x: 0, y: 0 }]);
    assert_eq!(painted, captured);
    assert_eq!(dump(&mut big).iter().flatten().count(), 1);
    // A selection wholly outside the area paints nothing and makes nothing.
    let (_d, mut s) = store();
    let away = RectCov {
        rect: r(2000, 2000, 10, 10),
        inverted: false,
    };
    let (painted, _) = gradient(
        &mut s,
        (0, 0, 520, 300),
        &gradient_spec(1.0),
        FillMode::Normal,
        Some(&away),
        false,
    );
    assert!(painted.is_empty());
    assert!(dump(&mut s).iter().all(Option::is_none));
}

#[test]
fn a_selected_fill_undoes_and_redoes_exactly() {
    let (_d, mut s) = store();
    start_content(&mut s);
    let before = dump(&mut s);
    let cov = RectCov {
        rect: r(150, 150, 200, 200),
        inverted: false,
    };
    let mut snap = StrokeSnapshot::new(surface());
    let Ok(outcome) = gradient_fill_covered(
        &mut s,
        surface(),
        (0, 0, 520, 400),
        &gradient_spec(0.8),
        FillMode::Normal,
        Some(&mut snap),
        Some(&cov),
    ) else {
        unreachable!("gradient");
    };
    assert!(!outcome.painted().is_empty());
    let after = dump(&mut s);
    assert_ne!(before, after);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s), before);
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s), after);
}

/// Prints the cost of one default-radius dab with no selection, a
/// rectangle that cuts the dab (tile-level `Partial`), an inverted one
/// (no bounds, per-texel lookups) and a fractional coverage (a blend per
/// texel). Run with `--nocapture`. Asserts nothing about time (CI
/// machines vary); it asserts only that every variant painted.
#[test]
fn dab_cost_with_and_without_a_selection_is_printed() {
    use std::time::Instant;
    let rect = RectCov {
        rect: r(300, 300, 400, 400),
        inverted: false,
    };
    let inverted = RectCov {
        rect: r(0, 0, 100, 100),
        inverted: true,
    };
    let half = Constant(0.5);
    let variants: [(&str, Option<&dyn Coverage>); 4] = [
        ("no selection", None),
        ("rectangle", Some(&rect)),
        ("inverted rectangle", Some(&inverted)),
        ("fractional 0.5", Some(&half)),
    ];
    for radius in [24.0_f32, 100.0] {
        for (name, cov) in variants {
            let (_d, mut s) = store();
            let mut snap = StrokeSnapshot::new(surface());
            let rounds = 400_u32;
            let start = Instant::now();
            let mut painted = 0_usize;
            for i in 0..rounds {
                let x = 320.0 + (i % 40) as f32 * 8.0;
                let y = 320.0 + (i / 40) as f32 * 8.0;
                let outcome = stamp_dab_covered(
                    &mut s,
                    surface(),
                    (x, y),
                    radius,
                    [1.0, 0.0, 0.0],
                    Some(&mut snap),
                    cov,
                );
                painted += outcome.painted().len();
            }
            let per = start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds);
            println!("dab radius {radius:>5}: {name:<20} {per:>8.1} us/dab");
            assert!(painted > 0, "{name}");
        }
    }
}
