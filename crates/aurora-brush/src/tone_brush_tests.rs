//! The Dodge, Burn and Sponge dab (0.224.0) at the brush level: the range
//! weights, hand-computed values for every range and both Sponge modes,
//! whole strokes against an independent model, the monotone guarantees,
//! alpha and transparent texels untouched, selection coverage, tile edges
//! (neighbour present and absent), no phantom tiles, undo, the cap and
//! degenerate input.

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
    Coverage, DabOutcome, MAX_TONE_DAB_TEXELS, PixelHistory, SpongeMode, StrokeSnapshot, ToneOp,
    ToneRange, dab_falloff, luminance, range_weight, tone_dab_covered,
};

const OPS: [ToneOp; 8] = [
    ToneOp::Dodge(ToneRange::Shadows),
    ToneOp::Dodge(ToneRange::Midtones),
    ToneOp::Dodge(ToneRange::Highlights),
    ToneOp::Burn(ToneRange::Shadows),
    ToneOp::Burn(ToneRange::Midtones),
    ToneOp::Burn(ToneRange::Highlights),
    ToneOp::Saturate,
    ToneOp::Desaturate,
];

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

/// A texel from a straight colour and an alpha, premultiplied.
fn straight(r: f32, g: f32, b: f32, a: f32) -> [f16; CHANNELS] {
    px(r * a, g * a, b * a, a)
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

/// A distinct texel per position: straight colour in sixteenths, alpha in
/// eighths (`0` for one position in nine, so transparent texels occur).
fn pattern_colour(x: u32, y: u32) -> [f16; CHANNELS] {
    let n = (x * 3 + y * 5) % 9;
    if n == 8 {
        return [f16::ZERO; CHANNELS];
    }
    let a = (n + 1) as f32 / 8.0;
    let s = |n: u32| (n % 17) as f32 / 16.0;
    straight(
        s(x * 7 + y),
        s(x + y * 5 + 3),
        s(x * 3 + y * 11 + 9),
        a.min(1.0),
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
    op: ToneOp,
    center: (f32, f32),
    radius: f32,
    amount: f32,
    snapshot: Option<&mut StrokeSnapshot>,
    coverage: Option<&dyn Coverage>,
) -> DabOutcome {
    tone_dab_covered(s, surface(), center, radius, op, amount, snapshot, coverage)
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

/// The straight colour of a stored texel.
fn straight_of(t: [f16; CHANNELS]) -> [f32; 3] {
    let a = t[3].to_f32();
    [0, 1, 2].map(|c| t[c].to_f32() / a)
}

fn close(got: f32, want: f64, what: &str) {
    assert!(
        (f64::from(got) - want).abs() < 1.5e-3,
        "{what}: got {got}, want {want}"
    );
}

// -- The independent model ----------------------------------------------------

/// A dab on a plain `f32` grid, written from the module docs.
fn model_dab(
    grid: &mut [[f32; 4]],
    width: usize,
    center: (f32, f32),
    radius: f32,
    op: ToneOp,
    amount: f32,
    selection: Option<&dyn Fn(i64, i64) -> f32>,
) {
    let height = grid.len() / width;
    let amount = amount.clamp(0.0, 1.0);
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
            let k = amount * a;
            let old = grid[y as usize * width + x as usize];
            if old[3] <= 0.0 {
                continue;
            }
            let s = [old[0] / old[3], old[1] / old[3], old[2] / old[3]];
            let luma = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
            let weight = |range: ToneRange| {
                let y = luma.clamp(0.0, 1.0);
                match range {
                    ToneRange::Shadows => (1.0 - y) * (1.0 - y),
                    ToneRange::Midtones => 4.0 * y * (1.0 - y),
                    ToneRange::Highlights => y * y,
                }
            };
            let mut fresh = [0.0_f32; 4];
            for ch in 0..3 {
                let c = s[ch];
                let moved = match op {
                    ToneOp::Dodge(r) => c + (1.0 - c).max(0.0) * (k * weight(r)),
                    ToneOp::Burn(r) => c - c.max(0.0) * (k * weight(r)),
                    ToneOp::Desaturate => c + (luma - c) * k,
                    ToneOp::Saturate => (c + (c - luma) * k).clamp(0.0, 1.0),
                };
                fresh[ch] = f16::from_f32(moved * old[3]).to_f32();
            }
            fresh[3] = old[3];
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

fn assert_stroke_matches_model(
    blocks: &[(u32, u32, u32, u32)],
    dims: (u32, u32),
    dabs: &[(f32, f32)],
    radius: f32,
    op: ToneOp,
    amount: f32,
) {
    let (_dir, mut s) = store();
    for block in blocks {
        paint_pattern(&mut s, block.0, block.1, block.2, block.3);
    }
    let (w, h) = dims;
    let mut grid = grid_of(&mut s, w, h);
    for &c in dabs {
        let _ = dab(&mut s, op, c, radius, amount, None, None);
        model_dab(&mut grid, w as usize, c, radius, op, amount, None);
    }
    assert_grid_matches(&mut s, &grid, w, h, &format!("{op:?} amount {amount}"));
}

// -- Range weights and luminance -------------------------------------------------

#[test]
fn the_range_weights_are_the_documented_curves() {
    for (y, shadows, mids, highlights) in [
        (0.0, 1.0, 0.0, 0.0),
        (0.25, 0.5625, 0.75, 0.0625),
        (0.5, 0.25, 1.0, 0.25),
        (0.75, 0.0625, 0.75, 0.5625),
        (1.0, 0.0, 0.0, 1.0),
    ] {
        assert_eq!(range_weight(ToneRange::Shadows, y), shadows, "shadows {y}");
        assert_eq!(range_weight(ToneRange::Midtones, y), mids, "mids {y}");
        assert_eq!(
            range_weight(ToneRange::Highlights, y),
            highlights,
            "highlights {y}"
        );
    }
    // Out-of-range and non-finite luminance clamp or weigh nothing.
    assert_eq!(range_weight(ToneRange::Shadows, -3.0), 1.0);
    assert_eq!(range_weight(ToneRange::Highlights, 7.0), 1.0);
    assert_eq!(range_weight(ToneRange::Midtones, 9.0), 0.0);
    for range in ToneRange::ALL {
        assert_eq!(range_weight(range, f32::NAN), 0.0);
        assert_eq!(range_weight(range, f32::INFINITY), 0.0);
    }
    // The weights stay in 0..=1 and the ranges are different curves.
    for i in 0..=64 {
        let y = i as f32 / 64.0;
        for range in ToneRange::ALL {
            let w = range_weight(range, y);
            assert!((0.0..=1.0).contains(&w), "{range:?} {y} {w}");
        }
    }
    assert_eq!(ToneRange::default(), ToneRange::Midtones);
}

#[test]
fn the_luminance_is_rec709_of_the_straight_colour() {
    assert!((luminance([1.0, 0.0, 0.0]) - 0.2126).abs() < 1e-6);
    assert!((luminance([0.0, 1.0, 0.0]) - 0.7152).abs() < 1e-6);
    assert!((luminance([0.0, 0.0, 1.0]) - 0.0722).abs() < 1e-6);
    assert!((luminance([0.5; 3]) - 0.5).abs() < 1e-6);
}

// -- Hand-computed values -----------------------------------------------------------

#[test]
fn dodge_and_burn_on_a_quarter_grey_give_the_hand_computed_values_per_range() {
    // Straight grey 0.25 has Y = 0.25 (weights 0.5625 / 0.75 / 0.0625) and
    // the centre texel has falloff 1, so at amount 1: dodge = 0.25 + 0.75 w,
    // burn = 0.25 (1 - w).
    let expected = [
        (ToneOp::Dodge(ToneRange::Shadows), 0.671_875),
        (ToneOp::Dodge(ToneRange::Midtones), 0.8125),
        (ToneOp::Dodge(ToneRange::Highlights), 0.296_875),
        (ToneOp::Burn(ToneRange::Shadows), 0.109_375),
        (ToneOp::Burn(ToneRange::Midtones), 0.0625),
        (ToneOp::Burn(ToneRange::Highlights), 0.234_375),
    ];
    for (op, want) in expected {
        let (_dir, mut s) = store();
        put(&mut s, 10, 10, straight(0.25, 0.25, 0.25, 1.0));
        let _ = dab(&mut s, op, (10.5, 10.5), 6.0, 1.0, None, None);
        let t = texel(&mut s, 10, 10);
        for c in 0..3 {
            close(t[c].to_f32(), want, &format!("{op:?} channel {c}"));
        }
        assert_eq!(t[3].to_f32(), 1.0);
    }
}

#[test]
fn the_exposure_and_the_falloff_scale_the_move() {
    let want = |amount: f64, falloff: f64| 0.25 + 0.75 * 0.75 * amount * falloff;
    for amount in [0.2_f32, 0.5, 1.0] {
        let (_dir, mut s) = store();
        // Three texels on a row at different distances from the centre.
        for x in [10, 13, 15] {
            put(&mut s, x, 10, straight(0.25, 0.25, 0.25, 1.0));
        }
        let radius = 8.0;
        let _ = dab(
            &mut s,
            ToneOp::Dodge(ToneRange::Midtones),
            (10.5, 10.5),
            radius,
            amount,
            None,
            None,
        );
        let mut last = f32::INFINITY;
        for x in [10_u32, 13, 15] {
            let falloff = f64::from(dab_falloff((x - 10) as f32, radius));
            let got = texel(&mut s, x, 10)[0].to_f32();
            close(
                got,
                want(f64::from(amount), falloff),
                &format!("{amount} {x}"),
            );
            assert!(got < last, "the move shrinks with distance: {got} {last}");
            last = got;
        }
    }
}

#[test]
fn the_sponge_desaturates_toward_the_luma_and_saturates_away_from_it() {
    // Pure red: Y = 0.2126.
    let (_dir, mut s) = store();
    put(&mut s, 5, 5, straight(1.0, 0.0, 0.0, 1.0));
    let _ = dab(&mut s, ToneOp::Desaturate, (5.5, 5.5), 4.0, 0.5, None, None);
    let t = texel(&mut s, 5, 5);
    close(t[0].to_f32(), 0.6063, "desaturate red");
    close(t[1].to_f32(), 0.1063, "desaturate green");
    close(t[2].to_f32(), 0.1063, "desaturate blue");
    let (_dir, mut s) = store();
    put(&mut s, 5, 5, straight(1.0, 0.0, 0.0, 1.0));
    let _ = dab(&mut s, ToneOp::Desaturate, (5.5, 5.5), 4.0, 1.0, None, None);
    let t = texel(&mut s, 5, 5);
    for c in 0..3 {
        close(t[c].to_f32(), 0.2126, "full desaturate is the grey");
    }

    // A muted red: Y = 0.44252.
    let (_dir, mut s) = store();
    put(&mut s, 5, 5, straight(0.6, 0.4, 0.4, 1.0));
    let _ = dab(&mut s, ToneOp::Saturate, (5.5, 5.5), 4.0, 0.5, None, None);
    let t = texel(&mut s, 5, 5);
    close(t[0].to_f32(), 0.678_74, "saturate red");
    close(t[1].to_f32(), 0.378_74, "saturate green");
    close(t[2].to_f32(), 0.378_74, "saturate blue");

    // Per-channel clamp: pure red saturated fully clamps to (1, 0, 0).
    let (_dir, mut s) = store();
    put(&mut s, 5, 5, straight(1.0, 0.0, 0.0, 1.0));
    let _ = dab(&mut s, ToneOp::Saturate, (5.5, 5.5), 4.0, 1.0, None, None);
    assert_eq!(
        bits(texel(&mut s, 5, 5)),
        bits(px(1.0, 0.0, 0.0, 1.0)),
        "a saturated colour has nowhere to go"
    );
}

#[test]
fn a_translucent_texel_is_toned_in_straight_colour_and_keeps_its_alpha() {
    let (_dir, mut s) = store();
    put(&mut s, 7, 7, straight(0.25, 0.25, 0.25, 0.5));
    let _ = dab(
        &mut s,
        ToneOp::Dodge(ToneRange::Midtones),
        (7.5, 7.5),
        5.0,
        1.0,
        None,
        None,
    );
    let t = texel(&mut s, 7, 7);
    // Straight 0.8125 premultiplied by the unchanged alpha 0.5.
    close(t[0].to_f32(), 0.8125 * 0.5, "premultiplied");
    assert_eq!(t[3].to_bits(), f16::from_f32(0.5).to_bits());
    let s2 = straight_of(t);
    close(s2[0], 0.8125, "straight");
}

#[test]
fn grey_texels_are_unchanged_by_either_sponge_mode() {
    for mode in [ToneOp::Saturate, ToneOp::Desaturate] {
        let (_dir, mut s) = store();
        for (i, g) in [0.0_f32, 0.0625, 0.25, 0.5, 0.75, 0.9375, 1.0]
            .into_iter()
            .enumerate()
        {
            for a in [1.0_f32, 0.5, 0.125] {
                put(&mut s, i as u32, (a * 8.0) as u32, straight(g, g, g, a));
            }
        }
        let before = dump(&mut s, 8, 9);
        let out = dab(&mut s, mode, (4.0, 4.0), 12.0, 1.0, None, None);
        assert_eq!(dump(&mut s, 8, 9), before, "{mode:?}");
        assert!(out.painted().is_empty(), "{mode:?} reports nothing painted");
    }
}

// -- Monotone and range guarantees ----------------------------------------------------------

#[test]
fn a_dodge_never_darkens_and_a_burn_never_lightens_any_channel() {
    for range in ToneRange::ALL {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        // An out-of-gamut channel too: straight 1.5 and -0.25 (alpha 1).
        put(&mut s, 20, 20, px(1.5, -0.25, 0.5, 1.0));
        let before = grid_of(&mut s, 64, 64);
        let mut lighter = store();
        paint_pattern(&mut lighter.1, 8, 8, 40, 40);
        put(&mut lighter.1, 20, 20, px(1.5, -0.25, 0.5, 1.0));
        let _ = dab(
            &mut s,
            ToneOp::Dodge(range),
            (28.5, 28.5),
            16.0,
            0.8,
            None,
            None,
        );
        let after = grid_of(&mut s, 64, 64);
        let _ = dab(
            &mut lighter.1,
            ToneOp::Burn(range),
            (28.5, 28.5),
            16.0,
            0.8,
            None,
            None,
        );
        let darker = grid_of(&mut lighter.1, 64, 64);
        let mut moved = 0;
        for i in 0..before.len() {
            for c in 0..3 {
                assert!(after[i][c] >= before[i][c], "dodge {range:?} {i} {c}");
                assert!(darker[i][c] <= before[i][c], "burn {range:?} {i} {c}");
            }
            if after[i] != before[i] {
                moved += 1;
            }
        }
        assert!(moved > 100, "{range:?} {moved}");
    }
}

#[test]
fn each_range_reaches_its_tones_and_the_ranges_differ() {
    // A dark, a middle and a light grey, dodged with each range: the
    // shadows range moves the dark one most, the midtones the middle one,
    // the highlights the light one.
    let run = |range: ToneRange| {
        let (_dir, mut s) = store();
        for (i, g) in [0.125_f32, 0.5, 0.875].into_iter().enumerate() {
            put(&mut s, 10 + i as u32, 10, straight(g, g, g, 1.0));
        }
        let _ = dab(
            &mut s,
            ToneOp::Dodge(range),
            (11.5, 10.5),
            6.0,
            1.0,
            None,
            None,
        );
        [0_u32, 1, 2].map(|i| {
            let g = [0.125_f32, 0.5, 0.875][i as usize];
            // Normalise by the room left and the falloff so only the
            // weight remains.
            let falloff = dab_falloff((i as f32 - 1.0).abs(), 6.0);
            (texel(&mut s, 10 + i, 10)[0].to_f32() - g) / ((1.0 - g) * falloff)
        })
    };
    let shadows = run(ToneRange::Shadows);
    let mids = run(ToneRange::Midtones);
    let highs = run(ToneRange::Highlights);
    assert!(
        shadows[0] > shadows[1] && shadows[1] > shadows[2],
        "{shadows:?}"
    );
    assert!(mids[1] > mids[0] && mids[1] > mids[2], "{mids:?}");
    assert!(highs[2] > highs[1] && highs[1] > highs[0], "{highs:?}");
}

#[test]
fn dodge_and_burn_are_opposites_and_the_sponge_modes_differ() {
    let make = || {
        let (dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 30, 30);
        (dir, s)
    };
    let mut results = Vec::new();
    for op in OPS {
        let (_dir, mut s) = make();
        let _ = dab(&mut s, op, (22.5, 22.5), 10.0, 0.9, None, None);
        results.push(dump(&mut s, 40, 40));
    }
    for i in 0..results.len() {
        for j in i + 1..results.len() {
            assert_ne!(results[i], results[j], "{:?} vs {:?}", OPS[i], OPS[j]);
        }
    }
    // Total brightness: every dodge up, every burn down.
    let sum = |d: &[[u16; 4]]| -> f32 {
        d.iter()
            .map(|t| (0..3).map(|c| f16::from_bits(t[c]).to_f32()).sum::<f32>())
            .sum()
    };
    let (_dir, mut base) = make();
    let before = sum(&dump(&mut base, 40, 40));
    for (i, op) in OPS.iter().enumerate().take(3) {
        assert!(sum(&results[i]) > before, "{op:?}");
        assert!(sum(&results[i + 3]) < before, "{:?}", OPS[i + 3]);
    }
}

#[test]
fn desaturate_shrinks_the_chroma_and_saturate_grows_it() {
    let chroma = |s: &mut TileStore| -> f32 {
        let mut total = 0.0;
        for y in 8..48 {
            for x in 8..48 {
                let t = texel(s, x, y);
                if t[3].to_f32() > 0.0 {
                    let c = straight_of(t);
                    let hi = c.iter().copied().fold(f32::MIN, f32::max);
                    let lo = c.iter().copied().fold(f32::MAX, f32::min);
                    total += hi - lo;
                }
            }
        }
        total
    };
    let (_dir, mut a) = store();
    paint_pattern(&mut a, 8, 8, 40, 40);
    let base = chroma(&mut a);
    let _ = dab(
        &mut a,
        ToneOp::Desaturate,
        (28.5, 28.5),
        14.0,
        1.0,
        None,
        None,
    );
    assert!(chroma(&mut a) < base * 0.95, "{} {base}", chroma(&mut a));
    let (_dir, mut b) = store();
    paint_pattern(&mut b, 8, 8, 40, 40);
    let _ = dab(
        &mut b,
        ToneOp::Saturate,
        (28.5, 28.5),
        14.0,
        1.0,
        None,
        None,
    );
    assert!(chroma(&mut b) > base);
}

// -- Alpha, transparent texels, strength zero ---------------------------------------------

#[test]
fn alpha_is_untouched_bit_for_bit_and_transparent_texels_stay_transparent() {
    for op in OPS {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        let before = dump(&mut s, 64, 64);
        let _ = dab(&mut s, op, (28.5, 28.5), 15.0, 1.0, None, None);
        let after = dump(&mut s, 64, 64);
        let mut changed = 0;
        for i in 0..before.len() {
            assert_eq!(after[i][3], before[i][3], "{op:?} alpha at {i}");
            if before[i][3] == 0 {
                assert_eq!(after[i], before[i], "{op:?} transparent texel {i}");
            } else if after[i] != before[i] {
                changed += 1;
            }
        }
        assert!(changed > 50, "{op:?} {changed}");
    }
}

#[test]
fn amount_zero_changes_nothing_for_every_op() {
    for op in OPS {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        let before = dump(&mut s, 64, 64);
        let mut snapshot = StrokeSnapshot::new(surface());
        let out = dab(
            &mut s,
            op,
            (28.5, 28.5),
            12.0,
            0.0,
            Some(&mut snapshot),
            None,
        );
        assert!(out.painted().is_empty());
        assert!(out.is_complete());
        assert_eq!(snapshot.captured().count(), 0);
        assert_eq!(dump(&mut s, 64, 64), before, "{op:?}");
        // A negative amount clamps to zero.
        let _ = dab(&mut s, op, (28.5, 28.5), 12.0, -1.0, None, None);
        assert_eq!(dump(&mut s, 64, 64), before, "{op:?} negative");
    }
}

// -- Whole strokes against the model ------------------------------------------------------------

#[test]
fn whole_strokes_match_an_independent_model_exactly() {
    for op in OPS {
        for amount in [0.25_f32, 0.5, 1.0] {
            assert_stroke_matches_model(
                &[(4, 4, 56, 40)],
                (64, 48),
                &[(12.5, 20.5), (20.0, 22.0), (27.5, 24.0), (35.0, 24.5)],
                9.0,
                op,
                amount,
            );
        }
    }
}

#[test]
fn later_dabs_see_earlier_dabs_and_the_order_matters() {
    let op = ToneOp::Dodge(ToneRange::Midtones);
    let run = |points: &[(f32, f32)]| {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 8, 8, 40, 40);
        for &p in points {
            let _ = dab(&mut s, op, p, 10.0, 0.6, None, None);
        }
        dump(&mut s, 64, 64)
    };
    let (a, b) = ((20.5, 28.5), (28.5, 28.5));
    // Two dabs on the same spot compound (not equal to one).
    assert_ne!(run(&[a, a]), run(&[a]));
    // The order over overlapping dabs changes the result.
    assert_ne!(run(&[a, b]), run(&[b, a]));
}

#[test]
fn the_footprint_is_the_brushs_soft_dab_and_symmetric() {
    // A uniform mid-grey block: the dodged result is symmetric about the
    // centre under both mirrors, and only texels strictly inside the
    // radius change.
    let (_dir, mut s) = store();
    for y in 10..50 {
        for x in 10..50 {
            put(&mut s, x, y, straight(0.3, 0.3, 0.3, 1.0));
        }
    }
    let before = dump(&mut s, 64, 64);
    let _ = dab(
        &mut s,
        ToneOp::Dodge(ToneRange::Midtones),
        (30.5, 30.5),
        7.0,
        1.0,
        None,
        None,
    );
    let after = dump(&mut s, 64, 64);
    let mut changed = 0;
    for y in 0..64_u32 {
        for x in 0..64_u32 {
            let i = (y * 64 + x) as usize;
            let d = (x as f32 + 0.5 - 30.5).hypot(y as f32 + 0.5 - 30.5);
            if d >= 7.0 {
                assert_eq!(after[i], before[i], "outside the radius at ({x}, {y})");
            } else if after[i] != before[i] {
                changed += 1;
            }
            // Mirror about the texel (30, 30).
            let (mx, my) = (60 - x as i64, 60 - y as i64);
            if d < 7.0 && (0..64).contains(&mx) && (0..64).contains(&my) {
                let j = (my * 64 + mx) as usize;
                assert_eq!(after[i], after[j], "mirror of ({x}, {y})");
            }
        }
    }
    assert!(changed > 100, "{changed}");
}

// -- Selections -------------------------------------------------------------------------------------

#[test]
fn a_selection_restricts_the_dab_and_blends_by_coverage() {
    let op = ToneOp::Burn(ToneRange::Midtones);
    // A rectangle: texels outside are bit-identical.
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 8, 8, 40, 40);
    let before = dump(&mut s, 64, 64);
    let rect = Rect {
        x: 20,
        y: 0,
        width: 40,
        height: 64,
    };
    let _ = dab(
        &mut s,
        op,
        (28.5, 28.5),
        14.0,
        1.0,
        None,
        Some(&RectCov(rect)),
    );
    let after = dump(&mut s, 64, 64);
    let mut inside = 0;
    for y in 0..64_u32 {
        for x in 0..64_u32 {
            let i = (y * 64 + x) as usize;
            if x < 20 {
                assert_eq!(after[i], before[i], "outside the selection ({x}, {y})");
            } else if after[i] != before[i] {
                inside += 1;
            }
        }
    }
    assert!(inside > 30, "{inside}");

    // Half coverage against the model, and between none and full.
    let half = Constant(0.5);
    let (_dir, mut a) = store();
    paint_pattern(&mut a, 8, 8, 40, 40);
    let mut grid = grid_of(&mut a, 64, 64);
    let _ = dab(&mut a, op, (28.5, 28.5), 14.0, 1.0, None, Some(&half));
    model_dab(
        &mut grid,
        64,
        (28.5, 28.5),
        14.0,
        op,
        1.0,
        Some(&|_, _| 0.5),
    );
    assert_grid_matches(&mut a, &grid, 64, 64, "half coverage");
    let (_dir, mut full) = store();
    paint_pattern(&mut full, 8, 8, 40, 40);
    let _ = dab(&mut full, op, (28.5, 28.5), 14.0, 1.0, None, None);
    let half_dump = dump(&mut a, 64, 64);
    let full_dump = dump(&mut full, 64, 64);
    let mut between = 0;
    for i in 0..half_dump.len() {
        if half_dump[i] != before[i] {
            assert_eq!(half_dump[i][3], before[i][3]);
            for c in 0..3 {
                let (o, h, f) = (
                    f16::from_bits(before[i][c]).to_f32(),
                    f16::from_bits(half_dump[i][c]).to_f32(),
                    f16::from_bits(full_dump[i][c]).to_f32(),
                );
                assert!(
                    h <= o + 1e-3 && h >= f - 1e-3,
                    "half {h} between {o} and {f}"
                );
            }
            between += 1;
        }
    }
    assert!(between > 100, "{between}");
}

#[test]
fn a_selection_that_excludes_a_tile_neither_reads_into_it_nor_changes_it() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 240, 8, 40, 30);
    let before = dump(&mut s, 300, 40);
    let rect = Rect {
        x: 0,
        y: 0,
        width: 256,
        height: 64,
    };
    let mut snapshot = StrokeSnapshot::new(surface());
    let out = dab(
        &mut s,
        ToneOp::Dodge(ToneRange::Midtones),
        (258.5, 20.5),
        12.0,
        1.0,
        Some(&mut snapshot),
        Some(&RectCov(rect)),
    );
    assert_eq!(out.painted(), &[TileId { x: 0, y: 0 }]);
    let after = dump(&mut s, 300, 40);
    for y in 0..40_usize {
        for x in 256..300_usize {
            assert_eq!(after[y * 300 + x], before[y * 300 + x], "({x}, {y})");
        }
    }
    let captured: BTreeSet<TileId> = snapshot.captured().collect();
    assert_eq!(captured, BTreeSet::from([TileId { x: 0, y: 0 }]));
}

