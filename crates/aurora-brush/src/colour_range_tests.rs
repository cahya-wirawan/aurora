//! Tests of Colour Range ([`crate::colour_range`], 0.238.0): the ramp's
//! numbers, the Lab distance against known values and an independent `f64`
//! model, alpha, absent tiles, windows, caps and its read-only promise.
//! Nothing here depends on a platform's float rounding beyond one coverage
//! byte.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::unreadable_literal,
    clippy::excessive_precision
)]

use std::num::NonZeroUsize;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, TILE, TileId, TileStore};
use half::f16;

use crate::test_support::{store_with_a_broken_tile, surface};
use crate::{
    ColourRangeSpec, FillError, MAX_FILL_TILES, MAX_FUZZINESS, colour_distance, colour_range,
    coverage_byte,
};

const SIDE: usize = TILE as usize;

fn new_store() -> (tempfile::TempDir, TileStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = TileStore::new(dir.path().to_path_buf(), NonZeroUsize::new(64).unwrap()).unwrap();
    (dir, store)
}

/// Writes the premultiplied f16 `rgba` at `(x, y)`.
fn put(store: &mut TileStore, x: u32, y: u32, rgba: [f32; 4]) {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    let tile = store.get_mut(surface(), id).unwrap();
    let index = ((y % TILE) as usize * SIDE + (x % TILE) as usize) * CHANNELS;
    for (offset, v) in rgba.iter().enumerate() {
        tile.texels_mut()[index + offset] = f16::from_f32(*v);
    }
}

fn spec(colour: [f32; 3], fuzziness: u16, window: Rect) -> ColourRangeSpec {
    ColourRangeSpec {
        colour,
        fuzziness,
        window,
        max_tiles: MAX_FILL_TILES,
    }
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

// ---------------------------------------------------------------------
// The independent model: f64, powf everywhere, the textbook formulas.

fn model_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn model_lab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(model_linear);
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    [
        116.0 * f(y) - 16.0,
        500.0 * (f(x) - f(y)),
        200.0 * (f(y) - f(z)),
    ]
}

fn model_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (model_lab(a), model_lab(b));
    ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2)).sqrt()
}

/// The coverage byte the rule gives a premultiplied texel, in `f64`.
fn model_byte(texel: [f32; 4], sample: [f32; 3], fuzziness: u16) -> f64 {
    let alpha = f64::from(f16::from_f32(texel[3]).to_f32());
    if alpha <= 0.0 {
        return 0.0;
    }
    let straight = [0, 1, 2].map(|i| f64::from(f16::from_f32(texel[i]).to_f32()) / alpha);
    let d = model_distance(straight, sample.map(f64::from));
    let t = f64::from(fuzziness.min(MAX_FUZZINESS)) * 0.5;
    let cover = if t == 0.0 {
        f64::from(u8::from(d <= 0.15))
    } else {
        ((t - d) / (t / 2.0)).clamp(0.0, 1.0)
    };
    cover * alpha.clamp(0.0, 1.0) * 255.0
}

// ---------------------------------------------------------------------
// Distance.

#[test]
fn the_distance_matches_known_lab_values() {
    // White to black is exactly the lightness range.
    assert!((colour_distance([1.0; 3], [0.0; 3]) - 100.0).abs() < 0.01);
    assert!(colour_distance([0.3, 0.6, 0.9], [0.3, 0.6, 0.9]).abs() < 1e-4);
    // sRGB red is L* 53.24, a* 80.09, b* 67.20 (D65): its distance to black
    // (L* 0, a* b* 0) is the length of that vector.
    let red = (53.2408_f64.powi(2) + 80.0925_f64.powi(2) + 67.2032_f64.powi(2)).sqrt();
    assert!(
        (f64::from(colour_distance([1.0, 0.0, 0.0], [0.0; 3])) - red).abs() < 0.05,
        "{}",
        colour_distance([1.0, 0.0, 0.0], [0.0; 3])
    );
    // Symmetric.
    let (a, b) = ([0.2, 0.4, 0.8], [0.9, 0.1, 0.3]);
    assert!((colour_distance(a, b) - colour_distance(b, a)).abs() < 1e-4);
}

#[test]
fn the_distance_follows_the_model_on_random_colours() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 40) as f32) / 16_777_216.0
    };
    for _ in 0..300 {
        let a = [next(), next(), next()];
        let b = [next(), next(), next()];
        let want = model_distance(a.map(f64::from), b.map(f64::from));
        let got = f64::from(colour_distance(a, b));
        assert!((got - want).abs() < 0.02, "{a:?} {b:?}: {got} vs {want}");
    }
}

