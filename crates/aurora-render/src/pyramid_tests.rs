//! `downsample_quad` against an independent `f64` box filter of a whole
//! image (not of tiles), so a mistake shared by the tile walk and the
//! reference cannot hide.

#![allow(clippy::many_single_char_names)]

use super::downsample_quad;
use aurora_tile::{CHANNELS, SAMPLES, TILE};
use half::f16;

const T: usize = TILE as usize;

/// A tiny deterministic generator (no `rand` dependency here).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }
}

/// A straight-alpha image `w` x `h` texels (row major, RGBA).
struct Image {
    w: usize,
    h: usize,
    texels: Vec<[f32; 4]>,
}

impl Image {
    fn at(&self, x: usize, y: usize) -> [f32; 4] {
        if x >= self.w || y >= self.h {
            return [0.0; 4];
        }
        self.texels.get(y * self.w + x).copied().unwrap_or([0.0; 4])
    }
}

/// Random content, with transparent, opaque and translucent texels, every
/// value already representable in `f16` (so the reference sees exactly the
/// tile's input).
fn random_image(w: usize, h: usize, seed: u64) -> Image {
    let mut rng = Lcg(seed);
    let q = |v: f32| f16::from_f32(v).to_f32();
    let texels = (0..w * h)
        .map(|_| {
            let kind = rng.next();
            let a = if kind < 0.15 {
                0.0
            } else if kind < 0.5 {
                1.0
            } else {
                0.05 + 0.95 * rng.next()
            };
            [q(rng.next()), q(rng.next()), q(rng.next()), q(a)]
        })
        .collect();
    Image { w, h, texels }
}

/// The tile `(tx, ty)` of `image`; texels past the image carry `outside`
/// (content that must never be seen).
fn tile_of(image: &Image, tx: usize, ty: usize, outside: [f32; 4]) -> Vec<f16> {
    let mut out = Vec::with_capacity(SAMPLES);
    for y in 0..T {
        for x in 0..T {
            let (gx, gy) = (tx * T + x, ty * T + y);
            let texel = if gx < image.w && gy < image.h {
                image.at(gx, gy)
            } else {
                outside
            };
            out.extend(texel.map(f16::from_f32));
        }
    }
    out
}

/// The `f64` reference: level `level` texel `(x, y)` of `image`, the mean
/// of the premultiplied `2^level` square block with the outside counted as
/// transparent, returned straight.
fn reference(image: &Image, level: u32, x: usize, y: usize) -> [f64; 4] {
    let n = 1_usize << level;
    let mut sum = [0.0_f64; 4];
    for dy in 0..n {
        for dx in 0..n {
            let [r, g, b, a] = image.at(x * n + dx, y * n + dy).map(f64::from);
            if !(r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite()) {
                continue;
            }
            sum[0] += r * a;
            sum[1] += g * a;
            sum[2] += b * a;
            sum[3] += a;
        }
    }
    if sum[3] > 0.0 {
        [
            sum[0] / sum[3],
            sum[1] / sum[3],
            sum[2] / sum[3],
            sum[3] / (n * n) as f64,
        ]
    } else {
        [0.0; 4]
    }
}

/// Builds pyramid level `level` of `image` tile by tile with
/// `downsample_quad`, exactly as the app does: each level from the one
/// below, `valid` from the document size in the input level's texels.
fn build_level(image: &Image, level: u32, outside: [f32; 4]) -> Vec<Vec<Vec<f16>>> {
    let tiles = |extent: usize, l: u32| extent.div_ceil(T << l).max(1);
    let mut grid: Vec<Vec<Vec<f16>>> = (0..tiles(image.h, 0))
        .map(|ty| {
            (0..tiles(image.w, 0))
                .map(|tx| tile_of(image, tx, ty, outside))
                .collect()
        })
        .collect();
    for l in 1..=level {
        let child_extent = |extent: usize| extent.div_ceil(1 << (l - 1));
        let (cw, ch) = (child_extent(image.w), child_extent(image.h));
        let next: Vec<Vec<Vec<f16>>> = (0..tiles(image.h, l))
            .map(|ty| {
                (0..tiles(image.w, l))
                    .map(|tx| {
                        let child = |dx: usize, dy: usize| {
                            grid.get(2 * ty + dy)
                                .and_then(|row| row.get(2 * tx + dx))
                                .map(Vec::as_slice)
                        };
                        let valid = (
                            u32::try_from(cw.saturating_sub(2 * tx * T)).unwrap_or(u32::MAX),
                            u32::try_from(ch.saturating_sub(2 * ty * T)).unwrap_or(u32::MAX),
                        );
                        let mut out = vec![f16::ZERO; SAMPLES];
                        downsample_quad(
                            [child(0, 0), child(1, 0), child(0, 1), child(1, 1)],
                            valid,
                            &mut out,
                        );
                        out
                    })
                    .collect()
            })
            .collect();
        grid = next;
    }
    grid
}

fn texel(grid: &[Vec<Vec<f16>>], x: usize, y: usize) -> [f32; 4] {
    let tile = grid.get(y / T).and_then(|row| row.get(x / T));
    let start = ((y % T) * T + x % T) * CHANNELS;
    let mut out = [0.0_f32; 4];
    if let Some(samples) = tile.and_then(|t| t.get(start..start + CHANNELS)) {
        for (o, s) in out.iter_mut().zip(samples) {
            *o = s.to_f32();
        }
    }
    out
}

/// One `f16` step at `v` (the spacing of `f16` values around it).
fn ulp(v: f64) -> f64 {
    let v = v.abs().max(f64::from(f16::MIN_POSITIVE.to_f32()));
    2.0_f64.powi(v.log2().floor() as i32 - 10)
}