// -- Tiles, origin, phantom tiles ------------------------------------------------------------------------

#[test]
fn a_stroke_straddling_a_tile_edge_matches_the_model_with_both_tiles_present() {
    for op in [ToneOp::Dodge(ToneRange::Shadows), ToneOp::Desaturate] {
        assert_stroke_matches_model(
            &[(236, 4, 40, 36)],
            (280, 44),
            &[(250.5, 20.5), (256.0, 22.0), (262.5, 24.5)],
            9.0,
            op,
            0.8,
        );
    }
}

#[test]
fn a_dab_over_an_absent_neighbour_tile_tones_the_present_one_and_makes_no_tile() {
    for op in OPS {
        let (_dir, mut s) = store();
        // Content only in tile (0, 0), right up to its edge.
        paint_pattern(&mut s, 230, 10, 26, 30);
        assert!(!s.contains_tile(surface(), TileId { x: 1, y: 0 }));
        let before = dump(&mut s, 256, 50);
        let out = dab(&mut s, op, (256.0, 24.5), 12.0, 1.0, None, None);
        assert_eq!(out.painted(), &[TileId { x: 0, y: 0 }], "{op:?}");
        assert!(
            !s.contains_tile(surface(), TileId { x: 1, y: 0 }),
            "{op:?} must not create the empty neighbour"
        );
        assert_ne!(dump(&mut s, 256, 50), before, "{op:?}");
        // The texels beyond the edge in the neighbour read as transparent.
        for y in 10..40 {
            assert_eq!(texel(&mut s, 257, y), [f16::ZERO; CHANNELS]);
        }
    }
}