#[test]
fn a_non_finite_or_out_of_range_colour_is_clamped_not_a_panic() {
    let finite = colour_distance([f32::NAN, 2.0, -1.0], [0.0, 1.0, 0.0]);
    assert!(finite.is_finite());
    assert!(finite.abs() < 1e-3, "NaN reads as 0, 2 as 1, -1 as 0");
}

// ---------------------------------------------------------------------
// The ramp.

#[test]
fn the_ramp_is_full_to_half_the_threshold_then_linear_to_zero() {
    // Fuzziness 40 is threshold 20: full up to 10, zero from 20.
    assert_eq!(coverage_byte(0.0, 40, 1.0), 255);
    assert_eq!(coverage_byte(10.0, 40, 1.0), 255);
    assert_eq!(coverage_byte(12.5, 40, 1.0), 191); // 0.75 * 255 = 191.25
    assert_eq!(coverage_byte(15.0, 40, 1.0), 128); // 0.5 * 255 = 127.5
    assert_eq!(coverage_byte(17.5, 40, 1.0), 64); // 0.25 * 255 = 63.75
    assert_eq!(coverage_byte(20.0, 40, 1.0), 0);
    assert_eq!(coverage_byte(35.0, 40, 1.0), 0);
    // The ramp never rises with distance.
    let mut last = 255;
    for step in 0..=100 {
        let byte = coverage_byte(step as f32 * 0.3, 40, 1.0);
        assert!(byte <= last);
        last = byte;
    }
    // Another fuzziness scales the whole ramp (200 is threshold 100).
    assert_eq!(coverage_byte(50.0, 200, 1.0), 255);
    assert_eq!(coverage_byte(75.0, 200, 1.0), 128);
    assert_eq!(coverage_byte(100.0, 200, 1.0), 0);
}

#[test]
fn fuzziness_zero_is_an_exact_match_and_above_200_is_200() {
    assert_eq!(coverage_byte(0.0, 0, 1.0), 255);
    assert_eq!(coverage_byte(0.15, 0, 1.0), 255);
    assert_eq!(coverage_byte(0.16, 0, 1.0), 0);
    assert_eq!(coverage_byte(0.5, 0, 1.0), 0);
    assert_eq!(
        coverage_byte(60.0, 9_999, 1.0),
        coverage_byte(60.0, MAX_FUZZINESS, 1.0)
    );
    assert_eq!(coverage_byte(0.0, 9_999, 1.0), 255);
}

#[test]
fn alpha_scales_the_coverage_and_a_bad_alpha_covers_nothing() {
    assert_eq!(coverage_byte(0.0, 40, 0.5), 128);
    assert_eq!(coverage_byte(0.0, 40, 0.0), 0);
    assert_eq!(coverage_byte(0.0, 40, -1.0), 0);
    assert_eq!(coverage_byte(0.0, 40, 7.0), 255);
    assert_eq!(coverage_byte(0.0, 40, f32::NAN), 0);
}

// ---------------------------------------------------------------------
// Over a layer.