fn assert_matches_reference(image: &Image, level: u32, tolerance_ulps: f64, outside: [f32; 4]) {
    let grid = build_level(image, level, outside);
    let n = 1_usize << level;
    let (w, h) = (image.w.div_ceil(n), image.h.div_ceil(n));
    let mut worst = 0.0_f64;
    for y in 0..h {
        for x in 0..w {
            let got = texel(&grid, x, y);
            let want = reference(image, level, x, y);
            for (channel, (g, r)) in got.iter().zip(want).enumerate() {
                // A straight colour under zero alpha carries no
                // information; both sides store 0 there.
                let error = (f64::from(*g) - r).abs();
                let allowed = tolerance_ulps * ulp(r).max(ulp(1.0) / 4.0);
                worst = worst.max(error / allowed);
                assert!(
                    error <= allowed,
                    "level {level} texel ({x}, {y}) channel {channel}: got {g}, \
                     reference {r}, error {error:e} > {allowed:e}"
                );
            }
        }
    }
    assert!(worst.is_finite());
}

#[test]
fn one_level_matches_the_f64_box_filter_within_one_f16_step() {
    // 600 x 520: neither a multiple of the tile nor of 2 on either axis,
    // and three tiles across, so blocks straddle tiles and the border.
    let image = random_image(601, 519, 7);
    assert_matches_reference(&image, 1, 1.0, [0.9, 0.1, 0.4, 1.0]);
}

#[test]
fn three_levels_match_the_f64_box_filter_of_the_whole_image() {
    let image = random_image(1100, 777, 11);
    assert_matches_reference(&image, 2, 4.0, [0.9, 0.1, 0.4, 1.0]);
    assert_matches_reference(&image, 3, 6.0, [0.9, 0.1, 0.4, 1.0]);
}

#[test]
fn content_past_the_document_border_never_reaches_the_pyramid() {
    // The same image with two different outside fills must build the same
    // level bit for bit.
    let image = random_image(300, 270, 3);
    let a = build_level(&image, 2, [1.0, 0.0, 0.0, 1.0]);
    let b = build_level(&image, 2, [0.0, 1.0, 1.0, 0.5]);
    assert_eq!(a, b);
}

#[test]
fn filtering_is_premultiplied_not_straight() {
    // Opaque white beside transparent red: premultiplied, the mean is
    // white at alpha 0.5; a straight mean would be pink.
    let mut child = vec![f16::ZERO; SAMPLES];
    for (i, texel) in child.chunks_exact_mut(CHANNELS).enumerate() {
        let colour = if i % 2 == 0 {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [1.0, 0.0, 0.0, 0.0]
        };
        for (s, v) in texel.iter_mut().zip(colour) {
            *s = f16::from_f32(v);
        }
    }
    let mut out = vec![f16::ZERO; SAMPLES];
    downsample_quad([Some(&child), None, None, None], (512, 512), &mut out);
    let first: Vec<f32> = out.iter().take(4).map(|v| v.to_f32()).collect();
    assert_eq!(first, vec![1.0, 1.0, 1.0, 0.5]);
}

#[test]
fn a_non_finite_texel_counts_as_transparent() {
    let mut child = vec![f16::ONE; SAMPLES];
    if let Some(r) = child.get_mut(0) {
        *r = f16::NAN;
    }
    if let Some(g) = child.get_mut(CHANNELS + 1) {
        *g = f16::INFINITY;
    }
    let mut out = vec![f16::ZERO; SAMPLES];
    downsample_quad([Some(&child), None, None, None], (512, 512), &mut out);
    let first: Vec<f32> = out.iter().take(4).map(|v| v.to_f32()).collect();
    // Two of the four texels count: opaque white at half coverage.
    assert_eq!(first, vec![1.0, 1.0, 1.0, 0.5]);
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn absent_children_are_transparent_and_the_output_is_written_in_full() {
    let mut out = vec![f16::ONE; SAMPLES];
    downsample_quad([None, None, None, None], (512, 512), &mut out);
    assert!(out.iter().all(|v| v.to_bits() == 0));
}

#[test]
fn each_quadrant_lands_in_its_own_quarter_of_the_output() {
    let solid = |v: f32| vec![f16::from_f32(v); SAMPLES];
    let (a, b, c, d) = (solid(0.25), solid(0.5), solid(0.75), solid(1.0));
    let mut out = vec![f16::ZERO; SAMPLES];
    downsample_quad(
        [Some(&a), Some(&b), Some(&c), Some(&d)],
        (512, 512),
        &mut out,
    );
    let at = |x: usize, y: usize| out.get((y * T + x) * CHANNELS).map(|v| v.to_f32());
    assert_eq!(at(0, 0), Some(0.25));
    assert_eq!(at(T - 1, 0), Some(0.5));
    assert_eq!(at(0, T - 1), Some(0.75));
    assert_eq!(at(T - 1, T - 1), Some(1.0));
    assert_eq!(at(T / 2 - 1, T / 2 - 1), Some(0.25));
    assert_eq!(at(T / 2, T / 2), Some(1.0));
}

#[test]
fn the_same_input_gives_the_same_bits_every_time() {
    let image = random_image(512, 512, 99);
    let first = build_level(&image, 1, [0.0; 4]);
    let second = build_level(&image, 1, [0.0; 4]);
    assert_eq!(first, second);
}

#[test]
fn a_wrong_sized_output_is_left_untouched() {
    let mut out = vec![f16::ONE; 8];
    downsample_quad([None, None, None, None], (512, 512), &mut out);
    assert!(out.iter().all(|v| *v == f16::ONE));
}