#[test]
fn a_stroke_near_the_layer_origin_matches_the_model() {
    for op in [ToneOp::Burn(ToneRange::Highlights), ToneOp::Saturate] {
        assert_stroke_matches_model(
            &[(0, 0, 30, 30)],
            (40, 40),
            &[(1.5, 2.5), (0.0, 0.0), (3.5, 1.0)],
            8.0,
            op,
            0.9,
        );
    }
}

#[test]
fn toning_empty_space_creates_no_tile_and_captures_nothing() {
    for op in OPS {
        let (_dir, mut s) = store();
        let mut snapshot = StrokeSnapshot::new(surface());
        let out = dab(
            &mut s,
            op,
            (300.5, 300.5),
            30.0,
            1.0,
            Some(&mut snapshot),
            None,
        );
        assert!(out.painted().is_empty(), "{op:?}");
        assert!(out.is_complete());
        assert_eq!(snapshot.captured().count(), 0);
        for ty in 0..4 {
            for tx in 0..4 {
                assert!(
                    !s.contains_tile(surface(), TileId { x: tx, y: ty }),
                    "{op:?} made a phantom tile ({tx}, {ty})"
                );
            }
        }
    }
}

#[test]
fn a_held_but_blank_tile_stays_blank_and_uncaptured() {
    let (_dir, mut s) = store();
    // Touching a tile makes it held; its texels are all transparent.
    put(&mut s, 5, 5, [f16::ZERO; CHANNELS]);
    let mut snapshot = StrokeSnapshot::new(surface());
    let out = dab(
        &mut s,
        ToneOp::Dodge(ToneRange::Shadows),
        (5.5, 5.5),
        8.0,
        1.0,
        Some(&mut snapshot),
        None,
    );
    assert!(out.painted().is_empty());
    assert_eq!(snapshot.captured().count(), 0);
}