#[test]
fn it_matches_the_model_on_a_random_image_across_tile_edges() {
    let (w, h) = (300_u32, 200_u32);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let (_dir, mut store) = new_store();
    let mut image = vec![[0.0_f32; 4]; (w * h) as usize];
    // Blocks of a few base colours with jitter and varied alpha, so the ramp
    // is exercised at every distance.
    for y in 0..h {
        for x in 0..w {
            let cell = (x / 20 + y / 20) % 4;
            let base = [
                [0.8, 0.2, 0.2],
                [0.75, 0.3, 0.25],
                [0.2, 0.6, 0.3],
                [0.5, 0.5, 0.5],
            ][cell as usize];
            let jitter = |v: f32, k: u64| (v + (k as f32 - 8.0) * 0.004).clamp(0.0, 1.0);
            let alpha = match next(5) {
                0 => 0.0,
                1 => 0.5,
                _ => 1.0,
            };
            let straight = [
                jitter(base[0], next(16)),
                jitter(base[1], next(16)),
                jitter(base[2], next(16)),
            ];
            let texel = [
                straight[0] * alpha,
                straight[1] * alpha,
                straight[2] * alpha,
                alpha,
            ];
            image[(y * w + x) as usize] = texel;
            put(&mut store, x, y, texel);
        }
    }
    let mut checked = 0;
    for (sample, fuzziness) in [
        ([0.8, 0.2, 0.2], 40_u16),
        ([0.8, 0.2, 0.2], 10),
        ([0.2, 0.6, 0.3], 120),
        ([0.5, 0.5, 0.5], 0),
        ([0.1, 0.1, 0.9], 200),
    ] {
        let got = colour_range(
            &mut store,
            surface(),
            (w, h),
            &spec(sample, fuzziness, rect(0, 0, w, h)),
        )
        .unwrap();
        let mut bytes = vec![0_u8; (w * h) as usize];
        if let Some(mask) = &got {
            for row in 0..mask.bounds.height {
                for col in 0..mask.bounds.width {
                    let (x, y) = (mask.bounds.x as u32 + col, mask.bounds.y as u32 + row);
                    bytes[(y * w + x) as usize] =
                        mask.data[(row * mask.bounds.width + col) as usize];
                }
            }
        }
        for (i, texel) in image.iter().enumerate() {
            let want = model_byte(*texel, sample, fuzziness);
            let have = f64::from(bytes[i]);
            // One byte of rounding slack for the f32 conversions.
            assert!(
                (want - have).abs() <= 1.5,
                "texel {i} sample {sample:?} fuzziness {fuzziness}: {want} vs {have}"
            );
            checked += 1;
        }
        // The box is tight: its edge rows and columns each hold a non-zero.
        if let Some(mask) = &got {
            let bw = mask.bounds.width as usize;
            let bh = mask.bounds.height as usize;
            assert!(mask.data.iter().any(|b| *b != 0));
            assert!((0..bw).any(|c| mask.data[c] != 0), "top row");
            assert!(
                (0..bw).any(|c| mask.data[(bh - 1) * bw + c] != 0),
                "bottom row"
            );
            assert!((0..bh).any(|r| mask.data[r * bw] != 0), "left column");
            assert!(
                (0..bh).any(|r| mask.data[r * bw + bw - 1] != 0),
                "right column"
            );
        }
    }
    assert_eq!(checked, 5 * 60_000);
}

#[test]
fn the_sampled_colour_itself_is_fully_covered_at_any_fuzziness() {
    let (_dir, mut store) = new_store();
    let colour = [0.3_f32, 0.55, 0.8];
    for x in 0..4 {
        put(&mut store, x, 0, [colour[0], colour[1], colour[2], 1.0]);
    }
    for fuzziness in [0_u16, 1, 40, 200] {
        let mask = colour_range(
            &mut store,
            surface(),
            (4, 1),
            &spec(colour, fuzziness, rect(0, 0, 4, 1)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(mask.bounds, rect(0, 0, 4, 1));
        assert_eq!(mask.data, vec![255; 4], "fuzziness {fuzziness}");
    }
}

#[test]
fn a_half_transparent_pixel_of_the_right_colour_is_half_covered() {
    let (_dir, mut store) = new_store();
    // Straight (0.5, 0.25, 0.75) at alpha 0.5, premultiplied.
    put(&mut store, 0, 0, [0.25, 0.125, 0.375, 0.5]);
    put(&mut store, 1, 0, [0.0, 0.0, 0.0, 0.0]);
    let mask = colour_range(
        &mut store,
        surface(),
        (2, 1),
        &spec([0.5, 0.25, 0.75], 40, rect(0, 0, 2, 1)),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        mask.bounds,
        rect(0, 0, 1, 1),
        "the transparent texel is never covered"
    );
    assert_eq!(mask.data, vec![128]);
}

#[test]
fn absent_tiles_are_never_covered_and_are_not_created() {
    let (_dir, mut store) = new_store();
    put(&mut store, 5, 5, [0.9, 0.1, 0.1, 1.0]);
    let before: Vec<bool> = (0..4)
        .map(|i| store.contains_tile(surface(), TileId { x: i % 2, y: i / 2 }))
        .collect();
    let mask = colour_range(
        &mut store,
        surface(),
        (512, 512),
        // Even black would match a transparent texel by colour; it must not.
        &spec([0.0, 0.0, 0.0], 200, rect(0, 0, 512, 512)),
    )
    .unwrap();
    assert!(mask.is_none());
    let after: Vec<bool> = (0..4)
        .map(|i| store.contains_tile(surface(), TileId { x: i % 2, y: i / 2 }))
        .collect();
    assert_eq!(before, after);
    assert_eq!(before, vec![true, false, false, false]);
}

#[test]
fn it_reads_only_the_window_and_clips_it_to_the_layer() {
    let (_dir, mut store) = new_store();
    for y in 0..10 {
        for x in 0..10 {
            put(&mut store, x, y, [0.7, 0.2, 0.2, 1.0]);
        }
    }
    let run = |store: &mut TileStore, window| {
        colour_range(
            store,
            surface(),
            (10, 10),
            &spec([0.7, 0.2, 0.2], 40, window),
        )
        .unwrap()
    };
    assert_eq!(
        run(&mut store, rect(3, 4, 3, 2)).unwrap().bounds,
        rect(3, 4, 3, 2)
    );
    // A window hanging off the layer is clipped to it.
    assert_eq!(
        run(&mut store, rect(-5, -5, 8, 8)).unwrap().bounds,
        rect(0, 0, 3, 3)
    );
    assert_eq!(
        run(&mut store, rect(8, 8, 50, 50)).unwrap().bounds,
        rect(8, 8, 2, 2)
    );
    // Wholly off the layer, or empty: nothing.
    assert!(run(&mut store, rect(10, 0, 5, 5)).is_none());
    assert!(run(&mut store, rect(-9, 0, 9, 5)).is_none());
    assert!(run(&mut store, rect(0, 0, 0, 5)).is_none());
}

#[test]
fn a_window_over_the_cap_is_refused_before_any_read() {
    // The only tile is unreadable: reading it would be an `Err(Tile)`.
    let (_dir, mut store) = store_with_a_broken_tile(TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 });
    let over = ColourRangeSpec {
        colour: [0.5; 3],
        fuzziness: 40,
        window: rect(0, 0, 1024, 1024),
        max_tiles: 15,
    };
    assert!(matches!(
        colour_range(&mut store, surface(), (1024, 1024), &over),
        Err(FillError::TooLarge { max: 15 })
    ));
    // The cap is on the clipped window: 16 tiles at exactly the cap passes
    // the check (and then meets the broken tile).
    let at_cap = ColourRangeSpec {
        max_tiles: 16,
        ..over
    };
    assert!(matches!(
        colour_range(&mut store, surface(), (1024, 1024), &at_cap),
        Err(FillError::Tile(_))
    ));
}

#[test]
fn an_unreadable_tile_is_an_error_not_a_partial_mask() {
    let (_dir, mut store) = store_with_a_broken_tile(TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 });
    assert!(matches!(
        colour_range(
            &mut store,
            surface(),
            (512, 256),
            &spec([0.5; 3], 40, rect(0, 0, 512, 256)),
        ),
        Err(FillError::Tile(_))
    ));
}

#[test]
fn it_writes_nothing() {
    let (_dir, mut store) = new_store();
    for x in 0..40 {
        put(&mut store, x, 0, [0.6, 0.3, 0.1, 1.0]);
    }
    let snapshot = |store: &mut TileStore| {
        store
            .get(surface(), TileId { x: 0, y: 0 })
            .unwrap()
            .texels()
            .to_vec()
    };
    let before = snapshot(&mut store);
    let _ = colour_range(
        &mut store,
        surface(),
        (40, 1),
        &spec([0.6, 0.3, 0.1], 100, rect(0, 0, 40, 1)),
    )
    .unwrap();
    assert_eq!(before, snapshot(&mut store));
}

#[test]
fn repeated_pixels_take_the_cached_path_with_the_same_answer() {
    // A long flat run uses the previous-texel cache; alternating colours
    // defeat it. Both agree with the model.
    let (_dir, mut store) = new_store();
    for x in 0..64_u32 {
        let texel = if x < 32 || x % 2 == 0 {
            [0.8, 0.2, 0.2, 1.0]
        } else {
            [0.2, 0.2, 0.8, 1.0]
        };
        put(&mut store, x, 0, texel);
    }
    let mask = colour_range(
        &mut store,
        surface(),
        (64, 1),
        &spec([0.8, 0.2, 0.2], 40, rect(0, 0, 64, 1)),
    )
    .unwrap()
    .unwrap();
    for x in 0..64_usize {
        let covered = x < 32 || x % 2 == 0;
        let byte = if x < mask.bounds.width as usize {
            mask.data[x]
        } else {
            0
        };
        assert_eq!(byte == 255, covered, "x {x}");
        assert!(covered || byte == 0, "x {x}");
    }
}