#[test]
fn undo_restores_exactly_the_captured_tiles() {
    for op in OPS {
        let (_dir, mut s) = store();
        paint_pattern(&mut s, 230, 10, 60, 30);
        let before = dump(&mut s, 300, 60);
        let mut snapshot = StrokeSnapshot::new(surface());
        let mut painted = BTreeSet::new();
        for x in [240.5_f32, 246.5, 252.5, 258.5, 264.5, 270.5] {
            let out = dab(&mut s, op, (x, 24.5), 8.0, 0.7, Some(&mut snapshot), None);
            painted.extend(out.painted().iter().copied());
        }
        assert!(painted.len() >= 2, "{painted:?}");
        let captured: BTreeSet<TileId> = snapshot.captured().collect();
        assert_eq!(captured, painted, "captured and painted are one set");
        assert_ne!(dump(&mut s, 300, 60), before);
        let mut history = PixelHistory::new();
        assert!(history.push(snapshot));
        assert!(matches!(history.undo(&mut s), Ok(true)));
        assert_eq!(dump(&mut s, 300, 60), before, "{op:?}");
    }
}

// -- Bounds and degenerate input --------------------------------------------------------------------------

#[test]
fn the_footprint_cap_refuses_a_big_dab_whole_and_accepts_one_just_under() {
    assert_eq!(MAX_TONE_DAB_TEXELS, 1 << 20);
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 600, 600, 30, 30);
    let before = dump(&mut s, 700, 700);
    let op = ToneOp::Burn(ToneRange::Midtones);
    // side = 2 ceil(r) + 3: r = 511 -> 1025^2 texels, over the cap.
    let out = dab(&mut s, op, (610.5, 610.5), 511.0, 1.0, None, None);
    assert!(out.painted().is_empty());
    assert!(out.is_complete());
    assert_eq!(dump(&mut s, 700, 700), before);
    // r = 510 -> 1023^2 texels, just under.
    let ok = dab(&mut s, op, (610.5, 610.5), 510.0, 1.0, None, None);
    assert!(ok.is_complete());
    assert!(!ok.painted().is_empty());
    assert_ne!(dump(&mut s, 700, 700), before);
}