#[test]
fn a_bigger_fuzziness_never_covers_less() {
    let (_dir, mut store) = new_store();
    for x in 0..50_u32 {
        let v = x as f32 / 50.0;
        put(&mut store, x, 0, [v, 0.4, 0.4, 1.0]);
    }
    let bytes = |store: &mut TileStore, fuzziness| {
        let m = colour_range(
            store,
            surface(),
            (50, 1),
            &spec([0.5, 0.4, 0.4], fuzziness, rect(0, 0, 50, 1)),
        )
        .unwrap();
        let mut out = vec![0_u8; 50];
        if let Some(m) = m {
            for (i, b) in m.data.iter().enumerate() {
                out[m.bounds.x as usize + i] = *b;
            }
        }
        out
    };
    let mut previous = vec![0_u8; 50];
    for fuzziness in [0_u16, 5, 20, 40, 80, 160, 200] {
        let now = bytes(&mut store, fuzziness);
        for (a, b) in previous.iter().zip(&now) {
            assert!(a <= b, "fuzziness {fuzziness}");
        }
        previous = now;
    }
}

#[test]
fn texels_past_the_layers_edge_are_never_read_even_if_the_tile_holds_them() {
    let (_dir, mut store) = new_store();
    // A 10 x 10 layer whose tile also holds red at (20, 20), outside it.
    for y in 0..10 {
        for x in 0..10 {
            put(&mut store, x, y, [0.7, 0.2, 0.2, 1.0]);
        }
    }
    put(&mut store, 20, 20, [0.7, 0.2, 0.2, 1.0]);
    put(&mut store, 20, 5, [0.7, 0.2, 0.2, 1.0]);
    put(&mut store, 5, 20, [0.7, 0.2, 0.2, 1.0]);
    let mask = colour_range(
        &mut store,
        surface(),
        (10, 10),
        &spec([0.7, 0.2, 0.2], 40, rect(0, 0, 100, 100)),
    )
    .unwrap()
    .unwrap();
    assert_eq!(mask.bounds, rect(0, 0, 10, 10));
}

/// Costs (run with `--ignored --nocapture`); asserts nothing about time.
#[test]
#[ignore = "a measurement, not a check"]
fn measure_colour_range_and_quick_select_costs() {
    let (w, h) = (4096_u32, 4096_u32);
    let (_dir, mut store) = new_store();
    let mut state = 1_u64;
    for ty in 0..16_u32 {
        for tx in 0..16_u32 {
            let tile = store.get_mut(surface(), TileId { x: tx, y: ty }).unwrap();
            for (i, texel) in tile.texels_mut().chunks_exact_mut(4).enumerate() {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let noise = ((state >> 40) % 64) as f32 / 255.0;
                let base = 0.3 + 0.3 * ((i % 256) as f32 / 256.0);
                texel[0] = f16::from_f32(base + noise);
                texel[1] = f16::from_f32(0.4 + noise);
                texel[2] = f16::from_f32(0.5);
                texel[3] = f16::from_f32(1.0);
            }
        }
    }
    let start = std::time::Instant::now();
    let m = colour_range(
        &mut store,
        surface(),
        (w, h),
        &spec([0.45, 0.45, 0.5], 40, rect(0, 0, w, h)),
    )
    .unwrap();
    println!(
        "colour range, noisy 16 Mpx: {:?} ({} bytes of mask)",
        start.elapsed(),
        m.map_or(0, |m| m.data.len())
    );
    for radius in [24.0_f32, 100.0, 256.0] {
        let mut grown = crate::QuickRegion::new(MAX_FILL_TILES);
        let start = std::time::Instant::now();
        let st = crate::QuickStamp {
            centre: (2000.0, 2000.0),
            radius,
            tolerance: 32,
        };
        let grew =
            crate::quick_select_stamp(&mut store, surface(), (w, h), &st, &mut grown).unwrap();
        println!(
            "quick select stamp r={radius}, noisy: {:?} (grew {grew})",
            start.elapsed()
        );
        let start = std::time::Instant::now();
        let r = grown.to_region().unwrap();
        println!(
            "  to_region: {:?} ({} texels box)",
            start.elapsed(),
            r.map_or(0, |r| r.data.len())
        );
    }
}