#[test]
fn degenerate_input_changes_nothing_and_never_makes_nan() {
    for op in OPS {
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
            let out = dab(&mut s, op, c, r, k, None, None);
            assert!(out.painted().is_empty(), "{op:?} {c:?} {r} {k}");
            assert!(out.is_complete());
        }
        assert_eq!(dump(&mut s, 32, 32), before, "{op:?}");

        // An out-of-range amount is clamped to 1; an infinite one too.
        let run = |k: f32| {
            let (_d, mut t) = store();
            paint_pattern(&mut t, 0, 0, 40, 20);
            let _ = dab(&mut t, op, (10.5, 10.5), 4.0, k, None, None);
            dump(&mut t, 40, 20)
        };
        assert_eq!(run(7.0), run(1.0));
        assert_eq!(run(f32::INFINITY), run(1.0));

        // A stored NaN or infinity is left alone and never spread.
        let (_d, mut t) = store();
        paint_pattern(&mut t, 0, 0, 40, 20);
        let bad = [
            [f16::NAN, f16::INFINITY, f16::NEG_INFINITY, f16::ONE],
            [f16::INFINITY; CHANNELS],
            [f16::ZERO, f16::ZERO, f16::ZERO, f16::NAN],
        ];
        for (i, v) in bad.iter().enumerate() {
            put(&mut t, 10 + 2 * i as u32, 10, *v);
        }
        let before_bad: Vec<_> = (0..3)
            .map(|i| bits(texel(&mut t, 10 + 2 * i, 10)))
            .collect();
        let _ = dab(&mut t, op, (10.5, 10.5), 6.0, 1.0, None, None);
        for (i, want) in before_bad.iter().enumerate() {
            assert_eq!(
                &bits(texel(&mut t, 10 + 2 * i as u32, 10)),
                want,
                "{op:?} bad texel {i}"
            );
        }
        for y in 5..16_u32 {
            for x in 5..16_u32 {
                if [10, 12, 14].contains(&x) && y == 10 {
                    continue;
                }
                for v in texel(&mut t, x, y) {
                    assert!(v.is_finite(), "{op:?} ({x}, {y})");
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
    // Make the healthy tile hold something a dab would change.
    if let Ok(tile) = s.get_mut(surf, healthy) {
        for texel in tile.texels_mut().chunks_mut(CHANNELS) {
            if let [r, g, b, a] = texel {
                *r = f16::from_f32(0.25);
                *g = f16::from_f32(0.25);
                *b = f16::from_f32(0.25);
                *a = f16::ONE;
            }
        }
    }
    let healthy_before: Vec<u16> = match s.get(surf, healthy) {
        Ok(tile) => tile.texels().iter().map(|v| v.to_bits()).collect(),
        Err(err) => unreachable!("{err:?}"),
    };
    for op in OPS {
        // The footprint around (260, 10) reaches the broken tile 0.
        let out = tone_dab_covered(&mut s, surf, (260.5, 10.5), 6.0, op, 1.0, None, None);
        assert!(out.painted().is_empty(), "{op:?}");
        assert_eq!(out.failed().len(), 1, "{op:?}");
    }
    let healthy_after: Vec<u16> = match s.get(surf, healthy) {
        Ok(tile) => tile.texels().iter().map(|v| v.to_bits()).collect(),
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(healthy_after, healthy_before, "nothing was written");
}

#[test]
fn a_snapshot_for_another_surface_is_ignored_not_captured() {
    let (_dir, mut s) = store();
    paint_pattern(&mut s, 8, 8, 40, 40);
    let mut other = StrokeSnapshot::new(SurfaceId::from_raw(9));
    let out = dab(
        &mut s,
        ToneOp::Dodge(ToneRange::Midtones),
        (27.5, 27.5),
        8.0,
        1.0,
        Some(&mut other),
        None,
    );
    assert!(!out.painted().is_empty());
    assert_eq!(other.captured().count(), 0);
}

#[test]
fn the_sponge_modes_map_to_their_operations_and_desaturate_is_the_default() {
    assert_eq!(SpongeMode::default(), SpongeMode::Desaturate);
    assert_eq!(SpongeMode::Desaturate.op(), ToneOp::Desaturate);
    assert_eq!(SpongeMode::Saturate.op(), ToneOp::Saturate);
    assert_eq!(
        SpongeMode::ALL,
        [SpongeMode::Desaturate, SpongeMode::Saturate]
    );
    // The mode's operation does what its name says on a muted red.
    for (mode, grows) in [
        (SpongeMode::Desaturate, false),
        (SpongeMode::Saturate, true),
    ] {
        let (_dir, mut s) = store();
        put(&mut s, 5, 5, straight(0.6, 0.4, 0.4, 1.0));
        let _ = dab(&mut s, mode.op(), (5.5, 5.5), 4.0, 0.5, None, None);
        let t = texel(&mut s, 5, 5);
        assert_eq!(t[0].to_f32() - t[1].to_f32() > 0.2, grows, "{mode:?}");
    }
}
